# Rahti Agent Guide

## Purpose

This repository is the Rahti framework workspace: the crates under `crates/`,
the canonical convention documentation under `docs/conventions/`, and the
scaffold assets that `cargo rahti new` ships into every new application. No
application lives here — users scaffold their own with `cargo rahti new`, the
way `npx create-next-app` scaffolds a Next.js project. Treat the
implementation and the crate test suites as the source of truth, and keep the
convention documentation synchronized whenever a framework rule changes.

Rahti is a server-rendered Rust framework with:

- file-system routing under `src/app/` of an application;
- typed HTML through `html!`;
- ordinary Rust functions as components;
- browser reactivity supplied by the bundled PulsePoint v2 runtime;
- typed Rust RPCs, streaming responses, and uploads;
- generated routing, component modules, manifests, and CSS integration.

## Required Reading

Read only the documents relevant to the change, but read this file first.
The convention documents describe the framework from inside a scaffolded
application, which is the perspective every rule below serves.

| Work area | Canonical document |
| --- | --- |
| Repository architecture and generated files | `docs/conventions/architecture.md` |
| Pages, layouts, route groups, dynamic routes, APIs, errors, 404s | `docs/conventions/routing.md` |
| `html!`, interpolation, escaping, components, children | `docs/conventions/rendering-and-components.md` |
| Page titles, descriptions, social cards, canonical URLs, robots | `docs/conventions/metadata-and-seo.md` |
| Static HTML builds, dynamic export paths, output safety | `docs/conventions/static-generation.md` |
| Cache stores, page caching, expiration, invalidation | `docs/conventions/caching.md` |
| Locales, Fluent catalogs, fallback, localized routes, language direction | `docs/conventions/localization.md` |
| Typed input rules, field errors, HTTP 422 validation failures | `docs/conventions/validation.md` |
| Browser state, hooks, bindings, events, lists, refs, SPA navigation, native web APIs (WebGPU, Workers, WebAssembly) | `docs/conventions/pulsepoint.md` |
| Browser/backend development diagnostics and `.rahti/dev.log` | `docs/conventions/diagnostics.md` |
| `#[rpc]`, errors, component RPCs, streams, files | `docs/conventions/rpc-and-uploads.md` |
| WebSockets: `#[socket]`, `pp.socket`, the optional `ws` feature | `docs/conventions/websockets.md` |
| Sessions, route protection, `#[rpc(auth)]` | `docs/conventions/authentication.md` |
| Abilities, resource policies, 401/403 decisions | `docs/conventions/authorization.md` |
| CSP, browser security headers, HSTS, policy overrides | `docs/conventions/security-middleware.md` |
| Request budgets, client IPs, proxy trust, HTTP 429 | `docs/conventions/rate-limiting.md` |
| Application middleware, ordering, short circuits | `docs/conventions/request-pipeline.md` |
| Request ids, JSONL events, metrics, health checks | `docs/conventions/production-observability.md` |
| Boundaries, Problem Details, redaction, correlation | `docs/conventions/error-handling.md` |
| Bounded queues, retries, timeouts, graceful drain | `docs/conventions/background-jobs.md` |
| Intervals, daily/one-shot schedules, overlap | `docs/conventions/task-scheduling.md` |
| IANA application time, conversion, DST-safe day bounds | `docs/conventions/application-timezone.md` |
| SeaORM entities, migrations, the connection, database tests | `docs/conventions/database.md` |
| Escaping, CSRF, redirects, upload trust, development exposure | `docs/conventions/security.md` |
| `rahti.config.json`, CSS, static files, CLI, environment variables | `docs/conventions/configuration-and-tooling.md` |
| Workspace test placement and verification | `docs/conventions/workspace-testing.md` |
| Application test suites (shipped to new projects) | `docs/conventions/testing.md` |
| Optional Windows/Android packaging, packaged paths, the loopback server | `docs/conventions/native-packaging.md` |

When documentation and code disagree, verify the behavior in source and tests,
fix the documentation in the same change, and call out the mismatch.

