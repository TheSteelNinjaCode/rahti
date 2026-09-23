# Error Handling

Rahti separates expected client failures from unexpected internal failures,
keeps nested page error boundaries, and preserves machine-readable responses
for APIs and RPCs. The server records the diagnostic before client redaction;
production HTML or JSON must never be the diagnostic channel.

## Choosing an Error

A fallible page, API, or RPC returns `rahti::Result<T>`:

```rust
use axum::http::StatusCode;
use crate::rahti::{Error, Html, Result, html};

pub async fn page() -> Result<Html> {
    let order = find_order().await?
        .ok_or_else(|| Error::new(StatusCode::NOT_FOUND, "Order not found"))?;

    Ok(html! { <h1>@{order.number}</h1> })
}
```

Use the constructors according to who may read the message:

| Form | Diagnostic | Production client message |
| --- | --- | --- |
| `Error::new(4xx, message)` | `message` | `message` |
| `Error::new(5xx, message)` | `message` | generic |
| `Error::internal(message)` | `message` | generic |
| `Error::public(status, message)` | `message` | `message` |
| `Error::internal(diagnostic).with_public_message(message)` | `diagnostic` | `message` |
| an ordinary `std::error::Error` crossing `?` | its display text | generic |

The generic text is `An unexpected error occurred.`. A server status is private
by default even when constructed with `Error::new`; use `Error::public` only
when the exact text is intentionally safe, stable, and useful to every client.
`with_public_message` keeps a detailed operational diagnostic while replacing
only the client text.

`Error::message()` returns the diagnostic for server-side decisions. It is not
a value to copy into a response. The `ErrorInfo::message()` handed to a page
boundary is already the environment-appropriate client message.

## Page Boundaries

An `error.rs` exports:

```rust
use crate::rahti::{ErrorInfo, Html, html};

pub fn error(error: ErrorInfo) -> Html {
    let reference = error
        .request_id()
        .map(|id| format!("Reference: {id}"));

    html! {
        <section>
            <h1>@{error.reason()}</h1>
            <p>@{error.message()}</p>
            <small>@{reference}</small>
        </section>
    }
}
```

The nearest ancestor boundary receives the status, numeric code, canonical
reason, safe message, and optional production-observability request id. The
request id is a correlation reference, not authentication or a database id.

The boundary replaces the failed subtree. Layouts below it are skipped;
layouts above it still wrap the result. A rendered boundary removes the
internal failure marker, so an outer boundary does not render the same failure
again. RPC responses never pass through page boundaries.

If no boundary owns a failed page, or an unwrapped `route.rs` returns
`Error`, Rahti responds with `application/problem+json`:

```json
{
  "type": "about:blank",
  "title": "Conflict",
  "status": 409,
  "detail": "That name is already in use",
  "requestId": "gateway-42"
}
```

`requestId` is present when production observability is enabled. The problem
shape is also the safe fallback when rendering a layout or boundary panics;
it is not wrapped in the document that just failed.

## RPC Failures

An RPC keeps the PulsePoint wire rather than becoming an HTML page or Problem
Details response:

```json
{
  "error": "Order not found",
  "requestId": "gateway-42"
}
```

The status is preserved. `requestId` is optional and `error` remains the field
the browser runtime reads. `Error::with_details(...)` adds explicitly public
structured RPC context. Object fields are merged beside `error`, except that
neither `error` nor `requestId` can be overwritten. A non-object value is put
under `details`.

The browser receives a PulsePoint `RpcError`: `message` contains `error`,
`status` contains the HTTP status, `errors` contains the validation field map
or `{}`, `requestId` contains the correlation id or `null`, and `body` contains
the parsed JSON response. The 401 and 403 messages are normalized to
`Authentication required` and `Permission denied`; the original response is
still available in `body`.

Structured details follow the same exposure rule as the message: unexpected
production failures suppress them. Do not put a credential, raw session,
password material, token, connection string, or unapproved personal data in
public details.

Malformed RPC envelopes and mistyped arguments remain HTTP 400. Validation
failures remain HTTP 422 with their field map. Authentication, authorization,
CSRF, rate-limit, and upload refusals keep their established RPC response
contracts.

## Panics

Generated page groups catch handler panics and turn them into an internal
failure a boundary can render. The RPC router catches panics separately so a
fetch call always receives JSON. The application request pipeline also catches
a middleware, layout, or boundary panic and uses the caller's page/API or RPC
fallback shape.

A panic is an unexpected bug, not flow control. Return `Error` for a known
failure. Panic diagnostics are visible during development and recorded on the
server in production, but the production response is generic.

Streaming bodies and upgraded WebSocket conversations have already sent their
HTTP status when later work fails. Their errors therefore use their own stream
or socket protocol; an HTTP boundary cannot replace bytes already delivered.

## Logging, Correlation, and Caching

Every `Error` records its status and diagnostic through Rahti's backend
diagnostic path. Expected 4xx failures use `warn`; 5xx failures and panics use
`error`. Development writes the terminal and `.rahti/dev.log`. Production uses
the structured observability event stream and automatically includes the
current request id.

All framework-built Error and RPC error responses set
`Cache-Control: no-store`. The development revalidation layer respects that
explicit policy. Page caching stores successful anonymous documents only and
never stores a failed response.

The request lifecycle record is separate from the error event: the lifecycle
record supplies method, path, status, and duration, while the error event holds
the diagnostic. Do not repeat request bodies, cookies, authorization headers,
or query secrets in either one.

## Production Boundary

Production redaction is active whenever Rahti development mode is off. A debug
build defaults development mode on and a release build defaults it off;
`RAHTI_DEV` can override that choice. Never enable development mode on a public
deployment: it also enables development diagnostics and intentionally detailed
error messages.

An error page is a user experience, not an operational report. Show a useful
status, safe next action, and correlation reference. Keep stack traces,
database messages, filesystem paths, dependency payloads, and source details
in access-controlled server telemetry.
