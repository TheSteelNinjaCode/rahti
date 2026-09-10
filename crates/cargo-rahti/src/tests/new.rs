//! The scaffold: what it writes, and what it records having written.

use super::*;

/// A directory that removes itself, so a failed assertion does not leave a
/// project behind for the next run to trip over.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("cargo-rahti-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a temporary directory");
        Tree(path)
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.0.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn scaffold(tree: &Tree, tailwind: bool) -> Ledger {
    scaffold_with(tree, tailwind, None)
}

fn scaffold_with(tree: &Tree, tailwind: bool, db: Option<Backend>) -> Ledger {
    scaffold_full(tree, tailwind, db, false)
}

fn scaffold_full(tree: &Tree, tailwind: bool, db: Option<Backend>, ws: bool) -> Ledger {
    scaffold_features(tree, tailwind, db, ws, false)
}

fn scaffold_features(
    tree: &Tree,
    tailwind: bool,
    db: Option<Backend>,
    ws: bool,
    mcp: bool,
) -> Ledger {
    let options = Options {
        name: "demo".to_string(),
        tailwind,
        db,
        ws,
        mcp,
        local: None,
    };
    let mut ledger = Ledger::new();
    write_project(&tree.0, &options, tailwind, db, ws, mcp, &mut ledger)
        .expect("a written project");
    write_config(&tree.0, tailwind, db, ws, mcp, &ledger).expect("a written config");
    ledger
}

// -------------------------------------------------------------------- names

/// The name becomes a cargo package and a directory, so what one of those
/// would refuse later is refused here, where the message can say why.
#[test]
fn a_usable_name_is_accepted() {
    assert!(check_name("my-app").is_ok());
    assert!(check_name("my_app").is_ok());
    assert!(check_name("app2").is_ok());
}

#[test]
fn an_unusable_name_is_refused() {
    assert!(check_name("").is_err());
    assert!(
        check_name("my app").is_err(),
        "a space is not a package name"
    );
    assert!(
        check_name("my/app").is_err(),
        "a separator would escape the directory"
    );
    assert!(
        check_name("2fast").is_err(),
        "cargo will not take a leading digit"
    );
}

// ------------------------------------------------------------------ options

/// A flag adds a feature. There is no flag that removes one, so the whole of
/// the command line is what to add.
#[test]
fn the_engine_flag_is_read() {
    assert!(Options::parse(&["app", "--tailwind"]).unwrap().tailwind);
}

/// Leaving the flag out is how you say no — and the only way, which is the
/// point. An interactive run still asks; nothing else does.
#[test]
fn no_flag_is_the_way_to_say_no() {
    let options = Options::parse(&["app"]).unwrap();
    assert!(!options.tailwind);
    assert!(options.db.is_none());
    assert!(!options.mcp);
}

#[test]
fn the_mcp_flag_is_read() {
    assert!(Options::parse(&["app", "--mcp"]).unwrap().mcp);
}

#[test]
fn a_missing_name_is_reported() {
    assert!(Options::parse(&["--tailwind"]).is_err());
}

#[test]
fn an_unknown_option_is_reported() {
    assert!(Options::parse(&["app", "--turbo"]).is_err());
}

#[test]
fn local_takes_the_path_after_it() {
    let options = Options::parse(&["app", "--local", "/checkout"]).unwrap();
    assert_eq!(options.local.as_deref(), Some("/checkout"));
    assert!(
        Options::parse(&["app", "--local"]).is_err(),
        "the path is not optional"
    );
}

// ------------------------------------------------------------------- ledger

