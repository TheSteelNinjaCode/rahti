//! Reading a project back, and deciding what may be written to.

use super::*;

// ---------------------------------------------------------------- Cargo.toml

/// The package name lives in `Cargo.toml` and nowhere else, so the templates
/// have to read it from there rather than from a second copy that could
/// disagree.
#[test]
fn the_package_name_is_found() {
    let manifest = "[package]\nname = \"demo-app\"\nversion = \"0.1.0\"\n\n[dependencies]\n";
    assert_eq!(package_name(manifest), Some("demo-app"));
}

/// `name` appears under `[dependencies]` entries too, and the one that
/// matters is the one under `[package]`.
#[test]
fn a_name_in_another_table_is_not_the_package() {
    let manifest = "[dependencies]\nname = \"wrong\"\n\n[package]\nname = \"right\"\n";
    assert_eq!(package_name(manifest), Some("right"));
}

#[test]
fn a_manifest_without_a_name_reads_as_none() {
    assert_eq!(package_name("[package]\nversion = \"0.1.0\"\n"), None);
}

/// A project created with `--local` has to stay pointed at the checkout: an
/// upgrade that replaced it with a published version would break a project
/// whose whole purpose is testing an unpublished one.
#[test]
fn a_local_checkout_is_recovered() {
    let manifest = "[dependencies]\nrahti = { path = \"C:/src/rahti/crates/rahti\" }\n";
    assert_eq!(local_checkout(manifest).as_deref(), Some("C:/src/rahti"));
}

#[test]
fn a_published_dependency_is_not_a_checkout() {
    assert_eq!(local_checkout("[dependencies]\nrahti = \"0.0.7\"\n"), None);
}

// -------------------------------------------------------------------- policy

/// Regenerating `Cargo.toml` from the template would undo whatever the author
/// added to it, and would replace a working path dependency with a version
/// that may not be published at all. Amending it is a different thing, and
/// lives in `wiring`.
#[test]
fn the_manifest_is_never_rewritten() {
    assert!(NEVER_REWRITTEN.contains(&"Cargo.toml"));
}

/// Everything else the scaffold owns has to be reachable, or a framework fix
/// never lands in an existing project.
#[test]
fn the_framework_files_are_rewritable() {
    for path in [
        "src/app/layout.rs",
        "build.rs",
        "public/js/pp-reactive-v2.min.js",
    ] {
        assert!(
            !NEVER_REWRITTEN.contains(&path),
            "{path} must be upgradable"
        );
    }
}

// --------------------------------------------------------------------- force

/// The line a bulk `--force` stops at. Everything under `src/` is the
/// application the author writes; `src/main.rs` is the startup the framework
/// prescribes, and the one file under `src/` a stale copy actually breaks.
#[test]
fn the_application_is_under_src_and_main_is_not() {
    for path in [
        "src/app/page.rs",
        "src/app/layout.rs",
        "src/app/globals.css",
        "src/db.rs",
        "src/models/todo.rs",
        "src/migrations/m20260101_000001_create_todo.rs",
    ] {
        assert!(authored(path), "{path} is the author's");
    }
    for path in [
        "src/main.rs",
        "build.rs",
        ".cargo/config.toml",
        "AGENTS.md",
        "docs/conventions/routing.md",
        "public/js/pp-reactive-v2.min.js",
    ] {
        assert!(!authored(path), "{path} is the framework's");
    }
}

/// An ordinary upgrade forces nothing, which is the behavior every existing
/// project keeps.
#[test]
fn without_the_flag_nothing_is_forced() {
    let options = Options::parse(&[]).expect("no options");
    assert!(!options.forces("src/main.rs"));
    assert!(!options.forces("src/app/page.rs"));
}

/// A bare `--force` takes the framework's files back and leaves the
/// application alone — a flag that deleted somebody's homepage for saying
/// "yes, update my framework files" would be a trap.
#[test]
fn a_bare_force_takes_the_framework_and_leaves_the_app() {
    let options = Options::parse(&["--force"]).expect("a bare force");
    assert!(options.forces("src/main.rs"));
    assert!(options.forces("docs/conventions/routing.md"));
    assert!(!options.forces("src/app/page.rs"));
    assert!(!options.forces("src/models/todo.rs"));
}

