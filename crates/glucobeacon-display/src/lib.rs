//! The bedside end of a glucobeacon pair.
//!
//! Listens on the LoRa link, keeps a few hours of readings, paints a 7-inch
//! e-ink panel, and drives a piezo buzzer and a lit silence button. It is the
//! end that decides when to make noise: it holds the alarm state, so a gateway
//! that dies looks like stale data and alarms, rather than looking like nothing
//! is wrong.
//!
//! [`app`] is the logic, with no I/O in it. [`hal`] is the hardware, as traits.
//! [`framebuffer`] is what gets drawn into. [`sim`] implements the traits
//! against a workstation — the panel becomes an image file, the buzzer and LED
//! log, and the button is the Enter key. With the `window` feature the panel
//! also appears in a window on the host, which is the quickest way to iterate
//! on the layout without hardware in front of you.
//!
//! Everything except [`sim`] is `no_std` and allocation-free, so it runs on the
//! ESP32-S3 unchanged. Build with `default-features = false` for a bare-metal
//! (`esp-hal`) firmware, or with `--features std` for an ESP-IDF one — `std`
//! deliberately does not pull in the simulator's command-line machinery.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod app;

#[cfg(feature = "embedded")]
pub mod embedded;

pub mod framebuffer;
pub mod glyphs;
pub mod hal;
pub mod pattern;
pub mod state;
pub mod ui;

#[cfg(feature = "sim")]
pub mod sim;

#[cfg(feature = "window")]
pub mod window;

pub use app::{DisplayApp, Tick};
pub use framebuffer::{FrameBuffer, bytes_for};
pub use pattern::{Player, Timeline};
pub use state::{Applied, DisplayState};
pub use ui::{Frame, PANEL_HEIGHT, PANEL_WIDTH, TITLE, title};

/// Bytes needed for a framebuffer covering the whole panel.
pub const PANEL_BYTES: usize = bytes_for(PANEL_WIDTH, PANEL_HEIGHT);

/// A framebuffer sized for the panel.
///
/// About 48 KB. Too big for the stack on an ESP32 — put it in a `static`, or in
/// PSRAM if the board has any.
pub type PanelBuffer = FrameBuffer<PANEL_BYTES>;
