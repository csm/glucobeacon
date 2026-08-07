//! Domain types shared by both ends of a glucobeacon deployment.
//!
//! The gateway (WiFi + Dexcom Share) and the display node (e-ink, buzzer,
//! button) agree on the meaning of a glucose reading and on when a reading
//! should raise an alarm. That agreement lives here so neither end can drift.
//!
//! The crate is `no_std` by default-off: enable `default-features = false` to
//! drop the `std` feature. Nothing here allocates — history is a fixed-capacity
//! ring buffer — so the display node can use it on bare metal.

#![cfg_attr(not(feature = "std"), no_std)]

pub mod alarm;
pub mod glucose;
pub mod history;
pub mod reading;
pub mod time;
pub mod trend;

pub use alarm::{AlarmEngine, AlarmEvent, AlarmKind, AlarmPolicy, Thresholds};
pub use glucose::Glucose;
pub use history::History;
pub use reading::Reading;
pub use time::{Duration, Timestamp};
pub use trend::Trend;
