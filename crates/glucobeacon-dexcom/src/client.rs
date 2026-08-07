//! The Share client itself.

use std::fmt;
use std::time::Duration;

use glucobeacon_core::Reading;
use serde::Serialize;
use tracing::{debug, warn};

use crate::error::{Error, Result};
use crate::model::{ShareFault, ShareGlucoseEntry};
use crate::region::Region;

/// The application ID the Dexcom mobile app presents. Share requires it.
const APPLICATION_ID: &str = "d89443d2-327c-4a6f-89e5-496bbb0317db";

/// Session ID Share returns to mean "login failed" instead of erroring.
const NULL_SESSION: &str = "00000000-0000-0000-0000-000000000000";

const AUTHENTICATE_PATH: &str = "/ShareWebServices/Services/General/AuthenticatePublisherAccount";
const LOGIN_PATH: &str = "/ShareWebServices/Services/General/LoginPublisherAccountById";
const GLUCOSE_PATH: &str = "/ShareWebServices/Services/Publisher/ReadPublisherLatestGlucoseValues";

/// How far back to ask for readings by default.
///
/// Ten minutes covers two sensor intervals, so a single missed poll still comes
/// back with everything the display has not seen.
pub const DEFAULT_LOOKBACK: Duration = Duration::from_secs(10 * 60);

/// How many readings to ask for by default.
pub const DEFAULT_MAX_COUNT: u16 = 2;

/// A Dexcom Share follower account's login.
///
/// This is the *follower* account, not the account on the phone doing the
/// uploading. `Debug` deliberately omits the password so it cannot end up in a
/// log line.
#[derive(Clone)]
pub struct Credentials {
    /// The account name (username, or the email address the account uses).
    pub account_name: String,
    /// The account password.
    pub password: String,
}

impl Credentials {
    /// Assembles credentials.
    pub fn new(account_name: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            account_name: account_name.into(),
            password: password.into(),
        }
    }
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("account_name", &self.account_name)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthenticateRequest<'a> {
    account_name: &'a str,
    password: &'a str,
    application_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LoginRequest<'a> {
    account_id: &'a str,
    password: &'a str,
    application_id: &'a str,
}

/// A client for one Share account.
///
/// Holds a session ID and renews it transparently: Share sessions expire after
/// a few hours, and every caller having to handle that would be a mistake
/// waiting to happen.
#[derive(Debug)]
pub struct ShareClient {
    http: reqwest::Client,
    region: Region,
    credentials: Credentials,
    session_id: Option<String>,
}

impl ShareClient {
    /// Builds a client with a sensible HTTP configuration.
    pub fn new(credentials: Credentials, region: Region) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("glucobeacon/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self::with_http_client(http, credentials, region))
    }

    /// Builds a client over a caller-supplied HTTP client.
    ///
    /// Useful for pointing tests at a local server, and for reusing a connection
    /// pool on a device where TLS handshakes are expensive.
    pub fn with_http_client(
        http: reqwest::Client,
        credentials: Credentials,
        region: Region,
    ) -> Self {
        Self {
            http,
            region,
            credentials,
            session_id: None,
        }
    }

    /// The region this client talks to.
    pub fn region(&self) -> Region {
        self.region
    }

    /// Whether a session has been established.
    pub fn has_session(&self) -> bool {
        self.session_id.is_some()
    }

    /// Forgets the current session, forcing a fresh login on the next call.
    pub fn forget_session(&mut self) {
        self.session_id = None;
    }

    /// The most recent reading, or `None` if Share has nothing in the window.
    pub async fn latest_reading(&mut self) -> Result<Option<Reading>> {
        Ok(self
            .readings(DEFAULT_LOOKBACK, 1)
            .await?
            .into_iter()
            .next_back())
    }

    /// Readings from the last `lookback`, oldest first, at most `max_count`.
    ///
    /// Share returns newest first; this reverses them so callers can feed them
    /// straight into a [`History`](glucobeacon_core::History), which wants
    /// increasing timestamps. Entries with unparseable timestamps are dropped
    /// with a warning rather than failing the whole poll.
    pub async fn readings(&mut self, lookback: Duration, max_count: u16) -> Result<Vec<Reading>> {
        let entries = self.fetch_entries(lookback, max_count).await?;
        let total = entries.len();

        let mut readings: Vec<Reading> = entries
            .iter()
            .filter_map(|entry| {
                let reading = entry.to_reading();
                if reading.is_none() {
                    warn!(wall_time = %entry.wall_time, "dropping Share entry with an unparseable timestamp");
                }
                reading
            })
            .collect();

        if readings.is_empty() && total > 0 {
            return Err(Error::Protocol(format!(
                "Share returned {total} entries, none of which had a usable timestamp"
            )));
        }

        readings.sort_unstable_by_key(|reading| reading.at);
        Ok(readings)
    }

    /// Fetches raw entries, logging in first and retrying once if the session
    /// turns out to be stale.
    async fn fetch_entries(
        &mut self,
        lookback: Duration,
        max_count: u16,
    ) -> Result<Vec<ShareGlucoseEntry>> {
        // Share measures the window in whole minutes and rejects zero.
        let minutes = (lookback.as_secs() / 60).clamp(1, 1440) as u32;
        let max_count = max_count.max(1);

        let session = self.session().await?;
        match self.get_entries(&session, minutes, max_count).await {
            Err(Error::SessionExpired { code }) => {
                debug!(%code, "Share session expired; logging in again");
                self.session_id = None;
                let session = self.session().await?;
                self.get_entries(&session, minutes, max_count).await
            }
            other => other,
        }
    }

