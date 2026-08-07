//! The simulated panel, in a window on the host.
//!
//! The e-ink panel is a grid of bits, and this paints those bits into a native
//! window instead of a file, so the layout can be tuned in a loop measured in
//! seconds rather than in "save, alt-tab, reopen the image". Nothing above this
//! file knows the window exists: it draws whatever [`crate::sim::SimPanel`]
//! flushed, which is the same buffer that would have gone to the controller.
//!
//! Two things about this are not incidental:
//!
//! - **The window must be built and pumped on the main thread.** macOS requires
//!   AppKit calls to come from the main thread, and violating that aborts the
//!   process rather than misbehaving politely. So the display node runs on a
//!   worker and mails frames here, not the other way around.
//! - **The window only redraws on flush.** That is the honest thing to show: on
//!   the real panel nothing changes until a refresh, which takes about a second
//!   and visibly flashes. A window that repainted continuously would hide
//!   exactly the bug — a redraw storm — that the refresh budget cares about.

use core::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use minifb::{Key, KeyRepeat, Scale, ScaleMode, Window, WindowOptions};
use tracing::{debug, info};

use crate::sim::PanelFrame;

/// Ink, as `0x00RRGGBB`. Not quite black: e-ink particles never are.
const INK: u32 = 0x0020_2124;
/// Unwritten panel. Not quite white either — the film is faintly warm and grey.
const PAPER: u32 = 0x00F2_F0E9;

/// How long the window waits for a frame before pumping events anyway.
///
/// Only the event loop runs this often; the pixels change on flush and not
/// before. About 60 Hz, which is what it takes for dragging and resizing to
/// feel attached to the mouse.
const PUMP: Duration = Duration::from_millis(16);

/// A host window showing the simulated panel.
pub struct PanelWindow {
    window: Window,
    pixels: Vec<u32>,
    width: usize,
    height: usize,
}

