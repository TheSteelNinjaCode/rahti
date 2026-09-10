//! `cargo rahti upgrade` — bring a project's scaffolded files up to date.
//!
//! The framework's own files drift: a bug is fixed in the root layout, the
//! client runtime gains a version, `build.rs` learns a new call. A project
//! scaffolded last year has the old ones and no way to tell which of its
//! files are still the scaffold's and which it has made its own.
//!
//! That is what the ledger in `rahti.config.json` answers. Every file is
//! classified by comparing the bytes on disk with the hash recorded when the
//! scaffold wrote it:
//!
//! - the hashes agree — nobody has touched it, so it is ours to replace;
//! - they disagree — the author edited it, so it is theirs and is left alone;
//! - the file is gone — the author deleted it, which is also an answer.
//!
//! Only the first is written to. An upgrade that cannot tell these apart has
//! to choose between never fixing anything and overwriting work, and both are
//! the wrong answer.
//!
//! The scaffold's questions do not end at `new`: a project that started
//! without a database, WebSockets or MCP can gain them here. A flag — `--db`,
//! `--ws`, `--mcp`, the same spellings `new` takes — says yes outright; an interactive
//! run is asked about whatever the project does not have, defaulting to no; a
//! dry run changes nothing and so asks nothing. The feature's files are
//! written, the choice is recorded in `rahti.config.json`, and the two files
//! that are never regenerated — `Cargo.toml` and `.env` — are amended in
//! place by the smallest edit that makes the new files compile and run: see
//! `wiring.rs`, which is also where the reason a printed instruction is not
//! good enough is written down. The CSS engine is deliberately not offered:
//! switching it rewrites an authored stylesheet, which is a migration, not an
//! addition.

use std::fs;
use std::path::{Path, PathBuf};

use rahti_build::{Backend, sha256};

use crate::VERSION;
use crate::new::{Ledger, backend_of, project_files};
use crate::prompt::{choose, confirm};
use crate::templates as t;
use crate::wiring;

/// Dependency versions are cargo's business, not the scaffold's.
///
/// Regenerating this from the template would undo whatever the author added,
/// and — for a project created with `--local` — would replace a working path
/// dependency with a version that may not be published. It stays in the
/// ledger, because knowing whether it is pristine is still worth something,
/// but the template's copy is never written over it.
///
/// Not the same thing as never being touched: a feature this run added needs
/// a dependency, and `wiring` appends that one line without disturbing the
/// rest of the file.
const NEVER_REWRITTEN: [&str; 1] = ["Cargo.toml"];

