//! Phase 3: make `src/components/` reachable without a mod file to maintain.
//!
//! Pages and layouts are wired up by their path — the router finds them. A
//! component is not routable, so it is reached by an ordinary `use`, and Rust
//! only sees a file that some `mod` declares. This walks the directory and
//! writes those declarations, so dropping `card.rs` into the tree is enough
//! to write `use crate::components::card::Card;` in a page.
//!
//! Unlike the app directory, the location is fixed: `src/main.rs` says
//! `mod components;`, and that name is the crate path components are imported
//! through.

use std::fs;
use std::path::Path;

use crate::route::Kind;
use crate::scanner::{self, Entry};

pub const COMPONENTS_DIR: &str = "src/components";

#[cfg(test)]
#[path = "tests/components.rs"]
mod tests;

/// One `#[rpc]` written beside a component.
///
/// A component renders wherever it is called, so unlike a page's rpc this one
/// belongs to no URL. What it belongs to is the app: the dispatcher every page
/// falls through to, which is the only answer that matches where the client
/// actually calls from.
#[derive(Debug, Clone)]
pub struct Rpc {
    /// The name the browser calls, which is the function's own.
    pub name: String,
    /// Module path below `crate::components`: `forms::input`.
    pub module: String,
    /// The file it is written in, for messages and the manifest.
    pub file: String,
    /// Whether the generated shim refuses a caller without a session.
    pub auth: bool,
}

/// The generated module files, plus what the manifest and cargo need.
pub struct Scan {
    /// `(path, contents)` for one `mod.rs` per directory.
    pub modules: Vec<(String, String)>,
    /// Every component file found, crate-root-relative.
    pub files: Vec<String>,
    /// Every `#[rpc]` found in those files, in the order they are read.
    pub rpcs: Vec<Rpc>,
    /// Every `#[socket]` found in those files, in the order they are read.
    /// The same shape as an rpc — a name, a module, a file — because the
    /// same three facts are what the manifest and the collision check need.
    pub sockets: Vec<Rpc>,
    /// Directories and files cargo must watch — see `scanner::Scan::watched`
    /// for why a directory is not enough on its own.
    pub watched: Vec<String>,
}

pub fn scan(dir: &Path) -> Scan {
    let mut scan = Scan {
        modules: Vec::new(),
        files: Vec::new(),
        rpcs: Vec::new(),
        sockets: Vec::new(),
        watched: vec![normalize(dir)],
    };

    // Created rather than reported missing: `mod components;` is unconditional
    // in `main.rs`, so a project that has not written a component yet still
    // has to compile.
    if !dir.is_dir() {
        fs::create_dir_all(dir)
            .unwrap_or_else(|e| panic!("rahti: cannot create {}: {e}", dir.display()));
    }

    walk(dir, &mut Vec::new(), &mut scan);
    scan
}

/// Collect one directory into a `mod.rs`, then recurse.
///
/// `parents` is the module chain below the components root, so a file knows
/// the `crate::components::…` path generated code has to name it by.
fn walk(dir: &Path, parents: &mut Vec<String>, scan: &mut Scan) {
    let mut modules: Vec<String> = Vec::new();
    let mut children: Vec<std::path::PathBuf> = Vec::new();

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
            modules.push(check_module_name(name, &path));
            children.push(path);
        } else if name.ends_with(".rs") && name != "mod.rs" {
            let stem = &name[..name.len() - 3];
            modules.push(check_module_name(stem, &path));

            let file = normalize(&path);
            let module = module_path(parents, stem);
            for callable in scanner::rpc_callables(&file) {
                scan.rpcs.push(Rpc {
                    name: callable.name,
                    module: module.clone(),
                    file: file.clone(),
                    auth: callable.auth,
                });
            }
            for callable in scanner::socket_callables(&file) {
                scan.sockets.push(Rpc {
                    name: callable.name,
                    module: module.clone(),
                    file: file.clone(),
                    auth: callable.auth,
                });
            }
            scan.files.push(file.clone());
            scan.watched.push(file);
        }
    }

    // Deterministic output regardless of how the filesystem enumerates.
    modules.sort();
    children.sort();

    scan.modules
        .push((normalize(&dir.join("mod.rs")), mod_rs(dir, &modules)));

    for child in children {
        scan.watched.push(normalize(&child));
        let name = child
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a directory that was named when it was collected")
            .to_string();
        parents.push(name);
        walk(&child, parents, scan);
        parents.pop();
    }
}

