//! Editing files the scaffold does not own, without disturbing them.

use super::*;

// ------------------------------------------------------------- dependencies

const MANIFEST: &str = "[package]\nname = \"demo\"\nedition = \"2024\"\n\n\
                        [dependencies]\naxum = \"0.8.9\"\nrahti = \"0.1\"\n\n\
                        [build-dependencies]\nrahti-build = \"0.1\"\n";

// ------------------------------------------------------------------ versions

/// A pin left behind is the failure that hides best: the project compiles,
/// runs locally, and does none of what the files this run just wrote say.
#[test]
fn a_stale_pin_moves_to_the_tools_version() {
    let manifest = "[dependencies]\naxum = \"0.8.9\"\nrahti = \"0.0.7\"\n";
    let (out, was) = with_version(manifest, "rahti", "0.0.9")
        .expect("readable")
        .expect("a bump");

    assert_eq!(was, "0.0.7");
    assert!(out.contains("rahti = \"0.0.9\""), "{out}");
    // The author's own dependency is untouched, and so is the shape.
    assert!(out.contains("axum = \"0.8.9\""), "{out}");
    assert!(out.ends_with('\n'));
}

/// `rahti-build` is a whole key of its own, and lives under a different table.
/// Looking for `rahti` must not find it, and bumping it must not need the
/// table it sits in.
#[test]
fn each_framework_pin_moves_on_its_own() {
    let (out, _) = with_version(MANIFEST, "rahti", "0.2").unwrap().unwrap();
    assert!(out.contains("rahti = \"0.2\""), "{out}");
    assert!(out.contains("rahti-build = \"0.1\""), "{out}");

    let (out, was) = with_version(&out, "rahti-build", "0.2").unwrap().unwrap();
    assert_eq!(was, "0.1");
    assert!(out.contains("rahti-build = \"0.2\""), "{out}");
}

/// The `ws` form has to survive the bump — a project that lost its feature to
/// a version bump would stop compiling on `#[socket]`.
#[test]
fn an_inline_table_keeps_its_features() {
    let manifest = "[dependencies]\nrahti = { version = \"0.0.7\", features = [\"ws\"] }\n";
    let (out, _) = with_version(manifest, "rahti", "0.0.9")
        .unwrap()
        .expect("a bump");
    assert!(
        out.contains("rahti = { version = \"0.0.9\", features = [\"ws\"] }"),
        "{out}"
    );
}

/// A `--local` project exists to test an unpublished framework. Replacing its
/// checkout with a version would break exactly the project that needs it most.
#[test]
fn a_path_dependency_is_left_alone() {
    let manifest = "[dependencies]\nrahti = { path = \"C:/src/rahti/crates/rahti\" }\n";
    assert_eq!(with_version(manifest, "rahti", "0.0.9"), Ok(None));
}

/// Only ever forward. A project ahead of the tool that ran is not a project to
/// quietly walk backwards.
#[test]
fn a_current_or_newer_pin_is_left_alone() {
    for pinned in ["0.0.9", "0.1.0", "1.0"] {
        let manifest = format!("[dependencies]\nrahti = \"{pinned}\"\n");
        assert_eq!(
            with_version(&manifest, "rahti", "0.0.9"),
            Ok(None),
            "{pinned} is not older than 0.0.9"
        );
    }
}

/// Numbers, not text: `0.0.10` is newer than `0.0.9` even though it sorts
/// before it.
#[test]
fn versions_compare_as_numbers() {
    let manifest = "[dependencies]\nrahti = \"0.0.9\"\n";
    let (out, was) = with_version(manifest, "rahti", "0.0.10")
        .unwrap()
        .expect("9 is older than 10");
    assert_eq!(was, "0.0.9");
    assert!(out.contains("rahti = \"0.0.10\""), "{out}");

    let manifest = "[dependencies]\nrahti = \"0.0.10\"\n";
    assert_eq!(with_version(manifest, "rahti", "0.0.9"), Ok(None));
}

