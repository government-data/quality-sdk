//! The HTTPS calls to AWS, behind a port so the clients are tested with a
//! fake: a request goes out signed, an answer comes back with its status.

use std::time::Duration;

/// A request, before or after it is signed.
#[derive(Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: &str, url: &str) -> Self {
        Self {
            method: method.into(),
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    /// The value of a header, by name in any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Signed requests carry credentials: they are never printed.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("body_len", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// An answer: its status and body, whatever the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends a request. An error is only that no answer came (DNS, TLS, a
/// timeout); an answer with any status is a [`Response`].
pub trait Transport {
    fn send(&self, request: Request) -> Result<Response, String>;
}

/// The largest answer read: an index, a module's package or a token.
const MAX_BODY: u64 = 1 << 30;

/// HTTPS with rustls and the Mozilla roots (ureq).
#[derive(Debug)]
pub struct Https {
    agent: ureq::Agent,
}

impl Default for Https {
    fn default() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_global(Some(Duration::from_secs(600)))
            .build();
        Self {
            agent: config.into(),
        }
    }
}

impl Transport for Https {
    fn send(&self, r: Request) -> Result<Response, String> {
        let mut builder = ureq::http::Request::builder()
            .method(r.method.as_str())
            .uri(r.url.as_str());
        for (k, v) in &r.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        // A GET carries no body at all, not an empty one.
        let sent = if r.body.is_empty() {
            builder
                .body(())
                .map_err(|e| e.to_string())
                .and_then(|q| self.agent.run(q).map_err(|e| e.to_string()))
        } else {
            builder
                .body(r.body)
                .map_err(|e| e.to_string())
                .and_then(|q| self.agent.run(q).map_err(|e| e.to_string()))
        };
        let response = sent.map_err(|e| format!("{} {}: {e}", r.method, r.url))?;
        let status = response.status().as_u16();
        let body = response
            .into_body()
            .with_config()
            .limit(MAX_BODY)
            .read_to_vec()
            .map_err(|e| format!("{} {}: {e}", r.method, r.url))?;
        Ok(Response { status, body })
    }
}
