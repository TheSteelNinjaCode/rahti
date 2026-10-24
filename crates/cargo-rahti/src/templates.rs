//! What a new project is made of.
//!
//! The two engines produce the same project in all but three files: the
//! stylesheet, the client entry that would otherwise carry a Tailwind helper
//! nobody asked for, and the config that records the choice. Everything else
//! is byte-identical, which is the point — choosing plain CSS is a setting,
//! not a different framework.

use rahti_build::Backend;

/// The client runtime, embedded so `new` never needs the network.
pub const PP_RUNTIME: &[u8] = include_bytes!("../assets/js/pp-reactive-v2.min.js");

/// The runtime's type declarations for the application's own modules in
/// `public/js/`, generated from the same build as [`PP_RUNTIME`]: a module
/// that starts with `// @ts-check` is checked against the `pp` it calls.
pub const PP_TYPES: &str = include_str!("../assets/js/pp.d.ts");

/// Makes `public/js/` a checkable JavaScript project: `/js/…` imports resolve
/// to the files the server serves, `pp.d.ts` is in scope, and a module opts
/// in with `// @ts-check` (editors do it live; `npx tsc -p jsconfig.json`
/// checks from a terminal). Nothing is compiled or emitted — there is no
/// JavaScript build step.
pub const JSCONFIG: &str = r#"{
  "compilerOptions": {
    "allowJs": true,
    "checkJs": false,
    "noEmit": true,
    "strict": false,
    "strictNullChecks": true,
    "target": "es2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "lib": ["es2022", "dom", "dom.iterable"],
    "types": [],
    "paths": { "/js/*": ["./public/js/*"] }
  },
  "include": ["public/js/**/*.js", "public/js/**/*.mjs", "public/js/pp.d.ts"],
  "exclude": ["public/js/pp-reactive-v2.min.js", "public/js/tailwind-merge.mjs"]
}
"#;

/// The convention documents every new project carries in `docs/conventions/`.
///
/// Rahti is not in anyone's training data, so a project that leaves the
/// scaffold without its documentation leaves without the framework: the next
/// coding agent opened inside it has nothing to read and guesses. The copies
/// live in this crate's `assets/` for the same reason the client runtime
/// does — a published crate cannot read a file outside its own directory —
/// and a test holds each one byte-identical to its canonical original.
///
/// These twenty-four are unconditional. The two that depend on a scaffold choice —
/// `database.md` and `websockets.md` — are below, written only when the
/// choice was made: a guide that documents a feature the project does not
/// have teaches an agent to reach for it.
///
/// `native-packaging.md` is in the unconditional set even though `new` has no
/// flag for it, because it is not a scaffold choice: a project opts in later
/// by installing a separate tool, and the document is what tells the reader
/// that the option exists and what it costs. Withholding it would leave
/// somebody reaching for a Tauri tutorial and wiring the framework up by hand.
pub const CORE_DOCS: [(&str, &str); 28] = [
    ("routing.md", include_str!("../assets/docs/routing.md")),
    (
        "rendering-and-components.md",
        include_str!("../assets/docs/rendering-and-components.md"),
    ),
    (
        "metadata-and-seo.md",
        include_str!("../assets/docs/metadata-and-seo.md"),
    ),
    (
        "static-generation.md",
        include_str!("../assets/docs/static-generation.md"),
    ),
    ("caching.md", include_str!("../assets/docs/caching.md")),
    (
        "localization.md",
        include_str!("../assets/docs/localization.md"),
    ),
    (
        "validation.md",
        include_str!("../assets/docs/validation.md"),
    ),
    (
        "pulsepoint.md",
        include_str!("../assets/docs/pulsepoint.md"),
    ),
    (
        "pulsepoint-hooks.md",
        include_str!("../assets/docs/pulsepoint-hooks.md"),
    ),
    (
        "pulsepoint-performance.md",
        include_str!("../assets/docs/pulsepoint-performance.md"),
    ),
    (
        "pulsepoint-web-apis.md",
        include_str!("../assets/docs/pulsepoint-web-apis.md"),
    ),
    (
        "pulsepoint-navigation.md",
        include_str!("../assets/docs/pulsepoint-navigation.md"),
    ),
    (
        "diagnostics.md",
        include_str!("../assets/docs/diagnostics.md"),
    ),
    (
        "production-observability.md",
        include_str!("../assets/docs/production-observability.md"),
    ),
    (
        "background-jobs.md",
        include_str!("../assets/docs/background-jobs.md"),
    ),
    (
        "task-scheduling.md",
        include_str!("../assets/docs/task-scheduling.md"),
    ),
    (
        "application-timezone.md",
        include_str!("../assets/docs/application-timezone.md"),
    ),
    (
        "error-handling.md",
        include_str!("../assets/docs/error-handling.md"),
    ),
    (
        "rpc-and-uploads.md",
        include_str!("../assets/docs/rpc-and-uploads.md"),
    ),
    (
        "authentication.md",
        include_str!("../assets/docs/authentication.md"),
    ),
    (
        "authorization.md",
        include_str!("../assets/docs/authorization.md"),
    ),
    (
        "security-middleware.md",
        include_str!("../assets/docs/security-middleware.md"),
    ),
    (
        "rate-limiting.md",
        include_str!("../assets/docs/rate-limiting.md"),
    ),
    (
        "request-pipeline.md",
        include_str!("../assets/docs/request-pipeline.md"),
    ),
    ("security.md", include_str!("../assets/docs/security.md")),
    (
        "configuration-and-tooling.md",
        include_str!("../assets/docs/configuration-and-tooling.md"),
    ),
    (
        "native-packaging.md",
        include_str!("../assets/docs/native-packaging.md"),
    ),
    ("testing.md", include_str!("../assets/docs/testing.md")),
];

/// Shipped only when the project chose a database.
pub const DOC_DATABASE: &str = include_str!("../assets/docs/database.md");
/// Shipped only when the project chose WebSockets.
pub const DOC_WEBSOCKETS: &str = include_str!("../assets/docs/websockets.md");

/// `CLAUDE.md`, one line long on purpose: the guide is `AGENTS.md`, the name
/// agent tools agree on, and a second copy of its content here would only
/// ever drift from the first.
pub const CLAUDE_MD: &str = "@AGENTS.md\n";
/// `.cargo/config.toml`, whose one alias is the missing half of live reload.
///
/// The running server watches `public/` itself and needs no help there; a
/// Rust edit needs a rebuild and a restart, which nothing inside the process
/// can do. `cargo watch` is that outside thing, and the alias is what makes
/// `cargo dev` mean "run it the way this project wants" — without the file,
/// the command in the documentation does not exist in the project.
pub const CARGO_CONFIG: &str = r#"[alias]
# `cargo dev` — rebuild-and-restart, the half of live reload the server
# cannot do for itself. The running server already watches `public/` and
# swaps CSS without a recompile; editing Rust needs a rebuild and a
# restart, so `cargo watch` re-runs `cargo run` on every save and the
# browser tab reloads itself when the new server comes up.
#
# Needs cargo-watch installed once per machine:
#
#     cargo install cargo-watch
dev = "watch -w src -w rahti.config.json -x run"
"#;
/// MCP client configuration, written only when the project opts in.
///
/// The server is a separate installed developer tool rather than an
/// application dependency. Keeping the project path relative makes the
/// committed file work in every clone and from every project root.
pub const MCP_CONFIG: &str = r#"{
  "mcpServers": {
    "rahti": {
      "command": "rahti-mcp",
      "args": ["--project", "."]
    }
  }
}
"#;
/// Tailwind's class-merging helper. Written only for a Tailwind project;
/// 100 KB is a strange thing to serve an app that has no utility classes.
pub const TAILWIND_MERGE: &[u8] = include_bytes!("../assets/js/tailwind-merge.mjs");
pub const FAVICON: &[u8] = include_bytes!("../assets/favicon.ico");

pub fn cargo_toml(name: &str, rahti: &str, db: Option<Backend>, ws: bool, mcp: bool) -> String {
    // WebSockets are a feature of the `rahti` dependency, not a crate of
    // their own: the wire brings tokio-tungstenite with it, so a project
    // that never opens a socket should not compile it. The build dependency
    // below stays as it is — `rahti-build` has no such feature.
    let mut rahti_dep = rahti.to_string();
    if ws {
        rahti_dep = with_feature(&rahti_dep, "ws");
    }
    if mcp {
        rahti_dep = with_feature(&rahti_dep, "mcp");
    }

    // SeaORM is the application's dependency, not the framework's — `rahti`
    // does not depend on it and never sees it. The scaffold adds it here
    // because a project that asked for a database should compile on the first
    // `cargo run`, not after a trip to the documentation.
    let database = match db {
        Some(backend) => {
            let mut out = String::from(
                "\n# The database. Yours to manage from here on: an upgrade adds a \
                 dependency\n# a feature it just wrote needs, and changes nothing else \
                 in this file —\n# a version bump is a version bump.\n",
            );
            for (_, line) in sea_orm_dependencies(backend) {
                out.push_str(&line);
                out.push('\n');
            }
            out
        }
        None => String::new(),
    };

    format!(
        r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2024"

# The application as a library, and a binary that calls it.
#
# `src/lib.rs` holds `initialize_application` — the database, the migrations,
# the auth policy, the router — and `src/main.rs` binds the public listener.
# The split is what lets an optional native package start the same application:
# Android has no `main` to reach, it loads a library and calls into it. See
# docs/conventions/native-packaging.md.
[lib]
name = "{lib}"
path = "src/lib.rs"

[[bin]]
name = "{name}"
path = "src/main.rs"

# `native/` is the optional Tauri shell, written by `cargo rahti native init`.
# Excluded so that a web build never resolves a native dependency; it is its
# own workspace and is built from inside itself.
[workspace]
exclude = ["native"]

[dependencies]
axum = "0.8.9"
rahti = {rahti_dep}
serde = {{ version = "1", features = ["derive"] }}
tokio = {{ version = "1", features = ["full"] }}
tower-http = {{ version = "0.7", features = ["catch-panic", "fs"] }}
{database}
# What `src/tests/support.rs` needs to drive the generated router in-process:
# a request in, a response out, no port bound. See docs/conventions/testing.md.
[dev-dependencies]
{dev}
[build-dependencies]
rahti-build = {build}
"#,
        // Cargo's own rule for a library target's name: the package name with
        // its hyphens turned into underscores, because a crate name is a Rust
        // identifier. Written out rather than left implicit so that
        // `native/src/main.rs` can name it and be right.
        lib = name.replace('-', "_"),
        dev = DEV_DEPENDENCIES
            .iter()
            .map(|(_, line)| format!("{line}\n"))
            .collect::<String>(),
        build = rahti.replace("crates/rahti\"", "crates/rahti-build\"")
    )
}

