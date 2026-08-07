//! Gateway configuration: a TOML file plus environment overrides.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};
use glucobeacon_core::{AlarmPolicy, Duration, Thresholds};
use glucobeacon_dexcom::Region;
use serde::{Deserialize, Serialize};

/// Environment variable holding the Share account name.
pub const ENV_ACCOUNT: &str = "GLUCOBEACON_DEXCOM_ACCOUNT";
/// Environment variable holding the Share password.
///
/// The password is deliberately *not* a config-file field: config files get
/// committed, copied into issues, and pasted into chat.
pub const ENV_PASSWORD: &str = "GLUCOBEACON_DEXCOM_PASSWORD";

/// Everything the gateway needs to run.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Upstream Dexcom Share account.
    pub dexcom: DexcomConfig,
    /// The radio link to the display node.
    pub link: LinkConfig,
    /// Polling cadence.
    pub poll: PollConfig,
    /// Alarm thresholds, pushed to the display node so they live in one place.
    pub alarms: AlarmConfig,
}

impl Config {
    /// Reads a config file.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config from {}", path.display()))?;
        let config: Config = toml::from_str(&text)
            .with_context(|| format!("parsing config from {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Checks the parts that cannot be expressed in the type.
    pub fn validate(&self) -> Result<()> {
        self.alarms
            .to_policy()
            .validate()
            .map_err(|e| anyhow::anyhow!("invalid alarm thresholds: {e}"))?;
        if self.poll.sensor_interval_secs == 0 {
            bail!("poll.sensor_interval_secs must be greater than zero");
        }
        if self.poll.retry_max_secs < self.poll.retry_base_secs {
            bail!("poll.retry_max_secs must be at least poll.retry_base_secs");
        }
        if self.link.listen == self.link.peer {
            bail!("link.listen and link.peer must differ");
        }
        Ok(())
    }

    /// The Share account name, preferring the environment.
    pub fn account_name(&self) -> Option<String> {
        std::env::var(ENV_ACCOUNT)
            .ok()
            .filter(|value| !value.is_empty())
            .or_else(|| self.dexcom.account.clone())
    }

    /// The Share password. Environment only.
    pub fn password(&self) -> Option<String> {
        std::env::var(ENV_PASSWORD)
            .ok()
            .filter(|value| !value.is_empty())
    }
}

/// Upstream account settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DexcomConfig {
    /// The follower account name. May also come from [`ENV_ACCOUNT`].
    pub account: Option<String>,
    /// Which Share deployment the account lives on.
    pub region: Region,
}

impl Default for DexcomConfig {
    fn default() -> Self {
        Self {
            account: None,
            region: Region::Us,
        }
    }
}

/// Where the radio link lives.
///
/// These are UDP addresses for the development link. A real build points at a
/// SPI-attached radio instead, and these become unused.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct LinkConfig {
    /// Local address to bind.
    pub listen: SocketAddr,
    /// The display node's address.
    pub peer: SocketAddr,
}

impl Default for LinkConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 9101)),
            peer: SocketAddr::from(([127, 0, 0, 1], 9102)),
        }
    }
}

/// How often to ask Dexcom for data.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PollConfig {
    /// How often the sensor produces a reading. Five minutes, for every Dexcom
    /// sensor currently shipping.
    pub sensor_interval_secs: u32,
    /// How long after a reading is due to wait before asking, so the phone has
    /// time to upload it.
    pub settle_secs: u32,
    /// A floor on the gap between polls, so a misconfigured interval cannot
    /// turn into a hot loop against Dexcom.
    pub min_interval_secs: u32,
    /// First retry delay after a failure.
    pub retry_base_secs: u32,
    /// Ceiling on the exponential backoff.
    pub retry_max_secs: u32,
    /// How often to send a status heartbeat even when nothing changed, so the
    /// display can tell a quiet sensor from a dead gateway.
    pub heartbeat_secs: u32,
}

impl Default for PollConfig {
    fn default() -> Self {
        Self {
            sensor_interval_secs: 300,
            settle_secs: 20,
            min_interval_secs: 30,
            retry_base_secs: 30,
            retry_max_secs: 300,
            heartbeat_secs: 600,
        }
    }
}

