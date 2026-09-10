# Configuration, Assets, CSS, and Tooling

## Configuration Source

`rahti.config.json` is the project configuration and scaffold ledger.
`rahti.schema.json` documents and validates its shape.

```json
{
  "$schema": "./rahti.schema.json",
  "schema": 1,
  "createdWith": "0.0.19",
  "app": {
    "dir": "src/app",
    "public": "public"
  },
  "server": {
    "host": "0.0.0.0",
    "port": 3000
  },
  "mcp": true,
  "css": {
    "engine": "tailwind",
    "entry": "src/app/globals.css",
    "output": "public/css/styles.css"
  },
  "db": {
    "backend": "sqlite",
    "models": "src/models",
    "migrations": "src/migrations"
  },
  "scaffold": {}
}
```

- `schema` is the format version; a newer unsupported value stops the build.
- `createdWith` records scaffold provenance, not the active dependency version.
- `scaffold` maps scaffold-owned files to hashes for safe upgrades.
- `db` is absent unless the project has a database, and carries no connection
  string: that is a credential and this file is committed. See
  `database.md`.
- `mcp` is absent unless the project opted into MCP. When true, `rahti` carries
  its optional `mcp` Cargo feature, the generated router mounts `/mcp/` and
  `/.well-known/mcp.json`, and the scaffold writes `.mcp.json` plus an editable
  `public/llms.txt`. False or absent compiles and mounts none of it.
- The active Rahti version comes from `Cargo.toml`.

All operational keys default, but keep the file valid JSON and update the
schema whenever adding a key.

`server.host` and `server.port` are where the server binds. A deployment can
name its own address instead: `PORT` overrides the configured port, `HOST`
overrides the configured host, and both are read through `rahti::load_env()` —
so a `.env` sets them on your machine and the platform's environment sets them
in production. A managed platform assigns a fresh port on every deploy and
expects the process to read it, which is what makes a scaffolded project
deployable without editing `main.rs` or branching on the platform. A variable
exported empty counts as unset, and the configured value stands.

The scaffolded host is `0.0.0.0`, and deliberately so: it is the one address
that is correct in both places a project runs. A platform routes traffic into
the container from outside the process, so a server bound to the loopback
address accepts nothing and fails the platform's health check without ever
logging a refusal — the failure that costs an afternoon precisely because
nothing reports it. The wildcard develops just as well, so there is no second
value to remember at deploy time and `HOST` is an override rather than a
requirement.

What it costs is that a development server answers on the local network as well
as on this machine. On an untrusted network, or for a project that simply wants
the narrower default, write `"host": "127.0.0.1"` here — or set `HOST`, which
is the same override a deployment would use, applied the other way.

The startup line names where to open a browser, which is not always the address
that bound. `0.0.0.0` and `[::]` mean "every interface on this machine" — a
sentence about listening, not a destination — and a browser handed one refuses
it outright. So a wildcard bind prints as `localhost`, the same server named
the way this machine reaches it; any other address prints as it is. That is
`rahti::local_url(&listener)`, which the scaffold's `main.rs` calls. A project
whose `main.rs` predates it prints the bind address verbatim and shows an
unclickable `http://0.0.0.0:3000`; `cargo rahti upgrade --force src/main.rs`
takes the current one.

In development a busy port is not fatal: the server increments from the port it
was given until it finds a free one and prints the address it actually bound —
however that port was chosen. A release build treats the same conflict as an
error and stops, naming where the port came from, because a platform's router
or a reverse proxy points at that port and moving off it silently would break
the deployment without anything reporting it. A `PORT` that is not a number
from 0 to 65535 stops the server the same way.

## Static Files

Contents of the configured public directory are served from URL root:

```text
public/css/styles.css -> /css/styles.css
public/js/main.js     -> /js/main.js
public/favicon.ico    -> /favicon.ico
```

`RAHTI_PUBLIC_DIR` overrides the configured directory at runtime, the way
`PORT` and `HOST` override the configured address. A relative path resolves
against the working directory, which is why it is the variable a native
package sets: an installed application does not control where it was launched
from, and its assets have been staged into application storage. See
`native-packaging.md`.
`DATABASE_URL` is read by `src/db.rs` at startup, from the environment or from
an ignored `.env`; a real environment variable always wins over the file.
`AUTH_SECRET`, `AUTH_COOKIE_NAME`, and `SESSION_LIFETIME_HOURS` are read the
same way, by `rahti::auth` — see `authentication.md` for what each does. The
first two are generated per project by `cargo rahti new`; the third is
optional. In production a missing or placeholder `AUTH_SECRET` stops the server
rather than falling back. None of the three belongs in `rahti.config.json` —
the auth policy is code, in `src/auth.rs`, and the secret is a credential.

