//! Display-node firmware for the Heltec WiFi LoRa 32 V3.
//!
//! Receives readings over the SX1262, keeps the alarm state, paints the e-ink
//! panel, and drives the buzzer and the silence button.
//!
//! Deliberately thin. Everything with a decision in it — when to alarm, when a
//! repaint is worth its refresh, how long a beep lasts, whether a press was
//! real or contact bounce — lives in `glucobeacon-display` and is tested on a
//! workstation. What is left here is SPI, GPIO, and a loop.
//!
//! Not yet built against hardware; see `firmware/README.md` for what that
//! means and what to check first.

#![no_std]
#![no_main]

mod board;
mod epaper;
mod radio_link;

use embedded_hal_bus::spi::ExclusiveDevice;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::main;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::{Instant, Rate};
use static_cell::StaticCell;

use glucobeacon_core::Duration;
use glucobeacon_display::app::DisplayApp;
use glucobeacon_display::embedded::{GpioButton, GpioBuzzer, GpioIndicator, Polarity};
use glucobeacon_display::hal::{Button, Buzzer, Indicator};
use glucobeacon_display::ui::{self, Frame};
use glucobeacon_display::{PANEL_HEIGHT, PANEL_WIDTH, PanelBuffer};
use glucobeacon_proto::Link;

/// The panel framebuffer: 48 KB, which is far too much for the stack and is
/// wanted for the whole life of the program. `StaticCell` gives it a `'static`
/// home without an allocator and without `static mut`.
static FRAMEBUFFER: StaticCell<PanelBuffer> = StaticCell::new();

/// How often to service the buzzer, LED, and button.
///
/// Fast enough that a press feels instant and a beep is not ragged. The panel
/// is repainted far less often — see `tick.redraw`.
const TICK_MILLIS: u32 = 5;

#[main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let delay = Delay::new();

    esp_println::logger::init_logger_from_env();
    log::info!("glucobeacon display node starting");

    // --- Radio: SPI2 to the SX1262. Pins per board::lora. ---
    let radio_spi = Spi::new(
        peripherals.SPI2,
        SpiConfig::default().with_frequency(Rate::from_mhz(10)),
    )
    .expect("SPI2")
    .with_sck(peripherals.GPIO9)
    .with_mosi(peripherals.GPIO10)
    .with_miso(peripherals.GPIO11);

    let radio_cs = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
    let radio_bus = ExclusiveDevice::new(radio_spi, radio_cs, delay).expect("radio SPI device");

    let mut link = radio_link::LoraLink::new(
        radio_bus,
        Output::new(peripherals.GPIO12, Level::High, OutputConfig::default()),
        Input::new(peripherals.GPIO13, InputConfig::default().with_pull(Pull::None)),
        Input::new(peripherals.GPIO14, InputConfig::default().with_pull(Pull::None)),
        board::RADIO,
        delay,
    )
    .expect("radio init");

    // --- Panel: SPI3 to the e-ink controller. Pins per board::epaper. ---
    let panel_spi = Spi::new(
        peripherals.SPI3,
        SpiConfig::default().with_frequency(Rate::from_mhz(20)),
    )
    .expect("SPI3")
    .with_sck(peripherals.GPIO3)
    .with_mosi(peripherals.GPIO4);

    let panel_cs = Output::new(peripherals.GPIO5, Level::High, OutputConfig::default());
    let mut panel = epaper::Panel::new(
        ExclusiveDevice::new(panel_spi, panel_cs, delay).expect("panel SPI device"),
        Output::new(peripherals.GPIO6, Level::Low, OutputConfig::default()),
        Output::new(peripherals.GPIO7, Level::High, OutputConfig::default()),
        Input::new(peripherals.GPIO15, InputConfig::default().with_pull(Pull::None)),
        delay,
    );
    panel.init().expect("panel init");

    // --- Buzzer, button LED, silence button. Polarity follows the wiring:
    // the button goes to ground behind a pull-up, so it reads active-low. ---
    let mut buzzer = GpioBuzzer::new(
        Output::new(peripherals.GPIO45, Level::Low, OutputConfig::default()),
        Polarity::ActiveHigh,
    );
    let mut led = GpioIndicator::new(
        Output::new(peripherals.GPIO46, Level::Low, OutputConfig::default()),
        Polarity::ActiveHigh,
    );
    let mut button = GpioButton::new(
        Input::new(peripherals.GPIO47, InputConfig::default().with_pull(Pull::Up)),
        Polarity::ActiveLow,
    );

    let framebuffer =
        FRAMEBUFFER.init(PanelBuffer::new(PANEL_WIDTH, PANEL_HEIGHT).expect("panel fits"));

    let mut app = DisplayApp::default();
    let booted = Instant::now();

    loop {
        let millis = booted.elapsed().as_millis();
        let uptime = Duration::from_secs((millis / 1_000) as u32);

        // Corrupt frames and foreign traffic are ordinary on a shared band:
        // drop them and keep listening.
        match link.recv() {
            Ok(Some(packet)) => {
                let applied = app.handle_packet(&packet, uptime);
                log::debug!("packet {} -> {applied:?}", packet.seq);
            }
            Ok(None) => {}
            Err(error) => log::debug!("discarded a frame: {error}"),
        }

        let tick = app.tick(uptime);
        if let Some(event) = tick.event {
            log::info!("alarm: {event:?}");
        }
        apply(&mut buzzer, &mut led, &tick);

        let pressed = button.take_press().unwrap_or(false);
        if pressed && app.press_silence(uptime) {
            log::info!("silenced");
            // Apply the resulting quiet now rather than next time round: the
            // button has to feel like it did something.
            let tick = app.tick(uptime);
            apply(&mut buzzer, &mut led, &tick);
            repaint(&mut panel, framebuffer, &app, uptime);
        } else if tick.redraw {
            repaint(&mut panel, framebuffer, &app, uptime);
        }

        // These three carry the pattern and debounce timing, so they run every
        // tick regardless of what else happened.
        let _ = buzzer.poll(millis);
        let _ = led.poll(millis);
        let _ = button.poll(millis);

        delay.delay_millis(TICK_MILLIS);
    }
}

/// Pushes a tick's buzzer and LED decisions to the hardware.
fn apply<B: Buzzer, I: Indicator>(
    buzzer: &mut B,
    led: &mut I,
    tick: &glucobeacon_display::Tick,
) {
    if let Some(pattern) = tick.buzzer {
        let _ = buzzer.set_pattern(pattern);
    }
    if let Some(state) = tick.led {
        let _ = led.set(state);
    }
}

/// Renders into the framebuffer and pushes it to the panel.
///
/// The expensive operation: an 800×480 full refresh takes about a second and
/// flashes the whole screen, which is why `tick.redraw` gates it.
fn repaint(
    panel: &mut epaper::Panel,
    framebuffer: &mut PanelBuffer,
    app: &DisplayApp,
    uptime: Duration,
) {
    let frame = Frame {
        now: app.now(uptime),
        uptime,
        state: app.state(),
        alarm: app.alarm(),
        silence_remaining: app.silence_remaining(uptime),
    };
    // Drawing into a framebuffer cannot fail.
    let Ok(()) = ui::render(framebuffer, &frame);

    if let Err(error) = panel.write(framebuffer.as_bytes()) {
        log::warn!("panel refresh failed: {error:?}");
    }
}
