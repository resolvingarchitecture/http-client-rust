# http-client (Rust)

A plain (non-anonymized) HTTP/HTTPS client for **1M5**. Outbound
(`sendOut`)-only — a Rust port of the client half of
[`http-client-java`](https://github.com/resolvingarchitecture/http-client-java)'s
`ra.http.HTTPService`. The Java version's embedded Jetty server (local
API/SPA/WebSocket hosting, used there to serve Tor hidden services) is not
ported; see `TODO.md`.

Used as the HTTP **protocol service** for
[`1m5-core-rust`](https://github.com/1m5/1m5-core-rust) via
`onemfive_core::protocol::HttpProtocolService` (`http` feature) — the router's
clearnet fallback path, alongside `tor-client-rust` and `i2p-rust`.

## Use

```rust
use std::collections::HashMap;
use http_client::{Envelope, HttpClient, Status};
use ra_common::envelope::{Action, HEADER_CONTENT_TYPE};

let client = HttpClient::from_config(&HashMap::new());
assert!(client.start());
assert_eq!(client.status(), Status::Connected);

let mut env = Envelope::document();
env.url = Some("https://resolvingarchitecture.io".to_string());
env.action = Some(Action::Get);
env.set_header(HEADER_CONTENT_TYPE, "text/html".into());
client.send(&mut env);                 // response body -> env.content(), errors -> env.message
client.stop();
```

`Envelope` is `ra_common::Envelope` (the same type `seda_bus::Envelope`
re-exports) — built on it directly rather than through `seda-bus`, because the
outbound path needs `::url`, `::action`, `::headers`, `::multipart` and
`::add_content`/`::content`, which is richer than the minimal
`headers`/`payload`/`to` shape `tor-client-rust` and `i2p-rust` were built
against.

### Config keys

| key | default | meaning |
|-----|---------|---------|
| `ra.http.trustAllCerts` | `false` | skip TLS certificate verification — test-only |
| `ra.http.requestTimeoutSecs` | `60` | per-request timeout |
| `ra.http.proxy` | none | a full proxy URL (`http://host:port` or `socks5://host:port`) |

## Build

```
cargo build
cargo test        # includes two live GET tests (http/https resolvingarchitecture.io);
                   # they skip cleanly (assert false send, don't panic) with no network
cargo clippy --all-targets
```

## Identity metadata leaks

**Fixed 2026-09-26**, the same class of bug found and fixed in
`http-client-java`/`-cpp`/`-python` and `1m5-remnant`'s Android `TorClient`:
this client used to set no explicit default `User-Agent`, leaving `ureq`
free to inject its own (documented as `ureq/<version>`) on any request with
none set. Now defaults to a generic, widely-shared browser value instead -
verified with `cargo build`/`cargo test` (6 tests + doctest, all pass,
including the live network tests). See `DESIGN.md` "Identity metadata
leaks" for the still-open SOCKS5 DNS-resolution check, which does need real
verification, not just assumed from `ureq`'s advertised `socks5://` support.

## Status

Early. GET/POST/PUT/DELETE over HTTP and HTTPS via [`ureq`](https://crates.io/crates/ureq)
(rustls backend). Optional HTTP/SOCKS5 proxy. Response bodies are attached to
the envelope as a (lossily UTF-8-decoded) string — not yet base64-safe for
binary bodies. No multipart *response* parsing, no redirect-follow
configuration beyond ureq's default, no `NetworkConnectionReport`-style
block detection (`ra-common-rust` doesn't have that type yet — see `TODO.md`).
No server / SPA / WebSocket hosting. See `DESIGN.md` and `TODO.md`.

**Note:** as of this writing, `tor-client-rust` and `i2p-rust` no longer
compile against the current `ra-common-rust::Envelope` shape (a pre-existing
break, unrelated to this crate) — see `TODO.md`.
