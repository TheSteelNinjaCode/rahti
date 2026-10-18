# Architecture and Feature Inventory

## Ownership

Rahti renders the initial response on the server. PulsePoint compiles and
binds reactive expressions in the browser after the document loads.

```text
src/app + src/components + src/models + src/migrations
        |
        v
rahti-build scans conventions
        |
        +--> src/routes.rs              (generated, checked in)
        +--> src/components/mod.rs      (generated, checked in)
        +--> src/models/mod.rs          (generated, when a db is configured)
        +--> src/migrations/mod.rs      (generated, carries the Migrator)
        +--> .rahti/manifest.json       (generated tooling metadata)
        +--> public/css/styles.css      (built/copied according to config)
        |
        v
Rust handlers render Html
        |
        v
browser loads /js/main.js
        |
        v
PulsePoint v2 mounts reactive blocks
```

The generated router is ordinary Rust compiled into the application. Rahti
does not discover routes at runtime.

An application's startup lives in its `src/lib.rs`, behind
`initialize_application`: the application timezone, the database, the
migrations it chooses to run, the auth policy, the application request
pipeline, the production-observability policy, and the generated router.
`src/main.rs` calls it and binds the public
listener. A native package calls the same function and binds a loopback one —
see `native-packaging.md` — which is why the sequence is in a library rather
than in a `main` Android would have no way to reach.

```text
src/lib.rs::initialize_application
        |
        +--> src/main.rs        binds HOST:PORT, the web deployment
        +--> native/src/lib.rs  binds 127.0.0.1:0, the packaged application
```

## Framework Packages

### `rahti`

The server runtime exports:

- `Html`, `Render`, `RenderJs`, and `Json`;
- `Attrs`, `Attributes`, `AttrList`, `Presence`, and `attribute`: whether an
  attribute is written at all, and what a `@{..spread}` puts on a tag;
- layout and error-boundary response layers, including the root-layout layer
  that names the document a response was rendered in;
- `Error`, `ErrorInfo`, and `Result`: nested page boundaries, production-safe
  message redaction, Problem Details fallbacks, RPC-preserving JSON failures,
  request correlation, and non-cacheable error responses;
- RPC parsing, serialization, CSRF, streaming, and upload types;
- `auth`: a signed session cookie, a route-protection policy, and the guard
  layer that applies it — no user storage, no password handling, no providers,
  no roles;
- `authorization`: application-defined named abilities, typed session and
  resource policies, optional before hooks, and fail-closed 401/403 decisions;
- `security`: application-wide browser security headers, a PulsePoint-compatible
  CSP, production-only HSTS, and validated application overrides that preserve
  response-owned values;
- `rate_limit`: a bounded process-local sliding-window request budget keyed by
  validated client IP, with real-public-file exemptions, proxy opt-in, 429 RPC
  compatibility, and limit/reset response headers;
- `request_pipeline`: ordered application middleware around matched pages,
  APIs, and RPCs, with owned request mutation, reverse response unwinding, and
  deliberate short circuits inside framework security boundaries;
- `observability`: production request ids, JSONL request/application events,
  fixed-cardinality process metrics, and framework-owned liveness/readiness
  probes with concurrent dependency checks;
- `jobs`: typed named handlers on bounded process-local queues, explicit
  backpressure, retry/backoff and attempt timeouts, request-origin
  correlation, fixed-cardinality counters, and grace-bounded shutdown drain;
- `schedule`: process-local interval, application-timezone/UTC daily, and
  one-shot timers that submit registered jobs, prevent overlap by default,
  coalesce missed ticks, and stop before job queues drain;
- `time`: validated IANA application time, aware local `now`/`today`, UTC
  conversion and half-open local-day query bounds, kept deliberately separate
  from absolute expiry and duration clocks;
- `i18n`: immutable embedded Fluent catalogs, request-task-local locale
  selection, fallback lookup, plural/select formatting, localized path
  construction, document direction, and `Content-Language`;
- `cache`: typed JSON caching over a replaceable asynchronous byte-store
  contract, with expiration and invalidation plus a bounded process-local LRU
  default; pages can opt into anonymous complete-document caching with TTL,
  tags, and path/tag revalidation;
- `validation`: deterministic field-error collection, common typed-input
  rules, custom checks, and structured HTTP 422 RPC failures that compose with
  `rahti::Result`;
- link-time component RPC registration;
- behind the optional `ws` cargo feature: `rahti::ws` — the socket endpoint,
  the `Socket`/`SocketSender` conversation types, link-time socket
  registration, same-origin handshake check, and PulsePoint heartbeat
  replies (see `websockets.md`);
