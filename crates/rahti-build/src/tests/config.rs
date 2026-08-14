//! Configuration, and the rule that keeps the build from looping.

use super::*;
use crate::tree::Tree;

#[test]
fn a_full_configuration_is_read() {
    let config = parse_config(
        // Every value here differs from its default — the loopback host
        // included — so a reader that quietly ignored the file would fail
        // this rather than pass it by coincidence.
        r#"{"app":{"dir":"src/pages","public":"assets"},"server":{"host":"127.0.0.1","port":8080}}"#,
    );
    assert_eq!(config.app_dir, "src/pages");
    assert_eq!(config.public_dir, "assets");
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 8080);
}

/// Every key is optional: an app that has not configured anything gets the
/// layout the framework documents.
#[test]
fn everything_has_a_default() {
    let config = parse_config("{}");
    assert_eq!(config.app_dir, "src/app");
    assert_eq!(config.public_dir, "public");
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 3000);
}

/// An empty file is a project that has configured nothing, which is a
/// supported way to have a project — not a parse error.
#[test]
fn an_empty_file_is_all_defaults() {
    let config = parse_config("");
    assert_eq!(config.app_dir, "src/app");
    assert_eq!(config.port, 3000);
    assert_eq!(config.css.entry, "src/app/globals.css");
}

#[test]
fn a_partial_object_keeps_the_other_defaults() {
    let config = parse_config(r#"{"server":{"port":4000}}"#);
    assert_eq!(config.port, 4000);
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.app_dir, "src/app");
}

