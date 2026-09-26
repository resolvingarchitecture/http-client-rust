//! # http-client (Rust)
//!
//! A plain (non-anonymized) HTTP/HTTPS client for **1M5**, a Rust port of
//! [`http-client-java`](https://github.com/resolvingarchitecture/http-client-java)'s
//! `ra.http.HTTPService` — **outbound (`sendOut`) only**. The Java version's
//! embedded Jetty server (local API/SPA/WebSocket hosting, used to serve Tor
//! hidden services) is out of scope here; see `TODO.md`.
//!
//! Built directly on [`ra_common::Envelope`] (not through `seda-bus`'s
//! re-export, though they are the same type) because the outbound path needs
//! `Envelope::url`, `::action`, `::headers`, `::multipart` and
//! `::add_content`/`::content` directly — richer surface than the minimal
//! `headers`/`payload`/`to` shape `tor-client-rust` and `i2p-rust` were built
//! against. **Note:** as of this writing those two crates no longer compile
//! against the current `ra-common-rust::Envelope` (a pre-existing break, not
//! caused by this crate — see this crate's `TODO.md`).
//!
//! Used as the HTTP **protocol service** for `1m5-core-rust`
//! (`onemfive_core::protocol::HttpProtocolService`, `http` feature).
//!
//! ```no_run
//! use http_client::HttpClient;
//! use ra_common::Envelope;
//! use ra_common::envelope::{Action, HEADER_CONTENT_TYPE};
//! use std::collections::HashMap;
//!
//! let client = HttpClient::from_config(&HashMap::new());
//! if client.start() {
//!     let mut env = Envelope::document();
//!     env.url = Some("https://resolvingarchitecture.io".to_string());
//!     env.action = Some(Action::Get);
//!     env.set_header(HEADER_CONTENT_TYPE, "text/html".into());
//!     client.send(&mut env);
//! }
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use log::{info, warn};
use ra_common::serde_json::{self, Value};

use ra_common::envelope::{
    Action, HEADER_AUTHORIZATION, HEADER_CONTENT_DISPOSITION, HEADER_CONTENT_TRANSFER_ENCODING,
    HEADER_CONTENT_TYPE, HEADER_USER_AGENT,
};
/// Re-exported so callers need one crate; identical to `seda_bus::Envelope`.
pub use ra_common::Envelope;

/// Connection status. Mirrors `tor-client-rust`/`i2p-rust`'s per-crate `Status`
/// closely enough for `1m5-core-rust` to map onto its own `NetworkStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Disconnected,
    Connecting,
    Connected,
    /// A response in this session looked like a network-level block (HTTP
    /// 403/408/410/418/451/511) — see `handle_failure`. Informational only;
    /// does not stop further sends.
    Blocked,
    Error,
}

fn status_to_u8(s: Status) -> u8 {
    match s {
        Status::Disconnected => 0,
        Status::Connecting => 1,
        Status::Connected => 2,
        Status::Blocked => 3,
        Status::Error => 4,
    }
}
fn status_from_u8(v: u8) -> Status {
    match v {
        1 => Status::Connecting,
        2 => Status::Connected,
        3 => Status::Blocked,
        4 => Status::Error,
        _ => Status::Disconnected,
    }
}

/// Header names copied from an [`Envelope`] onto the outbound request, ported
/// from `HTTPService.sendOut`'s explicit allow-list.
const FORWARDED_HEADERS: &[&str] = &[
    HEADER_AUTHORIZATION,
    HEADER_CONTENT_DISPOSITION,
    HEADER_CONTENT_TYPE,
    HEADER_CONTENT_TRANSFER_ENCODING,
    HEADER_USER_AGENT,
];

