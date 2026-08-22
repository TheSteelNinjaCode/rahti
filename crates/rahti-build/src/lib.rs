//! Rahti's build step.
//!
//! Runs before every compile: walks the app directory, validates it, and
//! writes an ordinary Rust router plus a manifest. Nothing here happens at
//! runtime — by the time the server boots, `page.rs` files are compiled
//! functions inside the binary.
//!
//! The app's `build.rs` is a one-line call to [`run`]. Every path below is
//! resolved against the working directory cargo hands a build script, which
//! is the root of the crate being built.

mod codegen;
mod components;
mod css;
mod db;
mod deps;
mod route;
mod scanner;
pub mod sha256;

// Tests live in `src/tests/`, one file per module — see `tests/README.md`
// for why they are declared from here rather than compiled on their own.
#[cfg(test)]
#[path = "tests/tree.rs"]
mod tree;

#[cfg(test)]
#[path = "tests/config.rs"]
mod tests;

use std::path::Path;
use std::{env, fs};

pub struct Config {
    pub app_dir: String,
    pub public_dir: String,
    pub host: String,
    pub port: u16,
    pub css: Css,
    /// The database wiring, when the project has one.
    ///
    /// `None` is the ordinary case and costs nothing: no directories, no
    /// generated modules, no mention of SeaORM anywhere in the build.
    pub db: Option<Db>,
    /// Whether any dependency ships Rahti components.
    ///
    /// Not configuration — read from the dependency graph, and carried here
    /// because the codegen needs it: a library's `#[rpc]` is reachable only
    /// through the registry fallback, and a page has to answer POST for that
    /// fallback to be reached at all. Nothing in this project's own files
    /// says whether such a library is present.
    pub has_libraries: bool,
}

/// How `entry` becomes `output`.
///
/// Tailwind is a choice, not the framework. An app that brings its own CSS
/// should not download a compiler to copy a file, and — more to the point —
/// should not have to learn a different project layout to avoid it: under
/// every engine the author edits `entry` and the browser is served `output`.
/// Switching is a one-word change, not a migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// Compile `entry` with the Tailwind standalone CLI.
    Tailwind,
    /// Copy `entry` to `output` as it is, with any library stylesheets
    /// appended. No CLI, no download, no network.
    Plain,
    /// Leave `output` alone. The app manages its public directory itself.
    None,
}

/// The stylesheet build — see `css.rs`.
///
/// The entry lives in the app directory, next to the pages whose classes it
/// compiles, and the output lands in `public/`. Nothing here is required:
/// with no `css` object and no entry file, the build does not change.
pub struct Css {
    /// Which of the three ways `entry` reaches `output`.
    pub engine: Engine,
    /// The stylesheet the app edits, relative to the crate root. Compiled,
    /// copied, or ignored according to `engine`.
    pub entry: String,
    /// Where the compiled sheet lands. Under `public/`, so it is served —
    /// and checked in, so a clone with no CLI still has one.
    pub output: String,
    /// Which standalone CLI release to use. Pinned rather than floating: a
    /// stylesheet that changes because upstream released is not wanted.
    pub version: String,
    /// Fetch the CLI when it is not already on disk.
    pub download: bool,
    /// Minify the output. A setting rather than a profile switch, so the
    /// checked-in sheet does not churn between debug and release builds.
    pub minify: bool,
    /// Expected hash of the CLI. Enforced when set; reported when not.
    pub sha256: Option<String>,
}

/// Which database the application talks to.
///
/// Recorded rather than inferred from the connection string, because the
/// string is not here to read: it carries credentials, so it lives in the
/// environment and never in a committed file. What the build needs the
/// backend for is the message it prints when something does not line up —
/// naming the wrong engine is worse than naming none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
    MySql,
}

impl Backend {
    /// The name as it is written in the config, for messages and the manifest.
    pub fn label(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
            Backend::MySql => "mysql",
        }
    }

    /// The cargo feature the application's `sea-orm` dependency needs, which
    /// is the one thing about a backend that is not interchangeable.
    pub fn feature(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlx-sqlite",
            Backend::Postgres => "sqlx-postgres",
            Backend::MySql => "sqlx-mysql",
        }
    }
}

