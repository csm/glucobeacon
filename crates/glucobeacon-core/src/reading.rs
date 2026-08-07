//! A single CGM reading.

use crate::glucose::Glucose;
use crate::time::{Duration, Timestamp};
use crate::trend::Trend;

/// One glucose measurement from the transmitter.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Reading {
    /// The measured concentration.
    pub glucose: Glucose,
    /// The transmitter's rate-of-change arrow.
    pub trend: Trend,
    /// When the sensor took the measurement (not when we received it).
    pub at: Timestamp,
}

impl Reading {
    /// Assembles a reading.
    pub const fn new(glucose: Glucose, trend: Trend, at: Timestamp) -> Self {
        Self { glucose, trend, at }
    }

    /// How old this reading is as of `now`.
    pub const fn age(&self, now: Timestamp) -> Duration {
        now.saturating_since(self.at)
    }

    /// Whether this reading is the same measurement as `other`.
    ///
    /// The gateway polls more often than the sensor produces readings, so the
    /// same measurement comes back repeatedly; the timestamp is what makes one
    /// distinct, not the value.
    pub const fn is_same_measurement(&self, other: &Reading) -> bool {
        self.at.as_unix_secs() == other.at.as_unix_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn age_is_measured_from_the_sensor_timestamp() {
        let r = Reading::new(
            Glucose::from_mgdl(112),
            Trend::Flat,
            Timestamp::from_unix_secs(1_000),
        );
        assert_eq!(r.age(Timestamp::from_unix_secs(1_300)).as_mins(), 5);
    }

    #[test]
    fn repeated_polls_of_one_measurement_compare_equal() {
        let at = Timestamp::from_unix_secs(1_000);
        let a = Reading::new(Glucose::from_mgdl(112), Trend::Flat, at);
        let b = Reading::new(Glucose::from_mgdl(112), Trend::FortyFiveUp, at);
        assert!(a.is_same_measurement(&b));

        let c = Reading::new(
            Glucose::from_mgdl(112),
            Trend::Flat,
            Timestamp::from_unix_secs(1_300),
        );
        assert!(!a.is_same_measurement(&c));
    }
}
