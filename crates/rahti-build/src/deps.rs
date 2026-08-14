//! The component libraries this app depends on.
//!
//! A component is a Rust function, so a crate can ship one and an app can
//! `cargo add` it. Its *styles* are the part that does not travel: class names
//! live in the library's `.rs` files, and those sit in the cargo registry —
//! outside every path `src/app/globals.css` tells Tailwind to scan. The
//! library would render with correct markup and no CSS behind it.
//!
//! So the paths are found here and handed to `css.rs`, which writes them into
//! a generated entry sheet as `@source` lines. A library opts in from its own
//! manifest:
//!
//! ```toml
//! [package.metadata.rahti]
//! components = "src"
//! ```
//!
//! Opt-in rather than automatic: pointing Tailwind at every crate in the
//! dependency graph would have it walking hundreds of megabytes of registry
//! source to find class names that are not there.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

#[cfg(test)]
#[path = "tests/deps.rs"]
mod tests;

/// Cargo reads this table and ignores it; it is ours by arrangement.
const MARKER: &str = "rahti";

/// One dependency that ships components.
///
/// A library says how it is styled, and the two answers are not exclusive:
///
/// - `components` — its class names are written in these `.rs` files, for a
///   Tailwind app to compile. Useless to an app that does not run Tailwind.
/// - `stylesheet` — plain CSS it ships, which works under either engine.
///
/// A library declaring only `components` is Tailwind-only, and saying so in
/// the manifest is what lets the build refuse it in a plain app instead of
/// rendering it unstyled with nothing to explain why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dep {
    /// The crate name, for messages.
    pub name: String,
    /// Its `Cargo.toml`, which is where the two keys below are declared.
    pub manifest: String,
    /// Absolute path to the directory Tailwind should scan, if it named one.
    pub components: Option<String>,
    /// Absolute path to the stylesheet it ships, if it ships one.
    pub stylesheet: Option<String>,
}

/// Every component library in the dependency graph, deepest version of the
/// question being "what will be linked into this binary".
///
/// Absolute paths, deliberately: they name a machine-specific registry
/// location, which is exactly why the file they are written into is generated
/// into `.rahti/` and never committed.
pub fn scan() -> Vec<Dep> {
    // The set of dependencies is the lock file's answer, so a change to it is
    // a reason to look again — a newly added component library has to reach
    // the stylesheet without a `cargo clean` first.
    println!("cargo:rerun-if-changed=Cargo.lock");

    let Some(metadata) = metadata() else {
        return Vec::new();
    };

    let mut deps = Vec::new();
    for package in packages(&metadata) {
        let Some(table) = marker(&package) else {
            continue;
        };
        let Some(name) = field(&package, "name") else {
            continue;
        };
        let Some(manifest) = field(&package, "manifest_path") else {
            continue;
        };

        let root = Path::new(&manifest)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let components = resolve(
            &root,
            field(&table, "components").as_deref(),
            &name,
            "components",
            true,
        );
        let stylesheet = resolve(
            &root,
            field(&table, "stylesheet").as_deref(),
            &name,
            "stylesheet",
            false,
        );

        // Declaring the table and then neither key says nothing at all.
        if components.is_none() && stylesheet.is_none() {
            continue;
        }

        deps.push(Dep {
            name,
            manifest: normalize(Path::new(&manifest)),
            components,
            stylesheet,
        });
    }

    // One order for a given dependency set, whoever built it — the generated
    // entry sheet is an input to Tailwind, and an input that reshuffles is an
    // output that reshuffles.
    deps.sort_by(|a, b| a.name.cmp(&b.name));
    deps.dedup();
    deps
}

/// Turn one declared path into an absolute one, or explain why it was dropped.
///
/// A marker pointing at nothing is the library's bug, not the app's, and the
/// app cannot fix it — so it is a warning here rather than a failed build in
/// somebody else's project.
fn resolve(
    root: &Path,
    declared: Option<&str>,
    name: &str,
    key: &str,
    want_dir: bool,
) -> Option<String> {
    let declared = declared?;
    let path = root.join(declared);

    let ok = if want_dir {
        path.is_dir()
    } else {
        path.is_file()
    };
    if !ok {
        warn(&format!(
            "`{name}` declares `package.metadata.rahti.{key} = \"{declared}\"`, but {} is \
             not {} — its styles will be missing from the compiled stylesheet.",
            normalize(&path),
            if want_dir { "a directory" } else { "a file" }
        ));
        return None;
    }

    Some(normalize(&path))
}