/// Whether a bulk `--force` leaves `path` alone as the author's.
///
/// The rule is the project's own shape. Everything under `src/` is the
/// application — pages, layouts, entities, migrations, the connection — and
/// the two startup files are the exception: `src/lib.rs` and `src/main.rs` are
/// the startup the framework prescribes, and the files a stale copy actually
/// breaks. So a bulk force sweeps the framework's own files and stops at the
/// door of `src/`.
///
/// `src/lib.rs` is on the list because it is what a native package calls into:
/// a project whose `initialize_application` is missing or old cannot be
/// packaged, and `cargo rahti native doctor` sends people here to fix it.
///
/// Taking an application file back is still possible, but it has to be asked
/// for by name: `--force src/app/page.rs` is a sentence somebody meant to
/// write, and a bare `--force` that quietly deleted a homepage is not.
fn authored(path: &str) -> bool {
    path.starts_with("src/") && !matches!(path, "src/main.rs" | "src/lib.rs")
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let options = Options::parse(args)?;

    let root = PathBuf::from(".");
    let mut config = Config::read(&root)?;

    // Features are added the way `new` adds them: a flag says yes and skips
    // the question, an interactive run is asked about whatever the project
    // does not have, and anywhere with nobody to answer takes the default —
    // no. A dry run never asks: it changes nothing, so it has no question to
    // put; flags still count, so `--dry-run --ws` previews the addition.
    let added_db = match (config.db, options.db) {
        (Some(_), _) => None,
        (None, Some(backend)) => Some(backend),
        (None, None) if !options.dry_run && confirm("Add a database?") => Some(backend_of(
            &choose("Which one?", &["sqlite", "postgres", "mysql"]),
        )?),
        (None, None) => None,
    };
    if added_db.is_some() {
        config.db = added_db;
    }

    let added_ws = !config.ws && (options.ws || (!options.dry_run && confirm("Add WebSockets?")));
    if added_ws {
        config.ws = true;
    }
    let added_mcp =
        !config.mcp && (options.mcp || (!options.dry_run && confirm("Add MCP tooling?")));
    if added_mcp {
        config.mcp = true;
    }

    let dry_run = options.dry_run;

    let files = project_files(
        &config.name,
        config.tailwind,
        config.db,
        config.ws,
        config.mcp,
        config.local.as_deref(),
    );
    // A named path is checked against the scaffold before anything is
    // written. A typo that silently forced nothing would report a clean
    // upgrade and leave the file exactly as it was, which is the one outcome
    // somebody typing `--force` cannot afford to misread.
    options.check_forced(&files)?;

    let mut plan = Plan::default();
    let mut ledger = config.ledger.clone();

    for (path, wanted) in &files {
        let full = root.join(path);
        let recorded = config.ledger.get(path);
        let actual = fs::read(&full).ok();

        match (recorded, actual) {
            // Known to the ledger and still on disk: whose is it?
            (Some(hash), Some(bytes)) => {
                let theirs = &sha256::hex(&bytes) != hash;
                if theirs && !options.forces(path) {
                    plan.yours.push(path.clone());
                } else if bytes == *wanted {
                    // Already the template's text. A forced file lands here
                    // once it has been taken, so a second `--force` re-adopts
                    // it into the ledger and says nothing.
                    plan.current.push(path.clone());
                    ledger.insert(path.clone(), sha256::hex(wanted));
                } else if NEVER_REWRITTEN.contains(&path.as_str()) {
                    plan.skipped.push(path.clone());
                } else {
                    // Same write either way; reported apart, because one of
                    // them destroys work and the other does not.
                    if theirs {
                        plan.forced.push(path.clone());
                    } else {
                        plan.updated.push(path.clone());
                    }
                    ledger.insert(path.clone(), sha256::hex(wanted));
                }
            }
            // Recorded, but gone. Deleting a file is a decision, and putting
            // it back is not this command's to make.
            (Some(_), None) => plan.deleted.push(path.clone()),
            // Not recorded: a file a newer Rahti adds to the project. If
            // something is already there under that name it is not ours.
            (None, actual) => {
                if actual.is_some() {
                    plan.yours.push(path.clone());
                } else if NEVER_REWRITTEN.contains(&path.as_str()) {
                    plan.skipped.push(path.clone());
                } else {
                    plan.added.push(path.clone());
                    ledger.insert(path.clone(), sha256::hex(wanted));
                }
            }
        }
    }

    if dry_run {
        let wiring = wire(&root, &config, true)?;
        report(
            &plan,
            &config,
            &wiring,
            true,
            added_db,
            added_ws,
            added_mcp,
            options.force.as_deref(),
        );
        return Ok(());
    }

    for (path, contents) in &files {
        if plan.writes(path) {
            let full = root.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
            }
            fs::write(&full, contents).map_err(|e| format!("cannot write {path}: {e}"))?;
        }
    }

    // After the files, because this is what makes them compile: `src/db.rs`
    // was just written and it opens with `use sea_orm::…`.
    let wiring = wire(&root, &config, false)?;

    // A manifest that was the scaffold's before this run is still the
    // scaffold's after it: the dependency added above was added by this
    // command, and leaving the old hash in the ledger would have the next run
    // read its own work as an edit of the author's and say so.
    //
    // A manifest the author had already changed keeps its recorded hash and
    // stays theirs, which is the answer it was before.
    let pristine = plan.current.contains(&"Cargo.toml".to_string())
        || plan.skipped.contains(&"Cargo.toml".to_string());
    if let Some(bytes) = &wiring.manifest
        && pristine
    {
        ledger.insert("Cargo.toml".to_string(), sha256::hex(bytes));
    }

    // Written even when nothing else was, because `createdWith` moves with
    // the tool that last touched the project.
    rewrite_config(&root, &ledger, added_db, added_ws, added_mcp)?;

    report(
        &plan,
        &config,
        &wiring,
        false,
        added_db,
        added_ws,
        added_mcp,
        options.force.as_deref(),
    );
    Ok(())
}

// ------------------------------------------------------------------- wiring