/// A key of the wrong type falls back rather than failing: the value is not
/// a port, so there is nothing to honour.
#[test]
fn a_value_of_the_wrong_type_falls_back() {
    assert_eq!(parse_config(r#"{"server":{"port":"8080"}}"#).port, 3000);
    assert_eq!(parse_config(r#"{"app":{"dir":42}}"#).app_dir, "src/app");
}

#[test]
#[should_panic(expected = "server.port must be 0-65535")]
fn a_port_outside_the_range_is_reported() {
    parse_config(r#"{"server":{"port":70000}}"#);
}

#[test]
#[should_panic(expected = "is not valid JSON")]
fn a_broken_file_is_reported() {
    parse_config(r#"{"server":{"port":3000"#);
}

// -------------------------------------------------------------------- schema

/// The quiet failure this avoids: a later Rahti gives an existing key a new
/// meaning, this build reads it the old way, and the app is configured as
/// nobody asked.
#[test]
#[should_panic(expected = "understands")]
fn a_newer_schema_is_refused() {
    parse_config(&format!(r#"{{"schema":{}}}"#, SCHEMA + 1));
}

#[test]
fn the_current_schema_is_read() {
    let config = parse_config(&format!(
        r#"{{"schema":{SCHEMA},"server":{{"port":4100}}}}"#
    ));
    assert_eq!(config.port, 4100);
}

/// A minimal hand-written config is a legitimate thing to have, so saying
/// nothing about the schema means the current one.
#[test]
fn a_missing_schema_is_assumed_current() {
    assert_eq!(parse_config(r#"{"server":{"port":4200}}"#).port, 4200);
}

// ----------------------------------------------------------------- provenance

/// `createdWith` and `scaffold` belong to the upgrade command. The build must
/// read straight past them rather than trip over keys it has no use for.
#[test]
fn provenance_keys_are_ignored_by_the_build() {
    let config = parse_config(
        r#"{"schema":1,"createdWith":"0.1.0",
            "scaffold":{"src/app/globals.css":"e0e260ce"},
            "server":{"port":4300}}"#,
    );
    assert_eq!(config.port, 4300);
}

// -------------------------------------------------------------------- css

#[test]
fn the_stylesheet_settings_are_read() {
    let config = parse_config(
        r#"{"css":{"entry":"src/styles.css","output":"public/app.css",
             "version":"4.2.0","download":true,"minify":true,"engine":"plain"}}"#,
    );
    assert_eq!(config.css.entry, "src/styles.css");
    assert_eq!(config.css.output, "public/app.css");
    assert_eq!(config.css.version, "4.2.0");
    assert!(config.css.download);
    assert!(config.css.minify);
    assert_eq!(config.css.engine, Engine::Plain);
}

// ------------------------------------------------------------------- engine

#[test]
fn every_engine_is_named() {
    assert_eq!(
        parse_config(r#"{"css":{"engine":"tailwind"}}"#).css.engine,
        Engine::Tailwind
    );
    assert_eq!(
        parse_config(r#"{"css":{"engine":"plain"}}"#).css.engine,
        Engine::Plain
    );
    assert_eq!(
        parse_config(r#"{"css":{"engine":"none"}}"#).css.engine,
        Engine::None
    );
}

/// Saying nothing gets the engine the framework documents.
#[test]
fn the_default_engine_is_tailwind() {
    assert_eq!(parse_config("{}").css.engine, Engine::Tailwind);
    assert_eq!(
        parse_config(r#"{"css":{"minify":true}}"#).css.engine,
        Engine::Tailwind
    );
}

/// Both silent outcomes are bad: falling back to Tailwind downloads a
/// compiler nobody asked for, and falling back to `none` leaves every page
/// unstyled with no error to explain it.
#[test]
#[should_panic(expected = "is not an engine")]
fn a_misspelled_engine_is_refused() {
    parse_config(r#"{"css":{"engine":"tailwnid"}}"#);
}

/// Downloading is off until asked for, so a build never reaches the network
/// on someone's behalf.
#[test]
fn downloading_is_off_by_default() {
    let config = parse_config("{}");
    assert!(!config.css.download);
    assert!(!config.css.minify, "a committed sheet should not churn");
}

// ----------------------------------------------------------------- database

/// No `db` object is an answer, not a gap: the build creates no directories,
/// generates no modules, and never mentions SeaORM.
#[test]
fn a_project_without_a_db_object_has_no_database() {
    assert!(parse_config("{}").db.is_none());
    assert!(parse_config(r#"{"server":{"port":4000}}"#).db.is_none());
}

/// A way to turn it off without losing the shape of the object you had.
#[test]
fn an_explicit_null_is_off() {
    assert!(parse_config(r#"{"db":null}"#).db.is_none());
}

#[test]
fn a_full_database_configuration_is_read() {
    let config = parse_config(
        r#"{"db":{"backend":"postgres","models":"src/entities","migrations":"src/schema"}}"#,
    );
    let db = config.db.expect("a configured database");
    assert_eq!(db.backend, Backend::Postgres);
    assert_eq!(db.models, "src/entities");
    assert_eq!(db.migrations, "src/schema");
}

/// The directories default, because they are the ones the scaffold writes and
/// the documentation names.
#[test]
fn the_directories_default() {
    let db = parse_config(r#"{"db":{"backend":"mysql"}}"#)
        .db
        .expect("a database");
    assert_eq!(db.backend, Backend::MySql);
    assert_eq!(db.models, "src/models");
    assert_eq!(db.migrations, "src/migrations");
}

/// SQLite is what a project gets by default: it is the one backend that needs
/// no server running to answer.
#[test]
fn an_unnamed_backend_is_sqlite() {
    let db = parse_config(r#"{"db":{}}"#).db.expect("a database");
    assert_eq!(db.backend, Backend::Sqlite);
}

/// A trailing slash is the same directory, and generated paths that differ by
/// one would be written twice.
#[test]
fn a_trailing_slash_is_trimmed() {
    let db = parse_config(r#"{"db":{"models":"src/models/"}}"#)
        .db
        .expect("a database");
    assert_eq!(db.models, "src/models");
}

/// The same refusal `css.engine` makes, for the same reason: falling back to
/// SQLite would wire a database nobody is talking to, and falling back to none
/// would leave `crate::models` undeclared with nothing to explain it.
#[test]
#[should_panic(expected = "which is not a backend")]
fn a_misspelled_backend_is_refused() {
    parse_config(r#"{"db":{"backend":"postgress"}}"#);
}

#[test]
#[should_panic(expected = "which is not a backend")]
fn a_backend_of_the_wrong_type_is_refused() {
    parse_config(r#"{"db":{"backend":42}}"#);
}

/// The feature name is the one thing about a backend that is not
/// interchangeable, so it is stated once here rather than in each template.
#[test]
fn each_backend_names_its_cargo_feature() {
    assert_eq!(Backend::Sqlite.feature(), "sqlx-sqlite");
    assert_eq!(Backend::Postgres.feature(), "sqlx-postgres");
    assert_eq!(Backend::MySql.feature(), "sqlx-mysql");
}

// ------------------------------------------------------------------ writing

/// `src/routes.rs` is checked in and lives under a directory cargo watches;
/// rewriting it unconditionally would force a rebuild on every build.
#[test]
fn an_unchanged_file_is_left_alone() {
    let tree = Tree::new("write");
    let path = tree.path().join("routes.rs").to_string_lossy().to_string();

    write_if_changed(&path, "one\n");
    let first = std::fs::metadata(&path).unwrap().modified().unwrap();

    write_if_changed(&path, "one\n");
    let second = std::fs::metadata(&path).unwrap().modified().unwrap();

    assert_eq!(
        first, second,
        "the file was rewritten with the same contents"
    );
}

#[test]
fn a_change_is_written() {
    let tree = Tree::new("rewrite");
    let path = tree.path().join("routes.rs").to_string_lossy().to_string();

    write_if_changed(&path, "one\n");
    write_if_changed(&path, "two\n");
    assert_eq!(tree.read("routes.rs"), "two\n");
}

/// The manifest lives in `.rahti/`, which no one creates by hand.
#[test]
fn the_directories_above_a_new_file_are_created() {
    let tree = Tree::new("mkdir");
    let path = tree
        .path()
        .join(".rahti/manifest.json")
        .to_string_lossy()
        .to_string();

    write_if_changed(&path, "{}\n");
    assert_eq!(tree.read(".rahti/manifest.json"), "{}\n");
}