    async fn get_entries(
        &self,
        session_id: &str,
        minutes: u32,
        max_count: u16,
    ) -> Result<Vec<ShareGlucoseEntry>> {
        let url = format!("{}{GLUCOSE_PATH}", self.region.base_url());
        let response = self
            .http
            .post(&url)
            .query(&[
                ("sessionId", session_id),
                ("minutes", &minutes.to_string()),
                ("maxCount", &max_count.to_string()),
            ])
            .header("Accept", "application/json")
            .send()
            .await?;

        let response = check_fault(response).await?;
        response
            .json::<Vec<ShareGlucoseEntry>>()
            .await
            .map_err(|e| Error::Protocol(format!("could not parse the glucose response: {e}")))
    }

    /// The current session ID, logging in if there is not one.
    async fn session(&mut self) -> Result<String> {
        if let Some(session) = &self.session_id {
            return Ok(session.clone());
        }
        let session = self.login().await?;
        self.session_id = Some(session.clone());
        Ok(session)
    }

    /// The two-step Share login: name and password to an account ID, then
    /// account ID and password to a session ID.
    async fn login(&self) -> Result<String> {
        let account_id = self.authenticate().await?;

        let url = format!("{}{LOGIN_PATH}", self.region.base_url());
        let response = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .json(&LoginRequest {
                account_id: &account_id,
                password: &self.credentials.password,
                application_id: APPLICATION_ID,
            })
            .send()
            .await?;

        let session_id = read_quoted_id(check_fault(response).await?, "session ID").await?;
        if session_id == NULL_SESSION {
            return Err(Error::Credentials {
                code: "NullSessionId".to_owned(),
                message: "Share accepted the account but issued no session".to_owned(),
            });
        }
        debug!(region = %self.region, "established a Dexcom Share session");
        Ok(session_id)
    }

    async fn authenticate(&self) -> Result<String> {
        let url = format!("{}{AUTHENTICATE_PATH}", self.region.base_url());
        let response = self
            .http
            .post(&url)
            .header("Accept", "application/json")
            .json(&AuthenticateRequest {
                account_name: &self.credentials.account_name,
                password: &self.credentials.password,
                application_id: APPLICATION_ID,
            })
            .send()
            .await?;

        let account_id = read_quoted_id(check_fault(response).await?, "account ID").await?;
        if account_id == NULL_SESSION {
            return Err(Error::Credentials {
                code: "NullAccountId".to_owned(),
                message: "no Share account matched those credentials".to_owned(),
            });
        }
        Ok(account_id)
    }
}

/// Turns a Share fault body into an [`Error`].
///
/// Share reports application errors with a 500 and a JSON body, so status alone
/// does not distinguish "wrong password" from "service is down".
async fn check_fault(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }

    let body = response.text().await.unwrap_or_default();
    match serde_json::from_str::<ShareFault>(&body) {
        Ok(fault) => Err(Error::from_fault(fault)),
        Err(_) => Err(Error::Service {
            code: status.as_u16().to_string(),
            message: truncate(&body, 200),
        }),
    }
}

/// Reads a bare JSON string body, which is how Share returns IDs.
async fn read_quoted_id(response: reqwest::Response, what: &str) -> Result<String> {
    let body = response.text().await?;
    serde_json::from_str::<String>(body.trim()).map_err(|_| {
        Error::Protocol(format!(
            "expected a {what} string, got {:?}",
            truncate(&body, 100)
        ))
    })
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

    #[test]
    fn credentials_do_not_leak_the_password_into_logs() {
        let credentials = Credentials::new("someone@example.com", "hunter2");
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("someone@example.com"), "{rendered}");
    }

    #[test]
    fn truncate_keeps_short_text_intact() {
        assert_eq!(truncate("  short  ", 100), "short");
        assert_eq!(
            truncate(&"x".repeat(300), 200),
            format!("{}…", "x".repeat(200))
        );
    }

    #[test]
    fn truncate_does_not_split_a_character() {
        // Multi-byte characters must not be cut mid-encoding.
        let text = "é".repeat(300);
        let cut = truncate(&text, 200);
        assert_eq!(cut.chars().count(), 201);
    }

    #[test]
    fn a_client_starts_without_a_session() {
        let client = ShareClient::new(Credentials::new("a", "b"), Region::Ous).expect("build");
        assert!(!client.has_session());
        assert_eq!(client.region(), Region::Ous);
    }

    #[test]
    fn the_authenticate_body_uses_the_field_names_share_expects() {
        let body = serde_json::to_value(AuthenticateRequest {
            account_name: "someone",
            password: "secret",
            application_id: APPLICATION_ID,
        })
        .expect("serialize");
        assert_eq!(body["accountName"], "someone");
        assert_eq!(body["password"], "secret");
        assert_eq!(body["applicationId"], APPLICATION_ID);
    }

    #[test]
    fn the_login_body_uses_the_field_names_share_expects() {
        let body = serde_json::to_value(LoginRequest {
            account_id: "an-account-id",
            password: "secret",
            application_id: APPLICATION_ID,
        })
        .expect("serialize");
        assert_eq!(body["accountId"], "an-account-id");
        assert_eq!(body["applicationId"], APPLICATION_ID);
    }
}
