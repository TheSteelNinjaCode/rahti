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

## Workspace Crates

### `rahti`

The server runtime exports:

- `Html`, `Render`, `RenderJs`, and `Json`;
- `Attrs`, `Attributes`, `AttrList`, `Presence`, and `attribute`: whether an
  attribute is written at all, and what a `@{..spread}` puts on a tag;
- layout and error-boundary response layers, including the root-layout layer
  that names the document a response was rendered in;
- `Error`, `ErrorInfo`, and `Result`;
- RPC parsing, serialization, CSRF, streaming, and upload types;
- `auth`: a signed session cookie, a route-protection policy, and the guard
  layer that applies it — no user storage, no password handling, no providers,
  no roles;
- link-time component RPC registration;
- behind the optional `ws` cargo feature: `rahti::ws` — the socket endpoint,
  the `Socket`/`SocketSender` conversation types, link-time socket
  registration, and the same-origin handshake check (see `websockets.md`);
- public-directory resolution;
- `listen`: the server's listener — `PORT` and `HOST` override the configured
  address so a deployment needs no code change, and a busy port falls forward
  to the next free one in dev mode while release treats it as an error;
- `local_url`: where to open a browser for a listener, printing a wildcard
  bind address as `localhost` because `0.0.0.0` is not a destination;
- development reload routes and graceful-shutdown coordination — `signal` for
  Ctrl+C, and `begin_shutdown` for a host that says when instead of waiting to
  be told, which is what an installed application without a terminal needs;
- behind the default `dev-watch` cargo feature: the `public/` filesystem
  watcher. On for every web project; a build that turns it off keeps the reload
  stream and the diagnostics endpoint and drops `notify` entirely, which is
  what a packaged application wants — its assets are written to internal
  storage once per version and nothing edits them;
- the macros re-exported from `rahti-macros` — `#[socket]` only when `ws` is
  enabled, so the error for a socket without the feature names the feature
  rather than generated code.

### `rahti-macros`

- `html!` parses typed inline HTML, separates Rust and client expressions,
  escapes interpolated data, and stamps component boundaries. An
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
- detects component-library dependencies through Cargo metadata;
- generates routes, modules, and `.rahti/manifest.json`;
- when `db` is configured, generates `src/models/mod.rs` and
  `src/migrations/mod.rs` — including the `Migrator` — without depending on
  SeaORM or reading any of its code;
- compiles/copies CSS and includes component-library styles;
- emits Cargo watch directives for every relevant path.

### `cargo-rahti`

- `cargo rahti new <name>` scaffolds a project and records hashes of owned
  scaffold files. `--tailwind`, `--db [backend]`, `--ws`, and `--mcp` answer its
  questions from the command line; each flag only ever says yes.
- Every scaffolded project carries its own documentation: `AGENTS.md`,
  `CLAUDE.md`, and the applicable convention documents under
  `docs/conventions/`. The documents are embedded from copies in
  `crates/cargo-rahti/assets/docs/`, which a test holds byte-identical to
  the canonical set here.
- Every scaffolded project also carries a `.cargo/config.toml` whose `dev`
  alias makes `cargo dev` rebuild and restart the server on Rust edits via
  cargo-watch — the half of live reload the running server cannot do itself.
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
  SeaORM dependencies, the `ws` feature on `rahti`, `DATABASE_URL` — so the
  files the upgrade just wrote compile and run. Additive and idempotent, driven by
  `rahti.config.json` so a half-wired project is repaired by running it
  again.
- `--dry-run` previews an upgrade.
- `cargo rahti native <command>` is forwarded to `cargo-rahti-native`, a
  separately installed executable. The forwarding is a `Command` spawn and adds
  no dependency; a missing helper prints the line that installs it. Nothing
  else in `cargo-rahti` knows native packaging exists.
- A scaffolded project is a library plus a thin binary: `src/lib.rs` holds
  `initialize_application` — the database, the migrations the project chooses
  to run, the auth policy, the generated router — and `src/main.rs` calls it
  and binds the configured address. The split is what lets an optional native
  package start the same application without a second copy of the sequence.

### `rahti-mcp`

The standalone read-only MCP server uses the official Rust MCP SDK and stdio
transport. It reads `rahti.config.json`, generated `.rahti/manifest.json`,
convention documents, and a bounded tail of `.rahti/dev.log`; it does not link
into the application server, read `.env`, mutate the project, or inherit a
browser session. Generated RPC and socket auth metadata is visible to tools,
while private route policy remains runtime-owned application code.
It is a first-class optional scaffold feature: choosing MCP writes a
ledger-owned `.mcp.json` and records `"mcp": true`, while the executable stays
a separately installed developer tool and never becomes an application
dependency.

## Implemented Feature Inventory

The current implementation covers:

- static, dynamic, catch-all, optional catch-all, and route-group paths;
- nested layouts, nearest error boundaries, root 404, and unwrapped API routes;
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
- local and dependency-provided components, nested components, and children;
- nested component module directories with Rust-valid module names;
- isolated component instances and component-owned RPCs;
- PulsePoint state and the complete v2 hook/runtime surface documented in
  `pulsepoint.md`;
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
  and no role system;
- generated manifest metadata for routes, layouts, errors, loading regions,
  components, RPCs, sockets, and the database;
- optional read-only MCP project inspection over stdio, selected with
  `cargo rahti new --mcp`, configured by `.mcp.json`, and absent from an
  application that did not ask;
- scaffold creation — including each project's own `AGENTS.md`, `CLAUDE.md`,
  and convention documentation — and non-destructive upgrades;
- the framework side of optional native packaging: shared application startup,
  a shutdown a host can begin itself, and a `dev-watch` feature a mobile build
  turns off. The packaging itself is a separate install and a separate
  package — see `native-packaging.md` — so a project that never asks for one
  compiles, downloads and resolves none of it.

## Source-of-Truth Order

Use this order when behavior is unclear:

1. executable tests;
2. implementation in `crates/`;
3. generated output in an application (`src/routes.rs`,
   `.rahti/manifest.json`);
4. these convention documents.

Generated output is evidence, not an editing surface.

## The Browser Runtime Asset

PulsePoint ships as a prebuilt bundle. Nothing in this workspace compiles or
bundles JavaScript, and a checkout is complete as it stands.

The canonical copy of the runtime asset is:

```text
crates/cargo-rahti/assets/js/pp-reactive-v2.min.js
```

`cargo rahti new` installs it into every project as
`public/js/pp-reactive-v2.min.js`, where `public/js/main.js` imports the
minified runtime, installs `twMerge`, and calls `pp.mount()` once the DOM is
ready. Never hand-edit the bundle; replace it wholesale.
