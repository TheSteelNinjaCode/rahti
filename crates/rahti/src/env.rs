//! `.env`, read once into the process environment.
//!
//! Small enough to write, so it is written: a dependency to split a handful of
//! lines on `=` is a poor trade, and this way you can see exactly what it does.
//!
//! It lives in the framework rather than in the application because two
//! separate things now need it and only one of them is optional. `src/db.rs`
//! wants `DATABASE_URL`, and a project with no database does not have that
//! file — but it may still have an `AUTH_SECRET`, and a secret that is only
//! read when you happen to have a database would be a trap.
//!
//! **A real environment variable always wins.** That is the rule that makes
//! the file safe to keep around: a deployment sets its variables properly, and
//! a stale `.env` on the same machine cannot quietly point production at a
//! developer's database or sign its cookies with a developer's key. A missing
//! file is not an error — it is what a deployment looks like.
//!
//! Understood: `KEY=value`, blank lines, `#` comments, an optional `export`
//! prefix, and surrounding single or double quotes. Anything more elaborate
//! belongs in a real parser — swap in `dotenvy` if you outgrow it.

use std::sync::OnceLock;

#[cfg(test)]
#[path = "tests/env.rs"]
mod tests;

/// The file, relative to the working directory the server was started in.
const ENV_PATH: &str = ".env";

/// Read `.env` into the environment, once per process.
///
/// Idempotent and safe to call from anywhere that is about to read a variable
/// — `db::connect` and `AuthSettings::from_env` both do, and neither has to
/// know whether the other ran first.
pub fn load_env() {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        let Ok(contents) = std::fs::read_to_string(ENV_PATH) else {
            return;
        };
        for (key, value) in parse(&contents) {
            // A variable the environment already carries is the real one.
            if std::env::var_os(&key).is_some() {
                continue;
            }
            // Safety: this runs at most once, inside a `OnceLock`, and every
            // caller reaches it before the server starts serving — so no other
            // thread is reading the environment while this writes to it.
            unsafe { std::env::set_var(&key, &value) };
        }
    });
}

/// The pairs in a `.env`, in the order they are written.
///
/// Separate from the setting so it can be tested without touching the
/// process's own environment, which every other test shares.
fn parse(contents: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
            .unwrap_or(value);

        pairs.push((key.to_string(), value.to_string()));
    }

    pairs
}
