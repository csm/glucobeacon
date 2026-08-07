//! The Share protocol, with no HTTP client in it.
//!
//! Building requests and reading responses is pure: URL in, string out, string
//! in, `Result` out. That keeps the awkward parts of Share — the two-step
//! login, faults reported as HTTP 500, a session ID that expires without
//! warning — testable without a network, and lets the same logic run over
//! whatever HTTP client the target has.

use glucobeacon_core::Reading;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::http::{HttpResponse, ShareRequest};
use crate::model::{ShareFault, ShareGlucoseEntry};
use crate::region::Region;

/// The application ID the Dexcom mobile app presents. Share requires it.
pub const APPLICATION_ID: &str = "d89443d2-327c-4a6f-89e5-496bbb0317db";

/// The ID Share returns to mean "that failed" instead of erroring.
pub const NULL_ID: &str = "00000000-0000-0000-0000-000000000000";

const AUTHENTICATE_PATH: &str = "/ShareWebServices/Services/General/AuthenticatePublisherAccount";
const LOGIN_PATH: &str = "/ShareWebServices/Services/General/LoginPublisherAccountById";
const GLUCOSE_PATH: &str = "/ShareWebServices/Services/Publisher/ReadPublisherLatestGlucoseValues";

/// The widest window Share will answer for.
pub const MAX_LOOKBACK_MINUTES: u32 = 1440;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticateBody<'a> {
    account_name: &'a str,
    password: &'a str,
    application_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginBody<'a> {
    account_id: &'a str,
    password: &'a str,
    application_id: &'a str,
}

/// Step one of the login: account name and password to an account ID.
pub fn authenticate_request(region: Region, account_name: &str, password: &str) -> ShareRequest {
    let body = serde_json::to_string(&AuthenticateBody {
        account_name,
        password,
        application_id: APPLICATION_ID,
    })
    // Serializing three borrowed strings cannot fail.
    .unwrap_or_default();

    ShareRequest::with_body(format!("{}{AUTHENTICATE_PATH}", region.base_url()), body)
}

/// Step two of the login: account ID and password to a session ID.
pub fn login_request(region: Region, account_id: &str, password: &str) -> ShareRequest {
    let body = serde_json::to_string(&LoginBody {
        account_id,
        password,
        application_id: APPLICATION_ID,
    })
    .unwrap_or_default();

    ShareRequest::with_body(format!("{}{LOGIN_PATH}", region.base_url()), body)
}

/// A request for the latest readings.
///
/// `minutes` is clamped to what Share accepts and `max_count` to at least one,
/// so a misconfigured caller gets a narrow query rather than a rejected one.
pub fn glucose_request(
    region: Region,
    session_id: &str,
    minutes: u32,
    max_count: u16,
) -> Result<ShareRequest> {
    // The session ID goes into a query string. It comes from Share rather than
    // from us, so it is checked rather than trusted: a UUID needs no escaping,
    // and anything that is not one has no business being pasted into a URL.
    if !is_uuid(session_id) {
        return Err(Error::Protocol(format!(
            "Share issued a session ID that is not a UUID: {session_id:?}"
        )));
    }

    let minutes = minutes.clamp(1, MAX_LOOKBACK_MINUTES);
    let max_count = max_count.max(1);

    Ok(ShareRequest::empty(format!(
        "{}{GLUCOSE_PATH}?sessionId={session_id}&minutes={minutes}&maxCount={max_count}",
        region.base_url()
    )))
}

/// Whether `value` is shaped like a hyphenated UUID.
fn is_uuid(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for expected in groups {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.len() != expected || !part.bytes().all(|b| b.is_ascii_hexdigit()) {
            return false;
        }
    }
    parts.next().is_none()
}

/// Reads an account or session ID out of a response.
///
/// Share returns these as a bare JSON string.
pub fn parse_id(response: &HttpResponse, what: &str) -> Result<String> {
    check_fault(response)?;

    let id = serde_json::from_str::<String>(response.body.trim()).map_err(|_| {
        Error::Protocol(format!(
            "expected a {what} string, got {:?}",
            truncate(&response.body, 100)
        ))
    })?;

    if id == NULL_ID {
        return Err(Error::Credentials {
            code: "NullId".to_owned(),
            message: format!("Share returned an empty {what}; the credentials did not match"),
        });
    }
    Ok(id)
}