/// The database wiring — see `db.rs`.
///
/// Two directories and the name of an engine. Deliberately not a connection
/// string, a pool size, or a migration policy: those are the application's,
/// read from the environment or written in its own `src/db.rs`, and a
/// framework that owned them would be deciding how somebody else's database
/// is reached.
pub struct Db {
    pub backend: Backend,
    /// Where the entities live. Generated `mod.rs`, hand-written files.
    pub models: String,
    /// Where the migrations live. Generated `mod.rs` — including the
    /// `Migrator` — and hand-written files.
    pub migrations: String,
}

/// The project's single source of truth, alongside `Cargo.toml`.
///
/// JSON rather than TOML because this file has two authors. A person edits it
/// — the port, the app directory — and so does the scaffold, which writes it
/// on `new` and rewrites it on an upgrade. TOML that survives a machine
/// rewrite with its comments intact needs a whole editing crate; JSON needs
/// nothing, and `$schema` gives an editor the descriptions and completion
/// that the comments used to carry, while typing rather than only while
/// reading.
///
/// Beyond settings it records provenance — `schema`, `createdWith`, and the
/// scaffold's file ledger. The build reads none of that and rewrites none of
/// it, so those keys pass through untouched; they exist for the upgrade
/// command, which has to tell a file it wrote from a file you then edited.
const CONFIG_PATH: &str = "rahti.config.json";

/// The configuration shape this build understands.
///
/// A file claiming a higher number was written by a newer Rahti and may mean
/// things by keys this one would misread — better to say so than to guess.
/// A file claiming nothing is assumed current: a minimal hand-written config
/// is a legitimate thing to have.
const SCHEMA: i64 = 1;
/// Generated, but checked in: it is ordinary Rust the compiler reads.
const ROUTES_PATH: &str = "src/routes.rs";
/// Generated, and not checked in: a description of the scan, for tooling.
const MANIFEST_PATH: &str = ".rahti/manifest.json";
/// The standalone CLI release used when `[css] version` says nothing.
const DEFAULT_TAILWIND: &str = "4.3.3";

pub fn run() {
    // Without this, editing configuration leaves the route table stale and
    // you get a 404 with no explanation. The modules in this crate need no
    // such line: changing one rebuilds the build script itself, which cargo
    // already treats as a reason to rerun it.
    println!("cargo:rerun-if-changed={CONFIG_PATH}");

    let mut config = load_config();

    // `html!` reads this during expansion to work out which part of a file
    // path is the route, so `pp-component` names survive a moved app dir.
    println!("cargo:rustc-env=RAHTI_APP_DIR={}", config.app_dir);

    let scan = scanner::scan(Path::new(&config.app_dir));
    let components = components::scan(Path::new(components::COMPONENTS_DIR));

    // Only when the project configured one. An app with no `db` object never
    // has these directories created, never has a module generated, and never
    // learns that SeaORM exists.
    let database = config.db.as_ref().map(db::scan);

    // A component's rpcs are reached by name from any page, so the names are
    // the app's rather than one file's — checked here, where both scans are in
    // hand.
    components::check_rpc_names(&components.rpcs, &scan.entries);

    // Sockets are one flat namespace wherever they were written — a page's
    // and a component's answer on the same endpoint — so the check is over
    // the combined list, and the combined list is what the router and the
    // manifest describe.
    let sockets = socket_list(&scan.entries, &components.sockets);
    components::check_socket_names(&sockets);

    // Every directory, not just the app root — see `scanner::Scan::watched`.
    let watched_db = database.as_ref().map(|d| &d.watched);
    for path in scan
        .watched
        .iter()
        .chain(&components.watched)
        .chain(watched_db.into_iter().flatten())
    {
        println!("cargo:rerun-if-changed={path}");
    }

    // Component libraries from the dependency graph. Nothing else in this
    // build ever looks outside the crate — see `deps.rs`.
    //
    // Read whatever the stylesheet is doing, because the CSS step is no
    // longer the only thing that cares: a library's `#[rpc]` needs the
    // registry fallback generated, and whether to generate it is a question
    // only the graph can answer. An app with no libraries pays one
    // `cargo metadata` and gets an empty list.
    let libraries = deps::scan();
    config.has_libraries = !libraries.is_empty();

    // A library edited in place — a `path` dependency, most often the one
    // being written — has to reach the stylesheet without a `cargo clean`.
    for path in deps::watched(&libraries) {
        println!("cargo:rerun-if-changed={path}");
    }

    // After the scan, because a stylesheet is compiled against the classes
    // written in the files the scan just walked.
    css::build(&config.css, &libraries);

    let db_modules = database.as_ref().map(|d| &d.modules);
    for (path, contents) in components
        .modules
        .iter()
        .chain(db_modules.into_iter().flatten())
    {
        write_if_changed(path, contents);
    }
    write_if_changed(
        ROUTES_PATH,
        &codegen::routes_rs(&scan.entries, &components.rpcs, &sockets, &config),
    );
    write_if_changed(
        MANIFEST_PATH,
        &codegen::manifest_json(
            &scan.entries,
            &components.files,
            &components.rpcs,
            &sockets,
            database.as_ref(),
            &config,
        ),
    );
}