/// `crate::components::` plus this — `forms::input` for
/// `src/components/forms/input.rs`.
fn module_path(parents: &[String], stem: &str) -> String {
    let mut segments: Vec<&str> = parents.iter().map(String::as_str).collect();
    segments.push(stem);
    segments.join("::")
}

/// Hold the component rpcs against the ones the pages declare.
///
/// A component's rpc is reached by name from whatever page rendered it, so
/// the names are the app's rather than one file's: two components claiming
/// one name leaves nothing to pick between them, and a page claiming a name a
/// component already has would quietly win on that page and nowhere else.
/// Both are worth a build error over a confusing afternoon.
pub fn check_rpc_names(rpcs: &[Rpc], entries: &[Entry]) {
    let mut seen: Vec<&Rpc> = Vec::new();
    for rpc in rpcs {
        if let Some(other) = seen.iter().find(|s| s.name == rpc.name) {
            panic!(
                "rahti: two components declare an rpc named `{}`.\n  \
                 {}\n  {}\n  A component's rpcs are called by name from whatever \
                 page rendered it, so the names have to differ across \
                 `{COMPONENTS_DIR}/`.",
                rpc.name, other.file, rpc.file
            );
        }
        seen.push(rpc);
    }

    for entry in entries.iter().filter(|e| e.kind == Kind::Page) {
        for name in &entry.rpcs {
            if let Some(rpc) = rpcs.iter().find(|r| &r.name == name) {
                panic!(
                    "rahti: `{name}` is declared both as a page rpc and as a \
                     component rpc.\n  \
                     {}\n  {}\n  The page would answer its own and the component's \
                     would be unreachable there — rename one of them.",
                    entry.file, rpc.file
                );
            }
        }
    }
}

/// Hold every socket name against every other, wherever it was written.
///
/// Sockets connect to one endpoint and are dispatched by name alone, so the
/// namespace is flatter than the rpcs': a page's socket and a component's
/// live in the same one, and two pages cannot reuse a name the way two pages
/// reuse a page rpc. `all` is the app's full list — pages and components —
/// with the file each name came from.
pub fn check_socket_names(all: &[Rpc]) {
    let mut seen: Vec<&Rpc> = Vec::new();
    for socket in all {
        if let Some(other) = seen.iter().find(|s| s.name == socket.name) {
            panic!(
                "rahti: two sockets are named `{}`.\n  \
                 {}\n  {}\n  Every socket connects to one endpoint and is picked \
                 by name alone, so socket names are application-wide — page and \
                 component alike. Rename one of them.",
                socket.name, other.file, socket.file
            );
        }
        seen.push(socket);
    }
}

fn mod_rs(dir: &Path, modules: &[String]) -> String {
    let mut out = String::new();
    out.push_str("// @generated by rahti-build — do not edit.\n");
    out.push_str(&format!(
        "// Regenerated on every build from {}/\n",
        normalize(dir)
    ));
    out.push_str("//\n");
    out.push_str("// Every `.rs` file here is declared as a module, so a component is in scope\n");
    out.push_str("// as `crate::components::<file>::<Component>` as soon as it is saved.\n\n");

    // A component the app has not called yet is a component in progress, not
    // dead code — the same reason the `rahti` crate carries this.
    out.push_str("#![allow(dead_code, unused_imports)]\n");

    if modules.is_empty() {
        out.push_str(
            "\n// No components yet. Add `src/components/<name>.rs` with a `#[component]` fn.\n",
        );
        return out;
    }

    out.push('\n');
    for module in modules {
        out.push_str(&format!("pub mod {module};\n"));
    }
    out
}

/// Words rustc will not accept as a module name, minus the ones that are not
/// legal file names anyway.
const RESERVED: [&str; 30] = [
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type",
];

/// A file name becomes a module name verbatim, so reject the ones that would
/// surface as a syntax error inside generated code instead.
fn check_module_name(name: &str, path: &Path) -> String {
    let valid = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');

    if !valid {
        panic!(
            "rahti: `{}` cannot be a module.\n  \
             Name component files in snake_case — letters, digits and `_` — so \
             `user_card.rs` holds `UserCard`.",
            normalize(path)
        );
    }
    if RESERVED.contains(&name) {
        panic!(
            "rahti: `{}` is named after a Rust keyword, so it cannot be a module.\n  \
             Rename it — `{name}_component.rs` would work.",
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