/// A requirement written to float is the author asking for something other
/// than one exact version, and is theirs to keep.
#[test]
fn a_floating_requirement_is_left_alone() {
    for pinned in ["^0.0.7", "*", "0.0.7-beta.1"] {
        let manifest = format!("[dependencies]\nrahti = \"{pinned}\"\n");
        assert_eq!(
            with_version(&manifest, "rahti", "0.0.9"),
            Ok(None),
            "{pinned}"
        );
    }
}

/// A dependency that is not there is nothing to do, not an error — a
/// build-dependency-only manifest has no `rahti` in `[dependencies]`.
#[test]
fn a_missing_dependency_is_not_an_error() {
    assert_eq!(
        with_version("[dependencies]\naxum = \"0.8.9\"\n", "rahti", "0.0.9"),
        Ok(None)
    );
}

/// The two shapes this will not guess at are reported rather than skipped, so
/// the author is told the pin is theirs to move.
#[test]
fn an_unreadable_shape_is_reported() {
    let table = "[dependencies.rahti]\nversion = \"0.0.7\"\n";
    assert!(with_version(table, "rahti", "0.0.9").is_err());

    let split = "[dependencies]\nrahti = {\n  version = \"0.0.7\",\n}\n";
    assert!(with_version(split, "rahti", "0.0.9").is_err());
}

/// The whole point: a project that gained a database compiles afterwards.
/// The addition goes inside `[dependencies]`, not into the table that happens
/// to be last in the file.
#[test]
fn a_dependency_lands_in_the_dependency_table() {
    let [(name, line), _] = t::sea_orm_dependencies(Backend::Sqlite);
    let out = with_dependency(MANIFEST, name, &line).expect("an amended manifest");

    let dependencies = out
        .split("[build-dependencies]")
        .next()
        .expect("the first table");
    assert!(dependencies.contains("sea-orm ="), "{out}");
    assert!(out.contains("sqlx-sqlite"), "{out}");

    // Everything the author had is still there, and still where it was.
    assert!(out.contains("axum = \"0.8.9\""), "{out}");
    assert!(out.contains("rahti = \"0.1\""), "{out}");
    assert!(out.contains("[build-dependencies]\nrahti-build"), "{out}");
    assert!(out.ends_with('\n'));
}

/// Twice is once. An upgrade run again must not write a second `sea-orm` key
/// — cargo refuses to parse a manifest with a duplicate.
#[test]
fn a_dependency_that_is_there_is_not_added_again() {
    let [(name, line), _] = t::sea_orm_dependencies(Backend::Sqlite);
    let once = with_dependency(MANIFEST, name, &line).expect("an amended manifest");
    assert_eq!(with_dependency(&once, name, &line), None);
}

/// `sea-orm-migration` starts with `sea-orm`, and a check by prefix would
/// read the migration crate as the ORM and leave the project short of one.
#[test]
fn a_longer_name_is_not_the_dependency() {
    let manifest = "[dependencies]\nsea-orm-migration = \"2\"\n";
    assert!(!declares(manifest, "sea-orm"));
    assert!(declares(manifest, "sea-orm-migration"));
}

/// The other spelling cargo accepts. A dependency declared as its own table
/// is still declared.
#[test]
fn a_dependency_table_counts_as_declared() {
    assert!(declares(
        "[dependencies.sea-orm]\nversion = \"2\"\n",
        "sea-orm"
    ));
}

/// A key written after a sub-table header belongs to the sub-table, so the
/// addition has to stop at the first header of any kind.
#[test]
fn a_sub_table_ends_the_dependency_table() {
    let manifest = "[dependencies]\naxum = \"0.8.9\"\n\n\
                    [dependencies.rahti]\nversion = \"0.1\"\n";
    let out = with_dependency(manifest, "sea-orm", "sea-orm = \"2\"").expect("an amended manifest");
    let before = out.split("[dependencies.rahti]").next().expect("the table");
    assert!(before.contains("sea-orm = \"2\""), "{out}");
}

