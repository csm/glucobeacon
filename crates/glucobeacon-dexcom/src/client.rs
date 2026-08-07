//! Session management on top of [`crate::protocol`].

use std::fmt;
use std::time::Duration;

use glucobeacon_core::Reading;
use tracing::debug;

use crate::error::{Error, Result};
use crate::http::HttpTransport;
use crate::protocol;
use crate::region::Region;

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

/// A client for one Share account.
///
/// Holds a session ID and renews it transparently: Share sessions expire after
/// a few hours, and every caller having to handle that would be a mistake
/// waiting to happen.
#[derive(Debug)]
pub struct ShareClient<T> {
    transport: T,
    region: Region,
    credentials: Credentials,
    session_id: Option<String>,
}

impl<T: HttpTransport> ShareClient<T> {
    /// Builds a client over `transport`.
    pub fn with_transport(transport: T, credentials: Credentials, region: Region) -> Self {
        Self {
            transport,
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
    /// Logs in if there is no session, and retries once if the session turns
    /// out to have expired.
    pub async fn readings(&mut self, lookback: Duration, max_count: u16) -> Result<Vec<Reading>> {
        // Share measures the window in whole minutes and rejects zero.
        let minutes =
            (lookback.as_secs() / 60).clamp(1, u64::from(protocol::MAX_LOOKBACK_MINUTES)) as u32;

        let session = self.session().await?;
        match self.fetch(&session, minutes, max_count).await {
            Err(Error::SessionExpired { code }) => {
                debug!(%code, "Share session expired; logging in again");
                self.session_id = None;
                let session = self.session().await?;
                self.fetch(&session, minutes, max_count).await
            }
            other => other,
        }
    }

    async fn fetch(&self, session_id: &str, minutes: u32, max_count: u16) -> Result<Vec<Reading>> {
        let request = protocol::glucose_request(self.region, session_id, minutes, max_count)?;
        let response = self.send(request).await?;
        protocol::parse_glucose(&response)
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
        let request = protocol::authenticate_request(
            self.region,
            &self.credentials.account_name,
            &self.credentials.password,
        );
        let response = self.send(request).await?;
        let account_id = protocol::parse_id(&response, "account ID")?;

        let request = protocol::login_request(self.region, &account_id, &self.credentials.password);
        let response = self.send(request).await?;
        let session_id = protocol::parse_id(&response, "session ID")?;

        debug!(region = %self.region, "established a Dexcom Share session");
        Ok(session_id)
    }

    async fn send(&self, request: crate::http::ShareRequest) -> Result<crate::http::HttpResponse> {
        self.transport
            .post(request)
            .await
            .map_err(|e| Error::Transport(Box::new(e)))
    }
}

#[cfg(feature = "reqwest-transport")]
impl ShareClient<crate::reqwest_transport::ReqwestTransport> {
    /// Builds a client with a sensible default HTTP configuration.
    pub fn new(credentials: Credentials, region: Region) -> Result<Self> {
        let transport = crate::reqwest_transport::ReqwestTransport::new()?;
        Ok(Self::with_transport(transport, credentials, region))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::http::{HttpResponse, ShareRequest, TransportError};
    use std::sync::Mutex;

    /// The crate's `Result` alias takes one parameter; the transport's takes two.
    type TransportResult = std::result::Result<HttpResponse, TransportError>;

    const ACCOUNT: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    const SESSION: &str = "11111111-2222-4333-8444-555555555555";
    const SESSION_TWO: &str = "99999999-8888-4777-8666-555555555555";

    /// A transport that replays a script and records what it was asked for.
    ///
    /// This is what makes the login-and-retry flow testable: it is the part
    /// most likely to be wrong and the part hardest to exercise against the
    /// real service.
    #[derive(Debug)]
    struct ScriptedTransport {
        responses: Mutex<std::collections::VecDeque<TransportResult>>,
        seen: Mutex<Vec<ShareRequest>>,
    }

    impl ScriptedTransport {
        fn new(responses: Vec<TransportResult>) -> Self {
            Self {
                responses: Mutex::new(responses.into()),
                seen: Mutex::new(Vec::new()),
            }
        }

        fn ok(bodies: Vec<(u16, &str)>) -> Self {
            Self::new(
                bodies
                    .into_iter()
                    .map(|(status, body)| Ok(HttpResponse::new(status, body)))
                    .collect(),
            )
        }

        fn requests(&self) -> Vec<ShareRequest> {
            self.seen.lock().expect("lock").clone()
        }

        fn remaining(&self) -> usize {
            self.responses.lock().expect("lock").len()
        }
    }

    impl HttpTransport for ScriptedTransport {
        type Error = TransportError;

        async fn post(&self, request: ShareRequest) -> TransportResult {
            self.seen.lock().expect("lock").push(request);
            self.responses
                .lock()
                .expect("lock")
                .pop_front()
                .unwrap_or_else(|| Err(TransportError("script exhausted".to_owned())))
        }
    }

    const GLUCOSE_BODY: &str = r#"[{"WT":"/Date(1700000400000)/","Value":142,"Trend":"Flat"}]"#;

    fn client(transport: ScriptedTransport) -> ShareClient<ScriptedTransport> {
        ShareClient::with_transport(
            transport,
            Credentials::new("someone@example.com", "hunter2"),
            Region::Us,
        )
    }

    #[tokio::test]
    async fn a_first_fetch_logs_in_then_reads() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (200, GLUCOSE_BODY),
        ]));