/// The two dev-dependencies `src/tests/support.rs` drives the router with.
///
/// One list for both writers: the scaffold's manifest template above, and
/// `upgrade`'s wiring — which appends the same lines to a manifest the author
/// has edited, so a project scaffolded before there was a test suite still
/// compiles the one an upgrade just gave it.
pub const DEV_DEPENDENCIES: [(&str, &str); 2] = [
    ("http-body-util", "http-body-util = \"0.1\""),
    (
        "tower",
        "tower = { version = \"0.5\", features = [\"util\"] }",
    ),
];

/// The two lines a database costs the manifest, each with the name cargo
/// knows it by.
///
/// Returned as a list rather than one block because they are added
/// independently: `upgrade` amends a manifest that may already carry one of
/// them, and adding a duplicate key is a manifest cargo refuses to parse.
///
/// Pinned to the major version the scaffold writes and the convention
/// documentation describes. An unpinned dependency would take whatever is
/// newest, which is how a project ends up on an ORM its documentation does
/// not match.
pub fn sea_orm_dependencies(backend: Backend) -> [(&'static str, String); 2] {
    let feature = backend.feature();
    [
        (
            "sea-orm",
            format!(
                "sea-orm = {{ version = \"2\", default-features = false, \
                 features = [\"macros\", \"runtime-tokio-rustls\", \"{feature}\"] }}"
            ),
        ),
        (
            "sea-orm-migration",
            format!(
                "sea-orm-migration = {{ version = \"2\", default-features = false, \
                 features = [\"runtime-tokio-rustls\", \"{feature}\"] }}"
            ),
        ),
    ]
}

/// The same dependency, with `features = ["ws"]` folded in.
///
/// `rahti` arrives as either a bare version — `"0.0.7"` — or an inline table
/// with a path, and the feature list has to end up inside the table either
/// way: `{ version = "0.0.7", features = ["ws"] }`. A table that already names
/// features keeps them and gains one, because this also runs over a manifest
/// somebody has been editing: `upgrade` adds the feature to a project that
/// gained WebSockets after it was created.
pub(crate) fn with_ws_feature(rahti: &str) -> String {
    with_feature(rahti, "ws")
}

pub(crate) fn with_mcp_feature(rahti: &str) -> String {
    with_feature(rahti, "mcp")
}

fn with_feature(rahti: &str, feature: &str) -> String {
    let rahti = rahti.trim();

    // An existing list is joined rather than replaced. Found by the bracket
    // rather than by an exact `features = [`, so `features=[` is read too.
    if let Some(open) = features_list(rahti) {
        let rest = &rahti[open..];
        let separator = if rest.trim_start().starts_with(']') {
            ""
        } else {
            ", "
        };
        return format!("{}\"{feature}\"{separator}{}", &rahti[..open], rest);
    }

    match rahti.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        Some(inner) => format!("{{{}, features = [\"{feature}\"] }}", inner.trim_end()),
        None => format!("{{ version = {rahti}, features = [\"{feature}\"] }}"),
    }
}

/// The index just past the `[` of a `features = [...]` entry, if there is one.
fn features_list(value: &str) -> Option<usize> {
    let at = value.find("features")?;
    let equals = at + value[at..].find('=')?;
    let open = equals + value[equals..].find('[')?;
    // Only whitespace may sit between the three, or this is some other key
    // whose name happens to contain the word.
    value[at + "features".len()..equals]
        .trim()
        .is_empty()
        .then_some(open + 1)
}

pub const BUILD_RS: &str = r#"//! The app's build step is Rahti's build step.

fn main() {
    rahti_build::run();
}
"#;

pub fn gitignore(db: bool) -> String {
    // `/.env` is ignored whether or not there is a database: it carries
    // AUTH_SECRET either way, and a session signing key is a credential for
    // exactly the same reason a connection string is. Nothing is committed in
    // its place — what a clone has to fill in is documented, not scaffolded.
    let mut out = String::from(
        r#"/target
# Generated by `cargo rahti build --static`.
/dist
# Generated manifests, build scratch space, and development diagnostics.
# Nothing below .rahti is application input or shipped output.
/.rahti
# Credentials: the session signing secret, and the connection string if this
# project has a database. Every variable this file may hold is documented
# at https://rahti.tsnc.tech/docs/environment — a clone reads it there
# rather than from a committed copy.
/.env
"#,
    );

    if db {
        out.push_str(
            r#"
# A local SQLite file, if that is the backend you are on.
*.db
*.db-shm
*.db-wal
"#,
        );
    }

    out
}

/// The values `new` generates once and writes into `.env`.
///
/// Generated per project rather than fixed, so two Rahti applications never
/// ship the same signing key or fight over the same cookie name.
pub struct EnvValues {
    /// The real signing key.
    pub secret: String,
    /// The cookie's name. Not a credential — it is the project's identity
    /// under a shared parent domain.
    pub cookie: String,
    /// Bearer credential for the optional public MCP endpoint.
    pub mcp_token: String,
}

/// The example connection string for a backend — what the scaffold's `.env`
/// starts with, and what `upgrade` prints when a database is added to a
/// project whose `.env` it will not touch.
pub fn database_url(backend: Backend) -> &'static str {
    match backend {
        Backend::Sqlite => "sqlite://./app.db?mode=rwc",
        Backend::Postgres => "postgres://user:password@localhost:5432/app",
        Backend::MySql => "mysql://user:password@localhost:3306/app",
    }
}

/// The `DATABASE_URL` section of a `.env`, on its own.
///
/// Separate from [`env`] because `upgrade` writes only this part: a project
/// that gains a database already has a `.env` full of session settings, and
/// the connection string is added above them rather than the file replaced.
/// The trailing blank line is part of it — it is what separates the section
/// from whatever it is prepended to.
pub fn database_env(backend: Backend) -> String {
    format!(
        r#"# =============================================================================
# DATABASE
# Read by src/db.rs at startup. A connection string is a credential, which is
# why this file is git-ignored — rahti.config.json records which backend this
# project is on, and deliberately not how to reach it.
# =============================================================================

DATABASE_URL="{url}"

"#,
        url = database_url(backend)
    )
}

