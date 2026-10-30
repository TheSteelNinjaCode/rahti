//! The project's `#[job]` functions, for the manifest.
//!
//! Jobs are registered at link time, so the running process knows them and
//! nothing else does. Agent tooling has to know them without running anything,
//! which is what reading the source here is for. `#[job]` refuses to compile
//! outside these two locations, so this inventory is complete.

use quote::ToTokens;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use syn::{FnArg, Item, Pat, ReturnType};

/// The job module, as one file.
pub const JOBS_FILE: &str = "src/jobs.rs";
/// The job module, split into a directory.
pub const JOBS_DIR: &str = "src/jobs";

pub struct Scan {
    /// One manifest entry per `#[job]`, sorted by file then name.
    pub jobs: Vec<Value>,
    /// Paths whose change has to rerun the build.
    pub watched: Vec<String>,
}

/// Read every job under `root`, which a build passes as the crate root.
pub fn scan(root: &Path) -> Scan {
    let mut files = Vec::new();
    if root.join(JOBS_FILE).is_file() {
        files.push(PathBuf::from(JOBS_FILE));
    }
    collect(root, Path::new(JOBS_DIR), &mut files);
    files.sort();

    let mut watched = Vec::new();
    let mut jobs = Vec::new();
    for file in &files {
        let relative = normalize(file);
        watched.push(relative.clone());
        // Unreadable or unparsable source is the compiler's to report, with a
        // better message than a build script could give.
        let Ok(source) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        let Ok(parsed) = syn::parse_file(&source) else {
            continue;
        };
        let mut found = Vec::new();
        functions_in(&parsed.items, &mut found);
        jobs.extend(found.iter().map(|function| describe(function, &relative)));
    }
    if root.join(JOBS_DIR).is_dir() {
        watched.push(JOBS_DIR.to_string());
    }

    jobs.sort_by_key(|job| (text(job, "file"), text(job, "name")));
    Scan { jobs, watched }
}

/// A stored job is found by name alone, so two functions cannot share one.
pub fn check_names(jobs: &[Value]) {
    for (index, job) in jobs.iter().enumerate() {
        if let Some(other) = jobs[..index].iter().find(|o| o["name"] == job["name"]) {
            panic!(
                "rahti: two jobs are named `{}`.\n  {}\n  {}\n  A worker finds a stored \
                 job by name alone. Rename one, or pin a distinct `name = \"…\"` on \
                 its `#[job]`.",
                text(job, "name"),
                text(other, "file"),
                text(job, "file"),
            );
        }
    }
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}

fn collect(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root.join(dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = dir.join(entry.file_name());
        let path = entry.path();
        if path.is_dir() {
            collect(root, &name, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(name);
        }
    }
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn functions_in<'a>(items: &'a [Item], out: &mut Vec<&'a syn::ItemFn>) {
    for item in items {
        match item {
            Item::Fn(f) if f.attrs.iter().any(|a| a.path().is_ident("job")) => out.push(f),
            Item::Mod(m) => {
                if let Some((_, items)) = &m.content {
                    functions_in(items, out);
                }
            }
            _ => {}
        }
    }
}

/// The Rust path `#[job]`'s handle is reached through from anywhere in the
/// application: `src/jobs/mail.rs` is `crate::jobs::mail`.
fn module_path(file: &str) -> String {
    let inner = file
        .strip_prefix("src/")
        .unwrap_or(file)
        .trim_end_matches(".rs")
        .trim_end_matches("/mod");
    format!("crate::{}", inner.replace('/', "::"))
}

fn describe(function: &syn::ItemFn, file: &str) -> Value {
    let signature = &function.sig;
    let function_name = signature.ident.to_string();

    let mut name = function_name.clone();
    let (mut attempts, mut timeout_secs, mut backoff_ms, mut max_backoff_ms) =
        (None, None, None, None);
    for attribute in function.attrs.iter().filter(|a| a.path().is_ident("job")) {
        // An attribute with a bad argument is refused by `#[job]` itself.
        let _ = attribute.parse_nested_meta(|meta| {
            let value = meta.value()?;
            if meta.path.is_ident("name") {
                name = value.parse::<syn::LitStr>()?.value();
                return Ok(());
            }
            let number = value.parse::<syn::LitInt>()?.base10_parse::<u64>()?;
            if meta.path.is_ident("attempts") {
                attempts = Some(number);
            } else if meta.path.is_ident("timeout_secs") {
                timeout_secs = Some(number);
            } else if meta.path.is_ident("backoff_ms") {
                backoff_ms = Some(number);
            } else if meta.path.is_ident("max_backoff_ms") {
                max_backoff_ms = Some(number);
            }
            Ok(())
        });
    }

    let parameters: Vec<_> = signature
        .inputs
        .iter()
        .filter_map(|arg| {
            let FnArg::Typed(arg) = arg else {
                return None;
            };
            let Pat::Ident(ident) = &*arg.pat else {
                return None;
            };
            Some((
                ident.ident.to_string(),
                arg.ty.to_token_stream().to_string(),
            ))
        })
        .collect();
    let arguments = parameters
        .iter()
        .map(|(parameter, _)| parameter.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let returns = match &signature.output {
        ReturnType::Default => "()".to_string(),
        ReturnType::Type(_, ty) => ty.to_token_stream().to_string(),
    };

    json!({
        "name": name,
        "function": function_name,
        "file": file,
        "line": signature.ident.span().start().line,
        "parameters": parameters
            .iter()
            .map(|(name, ty)| json!({ "name": name, "rustType": ty }))
            .collect::<Vec<_>>(),
        "returns": returns,
        "attempts": attempts,
        "timeoutSecs": timeout_secs,
        "backoffMs": backoff_ms,
        "maxBackoffMs": max_backoff_ms,
        "enqueue": format!("{}::{function_name}::enqueue({arguments}).await", module_path(file)),
    })
}

#[cfg(test)]
#[path = "tests/jobs.rs"]
mod tests;