/// What the manifest and `.env` needed to match the config, and what could
/// not be done.
#[derive(Default)]
struct Wiring {
    /// Dependency lines added to `[dependencies]` or `[dev-dependencies]`,
    /// as written.
    dependencies: Vec<String>,
    /// The `ws` feature folded into the `rahti` dependency.
    ws_feature: bool,
    /// The `mcp` feature folded into the `rahti` dependency.
    mcp_feature: bool,
    /// The files `DATABASE_URL` was added to.
    env: Vec<String>,
    /// The amended manifest, when there was one to amend. Handed back so the
    /// ledger can record it: the edit was the scaffold's, and a hash left
    /// describing the file as it was would have the next run report this
    /// command's own work as the author's.
    manifest: Option<Vec<u8>>,
    /// Framework pins moved up to this tool's version, as `name was -> now`.
    bumps: Vec<String>,
    /// What the author has to do by hand, and why.
    manual: Vec<Manual>,
}

/// One thing this could not do, with the line that does it.
struct Manual {
    reason: String,
    line: String,
}

impl Wiring {
    fn is_empty(&self) -> bool {
        self.dependencies.is_empty()
            && !self.ws_feature
            && !self.mcp_feature
            && self.env.is_empty()
            && self.bumps.is_empty()
    }
}

/// Bring `Cargo.toml` and `.env` up to what `rahti.config.json` says.
///
/// Driven by the config rather than by what this run added, so it repairs as
/// well as completes: a project whose config already says `db` and whose
/// manifest has no `sea-orm` — the state an earlier version of this command
/// left behind, or a hand-edited config — is fixed by running `upgrade`
/// again. Every edit is skipped when it is already there, so that is safe to
/// do any number of times.
///
/// A dry run computes all of this and writes none of it.
fn wire(root: &Path, config: &Config, dry_run: bool) -> Result<Wiring, String> {
    let mut wiring = Wiring::default();

    let path = root.join("Cargo.toml");
    let original = fs::read_to_string(&path).map_err(|e| format!("cannot read Cargo.toml: {e}"))?;
    let mut manifest = original.clone();

    // Before the feature wiring, and unconditional: the files this run just
    // wrote are the tool's, and they are written against the tool's own
    // version. A pin left behind is the failure that hides best — the project
    // compiles, runs locally on its configured port, and quietly does none of
    // what its new `main.rs` says, because the `rahti` under it is older than
    // the file above it.
    for name in ["rahti", "rahti-build"] {
        match wiring::with_version(&manifest, name, VERSION) {
            Ok(Some((amended, was))) => {
                manifest = amended;
                wiring.bumps.push(format!("{name} {was} -> {VERSION}"));
            }
            Ok(None) => {}
            Err(reason) => wiring.manual.push(Manual {
                reason,
                line: format!("{name} = \"{VERSION}\""),
            }),
        }
    }

    if let Some(backend) = config.db {
        for (name, line) in t::sea_orm_dependencies(backend) {
            if let Some(amended) = wiring::with_dependency(&manifest, name, &line) {
                manifest = amended;
                wiring.dependencies.push(line);
            }
        }
    }

    // The test suite's dev-dependencies, unconditional like the version pins:
    // every project has a `src/tests/` (this run may just have written it),
    // and the harness in it drives the router through these two. A manifest
    // the author edited before there was a suite gains them here, so the
    // suite compiles rather than arriving with instructions.
    for (name, line) in t::DEV_DEPENDENCIES {
        if let Some(amended) = wiring::with_dev_dependency(&manifest, name, line) {
            manifest = amended;
            wiring.dependencies.push(line.to_string());
        }
    }

    if config.ws {
        match wiring::with_ws_feature(&manifest) {
            Ok(Some(amended)) => {
                manifest = amended;
                wiring.ws_feature = true;
            }
            Ok(None) => {}
            Err(reason) => wiring.manual.push(Manual {
                reason,
                line: "rahti = { version = \"…\", features = [\"ws\"] }".to_string(),
            }),
        }
    }

    if config.mcp {
        match wiring::with_mcp_feature(&manifest) {
            Ok(Some(amended)) => {
                manifest = amended;
                wiring.mcp_feature = true;
            }
            Ok(None) => {}
            Err(reason) => wiring.manual.push(Manual {
                reason,
                line: "rahti = { version = \"…\", features = [\"mcp\"] }".to_string(),
            }),
        }
    }

    if manifest != original {
        if !dry_run {
            fs::write(&path, &manifest).map_err(|e| format!("cannot write Cargo.toml: {e}"))?;
        }
        wiring.manifest = Some(manifest.into_bytes());
    }

    // Only `.env`, and only when it is there. A `.env` the author deleted is a
    // decision — the same one an absent scaffold file records — and inventing
    // one here would write a connection string with none of the session
    // settings a running project also needs.
    if let Some(backend) = config.db {
        let name = ".env";
        let full = root.join(name);
        if let Ok(current) = fs::read_to_string(&full) {
            if let Some(amended) = wiring::with_database_url(&current, backend) {
                if !dry_run {
                    fs::write(&full, amended).map_err(|e| format!("cannot write {name}: {e}"))?;
                }
                wiring.env.push(name.to_string());
            }
        }

        if !root.join(".env").exists() {
            wiring.manual.push(Manual {
                reason: "this project has no `.env`".to_string(),
                line: format!("DATABASE_URL=\"{}\"", t::database_url(backend)),
            });
        }
    }

    Ok(wiring)
}

