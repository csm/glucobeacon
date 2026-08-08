//! Turning a buzzer pattern or an LED state into something to do right now.
//!
//! [`crate::hal::BuzzerPattern`] says *what* to play; this says whether the pin
//! should be high at this instant. Keeping it here rather than in the firmware
//! means the timing is testable, and means a chirp sounds the same on the
//! bench as it does on the device.
//!
//! The firmware's job reduces to calling [`Player::output`] each time round its
//! loop and driving a GPIO or a PWM channel with the answer.

use glucobeacon_core::Duration;

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

/// An on/off sequence, played either a fixed number of times or forever.
///
/// An empty timeline is permanently off; a single `on` step that repeats
/// forever is permanently on.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Timeline {
    steps: &'static [Step],
    /// How many times to play `steps`, or `None` to repeat forever.
    cycles: Option<u32>,
}

impl Timeline {
    /// A timeline over `steps`, repeating forever.
    pub const fn new(steps: &'static [Step]) -> Self {
        Self {
            steps,
            cycles: None,
        }
    }

    /// A timeline over `steps` that plays `cycles` times and then falls silent.
    ///
    /// This is what makes an alarm a burst rather than a siren. Ending the
    /// pattern here rather than by cutting it off from outside means it always
    /// stops on a step boundary — a beep chopped in half sounds like a fault.
    pub const fn burst(steps: &'static [Step], cycles: u32) -> Self {
        Self {
            steps,
            cycles: Some(cycles),
        }
    }

    /// Total length of one cycle in milliseconds.
    pub fn period_millis(&self) -> u32 {
        self.steps.iter().map(|step| step.millis).sum()
    }

    /// How long the whole timeline lasts, or `None` if it repeats forever.
    pub fn total_millis(&self) -> Option<u32> {
        Some(self.period_millis().saturating_mul(self.cycles?))
    }

