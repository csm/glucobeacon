//! When a reading should make noise, and for how long.
//!
//! This lives in the shared crate on purpose. The display node is what actually
//! sounds the buzzer, but the gateway needs the same verdict to decide whether
//! a packet is worth a retry, and putting the rules in one place keeps the two
//! from disagreeing about whether the user is currently low.
//!
//! The engine is a pure state machine: no clock of its own, no I/O. Feed it
//! `now` and the newest reading on every tick and it tells you what changed.

use crate::glucose::Glucose;
use crate::reading::Reading;
use crate::time::{Duration, Timestamp};

/// Glucose values at which alarms fire, in mg/dL.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Thresholds {
    /// At or below this, an urgent low alarm fires.
    pub urgent_low_mgdl: u16,
    /// Below this (and above urgent low), a low alarm fires.
    pub low_mgdl: u16,
    /// Above this (and below urgent high), a high alarm fires.
    pub high_mgdl: u16,
    /// At or above this, an urgent high alarm fires.
    pub urgent_high_mgdl: u16,
}

impl Thresholds {
    /// The values a Dexcom receiver ships with.
    pub const DEFAULT: Thresholds = Thresholds {
        urgent_low_mgdl: 55,
        low_mgdl: 70,
        high_mgdl: 180,
        urgent_high_mgdl: 250,
    };

    /// Checks that the four thresholds are strictly increasing.
    ///
    /// Out-of-order thresholds would make bands unreachable — a low alarm above
    /// the high threshold can never fire — so configuration is rejected rather
    /// than silently ignored.
    pub const fn validate(&self) -> Result<(), ThresholdError> {
        if self.urgent_low_mgdl >= self.low_mgdl {
            Err(ThresholdError::UrgentLowNotBelowLow)
        } else if self.low_mgdl >= self.high_mgdl {
            Err(ThresholdError::LowNotBelowHigh)
        } else if self.high_mgdl >= self.urgent_high_mgdl {
            Err(ThresholdError::HighNotBelowUrgentHigh)
        } else {
            Ok(())
        }
    }
}

impl Default for Thresholds {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Why a [`Thresholds`] set was rejected.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum ThresholdError {
    /// `urgent_low_mgdl` was not strictly below `low_mgdl`.
    UrgentLowNotBelowLow,
    /// `low_mgdl` was not strictly below `high_mgdl`.
    LowNotBelowHigh,
    /// `high_mgdl` was not strictly below `urgent_high_mgdl`.
    HighNotBelowUrgentHigh,
}

impl core::fmt::Display for ThresholdError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let msg = match self {
            Self::UrgentLowNotBelowLow => "urgent low threshold must be below the low threshold",
            Self::LowNotBelowHigh => "low threshold must be below the high threshold",
            Self::HighNotBelowUrgentHigh => {
                "high threshold must be below the urgent high threshold"
            }
        };
        f.write_str(msg)
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ThresholdError {}

/// What kind of alarm is being raised.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AlarmKind {
    /// No fresh reading has arrived for longer than the policy allows.
    Stale,
    /// Glucose is above the high threshold.
    High,
    /// Glucose is below the low threshold.
    Low,
    /// Glucose is at or above the urgent high threshold.
    UrgentHigh,
    /// Glucose is at or below the urgent low threshold.
    UrgentLow,
}

impl AlarmKind {
    /// How badly this alarm wants attention; higher wins.
    ///
    /// Lows outrank highs at the same tier because a low turns dangerous in
    /// minutes and a high does not.
    pub const fn severity(self) -> u8 {
        match self {
            Self::Stale => 1,
            Self::High => 2,
            Self::Low => 3,
            Self::UrgentHigh => 4,
            Self::UrgentLow => 5,
        }
    }

    /// Whether this alarm calls for the loud pattern rather than the polite one.
    pub const fn is_urgent(self) -> bool {
        matches!(self, Self::UrgentLow | Self::UrgentHigh)
    }

    /// A short label for the display banner.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stale => "NO DATA",
            Self::High => "HIGH",
            Self::Low => "LOW",
            Self::UrgentHigh => "URGENT HIGH",
            Self::UrgentLow => "URGENT LOW",
        }
    }
}