// ------------------------------------------------------------------ options

/// What the command line said.
///
/// The feature flags are the ones `new` takes, with the same meaning: each
/// adds something, and leaving it out is how you say no — the question an
/// interactive run asks defaults the same way.
struct Options {
    dry_run: bool,
    /// `--db [backend]`: add a database, if the project has none.
    db: Option<Backend>,
    /// `--ws`: add WebSockets, if the project does not have them.
    ws: bool,
    /// `--mcp`: enable local stdio and optional public HTTP MCP.
    mcp: bool,
    /// `--force`, and the paths that followed it. `None` is the ordinary
    /// upgrade, which never takes an edited file. `Some([])` is the bulk
    /// form, which takes the framework's files and leaves the application's;
    /// a non-empty list takes exactly what it names, application or not.
    force: Option<Vec<String>>,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self, String> {
        let mut dry_run = false;
        let mut db: Option<Backend> = None;
        let mut ws = false;
        let mut mcp = false;
        let mut force: Option<Vec<String>> = None;

        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            match *arg {
                "--dry-run" | "-n" => dry_run = true,
                "--ws" => ws = true,
                "--mcp" => mcp = true,
                // Every following argument that is not itself an option is a
                // path, so `--force a b --dry-run` names two files.
                "--force" | "-f" => {
                    let mut paths = Vec::new();
                    while let Some(next) = rest.clone().next().filter(|a| !a.starts_with('-')) {
                        rest.next();
                        // Typed at a Windows prompt, a path arrives with
                        // backslashes; the ledger's keys never do.
                        paths.push(next.replace('\\', "/"));
                    }
                    force = Some(paths);
                }
                // A bare `--db` is SQLite, the same short form `new` has.
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
                other => return Err(format!("`{other}` is not an option of `upgrade`.")),
            }
        }

        Ok(Options {
            dry_run,
            db,
            ws,
            mcp,
            force,
        })
    }

    /// Whether this run takes `path` back from the author.
    ///
    /// Only ever asked about a file the author has edited — a file that is
    /// still the scaffold's is rewritten by the ordinary rules, with or
    /// without a force.
    fn forces(&self, path: &str) -> bool {
        if NEVER_REWRITTEN.contains(&path) {
            return false;
        }
        match &self.force {
            None => false,
            Some(paths) if paths.is_empty() => !authored(path),
            Some(paths) => paths.iter().any(|named| named == path),
        }
    }

    /// Refuse a named path the scaffold cannot restore.
    ///
    /// Two ways to name one that does not work, and both look identical in
    /// the report if they are let through: a file this project's scaffold
    /// never wrote, and the manifest, which has a template but is deliberately
    /// never written over it.
    fn check_forced(&self, files: &[(String, Vec<u8>)]) -> Result<(), String> {
        let Some(paths) = &self.force else {
            return Ok(());
        };
        for named in paths {
            if NEVER_REWRITTEN.contains(&named.as_str()) {
                return Err(format!(
                    "`{named}` is never rewritten, forced or not — your dependencies\n       \
                     are yours. A feature you add amends it instead."
                ));
            }
            if !files.iter().any(|(path, _)| path == named) {
                return Err(format!(
                    "`{named}` is not a scaffold file of this project, so there is no\n       \
                     template to restore it from. The files the scaffold owns are the\n       \
                     keys under `scaffold` in rahti.config.json."
                ));
            }
        }
        Ok(())
    }
}

