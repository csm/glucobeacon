//! The display node's logic, with no I/O in it.
//!
//! Packets and time go in; redraw decisions, buzzer patterns and LED states
//! come out. Keeping it this way is what makes the interesting behaviour —
//! when to alarm, when to repaint a panel that takes a second to repaint —
//! testable without a radio or a screen.

use glucobeacon_core::{
    AlarmEngine, AlarmEvent, AlarmKind, AlarmPolicy, Duration, Thresholds, Timestamp, Trend,
};
use glucobeacon_proto::{Packet, UplinkState};

use crate::hal::{BuzzerPattern, LedState};
use crate::state::{Applied, DisplayState};

/// How long after boot to wait before alarming about missing data.
///
/// Without this the node beeps "NO DATA" the instant it powers on, every time,
/// which is the fastest way to teach someone to ignore it.
pub const BOOT_GRACE: Duration = Duration::from_secs(90);

/// How long the buzzer sounds for each announcement.
///
/// Long enough to wake someone, short enough that an unattended alarm is not
/// a continuous scream — the re-announce interval brings it back.
pub const SOUND_FOR: Duration = Duration::from_secs(30);

/// What one tick concluded.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct Tick {
    /// The alarm transition, if there was one.
    pub event: Option<AlarmEvent>,
    /// Whether the panel needs repainting.
    pub redraw: bool,
    /// A new buzzer pattern to apply, if it changed.
    pub buzzer: Option<BuzzerPattern>,
    /// A new LED state to apply, if it changed.
    pub led: Option<LedState>,
}

/// What is visible on the panel, reduced to something comparable.
///
/// An e-ink refresh takes about a second and flashes the whole panel, so the
/// node repaints only when one of these changes — not on a timer, and not on
/// every packet.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Fingerprint {
    latest_at: Option<i64>,
    latest_mgdl: Option<u16>,
    latest_trend: Option<Trend>,
    delta_mgdl: Option<i16>,
    reading_age_mins: Option<u32>,
    alarm: Option<AlarmKind>,
    silence_mins: Option<u32>,
    uplink: UplinkState,
    link_quiet_mins: Option<u32>,
    thresholds: Thresholds,
}

/// The display node.
#[derive(Clone, Debug)]
pub struct DisplayApp {
    state: DisplayState,
    alarms: AlarmEngine,
    painted: Option<Fingerprint>,
    buzzer: BuzzerPattern,
    led: LedState,
    sounding_until: Option<Timestamp>,
}

impl Default for DisplayApp {
    fn default() -> Self {
        Self::new(AlarmPolicy::DEFAULT)
    }
}

impl DisplayApp {
    /// A node that has just booted.
    pub fn new(policy: AlarmPolicy) -> Self {
        Self {
            state: DisplayState::new(policy),
            alarms: AlarmEngine::new(policy),
            painted: None,
            buzzer: BuzzerPattern::Quiet,
            led: LedState::Off,
            sounding_until: None,
        }
    }

    /// What the node knows, for rendering.
    pub const fn state(&self) -> &DisplayState {
        &self.state
    }

    /// The alarm in force.
    pub const fn alarm(&self) -> Option<AlarmKind> {
        self.alarms.active()
    }

    /// How much silence is left on the active alarm.
    pub fn silence_remaining(&self, uptime: Duration) -> Option<Duration> {
        self.alarms.silence_remaining(self.clock(uptime))
    }

    /// The wall clock, if known.
    pub fn now(&self, uptime: Duration) -> Option<Timestamp> {
        self.state.now(uptime)
    }

    /// Folds in a received packet.
    ///
    /// A policy pushed by the gateway takes effect on the alarm engine
    /// immediately, so the thresholds the panel draws and the thresholds the
    /// buzzer uses can never disagree.
    pub fn handle_packet(&mut self, packet: &Packet, uptime: Duration) -> Applied {
        let applied = self.state.apply(packet, uptime);
        if applied == Applied::Policy {
            self.alarms.set_policy(self.state.policy());
        }
        applied
    }