impl fmt::Debug for PanelWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PanelWindow")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl PanelWindow {
    /// Opens a window `width` × `height` panel pixels, magnified `scale` times.
    ///
    /// Must be called from the main thread; see the module docs.
    ///
    /// The window is resizable and the panel keeps its aspect ratio inside it,
    /// so a 7-inch panel can be pulled up to something you can read from a
    /// couch — which is, after all, the viewing distance being designed for.
    pub fn open(width: u32, height: u32, scale: u8) -> Result<Self, minifb::Error> {
        let width = width as usize;
        let height = height as usize;

        let mut window = Window::new(
            "glucobeacon panel",
            width,
            height,
            WindowOptions {
                resize: true,
                scale: match scale {
                    0 | 1 => Scale::X1,
                    2 => Scale::X2,
                    3 | 4 => Scale::X4,
                    _ => Scale::X8,
                },
                scale_mode: ScaleMode::AspectRatioStretch,
                ..WindowOptions::default()
            },
        )?;
        // No rate limiting inside minifb: the blocking wait for the next frame
        // in `run` already paces the loop, and a second limiter on top of it
        // would only add latency to a keypress.
        window.set_target_fps(0);
        // Behind the panel, where the letterboxing shows. Dark, so the eye goes
        // to the panel and the paper edge is unmistakable.
        window.set_background_color(0x20, 0x20, 0x22);

        Ok(Self {
            window,
            pixels: vec![PAPER; width * height],
            width,
            height,
        })
    }

    /// Whether the window is still up.
    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    /// Paints one frame, ignoring a frame that is not the size of the window.
    pub fn show(&mut self, frame: &PanelFrame) {
        if frame.width as usize != self.width || frame.height as usize != self.height {
            debug!(
                frame = ?(frame.width, frame.height),
                window = ?(self.width, self.height),
                "ignoring a frame that is not the panel's size"
            );
            return;
        }
        blit(frame, &mut self.pixels);
        self.window
            .set_title(&format!("glucobeacon panel — refresh {}", frame.refresh));
    }

    /// Runs the window until it is closed or `stop` is set.
    ///
    /// Frames arrive on `frames`; pressing Space or Enter over the window sets
    /// `press`, which is the silence button's latch, so the window is a stand-in
    /// for the lit button as well as for the panel. Escape closes the window.
    ///
    /// Returns once the window is gone, having set `stop` — the display node is
    /// watching that flag and will wind itself up.
    pub fn run(mut self, frames: &Receiver<PanelFrame>, press: &AtomicBool, stop: &AtomicBool) {
        info!("panel window open; Space silences an alarm, Esc closes");

        while self.window.is_open()
            && !self.window.is_key_down(Key::Escape)
            && !stop.load(Ordering::Acquire)
        {
            // Blocking here rather than spinning is what keeps an idle sim at
            // nearly no CPU, which matters when it is meant to be left running
            // for hours next to whatever else is being worked on.
            match frames.recv_timeout(PUMP) {
                Ok(frame) => {
                    self.show(&frame);
                    // Frames can bunch up behind a slow window; only the last
                    // one is worth drawing, exactly as on the real panel.
                    while let Ok(newer) = frames.try_recv() {
                        self.show(&newer);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                // The node hung up. Keep the last frame on screen so whatever
                // it died showing can still be read.
                Err(RecvTimeoutError::Disconnected) => {
                    debug!("the display node closed the frame channel");
                    break;
                }
            }

            if self.window.is_key_pressed(Key::Space, KeyRepeat::No)
                || self.window.is_key_pressed(Key::Enter, KeyRepeat::No)
            {
                press.store(true, Ordering::Release);
            }

            if let Err(error) =
                self.window
                    .update_with_buffer(&self.pixels, self.width, self.height)
            {
                debug!(%error, "the panel window could not be updated");
                break;
            }
        }

        stop.store(true, Ordering::Release);
        info!("panel window closed");
    }
}

/// Expands a packed one-bit frame into `0x00RRGGBB` pixels.
///
/// `out` is reused across frames — an 800×480 panel is 384 000 words, and
/// reallocating that every refresh for no reason would be a waste.
fn blit(frame: &PanelFrame, out: &mut Vec<u32>) {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let stride = width.div_ceil(8);

    out.clear();
    out.reserve(width * height);
    for y in 0..height {
        let row = frame.bits.get(y * stride..y * stride + stride);
        for x in 0..width {
            // A truncated frame reads as blank paper rather than panicking:
            // this is a debugging aid, and it failing is worse than it lying.
            let inked = row.is_some_and(|row| row[x / 8] & (0x80 >> (x % 8)) != 0);
            out.push(if inked { INK } else { PAPER });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with `bits` laid out for a `width` × `height` panel.
    fn frame(width: u32, height: u32, bits: &[u8]) -> PanelFrame {
        PanelFrame {
            width,
            height,
            refresh: 1,
            bits: bits.to_vec(),
        }
    }

    #[test]
    fn a_set_bit_becomes_ink_and_a_clear_bit_becomes_paper() {
        // 0b1010_0000: the leftmost pixel is set, matching the panel's MSB-first
        // order. Getting this backwards would mirror the whole display.
        let mut pixels = Vec::new();
        blit(&frame(4, 1, &[0b1010_0000]), &mut pixels);
        assert_eq!(pixels, [INK, PAPER, INK, PAPER]);
    }

    #[test]
    fn every_panel_pixel_gets_a_word() {
        let mut pixels = Vec::new();
        blit(&frame(16, 3, &[0; 6]), &mut pixels);
        assert_eq!(pixels.len(), 48);
        assert!(pixels.iter().all(|&p| p == PAPER));
    }

    #[test]
    fn row_padding_is_not_drawn() {
        // 12 pixels a row is 2 bytes, so 4 bits per row are padding. Drawing
        // them would smear the right-hand edge into the next row.
        let mut pixels = Vec::new();
        blit(&frame(12, 2, &[0xFF, 0xFF, 0xFF, 0xFF]), &mut pixels);
        assert_eq!(pixels.len(), 24);
        assert!(pixels.iter().all(|&p| p == INK));
    }

    #[test]
    fn the_second_row_starts_at_the_second_stride() {
        let mut pixels = Vec::new();
        blit(&frame(8, 2, &[0x00, 0xFF]), &mut pixels);
        assert!(pixels[..8].iter().all(|&p| p == PAPER));
        assert!(pixels[8..].iter().all(|&p| p == INK));
    }

    #[test]
    fn a_short_frame_reads_as_blank_rather_than_panicking() {
        let mut pixels = Vec::new();
        blit(&frame(8, 4, &[0xFF]), &mut pixels);
        assert_eq!(pixels.len(), 32);
        assert!(pixels[..8].iter().all(|&p| p == INK));
        assert!(pixels[8..].iter().all(|&p| p == PAPER));
    }

    #[test]
    fn the_buffer_is_reused_between_frames() {
        let mut pixels = vec![0; 999];
        blit(&frame(8, 1, &[0xFF]), &mut pixels);
        assert_eq!(pixels.len(), 8, "stale pixels must not survive a frame");
    }

    #[test]
    fn ink_and_paper_are_far_enough_apart_to_read() {
        // The panel is one bit deep; if these ever drift together the window
        // stops being a usable stand-in for it.
        let luma = |c: u32| (c >> 16 & 0xFF) + (c >> 8 & 0xFF) + (c & 0xFF);
        assert!(luma(PAPER) - luma(INK) > 500, "not enough contrast");
    }
}
