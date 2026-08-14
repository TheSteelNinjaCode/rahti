//! Live reload.
//!
//! Two different things change while you work, and they need two different
//! answers:
//!
//! - `public/` is read at runtime, so a stylesheet edit needs no rebuild.
//!   The server watches that directory itself and pushes an event.
//! - `src/app/` and `src/components/` are compiled into the binary by
//!   `rahti-build`, so a page edit needs a rebuild *and* a restart.
//!   Nothing inside a running process can observe that.
//!
//! The second case is handled by not trying to survive it. Every boot gets a
//! fresh id, handed to the browser over SSE. `EventSource` reconnects on its
//! own, so when the watcher respawns the server the client sees a different
//! id and reloads. That means the watcher has to *run* the server, not just
//! check it — plain `cargo watch` type-checks and leaves nothing listening.
//! The alias in `.cargo/config.toml` is the one to use:
//!
//! ```text
//! cargo dev
//! ```
//!
//! All of this is off in release builds, and `RAHTI_DEV` overrides either
//! way. When it is off, [`routes`] is an empty `Router` and [`inject`] is a
//! move, so the generated router can wire it in unconditionally.

use std::convert::Infallible;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Map, Value, json};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};

#[cfg(test)]
#[path = "tests/dev.rs"]
mod tests;

/// Forces dev mode on (`1`, `true`, `on`) or off (`0`, `false`, `off`),
/// whichever way the build profile leans.
pub const DEV_ENV: &str = "RAHTI_DEV";

const SSE_PATH: &str = "/__rahti/dev";
const SCRIPT_PATH: &str = "/__rahti/dev.js";
const LOG_PATH: &str = ".rahti/dev.log";
const LOG_INGEST_PATH: &str = "/__rahti/dev/log";
const MAX_LOG_BODY: usize = 128 * 1024;
const MAX_LOG_ENTRIES: usize = 50;
const MAX_LOG_FIELD: usize = 16 * 1024;

static LOG_WRITE: Mutex<()> = Mutex::new(());
static DIAGNOSTICS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Marks a document as already carrying the client.
///
/// `apply_layout` runs once per layout in the chain, so a page with nested
/// layouts passes through injection more than once.
const MARKER: &str = "id=\"rahti-dev\"";

/// A burst of filesystem events for one save collapses into one message.
const DEBOUNCE: Duration = Duration::from_millis(150);

/// Whether live reload is wired up at all.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var(DEV_ENV) {
        Ok(value) => flag(&value),
        Err(_) => cfg!(debug_assertions),
    })
}

/// How `RAHTI_DEV` reads. Anything that is not an "on" word is off, so
/// `RAHTI_DEV=0` in a debug build turns live reload off.
fn flag(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "on" | "yes")
}

/// Identifies this process to the browser. Any new value means "you are
/// looking at output from a server that no longer exists".
fn boot_id() -> &'static str {
    static BOOT_ID: OnceLock<String> = OnceLock::new();
    BOOT_ID.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!("{nanos:x}-{:x}", std::process::id())
    })
}

/// What the client should do about a change.
#[derive(Debug, Clone, Copy)]
enum Signal {
    /// Stylesheets only — swap them in place.
    Css,
    /// Anything else under `public/`.
    Reload,
}

impl Signal {
    fn name(self) -> &'static str {
        match self {
            Signal::Css => "css",
            Signal::Reload => "reload",
        }
    }
}

/// The dev routes, or an empty `Router` outside dev mode.
///
/// `public_dir` is the directory to watch — the same path the router serves
/// static files from.
pub fn routes(public_dir: PathBuf) -> Router {
    if !enabled() {
        return Router::new();
    }

    // Capacity is generous because a client that falls behind gets lagged
    // off the channel, and a dropped reload is a change you never see.
    let (tx, _) = broadcast::channel::<Signal>(64);
    watch_public(public_dir, tx.clone());
    if !cfg!(test) {
        init_diagnostics();
        DIAGNOSTICS_ACTIVE.store(true, Ordering::Release);
    }

    println!(
        "Rahti dev: live reload and diagnostics on, boot {}",
        boot_id()
    );
    println!("Rahti dev: browser/backend diagnostics -> {LOG_PATH}");

    Router::new()
        .route(SSE_PATH, get(move || stream(tx.subscribe())))
        .route(SCRIPT_PATH, get(script))
        .route(LOG_INGEST_PATH, post(browser_log))
}

