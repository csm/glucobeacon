//! GPIO-backed peripherals, written against `embedded-hal`.
//!
//! These are the real drivers for the buzzer, the button LED, and the silence
//! button. They live here rather than in the firmware crate because they are
//! generic over `embedded-hal` rather than over any particular chip, which
//! means they can be tested on a workstation against mock pins — and the
//! firmware is left with nothing but wiring.
//!
//! Each takes a millisecond clock from the caller rather than reading one:
//! there is no portable way to ask what time it is, and passing it in makes
//! debouncing and blink timing testable.

use embedded_hal::digital::{InputPin, OutputPin};

use crate::hal::{Buzzer, BuzzerPattern, Indicator, LedState};
use crate::pattern::Player;

/// How long a level must hold before it counts as a real press.
///
/// A tactile switch bounces for a few milliseconds; 25 is comfortably past it
/// without being perceptible.
pub const DEFAULT_DEBOUNCE_MILLIS: u32 = 25;

/// Whether a pin drives its load when high or when low.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Polarity {
    /// The load is on when the pin is high.
    ActiveHigh,
    /// The load is on when the pin is low. Usual for an LED wired to Vcc, and
    /// for a button wired to ground with a pull-up.
    ActiveLow,
}

impl Polarity {
    /// The pin level that means "asserted".
    const fn level_for(self, asserted: bool) -> bool {
        match self {
            Self::ActiveHigh => asserted,
            Self::ActiveLow => !asserted,
        }
    }

    /// Whether a pin level means "asserted".
    const fn is_asserted(self, level_high: bool) -> bool {
        match self {
            Self::ActiveHigh => level_high,
            Self::ActiveLow => !level_high,
        }
    }
}

/// Drives an on/off output from a pattern.
///
/// Shared by the buzzer and the LED, which differ only in what they are
/// playing. Suits an active buzzer, which makes its own tone; a passive one
/// wants this gating a PWM channel rather than a plain output.
#[derive(Debug)]
struct PatternOutput<P> {
    pin: P,
    polarity: Polarity,
    player: Player,
    pending: Option<crate::pattern::Timeline>,
    driven: Option<bool>,
}

impl<P: OutputPin> PatternOutput<P> {
    fn new(pin: P, polarity: Polarity) -> Self {
        Self {
            pin,
            polarity,
            player: Player::new(),
            pending: None,
            driven: None,
        }
    }

    /// Queues a new pattern, to start at the next [`Self::poll`].
    fn queue(&mut self, timeline: crate::pattern::Timeline) {
        self.pending = Some(timeline);
    }

    /// Applies the pattern to the pin.
    ///
    /// Writes only on a change, so a loop calling this every few milliseconds
    /// does not spend its time on redundant GPIO writes.
    fn poll(&mut self, now_millis: u64) -> Result<(), P::Error> {
        if let Some(timeline) = self.pending.take() {
            self.player.play(timeline, now_millis);
        }
        let asserted = self.player.output(now_millis);
        if self.driven == Some(asserted) {
            return Ok(());
        }
        if self.polarity.level_for(asserted) {
            self.pin.set_high()?;
        } else {
            self.pin.set_low()?;
        }
        self.driven = Some(asserted);
        Ok(())
    }
}

/// A piezo buzzer on a GPIO.
#[derive(Debug)]
pub struct GpioBuzzer<P> {
    output: PatternOutput<P>,
}

impl<P: OutputPin> GpioBuzzer<P> {
    /// A buzzer on `pin`, initially silent.
    pub fn new(pin: P, polarity: Polarity) -> Self {
        Self {
            output: PatternOutput::new(pin, polarity),
        }
    }

    /// Advances the pattern. Call this every few milliseconds.
    pub fn poll(&mut self, now_millis: u64) -> Result<(), P::Error> {
        self.output.poll(now_millis)
    }

    /// Whether the buzzer is currently sounding.
    pub fn is_sounding(&self, now_millis: u64) -> bool {
        self.output.player.output(now_millis)
    }
}

impl<P: OutputPin> Buzzer for GpioBuzzer<P> {
    type Error = P::Error;

    fn set_pattern(&mut self, pattern: BuzzerPattern) -> Result<(), Self::Error> {
        self.output.queue(pattern.timeline());
        Ok(())
    }
}

/// The LED in the silence button.
#[derive(Debug)]
pub struct GpioIndicator<P> {
    output: PatternOutput<P>,
}

impl<P: OutputPin> GpioIndicator<P> {
    /// An LED on `pin`, initially dark.
    pub fn new(pin: P, polarity: Polarity) -> Self {
        Self {
            output: PatternOutput::new(pin, polarity),
        }
    }

    /// Advances the blink. Call this every few milliseconds.
    pub fn poll(&mut self, now_millis: u64) -> Result<(), P::Error> {
        self.output.poll(now_millis)
    }

    /// Whether the LED is currently lit.
    pub fn is_lit(&self, now_millis: u64) -> bool {
        self.output.player.output(now_millis)
    }
}