/// Ask cargo what is in the graph.
///
/// `--offline` is the point of the call being safe from a build script: the
/// lock file is already resolved by the time one runs, so there is nothing to
/// fetch, and reaching the network here would break `--offline` builds,
/// vendored trees and sandboxed CI the way downloading the Tailwind CLI
/// unasked would.
///
/// `--filter-platform` is what makes that true. Without it cargo resolves for
/// every platform at once and insists on packages this machine has no reason
/// to own — a Windows build failing on a macOS-only dependency of `notify`
/// that was never downloaded, which is exactly what it did the first time
/// this ran. The host target is the one being built for, and its packages are
/// on disk already.
fn metadata() -> Option<String> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());

    let mut command = Command::new(&cargo);
    command.args(["metadata", "--format-version", "1", "--offline"]);

    // Set by cargo for a build script. Absent only if something else is
    // running this, in which case cargo's own default target is right.
    if let Ok(target) = env::var("TARGET") {
        command.args(["--filter-platform", &target]);
    }

    let output = command.output();

    let output = match output {
        Ok(output) => output,
        Err(e) => {
            warn(&format!(
                "cannot run `cargo metadata` ({e}), so no dependency's \
                 components are being styled."
            ));
            return None;
        }
    };

    if !output.status.success() {
        // One line, deliberately: cargo prints the first line of a warning and
        // swallows the rest, so a message that explains itself on line two
        // arrives as a blank complaint — which is how this looked when
        // `--filter-platform` was still missing.
        warn(&format!(
            "`cargo metadata` failed ({}), so no dependency's components are \
             being styled — {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        ));
        return None;
    }

    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The `packages` array, one JSON object per element, still as text.
///
/// Split by depth rather than parsed: the answer wanted from this document is
/// three strings per package, and a JSON dependency to read metadata about
/// dependencies is a poor trade in a build script.
fn packages(metadata: &str) -> Vec<String> {
    let Some(start) = find_array(metadata, "\"packages\"") else {
        return Vec::new();
    };

    let bytes = metadata.as_bytes();
    let mut objects = Vec::new();
    let mut depth = 0usize;
    let mut begin = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (i, &byte) in bytes.iter().enumerate().skip(start) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            _ if in_string => {}
            b'{' => {
                if depth == 0 {
                    begin = i;
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    objects.push(metadata[begin..=i].to_string());
                }
            }
            // The close of `packages` itself, at the depth its objects sit in.
            b']' if depth == 0 => break,
            _ => {}
        }
    }

    objects
}

/// Where the array named by `key` begins, past the `[`.
fn find_array(text: &str, key: &str) -> Option<usize> {
    let at = text.find(key)?;
    let open = text[at..].find('[')? + at;
    Some(open + 1)
}

/// The `metadata.rahti` table from one package object, if it has one.
///
/// The nesting is looked at rather than searched for: `"components"` appears
/// in plenty of packages that never asked us to read it.
fn marker(package: &str) -> Option<String> {
    let metadata = object_at(package, "\"metadata\"")?;
    object_at(&metadata, &format!("\"{MARKER}\""))
}

/// The object value of `key`, as text.
fn object_at(text: &str, key: &str) -> Option<String> {
    let at = text.find(key)?;
    let open = text[at..].find('{')? + at;

    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (i, &byte) in bytes.iter().enumerate().skip(open) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            _ if in_string => {}
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(text[open..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// The string value of `"key"` at the top level of one JSON object.
fn field(object: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut from = 0usize;

    // A key can appear nested — `"name"` inside `dependencies`, for one — so
    // candidates are checked until one is found at depth 1, which is this
    // object's own.
    while let Some(at) = object[from..].find(&needle) {
        let at = from + at;
        if depth_at(object, at) == 1 {
            let rest = &object[at + needle.len()..];
            let colon = rest.find(':')?;
            if let Some(value) = string_after(&rest[colon + 1..]) {
                return Some(value);
            }
        }
        from = at + needle.len();
    }
    None
}

/// Brace depth at a byte offset, ignoring braces inside strings.
fn depth_at(text: &str, offset: usize) -> usize {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for &byte in text.as_bytes().iter().take(offset) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' if in_string => escaped = true,
            b'"' => in_string = !in_string,
            _ if in_string => {}
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

/// The next JSON string, unescaped enough for a path.
fn string_after(text: &str) -> Option<String> {
    let open = text.find('"')?;
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut escaped = false;

    for &byte in bytes.iter().skip(open + 1) {
        if escaped {
            // Cargo writes paths, so the escapes that turn up are `\\` and
            // `\"`. Anything else is passed through as itself.
            out.push(byte as char);
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'"' => return Some(out),
            _ => out.push(byte as char),
        }
    }
    None
}

/// Forward slashes, so a generated `@source` line reads the same everywhere
/// and Tailwind is not handed a Windows separator to interpret as an escape.
fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Every `.rs` file under a dependency, so cargo reruns when one changes.
///
/// A library edited through a `path` dependency is the case that matters: its
/// classes are compiled into this app's stylesheet, and without this the
/// stylesheet would keep whatever it had the first time.
pub fn watched(deps: &[Dep]) -> Vec<String> {
    let mut paths = Vec::new();
    for dep in deps {
        // The manifest declares which of the two keys below exist at all, so
        // editing it has to be a reason to look again. Registry manifests
        // never change; the one being written next door does, and without
        // this a library that just declared a `stylesheet` would not reach
        // the app until something unrelated forced a rebuild.
        paths.push(dep.manifest.clone());

        if let Some(dir) = &dep.components {
            collect(Path::new(dir), &mut paths);
        }
        // A shipped stylesheet is one file, and it is an input just the same.
        if let Some(sheet) = &dep.stylesheet {
            paths.push(sheet.clone());
        }
    }
    paths.sort();
    paths
}

fn collect(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            children.push(path);
        } else if name.ends_with(".rs") {
            out.push(normalize(&path));
        }
    }

    for child in children {
        collect(&child, out);
    }
}

/// Build scripts have one channel to the terminal, and this is it.
fn warn(message: &str) {
    println!("cargo:warning=rahti: {message}");
}