    /// Handles a press of the silence button.
    ///
    /// Pressing while silenced un-silences, so someone who silenced by reflex
    /// and then thought better of it is not stuck waiting out the window.
    /// Returns whether the press did anything.
    pub fn press_silence(&mut self, uptime: Duration) -> bool {
        let now = self.clock(uptime);
        if self.alarms.active().is_none() {
            return false;
        }
        if self.alarms.is_silenced(now) {
            self.alarms.unsilence();
        } else {
            self.alarms.silence(now);
            self.sounding_until = None;
        }
        true
    }

    /// Advances time and reports what to do.
    pub fn tick(&mut self, uptime: Duration) -> Tick {
        let now = self.clock(uptime);

        // Hold off alarming during the boot grace period, but keep the panel
        // up to date so it can say it is waiting.
        let event = if uptime.as_secs() >= BOOT_GRACE.as_secs() || self.state.clock_is_set() {
            self.alarms.update(now, self.state.latest())
        } else {
            None
        };

        if let Some(event) = event
            && event.is_audible()
        {
            self.sounding_until = Some(now.saturating_add(SOUND_FOR));
        }
        if self.alarms.active().is_none() {
            self.sounding_until = None;
        }

        let silenced = self.alarms.is_silenced(now);
        let sounding = !silenced && self.sounding_until.is_some_and(|until| now < until);
        let buzzer = match (sounding, self.alarms.active()) {
            (true, Some(kind)) => BuzzerPattern::for_alarm(kind),
            _ => BuzzerPattern::Quiet,
        };
        let led = LedState::for_alarm(self.alarms.active(), silenced);

        let fingerprint = self.fingerprint(uptime);
        let redraw = self.painted.as_ref() != Some(&fingerprint);
        if redraw {
            self.painted = Some(fingerprint);
        }

        Tick {
            event,
            redraw,
            buzzer: (buzzer != self.buzzer).then(|| {
                self.buzzer = buzzer;
                buzzer
            }),
            led: (led != self.led).then(|| {
                self.led = led;
                led
            }),
        }
    }

    /// The clock to evaluate alarms against.
    ///
    /// Before the gateway has told the node what time it is, uptime stands in.
    /// Both are monotonic, and the only thing the alarm engine does with a
    /// timestamp is compare and subtract, so the substitution is safe — and it
    /// means a node that never hears from the gateway still raises "no data"
    /// rather than sitting there looking content.
    fn clock(&self, uptime: Duration) -> Timestamp {
        self.state
            .now(uptime)
            .unwrap_or_else(|| Timestamp::from_unix_secs(i64::from(uptime.as_secs())))
    }

    fn fingerprint(&self, uptime: Duration) -> Fingerprint {
        let now = self.state.now(uptime);
        let latest = self.state.latest();

        Fingerprint {
            latest_at: latest.map(|r| r.at.as_unix_secs()),
            latest_mgdl: latest.map(|r| r.glucose.mgdl()),
            latest_trend: latest.map(|r| r.trend),
            delta_mgdl: self.state.delta_mgdl(),
            // Only the whole-minute value is drawn, so only changes to it
            // are worth a refresh.
            reading_age_mins: match (now, latest) {
                (Some(now), Some(reading)) => Some(reading.age(now).as_mins()),
                _ => None,
            },
            alarm: self.alarms.active(),
            silence_mins: self
                .alarms
                .silence_remaining(self.clock(uptime))
                .map(|d| d.as_mins()),
            uplink: self.state.uplink(),
            link_quiet_mins: self.state.since_last_packet(uptime).map(|d| d.as_mins()),
            thresholds: self.state.policy().thresholds,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glucobeacon_core::{Glucose, Reading};
    use glucobeacon_proto::{Message, ReadingReport, Status};

    fn secs(v: u32) -> Duration {
        Duration::from_secs(v)
    }

    fn at(v: i64) -> Timestamp {
        Timestamp::from_unix_secs(v)
    }

    /// A reading packet: sensor value `mgdl` measured at `sensor_at`, sent now.
    fn reading(mgdl: u16, sensor_at: i64, sent_at: i64) -> Packet {
        Packet::new(
            0,
            Message::Reading(ReadingReport {
                reading: Reading::new(Glucose::from_mgdl(mgdl), Trend::Flat, at(sensor_at)),
                sent_at: at(sent_at),
            }),
        )
    }

    /// Boots a node and feeds it one in-range reading, past the grace period.
    fn running() -> (DisplayApp, Duration) {
        let mut app = DisplayApp::default();
        let uptime = secs(100);
        app.handle_packet(&reading(120, 1_000_000, 1_000_030), uptime);
        app.tick(uptime);
        (app, uptime)
    }

    #[test]
    fn a_fresh_boot_is_quiet_and_paints_once() {
        let mut app = DisplayApp::default();
        let first = app.tick(secs(1));
        assert!(first.redraw, "the first tick must paint something");
        assert_eq!(first.event, None, "no alarm during the grace period");
        assert_eq!(first.buzzer, None);

        // Nothing changed, so nothing to repaint.
        assert!(!app.tick(secs(2)).redraw);
    }

    #[test]
    fn a_node_that_never_hears_the_gateway_eventually_alarms() {
        let mut app = DisplayApp::default();
        assert_eq!(app.tick(secs(1)).event, None);
        let tick = app.tick(BOOT_GRACE);
        assert_eq!(tick.event, Some(AlarmEvent::Raised(AlarmKind::Stale)));
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Chirp));
        assert_eq!(tick.led, Some(LedState::SlowBlink));
    }

