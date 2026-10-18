//! Amending the two files the scaffold does not own.
//!
//! `Cargo.toml` and `.env` are the author's. Neither is in the ledger, and
//! neither is ever regenerated from a template: rewriting a manifest would
//! undo every dependency the author added and would replace a `--local`
//! project's path dependency with a version that may not be published, and
//! rewriting a `.env` would overwrite credentials.
//!
//! That is a rule about *replacing* a file, and it was read for a while as a
//! rule about touching one at all — so `cargo rahti upgrade --db` wrote
//! `src/db.rs`, which opens with `use sea_orm::…`, and then printed the two
//! `cargo add` commands that would make it compile. A tool that breaks the
//! build and leaves instructions has not finished, and the error the author
//! actually sees is `unresolved import sea_orm`, several minutes after the
//! note scrolled past.
//!
//! So the feature an upgrade adds is wired up the rest of the way here, by
//! the smallest edit that does it: a dependency appended to the end of
//! `[dependencies]`, a feature folded into the `rahti` line, a
//! `DATABASE_URL` put above the settings already in `.env`. Every function
//! below returns `None` when there is nothing to do, so a second run changes
//! nothing, and refuses rather than guesses when the file is not in a shape
//! it recognises — the caller prints what to do by hand instead.

use rahti_build::Backend;

use crate::templates as t;

/// The manifest with `line` added to `[dependencies]`, or `None` if a
/// dependency called `name` is already declared.
///
/// The name is checked separately from the line because they are not the
/// same question: an author who pinned their own `sea-orm` has one, in a
/// form that is theirs, and a second key with the same name is a manifest
/// cargo refuses to parse.
pub fn with_dependency(manifest: &str, name: &str, line: &str) -> Option<String> {
    if declares(manifest, name) {
        return None;
    }
    Some(append_to_table(manifest, "[dependencies]", line))
}

/// The manifest with `line` added to `[dev-dependencies]`, or `None` if a
/// dependency called `name` is already declared — in any table, because a
/// crate the author already depends on outright is available to the tests
/// without a second declaration.
pub fn with_dev_dependency(manifest: &str, name: &str, line: &str) -> Option<String> {
    let table = format!("[dev-dependencies.{name}]");
    if declares(manifest, name) || manifest.lines().any(|l| l.trim() == table) {
        return None;
    }
    Some(append_to_table(manifest, "[dev-dependencies]", line))
}

/// Whether `[dependencies]` names `name`, in either form cargo accepts.
///
/// Matched on the key rather than on the text, so `sea-orm-migration` is not
/// mistaken for `sea-orm`: the name has to be the whole key, with only an
/// `=` after it.
pub fn declares(manifest: &str, name: &str) -> bool {
    let table = format!("[dependencies.{name}]");
    manifest.lines().any(|line| {
        let line = line.trim();
        line == table
            || line
                .strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with('='))
    })
}

/// `line` written at the end of the named table.
///
/// The end of the table is the next table header, whatever it is — including
/// a `[dependencies.something]` sub-table, because a bare key written after
/// one of those would land in the sub-table and mean something else entirely.
/// Blank lines before that header are left where they are, so the addition
/// joins the list rather than the gap after it.
///
/// A manifest without the table at all gets one, at the end. Cargo allows
/// both — a package that lacks the table needs no entries in it, and table
/// order carries no meaning — and appending is the only answer that does not
/// depend on where anything else sits.
fn append_to_table(manifest: &str, header: &str, line: &str) -> String {
    let ends_with_newline = manifest.ends_with('\n');
    let mut lines: Vec<String> = manifest.lines().map(str::to_string).collect();

    match lines.iter().position(|l| l.trim() == header) {
        Some(header) => {
            let end = lines[header + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map(|offset| header + 1 + offset)
                .unwrap_or(lines.len());

            let mut at = end;
            while at > header + 1 && lines[at - 1].trim().is_empty() {
                at -= 1;
            }
            lines.insert(at, line.to_string());
        }
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header.to_string());
            lines.push(line.to_string());
        }
    }

    let mut out = lines.join("\n");
    if ends_with_newline {
        out.push('\n');
    }
    out
}

