//! Wire types for the Share service and their conversion to domain types.

use glucobeacon_core::{Glucose, Reading, Timestamp, Trend};
use serde::Deserialize;

/// One glucose entry as Share returns it.
///
/// Field names are Share's, not ours: `WT` is "wall time" and is the sensor
/// timestamp in UTC milliseconds, `ST` is system time, and `DT` is display time
/// carrying the uploader's UTC offset. `WT` is the one to trust.
#[derive(Clone, Debug, Deserialize)]
pub struct ShareGlucoseEntry {
    /// Sensor timestamp, `"/Date(1700000000000)/"`.
    #[serde(rename = "WT")]
    pub wall_time: String,
    /// Display time, which carries the uploading phone's UTC offset.
    #[serde(rename = "DT", default)]
    pub display_time: Option<String>,
    /// Glucose in mg/dL.
    #[serde(rename = "Value")]
    pub value: u16,
    /// The trend arrow. Older deployments send a number, newer ones a name.
    #[serde(rename = "Trend", default)]
    pub trend: Option<RawTrend>,
}

impl ShareGlucoseEntry {
    /// Converts to the shared domain type.
    ///
    /// Returns `None` when the timestamp is unparseable — a reading we cannot
    /// date is worse than no reading, because staleness detection depends on it.
    pub fn to_reading(&self) -> Option<Reading> {
        let at = parse_dotnet_date(&self.wall_time)?;
        let trend = self
            .trend
            .as_ref()
            .and_then(RawTrend::to_trend)
            .unwrap_or(Trend::None);
        Some(Reading::new(Glucose::from_mgdl(self.value), trend, at))
    }
}

/// The `Trend` field, which Share types inconsistently.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum RawTrend {
    /// The historical representation: an index into the arrow list.
    Index(u8),
    /// The current representation: the variant name, e.g. `"FortyFiveDown"`.
    Name(String),
}

impl RawTrend {
    /// Maps to the domain trend, or `None` if the value is not one we know.
    pub fn to_trend(&self) -> Option<Trend> {
        match self {
            Self::Index(value) => Trend::from_share_value(*value),
            Self::Name(name) => Trend::from_share_direction(name),
        }
    }
}

/// An error body, which Share returns with an HTTP 500 rather than a 4xx.
#[derive(Clone, Debug, Deserialize)]
pub struct ShareFault {
    /// A machine-readable code, e.g. `"SessionIdNotFound"`.
    #[serde(rename = "Code", default)]
    pub code: Option<String>,
    /// A human-readable message, often empty.
    #[serde(rename = "Message", default)]
    pub message: Option<String>,
}

impl ShareFault {
    /// The code, or `"Unknown"` when Share omitted it.
    pub fn code(&self) -> &str {
        self.code.as_deref().unwrap_or("Unknown")
    }

    /// Whether this fault means the session ID we used is no longer good, which
    /// is normal after a few hours and is fixed by logging in again.
    pub fn is_stale_session(&self) -> bool {
        matches!(
            self.code(),
            "SessionIdNotFound" | "SessionNotValid" | "SessionInvalid"
        )
    }

    /// Whether this fault means the credentials themselves are wrong, which
    /// retrying will never fix.
    pub fn is_credential_failure(&self) -> bool {
        matches!(
            self.code(),
            "AccountPasswordInvalid"
                | "SSO_AuthenticateAccountNotFound"
                | "SSO_AuthenticatePasswordInvalid"
                | "SSO_AuthenticateMaxAttemptsExceeded"
                | "AccountNotFound"
                | "InvalidArgument"
        )
    }
}

