# Rahti

A server-rendered web framework for Rust. Pages are files, components are
ordinary functions, server calls are attributes, and the browser receives
real HTML with reactivity exactly where you asked for it — no Node, no
bundler, no JavaScript build step.

```rust
// src/app/page.rs — where this file sits is the / route.
use crate::rahti::{Html, html, rpc};

pub async fn page() -> Html {
    html! {
        <section class="page">
            <h1 class="title">"Hello"</h1>

            <button class="button" onclick={greet()}>"Say hello"</button>
            <p class="lede">{message}</p>

            <script>r##"
                const [message, setMessage] = pp.state("");

                async function greet() {
                    setMessage(await pp.rpc("hello", {}));
                }
            "##</script>
        </section>
    }
}

/// Runs on the server, in Rust. The browser calls it by name over a typed,
/// CSRF-protected wire and never learns there was a function here.
#[rpc]
pub async fn hello() -> String {
    "Hello from Rust.".to_string()
}
```

The dividing line is the interpolation. `@{…}` is a Rust expression, rendered
and escaped on the server. `{…}` is a JavaScript expression, bound in the
browser by PulsePoint — the bundled reactive runtime with a React-shaped hook
surface (`pp.state`, `pp.effect`, `pp.memo`, context, reducers, transitions).
Client code is compiled during the Rust build into a registry served at
`/_rahti/client.js`; the browser runs registered code under a CSP without
`unsafe-eval`. One file, both worlds, and each side is handled by the machinery that
understands it.

## Getting started

```bash
cargo install cargo-rahti
```

```bash
cargo rahti new my-app --tailwind
```

```bash
cd my-app && cargo run
```

Open <http://localhost:3000> and edit `src/app/page.rs`. Add `--db
[sqlite|postgres|mysql]` for a SeaORM-backed database, `--ws` for WebSockets,
or `--mcp` for read-only local stdio plus secured Streamable HTTP MCP; leave a
flag out and the project carries nothing of the feature.

## What ships in the box

- **File-system routing** under `src/app/`: nested layouts, route groups,
  dynamic and catch-all segments, error boundaries, navigation loading
  regions, a root 404, and unwrapped API routes.
- **Typed HTML.** `html!` is parsed at compile time; interpolated values are
  escaped by default.
- **Components as functions.** `#[component]` on a PascalCase function, then
  `<Card title="…">…</Card>` in markup; children arrive as an `Html` argument
  and render with `<slot />`.
- **Typed RPC.** `#[rpc]` beside its caller, CSRF protection by default,
  `rahti::Result<T>` error propagation, compile-time checking of literal `pp.rpc` calls against Rust signatures,
  server streaming, and uploads with
  progress.
- **Localization** (optional): build-validated Fluent catalogs embedded in the
  binary, request-local locale and fallback selection, plural rules,
  locale-prefixed routes, language direction, localized auth redirects, and
  static export per locale.
- **WebSockets** (optional `ws` feature): `#[socket]` functions beside their
  pages, arguments as the connection's first frame, a same-origin handshake
  check, `#[socket(auth)]` refusing the upgrade without a session, and
  cloneable senders for broadcast.
- **Authentication** as a signed session cookie plus an application-owned
  route policy. Signing secrets and cookie names are generated per project,
  and a release build refuses to start on a placeholder secret.
- **Authorization and validation.** Named abilities and typed resource
  policies fail closed; `Validator` returns deterministic field errors as HTTP
  422 without confusing malformed RPC payloads with invalid business input.
- **Metadata, SEO, and static generation.** Layout/page metadata inheritance,
  request-specific overrides, canonical/social tags, and static HTML builds
  through the real generated router, including declared dynamic paths.
- **Caching and revalidation.** A backend-neutral typed cache with a bounded
  in-memory default, plus explicit anonymous page caching and path/tag
  invalidation.
- **Production HTTP foundations.** Security headers and CSP, process-local
  rate limiting, an ordered application middleware pipeline, correlated safe
  error responses, JSONL events, bounded metrics, and health/readiness probes.
- **Background work and time.** Bounded typed job queues with retries,
  timeouts, and graceful drain; interval, daily, and one-shot scheduling; and
  validated IANA application-timezone handling.
- **Database** (optional): SeaORM wiring with one file per table and one per
  migration, in filename order. Migrations never run automatically.
- **Deployment** is a compiled binary and a `public/` directory, or static
  output produced by `cargo rahti build --static`.

## Written for coding agents

Rahti is not in any model's training data, so every scaffolded project
carries the framework with it: an `AGENTS.md` project guide (with a
`CLAUDE.md` pointing at it) and the convention documents under
`docs/conventions/`, covering routing, rendering, reactivity, RPC, auth, and
security. Open a new project in a coding agent and it has everything it
needs to write correct, secure Rahti code — and `cargo rahti upgrade` keeps
unedited documents current as the framework evolves.

The rest of the framework holds up its end: there is one way to do each
thing, mistakes surface as build errors that name the broken rule, and
development diagnostics — browser warnings included — are captured as JSON
Lines in `.rahti/dev.log`, a file an agent can read.

## The workspace

| Crate | Role |
| --- | --- |
| `rahti` | The server runtime: rendering, metadata, RPC, auth, validation, caching, security, observability, background work, time, and sockets. |
| `rahti-js` | Compiles template bindings, handlers, and scripts ahead of time for the client registry. |
| `rahti-macros` | `html!`, `#[component]`, `#[rpc]`, `#[socket]` — re-exported by `rahti`. |
| `rahti-build` | The build step: scans conventions, generates the router and modules, compiles CSS. |
| `cargo-rahti` | `cargo rahti new`, `cargo rahti upgrade`, and `cargo rahti build --static`. |
| `rahti-mcp` | Optional read-only project inspection over local stdio or secured Streamable HTTP. |

This repository is the framework workspace: the crates above, and the
documents under [`docs/conventions/`](docs/conventions/) — the canonical
account of how Rahti behaves, shipped into every scaffolded project.

## Status

Early, and released honestly as such: all crates share one version, ship in
lockstep, and are pre-1.0. Projects keep up without churn — `cargo rahti
upgrade` refreshes scaffold files whose recorded hashes show you never
edited them, and leaves everything you touched alone.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
