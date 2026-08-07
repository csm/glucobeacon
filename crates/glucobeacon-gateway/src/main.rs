//! Command-line entry point for the glucobeacon gateway.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration as StdDuration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::Parser;
use glucobeacon_core::{Reading, Timestamp};
use glucobeacon_dexcom::{Credentials, Region, ReqwestTransport, ShareClient};
use glucobeacon_proto::link::SeqCounter;
use glucobeacon_proto::udp::UdpLink;
use glucobeacon_proto::{Message, Packet, ReadingReport, Status, UplinkState};
use tokio::time::{Instant, sleep_until};
use tracing::{debug, error, info, warn};

use glucobeacon_gateway::config::{Config, ENV_ACCOUNT, ENV_PASSWORD};
use glucobeacon_gateway::radio::{self, LinkHandle};
use glucobeacon_gateway::schedule::PollSchedule;
use glucobeacon_gateway::source::{DemoSource, Source, uplink_state};

/// The gateway binary talks to Dexcom over `reqwest`. An ESP-IDF firmware
/// build swaps this for a transport over ESP-IDF's own HTTP client and reuses
/// everything else.
type GatewaySource = Source<ReqwestTransport>;

/// How long the link thread waits for an inbound frame before looping.
const LINK_POLL_TIMEOUT: StdDuration = StdDuration::from_millis(200);

#[derive(Parser, Debug)]
#[command(
    name = "glucobeacon-gateway",
    version,
    about = "Relays Dexcom CGM readings to a glucobeacon display node"
)]
struct Cli {
    /// Path to a TOML config file. Defaults are used if omitted.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Generate a synthetic glucose waveform instead of contacting Dexcom.
    #[arg(long)]
    demo: bool,

    /// Address to bind the link to.
    #[arg(long, value_name = "ADDR")]
    listen: Option<SocketAddr>,

    /// The display node's link address.
    #[arg(long, value_name = "ADDR")]
    peer: Option<SocketAddr>,

    /// Dexcom Share region: us, ous, or jp.
    #[arg(long, value_name = "REGION")]
    region: Option<Region>,

    /// Poll once, report what came back, and exit. Useful for checking
    /// credentials without starting the relay.
    #[arg(long)]
    once: bool,

    /// Log filter, e.g. `debug` or `glucobeacon_gateway=debug,info`.
    #[arg(long, env = "GLUCOBEACON_LOG", default_value = "info")]
    log: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(&cli.log)?;

    let config = load_config(&cli)?;
    let mut source = build_source(&cli, &config)?;

    if cli.once {
        return poll_once(&mut source).await;
    }

    let link = UdpLink::bind(config.link.listen, config.link.peer, LINK_POLL_TIMEOUT)
        .with_context(|| format!("binding the link to {}", config.link.listen))?;
    info!(
        listen = %config.link.listen,
        peer = %config.link.peer,
        source = %source.description(),
        "gateway starting"
    );

    let link = radio::spawn(link)?;
    run(config, source, link).await
}

/// The relay loop.
async fn run(config: Config, mut source: GatewaySource, mut link: LinkHandle) -> Result<()> {
    let schedule = PollSchedule::new(config.poll);
    let mut seq = SeqCounter::new();

    // Push the alarm policy up front so the display is never running on stale
    // thresholds, then again on every heartbeat in case a packet was lost.
    let policy = config.alarms.to_policy();
    link.send(Packet::new(seq.take(), Message::Policy(policy)))
        .await?;

    let mut last_sent: Option<Timestamp> = None;
    let mut consecutive_failures: u32 = 0;
    let mut uplink = UplinkState::NoRecentData;

    let mut next_poll = Instant::now();
    let mut next_heartbeat = Instant::now() + schedule.heartbeat_interval();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("shutting down");
                return Ok(());
            }

            packet = link.recv() => {
                match packet {
                    Some(packet) => handle_inbound(packet),
                    None => bail!("the link thread stopped unexpectedly"),
                }
            }

            _ = sleep_until(next_poll.min(next_heartbeat)) => {
                let now = wall_clock();

                if Instant::now() >= next_poll {
                    let result = source.poll(now, last_sent).await;
                    uplink = uplink_state(&result);

                    match result {
                        Ok(readings) => {
                            consecutive_failures = 0;
                            for reading in &readings {
                                send_reading(&link, &mut seq, reading, now).await?;
                                last_sent = Some(reading.at);
                            }
                            if readings.is_empty() {
                                debug!("poll returned nothing new");
                            }
                        }
                        Err(error) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            if error.is_retryable() {
                                warn!(%error, consecutive_failures, "poll failed");
                            } else {
                                // Backoff still applies: retrying cannot fix
                                // this, but neither can giving up quietly, and
                                // the display needs to keep hearing about it.
                                error!(%error, "poll failed and will not succeed without attention");
                            }
                        }
                    }

                    let delay = schedule.next_delay(wall_clock(), last_sent, consecutive_failures);
                    debug!(delay_secs = delay.as_secs(), "scheduled the next poll");
                    next_poll = Instant::now() + delay;
                }

                if Instant::now() >= next_heartbeat {
                    send_status(&link, &mut seq, uplink, consecutive_failures, now).await?;
                    link.send(Packet::new(seq.take(), Message::Policy(policy))).await?;
                    next_heartbeat = Instant::now() + schedule.heartbeat_interval();
                }
            }
        }
    }
}

