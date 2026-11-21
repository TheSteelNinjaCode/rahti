//! Type-checking the project's browser scripts.
//!
//! A script under `public/js/` that starts with `// @ts-check` is checked
//! against `jsconfig.json` by an editor as it is typed, and by nothing else:
//! the Rust build never reads it, and a JSDoc type error never reaches the
//! browser, so it never reaches `.rahti/dev.log` either. An agent that works
//! from `cargo rahti restart` and `status` would never see one.
//!
//! This runs the TypeScript compiler the way an editor does and keeps what it
//! said in `.rahti/script-check.json`, so `cargo rahti restart`, `status` and
//! the MCP `server_status` tool all report the same errors. TypeScript is the
//! project's own `node_modules/.bin/tsc` when there is one, otherwise a `tsc`
//! on `PATH`; it is never downloaded. Without it the check is skipped, and
//! the report says how to install it.

use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where the last check's result is kept.
pub const RECORD_PATH: &str = ".rahti/script-check.json";
/// The TypeScript project the scaffold writes.
pub const CONFIG_PATH: &str = "jsconfig.json";
/// More than this many errors are counted, not listed.
const MAX_LISTED: usize = 50;
const SCRIPT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "ts", "mts", "cts"];

const INSTALL_HINT: &str = "TypeScript is not installed, so the scripts in public/js were not \
     type-checked. Install it with `npm install --global typescript`, or `npm install \
     --save-dev typescript` in the project.";

/// Check the scripts now, record the result, and return it.
pub fn run(root: &Path) -> Value {
    let result = check(root);
    if let Some(parent) = root.join(RECORD_PATH).parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(root.join(RECORD_PATH), result.to_string());
    with_staleness(root, result)
}

/// The last recorded result, without running anything. `stale` says a script
/// or `jsconfig.json` changed after it was taken, so it may no longer hold.
pub fn report(root: &Path) -> Value {
    if !root.join(CONFIG_PATH).is_file() {
        return skipped(format!(
            "{CONFIG_PATH} is absent, so there is nothing to check."
        ));
    }
    let recorded = fs::read_to_string(root.join(RECORD_PATH))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    match recorded {
        Some(result) => with_staleness(root, result),
        None => json!({
            "status": "unchecked",
            "stale": true,
            "errorCount": 0,
            "errors": [],
            "next": "Run `cargo rahti status` to type-check the scripts.",
        }),
    }
}

/// Whether a check is worth running: there is a config, and the last result
/// is missing, older than the scripts, or a skip (TypeScript may have been
/// installed since, and looking for it is cheap).
pub fn needs_run(root: &Path) -> bool {
    if !root.join(CONFIG_PATH).is_file() {
        return false;
    }
    let report = report(root);
    report["status"] == "skipped" || report["stale"] == true
}

fn check(root: &Path) -> Value {
    if !root.join(CONFIG_PATH).is_file() {
        return skipped(format!(
            "{CONFIG_PATH} is absent, so there is nothing to check."
        ));
    }
    let Some(tsc) = find_tsc(root) else {
        return skipped(INSTALL_HINT.to_string());
    };
    let checked_at = now_ms();
    let output = Command::new(&tsc)
        .args(["-p", CONFIG_PATH, "--pretty", "false", "--noEmit"])
        .current_dir(root)
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => return skipped(format!("cannot run {}: {error}", tsc.display())),
    };
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut errors = parse(&text);
    if !output.status.success() && errors.is_empty() {
        // Something tsc could not place in a file: a broken config, a crash.
        let message = text.lines().take(10).collect::<Vec<_>>().join("\n");
        errors.push(json!({
            "file": CONFIG_PATH,
            "line": 0,
            "column": 0,
            "code": "",
            "message": if message.trim().is_empty() {
                format!("tsc exited with {}", output.status)
            } else {
                message
            },
        }));
    }
    let count = errors.len();
    errors.truncate(MAX_LISTED);
    json!({
        "status": if count == 0 { "passed" } else { "failed" },
        "checkedAtMs": checked_at,
        "tool": tsc.strip_prefix(root).unwrap_or(&tsc).to_string_lossy().replace('\\', "/"),
        "errorCount": count,
        "errors": errors,
    })
}