/// The project's `.env`.
///
/// Written once and git-ignored, because a signing secret in git is not a
/// secret. What every variable means, and what a clone has to fill in, is
/// the environment reference at <https://rahti.tsnc.tech/docs/environment>
/// — one account of it, kept beside the runtime that reads them.
pub fn env(backend: Option<Backend>, mcp: bool, values: &EnvValues) -> String {
    let mut out = String::new();

    if let Some(backend) = backend {
        out.push_str(&database_env(backend));
    }

    out.push_str(&format!(
        r#"# =============================================================================
# AUTHENTICATION AND SESSIONS
# Read by rahti::auth. Which routes are private lives in src/auth.rs, not here:
# a policy is code, and only the three values below are environment.
#
# This file is git-ignored, so a clone starts without it. What to put back
# is documented at https://rahti.tsnc.tech/docs/environment
# — authentication reads exactly these three names.
# =============================================================================

# Session signing secret. Unique and strong per app and per environment.
# Generated for this project; regenerate per environment with:
#     openssl rand -base64 32
# In production the app refuses to start when this is missing or left on a
# placeholder; in development it invents one and warns, so a fresh clone runs.
AUTH_SECRET="{secret}"

# Session cookie name. Generated per project: several apps under one parent
# domain that share a cookie name overwrite each other's sessions.
AUTH_COOKIE_NAME="{cookie}"

# Optional. Session lifetime in hours; the default is 1.
SESSION_LIFETIME_HOURS="{lifetime}"

# =============================================================================
# APPLICATION CALENDAR
# IANA timezone used by rahti::time and local task schedules. Stored instants,
# session expiry, cache TTLs, and retry delays remain UTC/absolute.
# =============================================================================

APP_TIMEZONE="UTC"

# =============================================================================
# SERVER OVERRIDES (OPTIONAL)
# Defaults come from rahti.config.json. Uncomment only to override them for
# this process; deployment platforms commonly provide PORT themselves.
# =============================================================================

# HOST="0.0.0.0"
# PORT="3000"

# =============================================================================
# PUBLIC ASSETS (OPTIONAL)
# Overrides app.public from rahti.config.json. Relative paths start at the
# process working directory.
# =============================================================================

# RAHTI_PUBLIC_DIR="public"

# =============================================================================
# RPC UPLOADS (OPTIONAL)
# Sizes are bytes. Defaults: 128 MiB total, no separate per-file limit, and
# 256 KiB in memory before spilling to the operating-system temporary folder.
# =============================================================================

# RAHTI_MAX_UPLOAD="134217728"
# RAHTI_MAX_FILE="0"
# RAHTI_SPILL_THRESHOLD="262144"
# RAHTI_SPILL_DIR="path/to/private/temp"

# =============================================================================
# DEVELOPMENT AND BUILD TOOLING (OPTIONAL)
# RAHTI_DEV overrides profile-based development mode. RAHTI_TAILWIND points
# rahti-build at a Tailwind CLI executable when it is not on PATH.
# =============================================================================

# RAHTI_DEV="1"
# RAHTI_TAILWIND="path/to/tailwindcss"
"#,
        secret = values.secret,
        cookie = values.cookie,
        lifetime = DEFAULT_LIFETIME_HOURS,
    ));

    if mcp {
        out.push_str(&format!(
            r#"

# =============================================================================
# MODEL CONTEXT PROTOCOL
# Protects the optional Streamable HTTP endpoint at /mcp/. Generate a distinct
# value for every environment and send it only to MCP clients you authorize.
# Production also sets RAHTI_MCP_ALLOWED_HOSTS to its public hostnames.
# =============================================================================

RAHTI_MCP_AUTH_TOKEN="{token}"
RAHTI_MCP_ALLOWED_HOSTS="localhost,127.0.0.1"
# Optional. Full origins allowed for browser-based MCP clients.
# RAHTI_MCP_ALLOWED_ORIGINS="http://localhost:3000"
"#,
            token = values.mcp_token,
        ));
    }

    out
}

impl EnvValues {
    /// A fresh pair, from the operating system's randomness.
    ///
    /// Called once per `cargo rahti new`, so two projects made on one machine
    /// one second apart still differ — which is the whole point, and the
    /// reason this is not derived from the project's name or the clock.
    pub fn generate() -> Self {
        EnvValues {
            // 32 bytes is the HMAC-SHA256 block the key feeds, written as
            // base64 so it survives a `.env` line.
            secret: base64(&random(32)),
            // A name, not a key: short, and hex so it is a legal cookie name
            // without quoting.
            cookie: hex(&random(8)),
            mcp_token: base64(&random(32)),
        }
    }
}

pub fn llms_txt(name: &str) -> String {
    format!(
        "# {name}\n\n> A Rahti application. Replace this paragraph with the public service summary an agent should know.\n\n- Home: /\n- MCP discovery: /.well-known/mcp.json\n- MCP endpoint: /mcp/\n\n## Services\n\nDescribe the services, ideal customers, deliverables, constraints, pricing approach, and contact path here. Keep secrets and private implementation details out of this public file.\n"
    )
}

