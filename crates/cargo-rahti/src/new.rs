//! `cargo rahti new <name>` — write a project, and record what was written.
//!
//! The recording is the part that matters beyond today. Every file this
//! creates goes into `rahti.config.json` under `scaffold`, with its SHA-256
//! at the moment it was written. A later `upgrade` compares: a file whose
//! hash still matches is the scaffold's own and may be rewritten or removed
//! silently, and one whose hash has changed is the author's and is left alone
//! with a note. Without the ledger an upgrade has to choose between never
//! tidying anything up and destroying work, and both are wrong.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rahti_build::{Backend, sha256};

use crate::VERSION;
use crate::prompt::{choose, confirm};
use crate::templates as t;

#[cfg(test)]
#[path = "tests/new.rs"]
mod tests;

/// The files written, in the order they were written, each with its hash.
///
/// Ordered rather than hashed so the config is byte-identical for the same
/// choices — a scaffold whose output reshuffles between runs makes every
/// diff unreadable and every test flaky.
pub type Ledger = BTreeMap<String, String>;

pub fn run(args: &[&str]) -> Result<(), String> {
    let options = Options::parse(args)?;

    let root = PathBuf::from(&options.name);
    if root.exists() {
        return Err(format!(
            "`{}` already exists. Choose another name, or remove it first.",
            options.name
        ));
    }

    // A flag only ever says yes, so it short-circuits the question. Without
    // it there is nothing to skip: an interactive run asks, and anything else
    // gets the answer that omitting the flag already means.
    let tailwind = options.tailwind || confirm("Use Tailwind CSS?");

    // Asked as one question rather than two — "a database?" then "which?" —
    // because the second answer is the whole of the first.
    let db = match options.db {
        Some(backend) => Some(backend),
        None if confirm("Use a database?") => Some(backend_of(&choose(
            "Which one?",
            &["sqlite", "postgres", "mysql"],
        ))?),
        None => None,
    };

    // WebSockets are a cargo feature of `rahti`, so the choice has to be made
    // while the manifest is being written — turning it on later is one line
    // in Cargo.toml, but the scaffold should not make anyone learn which.
    let ws = options.ws || confirm("Use WebSockets?");
    let mcp = options.mcp || confirm("Use MCP developer tooling?");

    let mut ledger = Ledger::new();
    write_project(&root, &options, tailwind, db, ws, mcp, &mut ledger)?;
    write_config(&root, tailwind, db, ws, mcp, &ledger)?;

    report(&options.name, tailwind, db, ws, mcp);
    Ok(())
}

/// A backend by the name it is written by, in the config and on the flag.
/// Shared with `upgrade`, whose `--db` takes the same names.
pub(crate) fn backend_of(name: &str) -> Result<Backend, String> {
    match name {
        "sqlite" => Ok(Backend::Sqlite),
        "postgres" => Ok(Backend::Postgres),
        "mysql" => Ok(Backend::MySql),
        other => Err(format!(
            "`{other}` is not a backend. Use sqlite, postgres or mysql."
        )),
    }
}