async fn send_reading(
    link: &LinkHandle,
    seq: &mut SeqCounter,
    reading: &Reading,
    now: Timestamp,
) -> Result<()> {
    info!(
        mgdl = reading.glucose.mgdl(),
        trend = reading.trend.arrow(),
        age_secs = reading.age(now).as_secs(),
        "relaying reading"
    );
    link.send(Packet::new(
        seq.take(),
        Message::Reading(ReadingReport {
            reading: *reading,
            sent_at: now,
        }),
    ))
    .await
}

async fn send_status(
    link: &LinkHandle,
    seq: &mut SeqCounter,
    uplink: UplinkState,
    consecutive_failures: u32,
    now: Timestamp,
) -> Result<()> {
    debug!(
        uplink = uplink.label(),
        consecutive_failures, "sending status"
    );
    link.send(Packet::new(
        seq.take(),
        Message::Status(Status {
            sent_at: now,
            uplink,
            consecutive_failures: consecutive_failures.try_into().unwrap_or(u16::MAX),
            battery_millivolts: None,
        }),
    ))
    .await
}

fn handle_inbound(packet: Packet) {
    match packet.message {
        Message::Ack { acked_seq } => debug!(acked_seq, "display acknowledged a packet"),
        other => debug!(?other, "ignoring an unexpected packet from the display"),
    }
}

/// A single poll, for checking configuration.
async fn poll_once(source: &mut GatewaySource) -> Result<()> {
    let now = wall_clock();
    let readings = source
        .poll(now, None)
        .await
        .context("polling the reading source")?;

    if readings.is_empty() {
        println!("no readings available in the lookback window");
        return Ok(());
    }
    for reading in readings {
        println!(
            "{:>3} mg/dL ({:>2}.{} mmol/L) {} {}s old",
            reading.glucose.mgdl(),
            reading.glucose.mmol_l_tenths() / 10,
            reading.glucose.mmol_l_tenths() % 10,
            reading.trend.arrow(),
            reading.age(now).as_secs(),
        );
    }
    Ok(())
}

fn load_config(cli: &Cli) -> Result<Config> {
    let mut config = match &cli.config {
        Some(path) => Config::load(path)?,
        None => Config::default(),
    };
    if let Some(listen) = cli.listen {
        config.link.listen = listen;
    }
    if let Some(peer) = cli.peer {
        config.link.peer = peer;
    }
    if let Some(region) = cli.region {
        config.dexcom.region = region;
    }
    config.validate()?;
    Ok(config)
}

fn build_source(cli: &Cli, config: &Config) -> Result<GatewaySource> {
    if cli.demo {
        return Ok(Source::Demo(DemoSource::new(wall_clock())));
    }

    let Some(account) = config.account_name() else {
        bail!(
            "no Dexcom account configured: set {ENV_ACCOUNT}, or `dexcom.account` in the config \
             file, or pass --demo to run without Dexcom"
        );
    };
    let Some(password) = config.password() else {
        bail!("no Dexcom password: set {ENV_PASSWORD} (it is intentionally not a config field)");
    };

    let client = ShareClient::new(Credentials::new(account, password), config.dexcom.region)
        .context("building the Dexcom Share client")?;
    Ok(Source::Share(Box::new(client)))
}

/// The current wall clock, as the protocol's timestamp.
///
/// A gateway whose clock is before the epoch has bigger problems than this
/// saturating to zero.
fn wall_clock() -> Timestamp {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Timestamp::from_unix_secs(secs)
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
    fn cli_parses_the_documented_flags() {
        let cli = Cli::try_parse_from([
            "glucobeacon-gateway",
            "--demo",
            "--listen",
            "127.0.0.1:1111",
            "--peer",
            "127.0.0.1:2222",
            "--region",
            "ous",
        ])
        .expect("parse");

        assert!(cli.demo);
        assert_eq!(cli.listen.expect("listen").port(), 1111);
        assert_eq!(cli.peer.expect("peer").port(), 2222);
        assert_eq!(cli.region, Some(Region::Ous));
    }

    #[test]
    fn cli_rejects_an_unknown_region() {
        assert!(Cli::try_parse_from(["glucobeacon-gateway", "--region", "mars"]).is_err());
    }

    #[test]
    fn cli_flags_override_the_config_file() {
        let cli = Cli::try_parse_from([
            "glucobeacon-gateway",
            "--listen",
            "0.0.0.0:7000",
            "--peer",
            "10.0.0.5:7001",
        ])
        .expect("parse");

        let config = load_config(&cli).expect("load");
        assert_eq!(config.link.listen.port(), 7000);
        assert_eq!(config.link.peer.to_string(), "10.0.0.5:7001");
    }

    #[test]
    fn demo_mode_needs_no_credentials() {
        let cli = Cli::try_parse_from(["glucobeacon-gateway", "--demo"]).expect("parse");
        let source = build_source(&cli, &Config::default()).expect("build");
        assert!(matches!(source, Source::Demo(_)));
    }

    #[test]
    fn the_wall_clock_is_after_the_epoch() {
        assert!(wall_clock().as_unix_secs() > 1_700_000_000);
    }
}
