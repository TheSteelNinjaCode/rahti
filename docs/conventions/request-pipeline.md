# Request Pipeline

Rahti has two request pipelines with different owners:

- the framework pipeline enforces transport and security invariants;
- the application pipeline is an ordered chain around matched dynamic route
  handlers.

An application can add cross-cutting behavior without rebuilding the generated
router or weakening the framework's security order.

## Application Middleware

A middleware function receives the owned Axum request and Rahti's
`request_pipeline::Next`. Call `next.run(request).await` to continue. Work
before that call follows registration order; work after it sees the finished
response and unwinds in reverse order.

```rust
use axum::extract::Request;
use axum::http::HeaderValue;
use axum::response::Response;
use rahti::request_pipeline::{Next, RequestPipeline};

async fn request_marker(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-application",
        HeaderValue::from_static("accounts"),
    );
    response
}

pub fn pipeline() -> RequestPipeline {
    RequestPipeline::new().with(request_marker)
}
```

Middleware may mutate request headers, the URI visible to the handler, and
request extensions before continuing. Route matching has already happened, so
changing the URI does not reroute the call. Middleware may inspect or mutate
status, headers, extensions, and body on the returned response. The request and
`Next` are owned and `Next::run` consumes itself, so the same continuation
cannot accidentally dispatch twice.

Register several functions by chaining `.with(...)`:

```rust
RequestPipeline::new()
    .with(request_id)
    .with(application_logging)
    .with(tenant_context)
```

The request order is `request_id`, `application_logging`, `tenant_context`,
then the handler. The response returns through those functions in the opposite
order.

## Installation

Install the complete chain during `initialize_application`, before building
the generated router:

```rust
mod middleware;

pub async fn initialize_application() -> Result<ApplicationRuntime, StartupError> {
    // Database and application policy setup...
    rahti::request_pipeline::configure(middleware::pipeline());

    Ok(ApplicationRuntime {
        router: routes::router(),
        host: routes::HOST,
        port: routes::PORT,
    })
}
```

The router snapshots the configured chain when it is built. Configuring later
does not mutate an already-running router. Calling `configure` again affects
only routers built afterwards. An application that installs nothing gets an
empty, transparent chain.

Keep the pipeline in an application-owned module such as `src/middleware.rs`.
Middleware commonly needs application types and policy; neither belongs in
generated `src/routes.rs` or in the framework crate.

## Short-Circuit Responses

A middleware may return without calling `next.run`:

```rust
use axum::response::IntoResponse;

async fn maintenance(request: Request, next: Next) -> Response {
    if maintenance_is_active() {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "Maintenance in progress",
        ).into_response();
    }
    next.run(request).await
}
```

That response is final application content: page layouts, page caching, body
parsing, and the handler do not run. Framework security headers still apply.
Return the response shape the caller expects—normally JSON for an RPC or API
and HTML or a redirect for a page.

Do not use a short circuit to recreate authentication, authorization, CSRF, or
rate limiting. Rahti owns those boundaries, and their dedicated APIs preserve
their response contracts.

## Exact Scope and Order

The application chain runs for matched pages, `route.rs` APIs, and RPC POSTs.
It does not run for:

- files in `public/`;
- Rahti development endpoints;
- the static/not-found fallback;
- the optional MCP endpoint;
- WebSocket conversations.

A WebSocket is upgraded from HTTP and then has a different message lifecycle;
put connection behavior in its `#[socket]` handler. MCP owns a separate bearer
and protocol boundary. Framework fallbacks stay deterministic and cannot be
silently replaced by an unrelated application hook.

Static generation drives the real router, so application middleware does run
while an exported page renders. Middleware used by exportable pages must not
require a network peer or other live-server-only extension.

For a matched dynamic request, the effective request order is:

```text
production observability (outermost)
  -> security headers
    -> rate limiting
      -> CSRF (when the application has RPCs)
        -> development revalidation
          -> route match
            -> authentication/session scope
              -> application request pipeline
                -> page cache / layouts / boundary / handler
                   or RPC body limit / panic conversion / handler
```

The response travels outward in reverse. Consequences worth preserving:

- a protected route is refused before application middleware runs;
- rate-limited and CSRF-refused requests cannot reach application middleware;
- application middleware runs on both a page-cache hit and miss, while the
  cached document does not retain per-request headers added outside the cache;
- a short circuit is not wrapped in a page layout or stored in the page cache;
- security headers cover application short circuits and ordinary responses.

A panic in application middleware is converted to a logged HTTP 500. RPCs get
the JSON RPC error shape and other routes get a non-cacheable Problem Details
response. Production hides the panic diagnostic in both shapes. The page
handler's error boundary is inside the application chain, so it cannot render
a panic that originated in middleware itself.

The auth session scope is already active inside application middleware, so it
may read `rahti::auth::session()`. Authentication alone is not authorization:
middleware that gates an application action must still apply the same named
ability or resource policy as the handler.

## Middleware Design

Use middleware for behavior that genuinely spans routes: request IDs,
structured timing, tenant context in request extensions, maintenance gates,
or application-wide response headers. Keep business validation and resource
authorization at the handler boundary where typed input and the selected
resource are available.

Middleware surrounds streaming responses but does not observe chunks after
the body starts sending. Do not buffer a stream merely to log it. Avoid
reading request bodies in general middleware: an owned HTTP body is a stream
and consuming it makes it unavailable to the RPC, upload, or API handler unless
the middleware deliberately reconstructs it. Secrets, cookie values,
authorization headers, and upload bodies do not belong in logs.

## Testing

Drive the generated router and assert both sides of the contract:

- request mutations reach a matched handler;
- response mutations reach the caller;
- registration and response-unwind order are stable;
- a deliberate short circuit prevents handler side effects;
- public files and framework endpoints do not enter the application chain;
- protected routes and rejected CSRF/rate-limit requests stop outside it.

Because the generated router snapshots the process-wide installed chain,
configure it before calling `routes::router()` in a test. Prefer harmless test
middleware or isolate tests that replace the global pipeline.