// ------------------------------------------------------------------- rewrite

/// Update `rahti.config.json` in place, touching only what the upgrade owns.
///
/// The config has two authors, and almost all of it belongs to the other one:
/// the port, the app directory, a pinned CLI hash, `minify` — settings this
/// command never asked about and has no answer for. `new` writes the whole
/// file because there is nothing to preserve yet; an upgrade that did the
/// same would reset every one of those to the scaffold's defaults, quietly
/// undoing configuration in the name of refreshing files. So the file's text
/// is kept as it stands, and the two keys the scaffold owns — `createdWith`
/// and the ledger — are replaced where they sit.
fn rewrite_config(
    root: &Path,
    ledger: &Ledger,
    added_db: Option<Backend>,
    added_ws: bool,
    added_mcp: bool,
) -> Result<(), String> {
    let path = root.join("rahti.config.json");
    let raw =
        fs::read_to_string(&path).map_err(|e| format!("cannot read rahti.config.json: {e}"))?;

    // A config that never had the key — hand-written, most likely — is left
    // without one rather than having a key invented into someone else's file.
    let raw = replace_string_value(&raw, "createdWith", VERSION).unwrap_or(raw);
    let raw = record_features(raw, added_db, added_ws, added_mcp);
    let raw = replace_scaffold(&raw, ledger)?;

    fs::write(&path, raw).map_err(|e| format!("cannot write rahti.config.json: {e}"))
}

/// The keys a feature added this run writes into the config, in the shape
/// `new` writes them.
///
/// Inserted immediately above `"scaffold"` — the one key guaranteed to be
/// there, since `Config::read` refused the project without it — so the rest
/// of the file stays the author's, byte for byte. JSON does not care about
/// the order, and a reader finds the keys beside the ledger that ships them.
fn record_features(
    raw: String,
    added_db: Option<Backend>,
    added_ws: bool,
    added_mcp: bool,
) -> String {
    let mut block = String::new();
    if added_ws {
        block.push_str("\"ws\": true,\n\n  ");
    }
    if added_mcp {
        block.push_str("\"mcp\": true,\n\n  ");
    }
    if let Some(backend) = added_db {
        block.push_str(&format!(
            "\"db\": {{\n    \"backend\": \"{}\",\n    \"models\": \"src/models\",\n    \
             \"migrations\": \"src/migrations\"\n  }},\n\n  ",
            backend.label()
        ));
    }
    if block.is_empty() {
        return raw;
    }
    match raw.find("\"scaffold\"") {
        Some(at) => format!("{}{block}{}", &raw[..at], &raw[at..]),
        None => raw,
    }
}

/// The string value of `key` replaced with `value`, everything else untouched.
///
/// `None` when the key is not in the file. Found by text rather than parsed
/// and re-serialized, because a parser round trip rewrites the whole file —
/// reordering keys and dropping blank lines — and the point of this function
/// is that the file stays the author's.
fn replace_string_value(raw: &str, key: &str, value: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let at = raw.find(&needle)?;
    let after = at + needle.len();
    let colon = after + raw[after..].find(':')?;
    let open = colon + raw[colon..].find('"')?;
    let close = open + 1 + raw[open + 1..].find('"')?;
    Some(format!("{}\"{value}\"{}", &raw[..open], &raw[close + 1..]))
}