/// A package may have no dependency table yet — cargo does not require one —
/// and the addition still has to land somewhere legal.
#[test]
fn a_manifest_without_the_table_gains_one() {
    let out = with_dependency("[package]\nname = \"demo\"\n", "sea-orm", "sea-orm = \"2\"")
        .expect("an amended manifest");
    assert!(out.contains("[dependencies]\nsea-orm = \"2\""), "{out}");
}

// --------------------------------------------------------- dev-dependencies

/// A project scaffolded before there was a test suite gains the suite's two
/// dev-dependencies on upgrade, in their own table — not in `[dependencies]`,
/// where they would be compiled into the application.
#[test]
fn a_dev_dependency_lands_in_its_own_table() {
    let [(name, line), _] = t::DEV_DEPENDENCIES;
    let out = with_dev_dependency(MANIFEST, name, line).expect("an amended manifest");

    assert!(
        out.contains(&format!("[dev-dependencies]\n{line}")),
        "{out}"
    );
    // The tables the author had are untouched.
    assert!(out.contains("[dependencies]\naxum = \"0.8.9\""), "{out}");
    assert!(out.contains("[build-dependencies]\nrahti-build"), "{out}");
}

/// Twice is once, in either spelling: the bare key, or a
/// `[dev-dependencies.name]` table of the author's own.
#[test]
fn a_dev_dependency_that_is_there_is_not_added_again() {
    let [(name, line), _] = t::DEV_DEPENDENCIES;
    let once = with_dev_dependency(MANIFEST, name, line).expect("an amended manifest");
    assert_eq!(with_dev_dependency(&once, name, line), None);

    let table = format!("{MANIFEST}\n[dev-dependencies.{name}]\nversion = \"0.1\"\n");
    assert_eq!(with_dev_dependency(&table, name, line), None);
}

/// A crate the author already depends on outright is available to the tests
/// without a second declaration — and their version choice is theirs.
#[test]
fn an_ordinary_dependency_satisfies_the_dev_dependency() {
    let manifest = "[dependencies]\ntower = { version = \"0.5\", features = [\"full\"] }\n";
    let [_, (name, line)] = t::DEV_DEPENDENCIES;
    assert_eq!(name, "tower");
    assert_eq!(with_dev_dependency(manifest, name, line), None);
}

// -------------------------------------------------------------- ws feature

#[test]
fn the_ws_feature_is_folded_into_a_bare_version() {
    let out = with_ws_feature(MANIFEST)
        .expect("a readable manifest")
        .expect("an amended manifest");
    assert!(
        out.contains("rahti = { version = \"0.1\", features = [\"ws\"] }"),
        "{out}"
    );
    // The build dependency has no such feature, and starts with the same word.
    assert!(out.contains("rahti-build = \"0.1\""), "{out}");
}

/// A `--local` project points at a checkout, and has to keep pointing at it.
#[test]
fn the_ws_feature_is_folded_into_a_path_dependency() {
    let manifest = "[dependencies]\nrahti = { path = \"/checkout/crates/rahti\" }\n";
    let out = with_ws_feature(manifest)
        .expect("a readable manifest")
        .expect("an amended manifest");
    assert!(
        out.contains("rahti = { path = \"/checkout/crates/rahti\", features = [\"ws\"] }"),
        "{out}"
    );
}

/// A feature list the author already wrote is joined, not replaced.
#[test]
fn an_existing_feature_list_keeps_what_is_in_it() {
    let manifest = "[dependencies]\nrahti = { version = \"0.1\", features = [\"other\"] }\n";
    let out = with_ws_feature(manifest)
        .expect("a readable manifest")
        .expect("an amended manifest");
    assert!(out.contains("\"ws\""), "{out}");
    assert!(out.contains("\"other\""), "{out}");
}

#[test]
fn the_ws_feature_is_not_added_twice() {
    let manifest = "[dependencies]\nrahti = { version = \"0.1\", features = [\"ws\"] }\n";
    assert_eq!(
        with_ws_feature(manifest).expect("a readable manifest"),
        None
    );
}