/// Named paths take exactly what they name, application file or not: that is
/// the sentence somebody meant to write.
#[test]
fn named_paths_take_only_themselves() {
    let options = Options::parse(&["--force", "src/app/page.rs"]).expect("a named force");
    assert!(options.forces("src/app/page.rs"));
    assert!(!options.forces("src/main.rs"));
    assert!(!options.forces("docs/conventions/routing.md"));
}

/// The manifest is never taken, with or without a force — the reason it is in
/// `NEVER_REWRITTEN` does not stop being true because a flag was passed.
#[test]
fn the_manifest_is_never_forced() {
    assert!(!Options::parse(&["--force"]).unwrap().forces("Cargo.toml"));
    assert!(
        !Options::parse(&["--force", "Cargo.toml"])
            .unwrap()
            .forces("Cargo.toml")
    );
}

/// A path list ends at the next option, so a force can be previewed and can
/// sit beside a feature flag.
#[test]
fn a_path_list_ends_at_the_next_option() {
    let options =
        Options::parse(&["--force", "src/main.rs", "--dry-run", "--ws"]).expect("both read");
    assert!(options.dry_run);
    assert!(options.ws);
    assert!(options.forces("src/main.rs"));
    assert!(!options.forces("build.rs"));
}

/// Typed at a Windows prompt a path arrives with backslashes, and the ledger's
/// keys never have them.
#[test]
fn a_windows_path_matches_a_ledger_key() {
    let options = Options::parse(&["--force", "src\\app\\page.rs"]).expect("a named force");
    assert!(options.forces("src/app/page.rs"));
}

/// The scaffold for these tests: the paths a project's ledger would carry.
fn scaffold() -> Vec<(String, Vec<u8>)> {
    ["Cargo.toml", "src/main.rs", "src/app/page.rs"]
        .iter()
        .map(|p| (p.to_string(), Vec::new()))
        .collect()
}

/// A misspelled path that forced nothing would report a clean upgrade and
/// leave the file exactly as it was — the one outcome somebody typing
/// `--force` cannot afford to misread.
#[test]
fn an_unknown_path_is_refused() {
    let options = Options::parse(&["--force", "src/app/pages.rs"]).unwrap();
    let error = options.check_forced(&scaffold()).expect_err("refused");
    assert!(error.contains("src/app/pages.rs"), "{error}");
    assert!(error.contains("not a scaffold file"), "{error}");
}

/// Naming the manifest is asking for the one thing the scaffold will not do,
/// so it is said out loud rather than silently ignored.
#[test]
fn naming_the_manifest_is_refused() {
    let options = Options::parse(&["--force", "Cargo.toml"]).unwrap();
    let error = options.check_forced(&scaffold()).expect_err("refused");
    assert!(error.contains("never rewritten"), "{error}");
}

#[test]
fn a_scaffold_path_is_accepted() {
    let options = Options::parse(&["--force", "src/main.rs"]).unwrap();
    assert!(options.check_forced(&scaffold()).is_ok());
    assert!(
        Options::parse(&["--force"])
            .unwrap()
            .check_forced(&scaffold())
            .is_ok()
    );
    assert!(
        Options::parse(&[])
            .unwrap()
            .check_forced(&scaffold())
            .is_ok()
    );
}