/// Every `#[socket]` in the project, pages and components in one list.
///
/// One list because the wire is one namespace: every connection arrives on
/// the same endpoint and is picked by name alone. Sorted by file so the
/// generated output is the same for the same tree.
fn socket_list(entries: &[scanner::Entry], components: &[components::Rpc]) -> Vec<components::Rpc> {
    let mut sockets: Vec<components::Rpc> = entries
        .iter()
        .flat_map(|entry| {
            entry.sockets.iter().map(|name| components::Rpc {
                name: name.clone(),
                module: entry.module.clone(),
                file: entry.file.clone(),
                auth: entry.auth_sockets.contains(name),
            })
        })
        .chain(components.iter().cloned())
        .collect();
    sockets.sort_by(|a, b| (&a.file, &a.name).cmp(&(&b.file, &b.name)));
    sockets
}

fn load_config() -> Config {
    let raw = match fs::read_to_string(CONFIG_PATH) {
        Ok(raw) => raw,
        // The file this replaced. Saying "no such file" to someone holding a
        // working `rahti.toml` describes the symptom and hides the cause.
        Err(e) if Path::new("rahti.toml").is_file() => panic!(
            "rahti: this project still has a `rahti.toml`, and configuration \
             now lives in {CONFIG_PATH}.\n  \
             The keys are the same, one JSON object per table — `[server]` \
             becomes `\"server\": {{ … }}`.\n  \
             ({CONFIG_PATH}: {e})"
        ),
        Err(e) => panic!("rahti: cannot read {CONFIG_PATH}: {e}"),
    };
    parse_config(&raw)
}