/// The `scaffold` object replaced with the ledger as it now stands.
///
/// The object holds only string values, so the first `}` after the opening
/// brace closes it — there is nothing nested to skip. The replacement is
/// written in the shape `new` writes, which keeps an untouched config
/// byte-identical across the round trip.
fn replace_scaffold(raw: &str, ledger: &Ledger) -> Result<String, String> {
    // Unreachable in practice: `Config::read` refused the project already if
    // the ledger was missing. Kept as an error rather than a panic because
    // the file may have changed between that read and this write.
    let missing = || {
        "rahti.config.json has no scaffold ledger to rewrite.\n       \
         Was the file edited while the upgrade ran?"
            .to_string()
    };

    let at = raw.find("\"scaffold\"").ok_or_else(missing)?;
    let open = at + raw[at..].find('{').ok_or_else(missing)?;
    let close = open + raw[open..].find('}').ok_or_else(missing)?;

    let mut block = String::from("{\n");
    let mut entries = ledger.iter().peekable();
    while let Some((path, hash)) = entries.next() {
        let comma = if entries.peek().is_some() { "," } else { "" };
        block.push_str(&format!("    \"{path}\": \"{hash}\"{comma}\n"));
    }
    block.push_str("  }");

    Ok(format!("{}{block}{}", &raw[..open], &raw[close + 1..]))
}

// --------------------------------------------------------------------- plan

#[derive(Default)]
struct Plan {
    /// Ours, out of date, rewritten.
    updated: Vec<String>,
    /// New in this version of Rahti, created.
    added: Vec<String>,
    /// Ours and already current.
    current: Vec<String>,
    /// The author's, taken back because `--force` said to.
    forced: Vec<String>,
    /// The author's, left alone.
    yours: Vec<String>,
    /// Recorded but no longer on disk.
    deleted: Vec<String>,
    /// Ours and out of date, but never rewritten by policy.
    skipped: Vec<String>,
}

impl Plan {
    /// Whether this run writes `path`.
    ///
    /// Three lists reach the same write: ours and out of date, new to the
    /// project, and the author's but taken back by `--force`. They are
    /// separate in the report because they read differently to whoever is
    /// watching — a plan that reported a file and then did not write it would
    /// be worse than either.
    fn writes(&self, path: &str) -> bool {
        self.updated
            .iter()
            .chain(&self.added)
            .chain(&self.forced)
            .any(|written| written == path)
    }
}

