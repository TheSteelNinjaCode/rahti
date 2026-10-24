//! In-process static HTML export through the application's real router.

use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

const OUTPUT_ENV: &str = "RAHTI_STATIC_OUTPUT";
const OUTPUT_MARKER: &str = ".rahti-static-output";
const MAX_PAGE_BYTES: usize = 32 * 1024 * 1024;

/// One generated page route considered by a static export.
#[derive(Debug, Clone)]
pub struct StaticRoute {
    patterns: Vec<&'static str>,
    paths: Option<Vec<String>>,
}

impl StaticRoute {
    /// A route whose URL contains no dynamic segment.
    pub fn fixed(path: &'static str) -> Self {
        Self {
            patterns: vec![path],
            paths: Some(vec![path.to_string()]),
        }
    }

    /// A dynamic route and the concrete paths its page declared.
    pub fn dynamic<I, S>(patterns: &'static [&'static str], paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            patterns: patterns.to_vec(),
            paths: Some(paths.into_iter().map(Into::into).collect()),
        }
    }

    /// A dynamic route with no `static_paths()` declaration.
    pub fn undeclared(patterns: &'static [&'static str]) -> Self {
        Self {
            patterns: patterns.to_vec(),
            paths: None,
        }
    }

    fn label(&self) -> String {
        self.patterns.join(" | ")
    }
}

/// A route the exporter deliberately did not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticExportSkip {
    pub path: String,
    pub reason: String,
}

/// What a completed static export produced.
#[derive(Debug, Default)]
pub struct StaticExportReport {
    pub output: PathBuf,
    pub exported: Vec<String>,
    pub skipped: Vec<StaticExportSkip>,
    /// Pages that render, but whose HTML appears to call a backend-only API.
    pub runtime_dependencies: Vec<String>,
    pub assets: usize,
}

impl StaticExportReport {
    /// Print the build result under the command that requested it.
    pub fn print(&self) {
        println!("Rahti static export -> {}", self.output.display());
        println!("  exported {} page(s)", self.exported.len());
        for path in &self.exported {
            println!("    {path}");
        }
        println!("  copied {} public asset(s)", self.assets);
        if !self.runtime_dependencies.is_empty() {
            println!(
                "  warning: {} page(s) reference RPCs or WebSockets that need the server:",
                self.runtime_dependencies.len()
            );
            for path in &self.runtime_dependencies {
                println!("    {path}");
            }
        }
        if !self.skipped.is_empty() {
            println!("  skipped {} route(s):", self.skipped.len());
            for skip in &self.skipped {
                println!("    {} — {}", skip.path, skip.reason);
            }
        }
    }
}

/// A build-wide failure. Per-route failures are skips in the report.
#[derive(Debug)]
pub struct StaticExportError(String);

impl fmt::Display for StaticExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StaticExportError {}