/// Every hash recorded has to describe the bytes actually on disk, because a
/// later comparison has nothing else to go on.
#[test]
fn the_ledger_matches_what_was_written() {
    let tree = Tree::new("ledger");
    let ledger = scaffold(&tree, false);

    assert!(!ledger.is_empty());
    for (path, hash) in &ledger {
        let bytes = fs::read(tree.0.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(
            &sha256::hex(&bytes),
            hash,
            "{path} does not match its ledger entry"
        );
    }
}

/// The build owns the compiled stylesheet from the first compile onward, so a
/// hash taken at scaffold time would be wrong before anyone could read it —
/// and an upgrade would take that for an edit the author made.
#[test]
fn a_file_the_build_owns_is_not_in_the_ledger() {
    let tree = Tree::new("untracked");
    let ledger = scaffold(&tree, true);

    assert!(!ledger.contains_key("public/css/styles.css"));
    assert!(
        tree.0.join("public/css/styles.css").is_file(),
        "it still has to exist for the first build to overwrite"
    );
}

#[test]
fn the_config_records_the_ledger_and_the_choice() {
    let tree = Tree::new("config");
    scaffold(&tree, false);

    let config = tree.read("rahti.config.json");
    assert!(config.contains("\"schema\": 1"), "{config}");
    assert!(config.contains("\"createdWith\""), "{config}");
    assert!(config.contains("\"engine\": \"plain\""), "{config}");
    assert!(
        config.contains("\"src/app/page.rs\""),
        "the ledger is written:\n{config}"
    );
}

// ---------------------------------------------------------------------- MCP

#[test]
fn mcp_is_absent_until_the_project_opts_in() {
    let tree = Tree::new("no-mcp");
    let ledger = scaffold(&tree, false);

    assert!(!ledger.contains_key(".mcp.json"));
    assert!(!tree.0.join(".mcp.json").exists());
    assert!(!tree.read("rahti.config.json").contains("\"mcp\""));
}

#[test]
fn an_mcp_project_gets_a_recorded_client_configuration() {
    let tree = Tree::new("mcp");
    let ledger = scaffold_features(&tree, false, None, false, true);

    assert!(ledger.contains_key(".mcp.json"));
    let client = tree.read(".mcp.json");
    assert!(client.contains("\"command\": \"rahti-mcp\""), "{client}");
    assert!(client.contains("\"--project\", \".\""), "{client}");
    assert!(tree.read("rahti.config.json").contains("\"mcp\": true"));
    assert!(
        tree.read("Cargo.toml").contains("features = [\"mcp\"]"),
        "the application dependency must compile the HTTP endpoint"
    );
    assert!(ledger.contains_key("public/llms.txt"));
    assert!(
        tree.read("public/llms.txt")
            .contains("/.well-known/mcp.json")
    );
    let env = tree.read(".env");
    assert!(env.contains("RAHTI_MCP_AUTH_TOKEN="), "{env}");
    assert!(env.contains("RAHTI_MCP_ALLOWED_HOSTS="), "{env}");
}

#[test]
fn mcp_and_websockets_share_the_rahti_feature_list() {
    let manifest = t::cargo_toml("demo", "\"0.0.7\"", None, true, true);
    assert!(
        manifest.contains("features = [\"mcp\", \"ws\"]")
            || manifest.contains("features = [\"ws\", \"mcp\"]"),
        "{manifest}"
    );
}

// ------------------------------------------------------------------ engines

/// A project that chose plain CSS should carry no trace of the other choice —
/// that is the whole promise of the question being asked.
#[test]
fn a_plain_project_carries_nothing_of_tailwind() {
    let tree = Tree::new("plain");
    let ledger = scaffold(&tree, false);

    assert!(!ledger.contains_key("public/js/tailwind-merge.mjs"));
    assert!(!tree.0.join("public/js/tailwind-merge.mjs").exists());

    let main_js = tree.read("public/js/main.js");
    assert!(!main_js.contains("twMerge"), "{main_js}");

    let globals = tree.read("src/app/globals.css");
    assert!(!globals.contains("tailwindcss"), "{globals}");

    let config = tree.read("rahti.config.json");
    assert!(
        !config.contains("\"download\""),
        "no compiler to fetch:\n{config}"
    );
}

#[test]
fn a_tailwind_project_carries_the_helper_and_the_directives() {
    let tree = Tree::new("tw");
    let ledger = scaffold(&tree, true);

    assert!(ledger.contains_key("public/js/tailwind-merge.mjs"));
    assert!(tree.read("public/js/main.js").contains("twMerge"));
    assert!(
        tree.read("src/app/globals.css")
            .contains("@import \"tailwindcss\"")
    );
    assert!(
        tree.read("rahti.config.json")
            .contains("\"engine\": \"tailwind\"")
    );
}

/// The layout's single parent element is `<html>`, which is no place for a
/// `pp-component`: the client runtime would treat the whole document as a
/// component and re-render it, taking `<head>` and `<body>` with it — a blank
/// page from a freshly scaffolded project. `html!` handles that itself by
/// naming the `<template>` it injects around the slot, so the scaffolded file
/// writes neither.
#[test]
fn the_layout_leaves_the_component_marker_to_the_macro() {
    let tree = Tree::new("layout");
    scaffold(&tree, true);

    let layout = tree.read("src/app/layout.rs");
    let markup: String = layout
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        !markup.contains("pp-component"),
        "the marker is injected, not written:\n{layout}"
    );
    assert!(
        markup.contains("<slot />"),
        "the slot is where it goes:\n{layout}"
    );
}