fn report(
    plan: &Plan,
    config: &Config,
    wiring: &Wiring,
    dry_run: bool,
    added_db: Option<Backend>,
    added_ws: bool,
    added_mcp: bool,
    force: Option<&[String]>,
) {
    fn verb<'a>(dry_run: bool, past: &'a str, future: &'a str) -> &'a str {
        if dry_run { future } else { past }
    }
    let verb = |past, future| verb(dry_run, past, future);

    println!();
    for path in &plan.updated {
        println!("  {} {path}", verb("updated", "would update"));
    }
    for path in &plan.added {
        println!("  {} {path}", verb("added", "would add"));
    }
    // Named one at a time even in bulk, because this is the only line in the
    // report that stands for work being destroyed.
    for path in &plan.forced {
        println!(
            "  {} {path} — yours, replaced by --force",
            verb("forced ", "would force")
        );
    }
    for path in &plan.yours {
        println!("  kept    {path} — yours, left as it is");
    }
    for path in &plan.deleted {
        println!("  absent  {path} — you removed it, so it stays removed");
    }
    for path in &plan.skipped {
        println!("  skipped {path} — the template's copy is not yours to take");
    }

    // The files that were amended rather than replaced, named by the one
    // thing that changed in each. A line here is why the project compiles
    // after gaining a feature, so it is reported beside the writes and not
    // as a footnote.
    // First of the amendments, because it is the one that applies to every
    // project rather than to a feature being added.
    for bump in &wiring.bumps {
        println!("  {} Cargo.toml — {bump}", verb("wired  ", "would wire"));
    }
    for line in &wiring.dependencies {
        let name = line.split_whitespace().next().unwrap_or(line);
        println!("  {} Cargo.toml — {name}", verb("wired  ", "would wire"));
    }
    if wiring.ws_feature {
        println!(
            "  {} Cargo.toml — the `ws` feature on rahti",
            verb("wired  ", "would wire")
        );
    }
    if wiring.mcp_feature {
        println!(
            "  {} Cargo.toml — the `mcp` feature on rahti",
            verb("wired  ", "would wire")
        );
    }
    for name in &wiring.env {
        println!("  {} {name} — DATABASE_URL", verb("wired  ", "would wire"));
    }

    let changed = plan.updated.len() + plan.added.len() + plan.forced.len();
    println!();

    // What could not be wired, with the line that does it. Printed in both
    // modes: a dry run that hid this would promise an upgrade that lands
    // cleanly and then not deliver one.
    for item in &wiring.manual {
        println!(
            "  {}, so this is yours to add:\n\n    {}\n",
            item.reason, item.line
        );
    }

    if dry_run {
        println!(
            "  {changed} file(s) would change, {} already current, {} yours.\n",
            plan.current.len(),
            plan.yours.len()
        );
        if !plan.forced.is_empty() {
            println!(
                "  {} of those you had edited. Applying this replaces your version,\n  \
                 and nothing here keeps a copy.\n",
                plan.forced.len()
            );
        }
        // A feature previewed by flag has to be named again to land: the
        // apply run is a separate process with no memory of this one. A force
        // most of all — an apply line that dropped it would preview a
        // destructive run and hand back a harmless one.
        let mut apply = String::from("cargo rahti upgrade");
        if let Some(backend) = added_db {
            apply.push_str(&format!(" --db {}", backend.label()));
        }
        if added_ws {
            apply.push_str(" --ws");
        }
        if added_mcp {
            apply.push_str(" --mcp");
        }
        if let Some(paths) = force {
            apply.push_str(" --force");
            for path in paths {
                apply.push(' ');
                apply.push_str(path);
            }
        }
        println!("  Run `{apply}` to apply.\n");
        return;
    }

    // An amendment is a change even when no file was rewritten: a run that
    // moved a version pin and then said "already up to date" would be read as
    // having done nothing, which is how a stale pin survives an upgrade.
    if changed == 0 && wiring.is_empty() {
        println!("  Already up to date with cargo-rahti {VERSION}.\n");
    } else if changed == 0 {
        println!("  Upgraded to cargo-rahti {VERSION}.\n");
    } else {
        println!("  Upgraded to cargo-rahti {VERSION} — {changed} file(s) changed.\n");
    }

    // Before the note about the files that were kept, because this is the
    // half of the same sentence that cannot be undone from here.
    if !plan.forced.is_empty() {
        println!(
            "  {} file(s) you had edited were replaced. Your version is not\n  \
             kept anywhere — recover it from version control if you need it.\n",
            plan.forced.len()
        );
    }

    if !plan.yours.is_empty() {
        println!(
            "  {} file(s) you had edited were left alone. If a page misbehaves\n  \
             after this, compare them against a fresh `cargo rahti new`.\n",
            plan.yours.len()
        );
    }

    // The manifest changed under a build that has already run, and cargo will
    // not have noticed on its own.
    if !wiring.is_empty() {
        println!("  Cargo.toml or .env changed, so the next build fetches what is new:\n");
        println!("    cargo check\n");
    }

    // A pristine `src/main.rs` was just rewritten to connect at startup; an
    // edited one was left alone, and without these lines the database files
    // written above are not even compiled.
    if added_db.is_some() && plan.yours.contains(&"src/main.rs".to_string()) {
        println!(
            "  src/main.rs is yours, so wire the database in yourself: declare\n  \
             `mod db;`, `mod migrations;` and `mod models;`, and call\n  \
             `db::connect_if_configured().await` at the top of `main` — see\n  \
             docs/conventions/database.md.\n"
        );
    }

    if added_mcp {
        println!("  MCP is optional and now enabled. The app serves discovery at");
        println!("  /.well-known/mcp.json and Streamable HTTP at /mcp/. Set a strong");
        println!("  RAHTI_MCP_AUTH_TOKEN in the deployment environment before release.");
        println!("  Local stdio is configured in .mcp.json; install that server once:\n");
        println!("    cargo install rahti-mcp\n");
    }

    // A backend that needs a server has to be pointed at one. The line the
    // wiring wrote is an example, and for anything but SQLite it is a wrong
    // one until somebody edits it.
    if let Some(backend) = added_db
        && backend != Backend::Sqlite
        && wiring.env.iter().any(|name| name == ".env")
    {
        println!(
            "  DATABASE_URL in `.env` is an example. Point it at your {} server\n  \
             before the next run.\n",
            backend.label()
        );
    }

    if config.from != VERSION {
        println!(
            "  This project was created with cargo-rahti {}.\n",
            config.from
        );
    }
}

// ------------------------------------------------------------------- config