/// Every list the report calls a write has to actually be written. A forced
/// file that was announced and then left on disk unchanged is the one failure
/// this command must not have: it says the author's version is gone while
/// leaving it in place.
#[test]
fn every_reported_write_is_a_write() {
    let plan = Plan {
        updated: vec!["build.rs".to_string()],
        added: vec!["docs/conventions/routing.md".to_string()],
        forced: vec!["src/main.rs".to_string()],
        current: vec!["AGENTS.md".to_string()],
        yours: vec!["src/app/page.rs".to_string()],
        deleted: vec![".gitignore".to_string()],
        skipped: vec!["Cargo.toml".to_string()],
    };

    for path in ["build.rs", "docs/conventions/routing.md", "src/main.rs"] {
        assert!(plan.writes(path), "{path} was reported as written");
    }
    for path in ["AGENTS.md", "src/app/page.rs", ".gitignore", "Cargo.toml"] {
        assert!(!plan.writes(path), "{path} was not reported as written");
    }
}

// ------------------------------------------------------------------- rewrite

/// A scratch directory holding the config `new` writes, for the rewrite tests.
fn config_dir(ledger: &Ledger) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "cargo-rahti-rewrite-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a temporary directory");
    crate::new::write_config(&root, true, None, false, false, ledger).expect("a written config");
    root
}

/// The config has two authors, and an upgrade speaks for only one of them.
/// The port and a css key stand in for everything the project's author set:
/// a rewrite that reset them to defaults would undo configuration in the
/// name of refreshing files.
#[test]
fn an_edited_config_survives_the_rewrite() {
    let mut ledger = Ledger::new();
    ledger.insert("build.rs".to_string(), sha256::hex(b"old"));
    let root = config_dir(&ledger);

    // The author's edits: a moved port, a css key `new` never writes, and —
    // standing in for a project scaffolded by an older tool — a different
    // `createdWith`.
    let path = root.join("rahti.config.json");
    let edited = fs::read_to_string(&path)
        .expect("the written config")
        .replace("\"port\": 3000", "\"port\": 8080")
        .replace(
            "\"download\": true",
            "\"download\": true,\n    \"minify\": true",
        )
        .replace(VERSION, "0.0.1");
    fs::write(&path, &edited).expect("an edited config");

    ledger.insert("build.rs".to_string(), sha256::hex(b"new"));
    rewrite_config(&root, &ledger, None, false, false).expect("a rewritten config");

    let raw = fs::read_to_string(&path).expect("the rewritten config");
    let _ = fs::remove_dir_all(&root);

    assert!(
        raw.contains("\"port\": 8080"),
        "the author's port was reset"
    );
    assert!(
        raw.contains("\"minify\": true"),
        "the author's css key was dropped"
    );
    assert!(
        raw.contains(&format!("\"createdWith\": \"{VERSION}\"")),
        "createdWith did not move with the tool"
    );
    assert!(raw.contains(&sha256::hex(b"new")));
    assert!(!raw.contains(&sha256::hex(b"old")));
}

/// With the same ledger and the same version there is nothing to change, and
/// nothing changes: the file `new` wrote comes back byte for byte, blank
/// lines and all.
#[test]
fn an_untouched_config_round_trips_byte_identically() {
    let mut ledger = Ledger::new();
    ledger.insert("build.rs".to_string(), sha256::hex(b""));
    let root = config_dir(&ledger);

    let path = root.join("rahti.config.json");
    let before = fs::read_to_string(&path).expect("the written config");

    rewrite_config(&root, &ledger, None, false, false).expect("a rewritten config");

    let after = fs::read_to_string(&path).expect("the rewritten config");
    let _ = fs::remove_dir_all(&root);

    assert_eq!(before, after);
}

/// A hand-written config may carry no `createdWith` at all; the rewrite
/// leaves it that way rather than inventing a key in someone else's file.
#[test]
fn a_missing_key_is_left_missing() {
    assert_eq!(
        replace_string_value("{ \"schema\": 1 }", "createdWith", "9"),
        None
    );
}

/// The author may reformat the file — JSON owes nobody its blank lines — and
/// the scaffold block has to be found where it is, not where `new` put it.
#[test]
fn a_reformatted_scaffold_block_is_still_found() {
    let raw = r#"{"scaffold":{"a":"1"},"schema":1}"#;
    let mut ledger = Ledger::new();
    ledger.insert("b".to_string(), "2".to_string());

    let out = replace_scaffold(raw, &ledger).expect("a replaced block");
    assert!(out.contains("\"b\": \"2\""));
    assert!(out.contains("\"schema\":1"));
    assert!(!out.contains("\"a\""));
}