/// Output requested by `cargo rahti build --static`, if this process is one.
pub fn static_output() -> Option<PathBuf> {
    std::env::var(OUTPUT_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Render every declared page through `router` and copy its public assets.
pub async fn export_static(
    router: Router,
    routes: Vec<StaticRoute>,
    public_dir: impl AsRef<Path>,
    output_dir: impl AsRef<Path>,
) -> Result<StaticExportReport, StaticExportError> {
    let public_dir = absolute(public_dir.as_ref())?;
    let output_dir = absolute(output_dir.as_ref())?;
    guard_output(&public_dir, &output_dir)?;
    prepare_output(&output_dir)?;

    let mut report = StaticExportReport {
        output: output_dir.clone(),
        ..StaticExportReport::default()
    };
    if public_dir.is_dir() {
        report.assets = copy_public(&public_dir, &output_dir)?;
    }
    report.assets += write_client_registry(&output_dir)?;

    let mut written = HashSet::new();
    for route in routes {
        let label = route.label();
        let Some(paths) = route.paths else {
            report.skipped.push(StaticExportSkip {
                path: label,
                reason: "dynamic route has no `static_paths()` declaration".to_string(),
            });
            continue;
        };

        for path in paths {
            if !route
                .patterns
                .iter()
                .any(|pattern| matches_route(pattern, &path))
            {
                report.skipped.push(StaticExportSkip {
                    path,
                    reason: format!("does not match route {label}"),
                });
                continue;
            }
            let relative = match output_path(&path) {
                Ok(relative) => relative,
                Err(reason) => {
                    report.skipped.push(StaticExportSkip { path, reason });
                    continue;
                }
            };
            if !written.insert(relative.clone()) {
                report.skipped.push(StaticExportSkip {
                    path,
                    reason: "another route already writes the same output file".to_string(),
                });
                continue;
            }

            let request = match Request::builder()
                .uri(&path)
                .header(header::HOST, "rahti-static.local")
                .body(Body::empty())
            {
                Ok(request) => request,
                Err(error) => {
                    report.skipped.push(StaticExportSkip {
                        path,
                        reason: format!("is not a valid request URI: {error}"),
                    });
                    continue;
                }
            };
            let response = router.clone().oneshot(request).await.map_err(|never| {
                StaticExportError(format!("the application router failed: {never}"))
            })?;

            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("an unnamed location");
                report.skipped.push(StaticExportSkip {
                    path,
                    reason: format!("redirected to {location}; it may require a session"),
                });
                continue;
            }
            if response.status() != StatusCode::OK {
                report.skipped.push(StaticExportSkip {
                    path,
                    reason: format!("returned HTTP {}", response.status()),
                });
                continue;
            }
            let html_response = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.starts_with("text/html"));
            if !html_response {
                report.skipped.push(StaticExportSkip {
                    path,
                    reason: "returned a non-HTML response".to_string(),
                });
                continue;
            }

            let bytes = match to_bytes(response.into_body(), MAX_PAGE_BYTES).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    report.skipped.push(StaticExportSkip {
                        path,
                        reason: format!("response exceeded the static page limit: {error}"),
                    });
                    continue;
                }
            };
            let html = match String::from_utf8(bytes.into()) {
                Ok(html) => html,
                Err(error) => {
                    report.skipped.push(StaticExportSkip {
                        path,
                        reason: format!("response was not UTF-8: {error}"),
                    });
                    continue;
                }
            };
            if uses_runtime_backend(&html) {
                report.runtime_dependencies.push(path.clone());
            }

            let target = output_dir.join(&relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|error| {
                    StaticExportError(format!("cannot create {}: {error}", parent.display()))
                })?;
            }
            fs::write(&target, html).map_err(|error| {
                StaticExportError(format!("cannot write {}: {error}", target.display()))
            })?;
            report.exported.push(path);
        }
    }

    Ok(report)
}

fn absolute(path: &Path) -> Result<PathBuf, StaticExportError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                StaticExportError(format!("cannot read the working directory: {error}"))
            })?
            .join(path)
    };
    Ok(normalize(path))
}

fn normalize(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn guard_output(public: &Path, output: &Path) -> Result<(), StaticExportError> {
    let cwd = absolute(Path::new("."))?;
    if output == cwd || output.parent().is_none() {
        return Err(StaticExportError(
            "the static output must be a dedicated directory, not the project or filesystem root"
                .to_string(),
        ));
    }
    if output.starts_with(public) || public.starts_with(output) {
        return Err(StaticExportError(format!(
            "the static output `{}` and public directory `{}` cannot contain one another",
            output.display(),
            public.display()
        )));
    }
    Ok(())
}

fn prepare_output(output: &Path) -> Result<(), StaticExportError> {
    if !output.exists() {
        fs::create_dir_all(output).map_err(|error| {
            StaticExportError(format!("cannot create {}: {error}", output.display()))
        })?;
    }
    if !output.is_dir() {
        return Err(StaticExportError(format!(
            "static output {} is not a directory",
            output.display()
        )));
    }

    let marker = output.join(OUTPUT_MARKER);
    let entries: Vec<_> = fs::read_dir(output)
        .map_err(|error| StaticExportError(format!("cannot read {}: {error}", output.display())))?
        .collect::<Result<_, _>>()
        .map_err(|error| StaticExportError(format!("cannot read {}: {error}", output.display())))?;
    if !entries.is_empty() && !marker.is_file() {
        return Err(StaticExportError(format!(
            "refusing to clear non-empty {} because it was not created by Rahti",
            output.display()
        )));
    }
    for entry in entries {
        let path = entry.path();
        if path == marker {
            continue;
        }
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        result.map_err(|error| {
            StaticExportError(format!("cannot clear {}: {error}", path.display()))
        })?;
    }
    fs::write(marker, "Owned by `cargo rahti build --static`.\n")
        .map_err(|error| StaticExportError(format!("cannot mark {}: {error}", output.display())))
}

/// The client registry every page's `main.js` imports — each binding, handler
/// and component script, compiled ahead of time — at the path the router
/// serves it from.
fn write_client_registry(output: &Path) -> Result<usize, StaticExportError> {
    let path = output.join(crate::client::PATH.trim_start_matches('/'));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            StaticExportError(format!("cannot create {}: {error}", parent.display()))
        })?;
    }
    fs::write(&path, &crate::client::module().source)
        .map_err(|error| StaticExportError(format!("cannot write {}: {error}", path.display())))?;
    Ok(1)
}

