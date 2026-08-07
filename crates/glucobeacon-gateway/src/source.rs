//! Where readings come from.
//!
//! Two implementations: the real Dexcom Share client, and a synthetic source
//! that walks glucose through a slow wave. The demo source exists so the whole
//! system — link, framing, display, alarms, buzzer — can be exercised without a
//! Dexcom account and without waiting five minutes between interesting events.

use glucobeacon_core::{Glucose, Reading, Timestamp, Trend};
use glucobeacon_dexcom::{Error as DexcomError, ErrorKind, HttpTransport, ShareClient};
use glucobeacon_proto::UplinkState;

/// A source of CGM readings.
///
/// Generic over the HTTP transport so the same relay logic serves both the
/// workstation build, which uses `reqwest`, and an ESP-IDF build, which uses
/// the client ESP-IDF already links.
#[derive(Debug)]
pub enum Source<T> {
    /// The real thing.
    Share(Box<ShareClient<T>>),
    /// A synthetic waveform, for development.
    Demo(DemoSource),
}

impl<T: HttpTransport> Source<T> {
    /// Fetches any readings newer than `since`, oldest first.
    pub async fn poll(&mut self, now: Timestamp, since: Option<Timestamp>) -> PollResult {
        match self {
            Self::Share(client) => {
                let lookback = glucobeacon_dexcom::DEFAULT_LOOKBACK;
                match client
                    .readings(lookback, glucobeacon_dexcom::DEFAULT_MAX_COUNT)
                    .await
                {
                    Ok(readings) => Ok(newer_than(readings, since)),
                    Err(error) => Err(error),
                }
            }
            Self::Demo(demo) => Ok(newer_than(demo.readings_through(now), since)),
        }
    }

    /// A human-readable name for logs.
    pub fn description(&self) -> String {
        match self {
            Self::Share(client) => format!("Dexcom Share ({})", client.region()),
            Self::Demo(_) => "demo waveform".to_owned(),
        }
    }
}

/// What a poll produced.
pub type PollResult = Result<Vec<Reading>, DexcomError>;

/// Maps a poll outcome to the state the display should show.
pub fn uplink_state(result: &PollResult) -> UplinkState {
    match result {
        Ok(readings) if readings.is_empty() => UplinkState::NoRecentData,
        Ok(_) => UplinkState::Ok,
        Err(error) => match error.kind() {
            ErrorKind::Network => UplinkState::NoNetwork,
            ErrorKind::Credentials => UplinkState::AuthFailed,
            ErrorKind::Service | ErrorKind::Protocol => UplinkState::Unreachable,
        },
    }
}

fn newer_than(readings: Vec<Reading>, since: Option<Timestamp>) -> Vec<Reading> {
    match since {
        Some(since) => readings
            .into_iter()
            .filter(|reading| reading.at > since)
            .collect(),
        None => readings,
    }
}

/// A synthetic CGM that swings through the whole interesting range.
///
/// Deterministic: given the same start time and the same `now`, it produces the
/// same readings. That makes the demo reproducible, which matters when the
/// thing being demonstrated is an alarm.
#[derive(Clone, Debug)]
pub struct DemoSource {
    started_at: Timestamp,
    interval_secs: i64,
    period_secs: f32,
    center_mgdl: f32,
    amplitude_mgdl: f32,
}

impl DemoSource {
    /// A source whose wave starts at `started_at`.
    ///
    /// The defaults sweep 40–260 mg/dL over 45 minutes, which crosses every
    /// alarm threshold in both directions inside a coffee break.
    ///
    /// Readings come once a minute rather than the real sensor's five, because
    /// a sweep fast enough to watch moves more than the width of the low band
    /// in five minutes — and a demo that skips straight past "low" into "urgent
    /// low" would not exercise the thing most worth exercising.
    pub fn new(started_at: Timestamp) -> Self {
        Self {
            started_at,
            interval_secs: 60,
            period_secs: 45.0 * 60.0,
            center_mgdl: 150.0,
            amplitude_mgdl: 110.0,
        }
    }

    /// Overrides how long a full high-to-low-to-high sweep takes.
    pub fn with_period(mut self, period: std::time::Duration) -> Self {
        self.period_secs = period.as_secs_f32().max(1.0);
        self
    }

    /// Overrides the gap between synthetic readings.
    pub fn with_interval(mut self, interval: std::time::Duration) -> Self {
        self.interval_secs = (interval.as_secs() as i64).max(1);
        self
    }

