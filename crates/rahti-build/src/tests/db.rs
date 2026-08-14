//! Making `src/models/` and `src/migrations/` reachable without a mod file
//! to maintain — and, for migrations, without a `Migrator` to maintain.

use super::*;
use crate::Backend;
use crate::tree::Tree;

/// A `Db` pointing at a throwaway tree, so a test writes nowhere real.
fn db_at(tree: &Tree) -> Db {
    Db {
        backend: Backend::Sqlite,
        models: normalize(&tree.path().join("models")),
        migrations: normalize(&tree.path().join("migrations")),
    }
}

/// The contents generated for one of the two directories.
fn generated(scan: &Scan, dir: &str) -> String {
    let wanted = format!("{dir}/mod.rs");
    scan.modules
        .iter()
        .find(|(path, _)| path.ends_with(&wanted))
        .map(|(_, contents)| contents.clone())
        .unwrap_or_else(|| panic!("no mod.rs for `{dir}`"))
}

#[test]
fn every_entity_becomes_a_module() {
    let tree = Tree::new("models");
    tree.file("models/todo.rs", "pub struct Model;")
        .file("models/order_line.rs", "pub struct Model;");

    let out = generated(&scan(&db_at(&tree)), "models");
    assert!(out.contains("pub mod todo;"), "{out}");
    assert!(out.contains("pub mod order_line;"), "{out}");
}

/// The generated file is checked in, so its order must not depend on the
/// filesystem.
#[test]
fn the_entity_declarations_are_ordered() {
    let tree = Tree::new("model-order");
    tree.file("models/zebra.rs", "")
        .file("models/alpha.rs", "")
        .file("models/middle.rs", "");

    let out = generated(&scan(&db_at(&tree)), "models");
    let at = |name: &str| out.find(name).unwrap_or_else(|| panic!("{name} missing"));
    assert!(
        at("alpha") < at("middle") && at("middle") < at("zebra"),
        "{out}"
    );
}

/// The point of the whole module: a migration file that exists is a migration
/// that runs. There is no second list to forget to add it to.
#[test]
fn a_migration_file_is_registered_in_the_migrator() {
    let tree = Tree::new("migrator");
    tree.file(
        "migrations/m20260731_000001_create_todo.rs",
        "pub struct Migration;",
    );

    let out = generated(&scan(&db_at(&tree)), "migrations");
    assert!(
        out.contains("pub mod m20260731_000001_create_todo;"),
        "{out}"
    );
    assert!(
        out.contains("Box::new(m20260731_000001_create_todo::Migration)"),
        "{out}"
    );
}

/// Filename order is application order, which is why a migration is named
/// after the moment it was written.
#[test]
fn migrations_are_applied_in_filename_order() {
    let tree = Tree::new("migration-order");
    tree.file("migrations/m20260801_000001_second.rs", "")
        .file("migrations/m20260731_000001_first.rs", "")
        .file("migrations/m20260901_000001_third.rs", "");

    let out = generated(&scan(&db_at(&tree)), "migrations");
    let at = |name: &str| {
        out.find(&format!("Box::new({name}"))
            .unwrap_or_else(|| panic!("{name} is not in the migrator:\n{out}"))
    };
    assert!(
        at("m20260731_000001_first") < at("m20260801_000001_second"),
        "{out}"
    );
    assert!(
        at("m20260801_000001_second") < at("m20260901_000001_third"),
        "{out}"
    );
}

/// `main.rs` declares these modules unconditionally once a project has a
/// database, so an empty project still has to compile — which means the
/// `Migrator` has to exist before there is anything to migrate.
#[test]
fn an_empty_project_still_compiles() {
    let tree = Tree::new("empty-db");
    let scan = scan(&db_at(&tree));

    let models = generated(&scan, "models");
    assert!(
        models.contains("#![allow(dead_code, unused_imports)]"),
        "{models}"
    );
    assert!(!models.contains("pub mod "), "{models}");

    let migrations = generated(&scan, "migrations");
    assert!(migrations.contains("pub struct Migrator;"), "{migrations}");
    assert!(migrations.contains("Vec::new()"), "{migrations}");
}

/// Neither directory has to exist first: a project that configured a database
/// but has written nothing yet gets them made, the way `src/components/` is.
#[test]
fn the_directories_are_created_when_missing() {
    let tree = Tree::new("missing-dirs");
    let db = db_at(&tree);
    scan(&db);

    assert!(Path::new(&db.models).is_dir());
    assert!(Path::new(&db.migrations).is_dir());
}

#[test]
fn mod_rs_is_not_itself_a_module() {
    let tree = Tree::new("skip-mod");
    tree.file("models/mod.rs", "// stale, about to be overwritten")
        .file("models/todo.rs", "");

    let out = generated(&scan(&db_at(&tree)), "models");
    assert!(!out.contains("pub mod mod;"), "{out}");
    assert!(out.contains("pub mod todo;"), "{out}");
}

/// Both are listed for the manifest, so tooling can answer "what is wired in"
/// without walking the tree itself.
#[test]
fn the_files_are_reported_for_the_manifest() {
    let tree = Tree::new("reported");
    tree.file("models/todo.rs", "")
        .file("migrations/m20260731_000001_create_todo.rs", "");

    let scan = scan(&db_at(&tree));
    assert!(
        scan.models[0].ends_with("models/todo.rs"),
        "{:?}",
        scan.models
    );
    assert!(
        scan.migrations[0].ends_with("migrations/m20260731_000001_create_todo.rs"),
        "{:?}",
        scan.migrations
    );
}

/// A directory's mtime moves when a file is added, but not when one is
/// edited — so editing an entity has to rebuild too.
#[test]
fn every_file_is_watched_as_well_as_its_directory() {
    let tree = Tree::new("watched");
    tree.file("models/todo.rs", "");

    let scan = scan(&db_at(&tree));
    assert!(
        scan.watched.iter().any(|w| w.ends_with("/models")),
        "{:?}",
        scan.watched
    );
    assert!(
        scan.watched.iter().any(|w| w.ends_with("models/todo.rs")),
        "{:?}",
        scan.watched
    );
}

#[test]
#[should_panic(expected = "is flat")]
fn a_nested_entity_directory_is_refused() {
    let tree = Tree::new("nested-models");
    tree.file("models/billing/invoice.rs", "");
    scan(&db_at(&tree));
}

#[test]
#[should_panic(expected = "cannot be a module")]
fn a_file_name_that_is_not_a_module_name_is_refused() {
    let tree = Tree::new("bad-name");
    tree.file("models/order-line.rs", "");
    scan(&db_at(&tree));
}

#[test]
#[should_panic(expected = "named after a Rust keyword")]
fn a_keyword_file_name_is_refused() {
    let tree = Tree::new("keyword");
    tree.file("models/type.rs", "");
    scan(&db_at(&tree));
}