## Non-Negotiable Framework Conventions

These are the rules Rahti enforces on applications. Framework changes must
preserve them, and documentation or scaffold changes must teach them.

1. `rahti-build` owns the generated files in an application —
   `src/routes.rs`, `src/components/mod.rs`, `src/models/mod.rs` and
   `src/migrations/mod.rs` (when a database is configured), and
   `.rahti/manifest.json`. Applications never hand-edit them; the framework
   must keep regeneration deterministic and complete.
2. Application routes live under the configured `app.dir` (`src/app` by
   default). `_private` directories are ignored by the router.
3. A page is `page.rs`; a layout is `layout.rs`; an error boundary is
   `error.rs`; a navigation loading region is `loading.rs`; the root 404 is
   `not-found.rs`; an unwrapped API endpoint is `route.rs`. A route segment
   cannot contain both `page.rs` and `route.rs`.
4. `html!` has one authored root: a literal element, a single component tag,
   or a `<>…</>` fragment. The root layout is the exception: it writes a
   literal `<html>` root, optionally preceded by a doctype.
5. Server values use `@{rust_expression}`. PulsePoint/browser expressions use
   `{javascript_expression}`. Never interchange the two.
6. Text in `html!` is authored as quoted Rust strings. Runtime values are
   escaped; `Html::from_raw` is only for explicitly trusted markup.
7. An `@{…}` attribute value decides whether its attribute is written at all.
   `Option::None` writes none, and a `bool` on one of the names HTML reads by
   presence writes `name=""` or nothing. That list lives in `rahti-macros`'
   `parse.rs` and is consulted by name at compile time: `aria-*`,
   `contenteditable`, `draggable`, `spellcheck`, `translate` and
   `autocomplete` are enumerated rather than boolean and stay off it, because
   `aria-hidden="false"` says what no absent attribute says. `@{..expr}`
   spreads an attribute set onto an element, last-wins — two dots, inside the
   server dialect, never JSX's `{...expr}`, which is PulsePoint's. A component
   takes typed props and declares `attrs: Attrs` to forward attributes; there
   are no implicit rest-props, because an unknown prop has to stay a compile
   error.
8. Components are PascalCase functions marked `#[component]`, return `Html`,
   and take props as normal Rust arguments. In markup they are called as
   tags — `<Card title="…">…</Card>` — where props are named attributes
   (literal or `@{expr}`); the function-call form remains for expression
   contexts such as loops and conditionals.
9. Children are an explicit `Html` argument rendered with `<slot />`. An
   empty tag pair passes `Html::empty()`, a self-closing tag passes no
   `children` at all. `Html::concat(...)` joins a list built in Rust.
10. A reactive block and its `<script>` must be self-contained. Children
    written between component tags are the exception: a `{…}` binding there
    reads the writing block's state — `html!` ships such children as an
    owner-scoped `<template pp-owner="…">`, the React children semantics —
    and keeps that owner when a component hands them on with `@{children}`.
    Fragment-rooted (`<>…</>`) children and `Html` built in Rust read the
    component's scope instead. `pp.props` works in a component's script,
    markup, and inline handlers alike. A client `{…}` a component writes on
    its own root is a compile error (root attributes are evaluated in the
    caller's scope; handlers excepted). A script-less component whose own
    markup binds a client value gets an empty `<script></script>` from
    `html!`, so that markup reads its own scope.
11. A page-owned `#[rpc]` sits beside its caller in that `page.rs`; a
    component-owned `#[rpc]` sits in the component file. Component RPC names
    are application-wide; the same page RPC name may exist on different page
    URLs.
12. RPC parameters are owned, deserializable types (`String`, not `&str`).
    RPC return values must serialize. Failures go through `rahti::Result<T>`.
13. PulsePoint's runtime-managed DOM attributes (`data-pp-*`, `pp-owner`,
    `pp-ref-owner`, the root-layout markers) are never written by hand —
    not in applications, not in scaffold templates, not in documentation
    examples.
