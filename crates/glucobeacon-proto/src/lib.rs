//! The wire protocol spoken across the LoRa link.
//!
//! One gateway talks to one display node over a radio with a ~200 byte
//! payload budget, no acknowledgements at the MAC layer, and enough packet
//! loss that every message has to stand on its own. So messages are small,
//! self-contained, and idempotent: losing one costs you five minutes of
//! staleness, not a corrupted state machine.
//!
//! [`frame`] is the byte layout, [`message`] is what those bytes mean, and
//! [`link`] is the transport-agnostic trait the radio drivers implement.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod frame;
pub mod link;
pub mod message;
pub mod radio;

pub use frame::{FrameError, MAX_FRAME_LEN, MAX_PAYLOAD_LEN, PROTOCOL_VERSION, decode, encode};
pub use link::{Link, LinkError};
pub use message::{Message, Packet, ReadingReport, Status, UplinkState};
pub use radio::{Bandwidth, CodingRate, RadioConfig, Region, SpreadingFactor};

#[cfg(feature = "udp-link")]
pub mod udp;
