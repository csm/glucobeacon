//! Gateway firmware for the Heltec WiFi LoRa 32 V3.
//!
//! Joins WiFi, polls Dexcom Share, and relays each new reading over the SX1262.
//! Holds no alarm state: the display node decides what to beep about, so a
//! gateway that falls over reads as stale data rather than as silence.
//!
//! Thin by design. The poll cadence, the Share protocol, and the framing all
//! live in the workspace and are tested on a host; what is here is WiFi, an
//! HTTP transport, and a loop.
//!
//! Not yet built against hardware; see `firmware/README.md`.

mod esp_http;
mod radio_link;
mod wifi;

use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::Delay;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver, SpiDriverConfig, config::Config as SpiConfig};
use esp_idf_svc::hal::units::FromValueType;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sntp::EspSntp;

use glucobeacon_core::Timestamp;
use glucobeacon_dexcom::{Credentials, Region as DexcomRegion, ShareClient};
use glucobeacon_gateway::config::AlarmConfig;
use glucobeacon_gateway::schedule::PollSchedule;
use glucobeacon_gateway::source::{Source, uplink_state};
use glucobeacon_proto::link::SeqCounter;
use glucobeacon_proto::radio::{Bandwidth, RadioConfig, Region as RadioRegion, SpreadingFactor};
use glucobeacon_proto::{Link, Message, Packet, ReadingReport, Status};

use crate::esp_http::EspTransport;

/// Compiled-in configuration.
///
/// The gateway has no filesystem and no console to configure it from, so this
/// is set at build time. Secrets come from the environment at compile time
/// rather than being committed — see `firmware/README.md`.
mod settings {
    use super::*;

    /// WiFi SSID.
    pub const WIFI_SSID: &str = env!("GLUCOBEACON_WIFI_SSID");
    /// WiFi password.
    pub const WIFI_PASSWORD: &str = env!("GLUCOBEACON_WIFI_PASSWORD");
    /// Dexcom Share follower account.
    pub const DEXCOM_ACCOUNT: &str = env!("GLUCOBEACON_DEXCOM_ACCOUNT");
    /// Dexcom Share password.
    pub const DEXCOM_PASSWORD: &str = env!("GLUCOBEACON_DEXCOM_PASSWORD");

    /// Which Share deployment the account lives on. Unrelated to the radio
    /// region: a Dexcom account's server and a radio's band are separate
    /// facts that happen to share a word.
    pub const DEXCOM_REGION: DexcomRegion = DexcomRegion::Us;

    /// The radio settings. These must match
    /// `glucobeacon-fw-display`'s `board::RADIO` field for field — a mismatch
    /// in any one of them is not a weak link but a silent one.
    ///
    /// SF9 at 125 kHz on the 915 MHz board. See that file for the dwell-time
    /// constraint that comes with the narrow bandwidth in the US.
    pub const RADIO: RadioConfig = RadioConfig {
        spreading_factor: SpreadingFactor::Sf9,
        bandwidth: Bandwidth::Bw125,
        ..RadioConfig::preset(RadioRegion::Us915)
    };

    /// The largest frame the gateway may transmit, matching the display node.
    pub const MAX_FRAME_FOR_DWELL: usize = 48;

    const _: () = {
        assert!(RADIO.check_airtime(MAX_FRAME_FOR_DWELL).is_ok());
    };
}

