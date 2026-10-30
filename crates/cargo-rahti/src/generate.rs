//! `cargo rahti generate` — add framework-shaped code to a project.
//!
//! Generators exist so the conventions are not something to remember: a job
//! comes out in the file the build reads, in the shape `#[job]` accepts, with
//! the module declared and the workers started. Each one is additive and
//! refuses rather than overwrites.

use crate::templates;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const JOBS_FILE: &str = "src/jobs.rs";
const JOBS_DIR: &str = "src/jobs";
const LIB_FILE: &str = "src/lib.rs";
const STORE_FILE: &str = "src/job_store.rs";
const JOBS_HEADER: &str = r#"//! Background jobs: work that runs after the response, with retries.
//!
//! A `#[job]` here is reached from anywhere as
//! `crate::jobs::name::enqueue(arguments).await`. Every retryable job runs more
//! than once, so write it to be idempotent. See
//! docs/conventions/background-jobs.md.

use rahti::job;
use rahti::jobs::JobError;
"#;

pub fn run(args: &[&str]) -> Result<(), String> {
    let root = std::env::current_dir().map_err(|error| error.to_string())?;
    if !root.join("rahti.config.json").is_file() {
        return Err("this is not a Rahti project: rahti.config.json is missing".to_string());
    }
    match args {
        ["job", name, rest @ ..] => job(&root, name, rest),
        ["job-store"] => job_store(&root),
        _ => Err(
            "usage: cargo rahti generate job <name> [--params name:Type,…]\n       \
             cargo rahti generate job-store"
                .to_string(),
        ),
    }
}

fn job(root: &Path, name: &str, args: &[&str]) -> Result<(), String> {
    check_identifier(name)?;
    let params = match args {
        [] => Vec::new(),
        ["--params", list] => parse_params(list)?,
        [other, ..] => return Err(format!("unknown option `{other}`")),
    };

    if root.join(JOBS_DIR).is_dir() && !root.join(JOBS_FILE).is_file() {
        return Err(format!(
            "this project keeps its jobs under {JOBS_DIR}/. Add `{name}` to the file it \
             belongs in."
        ));
    }

    let path = root.join(JOBS_FILE);
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if defines(&existing, name) {
        return Err(format!("{JOBS_FILE} already defines `{name}`"));
    }

    let mut source = if existing.is_empty() {
        JOBS_HEADER.to_string()
    } else {
        existing
    };
    source.push_str(&job_source(name, &params));
    fs::write(&path, source).map_err(|error| error.to_string())?;
    println!("wrote {JOBS_FILE}: `{name}`");

    amend_lib(root, |lib| with_module(lib, "jobs", "Background jobs."))?;
    amend_lib(root, with_install)?;

    let arguments = params
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    println!("Enqueue it with `crate::jobs::{name}::enqueue({arguments}).await?`.");
    println!("Run `cargo check`, then read its entry under `jobs` in .rahti/manifest.json.");
    Ok(())
}