/// The shapes this refuses rather than guesses at. Each is reported to the
/// author with the line to write, which is a better answer than a manifest
/// edited wrongly.
#[test]
fn an_unreadable_dependency_is_refused_rather_than_mangled() {
    assert!(with_ws_feature("[dependencies]\naxum = \"0.8.9\"\n").is_err());
    assert!(with_ws_feature("[dependencies.rahti]\nversion = \"0.1\"\n").is_err());
    assert!(
        with_ws_feature("[dependencies]\nrahti = {\n  version = \"0.1\",\n}\n").is_err(),
        "a dependency spread over several lines is not a value to rewrite"
    );
}

// --------------------------------------------------------------------- .env

#[test]
fn the_connection_string_is_added_above_what_is_there() {
    let existing = "AUTH_SECRET=\"abc\"\n";
    let out = with_database_url(existing, Backend::Postgres).expect("an amended file");

    assert!(out.contains("DATABASE_URL=\"postgres://"), "{out}");
    assert!(out.ends_with(existing), "{out}");
    // Idempotent, so a second upgrade does not stack a second heading.
    assert_eq!(with_database_url(&out, Backend::Postgres), None);
}

/// The author's own value is theirs, whatever backend the config names.
#[test]
fn an_existing_connection_string_is_left_alone() {
    let env = "DATABASE_URL=\"postgres://elsewhere/db\"\n";
    assert_eq!(with_database_url(env, Backend::Sqlite), None);
}

/// A test-only feature declares the crate a second time, in
/// `[dev-dependencies]`. Both pins move together: two versions of one crate
/// do not resolve.
#[test]
fn a_dev_dependency_pin_moves_with_the_main_one() {
    let manifest = "[dependencies]\nrahti = { version = \"0.0.44\", features = [\"ws\"] }\n\n\
                    [dev-dependencies]\nrahti = { version = \"0.0.44\", features = [\"browser-test\"] }\n";
    let (out, was) = with_version(manifest, "rahti", "0.0.45").unwrap().expect("a bump");
    assert_eq!(was, "0.0.44");
    assert!(out.contains("rahti = { version = \"0.0.45\", features = [\"ws\"] }"), "{out}");
    assert!(
        out.contains("rahti = { version = \"0.0.45\", features = [\"browser-test\"] }"),
        "{out}"
    );
    assert!(!out.contains("0.0.44"), "{out}");
}

/// Platform tables are dependency tables too.
#[test]
fn a_target_dependency_pin_moves() {
    let manifest = "[target.'cfg(windows)'.dependencies]\nrahti = \"0.0.7\"\n";
    let (out, _) = with_version(manifest, "rahti", "0.0.9").unwrap().expect("a bump");
    assert!(out.contains("rahti = \"0.0.9\""), "{out}");
}

/// A `[patch]` points at a checkout, and `[workspace.dependencies]` or any
/// other table is not this project's pin: neither moves, even beside one
/// that does.
#[test]
fn only_dependency_tables_move() {
    let manifest = "[dependencies]\nrahti = \"0.0.7\"\n\n\
                    [patch.crates-io]\nrahti = { path = \"../rahti/crates/rahti\" }\n\n\
                    [workspace.dependencies]\nrahti = \"0.0.7\"\n\n\
                    [features]\nrahti = []\n";
    let (out, _) = with_version(manifest, "rahti", "0.0.9").unwrap().expect("a bump");
    assert!(out.starts_with("[dependencies]\nrahti = \"0.0.9\""), "{out}");
    assert!(out.contains("rahti = { path = \"../rahti/crates/rahti\" }"), "{out}");
    assert!(out.contains("[workspace.dependencies]\nrahti = \"0.0.7\""), "{out}");
    assert!(out.contains("[features]\nrahti = []"), "{out}");
}

/// The table form is refused by name, in whichever dependency table it is.
#[test]
fn a_dev_dependency_table_form_is_named() {
    let manifest = "[dependencies]\nrahti = \"0.0.7\"\n\n[dev-dependencies.rahti]\nversion = \"0.0.7\"\n";
    let err = with_version(manifest, "rahti", "0.0.9").unwrap_err();
    assert!(err.contains("[dev-dependencies.rahti]"), "{err}");
}
