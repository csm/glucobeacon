//! A minimal wall-clock representation.
//!
//! The display node has no RTC battery and no NTP; it learns the time from the
//! gateway, which stamps every reading. So "now" on the display side is
//! "whatever the last packet said, plus the milliseconds we have counted since"
//! — and all this module needs to support is comparison and subtraction.

/// A span of whole seconds.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Duration {
    secs: u32,
}

impl Duration {
    /// Zero seconds.
    pub const ZERO: Duration = Duration { secs: 0 };

    /// A span of `secs` seconds.
    pub const fn from_secs(secs: u32) -> Self {
        Self { secs }
    }

    /// A span of `mins` minutes, saturating at [`u32::MAX`] seconds.
    pub const fn from_mins(mins: u32) -> Self {
        Self {
            secs: mins.saturating_mul(60),
        }
    }

    /// The span in whole seconds.
    pub const fn as_secs(self) -> u32 {
        self.secs
    }

    /// The span in whole minutes, truncated.
    pub const fn as_mins(self) -> u32 {
        self.secs / 60
    }
}

impl core::ops::Add for Duration {
    type Output = Duration;

    fn add(self, rhs: Duration) -> Duration {
        Duration {
            secs: self.secs.saturating_add(rhs.secs),
        }
    }
}

impl core::ops::Sub for Duration {
    type Output = Duration;

    /// Saturating: durations are unsigned, so an underflow becomes zero.
    fn sub(self, rhs: Duration) -> Duration {
        Duration {
            secs: self.secs.saturating_sub(rhs.secs),
        }
    }
}

/// Seconds since the Unix epoch.
///
/// Signed, so pre-1970 values and differences both fit; the gateway only ever
/// sends present-day timestamps.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Timestamp {
    unix_secs: i64,
}

impl Timestamp {
    /// The Unix epoch.
    pub const EPOCH: Timestamp = Timestamp { unix_secs: 0 };

    /// A timestamp `unix_secs` seconds after the Unix epoch.
    pub const fn from_unix_secs(unix_secs: i64) -> Self {
        Self { unix_secs }
    }

    /// A timestamp from Unix milliseconds, truncating toward the epoch.
    pub const fn from_unix_millis(unix_millis: i64) -> Self {
        Self {
            unix_secs: unix_millis.div_euclid(1000),
        }
    }

    /// Seconds since the Unix epoch.
    pub const fn as_unix_secs(self) -> i64 {
        self.unix_secs
    }

    /// How long after `earlier` this timestamp is, or [`Duration::ZERO`] if it
    /// is not after it at all.
    ///
    /// Clock skew between the gateway and the display node can put a reading
    /// slightly in the future; saturating keeps that from reading as a
    /// staleness alarm's worth of elapsed time.
    pub const fn saturating_since(self, earlier: Timestamp) -> Duration {
        let delta = self.unix_secs - earlier.unix_secs;
        if delta <= 0 {
            Duration::ZERO
        } else if delta > u32::MAX as i64 {
            Duration::from_secs(u32::MAX)
        } else {
            Duration::from_secs(delta as u32)
        }
    }

    /// This timestamp advanced by `duration`.
    pub const fn saturating_add(self, duration: Duration) -> Timestamp {
        Timestamp {
            unix_secs: self.unix_secs.saturating_add(duration.as_secs() as i64),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_saturates_instead_of_wrapping() {
        let t0 = Timestamp::from_unix_secs(1_700_000_000);
        let t1 = Timestamp::from_unix_secs(1_700_000_300);
        assert_eq!(t1.saturating_since(t0), Duration::from_secs(300));
        // A reading stamped in the future reads as "no time has passed".
        assert_eq!(t0.saturating_since(t1), Duration::ZERO);
    }

    #[test]
    fn millis_truncate_toward_the_epoch() {
        assert_eq!(
            Timestamp::from_unix_millis(1_700_000_000_999).as_unix_secs(),
            1_700_000_000
        );
        assert_eq!(Timestamp::from_unix_millis(-1).as_unix_secs(), -1);
    }

    #[test]
    fn duration_arithmetic_saturates_at_both_ends() {
        assert_eq!(
            Duration::from_secs(60) + Duration::from_secs(30),
            Duration::from_secs(90)
        );
        assert_eq!(
            Duration::from_secs(30) - Duration::from_secs(60),
            Duration::ZERO
        );
        assert_eq!(
            Duration::from_secs(u32::MAX) + Duration::from_secs(1),
            Duration::from_secs(u32::MAX)
        );
    }

    #[test]
    fn durations_convert() {
        assert_eq!(Duration::from_mins(5).as_secs(), 300);
        assert_eq!(Duration::from_secs(359).as_mins(), 5);
        assert_eq!(Duration::from_mins(u32::MAX).as_secs(), u32::MAX);
    }
}
