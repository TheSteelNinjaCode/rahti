# Rate Limiting

Rahti applies one process-local sliding-window request budget to dynamic HTTP
traffic. The generated router installs it outside page, API, RPC, auth, CSRF,
and error work, and inside the security-header middleware. A refused request
therefore avoids application work and still receives the normal browser
security headers.

The default is **200 requests per client IP per minute**, retaining no more
than 10,000 client buckets. Defaults require no startup call.

## What is counted

Pages, API routes, RPCs, redirects, missing routes, and WebSocket handshakes
share the client's budget. These requests are exempt:

- `OPTIONS` preflight requests;
- `/health` and `/ready`, including their subtrees;
- Rahti's `/__rahti/dev` development subtree;
- an existing `GET` or `HEAD` file beneath the configured `public/` root.

Public-file exemption is based on the resolved file, not a URL extension or
directory prefix. `/css/app.css` is exempt only when that file exists beneath
`public/`; `/css/missing.css` is an ordinary limited 404. Decoded traversal and
symlink escape cannot turn a file outside `public/` into an exemption.

## Responses and headers

An admitted dynamic response receives:

| Header | Meaning |
| --- | --- |
| `RateLimit-Limit` | requests in the current policy |
| `RateLimit-Remaining` | requests left in the current client window |
| `RateLimit-Reset` | seconds until the oldest counted request expires |

Existing response values win. When the budget is exhausted, Rahti answers
HTTP 429 and adds `Retry-After` with the same reset duration. An RPC receives
the normal JSON error shape (`{"error":"…"}`), so `pp.rpc` rejects normally;
other requests receive a small HTML response.

## Configuration

Install a complete policy during `initialize_application`, before building the
router:

```rust
use std::time::Duration;
use rahti::rate_limit::{self, RateLimit};

rate_limit::configure(RateLimit {
    requests: 600,
    window: Duration::from_secs(60),
    max_clients: 25_000,
    trust_forwarded_headers: false,
    exempt_paths: vec![
        "/health".into(),
        "/__rahti/dev".into(),
        "/metrics".into(),
    ],
});
```

`RateLimit::per_second`, `per_minute`, and `per_hour` provide concise starting
policies. `RateLimit::disabled()` disables the middleware without removing the
generated layer. A positive policy requires a nonzero window and
`max_clients`; invalid configuration fails during startup.

Exempt paths cover their subtree, except `/`, which covers only the root. Do
not exempt a mutation, sign-in endpoint, or expensive dynamic route merely
because a monitoring tool calls it frequently.

Calling `configure` replaces the policy and clears all existing buckets. This
is useful for deterministic integration-test setup, but production startup
should call it at most once.

## Client identity and proxies

The scaffolded server uses `into_make_service_with_connect_info`, so each
request carries its transport peer address. Forwarding headers are ignored by
default: a directly connected client can write `X-Forwarded-For` itself and
otherwise obtain a new bucket with every request.

Behind a trusted reverse proxy, opt in only when the application server cannot
be reached directly and the proxy **replaces**, rather than appends to,
client-supplied `X-Forwarded-For`:

```rust
rate_limit::configure(RateLimit {
    trust_forwarded_headers: true,
    ..RateLimit::default()
});
```

Rahti accepts only a valid first IP address and falls back to the transport
peer when the header is absent or malformed. A custom server host must also
serve the router with connect information; without a transport peer (and
without a trusted valid forwarded address), the middleware deliberately does
not invent a shared `unknown` bucket.

## Process and deployment boundary

Buckets are in memory, bounded by `max_clients`, and disappear on restart.
When the table is full, an existing bucket is evicted without scanning the
entire table for perfect recency; that scan would itself become a CPU attack
under address churn. This makes memory and admission work predictable, but
sustained address churn can weaken the local limit. Multiple Rahti processes
also have independent budgets.

Use this middleware as the application-instance safety net. A deployment that
needs a durable, globally coordinated, account-based, credential-based, or
DDoS-grade limit must additionally enforce it at the reverse proxy, gateway,
or shared rate-limit service. Do not store authoritative rate-limit state in
Rahti's ordinary cache: cache eviction must not change what a caller is
allowed to do.

## Testing

An in-process router request has no network peer unless the test supplies one:

```rust
let mut request = axum::http::Request::builder()
    .uri("/account")
    .body(axum::body::Body::empty())?;
request.extensions_mut().insert(axum::extract::ConnectInfo(
    "192.0.2.10:3000".parse::<std::net::SocketAddr>()?,
));
```

Test admission, the 429 response, reset after the window, independent clients,
trusted-proxy behavior, and each deliberate exemption. Keep tests using a
temporary installed policy isolated because `configure` is process-wide.
