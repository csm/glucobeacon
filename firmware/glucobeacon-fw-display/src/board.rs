//! Heltec WiFi LoRa 32 V3 board facts, per wiring diagram v1.0.
//!
//! Everything board-specific belongs in exactly one place, because the failure
//! mode for a wrong pin is a peripheral that initializes cleanly and then never
//! works.
//!
//! # Departures from wiring diagram v1.0
//!
//! The diagram's e-paper pins land on GPIO 11, 12 and 13, and its notes list
//! GPIO 8, 9 and 14 as free. On this board they are not free: the on-board
//! SX1262 is hard-wired to GPIO 8–14, and it is not on a header, so those pins
//! cannot be reassigned. The e-paper bus has been moved to genuinely free pins
//! below. Two smaller changes go with it:
//!
//! - The diagram's e-paper DC (17) and CS (18) are the on-board OLED's I2C
//!   pins. Usable if the OLED is given up, but the OLED is the only display
//!   available until the e-paper is working, so they have been moved too.
//! - The diagram lists GPIO 22 and 23 as reserve. Neither exists on an
//!   ESP32-S3.
//!
//! The e-paper MISO in the diagram is also unnecessary: the Waveshare 7.5"
//! panel is write-only, and its HAT header has no data-out line.
//!
//! **All of this still needs checking against the board in hand.** These are
//! reference-design numbers, not measured ones.

use glucobeacon_proto::radio::{Bandwidth, RadioConfig, Region, SpreadingFactor};

/// Which band to transmit in.
///
/// The board is the 915 MHz variant, so US915. The module itself covers
/// 863–928 MHz and will happily transmit outside the band that is legal where
/// you are, which is why this is checked rather than assumed.
pub const REGION: Region = Region::Us915;

/// The radio settings both ends must share.
///
/// SF9 at 125 kHz: the narrowest channel, and therefore the most sensitive,
/// which is what gives the link its range. See [`MAX_FRAME_FOR_DWELL`] for the
/// constraint that comes with it in the US.
pub const RADIO: RadioConfig = RadioConfig {
    spreading_factor: SpreadingFactor::Sf9,
    bandwidth: Bandwidth::Bw125,
    ..RadioConfig::preset(REGION)
};

/// The largest frame this configuration may legally transmit.
///
/// Under FCC 15.247's hopping route the cap is 400 ms per transmission. At
/// SF9/125 kHz that works out at 66 bytes; a 25-byte reading is about 206 ms,
/// and a maximum-size protocol frame is over a second. 48 leaves real margin
/// over everything the node actually sends without sitting on the legal limit.
///
/// The test below ties this to the protocol rather than to a guess: if a
/// message variant ever grows past it, that fails rather than going on the air.
pub const MAX_FRAME_FOR_DWELL: usize = 48;

const _: () = {
    assert!(RADIO.check_airtime(MAX_FRAME_FOR_DWELL).is_ok());
};

/// SX1262 pins.
///
/// Fixed by the board — the radio is soldered to these and there is no way to
/// move it.
pub mod lora {
    /// SPI clock.
    pub const SCK: u8 = 9;
    /// SPI controller out, peripheral in.
    pub const MOSI: u8 = 10;
    /// SPI controller in, peripheral out.
    pub const MISO: u8 = 11;
    /// Chip select.
    pub const NSS: u8 = 8;
    /// Radio reset, active low.
    pub const RESET: u8 = 12;
    /// Busy line. The SX1262 holds this high while it is working; every
    /// command has to wait for it to fall.
    pub const BUSY: u8 = 13;
    /// Interrupt line, used for transmit-done and receive-done.
    pub const DIO1: u8 = 14;
}

/// The on-board 0.96" SSD1306 OLED.
///
/// Not the product display, but the only one available until the e-paper is
/// working, which is why its pins have been kept clear.
pub mod oled {
    /// I2C data.
    pub const SDA: u8 = 17;
    /// I2C clock.
    pub const SCL: u8 = 18;
    /// Display reset.
    pub const RESET: u8 = 21;
    /// Width in pixels.
    pub const WIDTH: u32 = 128;
    /// Height in pixels.
    pub const HEIGHT: u32 = 64;
}

