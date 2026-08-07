//! Byte layout of a packet on the air.
//!
//! ```text
//! +--------+--------+---------+--------+=============+--------+--------+
//! | 'G'    | 'B'    | version | len    | payload     | crc lo | crc hi |
//! +--------+--------+---------+--------+=============+--------+--------+
//!  0        1        2         3        4             4+len
//! ```
//!
//! The payload is a postcard-encoded [`Packet`]. The
//! CRC covers the version byte, the length byte, and the payload — not the
//! magic, which is only there to reject obvious garbage cheaply.
//!
//! LoRa hands us whole packets or nothing, so there is no framing delimiter and
//! no escaping: one radio packet is one frame. The length byte is still carried
//! because some radio drivers hand back a fixed-size buffer with the tail
//! undefined.

use crc::{CRC_16_IBM_SDLC, Crc};

use crate::message::Packet;

/// Bumped whenever the payload encoding changes incompatibly.
///
/// A node that receives a version it does not know drops the frame rather than
/// guessing, so a half-upgraded pair goes quiet instead of showing wrong
/// numbers.
pub const PROTOCOL_VERSION: u8 = 1;

/// Magic bytes at the start of every frame.
pub const MAGIC: [u8; 2] = *b"GB";

/// Magic + version + length.
pub const HEADER_LEN: usize = 4;

/// The CRC.
pub const TRAILER_LEN: usize = 2;

/// The largest payload a frame may carry.
///
/// SX127x/SX126x radios cap an explicit-header packet at 255 bytes; staying
/// well under that keeps airtime short at the slow spreading factors this link
/// needs for range.
pub const MAX_PAYLOAD_LEN: usize = 200;

/// The largest frame [`encode`] will produce.
pub const MAX_FRAME_LEN: usize = HEADER_LEN + MAX_PAYLOAD_LEN + TRAILER_LEN;

/// CRC-16/IBM-SDLC (a.k.a. X.25), the same check LoRaWAN uses.
const CRC: Crc<u16> = Crc::<u16>::new(&CRC_16_IBM_SDLC);

/// Why a frame could not be encoded or decoded.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum FrameError {
    /// The output buffer cannot hold the frame.
    BufferTooSmall,
    /// The input is shorter than a frame with an empty payload.
    TooShort,
    /// The frame does not begin with [`MAGIC`].
    BadMagic,
    /// The frame announces a protocol version this build does not speak.
    UnsupportedVersion(u8),
    /// The frame's length byte disagrees with how many bytes are present.
    LengthMismatch,
    /// The CRC did not match: the radio handed us a corrupted packet.
    BadChecksum,
    /// The payload is not a well-formed [`Packet`].
    MalformedPayload,
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferTooSmall => f.write_str("output buffer too small for frame"),
            Self::TooShort => f.write_str("frame shorter than the minimum"),
            Self::BadMagic => f.write_str("frame does not start with the protocol magic"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported protocol version {v}"),
            Self::LengthMismatch => f.write_str("declared payload length does not match the frame"),
            Self::BadChecksum => f.write_str("frame checksum mismatch"),
            Self::MalformedPayload => f.write_str("payload is not a valid packet"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for FrameError {}

/// Serializes `packet` into `buf`, returning the frame to hand the radio.
pub fn encode<'a>(packet: &Packet, buf: &'a mut [u8]) -> Result<&'a [u8], FrameError> {
    let capacity = buf
        .len()
        .checked_sub(HEADER_LEN + TRAILER_LEN)
        .ok_or(FrameError::BufferTooSmall)?
        .min(MAX_PAYLOAD_LEN);

    let payload_len = postcard::to_slice(packet, &mut buf[HEADER_LEN..HEADER_LEN + capacity])
        .map_err(|_| FrameError::BufferTooSmall)?
        .len();

    buf[0..MAGIC.len()].copy_from_slice(&MAGIC);
    buf[2] = PROTOCOL_VERSION;
    // `capacity` is capped at MAX_PAYLOAD_LEN (200), so this always fits a byte.
    buf[3] = payload_len as u8;

    let end = HEADER_LEN + payload_len;
    let checksum = CRC.checksum(&buf[2..end]).to_le_bytes();
    buf[end..end + TRAILER_LEN].copy_from_slice(&checksum);

    Ok(&buf[..end + TRAILER_LEN])
}

