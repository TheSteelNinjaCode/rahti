//! Phase 4: make `src/models/` and `src/migrations/` reachable, the way
//! `src/components/` already is.
//!
//! Rahti does not depend on SeaORM and never will — the connection, the
//! entities and the migrations are the application's, written against a crate
//! it chose. What the build contributes is the wiring, which is the part
//! nobody enjoys keeping in step:
//!
//! - a `models/mod.rs` declaring every entity file, so saving `todo.rs` is
//!   enough to write `use crate::models::todo` in a page;
//! - a `migrations/mod.rs` declaring every migration file *and* the `Migrator`
//!   that lists them in order.
//!
//! The second is the one that earns its keep. SeaORM's migrator is an ordinary
//! `Vec` an author maintains by hand, and a migration that is written but not
//! pushed into it is the quietest possible failure: the file is there, the
//! table is not, and nothing anywhere says why. Generated from the directory,
//! that state cannot be reached.
//!
//! Both directories are flat. A table name is a flat namespace — there is no
//! `users::billing` table — and a nested entity tree would only invite a
//! module path that the database has no counterpart for.

use std::fs;
use std::path::Path;

use crate::Db;

#[cfg(test)]
#[path = "tests/db.rs"]
mod tests;

/// The generated module files, plus what the manifest and cargo need.
pub struct Scan {
    /// `(path, contents)` for each generated `mod.rs`.
    pub modules: Vec<(String, String)>,
    /// Every entity file found, crate-root-relative, for the manifest.
    pub models: Vec<String>,
    /// Every migration file found, in the order they will be applied.
    pub migrations: Vec<String>,
    /// Directories and files cargo must watch — a directory's mtime moves
    /// when a file is added or removed, but not when one is edited, so both
    /// are listed for the same reason `scanner` lists both.
    pub watched: Vec<String>,
}

pub fn scan(db: &Db) -> Scan {
    let models = collect(Path::new(&db.models), "models");
    let migrations = collect(Path::new(&db.migrations), "migrations");

    let mut watched = vec![db.models.clone(), db.migrations.clone()];
    watched.extend(models.iter().map(|(_, file)| file.clone()));
    watched.extend(migrations.iter().map(|(_, file)| file.clone()));

    let modules = vec![
        (
            normalize(&Path::new(&db.models).join("mod.rs")),
            models_mod_rs(&db.models, &names(&models)),
        ),
        (
            normalize(&Path::new(&db.migrations).join("mod.rs")),
            migrations_mod_rs(&db.migrations, &names(&migrations)),
        ),
    ];

    Scan {
        modules,
        models: models.into_iter().map(|(_, file)| file).collect(),
        migrations: migrations.into_iter().map(|(_, file)| file).collect(),
        watched,
    }
}

/// Every `.rs` file directly inside `dir`, as `(module, file)`.
///
/// Created rather than reported missing, for the same reason `components`
/// creates its own: once a project has configured a database its `main.rs`
/// declares these modules unconditionally, so a project that has not written
/// an entity yet still has to compile.
fn collect(dir: &Path, label: &str) -> Vec<(String, String)> {
    if !dir.is_dir() {
        fs::create_dir_all(dir)
            .unwrap_or_else(|e| panic!("rahti: cannot create {}: {e}", dir.display()));
        return Vec::new();
    }

    let mut found = Vec::new();
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("rahti: cannot read {}: {e}", dir.display()));

    for item in entries {
        let path = item.expect("rahti: unreadable directory entry").path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        // Editor droppings and `.git` are not modules.
        if name.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            panic!(
                "rahti: `{}` is a directory, and `{}` is flat.\n  \
                 A table is a flat namespace — there is no `users::billing` \
                 table — so an entity is one file next to the others. Move the \
                 files up a level.",
                normalize(&path),
                normalize(dir)
            );
        }

        if !name.ends_with(".rs") || name == "mod.rs" {
            continue;
        }

        let stem = &name[..name.len() - 3];
        found.push((check_module_name(stem, &path, label), normalize(&path)));
    }

    // Deterministic output regardless of how the filesystem enumerates — and
    // for migrations it is more than cosmetic: this order is the order they
    // are applied, which is why one is named for the moment it was written.
    found.sort();
    found
}

