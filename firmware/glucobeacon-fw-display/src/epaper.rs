//! The Waveshare 7.5" e-Paper HAT (800×480, black/white).
//!
//! **Still needs finishing against the panel in hand.** The command constants
//! below are UC8179 values, which is the controller on the V2 revision of this
//! module; earlier revisions differ. Before writing this by hand, look at the
//! [`epd-waveshare`](https://crates.io/crates/epd-waveshare) crate, which has
//! a tested `Epd7in5_V2` driver and speaks `embedded-hal` — this module exists
//! to document the wiring and the two traps, not to compete with it.
//!
//! # Two traps
//!
//! **Ink polarity is inverted.** The framebuffer uses 1 for ink, matching
//! netpbm and every sane convention. The UC8179's RAM uses 0 for black. A
//! straight `write` of the framebuffer produces a photographic negative, which
//! looks like a bug in the layout rather than in the driver — so [`Panel::write`]
//! inverts as it goes.
//!
//! **Wait on BUSY, do not guess.** A full refresh takes on the order of a
//! second and varies with temperature. Polling the busy line is the only
//! reliable way to know it finished; a fixed delay is either wrong or slow.
//! Note the polarity: on this panel BUSY is *low* while the controller is
//! working, which is the opposite of most.

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal::spi::SpiDevice;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::Spi;

/// The concrete SPI device the panel sits on.
pub type PanelSpi = embedded_hal_bus::spi::ExclusiveDevice<
    Spi<'static, esp_hal::Blocking>,
    Output<'static>,
    Delay,
>;

/// Something that went wrong talking to the panel.
#[derive(Debug)]
pub enum PanelError {
    /// An SPI transfer or a GPIO write failed.
    Bus,
    /// The panel held BUSY past the timeout, which usually means it is not
    /// powered, or RST is wrong, or the ribbon is not seated.
    Timeout,
}

/// The e-ink panel.
pub struct Panel<
    SPI = PanelSpi,
    DC = Output<'static>,
    RST = Output<'static>,
    BSY = Input<'static>,
    DLY = Delay,
> {
    spi: SPI,
    /// Data/command select: low for a command byte, high for data.
    dc: DC,
    /// Panel reset, active low.
    reset: RST,
    /// Low while a refresh is in progress.
    busy: BSY,
    delay: DLY,
}

/// How long to wait for a refresh before giving up.
///
/// A full refresh at low temperature can take several seconds; well past that
/// means the wiring is wrong rather than the panel slow.
const BUSY_TIMEOUT_MILLIS: u32 = 30_000;

/// Bytes pushed per SPI transfer.
///
/// The whole framebuffer is 48 000 bytes, which is a long transaction to hold
/// in one call; chunking keeps each one bounded.
const CHUNK: usize = 512;