fn copy_public(source: &Path, target: &Path) -> Result<usize, StaticExportError> {
    let mut count = 0;
    for entry in fs::read_dir(source)
        .map_err(|error| StaticExportError(format!("cannot read {}: {error}", source.display())))?
    {
        let entry = entry.map_err(|error| {
            StaticExportError(format!("cannot read {}: {error}", source.display()))
        })?;
        let kind = entry.file_type().map_err(|error| {
            StaticExportError(format!(
                "cannot inspect {}: {error}",
                entry.path().display()
            ))
        })?;
        if kind.is_symlink() {
            return Err(StaticExportError(format!(
                "public asset {} is a symlink; static export does not follow links",
                entry.path().display()
            )));
        }
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir_all(&destination).map_err(|error| {
                StaticExportError(format!("cannot create {}: {error}", destination.display()))
            })?;
            count += copy_public(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &destination).map_err(|error| {
                StaticExportError(format!("cannot copy {}: {error}", entry.path().display()))
            })?;
            count += 1;
        }
    }
    Ok(count)
}

fn output_path(url: &str) -> Result<PathBuf, String> {
    if !url.starts_with('/') || url.contains('?') || url.contains('#') {
        return Err(
            "static paths must start with `/` and contain no query or fragment".to_string(),
        );
    }
    let mut relative = PathBuf::new();
    for segment in url.split('/').filter(|segment| !segment.is_empty()) {
        if matches!(segment, "." | "..")
            || segment.chars().any(|ch| {
                ch == '\0' || ch == '\\' || matches!(ch, ':' | '*' | '"' | '<' | '>' | '|')
            })
        {
            return Err(format!("path segment `{segment}` cannot be written safely"));
        }
        relative.push(segment);
    }
    relative.push("index.html");
    Ok(relative)
}

fn matches_route(pattern: &str, path: &str) -> bool {
    let patterns: Vec<_> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let paths: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    let mut at = 0;
    for pattern in patterns {
        if pattern.starts_with("{*") && pattern.ends_with('}') {
            return at < paths.len();
        }
        let Some(path) = paths.get(at) else {
            return false;
        };
        if !(pattern.starts_with('{') && pattern.ends_with('}')) && pattern != *path {
            return false;
        }
        at += 1;
    }
    at == paths.len()
}

/// Whether a page calls the server from the browser: in its markup (a
/// handler), or in a component script it names, whose code is in the client
/// registry rather than the page.
fn uses_runtime_backend(html: &str) -> bool {
    let calls_server = |code: &str| {
        ["pp.rpc(", "pp.socket(", "/__pulsepoint/ws"]
            .iter()
            .any(|needle| code.contains(needle))
    };
    calls_server(html)
        || html
            .split("pp-script=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter_map(crate::client::script)
            .any(calls_server)
}

#[cfg(test)]
#[path = "tests/static_export.rs"]
mod tests;
