//! What the display node knows.

use glucobeacon_core::{AlarmPolicy, Duration, History, Reading, Timestamp};
use glucobeacon_proto::{Message, Packet, UplinkState};

/// How many readings to keep. Three hours at one every five minutes, which is
/// the window the graph shows.
pub const HISTORY_LEN: usize = 36;

/// The longest gap across which a delta is still meaningful.
const MAX_DELTA_GAP: Duration = Duration::from_mins(10);

/// The display node's clock.
///
/// The node has no RTC and no network time. It learns the wall clock from the
/// gateway, which stamps every packet, and carries it forward with a monotonic
/// uptime counter between packets. Before the first packet arrives it does not
/// know what time it is — and says so, rather than guessing, because guessing
/// would mean showing a confident "2 min ago" that is hours wrong.
#[derive(Copy, Clone, Debug, Default)]
pub struct Clock {
    anchor: Option<Anchor>,
}

#[derive(Copy, Clone, Debug)]
struct Anchor {
    wall: Timestamp,
    uptime: Duration,
}

impl Clock {
    /// A clock that does not yet know the time.
    pub const fn new() -> Self {
        Self { anchor: None }
    }

    /// Records that `wall` was the time at `uptime` seconds since boot.
    pub fn sync(&mut self, wall: Timestamp, uptime: Duration) {
        self.anchor = Some(Anchor { wall, uptime });
    }

    /// The current wall clock, if it is known.
    pub fn now(&self, uptime: Duration) -> Option<Timestamp> {
        let anchor = self.anchor?;
        let elapsed = uptime.as_secs().saturating_sub(anchor.uptime.as_secs());
        Some(anchor.wall.saturating_add(Duration::from_secs(elapsed)))
    }

    /// Whether the clock has ever been set.
    pub const fn is_set(&self) -> bool {
        self.anchor.is_some()
    }
}

/// What applying a packet did.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Applied {
    /// A reading the node had not seen before.
    NewReading,
    /// The packet told the node nothing new — a reading it already had, or a
    /// policy identical to the one in force. Normal: the gateway re-sends both.
    Unchanged,
    /// The gateway's uplink status.
    Status,
    /// A new alarm policy.
    Policy,
    /// Something only the gateway should receive.
    Ignored,
}

impl Applied {
    /// Whether this changed anything worth redrawing for.
    pub const fn changed_something(self) -> bool {
        matches!(self, Self::NewReading | Self::Status | Self::Policy)
    }
}

/// Everything the display renders from.
#[derive(Clone, Debug)]
pub struct DisplayState {
    history: History<HISTORY_LEN>,
    clock: Clock,
    policy: AlarmPolicy,
    uplink: UplinkState,
    consecutive_failures: u16,
    gateway_battery_millivolts: Option<u16>,
    last_packet_at: Option<Duration>,
}

impl Default for DisplayState {
    fn default() -> Self {
        Self::new(AlarmPolicy::DEFAULT)
    }
}

impl DisplayState {
    /// A node that has just booted and heard nothing.
    pub const fn new(policy: AlarmPolicy) -> Self {
        Self {
            history: History::new(),
            clock: Clock::new(),
            policy,
            // Not `Ok`: we have not heard from the gateway, so as far as this
            // node knows there is no data.
            uplink: UplinkState::NoRecentData,
            consecutive_failures: 0,
            gateway_battery_millivolts: None,
            last_packet_at: None,
        }
    }

    /// Folds a received packet into the state.
    pub fn apply(&mut self, packet: &Packet, uptime: Duration) -> Applied {
        self.last_packet_at = Some(uptime);

        match &packet.message {
            Message::Reading(report) => {
                self.clock.sync(report.sent_at, uptime);
                if self.history.push(report.reading) {
                    Applied::NewReading
                } else {
                    Applied::Unchanged
                }
            }
            Message::Status(status) => {
                self.clock.sync(status.sent_at, uptime);
                self.uplink = status.uplink;
                self.consecutive_failures = status.consecutive_failures;
                self.gateway_battery_millivolts = status.battery_millivolts;
                Applied::Status
            }
            Message::Policy(policy) => {
                // A malformed policy would make every alarm decision nonsense,
                // so a bad one is dropped and the previous one stays in force.
                if policy.validate().is_ok() && *policy != self.policy {
                    self.policy = *policy;
                    Applied::Policy
                } else {
                    Applied::Unchanged
                }
            }
            Message::Ack { .. } => Applied::Ignored,
        }
    }

