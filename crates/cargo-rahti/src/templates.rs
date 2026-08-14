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

/// The convention documents every new project carries in `docs/conventions/`.
///
/// Rahti is not in anyone's training data, so a project that leaves the
/// scaffold without its documentation leaves without the framework: the next
/// coding agent opened inside it has nothing to read and guesses. The copies
/// live in this crate's `assets/` for the same reason the client runtime
/// does — a published crate cannot read a file outside its own directory —
/// and a test holds each one byte-identical to its canonical original.
///
/// These eight are unconditional. The two that depend on a scaffold choice —
/// `database.md` and `websockets.md` — are below, written only when the
/// choice was made: a guide that documents a feature the project does not
/// have teaches an agent to reach for it.
pub const CORE_DOCS: [(&str, &str); 8] = [
    ("routing.md", include_str!("../assets/docs/routing.md")),
    (
        "rendering-and-components.md",
        include_str!("../assets/docs/rendering-and-components.md"),
    ),
    (
        "pulsepoint.md",
        include_str!("../assets/docs/pulsepoint.md"),
    ),
    (
        "diagnostics.md",
        include_str!("../assets/docs/diagnostics.md"),
    ),
    (
        "rpc-and-uploads.md",
        include_str!("../assets/docs/rpc-and-uploads.md"),
    ),
    (
        "authentication.md",
        include_str!("../assets/docs/authentication.md"),
    ),
    ("security.md", include_str!("../assets/docs/security.md")),
    (
        "configuration-and-tooling.md",
        include_str!("../assets/docs/configuration-and-tooling.md"),
    ),
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
/// Tailwind's class-merging helper. Written only for a Tailwind project;
/// 100 KB is a strange thing to serve an app that has no utility classes.
pub const TAILWIND_MERGE: &[u8] = include_bytes!("../assets/js/tailwind-merge.mjs");
pub const FAVICON: &[u8] = include_bytes!("../assets/favicon.ico");

pub fn cargo_toml(name: &str, rahti: &str, db: Option<Backend>, ws: bool) -> String {
    // WebSockets are a feature of the `rahti` dependency, not a crate of
    // their own: the wire brings tokio-tungstenite with it, so a project
    // that never opens a socket should not compile it. The build dependency
    // below stays as it is — `rahti-build` has no such feature.
    let rahti_dep = if ws {
        with_ws_feature(rahti)
    } else {
        rahti.to_string()
    };

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

[dependencies]
axum = "0.8.9"
rahti = {rahti_dep}
serde = {{ version = "1", features = ["derive"] }}
tokio = {{ version = "1", features = ["full"] }}
tower-http = {{ version = "0.7", features = ["catch-panic", "fs"] }}
{database}
[build-dependencies]
rahti-build = {build}
"#,
        build = rahti.replace("crates/rahti\"", "crates/rahti-build\"")
    )
}

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
        return format!("{}\"ws\"{separator}{}", &rahti[..open], rest);
    }

    match rahti.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        Some(inner) => format!("{{{}, features = [\"ws\"] }}", inner.trim_end()),
        None => format!("{{ version = {rahti}, features = [\"ws\"] }}"),
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
    // exactly the same reason a connection string is. `.env.example` is
    // committed and carries a placeholder in its place.
    let mut out = String::from(
        r#"/target
# Generated manifests, build scratch space, and development diagnostics.
# Nothing below .rahti is application input or shipped output.
/.rahti
# Credentials: the session signing secret, and the connection string if this
# project has a database. `.env.example` is the committed shape of this file,
# with a placeholder where the secret goes.
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

/// The values `new` generates once and writes into both `.env` files.
///
/// Generated per project rather than fixed, so two Rahti applications never
/// ship the same signing key or fight over the same cookie name. Made once and
/// passed to both calls of [`env`]: the cookie name has to be *the same* in
/// the real file and the committed example, and the secret has to differ.
pub struct EnvValues {
    /// The real signing key. Written to `.env` only.
    pub secret: String,
    /// The cookie's name. Not a credential — it is the project's identity
    /// under a shared parent domain — so it is written to both files.
    pub cookie: String,
}

/// The placeholder the committed example carries in place of the real key.
///
/// A word the runtime knows: `rahti::auth` refuses to start a release build
/// whose `AUTH_SECRET` is still this, so copying `.env.example` to `.env` and
/// forgetting the rest fails loudly instead of signing every session in
/// production with a value that is in the repository.
pub const SECRET_PLACEHOLDER: &str = "change-me";

/// `.env`, and the committed `.env.example` beside it.
///
/// `example` is what separates them. The real file carries the generated key;
/// the example carries a placeholder, because a signing secret in git is not a
/// secret. Everything else is identical, so the example is an accurate account
/// of what a clone has to fill in.
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

pub fn env(backend: Option<Backend>, values: &EnvValues, example: bool) -> String {
    let mut out = String::new();

    if let Some(backend) = backend {
        out.push_str(&database_env(backend));
    }

    let secret = if example {
        SECRET_PLACEHOLDER
    } else {
        &values.secret
    };

    // The example says what the placeholder costs you, because the person
    // reading it is the person who is about to copy this file.
    let secret_note = if example {
        "\n# This is a placeholder, not a key. A release build refuses to start\n\
         # while AUTH_SECRET is still `change-me` — generate your own:\n\
         #     openssl rand -base64 32\n"
    } else {
        ""
    };

    out.push_str(&format!(
        r#"# =============================================================================
# AUTHENTICATION AND SESSIONS
# Read by rahti::auth. Which routes are private lives in src/auth.rs, not here:
# a policy is code, and only the three values below are environment.
# =============================================================================

# Session signing secret. Unique and strong per app and per environment.
# In production the app refuses to start when this is missing or left on a
# placeholder; in development it invents one and warns, so a fresh clone runs.
{secret_note}AUTH_SECRET="{secret}"

# Session cookie name. Generated per project: several apps under one parent
# domain that share a cookie name overwrite each other's sessions.
AUTH_COOKIE_NAME="{cookie}"

# Optional. Session lifetime in hours; the default is 1.
SESSION_LIFETIME_HOURS="{lifetime}"
"#,
        cookie = values.cookie,
        lifetime = DEFAULT_LIFETIME_HOURS,
    ));

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
        }
    }
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

