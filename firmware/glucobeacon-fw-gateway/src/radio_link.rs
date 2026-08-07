//! The [`Link`] implementation over the SX1262.
//!
//! Identical in shape to the display node's, and for the same reasons — see
//! `firmware/glucobeacon-fw-display/src/radio_link.rs`. The gateway is the
//! transmitting end, so it spends nearly all its time idle rather than in
//! receive; it listens only long enough to notice an acknowledgement.
//!
//! Kept as a copy rather than a shared crate for now: it is short, and the two
//! ends use different HAL types. If it grows, promote it to a crate under
//! `crates/` with the HAL types behind generics — the trait bounds are already
//! written that way.

#[path = "../../glucobeacon-fw-display/src/radio_link.rs"]
mod shared;

pub use shared::LoraLink;