/// `rahti.config.json`, with every key optional and defaulted.
fn parse_config(raw: &str) -> Config {
    // An absent or empty file is a project that has configured nothing, which
    // is a supported way to have a project — not a parse error.
    let value: serde_json::Value = if raw.trim().is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("rahti: {CONFIG_PATH} is not valid JSON: {e}"))
    };

    check_schema(&value);

    let string_at = |table: &str, key: &str, default: &str| -> String {
        value
            .get(table)
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .to_string()
    };

    let bool_at = |table: &str, key: &str| -> Option<bool> {
        value
            .get(table)
            .and_then(|t| t.get(key))
            .and_then(|v| v.as_bool())
    };

    let port = value
        .get("server")
        .and_then(|t| t.get("port"))
        .and_then(|v| v.as_i64())
        .unwrap_or(3000);

    let css = Css {
        entry: string_at("css", "entry", "src/app/globals.css"),
        output: string_at("css", "output", "public/css/styles.css"),
        version: string_at("css", "version", DEFAULT_TAILWIND),
        // Off by default. A build script that reaches the network without
        // being asked breaks `--offline`, vendored builds and sandboxed CI,
        // and the committed stylesheet means nothing is lost by waiting to
        // be asked.
        download: bool_at("css", "download").unwrap_or(false),
        // Readable by default: the sheet is committed, and a diff you can
        // read is worth more than the bytes gzip would have saved anyway.
        minify: bool_at("css", "minify").unwrap_or(false),
        // `css.sha256` is keyed by platform, because a checksum is of one
        // specific binary — a single value would be wrong for every machine
        // but the one that wrote it. An unlisted platform is unpinned, not
        // an error: the build prints its hash so the entry can be added.
        sha256: css::platform_key(env::consts::OS, env::consts::ARCH).and_then(|key| {
            value
                .get("css")
                .and_then(|t| t.get("sha256"))
                .and_then(|t| t.get(key))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        }),
        engine: engine_at(&value),
    };

    Config {
        // Filled in by `run` once the graph has been read; a parsed file says
        // nothing about it.
        has_libraries: false,
        db: db_at(&value),
        app_dir: string_at("app", "dir", "src/app"),
        // Deliberately not watched: assets are read at runtime, so editing
        // a stylesheet must not trigger a recompile.
        public_dir: string_at("app", "public", "public"),
        // The same wildcard `cargo rahti new` writes, so a config that drops
        // the key behaves like one that never edited it.
        host: string_at("server", "host", "0.0.0.0"),
        port: u16::try_from(port)
            .unwrap_or_else(|_| panic!("rahti: server.port must be 0-65535, got {port}")),
        css,
    }
}

/// Write only on a real change.
///
/// `src/routes.rs` is checked in and lives under a directory cargo watches;
/// rewriting it unconditionally would force a rebuild on every single build.
/// `css.engine`, defaulting to the one an app that says nothing wants.
///
/// A misspelling is refused rather than defaulted. The two silent outcomes
/// are both bad — a typo that falls back to Tailwind downloads a compiler
/// nobody asked for, and one that falls back to `none` leaves every page
/// unstyled with no error to explain it.
fn engine_at(value: &serde_json::Value) -> Engine {
    let Some(named) = value.get("css").and_then(|c| c.get("engine")) else {
        return Engine::Tailwind;
    };

    match named.as_str() {
        Some("tailwind") => Engine::Tailwind,
        Some("plain") => Engine::Plain,
        Some("none") => Engine::None,
        other => panic!(
            "rahti: css.engine is {}, which is not an engine.\n  \
             Use \"tailwind\" to compile with the Tailwind CLI, \"plain\" to \
             copy the entry sheet as it is, or \"none\" to manage the public \
             directory yourself.",
            other
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| named.to_string())
        ),
    }
}

/// `db`, absent unless the project asked for one.
///
/// No `db` object means no database, and that is the difference between a
/// build that generates two module files and one that does not touch the
/// disk — so absence is read as an answer rather than defaulted into a
/// SQLite project nobody set up.
///
/// A misspelled backend is refused for the same reason `css.engine` is: both
/// silent outcomes are wrong. Falling back to SQLite would have the build
/// wiring a database the application is not talking to, and falling back to
/// none would leave `crate::models` undeclared with nothing to explain it.
fn db_at(value: &serde_json::Value) -> Option<Db> {
    let db = value.get("db")?;

    // `"db": null` is how a project turns the database off without losing the
    // shape of the object it had.
    if db.is_null() {
        return None;
    }

    let named = db.get("backend");
    let backend = match named.and_then(|v| v.as_str()) {
        Some("sqlite") => Backend::Sqlite,
        Some("postgres") => Backend::Postgres,
        Some("mysql") => Backend::MySql,
        // Absent is SQLite: it is the backend a project gets by default, and
        // the one that needs no server to be running to answer.
        None if named.is_none() => Backend::Sqlite,
        other => panic!(
            "rahti: db.backend is {}, which is not a backend.\n  \
             Use \"sqlite\", \"postgres\" or \"mysql\" — the name decides which \
             `sea-orm` feature your Cargo.toml needs, and nothing else.",
            other
                .map(|s| format!("\"{s}\""))
                .unwrap_or_else(|| named.expect("a key that was read").to_string())
        ),
    };

    let path = |key: &str, default: &str| -> String {
        db.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or(default)
            .trim_end_matches('/')
            .to_string()
    };

    Some(Db {
        backend,
        models: path("models", "src/models"),
        migrations: path("migrations", "src/migrations"),
    })
}

