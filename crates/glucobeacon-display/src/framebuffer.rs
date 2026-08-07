//! A packed one-bit-per-pixel framebuffer.
//!
//! The obvious `Vec<bool>` costs a byte per pixel: 375 KB for an 800×480 panel,
//! on a device with about 320 KB of DRAM in total. Packed, the same panel is
//! 48 000 bytes — which fits, but only just, and only if it is not on the
//! stack. Allocate it as a `static`, or in PSRAM if the board has it.
//!
//! The bit order matches both what e-ink controllers expect over SPI and the
//! netpbm P4 format: most significant bit is the leftmost pixel.

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;

/// Bytes needed for a `width` × `height` one-bit framebuffer.
///
/// Rows are byte-aligned, so a width that is not a multiple of eight wastes up
/// to seven bits per row.
pub const fn bytes_for(width: u32, height: u32) -> usize {
    (width as usize).div_ceil(8) * height as usize
}

/// A one-bit-per-pixel framebuffer of `BYTES` bytes.
///
/// `BYTES` is a separate parameter rather than being computed from the
/// dimensions because Rust cannot yet do arithmetic in const generic bounds.
/// Use [`bytes_for`] to work it out; the constructor checks that it is enough.
#[derive(Clone, Debug)]
pub struct FrameBuffer<const BYTES: usize> {
    width: u32,
    height: u32,
    stride: usize,
    bits: [u8; BYTES],
}

impl<const BYTES: usize> FrameBuffer<BYTES> {
    /// A blank framebuffer, or `None` if `BYTES` is too small for the size.
    pub const fn new(width: u32, height: u32) -> Option<Self> {
        if BYTES < bytes_for(width, height) {
            return None;
        }
        Some(Self {
            width,
            height,
            stride: (width as usize).div_ceil(8),
            bits: [0; BYTES],
        })
    }

    /// The packed bits, row-major, MSB-first within each byte.
    ///
    /// This is what goes to the panel controller, and what a netpbm P4 file
    /// contains after its header.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bits[..bytes_for(self.width, self.height)]
    }

    /// Panel width in pixels.
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Panel height in pixels.
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Whether the pixel at `(x, y)` is inked. Out-of-bounds reads are `false`.
    pub fn get(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let index = y as usize * self.stride + (x / 8) as usize;
        self.bits[index] & (0x80 >> (x % 8)) != 0
    }

    /// Sets the pixel at `(x, y)`. Out-of-bounds writes are dropped.
    pub fn set(&mut self, x: u32, y: u32, on: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let index = y as usize * self.stride + (x / 8) as usize;
        let mask = 0x80 >> (x % 8);
        if on {
            self.bits[index] |= mask;
        } else {
            self.bits[index] &= !mask;
        }
    }

    /// Clears every pixel to `on`.
    pub fn fill(&mut self, on: bool) {
        self.bits.fill(if on { 0xFF } else { 0x00 });
        self.clear_row_padding();
    }

    /// Forces the bits past the right edge of each row back to zero.
    ///
    /// They are invisible either way, but keeping them canonical means one
    /// visible image is always one byte pattern. A partial-refresh driver
    /// decides what to send by diffing the previous framebuffer against the
    /// new one, and padding that flickered between `fill` and a full-width
    /// rectangle would show up as a change — costing a refresh on the device
    /// where refreshes are the expensive thing.
    fn clear_row_padding(&mut self) {
        let used = self.width % 8;
        if used == 0 {
            return;
        }
        let mask = 0xFFu8 << (8 - used);
        for row in 0..self.height as usize {
            let last = row * self.stride + self.stride - 1;
            self.bits[last] &= mask;
        }
    }

    /// How many pixels are inked.
    ///
    /// Counts only pixels inside the panel, so padding bits in a row that is
    /// not a multiple of eight wide do not inflate it.
    pub fn ink(&self) -> u32 {
        let mut count = 0;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.get(x, y) {
                    count += 1;
                }
            }
        }
        count
    }
}

impl<const BYTES: usize> OriginDimensions for FrameBuffer<BYTES> {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl<const BYTES: usize> DrawTarget for FrameBuffer<BYTES> {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(point, color) in pixels {
            // Clipping rather than erroring: a layout that runs a pixel off the
            // edge should look wrong, not take the node down at 3am.
            if point.x < 0 || point.y < 0 {
                continue;
            }
            self.set(point.x as u32, point.y as u32, color.is_on());
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.fill(color.is_on());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::primitives::{Primitive, PrimitiveStyle, Rectangle};

    const SMALL: usize = bytes_for(16, 8);

    fn buffer() -> FrameBuffer<SMALL> {
        FrameBuffer::new(16, 8).expect("fits")
    }