/// Parses a .NET `"/Date(1700000000000)/"` timestamp.
///
/// Share also emits offsets, as in `"/Date(1700000000000-0500)/"`. The
/// milliseconds are already UTC, so the offset is display metadata and is
/// ignored.
pub fn parse_dotnet_date(raw: &str) -> Option<Timestamp> {
    let inner = raw
        .trim()
        .trim_start_matches('/')
        .strip_prefix("Date(")?
        .split(')')
        .next()?;

    // A leading '-' belongs to the epoch value; a later one starts the offset.
    let digits_end = inner
        .char_indices()
        .position(|(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .unwrap_or(inner.len());

    let millis: i64 = inner[..digits_end].parse().ok()?;
    Some(Timestamp::from_unix_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plain_dotnet_date() {
        assert_eq!(
            parse_dotnet_date("/Date(1700000000000)/"),
            Some(Timestamp::from_unix_secs(1_700_000_000))
        );
    }

    #[test]
    fn ignores_the_display_offset() {
        // The milliseconds are UTC either way, so both forms are the same instant.
        assert_eq!(
            parse_dotnet_date("/Date(1700000000000-0500)/"),
            Some(Timestamp::from_unix_secs(1_700_000_000))
        );
        assert_eq!(
            parse_dotnet_date("/Date(1700000000000+0900)/"),
            Some(Timestamp::from_unix_secs(1_700_000_000))
        );
    }

    #[test]
    fn tolerates_the_slashless_form() {
        assert_eq!(
            parse_dotnet_date("Date(1700000000000)"),
            Some(Timestamp::from_unix_secs(1_700_000_000))
        );
    }

    #[test]
    fn rejects_things_that_are_not_dates() {
        for bad in ["", "/Date()/", "null", "/Date(abc)/", "1700000000000"] {
            assert_eq!(parse_dotnet_date(bad), None, "accepted {bad:?}");
        }
    }

    #[test]
    fn deserializes_a_numeric_trend() {
        let json = r#"{"WT":"/Date(1700000000000)/","Value":137,"Trend":5}"#;
        let entry: ShareGlucoseEntry = serde_json::from_str(json).expect("parse");
        let reading = entry.to_reading().expect("reading");
        assert_eq!(reading.glucose.mgdl(), 137);
        assert_eq!(reading.trend, Trend::FortyFiveDown);
        assert_eq!(reading.at, Timestamp::from_unix_secs(1_700_000_000));
    }

    #[test]
    fn deserializes_a_named_trend() {
        let json = r#"{"WT":"/Date(1700000000000)/","ST":"/Date(1700000000000)/",
                       "DT":"/Date(1700000000000-0500)/","Value":88,"Trend":"DoubleDown"}"#;
        let entry: ShareGlucoseEntry = serde_json::from_str(json).expect("parse");
        let reading = entry.to_reading().expect("reading");
        assert_eq!(reading.trend, Trend::DoubleDown);
        assert!(reading.trend.is_rapid());
    }

    #[test]
    fn an_unknown_trend_degrades_to_no_arrow_but_keeps_the_value() {
        let json = r#"{"WT":"/Date(1700000000000)/","Value":100,"Trend":"Sideways"}"#;
        let entry: ShareGlucoseEntry = serde_json::from_str(json).expect("parse");
        let reading = entry.to_reading().expect("reading");
        assert_eq!(reading.trend, Trend::None);
        assert_eq!(reading.glucose.mgdl(), 100);
    }

    #[test]
    fn a_missing_trend_is_allowed() {
        let json = r#"{"WT":"/Date(1700000000000)/","Value":100}"#;
        let entry: ShareGlucoseEntry = serde_json::from_str(json).expect("parse");
        assert_eq!(entry.to_reading().expect("reading").trend, Trend::None);
    }

    #[test]
    fn an_undateable_entry_is_dropped() {
        let json = r#"{"WT":"garbage","Value":100,"Trend":"Flat"}"#;
        let entry: ShareGlucoseEntry = serde_json::from_str(json).expect("parse");
        assert!(entry.to_reading().is_none());
    }

    #[test]
    fn parses_a_realistic_response_body() {
        let json = r#"[
            {"WT":"/Date(1700000400000)/","ST":"/Date(1700000400000)/","DT":"/Date(1700000400000-0500)/","Value":142,"Trend":"Flat"},
            {"WT":"/Date(1700000100000)/","ST":"/Date(1700000100000)/","DT":"/Date(1700000100000-0500)/","Value":138,"Trend":"FortyFiveUp"}
        ]"#;
        let entries: Vec<ShareGlucoseEntry> = serde_json::from_str(json).expect("parse");
        let readings: Vec<Reading> = entries
            .iter()
            .filter_map(ShareGlucoseEntry::to_reading)
            .collect();
        assert_eq!(readings.len(), 2);
        // Share returns newest first.
        assert!(readings[0].at > readings[1].at);
    }

    #[test]
    fn classifies_share_faults() {
        let stale: ShareFault =
            serde_json::from_str(r#"{"Code":"SessionIdNotFound","Message":""}"#).expect("parse");
        assert!(stale.is_stale_session());
        assert!(!stale.is_credential_failure());

        let bad_password: ShareFault =
            serde_json::from_str(r#"{"Code":"SSO_AuthenticatePasswordInvalid"}"#).expect("parse");
        assert!(bad_password.is_credential_failure());
        assert!(!bad_password.is_stale_session());

        let empty: ShareFault = serde_json::from_str("{}").expect("parse");
        assert_eq!(empty.code(), "Unknown");
        assert!(!empty.is_stale_session());
        assert!(!empty.is_credential_failure());
    }
}
