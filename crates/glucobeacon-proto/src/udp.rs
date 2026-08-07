//! A [`Link`] over UDP, for running both ends on one machine.
//!
//! This is development scaffolding, not a shipping transport: it exists so the
//! gateway and the display binaries can be run side by side and exercised end
//! to end before any radio hardware is on the bench. It deliberately mimics the
//! properties of the real link — datagram-oriented, unreliable, unordered — so
//! code that works against it has no excuse to break against a radio.

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use crate::link::Link;

/// A datagram socket pretending to be a LoRa modem.
#[derive(Debug)]
pub struct UdpLink {
    socket: UdpSocket,
    peer: SocketAddr,
}

impl UdpLink {
    /// Binds `local` and sends everything to `peer`.
    ///
    /// `recv_timeout` bounds how long [`Link::recv_frame`] blocks before
    /// reporting that nothing arrived.
    pub fn bind(local: SocketAddr, peer: SocketAddr, recv_timeout: Duration) -> io::Result<Self> {
        let socket = UdpSocket::bind(local)?;
        socket.set_read_timeout(Some(recv_timeout))?;
        Ok(Self { socket, peer })
    }

    /// The address actually bound, which is what to use when `local` asked for
    /// port 0.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// The address frames are sent to.
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    /// Points the link at a different peer.
    pub fn set_peer(&mut self, peer: SocketAddr) {
        self.peer = peer;
    }
}

impl Link for UdpLink {
    type Error = io::Error;

    fn send_frame(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        match self.socket.send_to(frame, self.peer) {
            Ok(sent) if sent == frame.len() => Ok(()),
            Ok(sent) => Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("sent {sent} of {} frame bytes", frame.len()),
            )),
            // Nobody is listening yet. A radio would not complain either.
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn recv_frame(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        match self.socket.recv_from(buf) {
            Ok((len, _from)) => Ok(Some(len)),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                Ok(None)
            }
            // An ICMP port-unreachable from an earlier send surfaces on the
            // next recv on some platforms. It says nothing about this read.
            Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{Message, Packet, ReadingReport};
    use glucobeacon_core::{Glucose, Reading, Timestamp, Trend};

    fn localhost(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[test]
    fn two_links_can_talk_to_each_other() {
        let timeout = Duration::from_millis(250);
        let mut a = UdpLink::bind(localhost(0), localhost(0), timeout).expect("bind a");
        let a_addr = a.local_addr().expect("addr a");
        let mut b = UdpLink::bind(localhost(0), a_addr, timeout).expect("bind b");
        a.set_peer(b.local_addr().expect("addr b"));

        let packet = Packet::new(
            3,
            Message::Reading(ReadingReport {
                reading: Reading::new(
                    Glucose::from_mgdl(88),
                    Trend::FortyFiveDown,
                    Timestamp::from_unix_secs(1_700_000_000),
                ),
                sent_at: Timestamp::from_unix_secs(1_700_000_005),
            }),
        );

        a.send(&packet).expect("send");
        assert_eq!(b.recv().expect("recv"), Some(packet));
    }

    #[test]
    fn a_silent_link_times_out_rather_than_blocking_forever() {
        let mut link =
            UdpLink::bind(localhost(0), localhost(1), Duration::from_millis(50)).expect("bind");
        assert_eq!(link.recv().expect("recv"), None);
    }
}