/// Refuse a file written by a Rahti newer than this one.
///
/// The failure this avoids is quiet: a later version gives an existing key a
/// new meaning, this build reads it the old way, and the app is configured as
/// nobody asked. A version number is cheap and the alternative is a bug that
/// looks like the framework misbehaving.
fn check_schema(value: &serde_json::Value) {
    let Some(schema) = value.get("schema").and_then(|v| v.as_i64()) else {
        return;
    };

    if schema > SCHEMA {
        panic!(
            "rahti: {CONFIG_PATH} is written for schema {schema}, and this build \
             understands {SCHEMA}.\n  \
             The project was set up by a newer Rahti — update the `rahti` \
             dependency in Cargo.toml to match."
        );
    }
}

fn write_if_changed(path: &str, contents: &str) {
    // Rust output goes through rustfmt first. See [`formatted`].
    let owned;
    let contents = if path.ends_with(".rs") {
        owned = formatted(contents);
        owned.as_str()
    } else {
        contents
    };

    if fs::read_to_string(path).is_ok_and(|existing| existing == contents) {
        return;
    }
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(path, contents).unwrap_or_else(|e| panic!("rahti: cannot write {path}: {e}"));
}

/// Generated Rust, as rustfmt would leave it.
///
/// ## Why the build does this
///
/// The generated files are checked in, and `cargo fmt --all -- --check` is one
/// of the commands a Rahti project is told to run. Those two facts collide:
/// rustfmt has opinions about the emitted code — it wraps the long `.layer(…)`
/// chains and sorts the `mod` declarations — so the gate failed on a file
/// nobody wrote. Worse, `cargo fmt --all` *fixed* it, and the next build put it
/// back, so the two commands undid each other forever.
///
/// Formatting here settles it: what the build writes is already what rustfmt
/// wants, so the check passes and running the formatter changes nothing.
///
/// This is deliberately not done in `codegen`. That module's job is to emit
/// correct Rust, its tests assert on the text it emits, and neither should
/// have to care what the current rustfmt thinks about line width.
///
/// ## When it cannot
///
/// rustfmt is part of a default toolchain but not guaranteed to be installed,
/// and a build must not fail because a formatter is missing. Anything that
/// goes wrong — no rustfmt, a non-zero exit, output that is not UTF-8 — leaves
/// the text exactly as `codegen` produced it, which is valid Rust that
/// compiles. The only thing lost is the `cargo fmt --check` gate, which is
/// what the situation was before.
///
/// Costs one process per changed generated file, around 60ms. It runs on every
/// build rather than only on a change, because whether there *is* a change can
/// only be decided after formatting — the file on disk is the formatted one.
fn formatted(source: &str) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let spawned = Command::new("rustfmt")
        .args(["--emit", "stdout", "--edition", "2024"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();

    let Ok(mut child) = spawned else {
        return source.to_string();
    };

    // Dropped after the write so rustfmt sees end-of-input and can finish;
    // holding it open would deadlock `wait_with_output`.
    if let Some(mut stdin) = child.stdin.take() {
        if stdin.write_all(source.as_bytes()).is_err() {
            return source.to_string();
        }
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            String::from_utf8(output.stdout).unwrap_or_else(|_| source.to_string())
        }
        _ => source.to_string(),
    }
}