    /// The current wall clock, if known.
    pub fn now(&self, uptime: Duration) -> Option<Timestamp> {
        self.clock.now(uptime)
    }

    /// Whether the node has learned what time it is.
    pub const fn clock_is_set(&self) -> bool {
        self.clock.is_set()
    }

    /// The readings held, oldest first.
    pub const fn history(&self) -> &History<HISTORY_LEN> {
        &self.history
    }

    /// The newest reading.
    pub fn latest(&self) -> Option<&Reading> {
        self.history.latest()
    }

    /// Change since the previous reading, when the two are close enough
    /// together for it to mean anything.
    pub fn delta_mgdl(&self) -> Option<i16> {
        self.history.delta_mgdl(MAX_DELTA_GAP)
    }

    /// The alarm policy in force.
    pub const fn policy(&self) -> AlarmPolicy {
        self.policy
    }

    /// The gateway's last reported uplink state.
    pub const fn uplink(&self) -> UplinkState {
        self.uplink
    }

    /// Consecutive upstream failures the gateway last reported.
    pub const fn consecutive_failures(&self) -> u16 {
        self.consecutive_failures
    }

    /// The gateway's battery, if it reports one.
    pub const fn gateway_battery_millivolts(&self) -> Option<u16> {
        self.gateway_battery_millivolts
    }