/// The alarm rules: what fires, how long silence lasts, when to nag again.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AlarmPolicy {
    /// The glucose bands.
    pub thresholds: Thresholds,
    /// How far past a threshold a reading must come back before the alarm
    /// clears, in mg/dL.
    ///
    /// Without this, a reading parked on the threshold flips the buzzer on and
    /// off every five minutes.
    pub hysteresis_mgdl: u16,
    /// How old the newest reading may get before [`AlarmKind::Stale`] fires.
    pub stale_after: Duration,
    /// How long the silence button quiets an alarm for.
    pub silence_for: Duration,
    /// How often an un-silenced, still-active alarm re-announces itself.
    pub reannounce_after: Duration,
}

impl AlarmPolicy {
    /// A reasonable starting policy: stock thresholds, 5 mg/dL of hysteresis,
    /// stale after 20 minutes, 30-minute silence, re-announce every 5 minutes.
    pub const DEFAULT: AlarmPolicy = AlarmPolicy {
        thresholds: Thresholds::DEFAULT,
        hysteresis_mgdl: 5,
        stale_after: Duration::from_mins(20),
        silence_for: Duration::from_mins(30),
        reannounce_after: Duration::from_mins(5),
    };

    /// Checks the policy's thresholds. See [`Thresholds::validate`].
    pub const fn validate(&self) -> Result<(), ThresholdError> {
        self.thresholds.validate()
    }
}

