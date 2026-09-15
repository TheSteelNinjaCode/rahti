# Production Observability

Rahti supplies production-safe request correlation, structured lifecycle logs,
aggregate process metrics, and fixed liveness/readiness endpoints. Development
browser diagnostics remain a separate system in `.rahti/dev.log`.

The production defaults turn on automatically when Rahti development mode is
off. Records are newline-delimited JSON written to stderr, which container
runtimes and process supervisors can collect without access to the application
filesystem.

## Request Correlation

Every response receives `X-Request-Id`. A valid incoming value is preserved by
default so a trusted gateway and Rahti can share one correlation id. Accepted
values contain 1–128 ASCII letters, numbers, `-`, `_`, `.`, or `:`. Missing or
invalid values are replaced with a random 128-bit lowercase hexadecimal id.

The normalized id is also written back into the request headers, inserted as a
`rahti::observability::RequestId` request extension, and available from an
ordinary page or RPC with no request parameter:

```rust
if let Some(request_id) = rahti::observability::request_id() {
    println!("handling {request_id}");
}
```

Request ids correlate work; they are not credentials, proof of identity, or a
source of uniqueness for database records. Set `.accept_request_ids(false)`
when every id must originate in this process.

## Request Records

One completed observed request produces one JSON object:

```json
{"kind":"request","timestampMs":1789400000000,"level":"info","service":"shop","requestId":"84d9...","method":"GET","path":"/orders/42","status":200,"durationMs":3.42,"slow":false}
```

Rahti records the method and URL path, never the query string, body, cookies,
authorization header, CSRF token, upload contents, or response body. Statuses
at 400–499 and slow requests use `warn`; 500–599 uses `error`; other responses
use `info`. The default slow threshold is one second.

Existing public files are omitted from lifecycle logs and metrics. `/health`
and `/ready` are omitted by default as well, preventing frequent probes from
distorting application traffic. Set `.log_health_checks(true)` to include the
two probe routes. Every response still receives a request id.

Duration measures time until the HTTP response is produced. For a streaming
RPC, SSE response, download, or WebSocket upgrade it does not measure how long
the body or upgraded connection remains open. Middleware must not buffer a
stream to obtain a misleading “complete” duration.

## Configuration

Install a complete policy during `initialize_application`, before building the
router:

```rust
use std::time::Duration;
use rahti::observability::{self, Observability};

observability::configure(
    Observability::default()
        .service_name("orders")
        .slow_request(Duration::from_millis(750))
        .accept_request_ids(true)
        .log_health_checks(false),
);

let router = routes::router();
```

Without a service-name override, generated code uses the application's Cargo
package name. `.request_logs(false)` keeps ids, probes, and metrics but turns
off request JSON. `.enabled(false)` turns off correlation, lifecycle logs, and
metrics; the health endpoints remain available.

The generated router snapshots the policy. Configure it before
`routes::router()` and do not attempt runtime policy mutation.

## Application Events

Application code can write a correlated event to the same JSONL stream:

```rust
use rahti::serde_json::json;

rahti::observability::record_with(
    "info",
    "checkout",
    "order accepted",
    json!({ "orderId": order.id }),
);
```

Inside a request, the current `requestId` is added automatically. Structured
fields are nested under `fields`, so application data cannot overwrite the
record's timestamp, level, service, or request id. Level, target, and message
text are truncated at a UTF-8 boundary after 16 KiB.

The application owns every supplied field. Never record credentials, raw
cookies, authorization headers, session payloads, password material, reset
tokens, untrusted upload bodies, or personal data that the logging destination
is not approved to hold.

Rahti-handled backend failures use this event stream in production. In
development they continue to use the terminal plus `.rahti/dev.log`, avoiding
duplicate records.

Background jobs write completion, retry, terminal failure, panic, timeout, and
shutdown-grace events with `target: "jobs"`. They include the job id, stable
job name, attempt, duration, and originating request id when enqueued inside a
request. Payload values are never logged. See `background-jobs.md`.

Task scheduling uses `target: "schedule"` for successful dispatches,
overlap skips, queue-capacity skips, and dispatch failures. Records name the
stable schedule and target job, but never its payload. See
`task-scheduling.md`.

## Process Metrics

`rahti::observability::metrics()` returns a label-free snapshot:

- completed observed requests;
- currently active observed requests;
- 4xx and 5xx totals;
- slow-request total;
- cumulative and maximum response-production duration in microseconds.

The counters are atomic, process-local, monotonic until restart, and have no
route, user, tenant, request-id, or status-code labels. That fixed cardinality
keeps attacker-controlled paths from growing a metrics table.

Rahti does not expose metrics publicly. An application can export the snapshot
through its approved metrics collector or a separately authenticated internal
API. With several processes, aggregate their snapshots in the monitoring
system; one process cannot describe the fleet.

## Liveness and Readiness

Rahti reserves two public framework routes:

- `GET /health` returns HTTP 200 when the process can serve HTTP;
- `GET /ready` runs every configured dependency check concurrently and returns
  HTTP 200 only when all pass, otherwise HTTP 503.

Both responses are JSON, carry `Cache-Control: no-store`, receive security
headers, and bypass sessions, application middleware, and page rendering.
Rate limiting exempts both so a traffic incident does not make the orchestrator
mistake a live process for a dead one. Do not create `src/app/health/page.rs` or
`src/app/ready/page.rs`; the build rejects those reserved URLs.

No readiness checks means the process is ready. Add checks to the policy:

```rust
let policy = Observability::default()
    .readiness_timeout(Duration::from_secs(2))
    .with_readiness("database", || async {
        database_ping().await.map_err(|error| error.to_string())
    })
    .with_readiness("queue", || async {
        queue_ping().await.map_err(|error| error.to_string())
    });

observability::configure(policy);
```

Check names use 1–64 ASCII letters, numbers, `-`, or `_` and must be unique.
Checks have a two-second default timeout and run concurrently, so one stalled
dependency cannot stall the probe indefinitely or multiply the total timeout.
The public response includes each check name and `ok`, `failed`, or `timeout`,
but not its error detail. The detail is written to the structured event stream.

A readiness check must be side-effect free and cheap. It may verify a database
or required service, but it must not migrate schemas, create records, consume
messages, refresh credentials, or call a dependency that is merely optional.
Liveness deliberately checks no dependency: restarting a healthy process does
not repair an unavailable database.

## Pipeline Position

Production observability is the outermost generated layer. It sees page, API,
RPC, static, probe, development, MCP, fallback, rate-limit, CSRF, auth, and
application-middleware responses. Suppression policy decides which successful
requests become lifecycle records; early 4xx/5xx responses remain observable.

Application middleware runs much further inside, after auth. Use that pipeline
for application context such as tenant selection. Use production observability
for transport-wide correlation and aggregate request outcomes.

## Operational Boundary

Rahti provides logs, ids, probes, and in-process counters; it does not operate a
telemetry backend. The deployment still owns stderr collection, retention,
redaction policy, alert rules, dashboards, distributed trace export, and fleet
aggregation. A reverse proxy should apply its own connection and upstream
metrics as well—Rahti cannot observe requests that never reach the process.
