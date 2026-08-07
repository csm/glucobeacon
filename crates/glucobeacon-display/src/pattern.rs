//! Turning a buzzer pattern or an LED state into something to do right now.
//!
//! [`crate::hal::BuzzerPattern`] says *what* to play; this says whether the pin
//! should be high at this instant. Keeping it here rather than in the firmware
//! means the timing is testable, and means a chirp sounds the same on the
//! bench as it does on the device.
//!
//! The firmware's job reduces to calling [`Player::output`] each time round its
//! loop and driving a GPIO or a PWM channel with the answer.

use crate::hal::{BuzzerPattern, LedState};

/// One leg of a pattern: on or off, for this long.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Step {
    /// Whether the output is asserted during this step.
    pub on: bool,
    /// How long the step lasts, in milliseconds.
    pub millis: u32,
}

impl Step {
    /// A step.
    pub const fn new(on: bool, millis: u32) -> Self {
        Self { on, millis }
    }
}

/// A repeating on/off sequence.
///
/// An empty timeline is permanently off; a single `on` step is permanently on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Timeline {
    steps: &'static [Step],
}

impl Timeline {
    /// A timeline over `steps`, repeating forever.
    pub const fn new(steps: &'static [Step]) -> Self {
        Self { steps }
    }

    /// Total length of one cycle in milliseconds.
    pub fn period_millis(&self) -> u32 {
        self.steps.iter().map(|step| step.millis).sum()
    }

    /// Whether the output is asserted `elapsed_millis` into the pattern.
    pub fn output_at(&self, elapsed_millis: u64) -> bool {
        let period = u64::from(self.period_millis());
        if period == 0 {
            // No steps, or every step is zero-length: nothing to play.
            return false;
        }
        let mut offset = elapsed_millis % period;
        for step in self.steps {
            let millis = u64::from(step.millis);
            if offset < millis {
                return step.on;
            }
            offset -= millis;
        }
        // Unreachable: the offset is under the total by construction.
        false
    }

    /// The fraction of a cycle spent asserted, in percent.
    ///
    /// Useful as a sanity check that a more urgent pattern is in fact more
    /// insistent than a less urgent one.
    pub fn duty_percent(&self) -> u32 {
        let period = self.period_millis();
        if period == 0 {
            return 0;
        }
        let on: u32 = self
            .steps
            .iter()
            .filter(|step| step.on)
            .map(|step| step.millis)
            .sum();
        on * 100 / period
    }
}

const QUIET: [Step; 0] = [];

/// One short beep every three seconds: noticeable, not alarming.
const CHIRP: [Step; 2] = [Step::new(true, 80), Step::new(false, 2_920)];

/// Two beeps every two seconds.
const DOUBLE_BEEP: [Step; 4] = [
    Step::new(true, 120),
    Step::new(false, 120),
    Step::new(true, 120),
    Step::new(false, 1_640),
];

/// Three hard beeps a second and a bit: meant to wake someone.
const URGENT: [Step; 6] = [
    Step::new(true, 200),
    Step::new(false, 100),
    Step::new(true, 200),
    Step::new(false, 100),
    Step::new(true, 200),
    Step::new(false, 400),
];

const LED_SOLID: [Step; 1] = [Step::new(true, 1_000)];
const LED_SLOW: [Step; 2] = [Step::new(true, 500), Step::new(false, 500)];
const LED_FAST: [Step; 2] = [Step::new(true, 150), Step::new(false, 150)];

impl BuzzerPattern {
    /// The on/off sequence for this pattern.
    pub const fn timeline(self) -> Timeline {
        Timeline::new(match self {
            Self::Quiet => &QUIET,
            Self::Chirp => &CHIRP,
            Self::DoubleBeep => &DOUBLE_BEEP,
            Self::Urgent => &URGENT,
        })
    }
}

impl LedState {
    /// The on/off sequence for this state.
    pub const fn timeline(self) -> Timeline {
        Timeline::new(match self {
            Self::Off => &QUIET,
            Self::Solid => &LED_SOLID,
            Self::SlowBlink => &LED_SLOW,
            Self::FastBlink => &LED_FAST,
        })
    }
}

/// Plays a timeline against a millisecond clock.
///
/// Restarting on every change is deliberate: a pattern that changed mid-cycle
/// and carried its phase over would clip its first beep, and a clipped alarm
/// sounds like a fault.
#[derive(Copy, Clone, Debug)]
pub struct Player {
    timeline: Timeline,
    started_millis: u64,
}

impl Player {
    /// A player showing nothing.
    pub const fn new() -> Self {
        Self {
            timeline: Timeline::new(&QUIET),
            started_millis: 0,
        }
    }

    /// Starts `timeline` from the beginning.
    pub fn play(&mut self, timeline: Timeline, now_millis: u64) {
        self.timeline = timeline;
        self.started_millis = now_millis;
    }