    /// How long since any packet arrived, by uptime.
    ///
    /// This measures the *radio link*, where [`DisplayState::latest`]'s age
    /// measures the sensor. A silent radio and a quiet sensor are different
    /// faults and the display distinguishes them.
    pub fn since_last_packet(&self, uptime: Duration) -> Option<Duration> {
        let last = self.last_packet_at?;
        Some(Duration::from_secs(
            uptime.as_secs().saturating_sub(last.as_secs()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glucobeacon_core::{Glucose, Thresholds, Trend};
    use glucobeacon_proto::{ReadingReport, Status};

    fn secs(v: u32) -> Duration {
        Duration::from_secs(v)
    }

    fn at(v: i64) -> Timestamp {
        Timestamp::from_unix_secs(v)
    }

    fn reading_packet(seq: u16, mgdl: u16, sensor_at: i64, sent_at: i64) -> Packet {
        Packet::new(
            seq,
            Message::Reading(ReadingReport {
                reading: Reading::new(Glucose::from_mgdl(mgdl), Trend::Flat, at(sensor_at)),
                sent_at: at(sent_at),
            }),
        )
    }

    fn status_packet(seq: u16, uplink: UplinkState, sent_at: i64) -> Packet {
        Packet::new(
            seq,
            Message::Status(Status {
                sent_at: at(sent_at),
                uplink,
                consecutive_failures: 3,
                battery_millivolts: Some(3_900),
            }),
        )
    }

    #[test]
    fn a_fresh_node_does_not_know_what_time_it_is() {
        let state = DisplayState::default();
        assert!(!state.clock_is_set());
        assert_eq!(state.now(secs(100)), None);
        assert_eq!(state.latest(), None);
    }

    #[test]
    fn the_clock_is_learned_from_the_first_packet() {
        let mut state = DisplayState::default();
        state.apply(
            &reading_packet(1, 120, 1_700_000_000, 1_700_000_030),
            secs(50),
        );
        assert!(state.clock_is_set());
        assert_eq!(state.now(secs(50)), Some(at(1_700_000_030)));
    }

    #[test]
    fn the_clock_advances_between_packets() {
        let mut state = DisplayState::default();
        state.apply(
            &reading_packet(1, 120, 1_700_000_000, 1_700_000_030),
            secs(50),
        );
        // 90 seconds of uptime later, 90 seconds of wall clock later.
        assert_eq!(state.now(secs(140)), Some(at(1_700_000_120)));
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_wrap() {
        let mut state = DisplayState::default();
        state.apply(
            &reading_packet(1, 120, 1_700_000_000, 1_700_000_030),
            secs(500),
        );
        assert_eq!(state.now(secs(100)), Some(at(1_700_000_030)));
    }

    #[test]
    fn readings_accumulate_and_repeats_are_recognised() {
        let mut state = DisplayState::default();
        assert_eq!(
            state.apply(&reading_packet(1, 120, 1_000, 1_030), secs(10)),
            Applied::NewReading
        );
        assert_eq!(
            state.apply(&reading_packet(2, 120, 1_000, 1_090), secs(70)),
            Applied::Unchanged
        );
        assert_eq!(
            state.apply(&reading_packet(3, 132, 1_300, 1_330), secs(310)),
            Applied::NewReading
        );
        assert_eq!(state.history().len(), 2);
        assert_eq!(state.latest().expect("latest").glucose.mgdl(), 132);
    }

    #[test]
    fn a_retransmission_still_advances_the_clock() {
        // The reading is old news, but the packet is fresh evidence of what
        // time it is and that the radio works.
        let mut state = DisplayState::default();
        state.apply(&reading_packet(1, 120, 1_000, 1_030), secs(10));
        state.apply(&reading_packet(2, 120, 1_000, 1_600), secs(580));
        assert_eq!(state.now(secs(580)), Some(at(1_600)));
        assert_eq!(state.since_last_packet(secs(600)), Some(secs(20)));
    }

    #[test]
    fn deltas_need_two_nearby_readings() {
        let mut state = DisplayState::default();
        assert_eq!(state.delta_mgdl(), None);
        state.apply(&reading_packet(1, 120, 1_000, 1_030), secs(10));
        assert_eq!(state.delta_mgdl(), None);
        state.apply(&reading_packet(2, 132, 1_300, 1_330), secs(310));
        assert_eq!(state.delta_mgdl(), Some(12));

        // A reading an hour later is not a five-minute delta.
        state.apply(&reading_packet(3, 90, 5_000, 5_030), secs(4_010));
        assert_eq!(state.delta_mgdl(), None);
    }

    #[test]
    fn status_packets_update_the_uplink_view() {
        let mut state = DisplayState::default();
        assert_eq!(state.uplink(), UplinkState::NoRecentData);
        assert_eq!(
            state.apply(&status_packet(1, UplinkState::AuthFailed, 2_000), secs(10)),
            Applied::Status
        );
        assert_eq!(state.uplink(), UplinkState::AuthFailed);
        assert_eq!(state.consecutive_failures(), 3);
        assert_eq!(state.gateway_battery_millivolts(), Some(3_900));
    }

    #[test]
    fn a_new_policy_is_adopted_and_a_repeat_is_not_a_change() {
        let mut state = DisplayState::default();
        let mut policy = AlarmPolicy::DEFAULT;
        policy.thresholds.low_mgdl = 75;

        let packet = Packet::new(1, Message::Policy(policy));
        assert_eq!(state.apply(&packet, secs(10)), Applied::Policy);
        assert_eq!(state.policy().thresholds.low_mgdl, 75);

        // The gateway re-sends the policy on every heartbeat; that is not a
        // reason to repaint an e-ink panel.
        assert!(!state.apply(&packet, secs(20)).changed_something());
    }

    #[test]
    fn an_invalid_policy_is_refused_and_the_old_one_survives() {
        let mut state = DisplayState::default();
        let broken = AlarmPolicy {
            thresholds: Thresholds {
                urgent_low_mgdl: 200,
                low_mgdl: 70,
                high_mgdl: 180,
                urgent_high_mgdl: 250,
            },
            ..AlarmPolicy::DEFAULT
        };
        let applied = state.apply(&Packet::new(1, Message::Policy(broken)), secs(10));
        assert!(!applied.changed_something());
        assert_eq!(state.policy(), AlarmPolicy::DEFAULT);
    }

    #[test]
    fn acks_are_ignored_by_the_display() {
        let mut state = DisplayState::default();
        let applied = state.apply(&Packet::new(1, Message::Ack { acked_seq: 9 }), secs(10));
        assert_eq!(applied, Applied::Ignored);
        assert!(!applied.changed_something());
    }

    #[test]
    fn link_silence_is_tracked_separately_from_sensor_staleness() {
        let mut state = DisplayState::default();
        assert_eq!(state.since_last_packet(secs(100)), None);
        state.apply(&reading_packet(1, 120, 1_000, 1_030), secs(10));
        assert_eq!(state.since_last_packet(secs(310)), Some(secs(300)));
    }

    #[test]
    fn history_is_bounded() {
        let mut state = DisplayState::default();
        for i in 0..(HISTORY_LEN as i64 * 2) {
            let t = 1_000 + i * 300;
            state.apply(
                &reading_packet(i as u16, 120, t, t + 30),
                secs(i as u32 * 300),
            );
        }
        assert_eq!(state.history().len(), HISTORY_LEN);
    }
}