/// Make the browser revalidate instead of trusting its own cache.
///
/// `ServeDir` sends `Last-Modified` and no `Cache-Control`, and a response
/// with no explicit freshness is one a browser may cache *heuristically* —
/// RFC 9111 §4.2.2, in practice a tenth of the file's age. A stylesheet that
/// has sat unmodified for a day is thus good for hours, so a rebuild does not
/// reach the page: the reload fires, the request is made, and the cache
/// answers it. Nothing looks broken, the CSS is simply old.
///
/// `no-cache` does not mean "do not store" — it means "ask first". The
/// conditional request still gets a 304 when nothing changed, so this costs
/// a header and buys a stylesheet that is never stale.
///
/// Dev only: a production build wants those heuristics.
pub async fn revalidate(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let mut response = next.run(request).await;
    if enabled() {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    response
}

/// Put the client into a finished document.
///
/// Called on every layout in the chain; the marker keeps the second and
/// later passes from stacking another copy.
pub fn inject(html: String) -> String {
    if !enabled() || html.contains(MARKER) {
        return html;
    }
    insert_client(html)
}

/// Where the client goes in a document that does not have one yet.
fn insert_client(html: String) -> String {
    // A classic script at the end of the parsed body executes before deferred
    // module scripts from the document head. That lets it wrap `console` and
    // install global error listeners before PulsePoint's `mount()` can report
    // its first warning or failure.
    let tag = format!("<script {MARKER} src=\"{SCRIPT_PATH}\"></script>");

    // Last `</body>`, so a literal one inside page content cannot capture
    // the tag. `</html>` covers documents that omit the body close tag.
    if let Some(at) = html.rfind("</body>").or_else(|| html.rfind("</html>")) {
        let mut out = String::with_capacity(html.len() + tag.len());
        out.push_str(&html[..at]);
        out.push_str(&tag);
        out.push_str(&html[at..]);
        return out;
    }

    html + &tag
}

async fn script() -> Response {
    let mut res = include_str!("dev.js").into_response();
    res.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    // The file is compiled in, so a stale copy would outlive a rebuild.
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// Receive warnings and errors captured by the injected browser client.
///
/// This route exists only when dev mode does. The body and each field are
/// bounded because a malformed page must not be able to grow a developer's
/// disk without limit.
async fn browser_log(body: Bytes) -> Response {
    if body.len() > MAX_LOG_BODY {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }

    let entries = match browser_entries(&body) {
        Ok(entries) => entries,
        Err(message) => return (StatusCode::BAD_REQUEST, message).into_response(),
    };

    for entry in entries {
        let level = entry
            .get("level")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let message = entry
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("browser diagnostic");
        let url = entry.get("url").and_then(Value::as_str).unwrap_or("");
        let source = source_of(&entry);
        let component = entry
            .get("component")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| format!(" [{value}]"))
            .unwrap_or_default();
        let url = if url.is_empty() {
            String::new()
        } else {
            format!(" — {url}")
        };

        eprintln!(
            "rahti browser {level}: {}{component}{url}{source}",
            limit(message, 1000)
        );
        append_diagnostic(entry);
    }

    StatusCode::NO_CONTENT.into_response()
}

fn browser_entries(body: &[u8]) -> Result<Vec<Value>, &'static str> {
    let value: Value =
        serde_json::from_slice(body).map_err(|_| "rahti dev: diagnostic body is not JSON")?;
    let values = value
        .as_array()
        .ok_or("rahti dev: diagnostic body must be an array")?;

    let mut entries = Vec::with_capacity(values.len().min(MAX_LOG_ENTRIES));
    for value in values.iter().take(MAX_LOG_ENTRIES) {
        let Some(input) = value.as_object() else {
            continue;
        };

        let mut entry = Map::new();
        entry.insert("kind".into(), Value::String("browser".into()));
        entry.insert("receivedAtMs".into(), json!(now_millis()));
        entry.insert("boot".into(), Value::String(boot_id().to_string()));

        for field in [
            "level",
            "message",
            "stack",
            "url",
            "source",
            "component",
            "timestamp",
        ] {
            if let Some(text) = input.get(field).and_then(Value::as_str) {
                entry.insert(field.into(), Value::String(limit(text, MAX_LOG_FIELD)));
            }
        }
        for field in ["line", "column"] {
            if let Some(number) = input.get(field).and_then(Value::as_u64) {
                entry.insert(field.into(), json!(number));
            }
        }

        entry
            .entry("level")
            .or_insert_with(|| Value::String("error".into()));
        entry
            .entry("message")
            .or_insert_with(|| Value::String("browser diagnostic".into()));
        entries.push(Value::Object(entry));
    }
    Ok(entries)
}

fn source_of(entry: &Value) -> String {
    let source = entry.get("source").and_then(Value::as_str).unwrap_or("");
    let line = entry.get("line").and_then(Value::as_u64);
    let column = entry.get("column").and_then(Value::as_u64);

    match (source.is_empty(), line, column) {
        (false, Some(line), Some(column)) => format!(" — {source}:{line}:{column}"),
        (false, Some(line), None) => format!(" — {source}:{line}"),
        (false, None, _) => format!(" — {source}"),
        _ => String::new(),
    }
}