/// Every file the scaffold owns, as bytes, without writing anything.
///
/// Produced rather than written so `upgrade` can diff against exactly what
/// `new` would have written today. Two copies of this list would drift, and
/// the drift would show up as an upgrade quietly skipping a file it should
/// have fixed.
pub fn project_files(
    name: &str,
    tailwind: bool,
    db: Option<Backend>,
    ws: bool,
    mcp: bool,
    local: Option<&str>,
) -> Vec<(String, Vec<u8>)> {
    let rahti = match local {
        // A path dependency, for working on the framework itself. Absolute,
        // because the generated project is rarely a sibling of the checkout.
        Some(path) => format!(
            "{{ path = \"{}/crates/rahti\" }}",
            path.replace('\\', "/").trim_end_matches('/')
        ),
        // Pinned to all three components, not `"0.0"`. Under cargo's semver
        // rules every `0.0.x` is its own incompatible line, so `"0.0"` would
        // let a project drift onto a release the framework never promised it,
        // and the framework ships in lockstep.
        //
        // The tool's own version rather than a literal, so a release
        // cannot ship a `new` whose pin the next `upgrade` immediately
        // moves — the stale pin this scaffold warns about, created by
        // the scaffold itself.
        None => format!("\"{VERSION}\""),
    };

    let mut files: Vec<(String, Vec<u8>)> = vec![
        (
            "Cargo.toml".into(),
            t::cargo_toml(name, &rahti, db, ws).into_bytes(),
        ),
        ("build.rs".into(), t::BUILD_RS.into()),
        (".cargo/config.toml".into(), t::CARGO_CONFIG.into()),
        (".gitignore".into(), t::gitignore(db.is_some()).into_bytes()),
        ("src/lib.rs".into(), t::lib_rs(db.is_some()).into_bytes()),
        ("src/main.rs".into(), t::main_rs(name).into_bytes()),
        // Born passing: the harness, and the first tests against the routes
        // the scaffold just wrote. A project that starts with a suite hands
        // the next coding agent a place to put the second test.
        (
            "src/tests/app.rs".into(),
            t::tests_rs(name, db.is_some()).into_bytes(),
        ),
        ("src/app/layout.rs".into(), t::layout_rs(name).into_bytes()),
        (
            "src/app/page.rs".into(),
            t::page_rs(name, tailwind).into_bytes(),
        ),
        (
            "src/app/globals.css".into(),
            if tailwind {
                t::GLOBALS_TAILWIND
            } else {
                t::GLOBALS_PLAIN
            }
            .into(),
        ),
        (
            "public/js/main.js".into(),
            t::main_js(tailwind).into_bytes(),
        ),
        (
            "public/js/pp-reactive-v2.min.js".into(),
            t::PP_RUNTIME.into(),
        ),
        ("public/favicon.ico".into(), t::FAVICON.into()),
        // The framework's own account of itself. Rahti is not in anyone's
        // training data, so a project without these files gives a coding
        // agent nothing to read — and the guide is for people just as much.
        (
            "AGENTS.md".into(),
            t::agents_md(name, db.is_some(), ws).into_bytes(),
        ),
        ("CLAUDE.md".into(), t::CLAUDE_MD.into()),
    ];

    for (doc, contents) in t::CORE_DOCS {
        files.push((format!("docs/conventions/{doc}"), contents.into()));
    }

    if ws {
        files.push((
            "docs/conventions/websockets.md".into(),
            t::DOC_WEBSOCKETS.into(),
        ));
    }

    if mcp {
        files.push((".mcp.json".into(), t::MCP_CONFIG.into()));
    }

    if tailwind {
        files.push((
            "public/js/tailwind-merge.mjs".into(),
            t::TAILWIND_MERGE.into(),
        ));
    }

    if db.is_some() {
        files.push((
            "docs/conventions/database.md".into(),
            t::DOC_DATABASE.into(),
        ));
        files.push(("src/db.rs".into(), t::DB_RS.into()));
        files.push(("src/models/todo.rs".into(), t::MODEL_TODO_RS.into()));
        files.push((
            "src/migrations/m20260101_000001_create_todo.rs".into(),
            t::MIGRATION_TODO.into(),
        ));
    }

    // `.env` is deliberately not here — see `UNTRACKED`.
    files
}

/// Files the scaffold creates but does not own afterwards.
///
/// The stylesheet is committed, so a clone with no Tailwind CLI still serves
/// CSS — which means it has to exist before the first build. From that build
/// onward it belongs to `rahti-build`, so it is deliberately kept out of the
/// ledger: a hash taken now is wrong by the time anyone could compare it, and
/// an upgrade would read that as "the author edited this" for the rest of the
/// project's life.
/// `.env` joins it for a different reason with the same shape: it is written
/// once so `cargo run` works immediately, and from then on it holds a real
/// connection string. Tracking it would have `upgrade` compare a hash against
/// somebody's credentials and report them as an edited scaffold file forever
/// — and, worse, would make `.env` a file the scaffold believes it may
/// rewrite.
///
/// Nothing is committed beside it: what a clone has to supply is the
/// environment reference at <https://rahti.dev>, which is written once and so
/// cannot drift per project.
const UNTRACKED: [(&str, &str); 1] = [("public/css/styles.css", "")];

/// Everything but the config, which needs the ledger this fills in.
fn write_project(
    root: &Path,
    options: &Options,
    tailwind: bool,
    db: Option<Backend>,
    ws: bool,
    mcp: bool,
    ledger: &mut Ledger,
) -> Result<(), String> {
    for (path, contents) in project_files(
        &options.name,
        tailwind,
        db,
        ws,
        mcp,
        options.local.as_deref(),
    ) {
        binary(root, &path, &contents, ledger)?;
    }

    let mut untracked: Vec<(String, Vec<u8>)> = UNTRACKED
        .iter()
        .map(|(path, contents)| ((*path).to_string(), contents.as_bytes().to_vec()))
        .collect();

    // Written whether or not the project has a database: `AUTH_SECRET` is not
    // a database setting, and a project whose `.env` only appeared when you
    // chose SQLite would be a trap.
    let values = t::EnvValues::generate();
    untracked.push((".env".to_string(), t::env(db, &values).into_bytes()));

    for (path, contents) in untracked {
        let mut throwaway = Ledger::new();
        binary(root, &path, &contents, &mut throwaway)?;
    }

    // `src/components/` is where `rahti-build` writes a generated `mod.rs`,
    // and `src/main.rs` declares the module unconditionally — so the
    // directory has to exist before the first build, empty or not.
    dir(root, "src/components")?;

    Ok(())
}