fn skipped(reason: String) -> Value {
    json!({
        "status": "skipped",
        "stale": false,
        "errorCount": 0,
        "errors": [],
        "reason": reason,
    })
}

fn with_staleness(root: &Path, mut result: Value) -> Value {
    if result["status"] == "skipped" {
        result["stale"] = json!(false);
        return result;
    }
    let checked_at = result["checkedAtMs"].as_u64().unwrap_or(0);
    let stale = newest_script_ms(root).is_some_and(|newest| newest > checked_at);
    result["stale"] = json!(stale);
    result["next"] = json!(match (result["status"].as_str(), stale) {
        (_, true) => "A script changed after this check. Run `cargo rahti status` to check again.",
        (Some("failed"), false) =>
            "Fix these type errors. public/js is served live, so no restart is needed: \
             `cargo rahti status` checks again.",
        _ => "The scripts type-check.",
    });
    result
}

/// `file(line,col): error TS1234: message`, one per line, as `tsc --pretty
/// false` writes them. An indented line continues the message before it, and
/// a line with no location is an error about the project itself.
pub fn parse(output: &str) -> Vec<Value> {
    let mut errors: Vec<Value> = Vec::new();
    for line in output.lines() {
        if line.starts_with(' ') && !line.trim().is_empty() {
            if let Some(last) = errors.last_mut() {
                let message = format!(
                    "{}\n{}",
                    last["message"].as_str().unwrap_or(""),
                    line.trim()
                );
                last["message"] = json!(message);
            }
            continue;
        }
        if let Some(error) = parse_located(line) {
            errors.push(error);
        } else if let Some(rest) = line.strip_prefix("error ")
            && let Some((code, message)) = rest.split_once(": ")
        {
            errors.push(json!({
                "file": CONFIG_PATH,
                "line": 0,
                "column": 0,
                "code": code,
                "message": message,
            }));
        }
    }
    errors
}

fn parse_located(line: &str) -> Option<Value> {
    let (location, rest) = line.split_once("): ")?;
    let (file, position) = location.rsplit_once('(')?;
    let (row, column) = position.split_once(',')?;
    let rest = rest.strip_prefix("error ")?;
    let (code, message) = rest.split_once(": ")?;
    Some(json!({
        "file": file.replace('\\', "/"),
        "line": row.parse::<u64>().ok()?,
        "column": column.parse::<u64>().ok()?,
        "code": code,
        "message": message,
    }))
}

/// The project's own TypeScript, then one on `PATH`.
pub fn find_tsc(root: &Path) -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["tsc.cmd", "tsc.exe"]
    } else {
        &["tsc"]
    };
    let local = root.join("node_modules").join(".bin");
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::iter::once(local)
        .chain(std::env::split_paths(&path))
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// The newest modification among `jsconfig.json` and the scripts under the
/// public directory.
fn newest_script_ms(root: &Path) -> Option<u64> {
    let mut newest = modified_ms(&root.join(CONFIG_PATH));
    walk(&root.join(public_dir(root)), &mut newest);
    newest
}

fn public_dir(root: &Path) -> String {
    fs::read_to_string(root.join("rahti.config.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|config| config["app"]["public"].as_str().map(str::to_string))
        .unwrap_or_else(|| "public".to_string())
}

fn walk(dir: &Path, newest: &mut Option<u64>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, newest);
        } else if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension))
            && let Some(modified) = modified_ms(&path)
        {
            *newest = Some(newest.map_or(modified, |known| known.max(modified)));
        }
    }
}

fn modified_ms(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
#[path = "tests/script_check.rs"]
mod tests;
