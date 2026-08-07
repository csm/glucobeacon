//! The WiFi end of a glucobeacon pair.
//!
//! Polls Dexcom Share and relays each new reading over the LoRa link to the
//! display node. It holds no alarm state of its own: the display node decides
//! what to beep about, using the policy this end pushes to it. That way a
//! gateway reboot cannot silence an alarm, and a gateway outage looks like
//! exactly what it is — stale data — rather than like everything being fine.
//!
//! The binary in `main.rs` is a thin wrapper around these modules.

pub mod config;
pub mod radio;
pub mod schedule;
pub mod source;

pub use config::Config;
pub use radio::LinkHandle;
pub use schedule::PollSchedule;
pub use source::{DemoSource, Source, uplink_state};
