# Design

## Scope

Ports `ra.http.HTTPService` (`http-client-java`) **outbound path only**:
`sendOut(Envelope)` — build a GET/POST/PUT/DELETE request from the envelope,
send it, write the response back onto the envelope. The Java class is also a
Jetty-based local HTTP **server** (`launch`, `EnvelopeHandler`, `SPAHandler`,
`EnvelopeWebSocket`, session handling) used there to serve `tor-client-java`'s
hidden service and a future desktop-facing localhost API. None of that is
needed for "HTTP as a protocol service" (the router only ever calls `send`),
so it's out of scope here — see `TODO.md` if a Rust equivalent is ever wanted.

## Why `ra_common::Envelope` directly, not `seda_bus::Envelope`

`tor-client-rust` and `i2p-rust` both `pub use seda_bus::Envelope` and talk to
it through three fields: `headers: HashMap<String, String>`, `payload:
Vec<u8>`, `to: String`. That shape no longer exists — `ra-common-rust`'s
`Envelope` (which `seda_bus::Envelope` has always literally been a re-export
of) has grown a full `ra.common.Envelope`-equivalent surface: `url: Option<
String>`, `action: Option<Action>` (`Get`/`Post`/`Put`/`Delete`), `headers:
serde_json::Map<String, Value>`, `multipart: Option<Multipart>`, plus
`add_content`/`content()` for the message body and `add_error_message` for
errors. Building `HTTPService.sendOut`'s header allow-list / method dispatch /
body logic on the old three-field shape would mean re-deriving most of that
richer surface by hand inside this crate. Depending on `ra-common` directly
and using it as designed is both less code and a closer port of the Java
original — at the cost of `tor-client-rust`/`i2p-rust` currently **not**
compiling against it (see `TODO.md`; not this crate's bug to fix).

## HTTP library: `ureq`

`tor-client-rust`/`i2p-rust` deliberately avoid an HTTP crate (hand-rolled
SOCKS + a bare GET) because they only ever needed one verb, unauthenticated,
over a tunnel they already owned. This crate needs GET/POST/PUT/DELETE,
arbitrary headers, a body, real HTTPS, and (for parity with the Java trust-all
test option) certificate-verification bypass — reimplementing that by hand
means reimplementing chunked transfer-encoding and a TLS stack. `ureq` is:

- blocking / sync, matching every other client in this codebase (no tokio
  requirement, unlike the `embedded` features of `tor-client-rust`/`i2p-rust`);
- `rustls`-backed (no C OpenSSL dependency), consistent with `arti-client`'s
  choice in `tor-client-rust`;
- small enough to be a base (non-optional) dependency rather than something
  gated behind a feature the way the embedded routers are.

`ureq::Agent` is rebuilt in `start()` (config: timeout, optional trust-all TLS,
optional proxy) and torn down in `stop()`. Its `RequestBuilder` is a typestate
(`WithBody`/`WithoutBody` per verb) — `send()` therefore branches once on the
verb, rather than building one generic request builder, so the header/body
application logic is written twice (GET/DELETE vs POST/PUT branches) instead
of once. A small duplication in exchange for not fighting the type system.

## Status model

`Status { Disconnected, Connecting, Connected, Blocked, Error }`, stored as an
`AtomicU8` — the same pattern `TorClient`/`I2pClient` use, so
`onemfive_core::protocol`'s `map_status` convention extends here unchanged.
`Blocked` is set only via `handle_failure`'s logging path today (see below);
nothing currently transitions the client's own status to `Blocked` — that's
future work once there's a reason to change routing on it (see `TODO.md`).

## Response body → envelope

Java stores the raw response bytes (`e.addContent(responseBody.bytes())`) and
its own test just wraps them back in a `String`. `ra_common::Envelope::
add_content` takes a `serde_json::Value`, which can't hold raw bytes — this
crate stores `Value::String(String::from_utf8_lossy(bytes))`. That's exact for
text responses (HTML, JSON, plain text — everything the Java test suite and
the protocol-service use case exercise) and lossy for binary ones. Proper
binary support (base64, or a `ra-common-rust` content type that isn't
`serde_json::Value`) is deferred — see `TODO.md`.

## Identity metadata leaks

Required standard for any HTTP client this project relies on for anonymized
traffic (Tor/I2P), enforced here and checked against every sibling
`http-client-*` port: no default header, response header, or connection
behavior may reveal more about the requester than it has to.

- **Fixed 2026-09-26**: this crate used to set no explicit default
  `User-Agent`. `ureq` is documented to inject its own `User-Agent:
  ureq/<version>` when a request has none - not independently re-verified
  against this crate's exact pinned version by disassembly the way
  `http-client-java`'s OkHttp behavior was, but the fix is the same
  regardless of the exact string: `DEFAULT_USER_AGENT` (a generic,
  widely-shared browser value) is now set on both the GET/DELETE and
  POST/PUT request-builder branches whenever the caller hasn't supplied
  one. Same fix already applied to `http-client-java` (confirmed via
  bytecode), `http-client-cpp`/`http-client-python` (both previously
  defaulted to the project-identifying literal `"ra-http-client"`, arguably
  worse), `http-client-go`/`http-client-ts`, and `1m5-remnant`'s Android
  `TorClient`. Verified with `cargo build`/`cargo test` (6 tests + doctest,
  all pass, including the live network tests) - not by inspecting the
  actual bytes sent, since this crate's tests have no local mock server to
  capture request headers against (unlike `http-client-ts`'s equivalent
  check).
- **Not yet verified**: `ureq`'s `socks5://` proxy support is a documented,
  advertised feature, presumed to hand the destination hostname to the SOCKS
  layer for remote resolution rather than resolving it locally first -
  matching what `http-client-cpp`'s `ConnectThroughSocks5` was directly
  confirmed to do. That presumption hasn't been checked against `ureq`'s
  actual source in this pass. Verify before relying on this crate (or a
  future `tor-client-rust` reuse of it, per `TODO.md`'s cross-repo item) to
  route anything through a SOCKS relay like `tor-client-java`'s
  `TorSocksRelay` - a local resolution would leak the destination outside
  the proxy entirely, the same bug found and fixed in
  `bitcoin-client-java`'s bitcoinj DNS-seed lookups (`tor-client-java`,
  2026-09-25).
- **No server/inbound half** (see "Scope" above), so the third known leak
  shape - a server-identifying response header, found and fixed in
  `http-client-java`'s Jetty listener (`Server: Jetty(<version>)`) - doesn't
  apply yet. Check for it if P3's server-hosting parity is ever built.

## Blocked-response detection

`HTTPService.handleFailure` maps 403/408/410/418/451/511 onto a
`NetworkConnectionReport` so the router can reason about censorship signals.
`ra-common-rust`'s `network` module doc comment states plainly that
`NetworkConnectionReport` (and the rest of the "full network service layer")
is deferred to a later phase — it doesn't exist yet. `handle_failure` here
does the same code-to-label mapping but only logs; wiring it to something the
router can see is blocked on that type landing upstream.