- behind the optional `mcp` cargo feature: the bearer-protected Streamable
  HTTP endpoint, its relative canonical redirect, and public discovery;
- public-directory resolution;
- in-process static export through the real application router, including
  guarded output ownership, public-asset copying, and a route-by-route report;
- `listen`: the server's listener — `PORT` and `HOST` override the configured
  address so a deployment needs no code change, and a busy port falls forward
  to the next free one in dev mode while release treats it as an error;
- `local_url`: where to open a browser for a listener, printing a wildcard
  bind address as `localhost` because `0.0.0.0` is not a destination;
- development reload routes and graceful-shutdown coordination — `signal` for
  Ctrl+C, and `begin_shutdown` for a host that says when instead of waiting to
  be told, which is every native package;
- behind the default `dev-watch` cargo feature: the `public/` filesystem
  watcher. A Windows native debug run serves the checkout and uses the same
  fast asset reload as the web host. Android cannot read the host directory,
  so `cargo-rahti-native` watches it on the development machine and rebuilds,
  reinstalls and restarts with a new embedded tree. A release host forces
  development mode off, leaving the reload routes and watcher inert;
- the macros re-exported from `rahti-macros` — `#[socket]` only when `ws` is
  enabled, so the error for a socket without the feature names the feature
  rather than generated code.

### `rahti-macros`

- `html!` reads a template written as tokens — markup, quoted text, `{…}`
  bindings and `@{…}` values — with each `<script>`/`<style>` body one raw
  string, so JavaScript and CSS never meet the Rust lexer. `parse.rs` refuses
  authoring mistakes (`authoring.rs`) on the token that caused them. It separates
  Rust and client expressions, escapes interpolated data, and stamps component
  boundaries. An
  uppercase-named tag is a component call: props map to the function's
  parameters by name, and the tag's children become its `children` argument.
  A `<>…</>` fragment roots several siblings between comment markers that
  carry the block's name; the runtime raises them into a layout-invisible
  boundary at hydration.
- In an open tag it also decides attribute *presence*: an `@{…}` value writes
  its own name, so `None` writes no attribute, and a `bool` on one of the
  names HTML reads by presence writes `name=""` or nothing. The list of those
  names is in `parse.rs`, consulted at compile time — the same `false` means
  opposite things on `disabled` and on `aria-hidden`. `@{..expr}` spreads an
  attribute set onto the element, and a tag carrying one builds its whole open
  tag through `rahti::AttrList` so that a later attribute replaces an earlier
  one instead of being written beside it.
- `#[component]` validates a PascalCase function, stamps its returned root,
  and generates the hidden named-props companion the tag form calls through.
- `#[rpc]` preserves the Rust function and generates its request shim and
  registry entry. `#[rpc(auth)]` adds a session check to the shim, ahead of
  reading the payload; `auth` is the only argument the attribute takes.
- `#[socket]` preserves the Rust function and generates the shim that reads
  the connection's first frame into the parameters, plus its registry entry.
  `#[socket(auth)]` marks the entry so the handshake is refused with a 401
  before the upgrade; `auth` is again the only argument.

### `rahti-build`

On every relevant Cargo build it:

- reads `rahti.config.json`;
- scans route convention files and `src/components/*.rs`;
- validates route, RPC, and socket-name conflicts;
- generates routes, modules, and `.rahti/manifest.json`;
- when `db` is configured, generates `src/models/mod.rs` and
  `src/migrations/mod.rs` — including the `Migrator` — without depending on
  SeaORM or reading any of its code;
- compiles or copies CSS;
- validates configured locale identifiers and Fluent catalogs, then embeds
  their sources into the generated router;
- emits Cargo watch directives for every relevant path.

### `cargo-rahti`

- `cargo rahti fmt [--check] [path...]` formats authored `html!` nesting and
  attribute lists while preserving expressions, raw strings, comments, and
  attribute order. It excludes generated sources and leaves blocks with
  source-sensitive bare text untouched. Ordinary Rust uses `cargo fmt`.

- `cargo rahti new <name>` scaffolds a project and records hashes of owned
  scaffold files. `--tailwind`, `--db [backend]`, `--ws`, and `--mcp` answer its
  questions from the command line; each flag only ever says yes.
- `cargo rahti build --static` runs the application binary in export mode,
  writing fixed pages and declared dynamic paths to `dist/` (or `--out`) and
  copying public assets without binding a listener.