/// `rahti.config.json`, written last because it describes everything above.
pub fn write_config(
    root: &Path,
    tailwind: bool,
    db: Option<Backend>,
    ws: bool,
    mcp: bool,
    ledger: &Ledger,
) -> Result<(), String> {
    let engine = if tailwind { "tailwind" } else { "plain" };

    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"$schema\": \"https://rahti.dev/schema/1.json\",\n");
    out.push_str("  \"schema\": 1,\n");
    out.push_str(&format!("  \"createdWith\": \"{VERSION}\",\n\n"));

    out.push_str("  \"app\": {\n    \"dir\": \"src/app\",\n    \"public\": \"public\"\n  },\n\n");

    // `0.0.0.0` rather than the loopback, because it is the one address that
    // is correct in both places a project runs. Loopback develops fine and
    // then fails in a container: a platform routes traffic in from outside
    // the process, so a server bound to `127.0.0.1` accepts nothing and fails
    // the health check without logging a refusal — the failure that costs an
    // afternoon because nothing reports it. The wildcard develops equally
    // well, and `local_url` prints it as `localhost` so the startup line is
    // still somewhere a browser can go.
    //
    // The cost is that a development server answers on the local network. A
    // project that does not want that writes the loopback here, or sets
    // `HOST` — the same override a deployment would use.
    out.push_str("  \"server\": {\n    \"host\": \"0.0.0.0\",\n    \"port\": 3000\n  },\n\n");

    // Recorded only when chosen, like `db`: absence is the answer, and an
    // `upgrade` reads this to know the manifest carries rahti's `ws` feature.
    // The build itself never needs the key — a `#[socket]` in the tree is
    // what wires the endpoint.
    if ws {
        out.push_str("  \"ws\": true,\n\n");
    }
    if mcp {
        out.push_str("  \"mcp\": true,\n\n");
    }

    out.push_str("  \"css\": {\n");
    out.push_str(&format!("    \"engine\": \"{engine}\",\n"));
    out.push_str("    \"entry\": \"src/app/globals.css\",\n");
    out.push_str("    \"output\": \"public/css/styles.css\"");
    if tailwind {
        // Only the Tailwind engine reads these, and a plain project carrying
        // a pinned compiler version it never runs invites the question of
        // why it is there.
        out.push_str(",\n    \"version\": \"4.3.3\",\n    \"download\": true");
    }
    out.push_str("\n  },\n\n");

    // Only when there is one. An app with no database carrying an empty `db`
    // object invites the question of what it is for, and the build reads its
    // absence as the answer rather than as a gap.
    //
    // The connection string is deliberately not here: it is a credential, and
    // this file is committed. `src/db.rs` reads DATABASE_URL from `.env`.
    if let Some(backend) = db {
        out.push_str("  \"db\": {\n");
        out.push_str(&format!("    \"backend\": \"{}\",\n", backend.label()));
        out.push_str("    \"models\": \"src/models\",\n");
        out.push_str("    \"migrations\": \"src/migrations\"\n");
        out.push_str("  },\n\n");
    }

    out.push_str("  \"scaffold\": {\n");
    let mut entries = ledger.iter().peekable();
    while let Some((path, hash)) = entries.next() {
        let comma = if entries.peek().is_some() { "," } else { "" };
        out.push_str(&format!("    \"{path}\": \"{hash}\"{comma}\n"));
    }
    out.push_str("  }\n}\n");

    fs::write(root.join("rahti.config.json"), &out)
        .map_err(|e| format!("cannot write rahti.config.json: {e}"))
}

// ------------------------------------------------------------------ writing

/// Write one file and record it.
///
/// Hashing the bytes rather than the template means the ledger describes what
/// is on disk, which is the only thing a later comparison can be against.
fn binary(root: &Path, path: &str, contents: &[u8], ledger: &mut Ledger) -> Result<(), String> {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    fs::write(&full, contents).map_err(|e| format!("cannot write {path}: {e}"))?;

    ledger.insert(path.to_string(), sha256::hex(contents));
    Ok(())
}

fn dir(root: &Path, path: &str) -> Result<(), String> {
    let full = root.join(path);
    fs::create_dir_all(&full).map_err(|e| format!("cannot create {}: {e}", full.display()))
}

// ------------------------------------------------------------------ options

