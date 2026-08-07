//! A client for the Dexcom Share service.
//!
//! Share is the private API behind the Dexcom "Followers" feature: a phone
//! running the Dexcom app uploads readings, and followers pull them back out.
//! It is not a documented, supported, or versioned API — it is what is
//! available, and this client is written to survive its quirks (two-step login,
//! `/Date(…)/` timestamps, a trend field that is sometimes a number and
//! sometimes a string, errors reported with a 500 and a JSON body).
//!
//! Only the gateway uses this crate; the display node never talks to Dexcom.

pub mod client;
pub mod error;
pub mod model;
pub mod region;

pub use client::{Credentials, DEFAULT_LOOKBACK, DEFAULT_MAX_COUNT, ShareClient};
pub use error::{Error, ErrorKind, Result};
pub use region::Region;