- Every scaffolded project carries its own documentation: `AGENTS.md`,
  `CLAUDE.md`, and the applicable convention documents under
  `docs/conventions/`. The documents are embedded from copies in
  `crates/cargo-rahti/assets/docs/`, which a test holds byte-identical to
  the canonical set here.
- Every scaffolded project also carries a `.cargo/config.toml` whose `dev`
  alias makes `cargo dev` rebuild and restart the server on Rust edits via
  cargo-watch — the half of live reload the running server cannot do itself.
- Every scaffolded project starts with a passing, area-organized test suite:
  `src/tests/mod.rs` collects focused modules, while `support.rs` drives the
  generated router in-process. The suite is included from `src/lib.rs` and
  run by `cargo test`. A database project also gets `database.rs`; its shared
  setup connects a file SQLite database and runs the migrations. The
  dev-dependencies the harness needs are written into the manifest, and
  `testing.md` is the shipped document for the structure.
- `cargo rahti upgrade` replaces only scaffold files whose recorded hashes
  still match, preserving user-edited files.
- `cargo rahti upgrade --force` takes edited files back: bare, the framework's
  own, leaving everything under `src/` except `src/main.rs`; with paths, only
  those. `Cargo.toml` is never forced, and an unknown path is refused.
- `cargo rahti upgrade` moves the `rahti` and `rahti-build` version pins to
  the running tool's version — forward only, never onto a `--local` path
  dependency or a floating requirement — because the framework ships in
  lockstep and a stale pin runs the new files against an older runtime.
- `cargo rahti upgrade --db`/`--ws`/`--mcp` add a feature to a project that started
  without it. `Cargo.toml` and `.env` are never regenerated from a template,
  but `crates/cargo-rahti/src/wiring.rs` amends them — the version pins, the
  SeaORM dependencies, the test suite's dev-dependencies, the `ws` feature on
  `rahti`, `DATABASE_URL` — so the files the upgrade just wrote compile and
  run. Additive and idempotent, driven by
  `rahti.config.json` so a half-wired project is repaired by running it
  again.
- `--dry-run` previews an upgrade.
- `cargo rahti native <command>` is forwarded to `cargo-rahti-native`, a
  separately installed executable. The forwarding is a `Command` spawn and adds
  no dependency; a missing helper prints the line that installs it. Nothing
  else in `cargo-rahti` knows native packaging exists.

### `rahti-mcp`

The read-only MCP server uses the official Rust MCP SDK. Local clients launch
its stdio executable through `.mcp.json`; an opted-in application compiles the
same tools behind bearer-protected Streamable HTTP at `/mcp/`. It reads
`rahti.config.json`, generated `.rahti/manifest.json`, convention documents,
the public `llms.txt`, and a bounded tail of `.rahti/dev.log`; it never mutates
the project or inherits a browser session. Generated RPC and socket auth
metadata is visible to tools, while private route policy remains runtime-owned
application code. Projects without the feature compile neither the HTTP
transport nor its routes.

### `rahti-native`

In the separate `rahti-native` repository, with `cargo-rahti-native` beside it.
Optional, and Tauri-free so that it compiles and tests in an ordinary
workspace run:

- `AppPaths`: where an installed application's data, config, cache, database,
  uploads, spill directory, logs and staged assets are, per platform — and the
  rule that none of them is inside the installation directory;
- `EmbeddedServer`: a `127.0.0.1` listener on an OS-assigned port, with no API
  to bind anything else, and `RunningServer` with a bounded graceful shutdown
  that also fires Rahti's shutdown broadcast;
- the per-installation session key: generated on the device at first launch,
  DPAPI-encrypted on Windows, restored into `AUTH_SECRET` before the auth
  policy is built;
- `NativeConfig`: `rahti.native.json`, and the validation that refuses an
  identifier Android would reject before a build starts;
- the loopback gate, the security headers, and the `window.rahtiNative` bridge script —
  which attaches to the PulsePoint runtime without the bundle being touched.

### `cargo-rahti-native`

- `init` writes the application-owned Tauri shell under `native/` and
  `rahti.native.json` beside it. Additive, idempotent, and non-destructive:
  an edited file is left alone and reported, `--force` takes it back.
- `doctor` checks only what the selected target needs, installs nothing, and
  says what is missing, why, and which command supplies it.
