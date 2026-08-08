//! The peripherals the display node drives, as traits.
//!
//! The application logic never touches a GPIO. It asks for a buzzer pattern and
//! an LED state, and something else decides whether that means a PWM channel or
//! a line of text on a terminal. That is what lets the whole node — alarms,
//! layout, refresh policy — be exercised on a laptop.

use glucobeacon_core::AlarmKind;

/// The sound the buzzer should be making.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum BuzzerPattern {
    /// Silence.
    #[default]
    Quiet,
    /// A single short chirp: something to notice, not to act on.
    Chirp,
    /// Two short beeps, repeating: a low or high that needs attention soon.
    DoubleBeep,
    /// A fast, loud, repeating pattern: act now.
    Urgent,
}

impl BuzzerPattern {
    /// The pattern an alarm calls for.
    ///
    /// Only the urgent bands make noise. A plain high or low, and a feed that
    /// has gone stale, are shown on the panel and on the button LED and are
    /// left at that: those are conditions to notice and act on in your own
    /// time, and a device that beeps at every one of them across a day is a
    /// device whose beeps stop meaning anything. The buzzer is spent on the two
    /// readings that want someone to look up right now.
    ///
    /// [`Self::Chirp`] and [`Self::DoubleBeep`] are kept for the gentler tiers
    /// so restoring one is a line here rather than a new pattern.
    pub const fn for_alarm(kind: AlarmKind) -> Self {
        match kind {
            AlarmKind::Stale | AlarmKind::High | AlarmKind::Low => Self::Quiet,
            AlarmKind::UrgentHigh | AlarmKind::UrgentLow => Self::Urgent,
        }
    }

    /// Whether this pattern makes any sound at all.
    pub const fn is_audible(self) -> bool {
        !matches!(self, Self::Quiet)
    }
}

/// What the button's LED should be doing.
///
/// The LED is inside the silence button, so it is also the affordance that
/// tells you the button is worth pressing.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum LedState {
    /// Dark: nothing wrong.
    #[default]
    Off,
    /// Steady: an alarm is active but silenced. Press again to un-silence.
    Solid,
    /// Slow blink: an alarm is active and sounding.
    SlowBlink,
    /// Fast blink: an urgent alarm is active and sounding.
    FastBlink,
}

impl LedState {
    /// The LED state for an alarm and whether it is currently silenced.
    pub const fn for_alarm(alarm: Option<AlarmKind>, silenced: bool) -> Self {
        match (alarm, silenced) {
            (None, _) => Self::Off,
            (Some(_), true) => Self::Solid,
            (Some(kind), false) if kind.is_urgent() => Self::FastBlink,
            (Some(_), false) => Self::SlowBlink,
        }
    }
}

/// A piezo buzzer.
pub trait Buzzer {
    /// What can go wrong driving the buzzer.
    type Error;

    /// Starts (or stops, for [`BuzzerPattern::Quiet`]) a pattern.
    ///
    /// Called only when the pattern changes, so implementations should keep
    /// playing until told otherwise rather than returning after one cycle.
    fn set_pattern(&mut self, pattern: BuzzerPattern) -> Result<(), Self::Error>;
}

/// The LED in the silence button.
pub trait Indicator {
    /// What can go wrong driving the LED.
    type Error;

    /// Sets the LED state.
    fn set(&mut self, state: LedState) -> Result<(), Self::Error>;
}

/// The silence button.
pub trait Button {
    /// What can go wrong reading the button.
    type Error;

    /// Whether the button was pressed since the last call.
    ///
    /// Latched rather than level-triggered: a press between polls must not be
    /// lost, because the one thing a user must always be able to do is silence
    /// an alarm. Debouncing belongs in the implementation.
    fn take_press(&mut self) -> Result<bool, Self::Error>;
}

/// An e-ink panel.
///
/// Drawing goes through [`embedded_graphics::draw_target::DrawTarget`]; this
/// adds the part e-ink needs that an LCD does not.
pub trait Panel {
    /// What can go wrong talking to the panel.
    type Error;

    /// Pushes the framebuffer to the panel.
    ///
    /// On a 7-inch e-ink panel this takes on the order of a second and visibly
    /// flashes, which is why the application redraws only when something
    /// actually changed.
    fn flush(&mut self) -> Result<(), Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urgent_alarms_get_the_urgent_pattern() {
        assert_eq!(
            BuzzerPattern::for_alarm(AlarmKind::UrgentLow),
            BuzzerPattern::Urgent
        );
        assert_eq!(
            BuzzerPattern::for_alarm(AlarmKind::UrgentHigh),
            BuzzerPattern::Urgent
        );
    }

    #[test]
    fn only_urgent_alarms_make_any_sound() {
        // The buzzer is a scarce resource: spend it on the two bands that want
        // someone to look up now, and let the panel and LED carry the rest.
        for kind in [AlarmKind::Stale, AlarmKind::High, AlarmKind::Low] {
            assert_eq!(BuzzerPattern::for_alarm(kind), BuzzerPattern::Quiet);
            assert!(!BuzzerPattern::for_alarm(kind).is_audible(), "{kind:?}");
        }
        for kind in [AlarmKind::UrgentHigh, AlarmKind::UrgentLow] {
            assert!(BuzzerPattern::for_alarm(kind).is_audible(), "{kind:?}");
        }
    }

    #[test]
    fn a_silent_band_still_shows_on_the_led() {
        // Silent must not mean invisible: the panel and the button are how a
        // plain high or a stale feed gets noticed at all now.
        for kind in [AlarmKind::Stale, AlarmKind::High, AlarmKind::Low] {
            assert_eq!(
                LedState::for_alarm(Some(kind), false),
                LedState::SlowBlink,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn the_led_is_dark_only_when_nothing_is_wrong() {
        assert_eq!(LedState::for_alarm(None, false), LedState::Off);
        assert_eq!(LedState::for_alarm(None, true), LedState::Off);
    }

    #[test]
    fn a_silenced_alarm_still_shows_on_the_led() {
        // Silenced is not the same as resolved, and the user needs to be able
        // to tell the difference at a glance.
        assert_eq!(
            LedState::for_alarm(Some(AlarmKind::UrgentLow), true),
            LedState::Solid
        );
    }

    #[test]
    fn urgency_shows_in_the_blink_rate() {
        assert_eq!(
            LedState::for_alarm(Some(AlarmKind::UrgentLow), false),
            LedState::FastBlink
        );
        assert_eq!(
            LedState::for_alarm(Some(AlarmKind::Low), false),
            LedState::SlowBlink
        );
    }
}