/// Both engines produce the same project but for the files that must differ.
#[test]
fn the_engines_differ_only_where_they_must() {
    let plain = Tree::new("same-plain");
    let tailwind = Tree::new("same-tw");
    let a = scaffold(&plain, false);
    let b = scaffold(&tailwind, true);

    let differ = [
        "src/app/globals.css",
        "src/app/page.rs",
        "public/js/main.js",
    ];
    for path in a.keys().filter(|p| b.contains_key(*p)) {
        let same = a[path] == b[path];
        if differ.contains(&path.as_str()) {
            assert!(!same, "{path} should differ between engines");
        } else {
            assert!(same, "{path} should be identical between engines");
        }
    }
}

// ----------------------------------------------------------------- database

#[test]
fn the_database_flag_is_read() {
    let db = |args: &[&str]| Options::parse(args).unwrap().db;

    assert_eq!(db(&["app", "--db", "postgres"]), Some(Backend::Postgres));
    assert_eq!(db(&["app", "--db", "mysql"]), Some(Backend::MySql));
    assert_eq!(db(&["app", "--db", "sqlite"]), Some(Backend::Sqlite));
}

/// The backend that needs no server running is the one worth having as the
/// short form.
#[test]
fn a_bare_db_flag_is_sqlite() {
    assert_eq!(
        Options::parse(&["app", "--db"]).unwrap().db,
        Some(Backend::Sqlite)
    );

    let both = Options::parse(&["app", "--db", "--tailwind"]).unwrap();
    assert_eq!(
        both.db,
        Some(Backend::Sqlite),
        "a flag after --db is a flag"
    );
    assert!(both.tailwind, "and it still counts as itself");
}

#[test]
fn a_misspelled_backend_is_refused() {
    assert!(Options::parse(&["app", "--db", "postgress"]).is_err());
}

/// A project with no database is a project with no database: no directories,
/// no dependency, no mention of it anywhere.
#[test]
fn a_project_without_a_database_carries_nothing_of_one() {
    let tree = Tree::new("no-db");
    let ledger = scaffold_with(&tree, true, None);

    for path in ledger.keys() {
        assert!(!path.starts_with("src/models"), "{path}");
        assert!(!path.starts_with("src/migrations"), "{path}");
        assert!(path != "src/db.rs", "{path}");
    }

    assert!(!tree.read("Cargo.toml").contains("sea-orm"));
    assert!(!tree.read("src/main.rs").contains("mod db;"));
    assert!(!tree.read("rahti.config.json").contains("\"db\""));

    // `.env` still exists, and it is not a database file: AUTH_SECRET lives
    // there whether or not the project has a database, and a `.env` that only
    // appeared once you chose SQLite would be a trap.
    let env = tree.read(".env");
    assert!(!env.contains("DATABASE_URL"), "{env}");
    assert!(env.contains("AUTH_SECRET="), "{env}");
    assert!(tree.read(".gitignore").contains("/.env"));
}

#[test]
fn a_database_project_gets_the_files_that_wire_one_up() {
    let tree = Tree::new("with-db");
    scaffold_with(&tree, true, Some(Backend::Sqlite));

    // The worked example everything else is copied from.
    assert!(
        tree.read("src/models/todo.rs")
            .contains("DeriveEntityModel")
    );
    assert!(
        tree.read("src/migrations/m20260101_000001_create_todo.rs")
            .contains("create_table")
    );
    // The connection, reached the only way a stateless handler can reach one.
    assert!(tree.read("src/db.rs").contains("OnceLock"));

    // Wired from `src/lib.rs`, not `src/main.rs`: startup is shared with the
    // optional native package, which has no `main` to call.
    let lib = tree.read("src/lib.rs");
    assert!(lib.contains("db::connect_if_configured()"), "{lib}");
    assert!(lib.contains("pub mod models;"), "{lib}");
    assert!(lib.contains("initialize_application"), "{lib}");

    // And the binary is the thin half that only knows an address.
    let main = tree.read("src/main.rs");
    assert!(main.contains("initialize_application()"), "{main}");
    assert!(!main.contains("db::connect"), "{main}");
}