/// What `upgrade` needs from the project, gathered in one place.
struct Config {
    /// The cargo package name, which the templates put in the page title.
    /// Read from `Cargo.toml`, which is where a package's name lives — the
    /// scaffold does not keep a second copy to disagree with it.
    name: String,
    tailwind: bool,
    /// The backend `rahti.config.json` records, so an upgrade regenerates the
    /// files this project actually has rather than the ones a default project
    /// would.
    db: Option<Backend>,
    /// `"ws": true` in the config: this project uses WebSockets.
    ws: bool,
    /// `"mcp": true` in the config: this project opts into MCP support.
    mcp: bool,
    /// A `path` dependency on a Rahti checkout, recovered from `Cargo.toml`
    /// so a project made with `--local` stays that way.
    local: Option<String>,
    ledger: Ledger,
    /// `createdWith`, for the closing note.
    from: String,
}

impl Config {
    fn read(root: &Path) -> Result<Self, String> {
        let path = root.join("rahti.config.json");
        let raw = fs::read_to_string(&path).map_err(|e| {
            format!(
                "no rahti.config.json here ({e}).\n       \
                 Run this from the root of a Rahti project."
            )
        })?;

        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("rahti.config.json is not valid JSON: {e}"))?;

        // The same refusal the build makes, for the same reason: a newer
        // schema may mean something different by a key this version reads.
        if let Some(schema) = value.get("schema").and_then(|v| v.as_i64())
            && schema > 1
        {
            return Err(format!(
                "this project is written for config schema {schema}, and this \
                 cargo-rahti understands 1.\n       \
                 Update it with `cargo install cargo-rahti`."
            ));
        }

        let tailwind = value
            .get("css")
            .and_then(|c| c.get("engine"))
            .and_then(|e| e.as_str())
            .map(|e| e == "tailwind")
            .unwrap_or(true);

        let ledger: Ledger = value
            .get("scaffold")
            .and_then(|s| s.as_object())
            .map(|table| {
                table
                    .iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        if ledger.is_empty() {
            return Err(
                "rahti.config.json records no scaffolded files, so there is nothing \
                 this can safely replace.\n       \
                 A project created before the ledger existed has to be upgraded by hand."
                    .to_string(),
            );
        }

        let manifest = fs::read_to_string(root.join("Cargo.toml"))
            .map_err(|e| format!("cannot read Cargo.toml: {e}"))?;

        let db = match value
            .get("db")
            .and_then(|d| d.get("backend"))
            .and_then(|b| b.as_str())
        {
            Some("sqlite") => Some(Backend::Sqlite),
            Some("postgres") => Some(Backend::Postgres),
            Some("mysql") => Some(Backend::MySql),
            Some(other) => {
                return Err(format!(
                    "rahti.config.json names `{other}` as db.backend, which is not \
                     a backend.\n       \
                     Use \"sqlite\", \"postgres\" or \"mysql\"."
                ));
            }
            // A `db` object with no backend named is SQLite, the same reading
            // the build makes. No `db` object at all is no database.
            None if value.get("db").is_some_and(|d| d.is_object()) => Some(Backend::Sqlite),
            None => None,
        };

        Ok(Config {
            name: package_name(&manifest)
                .ok_or("Cargo.toml has no [package] name")?
                .to_string(),
            tailwind,
            db,
            ws: value.get("ws").and_then(|v| v.as_bool()).unwrap_or(false),
            mcp: value.get("mcp").and_then(|v| v.as_bool()).unwrap_or(false),
            local: local_checkout(&manifest),
            ledger,
            from: value
                .get("createdWith")
                .and_then(|v| v.as_str())
                .unwrap_or("an unknown version")
                .to_string(),
        })
    }
}

/// `name = "..."` from the `[package]` table.
///
/// Read by hand: pulling in a TOML parser to find one string in a file cargo
/// has already validated is a poor trade.
fn package_name(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package && let Some(rest) = line.strip_prefix("name") {
            return rest.split('"').nth(1);
        }
    }
    None
}

/// The checkout a `--local` project points at, so an upgrade regenerates the
/// same path dependency instead of a published version that may not exist.
fn local_checkout(manifest: &str) -> Option<String> {
    let line = manifest
        .lines()
        .find(|l| l.trim_start().starts_with("rahti ="))?;
    let path = line.split("path = \"").nth(1)?.split('"').next()?;
    path.strip_suffix("/crates/rahti").map(str::to_string)
}

#[cfg(test)]
#[path = "tests/upgrade.rs"]
mod tests;