impl Default for AlarmPolicy {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A change in alarm state, returned by [`AlarmEngine::update`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AlarmEvent {
    /// An alarm started where there was none.
    Raised(AlarmKind),
    /// The alarm got worse. This breaks any active silence.
    Escalated {
        /// What was alarming before.
        from: AlarmKind,
        /// What is alarming now.
        to: AlarmKind,
    },
    /// The alarm got less severe but has not cleared. Silence carries over.
    Eased {
        /// What was alarming before.
        from: AlarmKind,
        /// What is alarming now.
        to: AlarmKind,
    },
    /// The alarm ended.
    Cleared(AlarmKind),
    /// The alarm is still active, unsilenced, and it has been long enough to
    /// make noise about it again.
    Reannounce(AlarmKind),
}

impl AlarmEvent {
    /// Whether this event should drive the buzzer.
    ///
    /// [`AlarmEvent::Eased`] is deliberately silent: the situation improved, and
    /// a beep saying so trains people to ignore the beeps.
    pub const fn is_audible(self) -> bool {
        matches!(
            self,
            Self::Raised(_) | Self::Escalated { .. } | Self::Reannounce(_)
        )
    }

    /// The alarm this event concerns.
    pub const fn kind(self) -> AlarmKind {
        match self {
            Self::Raised(k) | Self::Cleared(k) | Self::Reannounce(k) => k,
            Self::Escalated { to, .. } | Self::Eased { to, .. } => to,
        }
    }
}

/// Evaluates readings against an [`AlarmPolicy`] and tracks silence.
///
/// Drive it by calling [`update`](Self::update) on every tick — including ticks
/// where no new reading arrived, which is how staleness and re-announcement get
/// noticed.
#[derive(Clone, Debug)]
pub struct AlarmEngine {
    policy: AlarmPolicy,
    active: Option<AlarmKind>,
    silenced_until: Option<Timestamp>,
    /// When the active alarm may next make noise. `None` means "at the first
    /// opportunity", which is how a just-expired silence and an explicit
    /// [`unsilence`](AlarmEngine::unsilence) both get heard right away.
    next_announce: Option<Timestamp>,
}

impl AlarmEngine {
    /// A new engine with nothing alarming.
    pub const fn new(policy: AlarmPolicy) -> Self {
        Self {
            policy,
            active: None,
            silenced_until: None,
            next_announce: None,
        }
    }

    /// The policy in force.
    pub const fn policy(&self) -> &AlarmPolicy {
        &self.policy
    }

    /// Replaces the policy, keeping any active alarm.
    ///
    /// The next [`update`](Self::update) re-evaluates against the new bands, so
    /// raising a threshold above the current reading clears the alarm on the
    /// following tick rather than immediately.
    pub fn set_policy(&mut self, policy: AlarmPolicy) {
        self.policy = policy;
    }

    /// The alarm currently in force, if any.
    pub const fn active(&self) -> Option<AlarmKind> {
        self.active
    }

    /// Whether the active alarm is currently silenced.
    pub fn is_silenced(&self, now: Timestamp) -> bool {
        self.silenced_until.is_some_and(|until| now < until)
    }

    /// How much silence is left, if the alarm is silenced.
    pub fn silence_remaining(&self, now: Timestamp) -> Option<Duration> {
        let until = self.silenced_until?;
        let remaining = until.saturating_since(now);
        (remaining > Duration::ZERO).then_some(remaining)
    }

    /// Silences the active alarm for the policy's silence window.
    ///
    /// Returns `false` if there was nothing to silence, which is what the button
    /// handler uses to decide between an acknowledging blink and ignoring the
    /// press. Escalation to a more severe alarm cancels the silence.
    pub fn silence(&mut self, now: Timestamp) -> bool {
        if self.active.is_none() {
            return false;
        }
        self.silenced_until = Some(now.saturating_add(self.policy.silence_for));
        true
    }

    /// Cancels any silence, so the active alarm sounds again on the next tick.
    pub fn unsilence(&mut self) {
        self.silenced_until = None;
        self.next_announce = None;
    }

    /// Re-evaluates against `latest` and reports what changed.
    ///
    /// Pass the newest reading known, or `None` if none has ever arrived. Call
    /// this on a timer even when nothing new came in; staleness and
    /// re-announcement are both driven by the passage of `now`.
    pub fn update(&mut self, now: Timestamp, latest: Option<&Reading>) -> Option<AlarmEvent> {
        let next = self.evaluate(now, latest);

        match (self.active, next) {
            (None, None) => None,

            (None, Some(kind)) => {
                self.active = Some(kind);
                self.silenced_until = None;
                self.next_announce = Some(now.saturating_add(self.policy.reannounce_after));
                Some(AlarmEvent::Raised(kind))
            }

            (Some(previous), None) => {
                self.active = None;
                self.silenced_until = None;
                self.next_announce = None;
                Some(AlarmEvent::Cleared(previous))
            }

            (Some(previous), Some(kind)) if previous == kind => {
                if self.is_silenced(now) {
                    return None;
                }
                if self.next_announce.is_some_and(|due| now < due) {
                    return None;
                }
                self.next_announce = Some(now.saturating_add(self.policy.reannounce_after));
                Some(AlarmEvent::Reannounce(kind))
            }

            (Some(previous), Some(kind)) => {
                self.active = Some(kind);
                self.next_announce = Some(now.saturating_add(self.policy.reannounce_after));
                if kind.severity() > previous.severity() {
                    // Getting worse overrides the user's silence: they silenced
                    // a low, not the urgent low it turned into.
                    self.silenced_until = None;
                    Some(AlarmEvent::Escalated {
                        from: previous,
                        to: kind,
                    })
                } else {
                    Some(AlarmEvent::Eased {
                        from: previous,
                        to: kind,
                    })
                }
            }
        }
    }

    /// What should be alarming right now, hysteresis included.
    fn evaluate(&self, now: Timestamp, latest: Option<&Reading>) -> Option<AlarmKind> {
        let raw = match latest {
            // Nothing has ever arrived: that is itself worth alarming about,
            // otherwise a display that never received a packet looks fine.
            None => Some(AlarmKind::Stale),
            Some(reading) if reading.age(now) >= self.policy.stale_after => {
                // Losing signal while low is worse than losing signal while
                // flat, so keep whichever verdict is more severe.
                more_severe(Some(AlarmKind::Stale), self.band(reading.glucose))
            }
            Some(reading) => self.band(reading.glucose),
        };

        self.apply_hysteresis(raw, latest)
    }

    /// Which alarm band a value falls in, ignoring history.
    fn band(&self, glucose: Glucose) -> Option<AlarmKind> {
        let t = &self.policy.thresholds;
        let mgdl = glucose.mgdl();
        if mgdl <= t.urgent_low_mgdl {
            Some(AlarmKind::UrgentLow)
        } else if mgdl < t.low_mgdl {
            Some(AlarmKind::Low)
        } else if mgdl >= t.urgent_high_mgdl {
            Some(AlarmKind::UrgentHigh)
        } else if mgdl > t.high_mgdl {
            Some(AlarmKind::High)
        } else {
            None
        }
    }

    /// Holds an active alarm until the reading has come back past its threshold
    /// by the hysteresis margin. Escalation is never held back.
    fn apply_hysteresis(
        &self,
        raw: Option<AlarmKind>,
        latest: Option<&Reading>,
    ) -> Option<AlarmKind> {
        let Some(active) = self.active else {
            return raw;
        };
        if active == AlarmKind::Stale {
            return raw;
        }
        let raw_severity = raw.map_or(0, AlarmKind::severity);
        if raw_severity >= active.severity() {
            return raw;
        }
        // De-escalating. Only allow it once the value has cleared the active
        // band's edge by the margin.
        let Some(reading) = latest else {
            return raw;
        };
        let mgdl = reading.glucose.mgdl();
        let t = &self.policy.thresholds;
        let margin = self.policy.hysteresis_mgdl;
        let recovered = match active {
            AlarmKind::UrgentLow => mgdl > t.urgent_low_mgdl.saturating_add(margin),
            AlarmKind::Low => mgdl >= t.low_mgdl.saturating_add(margin),
            AlarmKind::High => mgdl <= t.high_mgdl.saturating_sub(margin),
            AlarmKind::UrgentHigh => mgdl < t.urgent_high_mgdl.saturating_sub(margin),
            AlarmKind::Stale => true,
        };
        if recovered { raw } else { Some(active) }
    }
}

/// The more severe of two optional alarms.
const fn more_severe(a: Option<AlarmKind>, b: Option<AlarmKind>) -> Option<AlarmKind> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if b.severity() > a.severity() {
                Some(b)
            } else {
                Some(a)
            }
        }
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trend::Trend;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_unix_secs(secs)
    }

    fn reading(mgdl: u16, secs: i64) -> Reading {
        Reading::new(Glucose::from_mgdl(mgdl), Trend::Flat, at(secs))
    }

    fn engine() -> AlarmEngine {
        AlarmEngine::new(AlarmPolicy::DEFAULT)
    }

    #[test]
    fn default_policy_is_valid() {
        assert_eq!(AlarmPolicy::DEFAULT.validate(), Ok(()));
    }

    #[test]
    fn out_of_order_thresholds_are_rejected() {
        let mut t = Thresholds::DEFAULT;
        t.low_mgdl = 50;
        assert_eq!(t.validate(), Err(ThresholdError::UrgentLowNotBelowLow));

        let mut t = Thresholds::DEFAULT;
        t.high_mgdl = 60;
        assert_eq!(t.validate(), Err(ThresholdError::LowNotBelowHigh));

        let mut t = Thresholds::DEFAULT;
        t.urgent_high_mgdl = 100;
        assert_eq!(t.validate(), Err(ThresholdError::HighNotBelowUrgentHigh));
    }

    #[test]
    fn lows_outrank_highs() {
        assert!(AlarmKind::UrgentLow.severity() > AlarmKind::UrgentHigh.severity());
        assert!(AlarmKind::Low.severity() > AlarmKind::High.severity());
        assert!(AlarmKind::High.severity() > AlarmKind::Stale.severity());
    }

    #[test]
    fn in_range_readings_are_quiet() {
        let mut e = engine();
        let r = reading(110, 0);
        assert_eq!(e.update(at(0), Some(&r)), None);
        assert_eq!(e.active(), None);
    }

    #[test]
    fn band_edges_are_inclusive_on_the_urgent_side() {
        let mut e = engine();
        // 70 is the low threshold and is *in* range; 69 is low.
        assert_eq!(e.update(at(0), Some(&reading(70, 0))), None);
        assert_eq!(
            e.update(at(0), Some(&reading(69, 0))),
            Some(AlarmEvent::Raised(AlarmKind::Low))
        );

        let mut e = engine();
        // 180 is the high threshold and is in range; 181 is high.
        assert_eq!(e.update(at(0), Some(&reading(180, 0))), None);
        assert_eq!(
            e.update(at(0), Some(&reading(181, 0))),
            Some(AlarmEvent::Raised(AlarmKind::High))
        );

        // The urgent thresholds themselves alarm.
        let mut e = engine();
        assert_eq!(
            e.update(at(0), Some(&reading(55, 0))),
            Some(AlarmEvent::Raised(AlarmKind::UrgentLow))
        );
        let mut e = engine();
        assert_eq!(
            e.update(at(0), Some(&reading(250, 0))),
            Some(AlarmEvent::Raised(AlarmKind::UrgentHigh))
        );
    }

    #[test]
    fn a_raised_alarm_does_not_re_raise() {
        let mut e = engine();
        assert!(e.update(at(0), Some(&reading(60, 0))).is_some());
        assert_eq!(e.update(at(60), Some(&reading(60, 0))), None);
        assert_eq!(e.active(), Some(AlarmKind::Low));
    }

    #[test]
    fn hysteresis_keeps_a_threshold_hugging_reading_from_flapping() {
        let mut e = engine();
        assert_eq!(
            e.update(at(0), Some(&reading(68, 0))),
            Some(AlarmEvent::Raised(AlarmKind::Low))
        );
        // Back over the threshold, but not by the 5 mg/dL margin: still low.
        // (Kept inside the re-announce interval so the only event that could
        // show up here is a state change.)
        assert_eq!(e.update(at(100), Some(&reading(72, 100))), None);
        assert_eq!(e.active(), Some(AlarmKind::Low));
        // 75 clears it.
        assert_eq!(
            e.update(at(200), Some(&reading(75, 200))),
            Some(AlarmEvent::Cleared(AlarmKind::Low))
        );
        assert_eq!(e.active(), None);
    }

    #[test]
    fn hysteresis_applies_to_the_high_side_too() {
        let mut e = engine();
        assert_eq!(
            e.update(at(0), Some(&reading(190, 0))),
            Some(AlarmEvent::Raised(AlarmKind::High))
        );
        assert_eq!(e.update(at(100), Some(&reading(178, 100))), None);
        assert_eq!(
            e.update(at(200), Some(&reading(175, 200))),
            Some(AlarmEvent::Cleared(AlarmKind::High))
        );
    }

    #[test]
    fn escalation_is_immediate_and_audible() {
        let mut e = engine();
        e.update(at(0), Some(&reading(65, 0)));
        let event = e.update(at(300), Some(&reading(50, 300)));
        assert_eq!(
            event,
            Some(AlarmEvent::Escalated {
                from: AlarmKind::Low,
                to: AlarmKind::UrgentLow
            })
        );
        assert!(event.expect("escalation event").is_audible());
    }

    #[test]
    fn easing_is_reported_but_silent() {
        let mut e = engine();
        e.update(at(0), Some(&reading(48, 0)));
        // 61 is past urgent-low + margin, but still under the low threshold.
        let event = e.update(at(300), Some(&reading(61, 300)));
        assert_eq!(
            event,
            Some(AlarmEvent::Eased {
                from: AlarmKind::UrgentLow,
                to: AlarmKind::Low
            })
        );
        assert!(!event.expect("ease event").is_audible());
        assert_eq!(e.active(), Some(AlarmKind::Low));
    }

    #[test]
    fn silence_suppresses_reannouncement_until_it_expires() {
        let mut e = engine();
        e.update(at(0), Some(&reading(60, 0)));
        assert!(e.silence(at(10)));
        assert!(e.is_silenced(at(10)));

        // 30 minutes of quiet, even though re-announce is every 5.
        for t in [300, 600, 1200, 1799] {
            assert_eq!(e.update(at(t), Some(&reading(60, 0))), None, "t={t}");
        }
        assert_eq!(
            e.silence_remaining(at(1500)),
            Some(Duration::from_secs(310))
        );

        // The window closes and the alarm speaks up again.
        assert_eq!(
            e.update(at(1811), Some(&reading(60, 0))),
            Some(AlarmEvent::Reannounce(AlarmKind::Low))
        );
    }

    #[test]
    fn silencing_nothing_is_a_no_op() {
        let mut e = engine();
        e.update(at(0), Some(&reading(110, 0)));
        assert!(!e.silence(at(0)));
        assert!(!e.is_silenced(at(0)));
        assert_eq!(e.silence_remaining(at(0)), None);
    }

    #[test]
    fn escalation_breaks_silence() {
        let mut e = engine();
        e.update(at(0), Some(&reading(65, 0)));
        assert!(e.silence(at(10)));
        let event = e.update(at(300), Some(&reading(52, 300)));
        assert!(event.expect("escalation event").is_audible());
        assert!(!e.is_silenced(at(300)));
    }

    #[test]
    fn easing_keeps_the_silence() {
        let mut e = engine();
        e.update(at(0), Some(&reading(48, 0)));
        assert!(e.silence(at(10)));
        e.update(at(300), Some(&reading(61, 300)));
        assert!(e.is_silenced(at(300)), "user silenced this episode already");
    }

    #[test]
    fn clearing_resets_the_silence_so_the_next_episode_sounds() {
        let mut e = engine();
        e.update(at(0), Some(&reading(60, 0)));
        e.silence(at(10));
        assert_eq!(
            e.update(at(300), Some(&reading(120, 300))),
            Some(AlarmEvent::Cleared(AlarmKind::Low))
        );
        assert!(!e.is_silenced(at(300)));
        assert_eq!(
            e.update(at(600), Some(&reading(60, 600))),
            Some(AlarmEvent::Raised(AlarmKind::Low))
        );
        assert!(!e.is_silenced(at(600)));
    }

    #[test]
    fn reannounce_repeats_on_the_policy_interval() {
        let mut e = engine();
        e.update(at(0), Some(&reading(60, 0)));
        assert_eq!(e.update(at(299), Some(&reading(60, 0))), None);
        assert_eq!(
            e.update(at(300), Some(&reading(60, 0))),
            Some(AlarmEvent::Reannounce(AlarmKind::Low))
        );
        assert_eq!(e.update(at(400), Some(&reading(60, 0))), None);
        assert_eq!(
            e.update(at(601), Some(&reading(60, 0))),
            Some(AlarmEvent::Reannounce(AlarmKind::Low))
        );
    }

    #[test]
    fn never_having_received_a_reading_alarms() {
        let mut e = engine();
        assert_eq!(
            e.update(at(0), None),
            Some(AlarmEvent::Raised(AlarmKind::Stale))
        );
    }

    #[test]
    fn readings_go_stale_on_the_clock_alone() {
        let mut e = engine();
        let r = reading(110, 0);
        assert_eq!(e.update(at(0), Some(&r)), None);
        assert_eq!(e.update(at(1199), Some(&r)), None);
        assert_eq!(
            e.update(at(1200), Some(&r)),
            Some(AlarmEvent::Raised(AlarmKind::Stale))
        );
        // A fresh reading clears it.
        assert_eq!(
            e.update(at(1260), Some(&reading(115, 1260))),
            Some(AlarmEvent::Cleared(AlarmKind::Stale))
        );
    }

    #[test]
    fn losing_signal_while_low_keeps_the_low_alarm() {
        let mut e = engine();
        let r = reading(50, 0);
        assert_eq!(
            e.update(at(0), Some(&r)),
            Some(AlarmEvent::Raised(AlarmKind::UrgentLow))
        );
        // Twenty minutes later the reading is stale, but "urgent low and now
        // we've lost contact" must not quietly demote to "no data" — the alarm
        // keeps nagging as an urgent low.
        assert_eq!(
            e.update(at(1500), Some(&r)),
            Some(AlarmEvent::Reannounce(AlarmKind::UrgentLow))
        );
        assert_eq!(e.active(), Some(AlarmKind::UrgentLow));
    }

    #[test]
    fn policy_changes_take_effect_on_the_next_tick() {
        let mut e = engine();
        e.update(at(0), Some(&reading(175, 0)));
        assert_eq!(e.active(), None);

        let mut policy = AlarmPolicy::DEFAULT;
        policy.thresholds.high_mgdl = 140;
        assert_eq!(policy.validate(), Ok(()));
        e.set_policy(policy);

        assert_eq!(
            e.update(at(60), Some(&reading(175, 0))),
            Some(AlarmEvent::Raised(AlarmKind::High))
        );
    }

    #[test]
    fn unsilence_restores_noise_immediately() {
        let mut e = engine();
        e.update(at(0), Some(&reading(60, 0)));
        e.silence(at(0));
        assert!(e.is_silenced(at(60)));
        e.unsilence();
        assert!(!e.is_silenced(at(60)));
        assert_eq!(
            e.update(at(60), Some(&reading(60, 0))),
            Some(AlarmEvent::Reannounce(AlarmKind::Low))
        );
    }
}