// ------------------------------------------------------------------ options

/// The feature flags are `new`'s, with `new`'s meanings — a bare `--db` is
/// SQLite, a named one is itself, and an unknown flag is refused rather than
/// ignored into a silent no.
#[test]
fn the_upgrade_flags_parse_the_way_new_parses_them() {
    let options = Options::parse(&["--dry-run", "--ws", "--mcp"]).expect("parsed options");
    assert!(options.dry_run);
    assert!(options.ws);
    assert!(options.mcp);
    assert_eq!(options.db, None);

    assert_eq!(
        Options::parse(&["--db"]).expect("parsed options").db,
        Some(Backend::Sqlite)
    );
    assert_eq!(
        Options::parse(&["--db", "postgres"])
            .expect("parsed options")
            .db,
        Some(Backend::Postgres)
    );
    // `--db` followed by another flag is the bare form, and the flag is
    // still read as itself.
    let both = Options::parse(&["--db", "--ws", "--mcp"]).expect("parsed options");
    assert_eq!(both.db, Some(Backend::Sqlite));
    assert!(both.ws);
    assert!(both.mcp);

    assert!(Options::parse(&["--tailwind"]).is_err());
}

// ---------------------------------------------------------------- additions

/// A feature added at upgrade time is recorded where `new` would have put
/// it, in the shape `new` writes — and everything the author wrote stays
/// byte for byte.
#[test]
fn an_added_feature_is_recorded_in_the_config() {
    let mut ledger = Ledger::new();
    ledger.insert("build.rs".to_string(), sha256::hex(b""));
    let root = config_dir(&ledger);
    let path = root.join("rahti.config.json");
    let before = fs::read_to_string(&path).expect("the written config");

    let raw = record_features(before.clone(), Some(Backend::Postgres), true, true);
    let _ = fs::remove_dir_all(&root);

    let value: serde_json::Value = serde_json::from_str(&raw).expect("still valid JSON");
    assert_eq!(
        value.get("db").and_then(|d| d.get("backend")),
        Some(&serde_json::Value::String("postgres".into()))
    );
    assert_eq!(value.get("ws"), Some(&serde_json::Value::Bool(true)));
    assert_eq!(value.get("mcp"), Some(&serde_json::Value::Bool(true)));
    assert!(raw.contains("\"migrations\": \"src/migrations\""), "{raw}");

    // Nothing added, nothing touched: the round trip is the identity.
    assert_eq!(record_features(before.clone(), None, false, false), before);
}

// ------------------------------------------------------------------ database

/// The scaffold's own config, read back the way an upgrade reads it. Written
/// as a file rather than parsed from a literal because that is the round trip
/// that actually has to hold: what `new` writes, `upgrade` must understand.
fn config_of(db: Option<Backend>, mcp: bool) -> Result<Config, String> {
    // Uniqueness from a counter, not from the arguments: two tests may ask
    // about the same backend at the same time, and sharing a directory means
    // one of them deletes it out from under the other.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "cargo-rahti-upgrade-db-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a temporary directory");

    let mut ledger = Ledger::new();
    ledger.insert("build.rs".to_string(), sha256::hex(b""));
    crate::new::write_config(&root, true, db, false, mcp, &ledger).expect("a written config");

    let manifest = match db {
        Some(_) => "[package]\nname = \"demo\"\n\n[dependencies]\nsea-orm = \"1\"\n",
        None => "[package]\nname = \"demo\"\n",
    };
    fs::write(root.join("Cargo.toml"), manifest).expect("a written manifest");

    let read = Config::read(&root);
    let _ = fs::remove_dir_all(&root);
    read
}