14. PulsePoint is a shipped binary asset. Its canonical copy in this
    repository is `crates/cargo-rahti/assets/js/pp-reactive-v2.min.js`,
    which `cargo rahti new` installs into a project as
    `public/js/pp-reactive-v2.min.js`, mounted by `public/js/main.js`.
    Never hand-edit the bundle; replace it wholesale. Describe it by its
    *behaviour* — what a page renders, what a binding updates, what reaches
    `.rahti/dev.log`. Documentation, doc comments, and code comments name the
    bundle or name nothing: no JavaScript or TypeScript module filenames, no
    line numbers inside the bundle, and no filesystem paths outside the
    repository. Everything a reader is pointed at must exist in their own
    checkout.
15. A layout that owns a document below the app root declares
    `pub const ROOT_LAYOUT: bool = true;` on a line of its own and renders a
    complete document. Rahti derives that layout's identity from its path and
    puts it in three places: the `<template>` injected around its `<slot />`,
    the `X-PP-Root-Layout` header, and a `pp-root-layout` meta tag.
16. A `loading.rs` is not a layer and inherits nothing. It takes no
    arguments, is rendered once at startup, and ships in every document; the
    browser picks one by walking the path it is *leaving* up to the nearest
    region. That walk is over literal segments, so a `loading.rs` under a
    dynamic segment is a build error. It does nothing until a layout marks
    the region it replaces with `pp-loading-content="true"`.
17. Development browser warnings/errors and Rahti-handled backend failures
    are written as JSON Lines to `.rahti/dev.log` in an application. That
    file is generated, gitignored, and never shipped.
18. The database is SeaORM and belongs to the application, not the framework.
    `rahti` does not depend on it, and `rahti-build` generates the module
    wiring without reading a line of it. `src/models/` holds one flat file
    per table; `src/migrations/` one flat file per change, named
    `mYYYYMMDD_NNNNNN_<what>.rs` because filename order is application order.
    The migration is the schema. The connection is a process-wide static
    (`crate::db::db()`) installed by `db::connect_if_configured()` when
    `DATABASE_URL` exists; database-using code must check `db::is_connected()`
    when the application may start before its owner configures a connection.
    `db::migrate()` is never automatic.
19. A connection string is a credential. It lives in `DATABASE_URL` via an
    ignored `.env`, never in `rahti.config.json`, which records only which
    backend the project is on.
20. Authentication is a signed session cookie and a route policy.
    `rahti::auth` owns the cookie and the guard; the application owns
    `src/auth.rs` — which routes are private, how a password is checked, and
    which authorization abilities exist. `rahti::authorization` evaluates
    application-defined abilities and typed resource policies without
    inventing roles, permissions, users, or OAuth providers.
21. The auth policy is installed once from `initialize_application` with
    `rahti::auth::configure(...)`, before the router is built. Everywhere
    else reads `rahti::auth::settings()` — the installed policy.
22. `#[rpc(auth)]` is not a substitute for listing a route in
    `private_routes`, and listing the route is not a substitute for
    `#[rpc(auth)]`. The guard decides who may load a page; the attribute
    decides who may make the call.
23. Auth reads exactly three environment values: `AUTH_SECRET`,
    `AUTH_COOKIE_NAME`, and the optional `SESSION_LIFETIME_HOURS`. The first
    two are generated per project by `cargo rahti new` so no two applications
    share a signing key or a cookie name. `AUTH_SECRET` is a credential and
    lives in the ignored `.env`. The scaffold writes that one file and commits
    nothing in its place: what a clone supplies is documented, on the Rahti
    site and in `authentication.md`. A release build refuses to start on a
    placeholder secret such as `change-me`; in development a missing secret is
    invented and warned about. Do not add a fourth variable.
24. `rahti::load_env()` is the only `.env` reader in the process. Do not
    write a second copy.
25. The session payload is signed, not encrypted, and readable by whoever
    holds the cookie. Sign in an id and what the UI needs; never a hash, a
    token, or anything you would not show the user.