    #[test]
    fn a_reading_clears_the_no_data_alarm() {
        let mut app = DisplayApp::default();
        app.tick(BOOT_GRACE);
        assert_eq!(app.alarm(), Some(AlarmKind::Stale));

        app.handle_packet(&reading(120, 1_000_000, 1_000_030), BOOT_GRACE);
        let tick = app.tick(BOOT_GRACE);
        assert_eq!(tick.event, Some(AlarmEvent::Cleared(AlarmKind::Stale)));
        assert_eq!(app.alarm(), None);
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Quiet));
        assert_eq!(tick.led, Some(LedState::Off));
    }

    #[test]
    fn a_low_reading_sounds_the_buzzer_and_lights_the_button() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(52, 1_000_300, 1_000_330), uptime);
        let tick = app.tick(uptime);

        assert_eq!(tick.event, Some(AlarmEvent::Raised(AlarmKind::UrgentLow)));
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Urgent));
        assert_eq!(tick.led, Some(LedState::FastBlink));
        assert!(tick.redraw);
    }

    #[test]
    fn the_buzzer_stops_after_the_announcement_and_returns_on_the_next() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(52, 1_000_300, 1_000_330), uptime);
        app.tick(uptime);

        // Still sounding partway through the announcement.
        assert_eq!(app.tick(uptime + secs(10)).buzzer, None);
        // Quiet once it finishes.
        assert_eq!(
            app.tick(uptime + SOUND_FOR).buzzer,
            Some(BuzzerPattern::Quiet)
        );
        // And back for the re-announcement five minutes later.
        let tick = app.tick(uptime + secs(301));
        assert_eq!(
            tick.event,
            Some(AlarmEvent::Reannounce(AlarmKind::UrgentLow))
        );
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Urgent));
    }

    #[test]
    fn silencing_stops_the_buzzer_but_keeps_the_led_lit() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(52, 1_000_300, 1_000_330), uptime);
        app.tick(uptime);

        assert!(app.press_silence(uptime));
        let tick = app.tick(uptime);
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Quiet));
        assert_eq!(tick.led, Some(LedState::Solid));
        assert_eq!(app.alarm(), Some(AlarmKind::UrgentLow), "still alarming");
    }

    #[test]
    fn pressing_again_un_silences() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(52, 1_000_300, 1_000_330), uptime);
        app.tick(uptime);
        app.press_silence(uptime);
        app.tick(uptime);

        assert!(app.press_silence(uptime));
        let tick = app.tick(uptime);
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Urgent));
        assert_eq!(tick.led, Some(LedState::FastBlink));
    }

    #[test]
    fn pressing_with_no_alarm_does_nothing() {
        let (mut app, uptime) = running();
        assert!(!app.press_silence(uptime));
        assert_eq!(app.tick(uptime).led, None);
    }

    #[test]
    fn silence_expires_and_the_alarm_speaks_up_again() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(52, 1_000_300, 1_000_330), uptime);
        app.tick(uptime);
        app.press_silence(uptime);
        app.tick(uptime);

        let policy = app.state().policy();
        let later = uptime + secs(policy.silence_for.as_secs() + 1);
        let tick = app.tick(later);
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Urgent));
    }

    #[test]
    fn escalating_past_a_silence_makes_noise_again() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(65, 1_000_300, 1_000_330), uptime);
        app.tick(uptime);
        app.press_silence(uptime);
        app.tick(uptime);
        assert_eq!(app.tick(uptime + secs(10)).buzzer, None);

        app.handle_packet(&reading(48, 1_000_600, 1_000_630), uptime + secs(300));
        let tick = app.tick(uptime + secs(300));
        assert!(matches!(tick.event, Some(AlarmEvent::Escalated { .. })));
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Urgent));
    }

    #[test]
    fn a_duplicate_reading_does_not_repaint_the_panel() {
        let (mut app, uptime) = running();
        let packet = reading(120, 1_000_000, 1_000_030);
        assert_eq!(app.handle_packet(&packet, uptime), Applied::Unchanged);
        assert!(
            !app.tick(uptime).redraw,
            "an e-ink refresh costs a second and a flash; do not spend it on nothing"
        );
    }

    #[test]
    fn the_panel_repaints_when_the_displayed_age_ticks_over() {
        // The reading is 30s old at the first tick, so it rolls over to
        // "1 min ago" 30s later.
        let (mut app, uptime) = running();
        // Still inside the same displayed minute: nothing on screen changed.
        assert!(!app.tick(uptime + secs(20)).redraw);
        // A new whole minute is a different screen.
        assert!(app.tick(uptime + secs(31)).redraw);
    }

    #[test]
    fn a_new_reading_repaints() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(133, 1_000_300, 1_000_330), uptime + secs(300));
        assert!(app.tick(uptime + secs(300)).redraw);
    }

    #[test]
    fn an_uplink_change_repaints() {
        let (mut app, uptime) = running();
        assert!(!app.tick(uptime).redraw);

        app.handle_packet(
            &Packet::new(
                1,
                Message::Status(Status {
                    sent_at: at(1_000_030),
                    uplink: UplinkState::AuthFailed,
                    consecutive_failures: 2,
                    battery_millivolts: None,
                }),
            ),
            uptime,
        );
        assert!(app.tick(uptime).redraw);
    }

    #[test]
    fn a_pushed_policy_takes_effect_on_the_alarm_engine_immediately() {
        let (mut app, uptime) = running();
        assert_eq!(app.alarm(), None, "120 mg/dL is in range by default");

        let mut policy = AlarmPolicy::DEFAULT;
        policy.thresholds.high_mgdl = 100;
        app.handle_packet(&Packet::new(1, Message::Policy(policy)), uptime);

        let tick = app.tick(uptime);
        assert_eq!(tick.event, Some(AlarmEvent::Raised(AlarmKind::High)));
        assert!(tick.redraw, "the thresholds on the graph moved too");
    }

    #[test]
    fn peripheral_updates_are_only_reported_when_they_change() {
        let (mut app, uptime) = running();
        app.handle_packet(&reading(52, 1_000_300, 1_000_330), uptime);
        assert_eq!(app.tick(uptime).buzzer, Some(BuzzerPattern::Urgent));
        // Same state a moment later: nothing to tell the peripherals.
        assert_eq!(app.tick(uptime + secs(1)).buzzer, None);
        assert_eq!(app.tick(uptime + secs(1)).led, None);
    }

    #[test]
    fn a_silent_radio_becomes_a_stale_alarm() {
        let (mut app, uptime) = running();
        let stale_after = app.state().policy().stale_after.as_secs();
        let tick = app.tick(uptime + secs(stale_after));
        assert_eq!(tick.event, Some(AlarmEvent::Raised(AlarmKind::Stale)));
        assert_eq!(tick.buzzer, Some(BuzzerPattern::Chirp));
    }
}