/// Alarm thresholds, as they appear in the config file.
///
/// A flat mirror of [`Thresholds`] plus the timing knobs, so the TOML reads
/// naturally instead of nesting.
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AlarmConfig {
    /// See [`Thresholds::urgent_low_mgdl`].
    pub urgent_low_mgdl: u16,
    /// See [`Thresholds::low_mgdl`].
    pub low_mgdl: u16,
    /// See [`Thresholds::high_mgdl`].
    pub high_mgdl: u16,
    /// See [`Thresholds::urgent_high_mgdl`].
    pub urgent_high_mgdl: u16,
    /// See [`AlarmPolicy::hysteresis_mgdl`].
    pub hysteresis_mgdl: u16,
    /// Minutes without a fresh reading before the display alarms.
    pub stale_after_mins: u32,
    /// Minutes the silence button quiets an alarm for.
    pub silence_for_mins: u32,
    /// Minutes between re-announcements of an unsilenced alarm.
    pub reannounce_after_mins: u32,
}

impl Default for AlarmConfig {
    fn default() -> Self {
        let policy = AlarmPolicy::DEFAULT;
        Self {
            urgent_low_mgdl: policy.thresholds.urgent_low_mgdl,
            low_mgdl: policy.thresholds.low_mgdl,
            high_mgdl: policy.thresholds.high_mgdl,
            urgent_high_mgdl: policy.thresholds.urgent_high_mgdl,
            hysteresis_mgdl: policy.hysteresis_mgdl,
            stale_after_mins: policy.stale_after.as_mins(),
            silence_for_mins: policy.silence_for.as_mins(),
            reannounce_after_mins: policy.reannounce_after.as_mins(),
        }
    }
}

impl AlarmConfig {
    /// The policy to push to the display node.
    pub fn to_policy(self) -> AlarmPolicy {
        AlarmPolicy {
            thresholds: Thresholds {
                urgent_low_mgdl: self.urgent_low_mgdl,
                low_mgdl: self.low_mgdl,
                high_mgdl: self.high_mgdl,
                urgent_high_mgdl: self.urgent_high_mgdl,
            },
            hysteresis_mgdl: self.hysteresis_mgdl,
            stale_after: Duration::from_mins(self.stale_after_mins),
            silence_for: Duration::from_mins(self.silence_for_mins),
            reannounce_after: Duration::from_mins(self.reannounce_after_mins),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default()
            .validate()
            .expect("defaults should validate");
    }

    #[test]
    fn the_default_alarm_config_matches_the_default_policy() {
        assert_eq!(AlarmConfig::default().to_policy(), AlarmPolicy::DEFAULT);
    }

    #[test]
    fn an_empty_config_file_yields_the_defaults() {
        let config: Config = toml::from_str("").expect("parse");
        assert_eq!(config.dexcom.region, Region::Us);
        assert_eq!(config.poll.sensor_interval_secs, 300);
    }

    #[test]
    fn a_partial_config_file_fills_in_the_rest() {
        let config: Config = toml::from_str(
            r#"
            [dexcom]
            account = "someone@example.com"
            region = "ous"

            [alarms]
            low_mgdl = 75
            "#,
        )
        .expect("parse");

        assert_eq!(
            config.dexcom.account.as_deref(),
            Some("someone@example.com")
        );
        assert_eq!(config.dexcom.region, Region::Ous);
        assert_eq!(config.alarms.low_mgdl, 75);
        // Untouched fields keep their defaults.
        assert_eq!(config.alarms.high_mgdl, 180);
        assert_eq!(config.link.listen.port(), 9101);
        config.validate().expect("should validate");
    }

    #[test]
    fn a_typo_in_a_key_is_an_error_not_a_silent_default() {
        let error = toml::from_str::<Config>(
            r#"
            [alarms]
            lo_mgdl = 75
            "#,
        )
        .expect_err("should reject the unknown key");
        assert!(error.to_string().contains("lo_mgdl"), "{error}");
    }

    #[test]
    fn nonsensical_thresholds_are_rejected() {
        let config: Config = toml::from_str(
            r#"
            [alarms]
            low_mgdl = 200
            "#,
        )
        .expect("parse");
        let error = config.validate().expect_err("should reject");
        assert!(error.to_string().contains("threshold"), "{error}");
    }

    #[test]
    fn a_link_pointing_at_itself_is_rejected() {
        let config: Config = toml::from_str(
            r#"
            [link]
            listen = "127.0.0.1:9101"
            peer = "127.0.0.1:9101"
            "#,
        )
        .expect("parse");
        assert!(config.validate().is_err());
    }

    #[test]
    fn backoff_bounds_must_make_sense() {
        let config: Config = toml::from_str(
            r#"
            [poll]
            retry_base_secs = 600
            retry_max_secs = 60
            "#,
        )
        .expect("parse");
        assert!(config.validate().is_err());
    }

    #[test]
    fn the_password_never_appears_in_the_config_schema() {
        // Serializing a default config must not produce a password field —
        // it would invite people to fill it in.
        let text = toml::to_string(&Config::default()).expect("serialize");
        assert!(!text.contains("password"), "{text}");
    }
}