/// Parses a frame received from the radio.
pub fn decode(frame: &[u8]) -> Result<Packet, FrameError> {
    if frame.len() < HEADER_LEN + TRAILER_LEN {
        return Err(FrameError::TooShort);
    }
    if frame[0..MAGIC.len()] != MAGIC {
        return Err(FrameError::BadMagic);
    }
    if frame[2] != PROTOCOL_VERSION {
        return Err(FrameError::UnsupportedVersion(frame[2]));
    }

    let payload_len = frame[3] as usize;
    let end = HEADER_LEN + payload_len;
    // Trailing bytes are tolerated: some radio drivers return a fixed-size
    // buffer. Fewer bytes than declared is a truncated packet.
    if payload_len > MAX_PAYLOAD_LEN || frame.len() < end + TRAILER_LEN {
        return Err(FrameError::LengthMismatch);
    }

    let expected = u16::from_le_bytes([frame[end], frame[end + 1]]);
    if CRC.checksum(&frame[2..end]) != expected {
        return Err(FrameError::BadChecksum);
    }

    postcard::from_bytes(&frame[HEADER_LEN..end]).map_err(|_| FrameError::MalformedPayload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, ReadingReport};
    use glucobeacon_core::{Glucose, Reading, Timestamp, Trend};

    fn sample() -> Packet {
        Packet {
            seq: 42,
            message: Message::Reading(ReadingReport {
                reading: Reading::new(
                    Glucose::from_mgdl(137),
                    Trend::FortyFiveDown,
                    Timestamp::from_unix_secs(1_700_000_100),
                ),
                sent_at: Timestamp::from_unix_secs(1_700_000_130),
            }),
        }
    }

    #[test]
    fn round_trips() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let frame = encode(&sample(), &mut buf).expect("encode");
        assert_eq!(decode(frame).expect("decode"), sample());
    }

    #[test]
    fn a_reading_frame_fits_a_short_lora_packet() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let frame = encode(&sample(), &mut buf).expect("encode");
        assert!(frame.len() <= 32, "reading frame was {} bytes", frame.len());
    }

    #[test]
    fn every_message_variant_round_trips() {
        use crate::message::{Status, UplinkState};
        use glucobeacon_core::AlarmPolicy;

        let messages = [
            sample().message,
            Message::Status(Status {
                sent_at: Timestamp::from_unix_secs(1_700_000_130),
                uplink: UplinkState::AuthFailed,
                consecutive_failures: 7,
                battery_millivolts: Some(3_912),
            }),
            Message::Policy(AlarmPolicy::DEFAULT),
            Message::Ack { acked_seq: 41 },
        ];
        for message in messages {
            let packet = Packet { seq: 1, message };
            let mut buf = [0u8; MAX_FRAME_LEN];
            let frame = encode(&packet, &mut buf).expect("encode");
            assert!(frame.len() <= MAX_FRAME_LEN);
            assert_eq!(decode(frame).expect("decode"), packet);
        }
    }

    #[test]
    fn a_flipped_bit_anywhere_is_caught() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = encode(&sample(), &mut buf).expect("encode").len();

        for byte in 0..len {
            for bit in 0..8u8 {
                let mut corrupted = buf;
                corrupted[byte] ^= 1 << bit;
                let result = decode(&corrupted[..len]);
                assert!(
                    result.is_err(),
                    "bit {bit} of byte {byte} flipped and decode still succeeded"
                );
            }
        }
    }

    #[test]
    fn truncated_frames_are_rejected() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = encode(&sample(), &mut buf).expect("encode").len();
        for short in 0..len {
            assert!(decode(&buf[..short]).is_err(), "accepted {short} bytes");
        }
    }

    #[test]
    fn trailing_garbage_is_ignored() {
        // Radio drivers that hand back a fixed-size buffer leave junk past the
        // packet; the length byte is what decides where the frame ends.
        let mut buf = [0xAAu8; MAX_FRAME_LEN];
        let len = encode(&sample(), &mut buf).expect("encode").len();
        assert_eq!(decode(&buf[..]).expect("decode"), sample());
        assert!(len < buf.len());
    }

    #[test]
    fn foreign_traffic_is_rejected_cheaply() {
        assert_eq!(decode(&[0u8; 16]), Err(FrameError::BadMagic));
        assert_eq!(decode(b"not a frame"), Err(FrameError::BadMagic));
    }

    #[test]
    fn a_future_protocol_version_is_refused_not_guessed() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = encode(&sample(), &mut buf).expect("encode").len();
        buf[2] = PROTOCOL_VERSION + 1;
        assert_eq!(
            decode(&buf[..len]),
            Err(FrameError::UnsupportedVersion(PROTOCOL_VERSION + 1))
        );
    }

    #[test]
    fn an_undersized_buffer_fails_instead_of_panicking() {
        let mut tiny = [0u8; 4];
        assert_eq!(
            encode(&sample(), &mut tiny),
            Err(FrameError::BufferTooSmall)
        );
        let mut empty: [u8; 0] = [];
        assert_eq!(
            encode(&sample(), &mut empty),
            Err(FrameError::BufferTooSmall)
        );
    }

    #[test]
    fn an_overlong_declared_length_is_rejected() {
        let mut buf = [0u8; MAX_FRAME_LEN];
        let len = encode(&sample(), &mut buf).expect("encode").len();
        buf[3] = 255;
        assert_eq!(decode(&buf[..len]), Err(FrameError::LengthMismatch));
    }
}
