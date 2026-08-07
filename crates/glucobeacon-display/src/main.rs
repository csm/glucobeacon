//! Command-line entry point for the glucobeacon display node.
//!
//! Runs the workstation simulator: the panel writes to an image file, the
//! buzzer and LED log, and the button is the Enter key.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration as StdDuration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use glucobeacon_core::Duration;
use glucobeacon_proto::udp::UdpLink;
use glucobeacon_proto::{Link, LinkError, Message, Packet, link::SeqCounter};
use tracing::{debug, info, warn};

use glucobeacon_display::app::DisplayApp;
use glucobeacon_display::hal::{Button, Buzzer, Indicator, Panel};
use glucobeacon_display::sim::{ConsoleBuzzer, ConsoleLed, SimPanel, StdinButton};
use glucobeacon_display::ui::{self, Frame, PANEL_HEIGHT, PANEL_WIDTH};

/// How long to block waiting for a frame. Also the tick rate: fast enough that
/// a button press feels instant, slow enough to idle at nearly no CPU.
const TICK: StdDuration = StdDuration::from_millis(200);

#[derive(Parser, Debug)]
#[command(
    name = "glucobeacon-display",
    version,
    about = "Receives CGM readings over the link and drives the e-ink display node"
)]
struct Cli {
    /// Address to bind the link to.
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:9102")]
    listen: SocketAddr,

    /// The gateway's link address.
    #[arg(long, value_name = "ADDR", default_value = "127.0.0.1:9101")]
    peer: SocketAddr,

    /// Where to write the simulated panel image on each refresh.
    #[arg(long, value_name = "PATH", default_value = "glucobeacon-panel.pbm")]
    panel: PathBuf,

    /// Acknowledge each packet back to the gateway.
    #[arg(long)]
    ack: bool,

    /// Log filter, e.g. `debug`.
    #[arg(long, env = "GLUCOBEACON_LOG", default_value = "info")]
    log: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log)?;

    let mut link = UdpLink::bind(cli.listen, cli.peer, TICK)
        .with_context(|| format!("binding the link to {}", cli.listen))?;
    let mut panel = SimPanel::new(PANEL_WIDTH, PANEL_HEIGHT, &cli.panel);
    let mut buzzer = ConsoleBuzzer::new();
    let mut led = ConsoleLed::new();
    let mut button = StdinButton::spawn().context("watching stdin for button presses")?;

    let mut app = DisplayApp::default();
    let mut seq = SeqCounter::new();
    let booted = Instant::now();

    info!(
        listen = %cli.listen,
        peer = %cli.peer,
        panel = %cli.panel.display(),
        "display node starting; press Enter to silence an alarm"
    );

    loop {
        // The link's receive timeout is what paces this loop.
        match link.recv() {
            Ok(Some(packet)) => {
                let uptime = uptime(booted);
                let applied = app.handle_packet(&packet, uptime);
                debug!(seq = packet.seq, ?applied, "received packet");
                if cli.ack {
                    let ack = Packet::new(
                        seq.take(),
                        Message::Ack {
                            acked_seq: packet.seq,
                        },
                    );
                    if let Err(error) = link.send(&ack) {
                        warn!(%error, "could not acknowledge");
                    }
                }
            }
            Ok(None) => {}
            // Corrupt frames and foreign traffic are ordinary on a shared band.
            Err(LinkError::Frame(error)) => debug!(%error, "discarded a frame"),
            Err(LinkError::Transport(error)) => {
                return Err(error).context("the link failed");
            }
        }

        let uptime = uptime(booted);
        let tick = app.tick(uptime);

        if let Some(event) = tick.event {
            info!(?event, "alarm");
        }
        if let Some(pattern) = tick.buzzer {
            buzzer.set_pattern(pattern).context("driving the buzzer")?;
        }
        if let Some(state) = tick.led {
            led.set(state).context("driving the LED")?;
        }

        if button.take_press().context("reading the button")? {
            if app.press_silence(uptime) {
                info!("silence button pressed");
                // Apply the resulting quiet immediately rather than waiting for
                // the next loop: the button has to feel like it did something.
                let tick = app.tick(uptime);
                if let Some(pattern) = tick.buzzer {
                    buzzer.set_pattern(pattern).context("driving the buzzer")?;
                }
                if let Some(state) = tick.led {
                    led.set(state).context("driving the LED")?;
                }
                repaint(&mut panel, &app, uptime)?;
            } else {
                debug!("button pressed with no alarm active");
            }
        } else if tick.redraw {
            repaint(&mut panel, &app, uptime)?;
        }
    }
}

fn repaint(panel: &mut SimPanel, app: &DisplayApp, uptime: Duration) -> Result<()> {
    let frame = Frame {
        now: app.now(uptime),
        uptime,
        state: app.state(),
        alarm: app.alarm(),
        silence_remaining: app.silence_remaining(uptime),
    };
    ui::render(panel, &frame).context("rendering the panel")?;
    panel.flush().context("refreshing the panel")?;
    Ok(())
}

/// Seconds since boot.
///
/// On the device this is a hardware timer; the point is only that it is
/// monotonic and independent of the wall clock, which this node does not have.
fn uptime(booted: Instant) -> Duration {
    Duration::from_secs(booted.elapsed().as_secs() as u32)
}

fn init_tracing(filter: &str) -> Result<()> {
    use tracing_subscriber::EnvFilter;

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(filter).context("parsing the log filter")?)
        .with_target(false)
        .init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_pair_with_the_gateway_defaults() {
        let cli = Cli::try_parse_from(["glucobeacon-display"]).expect("parse");
        // The gateway defaults to listening on 9101 and sending to 9102.
        assert_eq!(cli.listen.to_string(), "127.0.0.1:9102");
        assert_eq!(cli.peer.to_string(), "127.0.0.1:9101");
        assert!(!cli.ack);
    }

    #[test]
    fn addresses_and_the_panel_path_can_be_overridden() {
        let cli = Cli::try_parse_from([
            "glucobeacon-display",
            "--listen",
            "0.0.0.0:5000",
            "--peer",
            "10.0.0.2:5001",
            "--panel",
            "/tmp/panel.pbm",
            "--ack",
        ])
        .expect("parse");

        assert_eq!(cli.listen.port(), 5000);
        assert_eq!(cli.peer.to_string(), "10.0.0.2:5001");
        assert_eq!(cli.panel, PathBuf::from("/tmp/panel.pbm"));
        assert!(cli.ack);
    }

    #[test]
    fn a_bad_address_is_rejected() {
        assert!(Cli::try_parse_from(["glucobeacon-display", "--listen", "nope"]).is_err());
    }

    #[test]
    fn uptime_starts_at_zero() {
        assert_eq!(uptime(Instant::now()).as_secs(), 0);
    }
}