fn random(bytes: usize) -> Vec<u8> {
    let mut buf = vec![0u8; bytes];
    if getrandom::fill(&mut buf).is_err() {
        // Only reachable if the OS has no entropy source at all. Writing a
        // predictable secret would be worse than refusing to write one.
        eprintln!(
            "rahti: cannot read random bytes from the operating system, so \
             this project's session secret cannot be generated."
        );
        std::process::exit(1);
    }
    buf
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Standard base64, written out rather than pulled in.
///
/// `cargo-rahti` needs this in exactly one place and does not otherwise
/// encode anything; a dependency to turn 32 bytes into 44 characters, in the
/// crate whose job is to be installable, is a poor trade.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);

        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The lifetime a new project starts on.
///
/// Longer than the framework's one-hour default, because a scaffold is a
/// development starting point and being signed out every hour while building
/// something is a poor first impression. It is written out rather than left
/// implicit so that changing it is an obvious edit to an obvious line.
const DEFAULT_LIFETIME_HOURS: u32 = 8;

/// `src/lib.rs` — the application, and everything that starts it.
///
/// The startup sequence is here rather than in `main` because a Rahti project
/// can have more than one host. The web binary is one. An optional native
/// package is another, and on Android it is not a `main` at all: the operating
/// system loads a library and calls into it, so a `main` that connected the
/// database and built the router could not be reached from there.
///
/// It is also why every step returns rather than exits. A packaged process
/// that calls `std::process::exit` during startup is, to the person who
/// double-clicked it, a program that did not open.
pub fn lib_rs(db: bool) -> String {
    let (modules, connect) = if db {
        (
            r#"
/// The database connection, and the two directories `rahti-build` wires up
/// from their contents: an entity per file, and a migration per file.
pub mod db;
pub mod migrations;
pub mod models;
"#,
            r#"
    // A database is optional until the deployment owner supplies DATABASE_URL.
    // A configured but unreachable database still stops startup: serving
    // requests while persistence is broken would hide a configuration error.
    db::connect_if_configured()
        .await
        .map_err(|message| StartupError::new("database", message))?;

    // Not automatic, and not the framework's to decide. Uncomment it to
    // migrate on boot, or leave it and run migrations deliberately — a server
    // that changes a schema because somebody restarted it is a surprise, and
    // the production version of that surprise is much worse.
    //
    // Whichever you choose, this line is where both hosts read it from: the
    // web binary and the native package call this same function, so the
    // policy cannot differ between them.
    //
    // db::migrate()
    //     .await
    //     .map_err(|message| StartupError::new("migrations", message))?;
"#,
        )
    } else {
        ("", "")
    };

    // Concatenated rather than formatted: the body below is Rust that uses
    // `format!` and closures of its own, and escaping every brace in it to
    // satisfy one placeholder makes a template nobody can read.
    let mut out = String::from(
        r#"//! The application, as a library.
//!
//! `initialize_application` is everything that turns this project into a
//! running server, and `src/main.rs` is a short binary that calls it and binds
//! the public address.
//!
//! The split is what lets an optional native package start the same
//! application — see docs/conventions/native-packaging.md. Nothing here is
//! specific to one: a web deployment that never packages anything is unchanged
//! by it.

/// Framework runtime, from the `rahti` crate.
pub use rahti;

/// Shared components. `mod.rs` is generated by `rahti-build` from the files
/// in `src/components/`, so the directory name is part of the contract.
mod components;
pub mod routes;
"#,
    );
    out.push_str(modules);
    out.push_str(
        r#"
/// This application's tests, grouped by area under `src/tests/` and driven
/// through the generated router. See docs/conventions/testing.md.
#[cfg(test)]
#[path = "tests/mod.rs"]
mod app_tests;

/// A started application: the router, and the address a web deployment binds
/// it on.
pub struct ApplicationRuntime {
    pub router: axum::Router,
    pub host: &'static str,
    pub port: u16,
}

/// Why the application could not start.
///
/// A value rather than a printed line, so a host with no terminal has
/// something to show.
#[derive(Debug)]
pub struct StartupError {
    /// Which part of startup failed.
    pub step: &'static str,
    /// What went wrong, phrased for a person.
    pub message: String,
}

impl StartupError {
    #[allow(dead_code)]
    fn new(step: &'static str, message: impl Into<String>) -> Self {
        StartupError { step, message: message.into() }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for StartupError {}

/// Start the application, without binding anything.
///
/// An address is the one part of starting a server the two hosts genuinely
/// disagree about, so it belongs to each of them: `main` binds the configured
/// public address, and a native package binds a loopback port the operating
/// system assigns.
pub async fn initialize_application() -> Result<ApplicationRuntime, StartupError> {"#,
    );
    out.push_str(
        r#"
    // Resolve the application calendar once. A misspelled IANA timezone stops
    // startup instead of silently changing reports and scheduled work.
    let app_time = rahti::time::AppTime::from_env()
        .map_err(|error| StartupError::new("timezone", error.to_string()))?;
    rahti::time::configure(app_time);
"#,
    );
    out.push_str(connect);
    out.push_str(
        r#"
    // Once this project has private routes or protected actions, install both
    // application-owned policies here. Write `src/auth.rs`, declare
    // `mod auth;` above, and uncomment the applicable lines:
    //
    // rahti::auth::configure(auth::settings());
    // rahti::authorization::configure(auth::authorization());
    //
    // Here rather than in `main` so that both hosts install the same one: a
    // native package calls this function too, and a packaged application whose
    // private pages were public would be a quiet thing to ship.

    Ok(ApplicationRuntime {
        router: routes::router(),
        host: routes::HOST,
        port: routes::PORT,
    })
}
"#,
    );
    out
}

/// `src/main.rs` — the web binary, and nothing else.
pub fn main_rs(name: &str) -> String {
    let lib = name.replace('-', "_");
    format!(
        r#"//! The web binary.
//!
//! Everything that turns this checkout into a running application is in
//! `src/lib.rs`, so that an optional native package can call it too. What is
//! left here is the part only a web deployment has: the public address, and a
//! terminal to report to.

use {lib}::{{initialize_application, rahti, routes}};

#[tokio::main]
async fn main() {{
    let app = match initialize_application().await {{
        Ok(app) => app,
        Err(e) => {{
            // A terminal wants exactly this. A window has nowhere to print,
            // which is why the error is returned rather than printed by the
            // function that produced it.
            eprintln!("rahti: the application could not start ({{}}): {{e}}", e.step);
            std::process::exit(1);
        }}
    }};

    // Static export runs this same startup and router without binding a
    // listener, so build-time HTML cannot drift from the live application.
    if let Some(output) = rahti::static_output() {{
        match rahti::export_static(
            app.router,
            routes::static_routes().await,
            rahti::public_dir(routes::PUBLIC_DIR),
            output,
        )
        .await
        {{
            Ok(report) => {{
                report.print();
                rahti::jobs::finish_shutdown().await;
                return;
            }}
            Err(error) => {{
                eprintln!("rahti: static export failed: {{error}}");
                std::process::exit(1);
            }}
        }}
    }}

    // The configured address, unless the environment names another: a
    // deployment sets `PORT` (and `HOST` if it needs one), so this line is
    // the same in development and in production. In dev mode a busy port
    // falls forward to the next free one — the line below names the address
    // that actually bound. In release a busy port is an error: whatever
    // routes traffic to this process points at the port it was given.
    let listener = rahti::listen(app.host, app.port).await;

    // Not the bind address verbatim: `0.0.0.0` is every interface, which is
    // not somewhere a browser can go.
    println!("Server running on {{}}", rahti::local_url(&listener));

    // Attach the transport peer to every request. The rate limiter uses this
    // address by default and ignores spoofable forwarding headers.
    axum::serve(
        listener,
        app.router
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
        .with_graceful_shutdown(async {{
            rahti::shutdown_signal().await;
            println!("\nShutting down...");
        }})
        .await
        .unwrap();

    // HTTP has stopped accepting work. Finish jobs already accepted during
    // the request drain, bounded by each queue's shutdown grace period.
    rahti::jobs::finish_shutdown().await;
    println!("Server stopped.");
}}
"#
    )
}

/// `src/db.rs` — the connection, and the whole of how one is reached.
///
/// A global rather than something handed to a handler, because Rahti handlers
/// take no state: `page()` and an `#[rpc]` are ordinary functions, and
/// threading a pool through them would put a parameter in every signature in
/// the application. `DatabaseConnection` is an internally-pooled handle that
/// is cheap to clone, so this is the shape SeaORM expects anyway.
pub const DB_RS: &str = r#"//! The database connection.
//!
//! One connection for the process, reached from anywhere by `db()`. Rahti
//! handlers take no state — `page()` and an `#[rpc]` are ordinary functions —
//! so a pool cannot arrive as an argument, and `DatabaseConnection` is an
//! internally-pooled handle anyway: cloning it is cheap and sharing it is the
//! intended use.
//!
//! This file is yours. If you want a pool size, a statement timeout, or a
//! read replica, this is where they go.

// A connection you have not queried yet is not dead code, and a new project
// has not queried it yet — the same reason `src/models/mod.rs` carries this.
#![allow(dead_code)]

use std::sync::OnceLock;

use sea_orm::{ConnectOptions, Database, DatabaseConnection};

static DB: OnceLock<DatabaseConnection> = OnceLock::new();

/// The connection, from a page, an rpc, or a route handler.
///
/// Panics if the process has not connected yet, which can only happen by
/// calling this before `main` reaches `connect()`. That is a wiring mistake
/// rather than a runtime condition, and a message naming it is worth more
/// than an `Option` every call site has to unwrap.
pub fn db() -> &'static DatabaseConnection {
    DB.get().expect(
        "the database has not been connected.\n  \
         `db::connect_if_configured().await` runs from `main` before the \
         server starts — \
         if you removed it, put it back.",
    )
}

/// Whether a connection has been installed for this process.
pub fn is_connected() -> bool {
    DB.get().is_some()
}

/// Connect once at startup when a database has been configured.
///
/// An absent `DATABASE_URL` leaves the application online without persistence,
/// so deployment owners can set their chosen connection string later. A present
/// but unusable URL remains an error instead of silently hiding a broken setup.
pub async fn connect_if_configured() -> Result<(), String> {
    // One `.env` reader for the process, in the framework — `AuthSettings`
    // needs the same file, and a project with no database still has one.
    // Idempotent, so neither caller has to know which ran first, and a real
    // environment variable still wins over the file.
    rahti::load_env();

    let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(());
    };

    connect_to(&url).await.map_err(|e| {
        format!(
            "cannot connect to the database: {e}\n  \
             Check DATABASE_URL, and that the server is running."
        )
    })
}

/// Connect to a named database instead of DATABASE_URL.
///
/// This is how a test gets one. SQLite needs no server, so a whole suite can
/// run against a real engine:
///
/// ```ignore
/// # async fn example() {
/// let path = std::env::temp_dir().join("my-app-tests.db");
/// let _ = std::fs::remove_file(&path);
///
/// crate::db::connect_to(&format!("sqlite://{}?mode=rwc", path.display()))
///     .await
///     .unwrap();
/// crate::db::migrate().await.unwrap();
/// # }
/// ```
///
/// Use a file, not `:memory:`. The connection here is a process-wide static,
/// but every `#[tokio::test]` builds its own runtime and drops it at the end
/// of the test — so the pool that opened the database belongs to a runtime
/// that is gone by the time the next test runs. An in-memory database exists
/// only while something is connected to it, so it goes with that runtime and
/// every later test fails with `no such table` on a table the migration
/// certainly created. A file survives being reconnected to.
///
/// Delete it on the way in rather than on the way out: a test binary has no
/// reliable teardown, and starting from nothing is what makes a run
/// repeatable.
///
/// Connecting a second time is a no-op rather than an error. The connection
/// belongs to the process, and tests run in parallel within one.
pub async fn connect_to(url: &str) -> Result<(), sea_orm::DbErr> {
    if DB.get().is_some() {
        return Ok(());
    }

    let mut options = ConnectOptions::new(url.to_string());
    options.sqlx_logging(false);

    // Keep one connection open for the life of the process.
    //
    // A warm connection is a small win on any backend and the difference
    // between working and not on an in-memory SQLite: that database exists
    // only as long as something is connected to it, so a pool that drains to
    // zero between two queries takes the schema with it. The symptom is `no
    // such table` on a table the migration definitely created.
    options.min_connections(1);

    let connection = Database::connect(options).await?;
    let _ = DB.set(connection);
    Ok(())
}

/// Apply every pending migration.
///
/// Deliberately not called by `connect`. A framework that changes your schema
/// because you started the server is the same surprise as one that downloads
/// a compiler you did not ask for — and the version of that surprise which
/// happens in production is much worse. Call it from a `main` branch behind
/// an argument, or run it from a test, when you want it.
///
/// ```ignore
/// # async fn example() {
/// crate::db::migrate().await.unwrap();
/// # }
/// ```
pub async fn migrate() -> Result<(), String> {
    use sea_orm_migration::MigratorTrait;

    crate::migrations::Migrator::up(db(), None)
        .await
        .map_err(|e| format!("a migration failed: {e}"))
}
"#;

/// The one entity a new project has: the table its `/todo` page reads.
///
/// Written out in full rather than generated from the database, because the
/// point of the file is to be the worked example everything else — including
/// an agent asked to add a table — is copied from. Every derive it carries is
/// there for a reason the comments name.
pub const MODEL_TODO_RS: &str = r#"//! The `todo` table.
//!
//! One file per table, named after it in snake_case. `src/models/mod.rs` is
//! generated from this directory, so saving the file is the whole of adding
//! the entity — there is no list to register it in.
//!
//! Copy this file to add a table. The four pieces are always the same:
//!
//! 1. `Model` — the columns, as ordinary Rust fields.
//! 2. `Relation` — what this table points at. Empty is fine and common.
//! 3. `ActiveModelBehavior` — hooks. The empty impl is required.
//! 4. a migration in `src/migrations/`, which is what actually creates it.
//!
//! The struct does not create the table. Nothing here is read at startup and
//! nothing is checked against the database: an entity that disagrees with the
//! schema compiles fine and fails on the first query. The migration is the
//! schema; this is how Rust talks to it.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// `Serialize` so an `#[rpc]` can return a row as it stands, and the page can
/// seed a list into its script with `Json(&todos)`. `Deserialize` so the same
/// shape round-trips back.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "todo")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub text: String,
    pub done: bool,
}

/// What this table points at. A table with no foreign keys has none, and the
/// empty enum is how that is written.
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

/// Hooks for insert and update. Required even when empty — this is where a
/// `created_at` would be filled in.
impl ActiveModelBehavior for ActiveModel {}
"#;

/// The migration that creates that table.
///
/// Named for the moment it was written, because `src/migrations/mod.rs` lists
/// this directory in filename order and that order is the order they are
/// applied.
pub const MIGRATION_TODO: &str = r#"//! Create the `todo` table.
//!
//! The file name is the version: `src/migrations/mod.rs` is generated from
//! this directory in filename order, and that is the order migrations are
//! applied. So a new migration is named for the moment it was written, and
//! adding the file is the whole of adding the migration.
//!
//! `up` makes the change and `down` undoes it. Write both — a `down` you
//! never run costs a minute, and the one time you need it you need it badly.

use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Todo::Table)
                    .if_not_exists()
                    .col(pk_auto(Todo::Id))
                    .col(string(Todo::Text))
                    .col(boolean(Todo::Done).default(false))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Todo::Table).to_owned())
            .await
    }
}