/// Reads glucose entries out of a response, oldest first.
///
/// Share returns newest first; this reverses them so callers can feed them
/// straight into a [`History`](glucobeacon_core::History), which wants
/// increasing timestamps.
pub fn parse_glucose(response: &HttpResponse) -> Result<Vec<Reading>> {
    check_fault(response)?;

    let entries: Vec<ShareGlucoseEntry> = serde_json::from_str(response.body.trim())
        .map_err(|e| Error::Protocol(format!("could not parse the glucose response: {e}")))?;

    let total = entries.len();
    let mut readings: Vec<Reading> = entries
        .iter()
        .filter_map(ShareGlucoseEntry::to_reading)
        .collect();

    // A reading we cannot date is worse than no reading, because staleness
    // detection depends on the timestamp. Dropping a few is fine; dropping all
    // of them means the format changed and we should say so.
    if readings.is_empty() && total > 0 {
        return Err(Error::Protocol(format!(
            "Share returned {total} entries, none with a usable timestamp"
        )));
    }

    readings.sort_unstable_by_key(|reading| reading.at);
    Ok(readings)
}

/// Turns a non-2xx response into the right [`Error`].
///
/// Share reports application errors with a 500 and a JSON body, so the status
/// alone does not distinguish "wrong password" from "the service is down".
pub fn check_fault(response: &HttpResponse) -> Result<()> {
    if response.is_success() {
        return Ok(());
    }
    match serde_json::from_str::<ShareFault>(response.body.trim()) {
        Ok(fault) => Err(Error::from_fault(fault)),
        Err(_) => Err(Error::Service {
            code: response.status.to_string(),
            message: truncate(&response.body, 200),
        }),
    }
}