fn limit(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…[truncated]", &value[..end])
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// Mirror one framework-handled backend diagnostic into the same file the
/// browser bridge uses. Application logging remains the application's choice;
/// this covers the failures Rahti itself turns into page or RPC responses.
pub(crate) fn record_server(level: &str, target: &str, message: &str) {
    eprintln!("{message}");
    if !DIAGNOSTICS_ACTIVE.load(Ordering::Acquire) {
        return;
    }
    append_diagnostic(json!({
        "kind": "server",
        "receivedAtMs": now_millis(),
        "boot": boot_id(),
        "level": level,
        "target": target,
        "message": limit(message, MAX_LOG_FIELD),
    }));
}

fn init_diagnostics() {
    if cfg!(test) {
        return;
    }
    static INITIALIZED: OnceLock<()> = OnceLock::new();
    INITIALIZED.get_or_init(|| {
        if let Some(parent) = Path::new(LOG_PATH).parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!("rahti dev: cannot create {}: {error}", parent.display());
                return;
            }
        }

        let session = json!({
            "kind": "session",
            "receivedAtMs": now_millis(),
            "boot": boot_id(),
            "message": "Rahti development diagnostics started",
        });
        let contents = format!("{session}\n");
        if let Err(error) = std::fs::write(LOG_PATH, contents) {
            eprintln!("rahti dev: cannot initialize {LOG_PATH}: {error}");
        }
    });
}

fn append_diagnostic(value: Value) {
    let Ok(_guard) = LOG_WRITE.lock() else {
        return;
    };
    let mut file = match OpenOptions::new().create(true).append(true).open(LOG_PATH) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("rahti dev: cannot open {LOG_PATH}: {error}");
            return;
        }
    };
    if let Err(error) = writeln!(file, "{value}") {
        eprintln!("rahti dev: cannot write {LOG_PATH}: {error}");
    }
}

async fn stream(
    rx: broadcast::Receiver<Signal>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // The boot id goes out first so a client that reconnects after a
    // restart learns about it immediately rather than on the next edit.
    let boot = tokio_stream::once(Ok(Event::default().event("boot").data(boot_id())));

    let updates = BroadcastStream::new(rx)
        .filter_map(Result::ok)
        .map(|signal| Ok(Event::default().event(signal.name()).data("")));

    // A stream that stays pending until shutdown, then yields one `None`.
    //
    // `watch` runs now, while the request is being set up, rather than at
    // first poll — a broadcast receiver only hears what is sent after it
    // subscribes, so a later subscription could miss the signal entirely.
    let stop = tokio_stream::once(crate::shutdown::watch()).then(|shutdown| async {
        shutdown.fired().await;
        None
    });

    // The events above have no natural end: they run for as long as the page
    // is open. `axum::serve` shuts down by waiting for open connections, so
    // without an end of its own this one response would hold Ctrl+C forever.
    // Wrapping every event in `Some` and merging the `None` gives the stream
    // a terminator, and `map_while` stops the moment it arrives.
    let events = boot
        .chain(updates)
        .map(Some)
        .merge(stop)
        .map_while(|event| event);

    // Keep-alive comments stop an idle proxy, or the browser, from deciding
    // the connection died.
    Sse::new(events).keep_alive(KeepAlive::default())
}

/// Watch `public/` on a plain thread.
///
/// `notify` hands events to a blocking `std` channel, and the loop below is
/// what keeps the watcher alive — dropping it stops the watch.
fn watch_public(dir: PathBuf, tx: broadcast::Sender<Signal>) {
    std::thread::spawn(move || {
        use notify::{EventKind, RecursiveMode, Watcher};

        let (raw_tx, raw_rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(raw_tx) {
            Ok(watcher) => watcher,
            Err(e) => {
                eprintln!("rahti dev: cannot start the file watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&dir, RecursiveMode::Recursive) {
            // Not fatal: the boot-id half of live reload still works, and
            // that is the half that covers Rust changes.
            eprintln!("rahti dev: cannot watch {}: {e}", dir.display());
            return;
        }

        let mut last: Option<Instant> = None;
        for event in raw_rx {
            let Ok(event) = event else { continue };
            if !matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                continue;
            }
            if last.is_some_and(|at| at.elapsed() < DEBOUNCE) {
                continue;
            }
            last = Some(Instant::now());

            let css_only = !event.paths.is_empty() && event.paths.iter().all(is_css);
            // Fails only when nothing is connected, which is not an error.
            let _ = tx.send(if css_only {
                Signal::Css
            } else {
                Signal::Reload
            });
        }
    });
}

fn is_css(path: &PathBuf) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("css"))
}