fn names(found: &[(String, String)]) -> Vec<String> {
    found.iter().map(|(module, _)| module.clone()).collect()
}

fn models_mod_rs(dir: &str, modules: &[String]) -> String {
    let mut out = header(dir);
    out.push_str("//\n");
    out.push_str("// Every `.rs` file here is declared as a module, so an entity is in scope\n");
    out.push_str("// as `crate::models::<file>` as soon as it is saved.\n//\n");
    out.push_str("// The entities themselves are yours. Rahti writes no SeaORM code and\n");
    out.push_str("// reads none: this file is module wiring and nothing else.\n\n");
    out.push_str("#![allow(dead_code, unused_imports)]\n");

    if modules.is_empty() {
        out.push_str(&format!(
            "\n// No entities yet. Add `{dir}/<table>.rs` with a `DeriveEntityModel` struct.\n"
        ));
        return out;
    }

    out.push('\n');
    for module in modules {
        out.push_str(&format!("pub mod {module};\n"));
    }
    out
}

/// The migration modules, and the `Migrator` that orders them.
///
/// `MigratorTrait` is declared with `#[async_trait]`, but every method that
/// needs it has a default body — the only one implemented here is an ordinary
/// synchronous `fn`. So the attribute is deliberately absent: writing it would
/// mean the application had to carry an `async-trait` dependency to compile a
/// file it did not write.
fn migrations_mod_rs(dir: &str, modules: &[String]) -> String {
    let mut out = header(dir);
    out.push_str("//\n");
    out.push_str("// The `Migrator` below lists every file in this directory, in filename\n");
    out.push_str("// order — which is why a migration is named for the moment it was\n");
    out.push_str("// written. Adding the file is the whole of adding the migration.\n\n");
    out.push_str("#![allow(dead_code, unused_imports)]\n\n");

    for module in modules {
        out.push_str(&format!("pub mod {module};\n"));
    }
    if modules.is_empty() {
        out.push_str(&format!(
            "// No migrations yet. Add `{dir}/mYYYYMMDD_NNNNNN_<what>.rs`.\n"
        ));
    }

    out.push_str("\n/// The application's migrations, in the order they are applied.\n");
    out.push_str("pub struct Migrator;\n\n");
    out.push_str("impl ::sea_orm_migration::MigratorTrait for Migrator {\n");
    out.push_str("    fn migrations() -> Vec<Box<dyn ::sea_orm_migration::MigrationTrait>> {\n");

    if modules.is_empty() {
        out.push_str("        Vec::new()\n");
    } else {
        out.push_str("        vec![\n");
        for module in modules {
            out.push_str(&format!("            Box::new({module}::Migration),\n"));
        }
        out.push_str("        ]\n");
    }

    out.push_str("    }\n}\n");
    out
}

fn header(dir: &str) -> String {
    let mut out = String::new();
    out.push_str("// @generated by rahti-build — do not edit.\n");
    out.push_str(&format!("// Regenerated on every build from {dir}/\n"));
    out
}

/// Words rustc will not accept as a module name, minus the ones that are not
/// legal file names anyway. The same list `components` refuses by, because the
/// failure is the same one: a file name becomes a module name verbatim.
const RESERVED: [&str; 30] = [
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type",
];

fn check_module_name(name: &str, path: &Path, label: &str) -> String {
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');

    if !valid {
        let advice = if label == "migrations" {
            "Name a migration `mYYYYMMDD_NNNNNN_<what>.rs`, so filename order is \
             the order it is applied."
        } else {
            "Name an entity file in snake_case after its table — `todo.rs`, \
             `order_line.rs`."
        };
        panic!(
            "rahti: `{}` cannot be a module.\n  {advice}",
            normalize(path)
        );
    }

    if RESERVED.contains(&name) {
        panic!(
            "rahti: `{}` is named after a Rust keyword, so it cannot be a module.\n  \
             Rename it — `{name}s.rs` would work.",
            normalize(path)
        );
    }

    name.to_string()
}

/// Crate-root-relative path with forward slashes, so generated code and the
/// manifest read the same on every platform.
fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
