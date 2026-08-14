//! Reading `cargo metadata` without a JSON crate to do it with.
//!
//! The documents here are trimmed to the three fields that are read, plus the
//! shapes that would fool a naive search: a `"name"` nested in a dependency,
//! a `"metadata"` table belonging to somebody else, escapes inside a path.

use super::*;

/// One package object, as cargo writes it.
fn package(name: &str, manifest: &str, metadata: &str) -> String {
    format!(
        r#"{{"name":"{name}","version":"0.1.0","dependencies":[{{"name":"serde","req":"^1"}}],"manifest_path":"{manifest}","metadata":{metadata}}}"#
    )
}

fn document(packages: &[String]) -> String {
    format!(
        r#"{{"packages":[{}],"workspace_members":[],"resolve":null}}"#,
        packages.join(",")
    )
}

// ------------------------------------------------------------------- fields

#[test]
fn field_reads_a_top_level_string() {
    let object = package("rahti-ui", "/tmp/rahti-ui/Cargo.toml", "null");
    assert_eq!(field(&object, "name").as_deref(), Some("rahti-ui"));
    assert_eq!(
        field(&object, "manifest_path").as_deref(),
        Some("/tmp/rahti-ui/Cargo.toml")
    );
}

/// `dependencies` carries a `"name"` of its own, earlier in the document than
/// nothing and later than the real one — a plain `find` would be a coin toss.
#[test]
fn field_ignores_a_key_nested_deeper() {
    let object = r#"{"dependencies":[{"name":"serde"}],"name":"rahti-ui"}"#;
    assert_eq!(field(object, "name").as_deref(), Some("rahti-ui"));
}

#[test]
fn field_is_none_when_absent() {
    let object = package("rahti-ui", "/tmp/Cargo.toml", "null");
    assert_eq!(field(&object, "license"), None);
}

/// Windows manifest paths arrive with every separator doubled.
#[test]
fn field_unescapes_a_windows_path() {
    let object = r#"{"manifest_path":"C:\\src\\rahti-ui\\Cargo.toml"}"#;
    assert_eq!(
        field(object, "manifest_path").as_deref(),
        Some(r"C:\src\rahti-ui\Cargo.toml")
    );
}

// ------------------------------------------------------------------- marker

/// `marker` hands back the whole table; which keys matter is the caller's
/// business, because a library may declare either or both.
#[test]
fn marker_reads_the_components_directory() {
    let object = package(
        "rahti-ui",
        "/tmp/rahti-ui/Cargo.toml",
        r#"{"rahti":{"components":"src"}}"#,
    );
    let table = marker(&object).expect("the table");
    assert_eq!(field(&table, "components").as_deref(), Some("src"));
    assert_eq!(field(&table, "stylesheet"), None);
}

/// A library that ships plain CSS as well as Tailwind classes works under
/// either engine, so both keys have to survive the read.
#[test]
fn marker_reads_both_keys() {
    let object = package(
        "rahti-ui",
        "/tmp/rahti-ui/Cargo.toml",
        r#"{"rahti":{"components":"src","stylesheet":"ui.css"}}"#,
    );
    let table = marker(&object).expect("the table");
    assert_eq!(field(&table, "components").as_deref(), Some("src"));
    assert_eq!(field(&table, "stylesheet").as_deref(), Some("ui.css"));
}

#[test]
fn marker_is_none_without_the_table() {
    let object = package("serde", "/tmp/serde/Cargo.toml", "null");
    assert_eq!(marker(&object), None);
}

/// A `metadata` table that belongs to some other tool is not ours to read.
#[test]
fn marker_is_none_for_another_tools_metadata() {
    let object = package(
        "some-crate",
        "/tmp/some-crate/Cargo.toml",
        r#"{"docs":{"rs":{"components":"src"}}}"#,
    );
    assert_eq!(marker(&object), None);
}

// ----------------------------------------------------------------- packages

#[test]
fn packages_splits_one_object_per_element() {
    let doc = document(&[
        package("a", "/tmp/a/Cargo.toml", "null"),
        package(
            "b",
            "/tmp/b/Cargo.toml",
            r#"{"rahti":{"components":"src"}}"#,
        ),
    ]);

    let found = packages(&doc);
    assert_eq!(found.len(), 2, "{found:#?}");
    assert_eq!(field(&found[0], "name").as_deref(), Some("a"));
    assert!(marker(&found[0]).is_none(), "`a` declared no table");
    let table = marker(&found[1]).expect("`b` declared one");
    assert_eq!(field(&table, "components").as_deref(), Some("src"));
}

/// A brace inside a string is not a brace as far as the split is concerned —
/// crate descriptions contain all sorts of things.
#[test]
fn packages_is_not_confused_by_braces_in_strings() {
    let doc = format!(
        r#"{{"packages":[{{"name":"a","description":"uses {{}} and \" quotes","metadata":null}}]}}"#
    );
    let found = packages(&doc);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(field(&found[0], "name").as_deref(), Some("a"));
}

#[test]
fn packages_is_empty_for_a_document_without_the_array() {
    assert!(packages(r#"{"resolve":null}"#).is_empty());
}