/// The manifest with `features = ["ws"]` on its `rahti` dependency.
///
/// `Ok(None)` when the feature is already there. `Err` names what stopped it,
/// for the caller to print: this is the one edit that has to understand the
/// value rather than append a line, and a dependency spelled across several
/// lines as a `[dependencies.rahti]` table is a shape it will not guess at.
pub fn with_ws_feature(manifest: &str) -> Result<Option<String>, String> {
    with_rahti_feature(manifest, "ws")
}

pub fn with_mcp_feature(manifest: &str) -> Result<Option<String>, String> {
    with_rahti_feature(manifest, "mcp")
}

fn with_rahti_feature(manifest: &str, feature: &str) -> Result<Option<String>, String> {
    if manifest.lines().any(|l| l.trim() == "[dependencies.rahti]") {
        return Err("`rahti` is declared as a [dependencies.rahti] table".to_string());
    }

    let at = manifest
        .lines()
        .position(|l| {
            l.trim_start()
                .strip_prefix("rahti")
                .is_some_and(|rest| rest.trim_start().starts_with('='))
        })
        .ok_or_else(|| "Cargo.toml has no `rahti` dependency".to_string())?;

    let mut lines: Vec<String> = manifest.lines().map(str::to_string).collect();
    let line = &lines[at];
    let (key, value) = line.split_once('=').expect("the line was found by its `=`");

    // Only the value is read, so an indented line stays indented.
    let value = value.trim();
    if value.starts_with('{') && !value.ends_with('}') {
        return Err("the `rahti` dependency spans more than one line".to_string());
    }
    if declares_feature(value, feature) {
        return Ok(None);
    }

    let wired = format!(
        "{key}= {}",
        if feature == "ws" {
            t::with_ws_feature(value)
        } else {
            t::with_mcp_feature(value)
        }
    );
    lines[at] = wired;

    let mut out = lines.join("\n");
    if manifest.ends_with('\n') {
        out.push('\n');
    }
    Ok(Some(out))
}

/// Whether a dependency value already lists the `ws` feature. Quoted, so a
/// path with `ws` in it — or the `tokio/time` a feature enables — is not read
/// as the feature itself.
fn declares_feature(value: &str, feature: &str) -> bool {
    value.contains(&format!("\"{feature}\""))
}

/// The manifest with `name`'s version pin moved to `current`, and the version
/// it moved from.
///
/// The framework ships in lockstep: `html!` expands to paths in `rahti` and
/// `rahti-build` writes code calling functions in it, so the files an upgrade
/// just wrote are written against the tool's own version. Leaving the pin
/// behind produces the failure that is hardest to find — a project that
/// compiles, runs locally on its configured port, and quietly does none of
/// what the new files say it does.
///
/// Every pin of `name` moves, in every dependency table — `[dependencies]`,
/// `[dev-dependencies]` (where a test-only feature such as `browser-test`
/// declares the crate a second time), `[build-dependencies]`, and their
/// `[target.….dependencies]` forms — because two pins of one crate at two
/// versions do not resolve. The version returned is the first one moved.
///
/// `Ok(None)` when there is nothing to do: no such dependency, path
/// dependencies only (a `--local` checkout or a `[patch]`, which a version
/// would break), pins already `current` or newer, or requirements the author
/// wrote to float on their own. `Err` names a shape this will not guess at,
/// for the caller to print.
pub fn with_version(
    manifest: &str,
    name: &str,
    current: &str,
) -> Result<Option<(String, String)>, String> {
    let mut lines: Vec<String> = manifest.lines().map(str::to_string).collect();
    let mut table = String::new();
    let mut first_moved: Option<String> = None;

    for index in 0..lines.len() {
        let trimmed = lines[index].trim();
        if trimmed.starts_with('[') {
            table = trimmed.trim_matches(|c| c == '[' || c == ']').to_string();
            if table.ends_with(&format!("dependencies.{name}")) {
                return Err(format!("`{name}` is declared as a [{table}] table"));
            }
            continue;
        }
        if !is_dependency_table(&table) {
            continue;
        }
        // The whole key, with only an `=` after it — so `rahti-build` is not
        // found while looking for `rahti`.
        let Some(rest) = lines[index].trim_start().strip_prefix(name) else {
            continue;
        };
        if !rest.trim_start().starts_with('=') {
            continue;
        }

        let (key, value) = lines[index]
            .split_once('=')
            .map(|(k, v)| (k.to_string(), v.trim().to_string()))
            .expect("the line was found by its `=`");
        if value.starts_with('{') && !value.ends_with('}') {
            return Err(format!(
                "the `{name}` dependency in [{table}] spans more than one line"
            ));
        }
        // A checkout is the one pin that must not become a version: a project
        // made with `--local` exists to test an unpublished framework.
        if value.contains("path") {
            continue;
        }
        let Some(pinned) = pinned_version(&value) else {
            continue;
        };
        if !is_older(&pinned, current) {
            continue;
        }
        lines[index] = format!("{key}= {}", value.replacen(&pinned, current, 1));
        first_moved.get_or_insert(pinned);
    }

    let Some(pinned) = first_moved else {
        return Ok(None);
    };
    let mut out = lines.join("\n");
    if manifest.ends_with('\n') {
        out.push('\n');
    }
    Ok(Some((out, pinned)))
}