impl<SPI, DC, RST, BSY, DLY> Panel<SPI, DC, RST, BSY, DLY>
where
    SPI: SpiDevice<u8>,
    DC: OutputPin,
    RST: OutputPin,
    BSY: InputPin,
    DLY: DelayNs,
{
    /// Binds the panel to its bus and control pins.
    pub fn new(spi: SPI, dc: DC, reset: RST, busy: BSY, delay: DLY) -> Self {
        Self {
            spi,
            dc,
            reset,
            busy,
            delay,
        }
    }

    /// Resets the panel and loads its power-on configuration.
    pub fn init(&mut self) -> Result<(), PanelError> {
        self.hardware_reset()?;

        self.command(CMD_POWER_SETTING)?;
        self.data(&[0x07, 0x07, 0x3F, 0x3F])?;

        self.command(CMD_POWER_ON)?;
        self.delay.delay_ms(100);
        self.wait_while_busy()?;

        self.command(CMD_PANEL_SETTING)?;
        self.data(&[0x1F])?;

        // Resolution, big-endian: 800 x 480.
        self.command(CMD_RESOLUTION)?;
        self.data(&[0x03, 0x20, 0x01, 0xE0])?;

        self.command(CMD_DUAL_SPI)?;
        self.data(&[0x00])?;

        self.command(CMD_VCOM_AND_DATA_INTERVAL)?;
        self.data(&[0x10, 0x07])?;

        self.command(CMD_TCON_SETTING)?;
        self.data(&[0x22])?;

        Ok(())
    }

    /// Pushes a full framebuffer and waits for the refresh to complete.
    ///
    /// `bits` is packed one bit per pixel, MSB leftmost, rows padded to whole
    /// bytes — exactly what
    /// [`FrameBuffer::as_bytes`](glucobeacon_display::FrameBuffer::as_bytes)
    /// produces. The bits are inverted on the way out: the framebuffer uses 1
    /// for ink, the controller uses 0 for black.
    pub fn write(&mut self, bits: &[u8]) -> Result<(), PanelError> {
        self.command(CMD_DISPLAY_DATA)?;

        let mut chunk = [0u8; CHUNK];
        for block in bits.chunks(CHUNK) {
            let out = &mut chunk[..block.len()];
            for (dst, src) in out.iter_mut().zip(block) {
                *dst = !*src;
            }
            self.data(out)?;
        }

        self.command(CMD_REFRESH)?;
        self.delay.delay_ms(100);
        self.wait_while_busy()
    }

    /// Powers the panel down between refreshes.
    ///
    /// Worth doing: e-ink holds its image with no power, and leaving the
    /// controller awake is most of the idle draw. Waking it costs an
    /// [`init`](Self::init).
    pub fn sleep(&mut self) -> Result<(), PanelError> {
        self.command(CMD_POWER_OFF)?;
        self.wait_while_busy()?;
        self.command(CMD_DEEP_SLEEP)?;
        self.data(&[0xA5])
    }

    fn hardware_reset(&mut self) -> Result<(), PanelError> {
        self.reset.set_high().map_err(|_| PanelError::Bus)?;
        self.delay.delay_ms(20);
        self.reset.set_low().map_err(|_| PanelError::Bus)?;
        self.delay.delay_ms(4);
        self.reset.set_high().map_err(|_| PanelError::Bus)?;
        self.delay.delay_ms(20);
        Ok(())
    }

    fn command(&mut self, byte: u8) -> Result<(), PanelError> {
        self.dc.set_low().map_err(|_| PanelError::Bus)?;
        self.spi.write(&[byte]).map_err(|_| PanelError::Bus)
    }

    fn data(&mut self, bytes: &[u8]) -> Result<(), PanelError> {
        self.dc.set_high().map_err(|_| PanelError::Bus)?;
        self.spi.write(bytes).map_err(|_| PanelError::Bus)
    }

    /// Blocks until the panel finishes.
    ///
    /// BUSY is *low* while this controller is working — the opposite of most
    /// e-ink parts, and an easy way to get an init that returns instantly and
    /// a panel that never draws.
    fn wait_while_busy(&mut self) -> Result<(), PanelError> {
        let mut waited = 0;
        while self.busy.is_low().map_err(|_| PanelError::Bus)? {
            self.delay.delay_ms(10);
            waited += 10;
            if waited >= BUSY_TIMEOUT_MILLIS {
                return Err(PanelError::Timeout);
            }
        }
        Ok(())
    }
}

// UC8179 commands, per the Waveshare 7.5" V2 reference driver.
const CMD_PANEL_SETTING: u8 = 0x00;
const CMD_POWER_SETTING: u8 = 0x01;
const CMD_POWER_OFF: u8 = 0x02;
const CMD_POWER_ON: u8 = 0x04;
const CMD_DEEP_SLEEP: u8 = 0x07;
/// The "new data" buffer, which is what a full refresh displays.
const CMD_DISPLAY_DATA: u8 = 0x13;
const CMD_REFRESH: u8 = 0x12;
const CMD_VCOM_AND_DATA_INTERVAL: u8 = 0x50;
const CMD_TCON_SETTING: u8 = 0x60;
const CMD_RESOLUTION: u8 = 0x61;
const CMD_DUAL_SPI: u8 = 0x15;