    /// Whether the output should be asserted now.
    pub fn output(&self, now_millis: u64) -> bool {
        let elapsed = now_millis.saturating_sub(self.started_millis);
        self.timeline.output_at(elapsed)
    }

    /// The timeline being played.
    pub const fn timeline(&self) -> Timeline {
        self.timeline
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_is_silent_forever() {
        let timeline = BuzzerPattern::Quiet.timeline();
        assert_eq!(timeline.period_millis(), 0);
        for t in [0, 1, 1_000, u64::MAX] {
            assert!(!timeline.output_at(t), "t={t}");
        }
        assert_eq!(timeline.duty_percent(), 0);
    }

    #[test]
    fn a_solid_led_is_always_lit() {
        let timeline = LedState::Solid.timeline();
        for t in [0, 500, 999, 1_000, 10_000] {
            assert!(timeline.output_at(t), "t={t}");
        }
        assert_eq!(timeline.duty_percent(), 100);
    }

    #[test]
    fn a_chirp_beeps_briefly_then_waits() {
        let timeline = BuzzerPattern::Chirp.timeline();
        assert!(timeline.output_at(0));
        assert!(timeline.output_at(79));
        assert!(!timeline.output_at(80));
        assert!(!timeline.output_at(2_999));
        // And repeats.
        assert!(timeline.output_at(3_000));
        assert_eq!(timeline.period_millis(), 3_000);
    }

    #[test]
    fn a_double_beep_beeps_twice_per_cycle() {
        let timeline = BuzzerPattern::DoubleBeep.timeline();
        let transitions = count_transitions(timeline);
        assert_eq!(transitions, 4, "two beeps means four edges per cycle");
    }

    #[test]
    fn urgency_shows_up_as_more_noise() {
        let chirp = BuzzerPattern::Chirp.timeline().duty_percent();
        let double = BuzzerPattern::DoubleBeep.timeline().duty_percent();
        let urgent = BuzzerPattern::Urgent.timeline().duty_percent();
        assert!(
            urgent > double && double > chirp,
            "chirp {chirp}%, double {double}%, urgent {urgent}%"
        );
    }

    #[test]
    fn a_fast_blink_is_faster_than_a_slow_one() {
        assert!(
            LedState::FastBlink.timeline().period_millis()
                < LedState::SlowBlink.timeline().period_millis()
        );
        // Both spend half their time lit; only the rate differs.
        assert_eq!(LedState::FastBlink.timeline().duty_percent(), 50);
        assert_eq!(LedState::SlowBlink.timeline().duty_percent(), 50);
    }

    #[test]
    fn patterns_repeat_exactly() {
        for pattern in [
            BuzzerPattern::Chirp,
            BuzzerPattern::DoubleBeep,
            BuzzerPattern::Urgent,
        ] {
            let timeline = pattern.timeline();
            let period = u64::from(timeline.period_millis());
            for offset in [0, 1, 37, period - 1] {
                assert_eq!(
                    timeline.output_at(offset),
                    timeline.output_at(offset + period * 9),
                    "{pattern:?} at {offset}"
                );
            }
        }
    }

    #[test]
    fn a_player_starts_its_pattern_from_the_beginning() {
        let mut player = Player::new();
        // Switching patterns mid-cycle must not clip the first beep.
        player.play(BuzzerPattern::Urgent.timeline(), 12_345);
        assert!(player.output(12_345));
        assert!(player.output(12_345 + 199));
        assert!(!player.output(12_345 + 200));
    }

    #[test]
    fn a_player_that_is_asked_about_the_past_does_not_wrap() {
        let mut player = Player::new();
        player.play(BuzzerPattern::Urgent.timeline(), 1_000);
        // A clock that went backwards clamps to the start rather than
        // landing at a random point in the pattern.
        assert!(player.output(0));
        assert!(player.output(500));
    }

    #[test]
    fn silencing_takes_effect_immediately() {
        let mut player = Player::new();
        player.play(BuzzerPattern::Urgent.timeline(), 0);
        assert!(player.output(0));
        player.play(BuzzerPattern::Quiet.timeline(), 50);
        assert!(!player.output(50));
        assert!(!player.output(5_000));
    }

    /// Edges in one cycle, sampled at millisecond resolution.
    fn count_transitions(timeline: Timeline) -> u32 {
        let period = timeline.period_millis();
        let mut edges = 0;
        let mut previous = timeline.output_at(0);
        for t in 1..u64::from(period) {
            let current = timeline.output_at(t);
            if current != previous {
                edges += 1;
                previous = current;
            }
        }
        // The wrap back to the start counts too.
        if timeline.output_at(0) != previous {
            edges += 1;
        }
        edges
    }
}