        let readings = client.readings(DEFAULT_LOOKBACK, 2).await.expect("fetch");
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].glucose.mgdl(), 142);
        assert!(client.has_session());

        let requests = client.transport.requests();
        assert_eq!(requests.len(), 3, "authenticate, login, then read");
        assert!(requests[0].url.contains("AuthenticatePublisherAccount"));
        assert!(requests[1].url.contains("LoginPublisherAccountById"));
        assert!(requests[2].url.contains("ReadPublisherLatestGlucoseValues"));
        assert!(requests[2].url.contains(&format!("sessionId={SESSION}")));
    }

    #[tokio::test]
    async fn a_second_fetch_reuses_the_session() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (200, GLUCOSE_BODY),
            (200, GLUCOSE_BODY),
        ]));

        client.readings(DEFAULT_LOOKBACK, 2).await.expect("first");
        client.readings(DEFAULT_LOOKBACK, 2).await.expect("second");

        assert_eq!(
            client.transport.requests().len(),
            4,
            "the second fetch must not log in again"
        );
    }

    #[tokio::test]
    async fn an_expired_session_is_renewed_and_the_read_retried() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (200, GLUCOSE_BODY),
            // Hours later, Share forgets the session.
            (500, r#"{"Code":"SessionIdNotFound","Message":""}"#),
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION_TWO}\"")),
            (200, GLUCOSE_BODY),
        ]));

        client.readings(DEFAULT_LOOKBACK, 2).await.expect("first");
        let readings = client.readings(DEFAULT_LOOKBACK, 2).await.expect("second");
        assert_eq!(readings.len(), 1, "the caller never sees the expiry");

        let requests = client.transport.requests();
        assert_eq!(requests.len(), 7);
        assert!(
            requests[6]
                .url
                .contains(&format!("sessionId={SESSION_TWO}")),
            "the retry must use the new session"
        );
    }

    #[tokio::test]
    async fn the_session_is_renewed_at_most_once_per_call() {
        // Otherwise a server that always says "expired" becomes an infinite
        // login loop against Dexcom.
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (500, r#"{"Code":"SessionIdNotFound"}"#),
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION_TWO}\"")),
            (500, r#"{"Code":"SessionIdNotFound"}"#),
        ]));

        let error = client
            .readings(DEFAULT_LOOKBACK, 2)
            .await
            .expect_err("should give up");
        assert!(matches!(error, Error::SessionExpired { .. }));
        assert_eq!(client.transport.remaining(), 0);
        assert_eq!(client.transport.requests().len(), 6);
    }

    #[tokio::test]
    async fn a_wrong_password_fails_without_retrying() {
        let mut client = client(ScriptedTransport::ok(vec![(
            500,
            r#"{"Code":"SSO_AuthenticatePasswordInvalid","Message":"nope"}"#,
        )]));

        let error = client
            .readings(DEFAULT_LOOKBACK, 2)
            .await
            .expect_err("should fail");
        assert_eq!(error.kind(), ErrorKind::Credentials);
        assert!(!error.is_retryable());
        assert_eq!(client.transport.requests().len(), 1, "no second attempt");
        assert!(!client.has_session());
    }

    #[tokio::test]
    async fn a_transport_failure_surfaces_as_a_network_error() {
        let mut client = client(ScriptedTransport::new(vec![Err(TransportError(
            "dns failure".to_owned(),
        ))]));

        let error = client
            .readings(DEFAULT_LOOKBACK, 2)
            .await
            .expect_err("should fail");
        assert_eq!(error.kind(), ErrorKind::Network);
        assert!(error.is_retryable());
        // The underlying cause is preserved rather than flattened to a string.
        let source = std::error::Error::source(&error).expect("source");
        assert!(source.to_string().contains("dns failure"), "{source}");
    }

    #[tokio::test]
    async fn latest_reading_returns_the_newest_of_several() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (
                200,
                r#"[
                    {"WT":"/Date(1700000400000)/","Value":142,"Trend":"Flat"},
                    {"WT":"/Date(1700000100000)/","Value":138,"Trend":"FortyFiveUp"}
                ]"#,
            ),
        ]));

        let reading = client.latest_reading().await.expect("fetch").expect("some");
        assert_eq!(reading.glucose.mgdl(), 142);
    }

    #[tokio::test]
    async fn an_empty_window_is_not_an_error() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (200, "[]"),
        ]));
        assert_eq!(client.latest_reading().await.expect("fetch"), None);
    }

    #[tokio::test]
    async fn forgetting_the_session_forces_a_fresh_login() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (200, GLUCOSE_BODY),
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION_TWO}\"")),
            (200, GLUCOSE_BODY),
        ]));

        client.readings(DEFAULT_LOOKBACK, 2).await.expect("first");
        assert!(client.has_session());
        client.forget_session();
        assert!(!client.has_session());
        client.readings(DEFAULT_LOOKBACK, 2).await.expect("second");
        assert_eq!(client.transport.requests().len(), 6);
    }

    #[tokio::test]
    async fn the_lookback_window_reaches_the_query() {
        let mut client = client(ScriptedTransport::ok(vec![
            (200, &format!("\"{ACCOUNT}\"")),
            (200, &format!("\"{SESSION}\"")),
            (200, GLUCOSE_BODY),
        ]));
        client
            .readings(Duration::from_secs(30 * 60), 6)
            .await
            .expect("fetch");

        let url = &client.transport.requests()[2].url;
        assert!(url.contains("minutes=30"), "{url}");
        assert!(url.contains("maxCount=6"), "{url}");
    }

    #[test]
    fn credentials_do_not_leak_the_password_into_logs() {
        let credentials = Credentials::new("someone@example.com", "hunter2");
        let rendered = format!("{credentials:?}");
        assert!(!rendered.contains("hunter2"), "{rendered}");
        assert!(rendered.contains("someone@example.com"), "{rendered}");
    }

    #[test]
    fn a_client_starts_without_a_session() {
        let client = client(ScriptedTransport::ok(vec![]));
        assert!(!client.has_session());
        assert_eq!(client.region(), Region::Us);
    }
}
