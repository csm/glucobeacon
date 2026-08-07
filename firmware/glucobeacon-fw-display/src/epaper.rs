//! The 7-inch e-ink panel.
//!
//! **This is the one module with no driver behind it yet.** 800×480 panels of
//! this size are usually an SSD1677-class controller, but the command set and
//! the waveform LUT vary by module, so it needs writing against whichever panel
//! actually arrives. The shape is here; the register writes are not.
//!
//! What the rest of the firmware needs from it is small: initialize once, then
//! take a packed 1-bit framebuffer and put it on the glass.
//!
//! Two things to get right when filling this in:
//!
//! - **Wait on BUSY, do not guess.** A full refresh is on the order of a
//!   second and varies with temperature. Polling the busy line is the only
//!   reliable way to know it finished; a fixed delay is either wrong or slow.
//! - **Schedule a periodic full refresh.** Partial updates are much faster and
//!   do not flash, but a panel that only ever gets partial updates ghosts, and
//!   the ghosting is cumulative. A full refresh every so often clears it.

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
    /// The SPI transfer failed.
    Spi,
    /// The panel held BUSY past the timeout, which usually means it is not
    /// powered or the reset line is wrong.
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
    /// High while a refresh is in progress.
    busy: BSY,
    delay: DLY,
}

/// How long to wait for BUSY to fall before giving up.
///
/// A full refresh at low temperature can take several seconds; well past that
/// means something is wrong with the wiring rather than slow.
const BUSY_TIMEOUT_MILLIS: u32 = 20_000;

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
        // The controller-specific setup goes here: panel size, gate/source
        // driving, VCOM, and the waveform LUT if the module needs one loaded.
        self.wait_while_busy()
    }

    /// Pushes a full framebuffer and waits for the refresh to complete.
    ///
    /// `bits` is packed one bit per pixel, MSB leftmost, rows padded to whole
    /// bytes — which is exactly what
    /// [`FrameBuffer::as_bytes`](glucobeacon_display::FrameBuffer::as_bytes)
    /// produces, so this is a straight transfer with no repacking.
    pub fn write(&mut self, bits: &[u8]) -> Result<(), PanelError> {
        self.command(CMD_WRITE_RAM)?;
        self.data(bits)?;
        self.command(CMD_REFRESH)?;
        self.wait_while_busy()
    }

    /// Puts the panel into deep sleep.
    ///
    /// Worth doing between refreshes: an e-ink panel holds its image with no
    /// power, and leaving the controller awake is most of its idle draw.
    pub fn sleep(&mut self) -> Result<(), PanelError> {
        self.command(CMD_DEEP_SLEEP)?;
        self.data(&[0x01])
    }

    fn hardware_reset(&mut self) -> Result<(), PanelError> {
        self.reset.set_low().map_err(|_| PanelError::Spi)?;
        self.delay.delay_ms(10);
        self.reset.set_high().map_err(|_| PanelError::Spi)?;
        self.delay.delay_ms(10);
        Ok(())
    }

    fn command(&mut self, byte: u8) -> Result<(), PanelError> {
        self.dc.set_low().map_err(|_| PanelError::Spi)?;
        self.spi.write(&[byte]).map_err(|_| PanelError::Spi)
    }

    fn data(&mut self, bytes: &[u8]) -> Result<(), PanelError> {
        self.dc.set_high().map_err(|_| PanelError::Spi)?;
        self.spi.write(bytes).map_err(|_| PanelError::Spi)
    }

    /// Blocks until the panel drops BUSY.
    fn wait_while_busy(&mut self) -> Result<(), PanelError> {
        let mut waited = 0;
        while self.busy.is_high().map_err(|_| PanelError::Spi)? {
            self.delay.delay_ms(10);
            waited += 10;
            if waited >= BUSY_TIMEOUT_MILLIS {
                return Err(PanelError::Timeout);
            }
        }
        Ok(())
    }
}

// Controller commands. These are the SSD1677 values; confirm against the
// datasheet for the panel that arrives.
const CMD_WRITE_RAM: u8 = 0x24;
const CMD_REFRESH: u8 = 0x20;
const CMD_DEEP_SLEEP: u8 = 0x10;
