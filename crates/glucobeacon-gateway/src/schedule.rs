//! When to poll Dexcom next.
//!
//! Polling on a fixed timer is the obvious approach and the wrong one: the
//! sensor produces a reading every five minutes on *its* phase, so a fixed
//! timer spends most of its requests asking for data that already arrived, and
//! delivers each new reading up to five minutes late. Instead the schedule
//! tracks the phase of the last reading and aims just past when the next one is
//! due.
//!
//! Pure arithmetic, no clock and no I/O, so all of it is testable.

use std::time::Duration as StdDuration;

use glucobeacon_core::Timestamp;

use crate::config::PollConfig;

/// Computes the delay before the next poll.
#[derive(Copy, Clone, Debug)]
pub struct PollSchedule {
    config: PollConfig,
}

impl PollSchedule {
    /// A schedule following `config`.
    pub const fn new(config: PollConfig) -> Self {
        Self { config }
    }

    /// How long to wait before polling again.
    ///
    /// `last_reading_at` is the sensor timestamp of the newest reading known,
    /// not when it was fetched. `consecutive_failures` counts polls that
    /// errored; a poll that succeeds but returns nothing new is not a failure,
    /// it just means the reading is running late.
    pub fn next_delay(
        &self,
        now: Timestamp,
        last_reading_at: Option<Timestamp>,
        consecutive_failures: u32,
    ) -> StdDuration {
        if consecutive_failures > 0 {
            return self.backoff(consecutive_failures);
        }

        let floor = self.config.min_interval_secs;
        let Some(last) = last_reading_at else {
            // Nothing has ever arrived. Ask again soon; the sensor may be
            // warming up or the phone may be out of range.
            return secs(floor);
        };

        let due_at = last
            .as_unix_secs()
            .saturating_add(i64::from(self.config.sensor_interval_secs))
            .saturating_add(i64::from(self.config.settle_secs));

        let wait = due_at.saturating_sub(now.as_unix_secs());
        // The next reading is already overdue: check back at the floor rate
        // rather than hammering.
        let wait = u32::try_from(wait).unwrap_or(0);
        secs(wait.max(floor))
    }

    /// Exponential backoff, capped, for consecutive failures.
    fn backoff(&self, consecutive_failures: u32) -> StdDuration {
        let shift = consecutive_failures.saturating_sub(1).min(16);
        let delay = self
            .config
            .retry_base_secs
            .saturating_mul(1u32 << shift)
            .min(self.config.retry_max_secs);
        secs(delay.max(1))
    }

    /// How long between status heartbeats.
    pub fn heartbeat_interval(&self) -> StdDuration {
        secs(self.config.heartbeat_secs.max(1))
    }
}

const fn secs(value: u32) -> StdDuration {
    StdDuration::from_secs(value as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule() -> PollSchedule {
        PollSchedule::new(PollConfig::default())
    }

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_unix_secs(secs)
    }

    #[test]
    fn without_any_reading_it_checks_back_at_the_floor_rate() {
        assert_eq!(schedule().next_delay(at(0), None, 0), secs(30));
    }

    #[test]
    fn it_aims_just_past_when_the_next_reading_is_due() {
        // A reading landed at t=1000. The next is due at 1300, and we wait 20s
        // past that for the phone to upload it.
        let delay = schedule().next_delay(at(1000), Some(at(1000)), 0);
        assert_eq!(delay, secs(320));
    }

    #[test]
    fn a_reading_fetched_late_shortens_the_wait_rather_than_shifting_the_phase() {
        // We polled at 1200 and got the reading stamped 1000. The next reading
        // is still due at 1300, so wait 120s — not another full interval.
        let delay = schedule().next_delay(at(1200), Some(at(1000)), 0);
        assert_eq!(delay, secs(120));
    }

    #[test]
    fn an_overdue_reading_is_retried_at_the_floor_rate_not_immediately() {
        // Well past when the reading should have shown up: the sensor is late,
        // the phone is out of range, something. Do not spin.
        let delay = schedule().next_delay(at(2000), Some(at(1000)), 0);
        assert_eq!(delay, secs(30));
    }

    #[test]
    fn the_floor_always_applies() {
        // Even exactly at the due moment, never poll faster than the floor.
        let delay = schedule().next_delay(at(1320), Some(at(1000)), 0);
        assert_eq!(delay, secs(30));
    }

    #[test]
    fn failures_back_off_exponentially_up_to_the_cap() {
        let s = schedule();
        let delays: Vec<u64> = (1..=8)
            .map(|failures| s.next_delay(at(0), Some(at(0)), failures).as_secs())
            .collect();
        assert_eq!(delays, vec![30, 60, 120, 240, 300, 300, 300, 300]);
    }

    #[test]
    fn backoff_does_not_overflow_after_a_very_long_outage() {
        let s = schedule();
        for failures in [30u32, 1_000, u32::MAX] {
            let delay = s.next_delay(at(0), Some(at(0)), failures);
            assert_eq!(delay, secs(300), "failures={failures}");
        }
    }

    #[test]
    fn a_pathological_clock_does_not_produce_a_zero_delay() {
        // A reading stamped far in the future must not make the delay negative
        // and wrap; and one stamped at the epoch must not make it zero.
        let s = schedule();
        assert!(s.next_delay(at(0), Some(at(i64::MAX)), 0) >= secs(30));
        assert!(s.next_delay(at(i64::MAX), Some(at(0)), 0) >= secs(30));
    }

    #[test]
    fn a_zero_retry_base_still_waits_a_moment() {
        let config = PollConfig {
            retry_base_secs: 0,
            retry_max_secs: 0,
            ..PollConfig::default()
        };
        assert_eq!(
            PollSchedule::new(config).next_delay(at(0), None, 3),
            secs(1)
        );
    }
}