/// The column names, as the query builder needs them. Kept beside the
/// migration rather than shared with the entity on purpose: this describes the
/// table as it was at this version, and a later migration that renames a
/// column must not change what an earlier one did.
#[derive(DeriveIden)]
enum Todo {
    Table,
    Id,
    Text,
    Done,
}
"#;

pub fn layout_rs(title: &str) -> String {
    format!(
        r###"use crate::rahti::{{Html, Metadata, html}};

/// Site-wide metadata defaults. A nested layout or page only repeats the
/// fields it needs to override.
pub fn metadata() -> Metadata {{
    Metadata::new()
        .default_title("{title}")
        .title_template("{{title}} | {title}")
        .description("{title}, built with Rahti")
}}

/// The root layout: the document shell every page renders inside.
///
/// `children` is the page — or the next layout down — already rendered.
/// `<slot />` is where it lands.
///
/// The slot is also where Rahti puts this layout's identity. `html!` names
/// every convention file for the client runtime, and the single parent here is
/// `<html>` — naming that would make the whole document a component to
/// re-render, `<head>` and all. So a document is named around its slot
/// instead: the rendered page carries `<template pp-component="…">` there,
/// derived from this file's path. Nothing to write, and nothing to keep in
/// step.
pub fn layout(children: Html) -> Html {{
    html! {{
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="UTF-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                <link rel="icon" href="/favicon.ico" type="image/x-icon" />
                <link href="/css/styles.css" rel="stylesheet" />
                <script type="module" src="/js/main.js"></script>
            </head>
            <body>
                <slot />
            </body>
        </html>
    }}
}}
"###
    )
}

/// The one page a new project has, in the classes its engine can style.
pub fn page_rs(name: &str, tailwind: bool) -> String {
    let (section, heading, lede, button) = if tailwind {
        (
            "mx-auto w-full max-w-2xl px-6 py-16",
            "text-3xl font-bold tracking-tight text-gray-900 dark:text-gray-50",
            "mt-3 text-sm text-gray-600 dark:text-gray-400",
            "mt-6 inline-flex items-center rounded-lg bg-indigo-600 px-4 py-2 \
             text-sm font-medium text-white transition-colors hover:bg-indigo-700",
        )
    } else {
        ("page", "title", "lede", "button")
    };

    format!(
        r###"use crate::rahti::{{Html, html, rpc}};

/// The home page. Every `page.rs` under `src/app/` becomes a URL by where it
/// sits, and this one sits at the root.
pub async fn page() -> Html {{
    html! {{
        <section class="{section}">
            <h1 class="{heading}">"{name}"</h1>
            <p class="{lede}">"Edit src/app/page.rs and reload."</p>

            <button class="{button}" onclick={{greet()}}>"Say hello"</button>
            <p class="{lede}">{{message}}</p>

            <script>r##"
                const [message, setMessage] = pp.state("");

                async function greet() {{
                    setMessage(await pp.rpc("hello", {{}}));
                }}
            "##</script>
        </section>
    }}
}}

/// Called by `pp.rpc("hello")` above. It runs on the server, in Rust, and the
/// browser never learns there was a function here at all.
#[rpc]
pub async fn hello() -> String {{
    "Hello from Rust.".to_string()
}}
"###
    )
}

/// `src/tests/mod.rs` — one collector for the application's test areas.
pub fn tests_mod_rs(db: bool) -> String {
    let database = if db { "mod database;\n" } else { "" };

    format!(
        r#"//! This application, driven through its generated router.
//!
//! Add one module per cohesive feature area, not one file per test. Shared
//! setup and request helpers stay in `support.rs`. See
//! docs/conventions/testing.md.

mod support;
use support::*;

{database}mod pages;
mod rpc;
"#
    )
}

