//! The transport the protocol runs over.
//!
//! Both ends of the product talk to an SX127x-class LoRa module, but neither
//! application should care: they take a [`Link`], and the radio driver, a UDP
//! socket, or a test double all satisfy it equally. That is what lets the
//! gateway and the display run against each other on a laptop with no hardware
//! involved.

use crate::frame::{self, FrameError, MAX_FRAME_LEN};
use crate::message::Packet;

/// A failure either in the transport or in the bytes it carried.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum LinkError<E> {
    /// The radio or socket itself failed.
    Transport(E),
    /// Bytes arrived but were not a valid frame. Over a radio this is normal
    /// and means "drop it and wait for the next one", not "give up".
    Frame(FrameError),
}

impl<E: core::fmt::Display> core::fmt::Display for LinkError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "link transport error: {e}"),
            Self::Frame(e) => write!(f, "malformed frame: {e}"),
        }
    }
}

#[cfg(feature = "std")]
impl<E: core::fmt::Debug + core::fmt::Display> std::error::Error for LinkError<E> {}

impl<E> From<FrameError> for LinkError<E> {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

/// A packet-oriented, unreliable, bidirectional link.
///
/// Implementations move whole frames or nothing — no partial reads — which is
/// what a LoRa modem gives you and what the framing assumes.
pub trait Link {
    /// What can go wrong in the transport itself.
    type Error;

    /// Transmits one frame. Returns once the frame is handed off, which for a
    /// radio means the modem has it, not that anyone received it.
    fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error>;

    /// Receives at most one frame into `buf`.
    ///
    /// Returns `Ok(None)` when nothing arrived before the implementation's
    /// timeout — the ordinary case on a link that is quiet for five minutes at
    /// a stretch, not an error.
    fn recv_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error>;

    /// Encodes and transmits `packet`.
    fn send(&mut self, packet: &Packet) -> Result<(), LinkError<Self::Error>> {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let encoded = frame::encode(packet, &mut buf)?;
        // `encoded` borrows `buf`; copy the length out so the borrow ends.
        let len = encoded.len();
        self.send_frame(&buf[..len]).map_err(LinkError::Transport)
    }

    /// Receives and decodes one packet, if one arrived.
    fn recv(&mut self) -> Result<Option<Packet>, LinkError<Self::Error>> {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let Some(len) = self.recv_frame(&mut buf).map_err(LinkError::Transport)? else {
            return Ok(None);
        };
        Ok(Some(frame::decode(&buf[..len])?))
    }
}

/// Sequence numbers for outgoing packets.
///
/// Wraps at [`u16::MAX`]; a gateway sending one packet every five minutes takes
/// about seven months to get there, and nothing depends on monotonicity beyond
/// spotting adjacent duplicates.
#[derive(Clone, Debug, Default)]
pub struct SeqCounter {
    next: u16,
}

impl SeqCounter {
    /// A counter starting at zero.
    pub const fn new() -> Self {
        Self { next: 0 }
    }

    /// Returns the next sequence number and advances.
    pub fn take(&mut self) -> u16 {
        let seq = self.next;
        self.next = self.next.wrapping_add(1);
        seq
    }

    /// The sequence number [`take`](Self::take) will return next.
    pub const fn peek(&self) -> u16 {
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, ReadingReport};
    use glucobeacon_core::{Glucose, Reading, Timestamp, Trend};

    /// A link that hands back whatever was written to it, plus a knob for
    /// injecting the failures a real radio produces.
    #[derive(Default)]
    struct LoopbackLink {
        queue: std::collections::VecDeque<std::vec::Vec<u8>>,
        corrupt_next: bool,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Unplugged;

    impl Link for LoopbackLink {
        type Error = Unplugged;

        fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
            let mut copy = frame.to_vec();
            if self.corrupt_next {
                copy[HEADER_OFFSET] ^= 0xFF;
                self.corrupt_next = false;
            }
            self.queue.push_back(copy);
            Ok(())
        }

        fn recv_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
            let Some(frame) = self.queue.pop_front() else {
                return Ok(None);
            };
            buf[..frame.len()].copy_from_slice(&frame);
            Ok(Some(frame.len()))
        }
    }

    /// Index of a payload byte, past the header.
    const HEADER_OFFSET: usize = 5;

    fn packet(seq: u16) -> Packet {
        Packet::new(
            seq,
            Message::Reading(ReadingReport {
                reading: Reading::new(
                    Glucose::from_mgdl(101),
                    Trend::Flat,
                    Timestamp::from_unix_secs(1_700_000_000),
                ),
                sent_at: Timestamp::from_unix_secs(1_700_000_010),
            }),
        )
    }

    #[test]
    fn packets_survive_a_round_trip_through_a_link() {
        let mut link = LoopbackLink::default();
        link.send(&packet(7)).expect("send");
        assert_eq!(link.recv().expect("recv"), Some(packet(7)));
    }

    #[test]
    fn a_quiet_link_is_not_an_error() {
        let mut link = LoopbackLink::default();
        assert_eq!(link.recv().expect("recv"), None);
    }

    #[test]
    fn corruption_surfaces_as_a_frame_error_not_a_transport_error() {
        let mut link = LoopbackLink {
            corrupt_next: true,
            ..Default::default()
        };
        link.send(&packet(7)).expect("send");
        match link.recv() {
            Err(LinkError::Frame(_)) => {}
            other => panic!("expected a frame error, got {other:?}"),
        }
        // And the link keeps working afterwards.
        link.send(&packet(8)).expect("send");
        assert_eq!(link.recv().expect("recv"), Some(packet(8)));
    }

    #[test]
    fn sequence_numbers_advance_and_wrap() {
        let mut seq = SeqCounter::new();
        assert_eq!(seq.peek(), 0);
        assert_eq!(seq.take(), 0);
        assert_eq!(seq.take(), 1);

        let mut seq = SeqCounter { next: u16::MAX };
        assert_eq!(seq.take(), u16::MAX);
        assert_eq!(seq.take(), 0);
    }
}