    /// Every reading on the grid at or before `now`, oldest first.
    ///
    /// Backfills a few intervals, mirroring the lookback window the real Share
    /// client uses, so a restart does not replay hours of history over a link
    /// with a 200-byte packet budget.
    pub fn readings_through(&self, now: Timestamp) -> Vec<Reading> {
        const BACKFILL_INTERVALS: i64 = 4;

        let latest = self.grid_point(now);
        let earliest = latest.saturating_sub(BACKFILL_INTERVALS * self.interval_secs);

        let mut readings = Vec::new();
        let mut t = earliest.max(self.grid_point(self.started_at));
        while t <= latest {
            readings.push(self.reading_at(Timestamp::from_unix_secs(t)));
            t = t.saturating_add(self.interval_secs);
        }
        readings
    }

    /// The reading the synthetic sensor would have produced at `at`.
    pub fn reading_at(&self, at: Timestamp) -> Reading {
        let elapsed = (at.as_unix_secs() - self.started_at.as_unix_secs()) as f32;
        let phase = core::f32::consts::TAU * elapsed / self.period_secs;
        let value = self.center_mgdl + self.amplitude_mgdl * phase.sin();
        let glucose = Glucose::from_mgdl(value.clamp(39.0, 401.0) as u16);

        // Derive the arrow from the slope of the wave, the same way the
        // transmitter derives it from consecutive readings.
        let previous_at = Timestamp::from_unix_secs(at.as_unix_secs() - self.interval_secs);
        let previous = self.raw_value(previous_at);
        let per_minute = (value - previous) / (self.interval_secs as f32 / 60.0);

        Reading::new(glucose, trend_for_rate(per_minute), at)
    }

    fn raw_value(&self, at: Timestamp) -> f32 {
        let elapsed = (at.as_unix_secs() - self.started_at.as_unix_secs()) as f32;
        let phase = core::f32::consts::TAU * elapsed / self.period_secs;
        self.center_mgdl + self.amplitude_mgdl * phase.sin()
    }

    /// Rounds `at` down to the reading grid.
    fn grid_point(&self, at: Timestamp) -> i64 {
        at.as_unix_secs().div_euclid(self.interval_secs) * self.interval_secs
    }
}