pub fn main_rs(db: bool) -> String {
    let (modules, connect) = if db {
        (
            r#"
/// The database connection, and the two directories `rahti-build` wires up
/// from their contents: an entity per file, and a migration per file.
mod db;
mod migrations;
mod models;
"#,
            r#"
    // Before the listener, so a project that cannot reach its database says
    // so once at startup rather than once per request.
    db::connect().await;
"#,
        )
    } else {
        ("", "")
    };

    // Concatenated rather than formatted: the body below is Rust that uses
    // `format!` and closures of its own, and escaping every brace in it to
    // satisfy one placeholder makes a template nobody can read.
    let mut out = String::from(
        r#"/// Framework runtime, from the `rahti` crate.
pub use rahti;

/// Shared components. `mod.rs` is generated by `rahti-build` from the files
/// in `src/components/`, so the directory name is part of the contract.
mod components;
mod routes;
"#,
    );
    out.push_str(modules);
    out.push_str("\n#[tokio::main]\nasync fn main() {");
    out.push_str(connect);
    out.push_str(
        r#"
    let app = routes::router();

    // The configured address, unless the environment names another: a
    // deployment sets `PORT` (and `HOST` if it needs one), so this line is
    // the same in development and in production. In dev mode a busy port
    // falls forward to the next free one — the line below names the address
    // that actually bound. In release a busy port is an error: whatever
    // routes traffic to this process points at the port it was given.
    let listener = rahti::listen(routes::HOST, routes::PORT).await;

    // Not the bind address verbatim: `0.0.0.0` is every interface, which is
    // not somewhere a browser can go.
    println!("Server running on {}", rahti::local_url(&listener));

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            rahti::shutdown_signal().await;
            println!("\nShutting down...");
        })
        .await
        .unwrap();

    println!("Server stopped.");
}
"#,
    );
    out
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
         `db::connect().await` runs from `main` before the server starts — \
         if you removed it, put it back.",
    )
}

/// Connect once, at startup.
///
/// Before the listener binds, so a project that cannot reach its database
/// says so once with a readable message instead of once per request with a
/// 500. Exits rather than panicking: a backtrace through tokio describes the
/// framework, and the cause is almost always the URL or a server that is not
/// running.
pub async fn connect() {
    // One `.env` reader for the process, in the framework — `AuthSettings`
    // needs the same file, and a project with no database still has one.
    // Idempotent, so neither caller has to know which ran first, and a real
    // environment variable still wins over the file.
    rahti::load_env();

    let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        eprintln!(
            "rahti: DATABASE_URL is not set.\n  \
             It lives in `.env`, which is ignored by git because a connection \
             string is a credential.\n  \
             Copy `.env.example` to `.env`, or export it in your shell."
        );
        std::process::exit(1);
    });

    if let Err(e) = connect_to(&url).await {
        eprintln!(
            "rahti: cannot connect to the database: {e}\n  \
             Check DATABASE_URL, and that the server is running."
        );
        std::process::exit(1);
    }
}

