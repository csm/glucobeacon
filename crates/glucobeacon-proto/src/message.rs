//! What the gateway and the display node say to each other.

use glucobeacon_core::{AlarmPolicy, Reading, Timestamp};
use serde::{Deserialize, Serialize};

/// A framed message plus its sequence number.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Packet {
    /// Wraps at [`u16::MAX`]. Used for de-duplicating retransmissions and for
    /// spotting gaps in the log; it is not a reliability mechanism.
    pub seq: u16,
    /// The message itself.
    pub message: Message,
}

impl Packet {
    /// A packet carrying `message` with sequence number `seq`.
    pub const fn new(seq: u16, message: Message) -> Self {
        Self { seq, message }
    }
}

/// Everything either end can send.
///
/// Variants are append-only within a [protocol
/// version](crate::frame::PROTOCOL_VERSION): postcard encodes an enum as its
/// variant index, so reordering these silently changes the meaning of every
/// frame on the air.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Message {
    /// Gateway → display: a CGM reading. The bread and butter.
    Reading(ReadingReport),
    /// Gateway → display: how the uplink is doing, sent on a slow heartbeat so
    /// the display can distinguish "the sensor is quiet" from "the gateway
    /// fell over".
    Status(Status),
    /// Gateway → display: the alarm policy, so thresholds are configured in one
    /// place and pushed out rather than flashed into both nodes.
    Policy(AlarmPolicy),
    /// Display → gateway: a packet arrived intact. Optional; the gateway
    /// re-sends on a schedule regardless.
    Ack {
        /// The [`Packet::seq`] being acknowledged.
        acked_seq: u16,
    },
}

/// A CGM reading on its way to the display.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReadingReport {
    /// The reading, stamped with when the *sensor* measured it.
    pub reading: Reading,
    /// The gateway's wall clock when it transmitted.
    ///
    /// The display node has no RTC and no network time. This is how it learns
    /// what "now" is, and therefore how it can tell that a reading has gone
    /// stale.
    pub sent_at: Timestamp,
}

/// How the gateway's connection to Dexcom is faring.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Status {
    /// The gateway's wall clock when it transmitted.
    pub sent_at: Timestamp,
    /// The state of the uplink.
    pub uplink: UplinkState,
    /// How many polls in a row have failed. Zero when healthy.
    pub consecutive_failures: u16,
    /// Gateway battery, if it has one to measure.
    pub battery_millivolts: Option<u16>,
}

/// The gateway's view of its own upstream connection.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum UplinkState {
    /// Polling normally and getting data.
    Ok,
    /// No WiFi association or no route out.
    NoNetwork,
    /// Dexcom rejected the credentials. Needs a human, not a retry.
    AuthFailed,
    /// The Share service is reachable but erroring or timing out.
    Unreachable,
    /// Everything works, but Dexcom has no recent readings — the sensor is
    /// warming up, out of range of the phone, or expired.
    NoRecentData,
}

impl UplinkState {
    /// Whether the gateway believes it is currently getting data.
    pub const fn is_healthy(self) -> bool {
        matches!(self, Self::Ok)
    }

    /// Whether retrying is pointless without human intervention.
    pub const fn needs_attention(self) -> bool {
        matches!(self, Self::AuthFailed)
    }

    /// A short label for the display's status line.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoNetwork => "no wifi",
            Self::AuthFailed => "login failed",
            Self::Unreachable => "dexcom down",
            Self::NoRecentData => "no sensor data",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ok_is_healthy() {
        assert!(UplinkState::Ok.is_healthy());
        for state in [
            UplinkState::NoNetwork,
            UplinkState::AuthFailed,
            UplinkState::Unreachable,
            UplinkState::NoRecentData,
        ] {
            assert!(!state.is_healthy(), "{state:?}");
        }
    }

    #[test]
    fn only_auth_failure_needs_a_human() {
        assert!(UplinkState::AuthFailed.needs_attention());
        for state in [
            UplinkState::Ok,
            UplinkState::NoNetwork,
            UplinkState::Unreachable,
            UplinkState::NoRecentData,
        ] {
            assert!(!state.needs_attention(), "{state:?}");
        }
    }

    /// Postcard encodes enum variants by index, so appending is safe and
    /// reordering is not. This pins the current order; changing it should mean
    /// bumping `PROTOCOL_VERSION` deliberately, not by accident.
    #[test]
    fn message_variant_indices_are_pinned() {
        use glucobeacon_core::{Glucose, Trend};

        let reading = Reading::new(
            Glucose::from_mgdl(100),
            Trend::Flat,
            Timestamp::from_unix_secs(0),
        );
        let cases = [
            (
                0u8,
                Message::Reading(ReadingReport {
                    reading,
                    sent_at: Timestamp::EPOCH,
                }),
            ),
            (
                1,
                Message::Status(Status {
                    sent_at: Timestamp::EPOCH,
                    uplink: UplinkState::Ok,
                    consecutive_failures: 0,
                    battery_millivolts: None,
                }),
            ),
            (2, Message::Policy(AlarmPolicy::DEFAULT)),
            (3, Message::Ack { acked_seq: 0 }),
        ];

        let mut buf = [0u8; 256];
        for (index, message) in cases {
            let encoded = postcard::to_slice(&message, &mut buf).expect("encode");
            assert_eq!(encoded[0], index, "{message:?}");
        }
    }
}