`rahti::load_env()` is the one `.env` reader for the process. It is called by
`AuthSettings::from_env` and by the application's `db::connect`, is idempotent,
and never overwrites a variable the environment already carries.

Explicit application routes win. Public files are the router fallback, and
the root 404 runs only when neither claims a path.

## PulsePoint Assets

The application entry `public/js/main.js`:

1. imports `/js/pp-reactive-v2.min.js`;
2. imports and publishes `twMerge`;
3. calls `pp.mount()` after DOM readiness.

`twMerge` is available to every mounted PulsePoint script as the global
function `twMerge(classes)`. Use it when a reactive class string can contain
conflicting Tailwind utilities; it returns the resolved string without changing
the input. It handles variants, `!important`, arbitrary values, and
overlapping utility groups.

```rust
<div class={twMerge("p-2 px-4")}></div>
```

The runtime is the shipped asset `public/js/pp-reactive-v2.min.js`, replaced
by `cargo rahti upgrade` when the framework ships a new build. Never
hand-edit it. Generated applications have no JavaScript build step.

## CSS Engines

The `css.engine` modes are:

- `tailwind` - compile the entry with the pinned standalone Tailwind CLI;
- `plain` - copy the entry as it stands;
- `none` - leave the output untouched.

The editable `entry` and served `output` keep the same roles in all modes.
The output is committed so builds remain usable without downloading a CLI.

Tailwind configuration:

- `version` pins the standalone release;
- `download` allows the build to fetch a missing binary;
- `minify` controls deterministic output style;
- `sha256` maps platform keys such as `windows-x64` to trusted hashes.

Binary lookup includes `.rahti/bin`, `RAHTI_TAILWIND`, and `tailwindcss` on
`PATH`. A configured hash mismatch is fatal.

Tailwind scans this project's own sources. Class names are authored in
`src/app/` and `src/components/`, which the build already watches, so a
changed class rebuilds the stylesheet.

## Development Runtime

Development reload is enabled in debug builds and disabled in release builds,
unless overridden by `RAHTI_DEV`. True-like values enable it; other values
disable it.

Rahti injects a dev client into page HTML, watches public files, swaps changed
stylesheets, reloads for other changes, and forwards browser warnings/errors
to `.rahti/dev.log`. Dev event streams listen to the framework shutdown signal
so Ctrl+C can complete graceful server shutdown.

The filesystem watcher is behind `rahti`'s default `dev-watch` cargo feature.
Every web project has it. It is a feature because a packaged native
application has no `public/` on disk to watch — its assets are staged into
internal storage once per version — and turning it off keeps `notify` out of
that build entirely rather than compiling it and not calling it. Nothing else
changes when it is off: the reload stream and the diagnostics endpoint are
unaffected.

A release native package additionally *sets* `RAHTI_DEV=0` rather than
inheriting it, because an installed application inherits the environment of
whoever launched it.

That covers everything read at runtime. A Rust edit needs a rebuild and a
restart, which nothing inside the running process can do, so the scaffold
writes a `.cargo/config.toml` whose `dev` alias hands the job to `cargo
watch`:

```text
cargo dev
```

It re-runs `cargo run` on every change under `src/` or to
`rahti.config.json`, and the open tab reloads itself when the new server
comes up. `public/` is deliberately not on the watch list — the running
server handles it without a recompile. The alias needs cargo-watch installed
once per machine: `cargo install cargo-watch`. Plain `cargo run` still works;
you restart it yourself after a Rust edit, and the browser reload half
behaves the same.

The `.rahti` directory is gitignored by both the repository and new-project
scaffold. It contains generated manifests, CSS scratch files, and the current
development diagnostic log; none are application input or shipped assets.

## Model Context Protocol

`rahti-mcp` is the read-only MCP server for a Rahti project. Choose it when
`cargo rahti new` asks, or pass `--mcp`; the scaffold records `"mcp": true`,
enables the optional application feature, writes `public/llms.txt`, and writes
this ledger-owned `.mcp.json` for local clients:

```json
{
  "mcpServers": {
    "rahti": {
      "command": "rahti-mcp",
      "args": ["--project", "."]
    }
  }
}
```

Install the stdio tool once per machine, then let a local MCP client start it
from the project root:

```text
cargo install rahti-mcp
rahti-mcp --project /path/to/project
```

It exposes project summaries, routes, components, RPCs, sockets, database
artifacts, bounded recent diagnostics, the generated manifest, public
configuration, and these convention documents. It validates generated file
references but never writes source or generated output.