/// `src/lib.rs` must not call into a file the scaffold did not write.
///
/// The failure this catches happened: the template called
/// `auth::settings()`, and `cargo rahti new` does not write an `src/auth.rs`
/// — a project adds one when it wants private routes. Every scaffolded
/// project failed to compile, and nothing in the suite noticed, because no
/// test compiles a generated project.
#[test]
fn the_library_only_names_modules_it_declares() {
    for (label, db) in [("mods-none", None), ("mods-db", Some(Backend::Sqlite))] {
        let tree = Tree::new(label);
        scaffold_with(&tree, false, db);
        let lib = tree.read("src/lib.rs");

        // What it declares — and where a `#[path]` attribute says a module
        // actually lives, because `mod app_tests;` is `src/tests/app.rs`.
        let mut declared: Vec<&str> = Vec::new();
        let mut redirected: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        let mut pending_path: Option<&str> = None;
        for line in lib.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("#[path = \"") {
                pending_path = rest.strip_suffix("\"]");
                continue;
            }
            if line.starts_with("#[") {
                continue;
            }
            if let Some(name) = line
                .strip_prefix("pub mod ")
                .or_else(|| line.strip_prefix("mod "))
                .and_then(|rest| rest.strip_suffix(';'))
            {
                declared.push(name);
                if let Some(path) = pending_path.take() {
                    redirected.insert(name, path);
                }
            }
        }

        // What it calls. A path at the start of a statement is either a module
        // of this crate or a dependency, and the dependencies are known.
        const CRATES: &[&str] = &["rahti", "std", "axum", "tokio", "serde", "self", "crate"];

        for line in lib.lines() {
            let code = line.trim();
            if code.starts_with("//") || code.starts_with("///") {
                continue;
            }
            let Some((head, _)) = code.split_once("::") else {
                continue;
            };
            let ident: String = head
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();

            // A module path is lowercase; anything capitalised is a type or an
            // enum, and `StartupError::new` needs no `mod`.
            let lowercase = ident.starts_with(|c: char| c.is_ascii_lowercase());
            if ident.is_empty() || !lowercase || CRATES.contains(&ident.as_str()) {
                continue;
            }
            assert!(
                declared.contains(&ident.as_str()),
                "{label}: src/lib.rs calls `{ident}::…` and declares no `mod {ident};` — \
                 the generated project would not compile.\nline: {code}"
            );
        }

        // And every module it declares is a file somebody writes: the
        // scaffold, or `rahti-build` on the first cargo build.
        const GENERATED: &[&str] = &["components", "routes", "models", "migrations"];

        for module in declared {
            if GENERATED.contains(&module) {
                continue;
            }
            let candidates = match redirected.get(module) {
                // `#[path]` is relative to the file that carries it, and the
                // carrier is `src/lib.rs`.
                Some(path) => vec![tree.0.join("src").join(path)],
                None => vec![
                    tree.0.join(format!("src/{module}.rs")),
                    tree.0.join(format!("src/{module}/mod.rs")),
                ],
            };
            assert!(
                candidates.iter().any(|p| p.exists()),
                "{label}: src/lib.rs declares `mod {module};`, and neither the scaffold nor \
                 rahti-build writes it"
            );
        }
    }
}

/// The binary calls into the library by the name cargo gave it.
#[test]
fn the_binary_names_the_library_target() {
    let tree = Tree::new("lib-name");
    scaffold(&tree, false);

    // Cargo turns a hyphen in a package name into an underscore in the crate
    // name, and the manifest writes both out so neither is a guess.
    let manifest = tree.read("Cargo.toml");
    assert!(manifest.contains("[lib]"), "{manifest}");
    assert!(manifest.contains("path = \"src/lib.rs\""), "{manifest}");
    // Native packaging is opt-in, and this is what keeps it that way for a
    // project that never asks: a web build cannot resolve a Tauri dependency
    // it has excluded.
    assert!(manifest.contains("exclude = [\"native\"]"), "{manifest}");

    assert!(
        tree.read("src/main.rs").contains("initialize_application"),
        "the binary does not call into the library"
    );
}