    #[test]
    fn byte_counts_round_rows_up_to_whole_bytes() {
        assert_eq!(bytes_for(8, 1), 1);
        assert_eq!(bytes_for(9, 1), 2);
        assert_eq!(bytes_for(16, 8), 16);
        // The real panel: 100 bytes per row, 480 rows.
        assert_eq!(bytes_for(800, 480), 48_000);
    }

    #[test]
    fn the_panel_fits_in_esp32_dram_when_packed() {
        // A byte per pixel would be 384 000 — more DRAM than the chip has.
        assert!(bytes_for(800, 480) < 64 * 1024);
    }

    #[test]
    fn a_buffer_too_small_for_its_size_is_refused() {
        assert!(FrameBuffer::<15>::new(16, 8).is_none());
        assert!(FrameBuffer::<16>::new(16, 8).is_some());
        assert!(
            FrameBuffer::<17>::new(16, 8).is_some(),
            "extra room is fine"
        );
    }

    #[test]
    fn pixels_round_trip() {
        let mut fb = buffer();
        assert!(!fb.get(3, 2));
        fb.set(3, 2, true);
        assert!(fb.get(3, 2));
        assert_eq!(fb.ink(), 1);
        fb.set(3, 2, false);
        assert!(!fb.get(3, 2));
        assert_eq!(fb.ink(), 0);
    }

    #[test]
    fn neighbouring_pixels_in_one_byte_are_independent() {
        // The bug this catches is a mask that clobbers the rest of the byte.
        let mut fb = buffer();
        for x in 0..8 {
            fb.set(x, 0, true);
        }
        fb.set(3, 0, false);
        for x in 0..8 {
            assert_eq!(fb.get(x, 0), x != 3, "x={x}");
        }
    }

    #[test]
    fn the_leftmost_pixel_is_the_most_significant_bit() {
        // Both the panel controller and netpbm P4 expect this order.
        let mut fb = buffer();
        fb.set(0, 0, true);
        assert_eq!(fb.as_bytes()[0], 0x80);
        fb.set(7, 0, true);
        assert_eq!(fb.as_bytes()[0], 0x81);
    }

    #[test]
    fn out_of_bounds_access_is_ignored_not_fatal() {
        let mut fb = buffer();
        fb.set(16, 0, true);
        fb.set(0, 8, true);
        fb.set(u32::MAX, u32::MAX, true);
        assert_eq!(fb.ink(), 0);
        assert!(!fb.get(16, 0));
        assert!(!fb.get(u32::MAX, 0));
    }

    #[test]
    fn filling_sets_and_clears_everything() {
        let mut fb = buffer();
        fb.fill(true);
        assert_eq!(fb.ink(), 16 * 8);
        fb.fill(false);
        assert_eq!(fb.ink(), 0);
    }

    #[test]
    fn padding_bits_do_not_count_as_ink() {
        // 12 pixels wide occupies 2 bytes; the last 4 bits are padding.
        let mut fb = FrameBuffer::<{ bytes_for(12, 2) }>::new(12, 2).expect("fits");
        fb.fill(true);
        assert_eq!(fb.ink(), 24, "only real pixels count");
        assert_eq!(fb.as_bytes(), [0xFF, 0xF0, 0xFF, 0xF0]);
    }

    #[test]
    fn one_visible_image_is_always_one_byte_pattern() {
        // A partial-refresh driver decides what to send by diffing framebuffers.
        // If padding bits depended on how the image was drawn, an unchanged
        // screen would look changed and cost a refresh.
        let mut filled = FrameBuffer::<{ bytes_for(12, 2) }>::new(12, 2).expect("fits");
        filled.fill(true);

        let mut drawn = FrameBuffer::<{ bytes_for(12, 2) }>::new(12, 2).expect("fits");
        Rectangle::new(Point::zero(), Size::new(12, 2))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut drawn)
            .expect("draw");

        assert_eq!(filled.as_bytes(), drawn.as_bytes());
        assert_eq!(filled.ink(), drawn.ink());
    }

    #[test]
    fn it_works_as_a_draw_target() {
        let mut fb = buffer();
        Rectangle::new(Point::new(2, 1), Size::new(4, 3))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut fb)
            .expect("draw");
        assert_eq!(fb.ink(), 12);
        assert!(fb.get(2, 1));
        assert!(!fb.get(1, 1));
    }

    #[test]
    fn drawing_off_the_edge_clips() {
        let mut fb = buffer();
        Rectangle::new(Point::new(-4, -4), Size::new(100, 100))
            .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
            .draw(&mut fb)
            .expect("draw");
        assert_eq!(fb.ink(), 16 * 8);
    }

    #[test]
    fn clear_goes_through_the_fast_path() {
        let mut fb = buffer();
        fb.clear(BinaryColor::On).expect("clear");
        assert_eq!(fb.ink(), 16 * 8);
        fb.clear(BinaryColor::Off).expect("clear");
        assert_eq!(fb.ink(), 0);
    }
}