impl<P: OutputPin> Indicator for GpioIndicator<P> {
    type Error = P::Error;

    fn set(&mut self, state: LedState) -> Result<(), Self::Error> {
        self.output.queue(state.timeline());
        Ok(())
    }
}

/// A debounced push button.
///
/// Latches its press: [`crate::hal::Button::take_press`] reports a press that
/// happened between polls, because the one thing a user must always be able to
/// do is silence an alarm.
#[derive(Debug)]
pub struct GpioButton<P> {
    pin: P,
    polarity: Polarity,
    debounce_millis: u32,
    /// The last debounced state.
    stable: bool,
    /// The most recent raw reading.
    candidate: bool,
    /// When the raw reading last changed.
    changed_at: u64,
    latched: bool,
}

impl<P: InputPin> GpioButton<P> {
    /// A button on `pin` with the default debounce.
    pub fn new(pin: P, polarity: Polarity) -> Self {
        Self::with_debounce(pin, polarity, DEFAULT_DEBOUNCE_MILLIS)
    }

    /// A button with an explicit debounce interval.
    pub fn with_debounce(pin: P, polarity: Polarity, debounce_millis: u32) -> Self {
        Self {
            pin,
            polarity,
            debounce_millis,
            stable: false,
            candidate: false,
            changed_at: 0,
            latched: false,
        }
    }

    /// Samples the pin. Call this every few milliseconds.
    ///
    /// A press is latched on the edge into "pressed", once the level has held
    /// for the debounce interval. Releasing does nothing: the button is a
    /// momentary action, not a state.
    pub fn poll(&mut self, now_millis: u64) -> Result<(), P::Error> {
        let pressed = self.polarity.is_asserted(self.pin.is_high()?);

        if pressed != self.candidate {
            self.candidate = pressed;
            self.changed_at = now_millis;
            return Ok(());
        }
        if self.candidate == self.stable {
            return Ok(());
        }
        if now_millis.saturating_sub(self.changed_at) >= u64::from(self.debounce_millis) {
            self.stable = self.candidate;
            if self.stable {
                self.latched = true;
            }
        }
        Ok(())
    }

    /// Whether the button is being held down right now.
    pub const fn is_held(&self) -> bool {
        self.stable
    }
}

impl<P: InputPin> crate::hal::Button for GpioButton<P> {
    type Error = P::Error;