/// `src/tests/support.rs` — setup and request helpers shared by test areas.
///
/// Concatenated rather than one `format!`, like `lib_rs` and for the same
/// reason: the body is Rust that uses `format!` and braces of its own.
pub fn test_support_rs(name: &str, db: bool) -> String {
    let mut out = String::from(
        r#"//! Shared infrastructure for the application test areas.
//!
//! No server starts and no port binds: `oneshot` drives the same
//! `axum::Router` the binary serves.

use axum::body::Body;
use axum::http::Request;
pub(super) use axum::http::StatusCode;
use http_body_util::BodyExt;
use tower::ServiceExt;

/// Everything the suite installs once, before the first request.
///
/// Called from the request helpers rather than from each test, so a test
/// cannot forget it — and guarded, so the first request performs it and the
/// rest wait.
///
/// Once this project has private routes, install the auth policy here the
/// way `initialize_application` does — the guard the generated router
/// installs reads that policy on every request, and a suite that skips the
/// line exercises private routes that nothing protects:
///
/// ```ignore
/// rahti::auth::configure(crate::auth::settings());
/// rahti::authorization::configure(crate::auth::authorization());
/// ```
pub(super) async fn setup() {
    static READY: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

    READY
        .get_or_init(|| async {
"#,
    );

    if db {
        out.push_str(&format!(
            r#"            // A real database for the whole test binary: SQLite in a
            // file, with this application's own migrations run against it.
            // A file rather than `:memory:`, deleted on the way in —
            // docs/conventions/testing.md explains why both halves matter.
            let path = std::env::temp_dir().join("{name}-tests.db");
            let _ = std::fs::remove_file(&path);

            let url = format!(
                "sqlite://{{}}?mode=rwc",
                path.display().to_string().replace('\\', "/")
            );
            crate::db::connect_to(&url).await.expect("a test database");
            crate::db::migrate().await.expect("the migrations");
"#
        ));
    } else {
        out.push_str(
            r#"            // Nothing to install yet. The auth policy described above is
            // the first thing that will land here.
"#,
        );
    }

    out.push_str(
        r#"        })
        .await;
}

/// GET a URL through the real router.
pub(super) async fn get(url: &str) -> (StatusCode, String) {
    setup().await;

    let res = crate::routes::router()
        .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
        .await
        .expect("the router answered");

    let status = res.status();
    let bytes = res.into_body().collect().await.expect("a body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// Call an `#[rpc]` the way the browser does: load its page, keep the CSRF
/// cookie the response hands out, then POST back to the same URL with the
/// function's name in `x-pp-function` and the token sent both ways.
pub(super) async fn rpc(
    url: &str,
    name: &str,
    payload: &str,
) -> (StatusCode, String) {
    setup().await;

    let router = crate::routes::router();

    let visit = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(url)
                .header("host", "127.0.0.1:3000")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("the page answered");

    let cookie = visit
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .expect("the page hands out a csrf token")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let res = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(url)
                .header("host", "127.0.0.1:3000")
                .header("x-pp-function", name)
                .header("content-type", "application/json")
                .header("cookie", &cookie)
                .header("x-csrf-token", cookie.split_once('=').unwrap().1)
                .body(Body::from(payload.to_string()))
                .unwrap(),
        )
        .await
        .expect("the rpc answered");

    let status = res.status();
    let bytes = res.into_body().collect().await.expect("a body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

"#,
    );

    out
}

/// `src/tests/pages.rs` — the first page and routing tests.
pub fn page_tests_rs(name: &str) -> String {
    format!(
        r#"use super::*;

/// The scaffolded home page, inside the root layout's document.
#[tokio::test]
async fn the_home_page_renders() {{
    let (status, body) = get("/").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<!DOCTYPE html>"), "{{body}}");
    assert!(body.contains("{name}"), "{{body}}");
}}

/// A URL no `page.rs` claims: the 404 page, not a panic.
#[tokio::test]
async fn an_unknown_url_is_a_404() {{
    let (status, _) = get("/this-page-does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}}
"#
    )
}

/// `src/tests/rpc.rs` — the first RPC tests.
pub fn rpc_tests_rs() -> String {
    String::from(
        r#"use super::*;

/// `pp.rpc("hello", {})` from the scaffolded page, end to end: the CSRF
/// pair accepted, the function found, the return value serialized.
#[tokio::test]
async fn the_hello_rpc_answers() {
    let (status, body) = rpc("/", "hello", "{}").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Hello from Rust."), "{body}");
}

/// A name the page does not declare is refused by name.
#[tokio::test]
async fn an_unknown_rpc_name_is_refused() {
    let (status, _) = rpc("/", "no_such_function", "{}").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
"#,
    )
}

/// `src/tests/database.rs` — generated only when the project has a database.
pub fn database_tests_rs() -> String {
    String::from(
        r#"use super::*;

/// The migrations that ran in `setup` created a real schema: a row goes in
/// through the entity and comes back. A disagreement between the entity and
/// its migration fails here, on the first query.
#[tokio::test]
async fn the_todo_table_round_trips() {
    use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};

    setup().await;

    let saved = crate::models::todo::ActiveModel {
        text: Set("write a test".to_string()),
        done: Set(false),
        ..Default::default()
    }
    .insert(crate::db::db())
    .await
    .expect("the insert");

    let found = crate::models::todo::Entity::find_by_id(saved.id)
        .one(crate::db::db())
        .await
        .expect("the query")
        .expect("the row");

    assert_eq!(found.text, "write a test");
}
"#,
    )
}

/// `main.js`, with the Tailwind helper only where there is Tailwind.
pub fn main_js(tailwind: bool) -> String {
    let mut out = String::from(
        "import \"/js/pp-reactive-v2.min.js\";\n\
         // The application's client code, compiled ahead of time. PulsePoint runs\n\
         // nothing else, so it is registered before the first mount.\n\
         import \"/_rahti/client.js\";\n",
    );

    if tailwind {
        out.push_str("import { twMerge } from \"/js/tailwind-merge.mjs\";\n");
    }

    out.push_str("\nconst pp = (globalThis).pp;\n");

    if tailwind {
        out.push_str("\nglobalThis.twMerge = twMerge;\n");
    }

    out.push_str(
        r#"
// Same-root navigation replaces the body rather than the document. The
// server carries the resolved managed head in a body template so every SEO
// field follows the page, not only its title.
function syncMetadata() {
  const source = document.getElementById("rahti-metadata");
  if (!(source instanceof HTMLTemplateElement)) return;

  document.head
    .querySelectorAll("[data-rahti-metadata]")
    .forEach((element) => element.remove());
  document.head.append(source.content.cloneNode(true));
}

document.addEventListener("pp:navigation:complete", syncMetadata);

if (document.readyState !== "loading") {
  pp?.mount?.();
} else {
  document.addEventListener(
    "DOMContentLoaded",
    () => pp?.mount?.(),
    { once: true },
  );
}
"#,
    );

    out
}

pub const GLOBALS_TAILWIND: &str = r#"@import "tailwindcss" source(none);
@source "../";

:root {
  --background: #ffffff;
  --foreground: #171717;
}

@theme inline {
  --color-background: var(--background);
  --color-foreground: var(--foreground);
}

@media (prefers-color-scheme: dark) {
  :root {
    --background: #0a0a0a;
    --foreground: #ededed;
  }
}

body {
  background: var(--background);
  color: var(--foreground);
  font-family: system-ui, sans-serif;
}
"#;

/// The plain starter: ordinary CSS, and the classes `page.rs` actually uses.
///
/// Deliberately a real starting point rather than an empty file. An engine
/// whose first impression is an unstyled page teaches the wrong thing about
/// what choosing it costs.
pub const GLOBALS_PLAIN: &str = r#"/* Your stylesheet. `css.engine` is "plain", so this file is published to the
 * browser as it stands. Nothing is downloaded and nothing is compiled. */

:root {
  --background: #ffffff;
  --foreground: #171717;
  --muted: #52525b;
}

@media (prefers-color-scheme: dark) {
  :root {
    --background: #0a0a0a;
    --foreground: #ededed;
    --muted: #a1a1aa;
  }
}

body {
  margin: 0;
  background: var(--background);
  color: var(--foreground);
  font-family: system-ui, sans-serif;
}

.page {
  max-width: 42rem;
  margin: 0 auto;
  padding: 4rem 1.5rem;
}

.title {
  font-size: 1.875rem;
  font-weight: 700;
  letter-spacing: -0.025em;
  margin: 0;
}

.lede {
  margin-top: 0.75rem;
  font-size: 0.875rem;
  color: var(--muted);
}

.button {
  margin-top: 1.5rem;
  display: inline-flex;
  align-items: center;
  border-radius: 0.5rem;
  border: 0;
  padding: 0.5rem 1rem;
  font-size: 0.875rem;
  font-weight: 500;
  color: #ffffff;
  background-color: #4f46e5;
  cursor: pointer;
  transition: background-color 150ms;
}

.button:hover {
  background-color: #4338ca;
}
"#;

/// `AGENTS.md` — the guide a coding agent reads before its first edit.
///
/// Built rather than pasted because it must only ever name documents and
/// files this project actually has: a table row pointing at a
/// `websockets.md` the scaffold did not write sends the reader searching for
/// something their checkout does not contain, and they conclude the guide is
/// wrong. The rules are numbered here, in code, so a conditional rule slots
/// in without leaving a gap in the list.
///
/// Concatenated rather than formatted for the same reason `main_rs` is: the
/// body is full of `@{…}` and `{…}` braces, and escaping every one to
/// satisfy a placeholder makes a template nobody can read.
pub fn agents_md(name: &str, db: bool, ws: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {name} — Agent Guide\n\n"));
    out.push_str(
        "This is a Rahti application: a server-rendered Rust web framework with\n\
         file-system routing under `src/app/`, typed HTML through `html!`, and\n\
         browser reactivity supplied by the bundled PulsePoint runtime. This file\n\
         is written for coding agents and the people working beside them. Read it\n\
         first, and read the matching convention document before working in its\n\
         area.\n\n\
         This file is the scaffold's until you edit it: `cargo rahti upgrade`\n\
         refreshes an unedited copy as the framework evolves, and leaves an\n\
         edited one alone.\n\n",
    );

    out.push_str(
        "## RPC work for agents\n\n\
         Run `cargo check` after changing RPC declarations. Read `rpcContracts`\n\
         in `.rahti/manifest.json`, or optional MCP `list_rpcs`, before writing\n\
         the caller. Match scope, file and URLs; inspect parameters, response\n\
         types and checking limits. `schemaStatus: \"unresolved\"` requires\n\
         reading the Rust/Serde type. Use `pp.rpc` beside its `#[rpc]`; run\n\
         compiler and router/browser checks. Dynamic values and response-field\n\
         accesses are not proved by compilation. See `docs/conventions/rpc-and-uploads.md`.\n\n",
    );

    out.push_str("## Required Reading\n\n| Work area | Document |\n| --- | --- |\n");
    out.push_str(
        "| Pages, layouts, route groups, dynamic routes, APIs, errors, 404s | \
         `docs/conventions/routing.md` |\n",
    );
    out.push_str(
        "| `html!`, interpolation, escaping, components, children | \
         `docs/conventions/rendering-and-components.md` |\n",
    );
    out.push_str(
        "| Page titles, descriptions, social cards, canonical URLs, robots | \
         `docs/conventions/metadata-and-seo.md` |\n",
    );
    out.push_str(
        "| Static HTML builds, dynamic export paths, output safety | \
         `docs/conventions/static-generation.md` |\n",
    );
    out.push_str(
        "| Cache stores, page caching, revalidation, backend adapters | \
         `docs/conventions/caching.md` |\n",
    );
    out.push_str(
        "| Locales, Fluent catalogs, fallback, localized routes, language direction | \
         `docs/conventions/localization.md` |\n",
    );
    out.push_str(
        "| Typed input rules, field errors, HTTP 422 validation failures | \
         `docs/conventions/validation.md` |\n",
    );
    out.push_str(
        "| PulsePoint markup: not JSX, the closed directive and API list, bindings, \
         events, forms, conditionals, lists, props | \
         `docs/conventions/pulsepoint.md` |\n",
    );
    out.push_str(
        "| Every `pp.*` hook with an example: `pp.event`, effects, context, refs, \
         portals, error boundaries, external stores | \
         `docs/conventions/pulsepoint-hooks.md` |\n",
    );
    out.push_str(
        "| Slow inputs or lists, re-renders, keyed rows, identity, splitting a large \
         page | `docs/conventions/pulsepoint-performance.md` |\n",
    );
    out.push_str(
        "| WebGPU, Canvas/WebGL, workers, WebAssembly, media, third-party libraries, \
         games and widgets (DOM ownership) | \
         `docs/conventions/pulsepoint-web-apis.md` |\n",
    );
    out.push_str(
        "| SPA navigation, scroll, loading regions, `pp.redirect` | \
         `docs/conventions/pulsepoint-navigation.md` |\n",
    );
    out.push_str(
        "| Browser/backend development diagnostics and `.rahti/dev.log` | \
         `docs/conventions/diagnostics.md` |\n",
    );
    out.push_str(
        "| `#[rpc]`, errors, component RPCs, streams, files | \
         `docs/conventions/rpc-and-uploads.md` |\n",
    );
    if ws {
        out.push_str(
            "| WebSockets: `#[socket]`, `pp.socket` | \
             `docs/conventions/websockets.md` |\n",
        );
    }
    out.push_str(
        "| Sessions, route protection, `#[rpc(auth)]` | \
         `docs/conventions/authentication.md` |\n",
    );
    out.push_str(
        "| Abilities, resource policies, 401/403 decisions | \
         `docs/conventions/authorization.md` |\n",
    );
    out.push_str(
        "| CSP, browser security headers, HSTS, policy overrides | \
         `docs/conventions/security-middleware.md` |\n",
    );
    out.push_str(
        "| Request budgets, client IPs, proxy trust, HTTP 429 | \
         `docs/conventions/rate-limiting.md` |\n",
    );
    out.push_str(
        "| Application middleware, request/response order, short circuits | \
         `docs/conventions/request-pipeline.md` |\n",
    );
    out.push_str(
        "| Request IDs, JSON logs, metrics, liveness and readiness | \
         `docs/conventions/production-observability.md` |\n",
    );
    out.push_str(
        "| Background jobs, retries, queue bounds, graceful drain | \
         `docs/conventions/background-jobs.md` |\n",
    );
    out.push_str(
        "| Intervals, local/UTC daily and one-shot task dispatch | \
         `docs/conventions/task-scheduling.md` |\n",
    );
    out.push_str(
        "| Application timezone, UTC storage, calendar-day bounds | \
         `docs/conventions/application-timezone.md` |\n",
    );
    out.push_str(
        "| Error privacy, page boundaries, Problem Details, RPC failures | \
         `docs/conventions/error-handling.md` |\n",
    );
    if db {
        out.push_str(
            "| SeaORM entities, migrations, the connection, database tests | \
             `docs/conventions/database.md` |\n",
        );
    }
    out.push_str(
        "| Escaping, CSRF, redirects, upload trust | \
         `docs/conventions/security.md` |\n",
    );
    out.push_str(
        "| `rahti.config.json`, CSS, static files, the CLI | \
         `docs/conventions/configuration-and-tooling.md` |\n",
    );
    out.push_str(
        "| Where tests live, driving the router, RPC/auth/database tests | \
         `docs/conventions/testing.md` |\n",
    );
    out.push_str(
        "| Optional Windows/Android packaging, packaged paths, the loopback server | \
         `docs/conventions/native-packaging.md` |\n",
    );

    out.push_str("\n## Non-Negotiable Conventions\n\n");

    let generated: &str = if db {
        "Do not edit generated files by hand: `src/routes.rs`,\n\
         `src/components/mod.rs`, `src/models/mod.rs`, `src/migrations/mod.rs`,\n\
         and `.rahti/manifest.json`. They are regenerated by `rahti-build` on\n\
         every Cargo build."
    } else {
        "Do not edit generated files by hand: `src/routes.rs`,\n\
         `src/components/mod.rs`, and `.rahti/manifest.json`. They are\n\
         regenerated by `rahti-build` on every Cargo build."
    };

    let mut rules: Vec<String> = vec![generated.to_string()];
    rules.push(
        "Routes live under `src/app/`. A page is `page.rs`; a layout is\n\
         `layout.rs`; an error boundary is `error.rs`; a navigation loading\n\
         region is `loading.rs`; the root 404 is `not-found.rs`; an API\n\
         endpoint is `route.rs`. A segment cannot hold both `page.rs` and\n\
         `route.rs`. `_private` directories are ignored by the router."
            .to_string(),
    );
    rules.push(
        "A template is markup written as Rust tokens, `html! { … }`, and only\n\
         a `<script>` or `<style>` body is a string, always written\n\
         `<script>r##\"…\"##</script>` — JavaScript and CSS exactly as\n\
         written, `\"#fff\"` included; `html!` refuses fewer `#`s.\n\
         Text is a quoted string, `<p>\"Hello\"</p>`; a literal attribute is\n\
         quoted, `class=\"card\"`; a binding is braced, `hidden={!open}`; a\n\
         handler is a binding, `onclick={save()}`; a server value is\n\
         `href=@{url}`. A binding is JavaScript the Rust lexer must accept —\n\
         double-quoted strings, no template literals — so longer logic is a\n\
         named function in the script. Compute values in Rust above the\n\
         template and interpolate names. `html!` has one authored root: a\n\
         literal element, a single component tag, or a `<>…</>` fragment. The\n\
         root layout is the exception: it writes a literal `<html>` root,\n\
         optionally preceded by a doctype."
            .to_string(),
    );
    rules.push(
        "Server values use `@{rust_expression}`. PulsePoint browser\n\
         expressions use `{javascript_expression}`. Never interchange the two."
            .to_string(),
    );
    rules.push(
        "Runtime values are\n\
         escaped, braces included (`&#123;`), so no value can open a PulsePoint\n\
         expression; use `Html::from_raw` only for explicitly trusted markup,\n\
         whose braces are not escaped. Client code is written in `html!` and\n\
         nowhere else — it is compiled there into the client registry, and the\n\
         runtime runs nothing else. A component forwards its tag's `{…}` props\n\
         through a `bindings: Bindings` parameter; an `Attrs` value is always\n\
         data, and `Attrs::merge` and `Attrs::only` compose sets."
            .to_string(),
    );
    rules.push(
        "Components are PascalCase functions marked `#[component]`, return\n\
         `Html`, and take props as normal Rust arguments. In markup they are\n\
         called as tags — `<Card title=\"…\">…</Card>` — and children are an\n\
         explicit `Html` argument rendered with `<slot />`."
            .to_string(),
    );
    rules.push(
        "A reactive block and its `<script>` must be self-contained. Children\n\
         written between component tags are the exception: a `{…}` binding\n\
         there reads the writing block's state — `html!` ships such children\n\
         as an owner-scoped `<template pp-owner=\"…\">`, the React children\n\
         semantics. Fragment-rooted (`<>…</>`) and `@{expr}`-built children\n\
         still do not cross the boundary."
            .to_string(),
    );
    rules.push(
        "A page-owned `#[rpc]` lives beside its caller in that `page.rs`; a\n\
         component-owned `#[rpc]` lives in the component file. Parameters are\n\
         owned, deserializable types (`String`, not `&str`); return values\n\
         must serialize; use `rahti::Result<T>` for failures."
            .to_string(),
    );
    rules.push(
        "Static route metadata is `pub fn metadata() -> Metadata` in a page\n\
         or layout, inherited root to leaf. Request-specific metadata is\n\
         returned with `Page::new(content, metadata)`. Rahti writes the\n\
         managed SEO tags; do not duplicate them in the root layout."
            .to_string(),
    );
    rules.push(
        "`cargo rahti build --static` renders the real application router.\n\
         Static pages are automatic; a dynamic page exports exact concrete\n\
         URLs from `pub async fn static_paths()`. Keep `dist/` generated and\n\
         do not remove the export ownership marker by hand."
            .to_string(),
    );
    rules.push(
        "`rahti::cache` stores derived application data, never authority.\n\
         Cache keys include every user, tenant, locale, permission, and input\n\
         dimension that affects a value. Install a shared adapter during\n\
         application startup; its `clear()` removes only the namespace that\n\
         application owns."
            .to_string(),
    );
    rules.push(
        "Page caching is an explicit `pub fn cache() -> PageCache` export on\n\
         a public, deterministic `page.rs`. It caches anonymous complete-document\n\
         responses only; do not use it when cookies, locale headers, experiments\n\
         or other request state changes the HTML. Call `revalidate_path` or\n\
         `revalidate_tag` only after the authoritative write succeeds."
            .to_string(),
    );
    rules.push(
        "Deserialize into owned Rust types first, then use `rahti::Validator`\n\
         for business rules at the handler boundary before side effects.\n\
         Validation failures are HTTP 422 with named field messages; malformed\n\
         or mistyped RPC payloads remain HTTP 400. Database constraints remain\n\
         authoritative for invariants that concurrent requests can race."
            .to_string(),
    );
    rules.push(
        "The generated router applies `rahti::security_headers` once over every\n\
         response including public assets, probes, and failures, immediately\n\
         inside production observability.\n\
         Existing response headers win. Customize the baseline only through\n\
         `rahti::security::configure` during startup. `script-src` is\n\
         `'self'`, with `require-trusted-types-for 'script'; trusted-types\n\
         pulsepoint`: PulsePoint runs only the client registry `html!`\n\
         compiled. Name a vendor SDK's exact host, add `'wasm-unsafe-eval'`\n\
         only for WebAssembly, and never add `https:`, `'unsafe-inline'` or\n\
         `'unsafe-eval'`. Never enable HSTS `includeSubDomains` unless every\n\
         subdomain is HTTPS."
            .to_string(),
    );
    rules.push(
        "The generated router applies the process-local `rahti::rate_limit`\n\
         outside application/auth/CSRF work and inside security headers. The\n\
         default is 200 dynamic requests per client IP per minute; real public\n\
         files, `OPTIONS`, `/health`, `/ready`, and development tooling are\n\
         exempt. Never\n\
         trust `X-Forwarded-For` unless an inaccessible trusted proxy replaces\n\
         it, and do not mistake this bounded per-process safety net for\n\
         distributed DDoS protection."
            .to_string(),
    );
    rules.push(
        "Install application middleware once with\n\
         `rahti::request_pipeline::configure(...)` before building the router.\n\
         It runs in registration order around matched pages, APIs, and RPCs,\n\
         after auth and inside CSRF, rate limiting, and security headers. A\n\
         short circuit skips layouts, caching, body parsing, and the handler;\n\
         public files and framework endpoints never enter this chain."
            .to_string(),
    );
    rules.push(
        "Production observability is the generated router's outermost layer.\n\
         It assigns or validates `X-Request-Id`, emits secret-free JSONL request\n\
         records to stderr, and updates fixed-cardinality process metrics.\n\
         `/health` and `/ready` are reserved public framework routes outside\n\
         sessions and application middleware; readiness checks are named,\n\
         side-effect-free, concurrent, and timeout-bounded. Configure\n\
         `rahti::observability` before building the router, never log credentials\n\
         or bodies, and remember that response duration does not measure a\n\
         streaming body's lifetime."
            .to_string(),
    );
    rules.push(
        "Background jobs are typed, named application work running on Rahti's\n\
         bounded process-local queue. Keep a `JobQueue` handle in application-owned\n\
         state, enqueue only after the authoritative transaction commits, and\n\
         make retryable handlers idempotent. Payloads are neither durable nor\n\
         encrypted; use an external broker when losing queued work on process\n\
         failure is unacceptable. Every host calls\n\
         `rahti::jobs::finish_shutdown()` after HTTP graceful shutdown."
            .to_string(),
    );
    rules.push(
        "Task scheduling submits registered background jobs; it never creates\n\
         a second execution path. Recurring schedules prevent overlap by\n\
         default, coalesce missed ticks, and skip instead of waiting when the\n\
         bounded queue is full. Timers and uniqueness are process-local, so\n\
         use one designated scheduler replica or external durable coordination\n\
         when a fleet must produce one occurrence.\n\
         `rahti::jobs::finish_shutdown()` stops schedulers before queues."
            .to_string(),
    );
    rules.push(
        "`APP_TIMEZONE` is resolved and installed once during startup, defaults\n\
         to UTC, and must be a valid IANA name. It controls calendar display,\n\
         `today`, local-day query bounds, and `Scheduler::daily`; it never\n\
         controls stored instants, sessions, cache TTLs, request budgets,\n\
         retry delays, or signatures. Store instants in UTC and query local\n\
         dates with `rahti::time::day_bounds_utc`."
            .to_string(),
    );
    rules.push(
        "Localization is opt-in through `rahti.config.json`. Fluent catalogs\n\
         live under `<i18n.dir>/<locale>/`, are validated and embedded at build\n\
         time, and are read through request-task-local `rahti::i18n` state. A\n\
         root `[locale]` or `[lang]` segment is authoritative and rejects\n\
         unsupported values. Translation results are escaped text, not trusted\n\
         markup, and localized links keep an explicit locale prefix."
            .to_string(),
    );
    rules.push(
        "Expected 4xx errors may carry public messages; unexpected 5xx errors,\n\
         ordinary errors crossing `?`, and panics expose diagnostics only when\n\
         development mode is active. Use `Error::public` or\n\
         `with_public_message` only for deliberately safe server-error text.\n\
         Page boundaries receive the safe message and optional request id;\n\
         unbounded page/API failures use `application/problem+json`, while RPCs\n\
         keep their JSON wire. Framework error responses are `no-store`, and\n\
         public details never replace reserved error or request-id fields."
            .to_string(),
    );
    rules.push(
        "PulsePoint markup is HTML with `{…}` bindings, never JSX: a binding\n\
         renders text, a condition is `hidden={…}` on the element, a list is\n\
         `<template pp-for=\"item in items\">` with a `key` on the row, and a\n\
         form submit sends `Object.fromEntries(new FormData(event.currentTarget))`\n\
         through `pp.rpc`. Bind first-party events with `on…={…}`; never wire\n\
         your own UI with ids, `querySelector`, `addEventListener` or\n\
         `innerHTML`. The directive, hook and utility lists in\n\
         `docs/conventions/pulsepoint.md` are closed, and `html!` refuses JSX\n\
         markup in a binding, JSX attribute names (`className`, `onClick`, …),\n\
         `pp-for` off a `<template>`, directives that do not exist\n\
         (`pp-if`, `pp-show`, `pp-model`, …), a quoted handler, a template\n\
         written as a string, a script body written as tokens, and a server\n\
         value inside client code (`onclick=@{…}`, `@{…}` within a `{…}`\n\
         binding), naming the form to write. A script reads a server value as\n\
         data: `const id = @{Json(&id)};`."
            .to_string(),
    );
    rules.push(
        "Do not write PulsePoint's runtime-managed DOM attributes by hand. In\n\
         particular, do not invent `data-pp-*`, `pp-owner`, `pp-ref-owner`,\n\
         `pp-script`, `pp-data`, `pp-on*` or `pp-loop-scope`, and write no root-layout marker — `html!` injects\n\
         them."
            .to_string(),
    );
    rules.push(
        "`public/js/pp-reactive-v2.min.js` is a shipped framework asset.\n\
         Never hand-edit it; `cargo rahti upgrade` replaces it."
            .to_string(),
    );
    rules.push(
        "A component script is plain browser JavaScript, so every web API is\n\
         available to it directly: WebGPU (`navigator.gpu`), Canvas/WebGL, Web\n\
         Workers, WebAssembly, Web Audio, WebRTC, IndexedDB, and the device APIs.\n\
         PulsePoint wraps none of them; do not invent `pp.gpu` or similar. The\n\
         handle goes in `pp.ref`, a `pp.effect` acquires, feeds, and releases\n\
         it, and data arrives through `#[rpc]`/`pp.rpc`. See\n\
         `docs/conventions/pulsepoint-web-apis.md`."
            .to_string(),
    );
    rules.push(
        "A `<script>` body is JavaScript exactly as written — comments,\n\
         single quotes and template literals included — with `@{expr}`\n\
         interpolated. A function handed to a library or a long-lived listener is\n\
         wrapped in `pp.event(…)` so it reads current state, never a\n\
         hand-kept \"latest\" ref. A library mounts into an empty element of its\n\
         own, `<div pp-ref=\"{host}\"></div>`; re-renders leave its children and the\n\
         attributes it sets alone."
            .to_string(),
    );
    if ws {
        rules.push(
            "A `#[socket]` function lives beside the page or component whose\n\
             script opens it, takes a final `socket: rahti::ws::Socket`\n\
             parameter, and receives its other arguments as the connection's\n\
             first frame. `#[socket(auth)]` refuses the handshake with a 401\n\
             when there is no session."
                .to_string(),
        );
    }
    rules.push(
        "Authentication is a signed session cookie and a route policy;\n\
         authorization is an application-defined gate of named abilities and\n\
         typed resource policies. Neither infers roles, and there are no OAuth\n\
         providers. Both policies are application-owned (`src/auth.rs`) and\n\
         installed once from `initialize_application` with\n\
         `rahti::auth::configure(auth::settings())` and\n\
         `rahti::authorization::configure(auth::authorization())`, before the router is\n\
         built. `#[rpc(auth)]` guards the call and `private_routes` guards\n\
         the page; an ability guards the action/resource. They do not\n\
         substitute for each other, so enforce every applicable boundary."
            .to_string(),
    );
    rules.push(
        "Auth reads exactly three environment values — `AUTH_SECRET`,\n\
         `AUTH_COOKIE_NAME`, and the optional `SESSION_LIFETIME_HOURS` —\n\
         generated per project and living in the git-ignored `.env`, never in\n\
         `rahti.config.json`. The session payload is signed, not encrypted:\n\
         sign in an id and what the UI needs, never a hash or a token."
            .to_string(),
    );
    rules.push(
        "Development browser warnings/errors and Rahti-handled backend\n\
         failures are written as JSON Lines to `.rahti/dev.log`. Read it when\n\
         diagnosing frontend behavior; never commit it."
            .to_string(),
    );
    rules.push(
        "Tests live in `src/tests/`, included from `src/lib.rs`, and run\n\
         with `cargo test`. They drive the generated router in-process — no\n\
         server, no port. A feature lands with its tests: a page renders, an\n\
         `#[rpc]` answers and refuses a malformed payload, a private route\n\
         redirects without a session. Before testing private routes, install\n\
         the auth policy in `setup()` exactly as `initialize_application`\n\
         does. What only a browser shows — a binding updating, a window\n\
         opening, a library's DOM surviving a re-render — gets a browser\n\
         test with `rahti::browser` (the `browser-test` feature, in\n\
         `[dev-dependencies]`), ending with `browser.errors()` empty. See\n\
         `docs/conventions/testing.md`."
            .to_string(),
    );
    if db {
        rules.push(
            "The database is SeaORM and belongs to this application.\n\
             `src/models/` holds one flat file per table; `src/migrations/`\n\
             holds one flat file per change, named `mYYYYMMDD_NNNNNN_<what>.rs`\n\
             because filename order is application order. The migration is the\n\
             schema — an entity neither creates nor validates a table. Reach\n\
             the connection with `crate::db::db()`; `db::migrate()` is never\n\
             automatic."
                .to_string(),
        );
        rules.push(
            "A connection string is a credential. It lives in `DATABASE_URL`\n\
             via the ignored `.env`, never in `rahti.config.json`, which\n\
             records only which backend the project is on."
                .to_string(),
        );
        rules.push(
            "Tests reach the database the way `setup()` in `src/tests/support.rs`\n\
             shows: a *file* SQLite database, never `:memory:`, deleted on\n\
             the way in, with `db::migrate()` run against it. An in-memory\n\
             database vanishes between `#[tokio::test]` runtimes, and the\n\
             symptom is tests that pass alone and fail together."
                .to_string(),
        );
    }

    for (index, rule) in rules.iter().enumerate() {
        let numbered = format!("{}. {}\n", index + 1, rule.replace('\n', "\n    "));
        out.push_str(&numbered);
    }

    out.push_str(
        "\n## Commands\n\n\
         ```text\n\
         cargo run              # regenerate routes and serve\n\
         cargo dev              # cargo run, restarted on every edit (needs cargo-watch)\n\
         cargo check            # verify without running\n\
         cargo test             # this application's tests\n\
         cargo fmt              # format ordinary Rust\n\
         cargo rahti fmt        # format authored html! markup\n\
         cargo rahti fmt --check # verify markup formatting without writing\n\
         cargo rahti build --static # render the site into dist/\n\
         cargo rahti upgrade    # refresh unedited scaffold files, this guide included\n\
         ```\n\n\
         After editing markup, run `cargo fmt` then `cargo rahti fmt` on the changed\n\
         files. Use four-space nesting and separate lines for nested elements;\n\
         keep visible spaces inside quoted text. The formatter preserves client\n\
         and server expressions, attribute order, and script/style raw strings.\n\
         See the formatting rules in `docs/conventions/configuration-and-tooling.md`.\n",
    );

    out
}