/// What the command line said.
///
/// Every optional feature is one flag meaning "add this", and its absence
/// means "do not". One spelling per answer, so there is no pair of opposing
/// flags to reconcile when both are given.
struct Options {
    name: String,
    /// `--tailwind` was given. False means it was not, which is the answer.
    tailwind: bool,
    /// `--db [backend]` was given, and which backend it named.
    db: Option<Backend>,
    /// `--ws` was given: the project gets rahti's `ws` feature and the
    /// `#[socket]` attribute with it.
    ws: bool,
    /// `--mcp` was given: write the client configuration for `rahti-mcp`.
    mcp: bool,
    /// A Rahti checkout to depend on by path, for framework development.
    local: Option<String>,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self, String> {
        let mut name: Option<String> = None;
        let mut tailwind = false;
        let mut db: Option<Backend> = None;
        let mut ws = false;
        let mut mcp = false;
        let mut local: Option<String> = None;

        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            match *arg {
                "--tailwind" => tailwind = true,
                "--ws" => ws = true,
                "--mcp" => mcp = true,
                // A bare `--db` is SQLite: it is the backend that needs no
                // server running, so it is the one worth having as the short
                // form.
                "--db" => {
                    let named = rest.clone().next().filter(|a| !a.starts_with('-'));
                    match named {
                        Some(name) => {
                            rest.next();
                            db = Some(backend_of(name)?);
                        }
                        None => db = Some(Backend::Sqlite),
                    }
                }
                "--local" => {
                    local = Some(
                        rest.next()
                            .ok_or("--local needs a path to a Rahti checkout")?
                            .to_string(),
                    );
                }
                other if other.starts_with('-') => {
                    return Err(format!("`{other}` is not an option of `new`."));
                }
                other if name.is_none() => name = Some(other.to_string()),
                other => return Err(format!("unexpected argument `{other}`")),
            }
        }

        let name = name.ok_or("`new` needs a project name: cargo rahti new my-app")?;
        check_name(&name)?;

        Ok(Options {
            name,
            tailwind,
            db,
            ws,
            mcp,
            local,
        })
    }
}

/// The name becomes a cargo package and a directory, so refuse here what one
/// of those would refuse later with a worse message.
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the project name is empty".to_string());
    }

    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err(format!(
            "`{name}` cannot be a cargo package name.\n       \
             Use letters, digits, `-` and `_`."
        ));
    }

    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return Err(format!(
            "`{name}` starts with a digit, which cargo will not accept."
        ));
    }

    Ok(())
}

fn report(name: &str, tailwind: bool, db: Option<Backend>, ws: bool, mcp: bool) {
    let engine = if tailwind {
        "Tailwind CSS"
    } else {
        "plain CSS"
    };
    let sockets = if ws { ", with WebSockets" } else { "" };

    match db {
        Some(backend) => println!(
            "\n  Created `{name}`, styled with {engine}, on {}{sockets}.\n",
            backend.label()
        ),
        None => println!("\n  Created `{name}`, styled with {engine}{sockets}.\n"),
    }

    println!("    cd {name}");

    // A backend that needs a server has to be pointed at one before the first
    // run, and the run that fails without this line fails at startup with a
    // connection error — readable, but avoidable.
    if db.is_some_and(|b| b != Backend::Sqlite) {
        println!("    # then set DATABASE_URL in .env");
    }

    println!("    cargo run\n");
    // Not the bind address: the project binds `0.0.0.0`, which is a sentence
    // about listening rather than a destination. This is the same name
    // `local_url` prints once the server is up.
    println!("  Then open http://localhost:3000 and edit src/app/page.rs.\n");
    println!("  `cargo dev` runs the same server but rebuilds and restarts it on");
    println!("  every edit, and the open tab reloads itself. It needs cargo-watch");
    println!("  installed once: `cargo install cargo-watch`.\n");
    println!("  AGENTS.md is the project guide, and docs/conventions/ documents the");
    println!("  framework — written for coding agents and the people beside them,");
    println!("  and kept current by `cargo rahti upgrade`.\n");

    if ws {
        println!("  WebSockets are on: `rahti` carries its `ws` feature, so mark an");
        println!("  async function `#[socket]` beside the page whose script opens it,");
        println!("  give it a final `socket: rahti::ws::Socket` parameter, and connect");
        println!("  from the browser with `pp.socket(\"name\", {{ … }}, {{ onMessage }})`.\n");
    }

    if db.is_some() {
        println!("  The database is in src/models/ — one file per table — with the");
        println!("  migrations that create them in src/migrations/. Both are wired up");
        println!("  from their contents, so adding a file is the whole of adding a");
        println!("  table. See docs/conventions/database.md.\n");
    }
    if mcp {
        println!("  MCP is configured in .mcp.json. Install its server once:");
        println!("    cargo install rahti-mcp\n");
    }
}