The stdio process is local tooling, not an application request. It never reads
`.env`, never receives or reuses the browser's signed session cookie, and
cannot call an authenticated RPC or open an authenticated socket as the user.
It is authentication-aware only in the useful sense: it reports the generated
`#[rpc(auth)]` and `#[socket(auth)]` requirements. Private route policy remains
unknown to build tooling because the application installs that ordinary Rust
policy at runtime.

The opted-in application also mounts Streamable HTTP at `/mcp/`. `/mcp`
answers every supported method with a relative `307` redirect so an edge proxy
cannot downgrade HTTPS and a POST body is retained. `/.well-known/mcp.json`
advertises the relative endpoint and `/llms.txt` is public agent-facing context
the application owner edits with its services, constraints, and contact path.

Set `RAHTI_MCP_AUTH_TOKEN` to a distinct random bearer credential. A debug
build is open when it is absent for first-run development; a release build
fails the endpoint closed with `503`. Invalid or missing credentials receive
`401` and `WWW-Authenticate: Bearer realm="Rahti MCP"`. Public deployments
also set `RAHTI_MCP_ALLOWED_HOSTS` to a comma-separated hostname allowlist;
browser callers may set `RAHTI_MCP_ALLOWED_ORIGINS` to full origins. Neither
credential belongs in `rahti.config.json` or `public/llms.txt`.

## Scaffold CLI

```text
cargo rahti new <name> [--tailwind] [--db [backend]] [--ws] [--mcp] [--local <path>]
cargo rahti upgrade [--dry-run] [--force [path...]] [--db [backend]] [--ws] [--mcp]
```

Every feature flag adds something, and leaving it out is how you say no. One
spelling per answer, so there are no opposing flags to reconcile. An
interactive run asks about whatever was not named, defaulting to no; a run
with nowhere to ask takes the flags at their word.

- `--local` is for framework development and writes path dependencies.
- `--tailwind` compiles with the Tailwind CLI. Without it, plain CSS.
- `--db` takes `sqlite`, `postgres` or `mysql`; bare `--db` is `sqlite`. It
  adds SeaORM to `Cargo.toml`, writes `src/db.rs`, `src/models/` and
  `src/migrations/`, and records the backend in `rahti.config.json`.
- `--ws` puts `features = ["ws"]` on the `rahti` dependency — which is what
  compiles `rahti::ws` and the `#[socket]` attribute — and records
  `"ws": true` in `rahti.config.json`. See `websockets.md`.
- `--mcp` writes `.mcp.json` and `public/llms.txt`, records `"mcp": true`, and
  enables rahti's optional `mcp` feature. That feature mounts the secured HTTP
  endpoint; a project that leaves it out compiles no MCP application code.
  The separate stdio executable is still installed once per machine.
- `new` writes one `.env`, carrying this project's generated `AUTH_SECRET` and
  `AUTH_COOKIE_NAME` and, with `--db`, a `DATABASE_URL`. It is gitignored and
  untracked by the ledger, so no upgrade rewrites it, and nothing is committed
  in its place. What a clone has to supply is the environment reference at
  <https://rahti.tsnc.tech/docs/environment>; the variables
  themselves are covered in `authentication.md` and `database.md`.
- Every project is written with its own documentation: `AGENTS.md` — the
  project guide `CLAUDE.md` points at — and the convention documents under
  `docs/conventions/`, with `database.md` and `websockets.md` only where the
  feature was chosen. They are ledgered scaffold files, so `upgrade`
  refreshes unedited copies as the framework evolves and leaves edited ones
  alone.
- `upgrade` can add a database, WebSockets, or MCP to a project that started
  without them: `--db`/`--ws`/`--mcp` say yes outright, and an interactive run asks
  about whichever the project lacks, defaulting to no. A dry run never asks —
  name the feature to preview adding it. The feature's files are written and
  the choice is recorded in `rahti.config.json`. The CSS engine is not
  offered: switching it rewrites an authored stylesheet, which is a
  migration, not an addition.
- `upgrade` moves the `rahti` and `rahti-build` version pins to the version of
  the tool that is running, because the framework ships in lockstep and the
  files an upgrade writes are written against its own version. A pin left
  behind is the failure that hides best: the project compiles, runs locally on
  its configured port, and quietly does none of what its new `main.rs` says.
  Only ever forward, never onto a `--local` path dependency, and never over a
  requirement you wrote to float — a caret, a wildcard, a pre-release — which
  is you asking for something other than one exact version. A shape it cannot
  read is reported with the line rather than guessed at.