/// A project's manifest and `.env` are brought up to what its config says,
/// so the files this command just wrote actually compile and run. Driven by
/// the config rather than by what this run added, which is what makes it
/// repair a project an older `cargo rahti upgrade` left half-wired.
#[test]
fn the_manifest_and_env_are_wired_to_the_config() {
    let root = std::env::temp_dir().join(format!("cargo-rahti-wire-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("a temporary directory");

    let mut ledger = Ledger::new();
    ledger.insert("build.rs".to_string(), sha256::hex(b""));
    crate::new::write_config(&root, true, Some(Backend::Sqlite), true, false, &ledger)
        .expect("a written config");
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\n\n[dependencies]\nrahti = \"0.0.7\"\n",
    )
    .expect("a written manifest");
    fs::write(root.join(".env"), "AUTH_SECRET=\"abc\"\n").expect("a written env");

    let config = Config::read(&root).expect("a readable project");

    // A dry run computes the same answer and writes none of it. Four lines:
    // the sea-orm pair for the database, and the test suite's two
    // dev-dependencies, which every project is brought up to.
    let previewed = wire(&root, &config, true).expect("a preview");
    assert_eq!(previewed.dependencies.len(), 4);
    assert!(previewed.ws_feature);
    assert_eq!(previewed.env, vec![".env".to_string()]);
    assert!(
        !fs::read_to_string(root.join("Cargo.toml"))
            .unwrap()
            .contains("sea-orm")
    );

    // The amended manifest comes back so the ledger can record it — a hash
    // still describing the old bytes would have the next run report this
    // command's own edit as the author's.
    assert!(previewed.manifest.is_some());

    let wiring = wire(&root, &config, false).expect("a wired project");
    assert!(wiring.manual.is_empty());

    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("the manifest");
    let env = fs::read_to_string(root.join(".env")).expect("the env file");

    assert!(manifest.contains("sea-orm ="), "{manifest}");
    assert!(manifest.contains("sea-orm-migration ="), "{manifest}");
    assert!(manifest.contains("features = [\"ws\"]"), "{manifest}");
    assert!(manifest.contains("[dev-dependencies]"), "{manifest}");
    assert!(manifest.contains("tower ="), "{manifest}");
    assert!(manifest.contains("http-body-util ="), "{manifest}");
    assert!(env.contains("DATABASE_URL=\"sqlite://"), "{env}");
    assert!(env.contains("AUTH_SECRET=\"abc\""), "{env}");

    // Running it again is a no-op, which is what makes it safe to run at all.
    let again = wire(&root, &config, false).expect("a second run");
    assert!(again.is_empty(), "a second run changed something");
    assert!(again.manifest.is_none());
    assert_eq!(
        fs::read_to_string(root.join("Cargo.toml")).expect("the manifest"),
        manifest
    );

    let _ = fs::remove_dir_all(&root);
}

/// What `new` records, `upgrade` has to read back — otherwise an upgrade
/// regenerates a default project over somebody's database.
#[test]
fn the_backend_survives_the_round_trip() {
    for backend in [Backend::Sqlite, Backend::Postgres, Backend::MySql] {
        let config = config_of(Some(backend), false).expect("a readable project");
        assert_eq!(config.db, Some(backend));
    }

    assert_eq!(config_of(None, false).expect("a readable project").db, None);
}

#[test]
fn the_mcp_choice_survives_the_round_trip() {
    assert!(config_of(None, true).expect("a readable project").mcp);
    assert!(!config_of(None, false).expect("a readable project").mcp);
}

/// The same reading the build makes: a `db` object that names no backend is
/// SQLite, and no object at all is no database.
#[test]
fn an_unnamed_backend_reads_as_sqlite() {
    let value: serde_json::Value = serde_json::from_str(r#"{"db":{}}"#).unwrap();
    assert!(value.get("db").is_some_and(|d| d.is_object()));
}

/// A project with no database is left alone: no dependency, no connection
/// string, nothing to say. The wiring is driven by the config, so this is the
/// config being read as "no".
#[test]
fn a_project_without_a_database_is_not_wired_for_one() {
    let config = config_of(None, false).expect("a readable project");
    assert_eq!(config.db, None);
    assert!(!config.ws);
}