/// A Cargo table that declares dependencies: `dependencies`,
/// `dev-dependencies`, `build-dependencies`, or any of them under
/// `target.<cfg>`. Not `workspace.dependencies` or `patch.*`, which say
/// something else about a crate than "this project uses this version".
fn is_dependency_table(table: &str) -> bool {
    let last = table.rsplit('.').next().unwrap_or(table);
    let kind = matches!(
        last,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    );
    kind && (table == last || table.starts_with("target."))
}

/// The version inside a dependency value, in either form the scaffold writes:
/// a bare `"0.0.7"`, or the `version = "0.0.7"` of an inline table.
fn pinned_version(value: &str) -> Option<String> {
    let rest = match value.strip_prefix('{') {
        Some(table) => table.split_once("version")?.1.split_once('=')?.1,
        None => value,
    };
    let quoted = rest.trim_start().strip_prefix('"')?;
    Some(quoted.split_once('"')?.0.to_string())
}

/// Whether `pinned` names an older release than `current`.
///
/// Compared as numbers per component, because `0.0.9` is newer than `0.0.7`
/// where a string comparison says the opposite. A missing component counts as
/// zero, so `0.1` is newer than `0.0.9`. Anything that is not a plain dotted
/// number — a caret, a wildcard, a pre-release — is the author asking for
/// something other than one exact version, and is not ours to move.
fn is_older(pinned: &str, current: &str) -> bool {
    fn parts(version: &str) -> Option<Vec<u64>> {
        version.split('.').map(|p| p.parse::<u64>().ok()).collect()
    }
    let (Some(was), Some(now)) = (parts(pinned), parts(current)) else {
        return false;
    };
    for i in 0..was.len().max(now.len()) {
        let (a, b) = (
            was.get(i).copied().unwrap_or(0),
            now.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a < b;
        }
    }
    false
}

/// A `.env` with `DATABASE_URL` in it, or `None` if it already has one.
///
/// Prepended rather than appended, which is where `cargo rahti new` puts it:
/// the section is a heading and a value, and a connection string under the
/// "AUTHENTICATION AND SESSIONS" heading reads as one of its settings.
pub fn with_database_url(env: &str, backend: Backend) -> Option<String> {
    let present = env
        .lines()
        .any(|l| l.trim_start().starts_with("DATABASE_URL"));
    if present {
        return None;
    }

    let mut out = t::database_env(backend);
    out.push_str(env);
    Some(out)
}

#[cfg(test)]
#[path = "tests/wiring.rs"]
mod tests;