/// Waveshare 7.5" e-Paper HAT (800×480, black/white), on free header pins.
///
/// Write-only: the panel has no data-out line, so there is no MISO here.
pub mod epaper {
    /// SPI clock. Diagram says 12; that is the SX1262's reset.
    pub const SCK: u8 = 3;
    /// SPI data in to the panel. Diagram says 11; that is the SX1262's MISO.
    pub const MOSI: u8 = 2;
    /// Chip select. Diagram says 18; that is the OLED's I2C clock.
    pub const CS: u8 = 7;
    /// Data/command select. Diagram says 17; that is the OLED's I2C data.
    pub const DC: u8 = 6;
    /// Panel reset, active low. As per the diagram.
    pub const RESET: u8 = 5;
    /// Busy, high while a refresh is in progress. As per the diagram.
    pub const BUSY: u8 = 4;
}

/// Controls power to the OLED and the header's Vext rail. Active low.
pub const VEXT_CONTROL: u8 = 36;

/// The white LED on the board.
pub const BOARD_LED: u8 = 35;

/// Battery voltage divider input.
pub const VBAT_ADC: u8 = 1;

/// Enables the battery voltage divider. Active low.
pub const VBAT_ENABLE: u8 = 37;

/// The piezo buzzer, through MOSFET module #1. Active high, per the diagram.
///
/// An SFM-27-W is an *active* buzzer — it generates its own tone — so this is
/// a plain on/off output rather than a PWM channel.
pub const BUZZER: u8 = 15;

/// The LED ring in the arcade button, through MOSFET module #2. Active high.
pub const BUTTON_LED: u8 = 16;

/// The acknowledge button. Active low, with an internal pull-up.
///
/// # This is a strapping pin
///
/// GPIO0 is what the ESP32-S3 samples at reset to decide whether to enter the
/// serial bootloader. A button held down while the board resets — a brown-out,
/// a battery swap, a child leaning on it — leaves the node sitting in download
/// mode, dark and silent, until someone power-cycles it without touching the
/// button.
///
/// It is convenient because it parallels the on-board PRG button, which makes
/// bring-up easy. For a device whose job is to make noise when someone is low,
/// consider moving it to [`ALTERNATE_ACK_BUTTON`] before this leaves the bench.
pub const ACK_BUTTON: u8 = 0;