26. A scaffolded project's startup lives in its `src/lib.rs`, behind
    `initialize_application`, and returns a `StartupError` rather than
    exiting. `src/main.rs` binds the public listener. An optional native
    package binds a loopback one and calls the same function — Android has no
    `main` to put this in, and a packaged process that exits during startup is
    a program that did not open. `cargo-rahti` writes both files; do not
    collapse them back into one.
27. Native packaging is optional, additive, and **not in this workspace**. It
    is a separate package — `rahti-native` and `cargo-rahti-native` — that a
    project installs when it wants one. Nothing here may depend on Tauri, on
    an Android SDK, or on those crates. `cargo rahti native` reaches the tool
    by spawning it, which is why `cargo-rahti` gains no dependency from it.
28. `rahti` carries the pieces that package depends on: `begin_shutdown` (a
    packaged GUI never receives Ctrl+C), the default `dev-watch` feature (off,
    `notify` leaves a mobile build), and the scaffold's
    `initialize_application`. Removing or renaming one breaks a published
    package that is not in this repository to fail with it.
29. `rahti-build` runs generated Rust through rustfmt before writing it. The
    generated files are checked in and `cargo fmt --all -- --check` is a
    documented gate, so without it the formatter and the build undid each
    other on every run. If rustfmt is absent the text is written as codegen
    produced it, which still compiles.
30. MCP remains optional. A project with `"mcp": true` and the matching
    `rahti` cargo feature exposes discovery at `/.well-known/mcp.json`, public
    service context at `/llms.txt`, and bearer-protected Streamable HTTP at
    `/mcp/`; a project without the option has none of those generated MCP
    routes or application dependencies. Local stdio through `.mcp.json`
    remains available for opted-in development. Release HTTP access must fail
    closed when `RAHTI_MCP_AUTH_TOKEN` is absent.
31. Route metadata is server-resolved request state. Static `metadata()`
    values merge root layout to nested layout to page, and `Page::new`
    provides the dynamic request-specific override. Managed SEO tags belong
    to Rahti rather than page markup.
32. `cargo rahti build --static` renders the real application router. Fixed
    pages are automatic; dynamic pages export concrete URLs through async
    `static_paths()`. Static output is generated and ownership-guarded.
33. `rahti::cache` stores derived data, never authority. Page caching is an
    explicit `cache() -> PageCache` export on a public deterministic page and
    is limited to anonymous complete-document responses. Revalidate only
    after the authoritative write succeeds.
34. Deserialize owned input first, then apply `rahti::Validator` before side
    effects. Validation failures are HTTP 422; malformed RPC payloads remain
    HTTP 400, and database constraints remain authoritative under races.
35. Authentication and authorization never substitute for one another.
    Sensitive authenticated handlers call `authorize` or
    `authorize_resource`; unknown abilities and policy type mismatches fail
    closed. Presentation checks never replace handler enforcement.
36. The generated router applies security headers once around every response,
    with rate limiting and the application pipeline in their documented
    order. Applications customize policies during startup rather than editing
    generated code or bypassing framework boundaries.
37. Production observability owns request correlation, bounded events and
    metrics, plus liveness/readiness routes. Error handling preserves RPC
    shapes, uses Problem Details elsewhere, redacts private production
    diagnostics, and prevents failure responses from being cached.
38. Background jobs and schedules are bounded and process-local. They are not
    durable queues or distributed schedulers: applications must choose an
    external backend when work must survive a process or coordinate replicas.
39. Application calendar interpretation is installed once from the validated
    IANA `APP_TIMEZONE`. Local schedules and calendar queries use it; session
    expiry, cache TTLs, retries, and stored instants remain absolute/UTC.
40. Localization is opt-in through `rahti.config.json`. Catalogs live under
    the configured `i18n.dir` as `<locale>/**/*.ftl`, are validated and
    embedded at build time, and are read through request-task-local
    `rahti::i18n` state. A root `[locale]` or `[lang]` segment is authoritative
    and rejects unsupported values. Translation results are escaped text, not
    trusted markup; keep explicit locale prefixes in localized links and never
    replace a URL locale from `Accept-Language`.