/// The feature is the one thing about a backend that is not interchangeable,
/// so the manifest has to name the right one.
#[test]
fn the_manifest_names_the_backend_feature() {
    for (backend, feature) in [
        (Backend::Sqlite, "sqlx-sqlite"),
        (Backend::Postgres, "sqlx-postgres"),
        (Backend::MySql, "sqlx-mysql"),
    ] {
        let tree = Tree::new(&format!("feature-{feature}"));
        scaffold_with(&tree, true, Some(backend));

        let manifest = tree.read("Cargo.toml");
        assert!(manifest.contains(feature), "{backend:?}:\n{manifest}");
        assert!(manifest.contains("sea-orm-migration"), "{manifest}");
    }
}

/// The config records which backend, and deliberately not how to reach it.
#[test]
fn the_config_records_the_backend_and_not_the_credential() {
    let tree = Tree::new("db-config");
    scaffold_with(&tree, true, Some(Backend::Postgres));

    let config = tree.read("rahti.config.json");
    assert!(config.contains("\"backend\": \"postgres\""), "{config}");
    assert!(config.contains("\"models\": \"src/models\""), "{config}");
    assert!(
        !config.contains("DATABASE_URL") && !config.contains("password"),
        "a connection string is a credential and this file is committed:\n{config}"
    );
}

/// `.env` holds the credentials, so it is written once and never tracked —
/// tracking it would have `upgrade` compare a hash against somebody's
/// password, and would make it a file the scaffold believes it may rewrite.
#[test]
fn the_env_file_is_written_once_and_not_tracked() {
    let tree = Tree::new("db-env");
    let ledger = scaffold_with(&tree, true, Some(Backend::Sqlite));

    assert!(tree.read(".env").contains("DATABASE_URL="));
    assert!(
        !ledger.contains_key(".env"),
        "the credential is not the scaffold's"
    );
    assert!(tree.read(".gitignore").contains("/.env"));
}

// ------------------------------------------------------- generated secrets

/// The whole point of generating them: two projects made on one machine do not
/// share a signing key or a cookie name.
#[test]
fn two_projects_get_different_secrets() {
    let one = Tree::new("secret-one");
    let other = Tree::new("secret-two");
    scaffold_with(&one, true, Some(Backend::Sqlite));
    scaffold_with(&other, true, Some(Backend::Sqlite));

    let value = |tree: &Tree, key: &str| {
        tree.read(".env")
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{key}=")))
            .map(|v| v.trim_matches('"').to_string())
            .unwrap_or_else(|| panic!("{key} is not in .env"))
    };

    assert_ne!(value(&one, "AUTH_SECRET"), value(&other, "AUTH_SECRET"));
    assert_ne!(
        value(&one, "AUTH_COOKIE_NAME"),
        value(&other, "AUTH_COOKIE_NAME")
    );
}

/// The real key goes in the ignored file, and it is a key — not one of the
/// words `rahti::auth` refuses to start a release build on.
#[test]
fn the_env_file_carries_a_real_key() {
    let tree = Tree::new("secret-real");
    scaffold_with(&tree, true, Some(Backend::Sqlite));

    let secret = tree
        .read(".env")
        .lines()
        .find_map(|line| line.strip_prefix("AUTH_SECRET="))
        .expect("a secret")
        .trim_matches('"')
        .to_string();

    assert!(secret.len() >= 32, "a real key: {secret}");
    for placeholder in ["change-me", "changeme", "secret", "replace-me"] {
        assert_ne!(secret.to_lowercase(), placeholder, "{secret}");
    }
}

