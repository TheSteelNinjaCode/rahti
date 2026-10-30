//! Reading a running development server from outside it.
//!
//! An agent's first question after an edit is whether the server is running
//! that edit. The server cannot answer alone, because it does not know what
//! changed on disk, and the source tree cannot answer alone, because it does
//! not know what is running. This module joins them.
//!
//! `cargo rahti status`, `cargo rahti restart` and the MCP tools all read
//! through here, so the three cannot disagree about what "stale" means. It
//! reads `.rahti/` and file timestamps, and never `.env`.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Written by a dev server once it can be reached.
pub const INFO_PATH: &str = ".rahti/dev-server.json";
/// Browser and backend diagnostics for the current boot.
pub const LOG_PATH: &str = ".rahti/dev.log";
/// A detached server's standard output.
pub const SERVER_LOG_PATH: &str = ".rahti/dev-server.log";

const TOKEN_HEADER: &str = "x-rahti-dev-token";
const STATUS_PATH: &str = "/__rahti/dev/status";
const SHUTDOWN_PATH: &str = "/__rahti/dev/shutdown";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_RESPONSE: u64 = 1024 * 1024;
const MAX_CHANGED_FILES: usize = 20;
const MAX_BROWSER_PROBLEMS: usize = 20;

/// What a dev server wrote about itself. The token never leaves this type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub pid: u64,
    pub url: String,
    pub address: String,
    pub boot: String,
    pub started_at_ms: u64,
    token: String,
}

pub fn read_info(root: &Path) -> Option<ServerInfo> {
    let text = fs::read_to_string(root.join(INFO_PATH)).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some(ServerInfo {
        pid: value.get("pid")?.as_u64()?,
        url: value.get("url")?.as_str()?.to_string(),
        address: value.get("address")?.as_str()?.to_string(),
        boot: value.get("boot")?.as_str()?.to_string(),
        started_at_ms: value.get("startedAtMs")?.as_u64()?,
        token: value.get("token")?.as_str()?.to_string(),
    })
}

/// The server's own account of itself, or `None` when nothing answers as the
/// boot the file names: a stale file from a crash, or another program that
/// took the port.
pub fn live_status(info: &ServerInfo) -> Option<Value> {
    let (status, body) = request(info, "GET", STATUS_PATH).ok()?;
    if status != 200 {
        return None;
    }
    let value: Value = serde_json::from_str(&body).ok()?;
    (value.get("boot").and_then(Value::as_str) == Some(info.boot.as_str())).then_some(value)
}

/// Ask a server to stop the way Ctrl+C does. Answers the HTTP status.
pub fn request_shutdown(info: &ServerInfo) -> Result<u16, String> {
    request(info, "POST", SHUTDOWN_PATH).map(|(status, _)| status)
}

/// One HTTP/1.1 exchange over a plain socket. The server is on this machine
/// and speaks fixed-length bodies, so an HTTP client would be a dependency in
/// every project that enables MCP for the sake of thirty lines.
fn request(info: &ServerInfo, method: &str, path: &str) -> Result<(u16, String), String> {
    let address = info
        .address
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .ok_or_else(|| format!("{} is not an address", info.address))?;
    let mut stream =
        TcpStream::connect_timeout(&address, TIMEOUT).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|error| error.to_string())?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}\r\n{TOKEN_HEADER}: {}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n",
        info.address, info.token
    );
    stream
        .write_all(head.as_bytes())
        .map_err(|error| error.to_string())?;

    let mut raw = Vec::new();
    stream
        .take(MAX_RESPONSE)
        .read_to_end(&mut raw)
        .map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or("the server's answer was not HTTP")?;
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// Source files edited since `since_ms`, newest first. `public/` is absent on
/// purpose: a running server serves those files live.
pub fn changed_since(root: &Path, since_ms: u64) -> Vec<String> {
    let mut changed = Vec::new();
    collect_changed(root, &root.join("src"), since_ms, &mut changed);
    for file in ["Cargo.toml", "rahti.config.json", ".env", "build.rs"] {
        let path = root.join(file);
        if let Some(modified) = modified_ms(&path)
            && modified > since_ms
        {
            changed.push((modified, file.to_string()));
        }
    }
    changed.sort();
    changed.reverse();
    changed
        .into_iter()
        .map(|(_, file)| file)
        .take(MAX_CHANGED_FILES)
        .collect()
}