- `dev` and `build` delegate to `cargo tauri`, having checked the
  prerequisites first, and print the absolute path of every artifact. `dev`
  adds the application outside `native/` to Tauri's watch set: Rust edits
  rebuild and restart both targets, while Android asset edits also rebuild and
  redeploy the content-fingerprinted embedded tree.

Neither crate is reachable from a web-only project: `native/` is excluded from
the project workspace, and `cargo-rahti-native` is a separate install.

## Implemented Feature Inventory

The current implementation covers:

- optional server-side localization with configured Unicode locale
  identifiers, build-validated and binary-embedded Fluent catalogs,
  request-local lookup and fallback, named arguments, locale plural rules,
  `[locale]`/`[lang]` route validation, language direction, localized auth
  policy matching and redirects, `Content-Language`, and automatic static
  expansion when locale is a page's only dynamic segment;

- static, dynamic, catch-all, optional catch-all, and route-group paths;
- static site generation through the application's real startup and generated
  router: fixed pages automatically, dynamic pages through async
  `static_paths()`, request-resolved metadata intact, public assets copied,
  and redirects, non-HTML responses, failures, and server-dependent browser
  calls surfaced in the build report;
- nested layouts, nearest error boundaries, root 404, and unwrapped API routes;
- typed static and request-specific metadata inherited from root layout through
  nested layouts to the page, including titles and templates, descriptions,
  canonical links, robots directives, Open Graph and Twitter fields, with the
  managed head synchronized after same-document navigation;
- `loading.rs` regions: rendered once at startup, written into every document,
  and chosen in the browser by walking the current path up to the nearest one
  — with a build-time refusal of a region under a dynamic segment, which the
  literal-path walk could never match;
- multiple root layouts, with the identity Rahti derives for each — injected
  as the `<template>` around the layout's slot, as the `<meta>` in its head,
  and as the response header — and the full browser load PulsePoint performs
  when a navigation crosses two of them;
- Axum extractors in page handlers, including path parameters and query
  strings — with extractor rejections landing outside the error boundary;
- server HTML interpolation, browser expression passthrough, safe script
  seeding, trusted `Html`, optional fragments, concatenated fragments, slots;
- server values that can never become client code: braces escaped in every
  value and spread, raised one encoding layer inside the root layout's
  deferred template, compile-time refusal of a server value in a handler or
  a `{…}` binding, a render-time refusal of a handler from a data spread, and
  `Bindings` as the one carrier of a component tag's `{…}` source;
- local and dependency-provided components, nested components, and children;
- nested component module directories with Rust-valid module names;
- isolated component instances and component-owned RPCs;
- PulsePoint state and the complete v2 hook/runtime surface documented in
  `pulsepoint.md` and its companion guides (hooks, performance, web APIs,
  navigation), with native event bindings evaluated only when events fire,
  including long names such as `onlostpointercapture`;
- JSON RPCs, fallible RPCs, component dispatch, CSRF, server streaming,
  uploads with progress, buffered/spilled files, and direct streaming uploads;
- optional WebSockets behind rahti's `ws` feature: `#[socket]` functions in
  pages and components, one app-wide endpoint dispatching by name from the
  link-time registry, arguments as the connection's first frame, a same-origin
  handshake check, `#[socket(auth)]` refusing the upgrade without a session,
  shutdown-aware connections, and cloneable `SocketSender`s so shared state
  can broadcast one message to every open connection — chosen at
  `cargo rahti new --ws`, and absent from a project that did not ask;
- static asset fallback, configurable public directory, development reload,
  CSS hot swapping, browser/backend diagnostic capture, graceful shutdown;
- Tailwind, plain CSS, and externally managed CSS modes;
- globally available `twMerge` support for browser-computed Tailwind class
  strings;
- an optional SeaORM database: a scaffold choice of backend, generated module
  wiring for `src/models/` and a generated `Migrator` for `src/migrations/`,
  and a process-wide connection reached without handler state — the framework
  itself depends on no ORM;
- authentication as a signed, stateless session cookie plus an app-owned route
  policy: public-first or private-first, subtree-covering route scopes,
  `?next=` round-tripping with open-redirect refusal, and `#[rpc(auth)]` for
  the call as opposed to the page — user storage and password checking are the
  application's (a `user` table and argon2id in `src/auth.rs`, typically), and
  the framework stores no users, sees no password, and ships no OAuth provider
  and no role system; session lifetime comes from `SESSION_LIFETIME_HOURS`,
  while the application explicitly chooses fixed or sliding expiry;
