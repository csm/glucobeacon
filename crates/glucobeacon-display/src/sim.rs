//! Stand-in peripherals for running the display node on a workstation.
//!
//! The panel becomes an image file, the buzzer and LED become log lines, and
//! the button becomes a keypress. Everything above this file is the same code
//! that will run on the device.

use std::fs::File;
use std::io::{self, BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use tracing::info;

use crate::hal::{Button, Buzzer, BuzzerPattern, Indicator, LedState, Panel};

/// A framebuffer that writes itself out as a netpbm bitmap on flush.
///
/// One bit per pixel, matching the real panel, so what shows up in the file is
/// exactly what would show up on the e-ink.
pub struct SimPanel {
    width: u32,
    height: u32,
    pixels: Vec<bool>,
    path: PathBuf,
    flushes: u32,
}

impl std::fmt::Debug for SimPanel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimPanel")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("path", &self.path)
            .field("flushes", &self.flushes)
            .finish_non_exhaustive()
    }
}

impl SimPanel {
    /// A blank panel that writes to `path`.
    pub fn new(width: u32, height: u32, path: impl Into<PathBuf>) -> Self {
        Self {
            width,
            height,
            pixels: vec![false; (width * height) as usize],
            path: path.into(),
            flushes: 0,
        }
    }

    /// How many times the panel has been refreshed.
    ///
    /// Worth watching: on real hardware every one of these is a second of
    /// flashing and a slice of the panel's finite refresh budget.
    pub const fn flushes(&self) -> u32 {
        self.flushes
    }

    /// How many pixels are inked.
    pub fn ink(&self) -> usize {
        self.pixels.iter().filter(|on| **on).count()
    }

    /// Writes the framebuffer as a binary PBM (netpbm P4).
    ///
    /// PBM because it is eight lines of code and every image viewer reads it;
    /// a PNG encoder would be a dependency earning nothing here.
    pub fn write_pbm(&self, path: &Path) -> io::Result<()> {
        let mut out = BufWriter::new(File::create(path)?);
        writeln!(out, "P4")?;
        writeln!(out, "{} {}", self.width, self.height)?;

        let stride = self.width.div_ceil(8);
        let mut row = vec![0u8; stride as usize];
        for y in 0..self.height {
            row.fill(0);
            for x in 0..self.width {
                if self.pixels[(y * self.width + x) as usize] {
                    // In PBM a set bit is black, which is also what ink is.
                    row[(x / 8) as usize] |= 0x80 >> (x % 8);
                }
            }
            out.write_all(&row)?;
        }
        out.flush()
    }
}

impl OriginDimensions for SimPanel {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for SimPanel {
    type Color = BinaryColor;
    type Error = io::Error;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            // Clipping rather than erroring: a layout that runs a pixel off the
            // edge should look wrong, not take the node down.
            if point.x < 0 || point.y < 0 {
                continue;
            }
            let (x, y) = (point.x as u32, point.y as u32);
            if x < self.width && y < self.height {
                self.pixels[(y * self.width + x) as usize] = color.is_on();
            }
        }
        Ok(())
    }
}

impl Panel for SimPanel {
    type Error = io::Error;

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.flushes += 1;
        let path = self.path.clone();
        self.write_pbm(&path)?;
        info!(
            path = %path.display(),
            refresh = self.flushes,
            "panel refreshed"
        );
        Ok(())
    }
}

/// A buzzer that logs instead of making noise.
#[derive(Debug, Default)]
pub struct ConsoleBuzzer {
    pattern: BuzzerPattern,
}

impl ConsoleBuzzer {
    /// A silent buzzer.
    pub const fn new() -> Self {
        Self {
            pattern: BuzzerPattern::Quiet,
        }
    }

    /// The pattern currently playing.
    pub const fn pattern(&self) -> BuzzerPattern {
        self.pattern
    }
}

impl Buzzer for ConsoleBuzzer {
    type Error = io::Error;

    fn set_pattern(&mut self, pattern: BuzzerPattern) -> Result<(), Self::Error> {
        self.pattern = pattern;
        match pattern {
            BuzzerPattern::Quiet => info!("buzzer: off"),
            BuzzerPattern::Chirp => info!("buzzer: chirp"),
            BuzzerPattern::DoubleBeep => info!("buzzer: beep beep ..."),
            BuzzerPattern::Urgent => info!("buzzer: BEEP BEEP BEEP BEEP"),
        }
        Ok(())
    }
}

/// An LED that logs instead of lighting up.
#[derive(Debug, Default)]
pub struct ConsoleLed {
    state: LedState,
}

impl ConsoleLed {
    /// A dark LED.
    pub const fn new() -> Self {
        Self {
            state: LedState::Off,
        }
    }

    /// The current LED state.
    pub const fn state(&self) -> LedState {
        self.state
    }
}

impl Indicator for ConsoleLed {
    type Error = io::Error;

    fn set(&mut self, state: LedState) -> Result<(), Self::Error> {
        self.state = state;
        info!(?state, "button LED");
        Ok(())
    }
}

/// A button wired to the terminal: press Enter to silence.
#[derive(Debug)]
pub struct StdinButton {
    pressed: Arc<AtomicBool>,
}