fn collect_changed(root: &Path, dir: &Path, since_ms: u64, out: &mut Vec<(u64, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.is_dir() {
            collect_changed(root, &path, since_ms, out);
        } else if let Some(modified) = modified_ms(&path)
            && modified > since_ms
        {
            let relative = path.strip_prefix(root).unwrap_or(&path);
            out.push((modified, relative.to_string_lossy().replace('\\', "/")));
        }
    }
}

fn modified_ms(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let elapsed = modified.duration_since(UNIX_EPOCH).ok()?;
    u64::try_from(elapsed.as_millis()).ok()
}

/// What browsers reported to this boot: which routes really loaded, and what
/// went wrong on each. Only entries from `boot` count, so an old error cannot
/// fail a new server and an old load cannot prove one.
pub fn browser_report(root: &Path, boot: Option<&str>) -> Value {
    let text = fs::read_to_string(root.join(LOG_PATH)).unwrap_or_default();

    let mut routes: BTreeMap<String, (u64, u64, u64, u64)> = BTreeMap::new();
    let mut problems = Vec::new();
    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry.get("kind").and_then(Value::as_str) != Some("browser")
            || boot.is_some_and(|boot| entry.get("boot").and_then(Value::as_str) != Some(boot))
        {
            continue;
        }
        let level = entry
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
        let at = entry
            .get("receivedAtMs")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let route = routes.entry(route_of(url).to_string()).or_default();
        match level {
            "load" => {
                route.0 += 1;
                route.3 = route.3.max(at);
            }
            "warn" => route.2 += 1,
            _ => route.1 += 1,
        }
        if level != "load" {
            problems.push(json!({
                "level": level,
                "route": route_of(url),
                "message": entry.get("message"),
                "file": entry.get("file"),
                "atMs": at,
            }));
        }
    }

    let errors: u64 = routes.values().map(|route| route.1).sum();
    let warnings: u64 = routes.values().map(|route| route.2).sum();
    let skipped = problems.len().saturating_sub(MAX_BROWSER_PROBLEMS);
    json!({
        "boot": boot,
        "routes": routes
            .iter()
            .map(|(path, (loads, errors, warnings, last))| json!({
                "route": path,
                "loads": loads,
                "errors": errors,
                "warnings": warnings,
                "lastLoadAtMs": last,
            }))
            .collect::<Vec<_>>(),
        "errors": errors,
        "warnings": warnings,
        "problems": problems.split_off(skipped),
        "note": "A route that is not listed was never opened under this boot: no errors there proves nothing.",
    })
}

/// `http://localhost:3000/todo?x=1#y` to `/todo`.
fn route_of(url: &str) -> &str {
    let path = url
        .split_once("://")
        .map(|(_, rest)| rest.find('/').map_or("/", |at| &rest[at..]))
        .unwrap_or(url);
    path.split(['?', '#']).next().unwrap_or(path)
}

/// Everything an agent needs to decide its next step, with no secrets in it.
pub fn status(root: &Path) -> Value {
    let info = read_info(root);
    let live = info.as_ref().and_then(live_status);
    let Some((info, live)) = info.zip(live) else {
        return json!({
            "running": false,
            "stale": false,
            "changedFiles": [],
            "next": "Start the server with `cargo rahti restart`.",
        });
    };

    let changed = changed_since(root, info.started_at_ms);
    let stale = !changed.is_empty();
    json!({
        "running": true,
        "url": info.url,
        "pid": info.pid,
        "boot": info.boot,
        "startedAtMs": info.started_at_ms,
        "stale": stale,
        "changedFiles": changed,
        "browser": browser_report(root, Some(&info.boot)),
        "jobs": live.get("jobs"),
        "next": if stale {
            "The server is running old code. Run `cargo rahti restart`, then load the affected routes."
        } else {
            "The server is running the current source. Load the affected routes to verify them."
        },
    })
}

#[cfg(test)]
#[path = "tests/dev_server.rs"]
mod tests;