/// Sent whenever a caller's `Envelope` has no `User-Agent` header of its own. Without this,
/// `ureq` is documented to inject its own `ureq/<version>` on any request with none set -
/// identifying the exact library to every destination and any on-path observer, a real
/// fingerprinting signal. A generic, widely-shared value instead - deliberately not reflecting
/// this library or its version - matches Tor Browser's own practice of giving every user an
/// identical, unremarkable fingerprint. See DESIGN.md "Identity metadata leaks".
const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:128.0) Gecko/20100101 Firefox/128.0";

/// A plain HTTP/HTTPS client. One `ureq::Agent`, rebuilt on [`start`](Self::start).
pub struct HttpClient {
    trust_all_certs: bool,
    request_timeout: Duration,
    /// A full proxy URL (`http://host:port` or `socks5://host:port`), e.g. so
    /// a future `tor-client-rust`/`i2p-rust` could route through this client's
    /// SOCKS support instead of hand-rolling their own.
    proxy: Option<String>,
    status: AtomicU8,
    agent: Mutex<Option<ureq::Agent>>,
}

impl HttpClient {
    pub fn new() -> HttpClient {
        HttpClient {
            trust_all_certs: false,
            request_timeout: Duration::from_secs(60),
            proxy: None,
            status: AtomicU8::new(status_to_u8(Status::Disconnected)),
            agent: Mutex::new(None),
        }
    }

    /// Config keys: `ra.http.trustAllCerts` (`true`/`false` — test-only, skips
    /// TLS certificate verification), `ra.http.requestTimeoutSecs`,
    /// `ra.http.proxy` (a full proxy URL).
    pub fn from_config(cfg: &HashMap<String, String>) -> HttpClient {
        let mut c = HttpClient::new();
        if let Some(v) = cfg.get("ra.http.trustAllCerts") {
            c.trust_all_certs = v.eq_ignore_ascii_case("true");
        }
        if let Some(t) = cfg
            .get("ra.http.requestTimeoutSecs")
            .and_then(|s| s.parse().ok())
        {
            c.request_timeout = Duration::from_secs(t);
        }
        if let Some(p) = cfg.get("ra.http.proxy") {
            c.proxy = Some(p.clone());
        }
        c
    }

    pub fn status(&self) -> Status {
        status_from_u8(self.status.load(Ordering::Acquire))
    }

    fn set_status(&self, s: Status) {
        self.status.store(status_to_u8(s), Ordering::Release);
    }

    /// Build the underlying `ureq::Agent`. Never panics; returns `false` (and
    /// sets [`Status::Error`]) if the proxy URL is malformed.
    pub fn start(&self) -> bool {
        self.set_status(Status::Connecting);

        let mut config = ureq::Agent::config_builder().timeout_global(Some(self.request_timeout));

        if self.trust_all_certs {
            let tls = ureq::tls::TlsConfig::builder()
                .disable_verification(true)
                .build();
            config = config.tls_config(tls);
        }

        if let Some(p) = &self.proxy {
            match ureq::Proxy::new(p) {
                Ok(proxy) => config = config.proxy(Some(proxy)),
                Err(e) => {
                    warn!("bad ra.http.proxy {p:?}: {e}");
                    self.set_status(Status::Error);
                    return false;
                }
            }
        }

        let agent: ureq::Agent = config.build().into();
        *self.agent.lock().unwrap() = Some(agent);
        self.set_status(Status::Connected);
        info!(
            "http client started (trust_all_certs={}, proxy={:?})",
            self.trust_all_certs, self.proxy
        );
        true
    }

    pub fn stop(&self) -> bool {
        *self.agent.lock().unwrap() = None;
        self.set_status(Status::Disconnected);
        true
    }

