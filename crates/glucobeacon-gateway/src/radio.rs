//! Bridges the blocking [`Link`] into the async gateway.
//!
//! Radio drivers are blocking — SPI transfers and busy-waiting on a DIO pin —
//! and the gateway is async because HTTP is. Rather than contort either side,
//! the link runs on its own thread and talks to the gateway over channels. The
//! same shape works for the UDP development link and for a real SX127x driver.

use std::fmt::Display;
use std::thread;

use anyhow::{Context, Result};
use glucobeacon_proto::{Link, Packet};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// How many packets may queue in either direction before senders block.
///
/// The gateway produces a packet every five minutes; anything approaching this
/// depth means the radio thread is wedged.
const QUEUE_DEPTH: usize = 32;

/// The gateway's end of the link thread.
#[derive(Debug)]
pub struct LinkHandle {
    outbound: mpsc::Sender<Packet>,
    inbound: mpsc::Receiver<Packet>,
}

impl LinkHandle {
    /// Queues a packet for transmission.
    ///
    /// Returns an error only if the link thread has stopped, which is fatal.
    pub async fn send(&self, packet: Packet) -> Result<()> {
        self.outbound
            .send(packet)
            .await
            .context("the link thread has stopped")
    }

    /// Waits for the next packet from the display node.
    ///
    /// Resolves to `None` when the link thread stops.
    pub async fn recv(&mut self) -> Option<Packet> {
        self.inbound.recv().await
    }
}

/// Starts the link thread.
///
/// `link`'s receive timeout sets how long an outbound packet may sit in the
/// queue before the thread notices it, so keep it short — a few hundred
/// milliseconds.
pub fn spawn<L>(mut link: L) -> Result<LinkHandle>
where
    L: Link + Send + 'static,
    L::Error: Display,
{
    let (to_link, mut from_gateway) = mpsc::channel::<Packet>(QUEUE_DEPTH);
    let (to_gateway, from_link) = mpsc::channel::<Packet>(QUEUE_DEPTH);

    thread::Builder::new()
        .name("glucobeacon-link".to_owned())
        .spawn(move || {
            loop {
                // Drain everything queued for transmission first: a pending
                // alarm packet should not wait on a receive timeout.
                loop {
                    match from_gateway.try_recv() {
                        Ok(packet) => {
                            if let Err(error) = link.send(&packet) {
                                warn!(%error, seq = packet.seq, "failed to transmit packet");
                            }
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            debug!("gateway dropped the link; stopping the link thread");
                            return;
                        }
                    }
                }

                match link.recv() {
                    Ok(Some(packet)) => {
                        if to_gateway.blocking_send(packet).is_err() {
                            debug!("gateway stopped receiving; stopping the link thread");
                            return;
                        }
                    }
                    // A quiet link is the normal case.
                    Ok(None) => {}
                    // Corruption and foreign traffic are expected on a shared
                    // ISM band. Drop the frame and keep listening.
                    Err(error) => debug!(%error, "discarding an unusable frame"),
                }
            }
        })
        .context("spawning the link thread")?;

    Ok(LinkHandle {
        outbound: to_link,
        inbound: from_link,
    })
}