/// A non-strapping alternative for the acknowledge button.
pub const ALTERNATE_ACK_BUTTON: u8 = 47;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pin this firmware drives or reads, with a name for the failure
    /// message.
    const ASSIGNED: [(&str, u8); 22] = [
        ("lora.sck", lora::SCK),
        ("lora.mosi", lora::MOSI),
        ("lora.miso", lora::MISO),
        ("lora.nss", lora::NSS),
        ("lora.reset", lora::RESET),
        ("lora.busy", lora::BUSY),
        ("lora.dio1", lora::DIO1),
        ("oled.sda", oled::SDA),
        ("oled.scl", oled::SCL),
        ("oled.reset", oled::RESET),
        ("epaper.sck", epaper::SCK),
        ("epaper.mosi", epaper::MOSI),
        ("epaper.cs", epaper::CS),
        ("epaper.dc", epaper::DC),
        ("epaper.reset", epaper::RESET),
        ("epaper.busy", epaper::BUSY),
        ("vext", VEXT_CONTROL),
        ("board_led", BOARD_LED),
        ("vbat_adc", VBAT_ADC),
        ("vbat_enable", VBAT_ENABLE),
        ("buzzer", BUZZER),
        ("button_led", BUTTON_LED),
    ];

    #[test]
    fn no_two_peripherals_share_a_pin() {
        // The one mistake in this file a compiler cannot catch, and the one
        // that cost this revision an e-paper bus on top of the radio.
        for (i, (name_a, pin_a)) in ASSIGNED.iter().enumerate() {
            for (name_b, pin_b) in &ASSIGNED[i + 1..] {
                assert_ne!(pin_a, pin_b, "{name_a} and {name_b} are both GPIO{pin_a}");
            }
        }
    }

    #[test]
    fn the_epaper_bus_stays_clear_of_the_radio() {
        // The SX1262 is soldered to GPIO8-14 and cannot be moved, so anything
        // that lands there takes the radio down with it.
        let radio = [
            lora::NSS,
            lora::SCK,
            lora::MOSI,
            lora::MISO,
            lora::RESET,
            lora::BUSY,
            lora::DIO1,
        ];
        for (name, pin) in [
            ("sck", epaper::SCK),
            ("mosi", epaper::MOSI),
            ("cs", epaper::CS),
            ("dc", epaper::DC),
            ("reset", epaper::RESET),
            ("busy", epaper::BUSY),
        ] {
            assert!(
                !radio.contains(&pin),
                "epaper.{name} is GPIO{pin}, which belongs to the SX1262"
            );
        }
    }

    #[test]
    fn every_pin_exists_on_an_esp32_s3() {
        // GPIO0-48, with 22-25 absent and 26-32 taken by the SiP flash.
        for (name, pin) in ASSIGNED {
            assert!(pin <= 48, "{name} is GPIO{pin}, which does not exist");
            assert!(
                !(22..=25).contains(&pin),
                "{name} is GPIO{pin}, absent on S3"
            );
            assert!(
                !(26..=32).contains(&pin),
                "{name} is GPIO{pin}, used by flash"
            );
        }
    }

    #[test]
    fn the_radio_config_is_legal_for_its_region() {
        assert_eq!(RADIO.validate(), Ok(()));
        assert_eq!(REGION, Region::Us915, "this is the 915 MHz board");
    }

    #[test]
    fn everything_the_node_sends_fits_the_dwell_limit() {
        assert_eq!(RADIO.check_airtime(MAX_FRAME_FOR_DWELL), Ok(()));

        // The cap is real: a maximum-size protocol frame is over a second, so
        // MAX_FRAME_FOR_DWELL exists rather than trusting the protocol maximum.
        let ceiling = RADIO
            .max_payload_within_dwell()
            .expect("US915 caps dwell time");
        assert_eq!(ceiling, 66, "SF9/125 kHz against a 400 ms cap");
        assert!(
            MAX_FRAME_FOR_DWELL < ceiling,
            "leave margin under the limit"
        );
        assert!(ceiling < glucobeacon_proto::MAX_FRAME_LEN);
    }

    #[test]
    fn every_message_the_node_can_send_is_under_the_budget() {
        use glucobeacon_core::{AlarmPolicy, Glucose, Reading, Timestamp, Trend};
        use glucobeacon_proto::{
            MAX_FRAME_LEN, Message, Packet, ReadingReport, Status, UplinkState, encode,
        };

        let reading = Reading::new(
            Glucose::from_mgdl(400),
            Trend::DoubleDown,
            Timestamp::from_unix_secs(i64::from(u32::MAX)),
        );
        let messages = [
            Message::Reading(ReadingReport {
                reading,
                sent_at: Timestamp::from_unix_secs(i64::from(u32::MAX)),
            }),
            Message::Status(Status {
                sent_at: Timestamp::from_unix_secs(i64::from(u32::MAX)),
                uplink: UplinkState::AuthFailed,
                consecutive_failures: u16::MAX,
                battery_millivolts: Some(u16::MAX),
            }),
            Message::Policy(AlarmPolicy::DEFAULT),
            Message::Ack {
                acked_seq: u16::MAX,
            },
        ];

        let mut buf = [0u8; MAX_FRAME_LEN];
        for message in messages {
            let packet = Packet::new(u16::MAX, message);
            let frame = encode(&packet, &mut buf).expect("encode");
            assert!(
                frame.len() <= MAX_FRAME_FOR_DWELL,
                "{:?} encodes to {} bytes, over the {MAX_FRAME_FOR_DWELL}-byte budget",
                packet.message,
                frame.len()
            );
        }
    }

    #[test]
    fn the_acknowledge_button_is_flagged_as_a_strapping_pin() {
        // Not a bug, but a decision that should be made deliberately: holding
        // GPIO0 at reset drops the board into the bootloader.
        assert_eq!(ACK_BUTTON, 0);
        assert_ne!(ALTERNATE_ACK_BUTTON, 0);
        assert!(!ASSIGNED.iter().any(|(_, p)| *p == ALTERNATE_ACK_BUTTON));
    }
}