/// Connect to a named database instead of DATABASE_URL.
///
/// This is how a test gets one. SQLite needs no server, so a whole suite can
/// run against a real engine:
///
/// ```no_run
/// # async fn example() {
/// let path = std::env::temp_dir().join("my-app-tests.db");
/// let _ = std::fs::remove_file(&path);
///
/// crate::db::connect_to(&format!("sqlite://{}?mode=rwc", path.display()))
///     .await
///     .unwrap();
/// crate::db::migrate().await;
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
/// ```no_run
/// # async fn example() {
/// crate::db::migrate().await;
/// # }
/// ```
pub async fn migrate() {
    use sea_orm_migration::MigratorTrait;

    if let Err(e) = crate::migrations::Migrator::up(db(), None).await {
        eprintln!("rahti: a migration failed: {e}");
        std::process::exit(1);
    }
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
        r#"use crate::rahti::{{Html, html}};

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
///
/// No `pp-loading-content="true"` is written here. That marker names the
/// region a `loading.rs` replaces while the next page is fetched, and the
/// runtime takes the first marked element in the document — so it belongs in
/// the deepest layout that wraps exactly what the swap should replace (a
/// dashboard layout's content pane, say, so its top bar stays put). Marking
/// this root slot would swap the whole page and override every layout below.
pub fn layout(children: Html) -> Html {{
    html! {{
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="UTF-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1.0" />
                <title>"{title}"</title>
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
"#
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
        r#"use crate::rahti::{{Html, html, rpc}};

/// The home page. Every `page.rs` under `src/app/` becomes a URL by where it
/// sits, and this one sits at the root.
pub async fn page() -> Html {{
    html! {{
        <section class="{section}">
            <h1 class="{heading}">"{name}"</h1>
            <p class="{lede}">"Edit src/app/page.rs and reload."</p>

            <button class="{button}" onclick={{greet()}}>"Say hello"</button>
            <p class="{lede}">{{message}}</p>

            <script>
                const [message, setMessage] = pp.state("");

                async function greet() {{
                    setMessage(await pp.rpc("hello", {{}}));
                }}
            </script>
        </section>
    }}
}}

/// Called by `pp.rpc("hello")` above. It runs on the server, in Rust, and the
/// browser never learns there was a function here at all.
#[rpc]
pub async fn hello() -> String {{
    "Hello from Rust.".to_string()
}}
"#
    )
}

/// `main.js`, with the Tailwind helper only where there is Tailwind.
pub fn main_js(tailwind: bool) -> String {
    let mut out = String::from("import \"/js/pp-reactive-v2.min.js\";\n");

    if tailwind {
        out.push_str("import { twMerge } from \"/js/tailwind-merge.mjs\";\n");
    }

    out.push_str("\nconst pp = (globalThis).pp;\n");

    if tailwind {
        out.push_str("\nglobalThis.twMerge = twMerge;\n");
    }

    out.push_str(
        r#"
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
 * browser as it stands, with any stylesheets your component libraries ship
 * appended after it. Nothing is downloaded and nothing is compiled. */

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
        "| Browser state, hooks, bindings, events, lists, refs, SPA navigation | \
         `docs/conventions/pulsepoint.md` |\n",
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
        "`html!` has one authored root: a literal element, a single component\n\
         tag, or a `<>…</>` fragment. The root layout is the exception: it\n\
         writes a literal `<html>` root, optionally preceded by a doctype."
            .to_string(),
    );
    rules.push(
        "Server values use `@{rust_expression}`. PulsePoint browser\n\
         expressions use `{javascript_expression}`. Never interchange the two."
            .to_string(),
    );
    rules.push(
        "Author text in `html!` as quoted Rust strings. Runtime values are\n\
         escaped; use `Html::from_raw` only for explicitly trusted markup."
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
        "A reactive block and its `<script>` must be self-contained.\n\
         PulsePoint state does not cross an `html!` component/children\n\
         boundary."
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
        "Do not write PulsePoint's runtime-managed DOM attributes by hand. In\n\
         particular, do not invent `data-pp-*`, `pp-owner`, or\n\
         `pp-ref-owner`, and write no root-layout marker — `html!` injects\n\
         them."
            .to_string(),
    );
    rules.push(
        "`public/js/pp-reactive-v2.min.js` is a shipped framework asset.\n\
         Never hand-edit it; `cargo rahti upgrade` replaces it."
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
        "Authentication is a signed session cookie and a route policy, and\n\
         nothing else — no OAuth providers, no roles. The policy is\n\
         application-owned (`src/auth.rs`, once you add private routes) and\n\
         installed once from `main` with\n\
         `rahti::auth::configure(auth::settings())`, before the router is\n\
         built. `#[rpc(auth)]` guards the call and `private_routes` guards\n\
         the page; they do not substitute for each other, so mark every rpc\n\
         that touches something private."
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
         cargo rahti upgrade    # refresh unedited scaffold files, this guide included\n\
         ```\n",
    );

    out
}
