//! Command-line entry point for the glucobeacon display node.
//!
//! Runs the workstation simulator: the panel writes to an image file, the
//! buzzer and LED log, and the button is the Enter key. With `--window` the
//! panel is also mirrored into a window on the host.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::{Duration as StdDuration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use glucobeacon_core::Duration;
use glucobeacon_proto::udp::UdpLink;
use glucobeacon_proto::{Link, LinkError, Message, Packet, link::SeqCounter};
use tracing::{debug, info, warn};

use glucobeacon_display::PANEL_BYTES;
use glucobeacon_display::app::DisplayApp;
use glucobeacon_display::hal::{Button, Buzzer, Indicator, Panel};
use glucobeacon_display::sim::{ConsoleBuzzer, ConsoleLed, PanelFrame, SimPanel, StdinButton};
use glucobeacon_display::ui::{self, Frame, PANEL_HEIGHT, PANEL_WIDTH};

/// How long to block waiting for a frame. Also the tick rate: fast enough that
/// a button press feels instant, slow enough to idle at nearly no CPU.
const TICK: StdDuration = StdDuration::from_millis(200);

#[derive(Parser, Clone, Debug)]
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

    /// Also show the panel in a window on this machine.
    #[cfg(feature = "window")]
    #[arg(long)]
    window: bool,

    /// How many times to magnify the panel in the window (1, 2, 4, or 8).
    ///
    /// The window is resizable either way; this only sets its opening size.
    #[cfg(feature = "window")]
    #[arg(long, value_name = "N", default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=8))]
    window_scale: u8,

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

    #[cfg(feature = "window")]
    if cli.window {
        return run_windowed(&cli);
    }

    run_node(&cli, None, &AtomicBool::new(false), &Arc::default())
}

/// Runs the node with its panel mirrored into a window.
///
/// The window goes on this thread and the node onto a worker, not the other way
/// round: macOS aborts on AppKit calls from anywhere but the main thread.
#[cfg(feature = "window")]
fn run_windowed(cli: &Cli) -> Result<()> {
    use anyhow::anyhow;
    use glucobeacon_display::window::PanelWindow;
    use std::sync::mpsc;

    let (frames_tx, frames_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let press = Arc::new(AtomicBool::new(false));

    let window = PanelWindow::open(PANEL_WIDTH, PANEL_HEIGHT, cli.window_scale)
        .map_err(|error| anyhow!("{error}"))
        .context("opening the panel window")?;

    let node = {
        let cli = cli.clone();
        let stop = Arc::clone(&stop);
        let press = Arc::clone(&press);
        std::thread::Builder::new()
            .name("glucobeacon-node".to_owned())
            .spawn(move || {
                let result = run_node(&cli, Some(frames_tx), &stop, &press);
                // However this ended — a bind failure, a dead link, the window
                // asking it to stop — the window must not be left up waiting for
                // frames that are never coming.
                stop.store(true, Ordering::Release);
                result
            })
            .context("starting the display node")?
    };

    window.run(&frames_rx, &press, &stop);

    node.join()
        .map_err(|_| anyhow!("the display node thread panicked"))?
}

/// The display node itself: link in, panel, buzzer, LED and button out.
///
/// Returns when `stop` is set; without a window nothing ever sets it, and this
/// runs until interrupted. Every refresh is mirrored to `frames` if given, and
/// `press` is the silence button's latch, shared so the window can set it too.
fn run_node(
    cli: &Cli,
    frames: Option<Sender<PanelFrame>>,
    stop: &AtomicBool,
    press: &Arc<AtomicBool>,
) -> Result<()> {
    let mut link = UdpLink::bind(cli.listen, cli.peer, TICK)
        .with_context(|| format!("binding the link to {}", cli.listen))?;
    let mut panel = SimPanel::<PANEL_BYTES>::new(PANEL_WIDTH, PANEL_HEIGHT, &cli.panel)
        .context("sizing the panel framebuffer")?;
    if let Some(frames) = frames {
        panel.mirror_to(frames);
    }
    let mut buzzer = ConsoleBuzzer::new();
    let mut led = ConsoleLed::new();
    let mut button = StdinButton::spawn_with_latch(Arc::clone(press))
        .context("watching stdin for button presses")?;

    let mut app = DisplayApp::default();
    let mut seq = SeqCounter::new();
    let booted = Instant::now();

    info!(
        listen = %cli.listen,
        peer = %cli.peer,
        panel = %cli.panel.display(),
        "display node starting; press Enter to silence an alarm"
    );

    // The first tick always paints, so the window has a frame within one tick
    // of opening without a second refresh being spent here to arrange it.
    while !stop.load(Ordering::Acquire) {
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

    info!(refreshes = panel.flushes(), "display node stopping");
    Ok(())
}

fn repaint(panel: &mut SimPanel<PANEL_BYTES>, app: &DisplayApp, uptime: Duration) -> Result<()> {
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

    #[cfg(feature = "window")]
    #[test]
    fn the_window_is_off_unless_asked_for() {
        // A sim run over SSH or in CI has no display to open one on.
        let cli = Cli::try_parse_from(["glucobeacon-display"]).expect("parse");
        assert!(!cli.window);
        assert_eq!(cli.window_scale, 1);
    }

    #[cfg(feature = "window")]
    #[test]
    fn the_window_scale_is_bounded() {
        let cli = Cli::try_parse_from(["glucobeacon-display", "--window", "--window-scale", "4"])
            .expect("parse");
        assert!(cli.window);
        assert_eq!(cli.window_scale, 4);

        // A scale minifb has no variant for, or one that would open a window
        // larger than any monitor, is a typo rather than a request.
        assert!(Cli::try_parse_from(["glucobeacon-display", "--window-scale", "0"]).is_err());
        assert!(Cli::try_parse_from(["glucobeacon-display", "--window-scale", "16"]).is_err());
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
