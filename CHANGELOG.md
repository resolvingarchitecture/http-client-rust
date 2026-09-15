# Changelog

## 0.1.0 — 2026-09-14

First Rust implementation, porting the outbound (`sendOut`) half of
`http-client-java` 1.2.0's `ra.http.HTTPService`.

- `HttpClient` — `from_config` (keys `ra.http.trustAllCerts` /
  `ra.http.requestTimeoutSecs` / `ra.http.proxy`), `start` / `stop` / `send`,
  `Status { Disconnected, Connecting, Connected, Blocked, Error }` as an
  `AtomicU8`.
- `send(&mut Envelope)`: GET/POST/PUT/DELETE via `ureq` (rustls); forwards
  Authorization/Content-Disposition/Content-Type/Content-Transfer-Encoding/
  User-Agent headers; body from `envelope.multipart` (boundary overrides
  Content-Type) else `envelope.content()`; response body written back via
  `add_content` as a UTF-8 string; HTTP error status recorded via
  `add_error_message` without failing the call, matching `HTTPService.sendOut`.
- Built directly on `ra_common::Envelope` rather than through `seda_bus`'s
  re-export — `tor-client-rust`/`i2p-rust`'s minimal `headers`/`payload`/`to`
  pattern is stale against the current `Envelope` (see `TODO.md`); this crate
  targets the current, richer shape (`url`/`action`/`headers`/`multipart`/
  `content()`) instead.
- Consumed by `1m5-core-rust` as `onemfive_core::protocol::HttpProtocolService`
  (`http` feature).

Not yet: binary-safe response bodies (currently UTF-8-lossy), status
code/headers on the envelope, `NetworkConnectionReport`-based block detection
(the type doesn't exist in `ra-common-rust` yet).