fn truncate(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    match trimmed.char_indices().nth(limit) {
        Some((cut, _)) => format!("{}…", &trimmed[..cut]),
        None => trimmed.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    const SESSION: &str = "1e2d3c4b-5a69-4780-8b9c-0d1e2f3a4b5c";

    #[test]
    fn the_authenticate_body_uses_the_field_names_share_expects() {
        let request = authenticate_request(Region::Us, "someone", "secret");
        assert!(request.url.ends_with(AUTHENTICATE_PATH));
        assert!(request.url.starts_with("https://share2.dexcom.com"));

        let body: serde_json::Value =
            serde_json::from_str(&request.body.expect("body")).expect("parse");
        assert_eq!(body["accountName"], "someone");
        assert_eq!(body["password"], "secret");
        assert_eq!(body["applicationId"], APPLICATION_ID);
    }

    #[test]
    fn the_login_body_uses_the_field_names_share_expects() {
        let request = login_request(Region::Ous, "an-account-id", "secret");
        assert!(request.url.starts_with("https://shareous1.dexcom.com"));

        let body: serde_json::Value =
            serde_json::from_str(&request.body.expect("body")).expect("parse");
        assert_eq!(body["accountId"], "an-account-id");
        assert_eq!(body["applicationId"], APPLICATION_ID);
    }

    #[test]
    fn the_glucose_request_carries_the_query_share_expects() {
        let request = glucose_request(Region::Us, SESSION, 10, 2).expect("build");
        assert!(request.url.contains(&format!("sessionId={SESSION}")));
        assert!(request.url.contains("minutes=10"));
        assert!(request.url.contains("maxCount=2"));
        assert_eq!(request.body, None);
    }

    #[test]
    fn the_glucose_window_is_clamped_to_what_share_accepts() {
        let wide = glucose_request(Region::Us, SESSION, 100_000, 1).expect("build");
        assert!(
            wide.url
                .contains(&format!("minutes={MAX_LOOKBACK_MINUTES}"))
        );

        let narrow = glucose_request(Region::Us, SESSION, 0, 0).expect("build");
        assert!(narrow.url.contains("minutes=1"));
        assert!(narrow.url.contains("maxCount=1"));
    }

    #[test]
    fn a_session_id_that_is_not_a_uuid_never_reaches_a_url() {
        // Share is the source of this value, but it lands in a query string,
        // so it gets checked rather than trusted.
        for bad in [
            "",
            "not-a-uuid",
            "1e2d3c4b-5a69-4780-8b9c-0d1e2f3a4b5c&maxCount=9999",
            "../../etc/passwd",
            "1e2d3c4b5a6947808b9c0d1e2f3a4b5c",
        ] {
            let error = glucose_request(Region::Us, bad, 10, 1).expect_err("should reject");
            assert_eq!(error.kind(), ErrorKind::Protocol, "{bad:?}");
        }
    }

    #[test]
    fn uuid_shape_is_checked_properly() {
        assert!(is_uuid(SESSION));
        assert!(is_uuid(NULL_ID));
        assert!(!is_uuid(&SESSION[..35]));
        assert!(!is_uuid(&format!("{SESSION}-extra")));
        assert!(
            !is_uuid("1e2d3c4g-5a69-4780-8b9c-0d1e2f3a4b5c"),
            "g is not hex"
        );
    }

    #[test]
    fn ids_are_read_from_a_bare_json_string() {
        let response = HttpResponse::new(200, format!("\"{SESSION}\""));
        assert_eq!(parse_id(&response, "session ID").expect("parse"), SESSION);
    }

    #[test]
    fn a_null_id_is_a_credential_failure_not_a_success() {
        // Share answers 200 with this rather than erroring, which is the single
        // easiest way to end up with a client that silently never works.
        let response = HttpResponse::new(200, format!("\"{NULL_ID}\""));
        let error = parse_id(&response, "account ID").expect_err("should reject");
        assert_eq!(error.kind(), ErrorKind::Credentials);
        assert!(!error.is_retryable());
    }

    #[test]
    fn a_non_string_id_body_is_a_protocol_error() {
        let response = HttpResponse::new(200, "<html>maintenance</html>");
        let error = parse_id(&response, "session ID").expect_err("should reject");
        assert_eq!(error.kind(), ErrorKind::Protocol);
    }

    #[test]
    fn faults_are_classified_from_the_body_not_the_status() {
        // Share sends 500 for a wrong password. Reading the status alone would
        // make that indistinguishable from an outage, and retrying forever.
        let response = HttpResponse::new(
            500,
            r#"{"Code":"SSO_AuthenticatePasswordInvalid","Message":"nope"}"#,
        );
        let error = parse_id(&response, "account ID").expect_err("should reject");
        assert_eq!(error.kind(), ErrorKind::Credentials);
        assert!(!error.is_retryable());
    }

    #[test]
    fn an_expired_session_is_reported_as_such() {
        let response = HttpResponse::new(500, r#"{"Code":"SessionIdNotFound","Message":""}"#);
        let error = parse_glucose(&response).expect_err("should reject");
        assert!(matches!(error, Error::SessionExpired { .. }));
        assert!(error.is_retryable());
    }

    #[test]
    fn a_fault_body_that_is_not_json_still_produces_a_service_error() {
        let response = HttpResponse::new(502, "<html>Bad Gateway</html>");
        let error = parse_glucose(&response).expect_err("should reject");
        assert_eq!(error.kind(), ErrorKind::Service);
        assert!(error.to_string().contains("502"), "{error}");
    }

    #[test]
    fn glucose_entries_come_back_oldest_first() {
        let response = HttpResponse::new(
            200,
            r#"[
                {"WT":"/Date(1700000400000)/","Value":142,"Trend":"Flat"},
                {"WT":"/Date(1700000100000)/","Value":138,"Trend":"FortyFiveUp"}
            ]"#,
        );
        let readings = parse_glucose(&response).expect("parse");
        assert_eq!(readings.len(), 2);
        assert!(readings[0].at < readings[1].at, "Share sends newest first");
        assert_eq!(readings[1].glucose.mgdl(), 142);
    }

    #[test]
    fn an_empty_window_is_not_an_error() {
        let readings = parse_glucose(&HttpResponse::new(200, "[]")).expect("parse");
        assert!(readings.is_empty());
    }

    #[test]
    fn a_single_undateable_entry_is_dropped_but_the_rest_survive() {
        let response = HttpResponse::new(
            200,
            r#"[
                {"WT":"/Date(1700000400000)/","Value":142,"Trend":"Flat"},
                {"WT":"garbage","Value":138,"Trend":"Flat"}
            ]"#,
        );
        assert_eq!(parse_glucose(&response).expect("parse").len(), 1);
    }

    #[test]
    fn entries_that_are_all_undateable_mean_the_format_changed() {
        let response = HttpResponse::new(200, r#"[{"WT":"garbage","Value":138,"Trend":"Flat"}]"#);
        let error = parse_glucose(&response).expect_err("should reject");
        assert_eq!(error.kind(), ErrorKind::Protocol);
    }
}
