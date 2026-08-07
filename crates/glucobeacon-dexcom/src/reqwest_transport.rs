//! A [`HttpTransport`] backed by `reqwest`.
//!
//! What the workstation build uses. An ESP-IDF build should implement the trait
//! over `esp_idf_svc::http::client::EspHttpConnection` instead and turn this
//! feature off: ESP-IDF links mbedTLS whether or not we use it, and carrying a
//! second TLS stack to reach one JSON API costs flash and RAM that the display
//! side has better uses for.

use std::time::Duration;

use crate::error::{Error, Result};
use crate::http::{HttpResponse, HttpTransport, ShareRequest};

/// The default transport.
#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    /// A transport with timeouts suited to a device on domestic WiFi.
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("glucobeacon/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Transport(Box::new(e)))?;
        Ok(Self { client })
    }

    /// A transport over a caller-supplied client.
    ///
    /// Useful for pointing tests at a local server, and for reusing a
    /// connection pool where TLS handshakes are expensive.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl HttpTransport for ReqwestTransport {
    type Error = reqwest::Error;

    async fn post(&self, request: ShareRequest) -> std::result::Result<HttpResponse, Self::Error> {
        let mut builder = self
            .client
            .post(&request.url)
            .header("Accept", "application/json");
        if let Some(body) = request.body {
            builder = builder
                .header("Content-Type", "application/json")
                .body(body);
        }

        let response = builder.send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        Ok(HttpResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transport_builds_with_defaults() {
        assert!(ReqwestTransport::new().is_ok());
    }

    #[tokio::test]
    async fn an_unreachable_host_is_a_transport_error_not_a_panic() {
        let transport = ReqwestTransport::with_client(
            reqwest::Client::builder()
                .timeout(Duration::from_millis(150))
                // No proxy: this must fail at connect, not be answered by one.
                .no_proxy()
                .build()
                .expect("build"),
        );
        let request = ShareRequest::empty("http://127.0.0.1:1/nothing");
        assert!(transport.post(request).await.is_err());
    }
}