- `upgrade` never regenerates `Cargo.toml` or `.env` from a template — that
  would undo your dependencies, a `--local` path dependency, or a real
  credential — but it does amend them, by the smallest edit that makes the
  files it just wrote compile and run: the version pins above, `sea-orm` and
  `sea-orm-migration` appended to `[dependencies]`, `features = ["ws"]` folded
  into the `rahti` line, `DATABASE_URL` added above what is already in `.env`.
  Driven by `rahti.config.json` rather than by what this run
  added, so a project that gained a feature by editing its config is repaired
  by running `upgrade` again; every edit is skipped when it is already there,
  so running it any number of times is the same as running it once. A shape
  it cannot read — a `[dependencies.rahti]` table, a dependency spread over
  several lines — is reported with the line to add rather than guessed at.
- Upgrade rewrites/removes only files whose current hash still matches the
  scaffold ledger. Edited files are preserved and reported.
- `--force` takes edited files back anyway, which is how a project that
  diverged from the scaffold rejoins it. Bare, it takes the framework's own
  files — `src/main.rs`, `build.rs`, `.cargo/config.toml`, the documents, the
  PulsePoint assets — and leaves everything else under `src/`, because that is
  your application and a flag that deleted a homepage for saying "update my
  framework files" would be a trap. Naming paths takes exactly those, an
  application file included: `--force src/app/page.rs`. A path that is not a
  scaffold file of this project is refused rather than silently ignored, and
  `Cargo.toml` is refused either way — a forced manifest would undo your
  dependencies, which is the reason it is never rewritten in the first place.
  The replaced version is not kept anywhere, so pair it with `--dry-run`
  first; the preview names every file and prints back the exact command that
  applies it. A forced file whose text already matches the template is simply
  re-adopted into the ledger, so running it twice changes nothing the second
  time.
- In `rahti.config.json`, `upgrade` rewrites only what it owns: `createdWith`
  and the `scaffold` ledger. Everything else — the port, the app directory,
  any CSS setting — stays exactly as you wrote it.

When changing scaffold behavior, update templates/assets, hash-ledger behavior,
tests, and this document together.

## Native Packaging CLI

Optional, and a separate executable — which is what makes it optional. A
project that never runs it compiles, downloads and depends on no part of it.

```text
cargo install cargo-rahti-native

cargo rahti native init --identifier com.example.myapp --windows --android
cargo rahti native doctor [--target windows|android] [--release]
cargo rahti native dev   --target windows|android [--device <name>]
cargo rahti native build --target windows
cargo rahti native build --target android [--format apk|aab] [--debug]
```

`cargo rahti native …` is forwarded by `cargo-rahti` to the installed helper;
`cargo rahti-native …` reaches it directly through cargo's own subcommand
dispatch. When the helper is not installed, `cargo rahti native` prints the
line that installs it and installs nothing.

- `init` writes `rahti.native.json`, `rahti.native.schema.json` and the
  application-owned Tauri shell under `native/`. It uses the same hash-ledger
  rule as `upgrade`: a generated file whose hash still matches is regenerated,
  an edited one is left alone and reported, and `--force` takes it back.
  Editing the configuration and re-running is how the shell learns a new
  window size, version or target. `--local <path>` writes path dependencies,
  for framework development, and is recorded so a later run does not need it.
- `doctor` checks only what the named target needs — every configured target
  when none is named — and installs nothing. Each failure names what is
  missing, why it is needed, and the command that supplies it.
- `dev` and `build` run `doctor`'s checks first, then delegate to `cargo tauri`
  from inside `native/`. Signing is read from `RAHTI_NATIVE_*` environment
  variables and translated for each platform's tooling; nothing about it is in
  a committed file. `build` prints the absolute path of every artifact.

`native/` is excluded from the project workspace, so `cargo check --workspace`
in a web project never resolves a Tauri dependency. Full rules, paths and
security model: `native-packaging.md`.

## Generated Artifacts

- `src/routes.rs` - checked-in generated Axum router.
- `src/components/mod.rs` - checked-in generated module declarations.
- `src/models/mod.rs` - checked-in generated entity module declarations.
- `src/migrations/mod.rs` - checked-in generated migration modules and the
  `Migrator`, ordered by filename.
- `.rahti/manifest.json` - generated tooling description. Route entries keep
  RPC names in `rpcs` and authenticated ones in `authRpcs`; component RPC and
  socket entries carry an `auth` boolean. The `db` section names the backend,
  entities, and migrations.
- `public/css/styles.css` - generated/copied but intentionally checked in.

Run a Cargo build after changing route/component/configuration inputs and
review the resulting generated diff. Edit generators, never their output.