fn main() -> Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().context("taking peripherals")?;
    let event_loop = EspSystemEventLoop::take().context("system event loop")?;
    let nvs = EspDefaultNvsPartition::take().context("nvs partition")?;

    let _wifi = wifi::connect(
        peripherals.modem,
        event_loop,
        nvs,
        settings::WIFI_SSID,
        settings::WIFI_PASSWORD,
    )
    .context("joining wifi")?;

    // The gateway is the only end with a real clock, and every packet it sends
    // carries the time — the display node has no RTC and learns it from these.
    // So NTP is not a nicety here: without it the display cannot tell how old
    // a reading is.
    let _sntp = EspSntp::new_default().context("starting sntp")?;
    wait_for_clock()?;

    // --- Radio on SPI2, per the V3 pinout. ---
    let spi = SpiDriver::new(
        peripherals.spi2,
        peripherals.pins.gpio9,  // SCK
        peripherals.pins.gpio10, // MOSI
        Some(peripherals.pins.gpio11),
        &SpiDriverConfig::new(),
    )
    .context("spi driver")?;

    let spi_device = SpiDeviceDriver::new(
        spi,
        Option::<esp_idf_svc::hal::gpio::AnyIOPin>::None,
        &SpiConfig::new().baudrate(10.MHz().into()),
    )
    .context("spi device")?;

    let mut cs = PinDriver::output(peripherals.pins.gpio8)?;
    cs.set_high()?;
    let bus = ExclusiveDevice::new(spi_device, cs, Delay::new_default())
        .map_err(|_| anyhow::anyhow!("spi bus"))?;

    let mut busy = PinDriver::input(peripherals.pins.gpio13)?;
    busy.set_pull(Pull::Floating)?;
    let mut dio1 = PinDriver::input(peripherals.pins.gpio14)?;
    dio1.set_pull(Pull::Floating)?;

    let mut link = radio_link::LoraLink::new(
        bus,
        PinDriver::output(peripherals.pins.gpio12)?,
        busy,
        dio1,
        settings::RADIO,
        Delay::new_default(),
    )
    .map_err(|e| anyhow::anyhow!("radio init: {e:?}"))?;

    let transport = EspTransport::new().context("http transport")?;
    let client = ShareClient::with_transport(
        transport,
        Credentials::new(settings::DEXCOM_ACCOUNT, settings::DEXCOM_PASSWORD),
        settings::DEXCOM_REGION,
    );

    run(Source::Share(Box::new(client)), &mut link)
}

/// The relay loop.
fn run<L: Link>(mut source: Source<EspTransport>, link: &mut L) -> Result<()>
where
    L::Error: core::fmt::Debug,
{
    let poll_config = glucobeacon_gateway::config::PollConfig::default();
    let schedule = PollSchedule::new(poll_config);
    let policy = AlarmConfig::default().to_policy();
    let mut seq = SeqCounter::new();

    // Push the thresholds up front so the display is never running on stale
    // ones, and again periodically in case a packet was lost.
    send(link, Packet::new(seq.take(), Message::Policy(policy)));

    let mut last_sent: Option<Timestamp> = None;
    let mut consecutive_failures: u32 = 0;
    let delay = Delay::new_default();

    loop {
        let now = wall_clock();
        let result = futures_lite::future::block_on(source.poll(now, last_sent));
        let uplink = uplink_state(&result);

        match &result {
            Ok(readings) => {
                consecutive_failures = 0;
                for reading in readings {
                    log::info!(
                        "relaying {} mg/dL {}",
                        reading.glucose.mgdl(),
                        reading.trend.arrow()
                    );
                    send(
                        link,
                        Packet::new(
                            seq.take(),
                            Message::Reading(ReadingReport {
                                reading: *reading,
                                sent_at: now,
                            }),
                        ),
                    );
                    last_sent = Some(reading.at);
                }
            }
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                if error.is_retryable() {
                    log::warn!("poll failed ({consecutive_failures}): {error}");
                } else {
                    // Retrying cannot fix this, but going quiet is worse: the
                    // display needs to keep hearing that something is wrong.
                    log::error!("poll failed and needs attention: {error}");
                }
            }
        }

        send(
            link,
            Packet::new(
                seq.take(),
                Message::Status(Status {
                    sent_at: now,
                    uplink,
                    consecutive_failures: consecutive_failures.try_into().unwrap_or(u16::MAX),
                    battery_millivolts: None,
                }),
            ),
        );

        let wait = schedule.next_delay(wall_clock(), last_sent, consecutive_failures);
        log::debug!("next poll in {}s", wait.as_secs());
        delay.delay_ms(wait.as_millis().min(u128::from(u32::MAX)) as u32);
    }
}

/// Transmits a packet, logging rather than failing on a radio error.
///
/// A failed transmission costs five minutes of staleness on the display, which
/// the display already knows how to show. Taking the gateway down over it would
/// cost more.
fn send<L: Link>(link: &mut L, packet: Packet)
where
    L::Error: core::fmt::Debug,
{
    if let Err(error) = link.send(&packet) {
        log::warn!("transmit failed for seq {}: {error:?}", packet.seq);
    }
}

/// Blocks until SNTP has set the clock.
fn wait_for_clock() -> Result<()> {
    let delay = Delay::new_default();
    for _ in 0..60 {
        if wall_clock().as_unix_secs() > 1_700_000_000 {
            log::info!("clock synchronised");
            return Ok(());
        }
        delay.delay_ms(1_000);
    }
    // Without a clock, every timestamp the display receives is wrong, and it
    // has no other source. Better to fail loudly at boot.
    bail!("SNTP did not set the clock within a minute")
}

fn wall_clock() -> Timestamp {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(StdDuration::ZERO)
        .as_secs() as i64;
    Timestamp::from_unix_secs(secs)
}