/// The cookie name is not a credential — it is the project's identity under a
/// shared parent domain — so it is generated rather than fixed, and has to be
/// a name a `Cookie` header can carry unquoted.
#[test]
fn the_cookie_name_is_a_legal_token() {
    let tree = Tree::new("cookie-name");
    scaffold_with(&tree, true, Some(Backend::Sqlite));

    let name = tree
        .read(".env")
        .lines()
        .find_map(|line| line.strip_prefix("AUTH_COOKIE_NAME="))
        .map(|v| v.trim_matches('"').to_string())
        .expect("a cookie name");

    // Hex, and long enough not to collide.
    assert_eq!(name.len(), 16, "{name}");
    assert!(name.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
}

/// All three, under the names `rahti::auth` reads.
#[test]
fn the_env_file_names_the_three_auth_variables() {
    let tree = Tree::new("auth-vars");
    scaffold_with(&tree, true, None);
    let env = tree.read(".env");

    for key in ["AUTH_SECRET", "AUTH_COOKIE_NAME", "SESSION_LIFETIME_HOURS"] {
        assert!(env.contains(&format!("{key}=")), "{key} missing:\n{env}");
    }
}

/// Adding a database changes the files it has to change and no others.
#[test]
fn a_database_changes_only_what_it_must() {
    let without = Tree::new("diff-none");
    let with = Tree::new("diff-db");
    let a = scaffold_with(&without, true, None);
    let b = scaffold_with(&with, true, Some(Backend::Sqlite));

    // The guide is one of them: a database project's `AGENTS.md` carries the
    // database rules and the `database.md` table row.
    //
    // `src/lib.rs` rather than `src/main.rs`, because that is where startup
    // is: the binary only knows an address, and an address does not change
    // when a database appears.
    //
    // The test suite differs too: a database project's `setup()` connects a
    // real SQLite file and runs the migrations, and the suite gains the
    // entity round-trip test.
    let differ = [
        "Cargo.toml",
        "src/lib.rs",
        "src/tests/app.rs",
        ".gitignore",
        "AGENTS.md",
    ];
    for path in a.keys().filter(|p| b.contains_key(*p)) {
        let same = a[path] == b[path];
        if differ.contains(&path.as_str()) {
            assert!(!same, "{path} should differ once there is a database");
        } else {
            assert!(same, "{path} should be identical either way");
        }
    }
}

// ------------------------------------------------------------------ testing

/// Every project is born with a passing suite: the harness, the first tests,
/// the module wiring that includes them, and the dev-dependencies that drive
/// the router in-process. A scaffold with no tests teaches that tests are
/// someone else's problem.
#[test]
fn every_project_starts_with_a_test_suite() {
    let tree = Tree::new("suite");
    let ledger = scaffold(&tree, false);

    assert!(ledger.contains_key("src/tests/app.rs"));

    let tests = tree.read("src/tests/app.rs");
    assert!(tests.contains("oneshot"), "{tests}");
    assert!(
        tests.contains("x-pp-function"),
        "the browser's rpc flow: {tests}"
    );
    assert!(
        tests.contains("rahti::auth::configure"),
        "the setup hook names where the policy goes: {tests}"
    );

    let lib = tree.read("src/lib.rs");
    assert!(lib.contains("#[path = \"tests/app.rs\"]"), "{lib}");
    assert!(lib.contains("mod app_tests;"), "{lib}");

    let manifest = tree.read("Cargo.toml");
    assert!(manifest.contains("[dev-dependencies]"), "{manifest}");
    assert!(manifest.contains("http-body-util"), "{manifest}");
    assert!(
        manifest.contains("tower = { version = \"0.5\", features = [\"util\"] }"),
        "{manifest}"
    );
}

/// The suite follows the database: a project that chose one tests against a
/// real SQLite file with its migrations run, and one that did not carries no
/// trace of a database in its tests.
#[test]
fn the_suite_tests_against_the_database_only_when_there_is_one() {
    let with = Tree::new("suite-db");
    scaffold_with(&with, false, Some(Backend::Sqlite));
    let tests = with.read("src/tests/app.rs");
    assert!(tests.contains("crate::db::connect_to"), "{tests}");
    assert!(tests.contains("crate::db::migrate"), "{tests}");
    assert!(tests.contains("mode=rwc"), "a file, not :memory:: {tests}");

    let without = Tree::new("suite-no-db");
    scaffold_with(&without, false, None);
    let tests = without.read("src/tests/app.rs");
    assert!(!tests.contains("crate::db"), "{tests}");
    assert!(!tests.contains("crate::models"), "{tests}");
}

// ------------------------------------------------------------ documentation

/// Rahti is not in anyone's training data, so a project that leaves the
/// scaffold without its documentation gives the next coding agent nothing to
/// read. The guide and the convention documents are scaffold files like any
/// other: written, ledgered, and refreshed by `upgrade` until edited.
#[test]
fn every_project_carries_its_own_documentation() {
    let tree = Tree::new("docs");
    let ledger = scaffold(&tree, false);

    assert!(ledger.contains_key("AGENTS.md"));
    assert!(ledger.contains_key("CLAUDE.md"));
    assert_eq!(tree.read("CLAUDE.md"), "@AGENTS.md\n");

    for (doc, _) in t::CORE_DOCS {
        let path = format!("docs/conventions/{doc}");
        assert!(ledger.contains_key(&path), "{path} is not in the ledger");
        assert!(tree.0.join(&path).is_file(), "{path} was not written");
    }
}

/// The guide must only ever name documents the project actually has: a table
/// row pointing at a file the scaffold did not write sends the reader
/// searching for something their checkout does not contain.
#[test]
fn the_guide_names_only_documents_the_project_has() {
    let bare = Tree::new("guide-bare");
    scaffold(&bare, false);

    let guide = bare.read("AGENTS.md");
    assert!(!guide.contains("database.md"), "{guide}");
    assert!(!guide.contains("websockets.md"), "{guide}");
    assert!(!bare.0.join("docs/conventions/database.md").exists());
    assert!(!bare.0.join("docs/conventions/websockets.md").exists());

    let full = Tree::new("guide-full");
    let ledger = scaffold_full(&full, false, Some(Backend::Sqlite), true);

    let guide = full.read("AGENTS.md");
    assert!(guide.contains("docs/conventions/database.md"), "{guide}");
    assert!(guide.contains("docs/conventions/websockets.md"), "{guide}");
    assert!(ledger.contains_key("docs/conventions/database.md"));
    assert!(ledger.contains_key("docs/conventions/websockets.md"));
}

/// The copies under `assets/docs/` exist because a published crate cannot
/// read a file outside its own directory. This holds each one byte-identical
/// to its canonical original in `docs/conventions/`, the same promise the
/// client runtime keeps between its two copies.
#[test]
fn shipped_docs_match_the_canonical_conventions() {
    let canonical = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/conventions");

    let mut shipped: Vec<(&str, &str)> = t::CORE_DOCS.to_vec();
    shipped.push(("database.md", t::DOC_DATABASE));
    shipped.push(("websockets.md", t::DOC_WEBSOCKETS));

    for (doc, embedded) in shipped {
        let original = fs::read_to_string(canonical.join(doc))
            .unwrap_or_else(|e| panic!("docs/conventions/{doc}: {e}"));
        assert_eq!(
            original.replace("\r\n", "\n"),
            embedded.replace("\r\n", "\n"),
            "assets/docs/{doc} has drifted from docs/conventions/{doc} — \
             copy the canonical file over it"
        );
    }
}

// --------------------------------------------------------------- websockets

#[test]
fn ws_is_off_unless_asked_for() {
    assert!(!Options::parse(&["app"]).unwrap().ws);
    assert!(Options::parse(&["app", "--ws"]).unwrap().ws);
}

/// The feature has to land inside the dependency table whichever form the
/// dependency takes — a published version, or a `--local` path.
#[test]
fn ws_puts_the_feature_on_the_rahti_dependency() {
    let with = t::cargo_toml("demo", "\"0.0.7\"", None, true, false);
    assert!(
        with.contains("rahti = { version = \"0.0.7\", features = [\"ws\"] }"),
        "{with}"
    );

    let local = t::cargo_toml(
        "demo",
        "{ path = \"/checkout/crates/rahti\" }",
        None,
        true,
        false,
    );
    assert!(
        local.contains("rahti = { path = \"/checkout/crates/rahti\", features = [\"ws\"] }"),
        "{local}"
    );
    // The build dependency has no `ws` feature to name.
    assert!(
        local.contains("rahti-build = { path = \"/checkout/crates/rahti-build\" }"),
        "{local}"
    );

    let without = t::cargo_toml("demo", "\"0.0.7\"", None, false, false);
    assert!(without.contains("rahti = \"0.0.7\""), "{without}");
    assert!(!without.contains("ws"), "{without}");
}

/// `upgrade` reads the key back to regenerate the same project, so the
/// choice has to be in the config.
#[test]
fn ws_is_recorded_in_the_config() {
    let tree = Tree::new("ws-config");
    scaffold_full(&tree, false, None, true);
    let config = fs::read_to_string(tree.0.join("rahti.config.json")).expect("a config");
    assert!(config.contains("\"ws\": true"), "{config}");

    let tree = Tree::new("ws-config-off");
    scaffold_full(&tree, false, None, false);
    let config = fs::read_to_string(tree.0.join("rahti.config.json")).expect("a config");
    assert!(!config.contains("\"ws\""), "{config}");
}