fn job_store(root: &Path) -> Result<(), String> {
    let config: Value = fs::read_to_string(root.join("rahti.config.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .ok_or("cannot read rahti.config.json")?;
    if !config.get("db").is_some_and(Value::is_object) {
        return Err(
            "this project has no database. Add one with `cargo rahti upgrade --db`, then \
             run this again."
                .to_string(),
        );
    }

    if root.join(STORE_FILE).exists() {
        return Err(format!("{STORE_FILE} already exists"));
    }
    let model = "src/models/job.rs";
    if root.join(model).exists() {
        return Err(format!("{model} already exists"));
    }

    let migrations = root.join("src/migrations");
    let (year, month, day) = civil_date(now_seconds());
    let migration = format!("src/migrations/m{year:04}{month:02}{day:02}_000001_create_job.rs");
    if !migrations.is_dir() {
        return Err("src/migrations/ does not exist".to_string());
    }
    if root.join(&migration).exists() {
        return Err(format!("{migration} already exists"));
    }

    for (path, contents) in [
        (STORE_FILE, templates::JOB_STORE_RS),
        (model, templates::MODEL_JOB_RS),
        (migration.as_str(), templates::MIGRATION_JOB),
    ] {
        fs::write(root.join(path), contents).map_err(|error| error.to_string())?;
        println!("wrote {path}");
    }

    amend_lib(root, |lib| {
        with_module(lib, "job_store", "Durable storage for background jobs.")
    })?;
    amend_lib(root, with_install)?;
    amend_lib(root, with_store)?;

    println!("Accepted jobs now live in the application's database.");
    println!(
        "Apply the new migration with `db::migrate()`, and add a test that calls \
         `rahti::jobs::conformance(&crate::job_store::SeaJobStore).await`."
    );
    println!(
        "To enqueue inside a transaction, call \
         `crate::job_store::enqueue_in(&txn, crate::jobs::name::request(arguments)?)`."
    );
    Ok(())
}

fn amend_lib(root: &Path, amend: impl Fn(&str) -> Option<String>) -> Result<(), String> {
    let path = root.join(LIB_FILE);
    let lib =
        fs::read_to_string(&path).map_err(|error| format!("cannot read {LIB_FILE}: {error}"))?;
    if let Some(amended) = amend(&lib) {
        fs::write(&path, amended).map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// Declare `module` after `pub mod routes;`, unless it is declared already.
pub(crate) fn with_module(lib: &str, module: &str, doc: &str) -> Option<String> {
    let declared = lib.lines().any(|line| {
        let line = line.trim();
        line == format!("mod {module};") || line == format!("pub mod {module};")
    });
    if declared {
        return None;
    }
    let anchor = "pub mod routes;\n";
    let declaration = format!("{anchor}\n/// {doc}\npub mod {module};\n");
    if lib.contains(anchor) {
        return Some(lib.replacen(anchor, &declaration, 1));
    }
    Some(format!("{lib}\n/// {doc}\npub mod {module};\n"))
}

/// Start the queue before the router, unless the project already does.
pub(crate) fn with_install(lib: &str) -> Option<String> {
    if lib.contains("Jobs::new()") {
        return None;
    }
    let anchor = "    Ok(ApplicationRuntime {";
    lib.contains(anchor)
        .then(|| lib.replacen(anchor, &format!("{}\n{anchor}", templates::INSTALL_JOBS), 1))
}

/// Point the queue at the database, unless it has a store already.
pub(crate) fn with_store(lib: &str) -> Option<String> {
    if lib.contains(".store(") {
        return None;
    }
    lib.contains("Jobs::new()").then(|| {
        lib.replacen(
            "Jobs::new()",
            "Jobs::new().store(job_store::SeaJobStore)",
            1,
        )
    })
}

pub(crate) fn defines(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        line.trim_start()
            .strip_prefix("pub ")
            .unwrap_or(line.trim_start())
            .strip_prefix("async fn ")
            .and_then(|rest| rest.strip_prefix(name))
            .is_some_and(|rest| rest.starts_with('('))
    })
}

pub(crate) fn job_source(name: &str, params: &[(String, String)]) -> String {
    let signature = params
        .iter()
        .map(|(name, ty)| format!("{name}: {ty}"))
        .collect::<Vec<_>>()
        .join(", ");
    let unused = params
        .iter()
        .map(|(name, _)| format!("{name},"))
        .collect::<String>();
    let body = if params.is_empty() {
        String::new()
    } else {
        format!("    let _ = ({unused});\n")
    };
    format!(
        "\n#[job]\npub async fn {name}({signature}) -> Result<(), JobError> {{\n{body}    \
         Ok(())\n}}\n"
    )
}

/// `order_id:i64,note:Option<String>`. A parameter with no type is a `String`,
/// and a comma inside a type's brackets does not end it.
pub(crate) fn parse_params(list: &str) -> Result<Vec<(String, String)>, String> {
    let mut params = Vec::new();
    for item in split_top_level(list) {
        let (name, ty) = item
            .split_once(':')
            .map_or((item.trim(), "String"), |(name, ty)| {
                (name.trim(), ty.trim())
            });
        check_identifier(name)?;
        if ty.is_empty() {
            return Err(format!("parameter `{name}` has an empty type"));
        }
        if ty.starts_with('&') {
            return Err(format!(
                "parameter `{name}` is borrowed, and a job argument is rebuilt from a \
                 stored payload: take it by value"
            ));
        }
        params.push((name.to_string(), ty.to_string()));
    }
    Ok(params)
}

fn split_top_level(list: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (at, character) in list.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                items.push(&list[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    items.push(&list[start..]);
    items
        .into_iter()
        .filter(|item| !item.trim().is_empty())
        .collect()
}

pub(crate) fn check_identifier(name: &str) -> Result<(), String> {
    const KEYWORDS: [&str; 40] = [
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while", "abstract", "yield",
    ];
    let mut characters = name.chars();
    let starts_well = characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first == '_');
    let valid = starts_well
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && name != "_"
        && !KEYWORDS.contains(&name);
    if valid {
        Ok(())
    } else {
        Err(format!(
            "`{name}` is not a usable name: use snake_case, and not a Rust keyword"
        ))
    }
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// The UTC calendar date of a Unix time, which names a migration. Days from
/// civil, inverted: a dependency for one filename would be a poor trade.
pub(crate) fn civil_date(seconds: i64) -> (i64, u32, u32) {
    let days = seconds.div_euclid(86_400) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
#[path = "tests/generate.rs"]
mod tests;