    /// Whether the output is asserted `elapsed_millis` into the pattern.
    pub fn output_at(&self, elapsed_millis: u64) -> bool {
        let period = u64::from(self.period_millis());
        if period == 0 {
            // No steps, or every step is zero-length: nothing to play.
            return false;
        }
        if self
            .total_millis()
            .is_some_and(|total| elapsed_millis >= u64::from(total))
        {
            // The burst has played out.
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

/// One short beep, then a long wait.
const CHIRP: [Step; 2] = [Step::new(true, 80), Step::new(false, 2_920)];

/// Two beeps, then a wait.
const DOUBLE_BEEP: [Step; 4] = [
    Step::new(true, 120),
    Step::new(false, 120),
    Step::new(true, 120),
    Step::new(false, 1_640),
];

/// Three hard beeps in a second: meant to be impossible to miss.
const URGENT: [Step; 6] = [
    Step::new(true, 150),
    Step::new(false, 100),
    Step::new(true, 150),
    Step::new(false, 100),
    Step::new(true, 150),
    Step::new(false, 350),
];

const LED_SOLID: [Step; 1] = [Step::new(true, 1_000)];
const LED_SLOW: [Step; 2] = [Step::new(true, 500), Step::new(false, 500)];
const LED_FAST: [Step; 2] = [Step::new(true, 150), Step::new(false, 150)];

impl BuzzerPattern {
    /// The on/off sequence for this pattern.
    ///
    /// Every audible pattern is a *burst*: it plays a few cycles and stops on
    /// its own. An alarm that has not been dealt with comes back on the
    /// policy's re-announce interval rather than sounding continuously, because
    /// a buzzer that will not stop gets muffled, unplugged, or ignored — and
    /// then it is not there for the reading that mattered.
    pub const fn timeline(self) -> Timeline {
        match self {
            Self::Quiet => Timeline::new(&QUIET),
            Self::Chirp => Timeline::burst(&CHIRP, 1),
            Self::DoubleBeep => Timeline::burst(&DOUBLE_BEEP, 2),
            Self::Urgent => Timeline::burst(&URGENT, 3),
        }
    }

    /// How long one announcement of this pattern lasts.
    ///
    /// Rounded up to whole seconds, which is all [`Duration`] carries, plus a
    /// second of slack. Callers use this to decide how long to leave the
    /// pattern selected, and their clock is coarser than the player's: cutting
    /// the selection at the burst's exact length could truncate the last cycle
    /// by up to a second. The timeline already stops itself at the right
    /// moment, so erring long costs nothing but erring short clips a beep.
    pub fn burst(self) -> Duration {
        match self.timeline().total_millis() {
            Some(0) | None => Duration::ZERO,
            Some(millis) => Duration::from_secs(millis.div_ceil(1_000).saturating_add(1)),
        }
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
    fn a_chirp_beeps_briefly_and_is_done() {
        let timeline = BuzzerPattern::Chirp.timeline();
        assert!(timeline.output_at(0));
        assert!(timeline.output_at(79));
        assert!(!timeline.output_at(80));
        assert_eq!(timeline.period_millis(), 3_000);
        // One cycle only: it does not come back on its own.
        assert_eq!(timeline.total_millis(), Some(3_000));
        assert!(!timeline.output_at(3_000));
        assert!(!timeline.output_at(100_000));
    }

    #[test]
    fn an_urgent_burst_pulses_a_few_times_and_stops() {
        let timeline = BuzzerPattern::Urgent.timeline();
        assert_eq!(timeline.period_millis(), 1_000, "one cycle a second");
        assert_eq!(timeline.total_millis(), Some(3_000));

        // Three pulses per cycle, three cycles, and then silence for good.
        assert_eq!(count_pulses(timeline), 9);
        assert!(!timeline.output_at(3_000));
        assert!(!timeline.output_at(u64::MAX));
    }

    #[test]
    fn a_burst_is_short_enough_to_be_a_prompt_rather_than_a_siren() {
        // The whole point: an unattended alarm must not hold the buzzer on for
        // minutes. Each announcement is a few seconds; the policy's
        // re-announce interval is what brings it back.
        for pattern in [
            BuzzerPattern::Chirp,
            BuzzerPattern::DoubleBeep,
            BuzzerPattern::Urgent,
        ] {
            let total = pattern
                .timeline()
                .total_millis()
                .expect("an audible pattern is a burst, not an endless loop");
            assert!(total <= 5_000, "{pattern:?} runs for {total} ms");
        }
    }

    #[test]
    fn quiet_and_the_led_states_have_no_end() {
        // The LED is a status light, not an announcement: it stays as it is
        // until the application says otherwise.
        for state in [LedState::Solid, LedState::SlowBlink, LedState::FastBlink] {
            assert_eq!(state.timeline().total_millis(), None, "{state:?}");
        }
        assert_eq!(BuzzerPattern::Quiet.burst(), Duration::ZERO);
    }

    #[test]
    fn a_burst_window_outlasts_the_burst_itself() {
        // The application's clock is whole seconds and the player's is
        // milliseconds, so the window it holds the pattern for has to have
        // room for the rounding — otherwise it cuts the last cycle short.
        for pattern in [
            BuzzerPattern::Chirp,
            BuzzerPattern::DoubleBeep,
            BuzzerPattern::Urgent,
        ] {
            let total = pattern.timeline().total_millis().expect("a burst");
            let window = pattern.burst().as_secs() * 1_000;
            assert!(
                window >= total + 1_000,
                "{pattern:?}: window {window} ms against a {total} ms burst"
            );
        }
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
    fn a_bursts_cycles_are_identical_while_it_lasts() {
        for pattern in [BuzzerPattern::DoubleBeep, BuzzerPattern::Urgent] {
            let timeline = pattern.timeline();
            let period = u64::from(timeline.period_millis());
            let total = u64::from(timeline.total_millis().expect("a burst"));
            for offset in [0, 1, 37, period - 1] {
                let mut cycle = 0;
                while (cycle + 1) * period + offset < total {
                    assert_eq!(
                        timeline.output_at(offset),
                        timeline.output_at(offset + (cycle + 1) * period),
                        "{pattern:?} at {offset}, cycle {cycle}"
                    );
                    cycle += 1;
                }
                assert!(cycle > 0, "{pattern:?} should repeat at least once");
            }
        }
    }

    #[test]
    fn led_blinks_repeat_forever() {
        let timeline = LedState::SlowBlink.timeline();
        let period = u64::from(timeline.period_millis());
        for offset in [0, 1, 37, period - 1] {
            assert_eq!(
                timeline.output_at(offset),
                timeline.output_at(offset + period * 9_999),
                "at {offset}"
            );
        }
    }

    #[test]
    fn a_player_starts_its_pattern_from_the_beginning() {
        let mut player = Player::new();
        // Switching patterns mid-cycle must not clip the first beep.
        player.play(BuzzerPattern::Urgent.timeline(), 12_345);
        assert!(player.output(12_345));
        assert!(player.output(12_345 + 149));
        assert!(!player.output(12_345 + 150));
    }

    #[test]
    fn a_player_goes_quiet_when_its_burst_runs_out() {
        // Nothing has to tell it to stop: an unattended alarm falls silent by
        // itself, and the application's re-announcement is what brings it back.
        let mut player = Player::new();
        player.play(BuzzerPattern::Urgent.timeline(), 1_000);
        assert!(player.output(1_000));
        assert!(!player.output(1_000 + 3_000));
        assert!(!player.output(1_000 + 600_000));
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

    /// Rising edges over a whole burst, sampled at millisecond resolution.
    fn count_pulses(timeline: Timeline) -> u32 {
        let total = u64::from(timeline.total_millis().expect("a burst"));
        let mut pulses = u32::from(timeline.output_at(0));
        for t in 1..total {
            if timeline.output_at(t) && !timeline.output_at(t - 1) {
                pulses += 1;
            }
        }
        pulses
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