41. A component script is plain browser JavaScript, so every web API is
    available to it directly: WebGPU (`navigator.gpu`), Canvas/WebGL, Web
    Workers, WebAssembly, Web Audio, WebRTC, IndexedDB, and the device APIs.
    PulsePoint wraps none of them; do not invent `pp.gpu` or similar. Server
    data arrives through `#[rpc]`/`pp.rpc`, displayed values go in
    `pp.state`, the device or worker handle goes in `pp.ref`, and a
    `pp.effect` acquires it, feeds it, and releases it in its cleanup. See
    "Native Web APIs" in `docs/conventions/pulsepoint.md`.

## Repository Map

- `crates/rahti/` - server runtime: HTML values, layouts, metadata, errors,
  RPC, streaming, uploads, authentication/authorization, caching, validation,
  security, rate limiting, request middleware, observability, jobs,
  scheduling, application time, localization, sockets, statics, live reload,
  and shutdown.
- `crates/rahti-macros/` - `html!`, `#[component]`, `#[rpc]`, `#[socket]`.
- `crates/rahti-build/` - configuration, filesystem scan, router/component
  generation, manifest, and CSS.
- `crates/cargo-rahti/` - `cargo rahti new` and `cargo rahti upgrade`, plus
  the scaffold assets under `assets/` (documents, templates, the PulsePoint
  bundle). `cargo rahti native` is forwarded from here to a separately
  installed `cargo-rahti-native`; the forwarding is a `Command` spawn and adds
  no dependency.
- `crates/rahti-mcp/` - read-only MCP server for generated project metadata,
  public site information, conventions, database artifacts, and development
  diagnostics; stdio is the default transport and Streamable HTTP is optional.
- `docs/conventions/` - the canonical convention documentation.
- `rahti.schema.json` - the JSON schema for `rahti.config.json`.

## Commands

Run from the repository root.

```text
cargo check --workspace
cargo test --workspace
cargo fmt --all -- --check
```

During implementation, run the narrow package or test first, then the
workspace suite before handing off a framework-wide change.

The scaffold CLI is exercised from this checkout with:

```text
cargo run -p cargo-rahti -- new <name> --local <path>
cargo run -p cargo-rahti -- upgrade --dry-run
```

`--local <path>` points the generated project's dependencies at this
checkout instead of crates.io — scaffold a throwaway app into a temporary
directory and `cargo run` it there to verify a change end to end.

## Documentation Maintenance

Documentation under `docs/conventions/` is written for coding agents and
maintainers. State rules, supported shapes, ownership, generated behavior,
and failure modes. Prefer a small verified example over a broad promise.

Write every convention document for a reader inside a **scaffolded
application**, not inside this repository. Teach each feature with an inline
example the reader can paste. `architecture.md` and `workspace-testing.md`
are the two exceptions — they describe this workspace and say so.
`testing.md` is the shipped counterpart of `workspace-testing.md`: it
teaches a scaffolded application's suite, and follows the same
no-workspace-references rule as every other shipped document.

Every convention document except `architecture.md` and
`workspace-testing.md` ships
into new applications through `cargo rahti new`, embedded from copies in
`crates/cargo-rahti/assets/docs/`. A cargo-rahti test holds each copy
byte-identical to its canonical file here, so after editing a shipped
document, copy it over its `assets/docs/` twin in the same change.

Document what Rahti implements today. Do not describe a planned feature, a
migration path, or compatibility with something Rahti never shipped; a reader
cannot tell an aspiration from a rule.

When adding a feature:

1. add or update implementation tests in the relevant crate;
2. update the applicable convention document;
3. update the feature inventory in `docs/conventions/architecture.md`;
4. update `rahti.schema.json` when configuration changes;
5. update scaffold assets/templates when new applications need the change.