/// The arrow a transmitter would report for a rate of change in mg/dL/min.
fn trend_for_rate(per_minute: f32) -> Trend {
    match per_minute {
        r if r >= 3.0 => Trend::DoubleUp,
        r if r >= 2.0 => Trend::SingleUp,
        r if r >= 1.0 => Trend::FortyFiveUp,
        r if r > -1.0 => Trend::Flat,
        r if r > -2.0 => Trend::FortyFiveDown,
        r if r > -3.0 => Trend::SingleDown,
        _ => Trend::DoubleDown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_unix_secs(secs)
    }

    fn demo() -> DemoSource {
        DemoSource::new(at(0))
    }

    #[test]
    fn readings_land_on_the_interval_grid() {
        let source = demo();
        let readings = source.readings_through(at(1_000));
        assert!(!readings.is_empty());
        for reading in &readings {
            assert_eq!(
                reading.at.as_unix_secs() % source.interval_secs,
                0,
                "{reading:?}"
            );
        }
    }

    #[test]
    fn a_restart_does_not_replay_hours_of_history() {
        assert!(demo().readings_through(at(86_400)).len() <= 8);
    }

    #[test]
    fn readings_are_ordered_oldest_first_and_stop_at_now() {
        let readings = demo().readings_through(at(1_000));
        assert!(readings.windows(2).all(|w| w[0].at < w[1].at));
        assert!(readings.last().expect("some readings").at <= at(1_000));
    }

    #[test]
    fn the_waveform_is_deterministic() {
        assert_eq!(demo().reading_at(at(600)), demo().reading_at(at(600)));
    }

    #[test]
    fn the_sweep_crosses_every_alarm_threshold() {
        use glucobeacon_core::{AlarmEngine, AlarmKind, AlarmPolicy};

        let source = demo();
        let mut engine = AlarmEngine::new(AlarmPolicy::DEFAULT);
        let mut seen = std::collections::HashSet::new();

        // One full period, plus a little, at the reading interval.
        for step in 0..=(45 * 60 / source.interval_secs + 1) {
            let t = at(step * source.interval_secs);
            let reading = source.reading_at(t);
            engine.update(t, Some(&reading));
            if let Some(kind) = engine.active() {
                seen.insert(kind);
            }
        }

        for kind in [
            AlarmKind::UrgentLow,
            AlarmKind::Low,
            AlarmKind::High,
            AlarmKind::UrgentHigh,
        ] {
            assert!(
                seen.contains(&kind),
                "demo never reached {kind:?}: {seen:?}"
            );
        }
    }

    #[test]
    fn values_stay_inside_the_sensor_range() {
        let source = demo();
        for step in 0..200 {
            let mgdl = source.reading_at(at(step * 300)).glucose.mgdl();
            assert!((39..=401).contains(&mgdl), "step {step} gave {mgdl}");
        }
    }

    #[test]
    fn arrows_follow_the_slope() {
        let source = demo();
        let quarter = 45 * 60 / 4;

        // Trends are a backward difference, the same way a transmitter derives
        // them, so the arrow reflects how the value got here — not where it is
        // headed. Rising into the peak, falling out of it.
        let climbing = source.reading_at(at(quarter - 300)).trend;
        let falling = source.reading_at(at(quarter + 300)).trend;

        assert!(
            matches!(
                climbing,
                Trend::FortyFiveUp | Trend::SingleUp | Trend::DoubleUp
            ),
            "before the peak the arrow should point up, got {climbing:?}"
        );
        assert!(
            matches!(
                falling,
                Trend::FortyFiveDown | Trend::SingleDown | Trend::DoubleDown
            ),
            "after the peak the arrow should point down, got {falling:?}"
        );
    }

    #[test]
    fn the_sweep_produces_a_flat_arrow_somewhere() {
        let source = demo();
        let flat = (0..(45 * 60 / source.interval_secs))
            .any(|step| source.reading_at(at(step * source.interval_secs)).trend == Trend::Flat);
        assert!(flat, "the turning points should read as flat");
    }

    #[test]
    fn a_custom_period_and_interval_are_honoured() {
        let source = demo()
            .with_period(Duration::from_secs(600))
            .with_interval(Duration::from_secs(60));
        let readings = source.readings_through(at(600));
        assert!(
            readings
                .windows(2)
                .all(|w| { w[1].at.as_unix_secs() - w[0].at.as_unix_secs() == 60 })
        );
    }

    #[test]
    fn a_degenerate_period_does_not_divide_by_zero() {
        let source = demo().with_period(Duration::from_secs(0));
        let mgdl = source.reading_at(at(300)).glucose.mgdl();
        assert!((39..=401).contains(&mgdl));
    }

    #[test]
    fn polling_filters_out_readings_already_sent() {
        let all = demo().readings_through(at(600));
        let cutoff = all[all.len() - 2].at;
        let fresh = newer_than(all.clone(), Some(cutoff));
        assert_eq!(fresh.len(), 1);
        assert!(fresh[0].at > cutoff);

        assert_eq!(newer_than(all.clone(), None).len(), all.len());
    }

    #[test]
    fn rate_thresholds_map_to_the_expected_arrows() {
        assert_eq!(trend_for_rate(5.0), Trend::DoubleUp);
        assert_eq!(trend_for_rate(2.5), Trend::SingleUp);
        assert_eq!(trend_for_rate(1.5), Trend::FortyFiveUp);
        assert_eq!(trend_for_rate(0.0), Trend::Flat);
        assert_eq!(trend_for_rate(-1.5), Trend::FortyFiveDown);
        assert_eq!(trend_for_rate(-2.5), Trend::SingleDown);
        assert_eq!(trend_for_rate(-5.0), Trend::DoubleDown);
    }

    #[test]
    fn an_empty_successful_poll_reads_as_no_sensor_data() {
        let result: PollResult = Ok(Vec::new());
        assert_eq!(uplink_state(&result), UplinkState::NoRecentData);
    }

    #[test]
    fn a_successful_poll_with_data_is_healthy() {
        let result: PollResult = Ok(demo().readings_through(at(1_000)));
        assert_eq!(uplink_state(&result), UplinkState::Ok);
    }

    #[test]
    fn credential_failures_are_reported_distinctly() {
        let result: PollResult = Err(DexcomError::Credentials {
            code: "SSO_AuthenticatePasswordInvalid".to_owned(),
            message: String::new(),
        });
        assert_eq!(uplink_state(&result), UplinkState::AuthFailed);
        assert!(uplink_state(&result).needs_attention());
    }

    #[test]
    fn service_failures_do_not_look_like_a_bad_password() {
        let result: PollResult = Err(DexcomError::Service {
            code: "500".to_owned(),
            message: "boom".to_owned(),
        });
        assert_eq!(uplink_state(&result), UplinkState::Unreachable);
        assert!(!uplink_state(&result).needs_attention());
    }
}