- authorization through application-defined named abilities, typed session and
  resource policies, optional before hooks, and fail-closed decisions that
  distinguish an unauthenticated caller from an authenticated but forbidden
  one without inventing a role or permission model;
- application-wide security middleware around the complete generated router:
  a PulsePoint-compatible content security policy, browser security headers,
  production-only HSTS, and validated startup overrides that cannot silently
  replace a response-owned header;
- a bounded process-local sliding-window rate limiter outside application,
  authentication, and CSRF work, with validated client-IP selection, explicit
  trusted-proxy opt-in, exemptions for real public files and framework health
  traffic, RPC-compatible 429 failures, and limit/reset headers;
- an ordered application request pipeline around matched pages, APIs, and
  RPCs: middleware can mutate the owned request, unwind over the response in
  reverse order, or short-circuit application work without escaping the
  framework's CSRF, rate-limit, security-header, or observability boundaries;
- production observability through request correlation, bounded JSONL request
  and application events, fixed-cardinality process metrics, and
  framework-owned liveness/readiness endpoints whose named dependency checks
  run concurrently under a common timeout;
- error handling that sends page failures and panics to the nearest boundary,
  preserves the RPC error wire format, emits Problem Details for unbounded HTTP
  failures, redacts private server diagnostics in production, correlates the
  response with its request, and prevents failures from being cached; the
  bundled PulsePoint client exposes structured `RpcError` fields and client
  component error boundaries;
- deterministic typed-input validation through `Validator`, including common
  rules, custom checks, named field messages, and structured HTTP 422 RPC
  failures while malformed or mistyped payloads remain HTTP 400;
- application-data caching through a backend-neutral async `CacheStore`, a
  typed JSON facade, explicit expiration/removal/clear operations, and a
  bounded in-memory LRU default; a public deterministic page can separately
  opt into anonymous complete-document caching with TTL and tags, and
  successful authoritative writes can explicitly revalidate a concrete path
  or tag; no distributed adapter or implicit caching is implied;
- typed process-local background jobs on bounded named queues, with explicit
  backpressure, retries and backoff, attempt timeouts, request-origin
  correlation, fixed-cardinality metrics, and grace-bounded shutdown drain;
- process-local task scheduling for intervals, one-shot instants, and daily
  application-timezone or UTC wall times, submitting registered jobs with
  overlap prevention by default, missed-tick coalescing, bounded-queue
  behavior, and shutdown ordering ahead of job-queue drain;
- application time fixed once at startup from a validated IANA
  `APP_TIMEZONE`, defaulting to UTC, with aware local `now`/`today`, UTC
  conversion, DST-correct half-open local-day query bounds, and deliberate
  separation from absolute expiration and duration clocks;
- generated manifest metadata for routes, layouts, errors, loading regions,
  components, RPCs, sockets, and the database;
- optional read-only MCP project inspection over local stdio and secured
  Streamable HTTP, selected with `cargo rahti new --mcp`, with public
  `llms.txt` and well-known discovery, and absent from an application that did
  not ask;
- scaffold creation — including each project's own `AGENTS.md`, `CLAUDE.md`,
  convention documentation, and a passing test suite in `src/tests/` — and
  non-destructive upgrades;
- optional native packaging for Windows and Android: the existing application's
  Rust backend compiled for the platform, serving the existing generated router
  on a loopback socket to a system WebView, with packaged application paths, a
  per-installation session key, a launch-scoped loopback gate, and a five-entry
  native command allowlist, and development reload for application Rust and
  assets — opt-in through a separate `cargo-rahti-native` install and an
  application-owned `native/` shell, and absent from a project that did not
  ask. Not a translation of HTML into platform widgets: see
  `native-packaging.md`.

## Source-of-Truth Order

Use this order when behavior is unclear:

1. executable tests;
2. implementation in `crates/`;
3. generated output (`src/routes.rs`, `.rahti/manifest.json`);
4. scaffold templates and assets under `crates/cargo-rahti/assets/`;
5. these convention documents.

Generated output is evidence, not an editing surface.

## The Browser Runtime Asset

PulsePoint ships as a prebuilt bundle. Nothing in this workspace compiles or
bundles JavaScript, and a checkout is complete as it stands.

The canonical runtime asset in this framework workspace is:

```text
crates/cargo-rahti/assets/js/pp-reactive-v2.min.js
```

`cargo rahti new` copies it to `public/js/pp-reactive-v2.min.js` in the new
application. The scaffolded `public/js/main.js` imports that bundle, installs
`twMerge`, and calls `pp.mount()` once the DOM is ready.
