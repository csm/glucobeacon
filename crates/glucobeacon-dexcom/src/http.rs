//! The HTTP transport, as a trait.
//!
//! The gateway runs on an ESP32, where the natural HTTP client is the one
//! ESP-IDF already provides — it has an mbedTLS stack linked in whether or not
//! we use it, and bundling a second TLS implementation to reach one JSON API is
//! a poor trade on a device with a few hundred kilobytes of RAM. On a
//! workstation `reqwest` is the obvious choice.
//!
//! So the Share protocol is written against this trait instead of against a
//! client, and the two live behind [`crate::client::ShareClient`]'s type
//! parameter. It also means the login-and-retry logic can be tested against a
//! scripted transport rather than against Dexcom.

use core::fmt;

/// One request to Share.
///
/// Every Share endpoint is a POST, and query parameters are already baked into
/// [`ShareRequest::url`], so there is nothing else a transport needs to know.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ShareRequest {
    /// The absolute URL, query string included.
    pub url: String,
    /// A JSON body, or `None` for the endpoints that take only query
    /// parameters.
    pub body: Option<String>,
}

impl ShareRequest {
    /// A request with a JSON body.
    pub fn with_body(url: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            body: Some(body.into()),
        }
    }

    /// A request with no body.
    pub fn empty(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            body: None,
        }
    }
}

/// What a transport got back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The response body, as text.
    ///
    /// Share responses are small — a couple of readings — so buffering is fine
    /// and streaming would only add complexity.
    pub body: String,
}

impl HttpResponse {
    /// A response with the given status and body.
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// Whether the status is 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Something that can POST to Share.
///
/// Implementations must send `Content-Type: application/json` when
/// [`ShareRequest::body`] is present, and should ask for `application/json`
/// back. Share is content-type sensitive and answers a request without the
/// header with an error that looks nothing like the real problem.
pub trait HttpTransport {
    /// What can go wrong in the transport itself.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Performs one request.
    ///
    /// Non-2xx statuses are *not* errors: Share reports application faults with
    /// a 500 and a JSON body, and telling those apart from a genuinely broken
    /// service is the caller's job.
    fn post(
        &self,
        request: ShareRequest,
    ) -> impl Future<Output = Result<HttpResponse, Self::Error>> + Send;
}

/// A transport error that carries only a message.
///
/// For implementations whose underlying error type is awkward to surface, and
/// for tests.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TransportError(pub String);

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TransportError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_covers_exactly_the_2xx_range() {
        assert!(HttpResponse::new(200, "").is_success());
        assert!(HttpResponse::new(299, "").is_success());
        assert!(!HttpResponse::new(199, "").is_success());
        assert!(!HttpResponse::new(300, "").is_success());
        assert!(!HttpResponse::new(500, "").is_success());
    }

    #[test]
    fn requests_carry_a_body_only_when_given_one() {
        assert_eq!(ShareRequest::empty("https://example.com").body, None);
        assert_eq!(
            ShareRequest::with_body("https://example.com", "{}")
                .body
                .as_deref(),
            Some("{}")
        );
    }
}