impl StdinButton {
    /// Starts watching stdin on a background thread.
    ///
    /// Any line — Enter is enough — counts as a press. The flag is latched, so
    /// a press between polls is never dropped.
    pub fn spawn() -> io::Result<Self> {
        let pressed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&pressed);

        std::thread::Builder::new()
            .name("glucobeacon-button".to_owned())
            .spawn(move || {
                let stdin = io::stdin();
                for line in stdin.lock().lines() {
                    if line.is_err() {
                        break;
                    }
                    flag.store(true, Ordering::Release);
                }
            })?;

        Ok(Self { pressed })
    }
}

impl Button for StdinButton {
    type Error = io::Error;

    fn take_press(&mut self) -> Result<bool, Self::Error> {
        Ok(self.pressed.swap(false, Ordering::AcqRel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::primitives::{Primitive, PrimitiveStyle, Rectangle};

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("glucobeacon-{}-{name}.pbm", std::process::id()));
        path
    }

    #[test]
    fn a_new_panel_is_blank() {
        let panel = SimPanel::new(64, 32, temp_path("blank"));
        assert_eq!(panel.ink(), 0);
        assert_eq!(panel.size(), Size::new(64, 32));
    }

    #[test]
    fn drawing_inks_pixels() {
        let mut panel = SimPanel::new(64, 32, temp_path("draw"));
        Rectangle::new(Point::new(4, 4), Size::new(10, 10))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut panel)
            .expect("draw");
        assert_eq!(panel.ink(), 100);
    }

    #[test]
    fn drawing_off_the_edge_clips_instead_of_failing() {
        let mut panel = SimPanel::new(64, 32, temp_path("clip"));
        Rectangle::new(Point::new(-20, -20), Size::new(200, 200))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut panel)
            .expect("draw");
        assert_eq!(panel.ink(), 64 * 32, "the visible area should be filled");
    }

    #[test]
    fn clearing_erases() {
        let mut panel = SimPanel::new(64, 32, temp_path("clear"));
        panel.clear(BinaryColor::On).expect("fill");
        assert_eq!(panel.ink(), 64 * 32);
        panel.clear(BinaryColor::Off).expect("clear");
        assert_eq!(panel.ink(), 0);
    }

    #[test]
    fn a_flushed_panel_is_a_readable_pbm() {
        let path = temp_path("pbm");
        let mut panel = SimPanel::new(16, 4, &path);
        Rectangle::new(Point::zero(), Size::new(8, 4))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut panel)
            .expect("draw");
        panel.flush().expect("flush");

        let bytes = std::fs::read(&path).expect("read back");
        let header = b"P4\n16 4\n";
        assert!(bytes.starts_with(header), "bad header: {:?}", &bytes[..8]);

        // Two bytes per row, four rows; the left half of each row is set.
        let pixels = &bytes[header.len()..];
        assert_eq!(pixels.len(), 8);
        for row in pixels.chunks(2) {
            assert_eq!(row, [0xFF, 0x00]);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn flushes_are_counted() {
        let path = temp_path("count");
        let mut panel = SimPanel::new(8, 8, &path);
        assert_eq!(panel.flushes(), 0);
        panel.flush().expect("flush");
        panel.flush().expect("flush");
        assert_eq!(panel.flushes(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_width_that_is_not_a_multiple_of_eight_still_writes_whole_bytes() {
        let path = temp_path("odd");
        let mut panel = SimPanel::new(12, 2, &path);
        panel.clear(BinaryColor::On).expect("fill");
        panel.flush().expect("flush");

        let bytes = std::fs::read(&path).expect("read back");
        let pixels = &bytes[b"P4\n12 2\n".len()..];
        // 12 bits rounds up to 2 bytes per row.
        assert_eq!(pixels.len(), 4);
        assert_eq!(pixels, [0xFF, 0xF0, 0xFF, 0xF0]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_console_buzzer_tracks_what_it_was_told() {
        let mut buzzer = ConsoleBuzzer::new();
        assert_eq!(buzzer.pattern(), BuzzerPattern::Quiet);
        buzzer.set_pattern(BuzzerPattern::Urgent).expect("set");
        assert_eq!(buzzer.pattern(), BuzzerPattern::Urgent);
    }

    #[test]
    fn the_console_led_tracks_what_it_was_told() {
        let mut led = ConsoleLed::new();
        assert_eq!(led.state(), LedState::Off);
        led.set(LedState::FastBlink).expect("set");
        assert_eq!(led.state(), LedState::FastBlink);
    }

    #[test]
    fn a_button_press_is_latched_and_consumed_once() {
        // Exercised without stdin: the latch is the part with the logic in it.
        let pressed = Arc::new(AtomicBool::new(false));
        let mut button = StdinButton {
            pressed: Arc::clone(&pressed),
        };
        assert!(!button.take_press().expect("poll"));

        pressed.store(true, Ordering::Release);
        assert!(
            button.take_press().expect("poll"),
            "the press must not be lost"
        );
        assert!(!button.take_press().expect("poll"), "and not seen twice");
    }
}
