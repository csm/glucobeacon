//! An [`HttpTransport`] over ESP-IDF's HTTP client.
//!
//! This is why `glucobeacon-dexcom` is written against a trait rather than
//! against `reqwest`. ESP-IDF links mbedTLS and a certificate bundle whether or
//! not we use them; carrying rustls as well to reach one JSON API would cost
//! flash and RAM for nothing.
//!
//! The client is blocking, and the trait's method is `async`. That is fine
//! here: the gateway runs its poll loop on its own thread and blocks on the
//! future, so there is no executor to stall. It would not be fine if this were
//! ever dropped into a shared async runtime.

use std::io::Read as _;
use std::sync::Mutex;

use embedded_svc::http::client::Client;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::sys::esp_crt_bundle_attach;

use glucobeacon_dexcom::http::{HttpResponse, HttpTransport, ShareRequest};

/// Largest response body to accept.
///
/// Share returns a couple of readings; anything approaching this means the
/// service is answering with something other than what we asked for, and
/// buffering it would be the wrong response on a device with this much RAM.
const MAX_BODY_BYTES: usize = 8 * 1024;

/// A transport backed by ESP-IDF.
#[derive(Debug)]
pub struct EspTransport {
    // The trait takes `&self` so a client can be shared; the underlying
    // connection is single-use at a time.
    client: Mutex<Client<EspHttpConnection>>,
}

/// What can go wrong on the device's network stack.
#[derive(Debug, thiserror::Error)]
pub enum EspHttpError {
    /// ESP-IDF reported a failure.
    #[error("esp-idf http error")]
    Esp(#[from] esp_idf_svc::sys::EspError),
    /// Reading the response body failed.
    #[error("reading the response body failed")]
    Io(#[from] std::io::Error),
    /// The response body exceeded [`MAX_BODY_BYTES`].
    #[error("response body larger than {MAX_BODY_BYTES} bytes")]
    BodyTooLarge,
    /// The response body was not valid UTF-8.
    #[error("response body was not valid UTF-8")]
    NotUtf8,
    /// The client was left poisoned by a panic on another thread.
    #[error("the http client is poisoned")]
    Poisoned,
}

impl EspTransport {
    /// Builds a transport using ESP-IDF's bundled certificate roots.
    ///
    /// `crt_bundle_attach` is what makes TLS to Dexcom work without shipping a
    /// certificate; without it every request fails the handshake.
    pub fn new() -> Result<Self, EspHttpError> {
        let connection = EspHttpConnection::new(&Configuration {
            crt_bundle_attach: Some(esp_crt_bundle_attach),
            timeout: Some(std::time::Duration::from_secs(30)),
            ..Default::default()
        })?;
        Ok(Self {
            client: Mutex::new(Client::wrap(connection)),
        })
    }

    fn request(&self, request: &ShareRequest) -> Result<HttpResponse, EspHttpError> {
        let mut client = self.client.lock().map_err(|_| EspHttpError::Poisoned)?;

        let headers: &[(&str, &str)] = match request.body {
            // Share is content-type sensitive: without this header it answers
            // with an error that looks nothing like the real problem.
            Some(_) => &[
                ("Content-Type", "application/json"),
                ("Accept", "application/json"),
            ],
            None => &[("Accept", "application/json")],
        };

        let mut outgoing = client.post(&request.url, headers)?;
        if let Some(body) = &request.body {
            use embedded_svc::io::Write as _;
            outgoing.write_all(body.as_bytes())?;
            outgoing.flush()?;
        }

        let mut incoming = outgoing.submit()?;
        let status = incoming.status();

        let mut body = Vec::new();
        let mut chunk = [0u8; 512];
        loop {
            let read = incoming.read(&mut chunk)?;
            if read == 0 {
                break;
            }
            if body.len() + read > MAX_BODY_BYTES {
                return Err(EspHttpError::BodyTooLarge);
            }
            body.extend_from_slice(&chunk[..read]);
        }

        let body = String::from_utf8(body).map_err(|_| EspHttpError::NotUtf8)?;
        Ok(HttpResponse { status, body })
    }
}

impl HttpTransport for EspTransport {
    type Error = EspHttpError;

    async fn post(&self, request: ShareRequest) -> Result<HttpResponse, Self::Error> {
        // Blocking work in an async fn, deliberately: see the module docs.
        self.request(&request)
    }
}
