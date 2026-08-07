//! The bedside end of a glucobeacon pair.
//!
//! Listens on the LoRa link, keeps a few hours of readings, paints a 7-inch
//! e-ink panel, and drives a piezo buzzer and a lit silence button. It is the
//! end that decides when to make noise: it holds the alarm state, so a gateway
//! that dies looks like stale data and alarms, rather than looking like nothing
//! is wrong.
//!
//! [`app`] is the logic, with no I/O in it. [`hal`] is the hardware, as traits.
//! [`sim`] implements those traits against a workstation — the panel becomes an
//! image file, the buzzer and LED log, and the button is the Enter key — so
//! everything above the traits is the same code that will run on the device.

pub mod app;
pub mod glyphs;
pub mod hal;
pub mod sim;
pub mod state;
pub mod ui;

pub use app::{DisplayApp, Tick};
pub use state::{Applied, DisplayState};
pub use ui::{Frame, PANEL_HEIGHT, PANEL_WIDTH};