    /// Send `envelope.url` with `envelope.action` (GET/POST/PUT/DELETE),
    /// forwarding [`FORWARDED_HEADERS`] and a body from `envelope.multipart`
    /// (if set) else `envelope.content()`. The response body is written back
    /// via `envelope.add_content` as a (lossily UTF-8-decoded) string — binary
    /// bodies are not yet base64-safe, see `TODO.md`. Returns `false` only on
    /// a pre-flight error (not connected, no URL, no action) or a transport
    /// failure (couldn't reach the server); an HTTP error status (4xx/5xx)
    /// still returns `true` with the error recorded on the envelope, mirroring
    /// `HTTPService.sendOut`.
    pub fn send(&self, envelope: &mut Envelope) -> bool {
        let guard = self.agent.lock().unwrap();
        let Some(agent) = guard.as_ref() else {
            envelope.add_error_message("HTTP client not connected and unable to connect.");
            return false;
        };

        let Some(url) = envelope.url.clone() else {
            envelope.add_error_message("Must provide a URL.");
            return false;
        };

        let Some(action) = envelope.action else {
            envelope.add_error_message("Envelope.action must be set to Get, Post, Put, or Delete");
            return false;
        };

        // Body: multipart (if present, and its boundary overrides Content-Type)
        // takes priority over the plain document content.
        let mut header_overrides: Vec<(&'static str, String)> = Vec::new();
        let body: Vec<u8> = if let Some(mp) = envelope.multipart.take() {
            header_overrides.push((
                HEADER_CONTENT_TYPE,
                format!("multipart/form-data; boundary={}", mp.boundary),
            ));
            mp.finish().into_bytes()
        } else {
            match envelope.content() {
                Some(Value::String(s)) => s.clone().into_bytes(),
                Some(v) => serde_json::to_vec(v).unwrap_or_default(),
                None => Vec::new(),
            }
        };

        // ureq's RequestBuilder is typestate (WithBody/WithoutBody) so the two
        // branches can't share one `let mut builder = match ...` binding; each
        // applies the same headers and produces the same `Result` type.
        let result = match action {
            Action::Get | Action::Delete => {
                let mut builder = if matches!(action, Action::Get) {
                    agent.get(&url)
                } else {
                    agent.delete(&url)
                };
                for name in FORWARDED_HEADERS {
                    if let Some(Value::String(v)) = envelope.header(name) {
                        builder = builder.header(*name, v);
                    }
                }
                if !matches!(envelope.header(HEADER_USER_AGENT), Some(Value::String(_))) {
                    builder = builder.header(HEADER_USER_AGENT, DEFAULT_USER_AGENT);
                }
                for (name, value) in &header_overrides {
                    builder = builder.header(*name, value);
                }
                if body.is_empty() {
                    builder.call()
                } else {
                    builder.force_send_body().send(&body[..])
                }
            }
            Action::Post | Action::Put => {
                let mut builder = if matches!(action, Action::Post) {
                    agent.post(&url)
                } else {
                    agent.put(&url)
                };
                for name in FORWARDED_HEADERS {
                    if let Some(Value::String(v)) = envelope.header(name) {
                        builder = builder.header(*name, v);
                    }
                }
                if !matches!(envelope.header(HEADER_USER_AGENT), Some(Value::String(_))) {
                    builder = builder.header(HEADER_USER_AGENT, DEFAULT_USER_AGENT);
                }
                for (name, value) in &header_overrides {
                    builder = builder.header(*name, value);
                }
                builder.send(&body[..])
            }
        };

        match result {
            Ok(mut response) => {
                let status_code = response.status().as_u16();
                if !response.status().is_success() {
                    handle_failure(status_code, &url);
                    envelope.add_error_message(status_code.to_string());
                }
                match response.body_mut().read_to_vec() {
                    Ok(bytes) => {
                        envelope.add_content(Value::String(
                            String::from_utf8_lossy(&bytes).into_owned(),
                        ));
                    }
                    Err(e) => {
                        warn!("reading response body from {url} failed: {e}");
                        envelope.add_error_message(e.to_string());
                    }
                }
                true
            }
            Err(e) => {
                warn!("HTTP request to {url} failed: {e}");
                envelope.add_error_message(e.to_string());
                false
            }
        }
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Logs a network-block hint for well-known "you're being blocked" HTTP codes.
/// Ports `HTTPService.handleFailure`'s `switch` (403/408/410/418/451/511) —
/// logging only; `ra-common-rust` has no `NetworkConnectionReport` type yet to
/// carry this further (see `TODO.md`).
fn handle_failure(code: u16, url: &str) {
    let label = match code {
        403 => Some("BLOCKED-FORBIDDEN"),
        408 => Some("BLOCKED-TIMEOUT"),
        410 => Some("BLOCKED-GONE"),
        418 => Some("BLOCKED-TEAPOT"),
        451 => Some("BLOCKED-LEGAL"),
        511 => Some("BLOCKED-AUTHN"),
        _ => None,
    };
    if let Some(label) = label {
        warn!("HTTP {code} from {url}: {label} - request considered blocked.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_round_trips() {
        for s in [
            Status::Disconnected,
            Status::Connecting,
            Status::Connected,
            Status::Blocked,
            Status::Error,
        ] {
            assert_eq!(status_from_u8(status_to_u8(s)), s);
        }
    }

    #[test]
    fn from_config_parses_known_keys() {
        let mut cfg = HashMap::new();
        cfg.insert("ra.http.trustAllCerts".to_string(), "true".to_string());
        cfg.insert("ra.http.requestTimeoutSecs".to_string(), "5".to_string());
        cfg.insert(
            "ra.http.proxy".to_string(),
            "http://127.0.0.1:8080".to_string(),
        );
        let c = HttpClient::from_config(&cfg);
        assert!(c.trust_all_certs);
        assert_eq!(c.request_timeout, Duration::from_secs(5));
        assert_eq!(c.proxy.as_deref(), Some("http://127.0.0.1:8080"));
    }

    #[test]
    fn send_without_start_fails_cleanly() {
        let client = HttpClient::new();
        let mut env = Envelope::document();
        env.url = Some("http://example.invalid/".to_string());
        env.action = Some(Action::Get);
        assert!(!client.send(&mut env));
        assert!(!env.message.as_ref().unwrap().error_messages().is_empty());
    }

    #[test]
    fn send_without_url_fails_cleanly() {
        let client = HttpClient::from_config(&HashMap::new());
        assert!(client.start());
        let mut env = Envelope::document();
        env.action = Some(Action::Get);
        assert!(!client.send(&mut env));
        client.stop();
    }

    #[test]
    fn http_get_live() {
        let client = HttpClient::from_config(&HashMap::new());
        assert!(client.start());
        let mut env = Envelope::document();
        env.url = Some("http://resolvingarchitecture.io".to_string());
        env.action = Some(Action::Get);
        env.set_header(HEADER_CONTENT_TYPE, "text/html".into());
        let sent = client.send(&mut env);
        if !sent {
            eprintln!(
                "skipping: no outbound network ({:?})",
                env.message.as_ref().map(|m| m.error_messages())
            );
            return;
        }
        let body = env.content().and_then(Value::as_str).unwrap_or_default();
        assert!(body.contains("Resolving Architecture") || !body.is_empty());
        client.stop();
    }

    #[test]
    fn https_get_live() {
        let client = HttpClient::from_config(&HashMap::new());
        assert!(client.start());
        let mut env = Envelope::document();
        env.url = Some("https://resolvingarchitecture.io".to_string());
        env.action = Some(Action::Get);
        env.set_header(HEADER_CONTENT_TYPE, "text/html".into());
        let sent = client.send(&mut env);
        if !sent {
            eprintln!(
                "skipping: no outbound network ({:?})",
                env.message.as_ref().map(|m| m.error_messages())
            );
            return;
        }
        let body = env.content().and_then(Value::as_str).unwrap_or_default();
        assert!(body.contains("Resolving Architecture") || !body.is_empty());
        client.stop();
    }
}
