# Development Diagnostics

## Outcome

While development mode is active, Rahti makes frontend failures visible to
tools that can read the workspace but cannot see browser DevTools:

```text
.rahti/dev.log
```

The file is newline-delimited JSON (JSONL): one complete JSON object per line.
It is truncated when a new development server process initializes, so it
describes the current server session. `/.rahti` is gitignored in both this
repository and newly scaffolded applications.

Browser diagnostics are also mirrored in concise form to the server terminal.

## What Is Captured

The injected development client starts before deferred module scripts,
including `public/js/main.js`, execute. It captures:

- `console.warn(...)`;
- `console.error(...)`;
- uncaught JavaScript exceptions;
- unhandled Promise rejections;
- script, stylesheet, image, and other resource load failures.

Rahti also records framework-handled backend failures:

- fallible page responses and handler panics;
- failed and panicking RPCs;
- upload spooling failures and invalid upload-limit configuration;
- RPC stream serialization failures;
- CSRF random-source failures.

Arbitrary application `println!`, `eprintln!`, or third-party Rust logging is
not intercepted. Those remain terminal output. Production request records,
correlation ids, health probes, process metrics, and application events use the
separate contract in `production-observability.md`.

## Browser Record

Example:

```json
{"kind":"browser","receivedAtMs":1785300000000,"boot":"...","level":"error","message":"[PP-ERROR] Handler failed: ...","stack":"Error: console.error\n ...","url":"http://127.0.0.1:3000/counter","source":"http://127.0.0.1:3000/counter","line":42,"column":17,"component":"page_7db1cef0 (section)","timestamp":"2026-07-29T12:00:00.000Z"}
```

Fields:

- `kind`: `browser`;
- `receivedAtMs`: server receipt time;
- `boot`: development server process identifier;
- `level`: `warn`, `error`, `uncaught`, `unhandledrejection`, or
  `resource-error`;
- `message`: safely serialized console arguments or error reason;
- `stack`: browser stack when available;
- `url`: document URL;
- `source`, `line`, `column`: browser-reported source location when available;
- `component`: best-effort active `pp-component` boundary;
- `file`: the application file that boundary comes from (`src/app/page.rs`),
  when the build manifest knows it — resolved from `component`, or from a
  `pp://component/<id>.js` frame in the stack;
- `timestamp`: browser capture time.

Console APIs do not expose their call-site filename separately. Their captured
stack is therefore the source-location evidence. The PulsePoint bundle ships
minified and without a source map, so a stack that lands inside it gives a
bundle line/column and nothing more readable. Identify such a record by its
message text and by the `component` boundary it came from.

Code that runs in a component's own `<script>` is different: PulsePoint
compiles each one under the name `pp://component/<id>.js`, so its frames read
`at walkIn (pp://component/page_33bf1147.js:12:7)` — the function, the
component, and the line within that script.

## Server Record

Example:

```json
{"kind":"server","receivedAtMs":1785300000000,"boot":"...","level":"warn","target":"rpc","message":"rahti: rpc 422 Unprocessable Entity — The text field is required."}
```

`target` identifies the framework subsystem, such as `page`, `page-panic`,
`rpc`, `rpc-panic`, `rpc-upload`, `rpc-stream`, `socket`, `upload-config`,
`csrf`, `jobs`, or `schedule`. An expected refusal — any 4xx an rpc or socket
returns on purpose — is `warn`; `error` is reserved for failures.

## Limits and Failure Behavior

- one browser request is limited to 128 KiB;
- a batch is limited to 50 entries;
- individual accepted string fields are limited to 16 KiB;
- the client serializes circular values and truncates large console arguments;
- diagnostic delivery failures are silent to prevent recursive
  “logging failed” warnings;
- the endpoint and injected client exist only when Rahti development mode is
  enabled.

The bridge is diagnostic, not durable telemetry. A browser closing during a
network failure can lose its final batch.

## AI Debugging Workflow

1. Start the application with `cargo dev` or another dev-enabled run.
2. Reproduce the frontend behavior in a browser.
3. Read `.rahti/dev.log` from the beginning of the current session.
4. Group records by `boot`, URL, component, and message.
5. Open the record's `file` when present — it is the page, layout, error
   boundary or loading region the failure came from. Use `source`/line/column
   when present; otherwise inspect `stack`, where a `pp://component/` frame
   names the component script and the line inside it.
6. A message from the runtime itself names the condition it detected — read
   it against the markup and script of the reported `component`.
7. Fix the smallest owning block and rerun the reproduction.
8. Confirm no new matching record appears after the server restarts.

Do not treat an old record as proof that the current code still fails. The
session header and `boot` field identify the server instance that received it.
