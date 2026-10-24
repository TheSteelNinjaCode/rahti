# Workspace Testing Conventions

This document describes **this workspace's** test suite — the framework
crates — and is deliberately not shipped by `cargo rahti new`. The shipped
counterpart is `testing.md`, which teaches a scaffolded application's suite.

## Test Locations

- `crates/<crate>/src/tests/*.rs` - unit and module-level behavior, included
  from the crate/module using `#[path = "tests/..."]`.
- `crates/rahti/tests/router.rs` - the framework end to end, through a
  router built by hand the way codegen builds one, using only the public
  API.

The workspace has no JavaScript test suite; the browser runtime is a prebuilt
asset, tested in its own repository. Rahti's browser-test driver,
`rahti::browser`, is tested against a real headless browser in
`crates/rahti/src/tests/browser.rs` with
`cargo test -p rahti --features browser-test browser::`; those tests skip,
passing, on a machine with no Chrome, Chromium or Edge.

Follow the existing placement instead of creating ad hoc test binaries.

Behavior that only shows up in a full application — the generated router, the
auth guard in front of real routes, database RPCs — is verified by
scaffolding a throwaway project against this checkout and driving it:

```text
cargo run -p cargo-rahti -- new smoke --local <path-to-this-checkout>
```

A suite that drives a generated router must install the auth policy the way
an application's `main` does — `rahti::auth::configure(...)` — or it
exercises private routes that nothing protects.

## Verification by Change Type

### Compile-time browser code

Run `cargo test -p rahti-js` for the JavaScript compiler and RPC call
analysis, and `cargo test -p rahti-macros` for template integration.
MCP tests use a temporary project with a generated-contract inventory;
scaffold tests verify that applications receive the embedded browser assets.

### Rendering or macro change

Run:

```text
cargo test -p rahti-macros
cargo test -p rahti
```

Add parser/emitter tests for syntax and runtime HTML tests for escaping/value
semantics.

### Routing, generated code, configuration, or CSS change

Run:

```text
cargo test -p rahti-build
```

Scaffold a local project and inspect its regenerated `src/routes.rs` and
`.rahti/manifest.json`.

### Authentication change

Run:

```text
cargo test -p rahti --lib auth
cargo test -p rahti-macros
```

Cookie encoding and route matching are unit-tested in
`crates/rahti/src/tests/auth.rs`; the guard, the redirects, and
`#[rpc(auth)]` are driven through a scaffolded application's router.

### RPC, streaming, upload, CSRF, or errors

Run:

```text
cargo test -p rahti
```

Test success, malformed payloads, missing values, wrong types, limits,
serialization failures where possible, and boundary isolation.

### Database change

Run:

```text
cargo test -p rahti-build
cargo test -p cargo-rahti
```

Cover generated module wiring, migration ordering in the `Migrator`, and
config parsing and its refusals.

Use a *file* database in tests, not `:memory:`. Every `#[tokio::test]` builds
and drops its own runtime, and an in-memory database exists only while
something is connected to it — so it disappears with the first test's runtime
and the rest fail with `no such table`. The symptom is tests that pass alone
and fail together. See `database.md`.

### Native packaging change

Native packaging is a separate package — `rahti-native` and
`cargo-rahti-native` — with its own repository and its own tests. What is
verified *here* is only the framework side it depends on:

```text
cargo test -p rahti --lib shutdown
cargo test -p cargo-rahti native
cargo test -p cargo-rahti
```

`begin_shutdown`, the `dev-watch` feature and the scaffold's
`initialize_application` are a published package's API whether or not this
workspace calls them. Renaming one compiles cleanly here and breaks something
that is not in this repository to fail with it.

### Scaffold or upgrade change

Formatter changes also run `cargo test -p cargo-rahti`. Cover idempotence,
unchanged expressions and raw strings, rendered output, comments, Unicode,
CRLF, generated-file exclusions, and read-only `--check`. Exercise authored
sources in a scaffolded project with
`cargo run -p cargo-rahti -- fmt --check <application-path>/src`.

Run:

```text
cargo test -p cargo-rahti
```

Cover fresh generation, local/version dependencies, dry run, unchanged owned
files, edited files, retired files, asset copying, and each database backend's
cargo feature.

### PulsePoint runtime change

Install the new bundle at
`crates/cargo-rahti/assets/js/pp-reactive-v2.min.js`, then verify:

- a freshly scaffolded project's pages retain their bindings/scripts;
- `public/js/main.js` in that project still mounts the exposed global runtime;
- a deliberate browser warning/error reaches its `.rahti/dev.log`;
- Rust workspace tests remain green.

## Workspace Gate

For MCP changes, run `cargo test -p rahti-mcp --all-features` first. Its tests
use an isolated temporary Rahti project and cover the stdio server's advertised
capabilities, bounded diagnostics, generated RPC authentication metadata, HTTP
discovery, bearer protection, and the Streamable HTTP transport. Scaffold
integration belongs to `cargo-rahti` tests as well: they verify that `--mcp`
writes and ledgers `.mcp.json` and `public/llms.txt`, records and wires the
feature, provisions HTTP protection in `.env`, and leaves all MCP pieces
entirely absent when not selected.

Before completing a framework-wide change:

```text
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```

Do not “fix” unrelated user changes or generated diffs without tracing their
inputs. Document intentionally unrun checks and the reason.

## Documentation Coverage Gate

When a new feature changes an author-facing convention, update:

1. the relevant document in `docs/conventions/`;
2. the inventory in `architecture.md`;
3. `AGENTS.md` if the rule is non-negotiable or changes document routing;
4. `rahti.schema.json` if configuration changed.
