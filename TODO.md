# http-client (Rust) — TODO

## P0 — done
- [x] `HttpClient` — `from_config` (keys `ra.http.trustAllCerts` /
      `ra.http.requestTimeoutSecs` / `ra.http.proxy`), `start` / `stop` / `send`,
      `Status { Disconnected, Connecting, Connected, Blocked, Error }` as an
      `AtomicU8`.
- [x] `send`: GET/POST/PUT/DELETE via `ureq`, headers from `Envelope::
      FORWARDED_HEADERS` (Authorization / Content-Disposition / Content-Type /
      Content-Transfer-Encoding / User-Agent), body from `multipart` (boundary
      overrides Content-Type) else `content()`. Response body -> `add_content`
      as a UTF-8 (lossy) string; non-2xx recorded via `add_error_message` but
      still returns `true` (matches `HTTPService.sendOut`).
- [x] HTTPS via `ureq`'s rustls backend; `ra.http.trustAllCerts` disables
      verification for tests.
- [x] Optional HTTP/SOCKS5 proxy (`ra.http.proxy`).
- [x] Consumed by `1m5-core-rust` as `onemfive_core::protocol::HttpProtocolService`.

## P0.5 — Identity metadata leaks
- [x] **Fixed 2026-09-26**: no default `User-Agent` was set, and `ureq` is
      documented to inject its own `ureq/<version>` - see DESIGN.md
      "Identity metadata leaks". Now sends `DEFAULT_USER_AGENT` (generic)
      when the caller hasn't supplied one. `cargo build`/`cargo test` pass
      (6 tests + doctest, including live network tests).
- [ ] **Verify, don't just cite the crate's docs**: confirm `ureq`'s
      `socks5://` proxy support genuinely resolves the destination hostname
      via the proxy, not local DNS, before this crate (or a future
      `tor-client-rust` reuse, see the cross-repo item below) is trusted to
      route anything through a SOCKS relay - see DESIGN.md "Identity
      metadata leaks".

## P1 — response fidelity
- [ ] Binary response bodies: `add_content` takes `serde_json::Value`, which
      can't hold raw bytes; bodies are currently `String::from_utf8_lossy`'d.
      Base64-encode (with a header/flag marking the encoding) or wait for a
      `ra-common-rust` content type that isn't JSON.
- [ ] Surface response status code + headers on the envelope (Java doesn't
      either, but the router would benefit).
- [ ] Configurable redirect-follow / max-redirects (currently ureq's default).

## P2 — network-status integration
- [ ] `handle_failure` (403/408/410/418/451/511) only logs today —
      `ra-common-rust::network` has no `NetworkConnectionReport` type yet (its
      module doc says so explicitly). Wire it through once that lands, and
      transition `Status::Blocked` from it rather than leaving `Blocked`
      unreachable in practice.
- [ ] `ExternalRoute` / `SimpleExternalRoute::send_content_only` (java's
      `sendOut` branches on this to send raw content instead of the full
      envelope JSON) isn't consulted — `Envelope::url` already covers the
      primary "where do I send this" need directly, so this was skipped as a
      java-only nuance rather than ported.

## P3 — parity with the Java server half (not started, maybe never)
- [ ] `http-client-java`'s Jetty-based `launch`/`EnvelopeHandler`/`SPAHandler`/
      `EnvelopeWebSocket` (local API/SPA/WebSocket hosting) has no Rust
      equivalent. Out of scope for "HTTP as a protocol service"; would only
      matter if something here needs to host a hidden service or a localhost
      API the way `tor-client-java` does today.

## Testing / ops
- [x] Unit tests: status/config round-trips, pre-flight failures (not
      started, no URL, no action) without touching the network.
- [x] Live tests (`http_get_live`, `https_get_live`, unconditional — no
      `#[ignore]`, matching `HTTPServiceTest.java`): GET `http(s)://
      resolvingarchitecture.io`; skip cleanly (assert `send` returned `false`,
      don't panic) if there's no outbound network, mirroring how
      `tor-client-rust`'s embedded test is gated instead by feature/`#[ignore]`.
- [ ] `cargo clippy -- -D warnings`, `cargo fmt --check` in CI.
- [ ] Publish to crates.io once the API settles (currently git/path dep only).

## Cross-repo — pre-existing, not this crate's to fix
- [ ] `tor-client-rust` and `i2p-rust` currently fail `cargo build`: both are
      written against an older, minimal `ra_common::Envelope` shape
      (`headers: HashMap<String,String>`, `payload: Vec<u8>`, `to: String`)
      that no longer exists on `ra-common-rust`'s current `Envelope` (now
      `headers: serde_json::Map<String,Value>`, no `payload`/`to` fields, plus
      `url`/`action`/`multipart`/`content()`). Confirmed via `cargo build` in
      both repos as of this writing. This crate was built against the
      *current* shape instead of copying their (broken) pattern.
- [ ] Once fixed, decide whether `tor-client-rust`/`i2p-rust` should grow real
      HTTP(S) support by depending on *this* crate (via its proxy support)
      instead of each hand-rolling a raw-socket GET — would let both drop
      their bespoke `http`/`socks` modules for the HTTP-over-SOCKS path and
      pick up HTTPS for free.