    fn take_press(&mut self) -> Result<bool, Self::Error> {
        Ok(core::mem::take(&mut self.latched))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::Button as _;
    use embedded_hal::digital::{ErrorType, PinState};

    #[derive(Debug, Default)]
    struct MockPin {
        level: bool,
        writes: u32,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Never;

    impl embedded_hal::digital::Error for Never {
        fn kind(&self) -> embedded_hal::digital::ErrorKind {
            embedded_hal::digital::ErrorKind::Other
        }
    }

    impl ErrorType for MockPin {
        type Error = Never;
    }

    impl OutputPin for MockPin {
        fn set_low(&mut self) -> Result<(), Never> {
            self.level = false;
            self.writes += 1;
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), Never> {
            self.level = true;
            self.writes += 1;
            Ok(())
        }

        fn set_state(&mut self, state: PinState) -> Result<(), Never> {
            match state {
                PinState::Low => self.set_low(),
                PinState::High => self.set_high(),
            }
        }
    }

    impl InputPin for MockPin {
        fn is_high(&mut self) -> Result<bool, Never> {
            Ok(self.level)
        }

        fn is_low(&mut self) -> Result<bool, Never> {
            Ok(!self.level)
        }
    }

    #[test]
    fn an_urgent_pattern_actually_toggles_the_pin() {
        let mut buzzer = GpioBuzzer::new(MockPin::default(), Polarity::ActiveHigh);
        buzzer.set_pattern(BuzzerPattern::Urgent).expect("set");

        let mut edges = 0;
        let mut previous = false;
        for t in 0..1_200u64 {
            buzzer.poll(t).expect("poll");
            let level = buzzer.output.pin.level;
            if level != previous {
                edges += 1;
                previous = level;
            }
        }
        assert!(
            edges >= 5,
            "urgent should beep repeatedly, saw {edges} edges"
        );
    }

    #[test]
    fn a_quiet_buzzer_never_drives_the_pin_high() {
        let mut buzzer = GpioBuzzer::new(MockPin::default(), Polarity::ActiveHigh);
        buzzer.set_pattern(BuzzerPattern::Quiet).expect("set");
        for t in 0..5_000u64 {
            buzzer.poll(t).expect("poll");
            assert!(!buzzer.output.pin.level, "sounded at t={t}");
        }
    }

    #[test]
    fn active_low_wiring_is_inverted() {
        let mut led = GpioIndicator::new(MockPin::default(), Polarity::ActiveLow);
        led.set(LedState::Solid).expect("set");
        led.poll(0).expect("poll");
        assert!(!led.output.pin.level, "active low means lit is a low pin");
        assert!(led.is_lit(0));

        let mut led = GpioIndicator::new(MockPin::default(), Polarity::ActiveHigh);
        led.set(LedState::Solid).expect("set");
        led.poll(0).expect("poll");
        assert!(led.output.pin.level);
    }

    #[test]
    fn the_pin_is_only_written_when_it_changes() {
        // A loop polling every few milliseconds should not spend itself on
        // redundant GPIO writes.
        let mut led = GpioIndicator::new(MockPin::default(), Polarity::ActiveHigh);
        led.set(LedState::Solid).expect("set");
        for t in 0..1_000u64 {
            led.poll(t).expect("poll");
        }
        assert_eq!(led.output.pin.writes, 1, "solid needs exactly one write");
    }

    #[test]
    fn a_slow_blink_writes_twice_per_cycle() {
        let mut led = GpioIndicator::new(MockPin::default(), Polarity::ActiveHigh);
        led.set(LedState::SlowBlink).expect("set");
        for t in 0..1_000u64 {
            led.poll(t).expect("poll");
        }
        assert_eq!(led.output.pin.writes, 2, "on at 0, off at 500");
    }

    #[test]
    fn switching_patterns_restarts_from_the_beginning() {
        let mut buzzer = GpioBuzzer::new(MockPin::default(), Polarity::ActiveHigh);
        buzzer.set_pattern(BuzzerPattern::Urgent).expect("set");
        buzzer.poll(0).expect("poll");
        assert!(buzzer.output.pin.level);

        // Mid-beep, switch to quiet: it must stop, not finish the beep.
        buzzer.set_pattern(BuzzerPattern::Quiet).expect("set");
        buzzer.poll(100).expect("poll");
        assert!(!buzzer.output.pin.level);
    }

    /// Drives a button through a sequence of `(millis, level)` samples.
    fn run(button: &mut GpioButton<MockPin>, samples: &[(u64, bool)]) {
        for (t, level) in samples {
            button.pin.level = *level;
            button.poll(*t).expect("poll");
        }
    }

    #[test]
    fn a_clean_press_is_latched_once() {
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveHigh, 25);
        assert!(!button.take_press().expect("poll"));

        run(&mut button, &[(0, true), (10, true), (30, true)]);
        assert!(button.is_held());
        assert!(
            button.take_press().expect("take"),
            "the press must register"
        );
        assert!(!button.take_press().expect("take"), "and only once");
    }

    #[test]
    fn contact_bounce_does_not_produce_extra_presses() {
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveHigh, 25);
        // A real switch chatters for a few milliseconds on contact.
        run(
            &mut button,
            &[
                (0, true),
                (2, false),
                (3, true),
                (5, false),
                (6, true),
                (40, true),
                (60, true),
            ],
        );
        assert!(button.take_press().expect("take"));
        assert!(!button.take_press().expect("take"), "one press, not five");
    }

    #[test]
    fn a_spike_shorter_than_the_debounce_is_ignored() {
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveHigh, 25);
        run(
            &mut button,
            &[(0, false), (10, true), (15, false), (50, false)],
        );
        assert!(!button.take_press().expect("take"), "noise is not a press");
        assert!(!button.is_held());
    }

    #[test]
    fn releasing_does_not_count_as_a_press() {
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveHigh, 25);
        run(&mut button, &[(0, true), (30, true)]);
        assert!(button.take_press().expect("take"));

        run(&mut button, &[(40, false), (80, false)]);
        assert!(!button.take_press().expect("take"));
        assert!(!button.is_held());
    }

    #[test]
    fn a_second_press_registers_after_a_release() {
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveHigh, 25);
        run(&mut button, &[(0, true), (30, true)]);
        assert!(button.take_press().expect("take"));
        run(&mut button, &[(40, false), (80, false)]);
        run(&mut button, &[(90, true), (130, true)]);
        assert!(button.take_press().expect("take"), "press, release, press");
    }

    #[test]
    fn a_press_between_polls_is_never_lost() {
        // The application may go a whole second between polls while it repaints
        // the panel. A press during that window must still silence the alarm.
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveHigh, 25);
        run(&mut button, &[(0, true), (30, true)]);
        // ... a long repaint happens here, take_press is not called ...
        run(&mut button, &[(1_000, false), (1_100, false)]);
        assert!(button.take_press().expect("take"));
    }

    #[test]
    fn an_active_low_button_reads_a_grounded_pin_as_pressed() {
        // A button to ground with an internal pull-up, which is how the PRG
        // button and most panel switches are wired.
        let mut button = GpioButton::with_debounce(MockPin::default(), Polarity::ActiveLow, 25);
        run(&mut button, &[(0, true), (10, true)]);
        assert!(!button.is_held(), "high is released when active low");
        run(&mut button, &[(20, false), (60, false)]);
        assert!(button.is_held());
        assert!(button.take_press().expect("take"));
    }
}
