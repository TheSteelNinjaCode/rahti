//! Browser tests: the application in a real, headless browser.
//!
//! `cargo test` drives the router in-process, which proves what the server
//! sends. What PulsePoint then does with it — state, bindings, effects, the
//! DOM a library builds, what a click or a key press changes — happens in a
//! browser, and this module is how a test reaches one. It speaks the Chrome
//! DevTools Protocol to a headless Chrome or Edge it launches itself, so it
//! needs no WebDriver, no Node, and no browser download: it uses the one
//! already installed, and [`Browser::launch`] answers `None` when there is
//! none, so a suite can skip rather than fail on a machine without one.
//!
//! ```ignore
//! #[tokio::test]
//! async fn the_counter_counts() {
//!     let addr = rahti::browser::serve(crate::routes::router()).await;
//!     let Some(mut browser) = rahti::browser::Browser::launch().await.unwrap() else {
//!         return; // no Chrome or Edge on this machine
//!     };
//!     browser.goto(&format!("http://{addr}/")).await.unwrap();
//!     browser.click("button").await.unwrap();
//!     browser.wait_for("document.querySelector('p').textContent === '1'").await.unwrap();
//!     assert!(browser.errors().is_empty(), "{:?}", browser.errors());
//! }
//! ```
//!
//! Input goes through the protocol's input domain, so a click or a key press
//! is a trusted event — the same kind a person produces — and handlers that
//! check `event.isTrusted`, pointer capture, or `movementX` see the real thing.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

#[cfg(test)]
#[path = "tests/browser.rs"]
mod tests;

/// How long one protocol command, or one [`Browser::wait_for`], may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The environment variable naming a browser executable to use instead of
/// searching for Chrome and Edge.
pub const BROWSER_ENV: &str = "RAHTI_BROWSER";

/// Set (to anything but `0` or nothing) to make a missing browser an error
/// rather than a skipped test, so a machine that is meant to run the browser
/// suite cannot pass it without a browser.
pub const BROWSER_REQUIRED_ENV: &str = "RAHTI_BROWSER_REQUIRED";

/// What went wrong talking to the browser, in words a test failure can show.
#[derive(Debug, Clone)]
pub struct BrowserError(pub String);

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BrowserError {}

pub type BrowserResult<T> = Result<T, BrowserError>;

fn fail<T>(message: impl Into<String>) -> BrowserResult<T> {
    Err(BrowserError(message.into()))
}

/// A console message or uncaught error the page produced.
#[derive(Debug, Clone, PartialEq)]
pub struct ConsoleEntry {
    /// `log`, `info`, `warning`, `error`, … as the console reports it, or
    /// `exception` for an uncaught error or rejected promise.
    pub level: String,
    pub text: String,
}

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// One headless browser with one page, closed when dropped.
pub struct Browser {
    child: Child,
    profile: PathBuf,
    socket: Socket,
    session: String,
    next_id: u64,
    console: Vec<ConsoleEntry>,
    pointer: (f64, f64),
}

/// Serve a router on a free loopback port for a browser to load, the way the
/// binary serves it — with connection info, so rate limiting sees a client.
pub async fn serve(router: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let addr = listener.local_addr().expect("a bound address");
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    addr
}

/// Whether [`BROWSER_REQUIRED_ENV`] asks for a missing browser to fail.
pub fn browser_required() -> bool {
    required_by(std::env::var(BROWSER_REQUIRED_ENV).ok().as_deref())
}

fn required_by(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

/// The browser to drive: [`BROWSER_ENV`], else an installed Chrome, Chromium
/// or Edge.
pub fn find_browser() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BROWSER_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        for base in ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"] {
            if let Some(root) = std::env::var_os(base) {
                let root = PathBuf::from(root);
                candidates.push(root.join(r"Google\Chrome\Application\chrome.exe"));
                candidates.push(root.join(r"Chromium\Application\chrome.exe"));
                candidates.push(root.join(r"Microsoft\Edge\Application\msedge.exe"));
            }
        }
    } else if cfg!(target_os = "macos") {
        for app in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        ] {
            candidates.push(PathBuf::from(app));
        }
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        for dir in std::env::split_paths(&path) {
            for name in [
                "google-chrome",
                "google-chrome-stable",
                "chromium",
                "chromium-browser",
                "microsoft-edge",
            ] {
                candidates.push(dir.join(name));
            }
        }
    }
    candidates.into_iter().find(|candidate| candidate.is_file())
}

impl Browser {
    /// Launch a headless browser with a fresh profile and open a blank page.
    ///
    /// `Ok(None)` when no browser is installed: skip the test rather than
    /// fail it, and say so. With [`BROWSER_REQUIRED_ENV`] set, a missing
    /// browser is an error instead.
    pub async fn launch() -> BrowserResult<Option<Browser>> {
        let Some(executable) = find_browser() else {
            if browser_required() {
                return Err(BrowserError(format!(
                    "no Chrome, Chromium or Edge found, and {BROWSER_REQUIRED_ENV} is set. \
                     Install one, or set {BROWSER_ENV} to a browser executable."
                )));
            }
            eprintln!(
                "rahti browser test skipped: no Chrome, Chromium or Edge found \
                 (set {BROWSER_ENV} to a browser executable, and \
                 {BROWSER_REQUIRED_ENV}=1 to fail instead of skipping)"
            );
            return Ok(None);
        };
        Browser::launch_with(&executable).await.map(Some)
    }

    /// Launch a particular executable.
    pub async fn launch_with(executable: &Path) -> BrowserResult<Browser> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let profile = std::env::temp_dir().join(format!(
            "rahti-browser-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)
            .map_err(|e| BrowserError(format!("cannot create a browser profile: {e}")))?;

        let child = Command::new(executable)
            .args([
                "--headless=new",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-renderer-backgrounding",
                "--disable-background-timer-throttling",
                "--mute-audio",
                // WebGL in a headless browser, for pages that draw with it.
                "--use-angle=swiftshader",
                "--enable-unsafe-swiftshader",
                "--window-size=1280,800",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| BrowserError(format!("cannot start {}: {e}", executable.display())))?;

        // The browser writes its debugging port and path here once it listens.
        let port_file = profile.join("DevToolsActivePort");
        let started = Instant::now();
        let endpoint = loop {
            if let Ok(text) = std::fs::read_to_string(&port_file) {
                let mut lines = text.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                    break format!("ws://127.0.0.1:{}{}", port.trim(), path.trim());
                }
            }
            if started.elapsed() > TIMEOUT {
                let mut child = child;
                let _ = child.kill();
                return fail(format!(
                    "{} did not open its debugging port within {TIMEOUT:?}",
                    executable.display()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        let (socket, _) = connect_async(endpoint.as_str())
            .await
            .map_err(|e| BrowserError(format!("cannot reach the browser: {e}")))?;

        let mut browser = Browser {
            child,
            profile,
            socket,
            session: String::new(),
            next_id: 0,
            console: Vec::new(),
            pointer: (0.0, 0.0),
        };

        let target = browser
            .command(
                "Target.createTarget",
                json!({ "url": "about:blank" }),
                false,
            )
            .await?;
        let target_id = target["targetId"].as_str().unwrap_or_default().to_string();
        let attached = browser
            .command(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
                false,
            )
            .await?;
        browser.session = attached["sessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        browser.command("Page.enable", json!({}), true).await?;
        browser.command("Runtime.enable", json!({}), true).await?;
        // The browser reports a Content Security Policy violation on this
        // domain rather than the console's, so enable it to see one.
        browser.command("Log.enable", json!({}), true).await?;
        browser
            .command(
                "Emulation.setDeviceMetricsOverride",
                json!({ "width": 1280, "height": 800, "deviceScaleFactor": 1, "mobile": false }),
                true,
            )
            .await?;
        // A page in a headless browser is "visible", so animation frames
        // run; say so explicitly for pages that pause when hidden.
        let _ = browser
            .command(
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": true }),
                true,
            )
            .await;
        Ok(browser)
    }

    /// Load a URL and wait until the document has finished loading.
    pub async fn goto(&mut self, url: &str) -> BrowserResult<()> {
        let navigated = self
            .command("Page.navigate", json!({ "url": url }), true)
            .await?;
        if let Some(error) = navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            return fail(format!("cannot load {url}: {error}"));
        }
        self.wait_for("document.readyState === 'complete'").await
    }

    /// Evaluate JavaScript in the page and read the result back as `T`.
    /// A promise is awaited; a thrown error is an `Err` carrying its message.
    pub async fn eval<T: DeserializeOwned>(&mut self, expression: &str) -> BrowserResult<T> {
        let reply = self
            .command(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "awaitPromise": true,
                    "returnByValue": true,
                    "userGesture": true,
                    // DevTools may eval past the page's CSP by default; a
                    // test sees the page as a visitor's browser does.
                    "allowUnsafeEvalBlockedByCSP": false,
                }),
                true,
            )
            .await?;
        if let Some(details) = reply.get("exceptionDetails") {
            return fail(format!(
                "`{}` threw: {}",
                short(expression),
                exception_text(details)
            ));
        }
        let value = reply["result"].get("value").cloned().unwrap_or(Value::Null);
        serde_json::from_value(value).map_err(|e| {
            BrowserError(format!(
                "`{}` returned an unexpected value: {e}",
                short(expression)
            ))
        })
    }

    /// Wait until a JavaScript condition is truthy, up to [`TIMEOUT`].
    pub async fn wait_for(&mut self, condition: &str) -> BrowserResult<()> {
        self.wait_for_within(condition, TIMEOUT).await
    }

    /// Wait until a JavaScript condition is truthy, up to `timeout`.
    pub async fn wait_for_within(
        &mut self,
        condition: &str,
        timeout: Duration,
    ) -> BrowserResult<()> {
        let started = Instant::now();
        let probe =
            format!("(() => {{ try {{ return !!({condition}); }} catch {{ return false; }} }})()");
        loop {
            if self.eval::<bool>(&probe).await? {
                return Ok(());
            }
            if started.elapsed() > timeout {
                return fail(format!(
                    "waited {timeout:?} for `{condition}` and it never became true{}",
                    self.errors_suffix()
                ));
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait a fixed time — for animation that has no condition to wait on.
    pub async fn pause(&mut self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    /// Click the centre of the first element matching a CSS selector.
    pub async fn click(&mut self, selector: &str) -> BrowserResult<()> {
        let (x, y) = self.center_of(selector).await?;
        self.click_at(x, y, 1).await
    }

    /// Double-click the centre of the first element matching a selector.
    pub async fn double_click(&mut self, selector: &str) -> BrowserResult<()> {
        let (x, y) = self.center_of(selector).await?;
        self.click_at(x, y, 2).await
    }

    /// Click at a point in CSS pixels; `count` 2 is a double click.
    pub async fn click_at(&mut self, x: f64, y: f64, count: u32) -> BrowserResult<()> {
        self.move_mouse(x, y).await?;
        for click in 1..=count {
            for kind in ["mousePressed", "mouseReleased"] {
                self.command(
                    "Input.dispatchMouseEvent",
                    json!({ "type": kind, "x": x, "y": y, "button": "left", "buttons": 1, "clickCount": click }),
                    true,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Move the mouse to a point. The page sees `movementX`/`movementY` from
    /// where it last was, as with a real mouse.
    pub async fn move_mouse(&mut self, x: f64, y: f64) -> BrowserResult<()> {
        self.pointer = (x, y);
        self.command(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseMoved", "x": x, "y": y }),
            true,
        )
        .await
        .map(|_| ())
    }

    /// Press and release a key: `"Escape"`, `"Enter"`, `"Tab"`, an arrow
    /// (`"ArrowUp"`), or one character (`"e"`, `"N"`, `"7"`).
    pub async fn press(&mut self, key: &str) -> BrowserResult<()> {
        self.key_down(key).await?;
        self.key_up(key).await
    }

    /// Hold a key down — walking a character, holding Shift — until
    /// [`Browser::key_up`].
    pub async fn key_down(&mut self, key: &str) -> BrowserResult<()> {
        let k = Key::of(key)?;
        let mut params = json!({
            "type": if k.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": k.key, "code": k.code, "windowsVirtualKeyCode": k.virtual_key,
        });
        if let Some(text) = &k.text {
            params["text"] = json!(text);
        }
        self.command("Input.dispatchKeyEvent", params, true)
            .await
            .map(|_| ())
    }

    pub async fn key_up(&mut self, key: &str) -> BrowserResult<()> {
        let k = Key::of(key)?;
        self.command(
            "Input.dispatchKeyEvent",
            json!({ "type": "keyUp", "key": k.key, "code": k.code, "windowsVirtualKeyCode": k.virtual_key }),
            true,
        )
        .await
        .map(|_| ())
    }

    /// Type text into whatever has focus, as an input method would.
    pub async fn type_text(&mut self, text: &str) -> BrowserResult<()> {
        self.command("Input.insertText", json!({ "text": text }), true)
            .await
            .map(|_| ())
    }

    /// Everything the console said, and every uncaught error, so far.
    pub fn console(&self) -> &[ConsoleEntry] {
        &self.console
    }

    /// Console errors and uncaught exceptions so far. Collected as replies
    /// arrive, so read it after the step that might have produced one.
    pub fn errors(&self) -> Vec<ConsoleEntry> {
        self.console
            .iter()
            .filter(|entry| entry.level == "error" || entry.level == "exception")
            .cloned()
            .collect()
    }

    async fn center_of(&mut self, selector: &str) -> BrowserResult<(f64, f64)> {
        let selector_js = serde_json::to_string(selector).unwrap_or_default();
        let point: Option<(f64, f64)> = self
            .eval(&format!(
                "(() => {{ const el = document.querySelector({selector_js}); if (!el) return null; \
                 el.scrollIntoView({{ block: 'center', inline: 'center' }}); \
                 const r = el.getBoundingClientRect(); \
                 if (r.width === 0 && r.height === 0) return null; \
                 return [r.left + r.width / 2, r.top + r.height / 2]; }})()"
            ))
            .await?;
        match point {
            Some(point) => Ok(point),
            None => fail(format!(
                "no visible element matches `{selector}`{}",
                self.errors_suffix()
            )),
        }
    }

    fn errors_suffix(&self) -> String {
        let errors = self.errors();
        if errors.is_empty() {
            String::new()
        } else {
            let list: Vec<String> = errors
                .iter()
                .map(|e| format!("  [{}] {}", e.level, e.text))
                .collect();
            format!("; the page reported:\n{}", list.join("\n"))
        }
    }

    /// Send one command and wait for its reply, keeping the events that
    /// arrive meanwhile.
    async fn command(
        &mut self,
        method: &str,
        params: Value,
        in_page: bool,
    ) -> BrowserResult<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if in_page {
            message["sessionId"] = json!(self.session);
        }
        self.socket
            .send(Message::text(message.to_string()))
            .await
            .map_err(|e| BrowserError(format!("the browser went away: {e}")))?;

        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            let next = tokio::time::timeout_at(deadline, self.socket.next())
                .await
                .map_err(|_| {
                    BrowserError(format!("`{method}` got no answer within {TIMEOUT:?}"))
                })?;
            let frame = match next {
                Some(Ok(frame)) => frame,
                Some(Err(e)) => return fail(format!("the browser went away: {e}")),
                None => return fail("the browser closed the connection"),
            };
            let Message::Text(text) = frame else { continue };
            let Ok(reply) = serde_json::from_str::<Value>(text.as_str()) else {
                continue;
            };
            if reply["id"].as_u64() == Some(id) {
                if let Some(error) = reply.get("error") {
                    return fail(format!(
                        "`{method}` failed: {}",
                        error["message"].as_str().unwrap_or("unknown error")
                    ));
                }
                return Ok(reply["result"].clone());
            }
            self.record_event(&reply);
        }
    }

    fn record_event(&mut self, event: &Value) {
        match event["method"].as_str() {
            Some("Runtime.consoleAPICalled") => {
                let params = &event["params"];
                let text: Vec<String> = params["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|arg| {
                        arg.get("value")
                            .map(|v| match v {
                                Value::String(s) => s.clone(),
                                other => other.to_string(),
                            })
                            .or_else(|| arg["description"].as_str().map(str::to_string))
                            .unwrap_or_default()
                    })
                    .collect();
                self.console.push(ConsoleEntry {
                    level: params["type"].as_str().unwrap_or("log").to_string(),
                    text: text.join(" "),
                });
            }
            Some("Runtime.exceptionThrown") => {
                self.console.push(ConsoleEntry {
                    level: "exception".into(),
                    text: exception_text(&event["params"]["exceptionDetails"]),
                });
            }
            // A blocked script, `eval`, handler or connection: the page's
            // security policy refused it, which the page cannot catch and
            // which is an error a test must see.
            Some("Log.entryAdded") => {
                let entry = &event["params"]["entry"];
                if entry["source"] == "security" && entry["level"] == "error" {
                    self.console.push(ConsoleEntry {
                        level: "error".into(),
                        text: entry["text"].as_str().unwrap_or_default().to_string(),
                    });
                }
            }
            _ => {}
        }
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // The browser may hold its profile a moment after exiting.
        for _ in 0..10 {
            if std::fs::remove_dir_all(&self.profile).is_ok() || !self.profile.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// The readable part of a protocol exception: the error's own description
/// (message and stack) when there is one, else the summary line.
fn exception_text(details: &Value) -> String {
    details["exception"]["description"]
        .as_str()
        .or_else(|| details["text"].as_str())
        .unwrap_or("an error")
        .to_string()
}

fn short(expression: &str) -> String {
    let line = expression.lines().next().unwrap_or_default();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(80).collect::<String>())
    } else {
        line.to_string()
    }
}

/// What the input domain needs to know about a key.
struct Key {
    key: String,
    code: String,
    virtual_key: u32,
    text: Option<String>,
}

impl Key {
    fn of(name: &str) -> BrowserResult<Key> {
        let named = |key: &str, code: &str, virtual_key: u32, text: Option<&str>| Key {
            key: key.into(),
            code: code.into(),
            virtual_key,
            text: text.map(str::to_string),
        };
        Ok(match name {
            "Escape" => named("Escape", "Escape", 27, None),
            "Enter" => named("Enter", "Enter", 13, Some("\r")),
            "Tab" => named("Tab", "Tab", 9, None),
            "Backspace" => named("Backspace", "Backspace", 8, None),
            " " | "Space" => named(" ", "Space", 32, Some(" ")),
            "Shift" => named("Shift", "ShiftLeft", 16, None),
            "ArrowUp" => named("ArrowUp", "ArrowUp", 38, None),
            "ArrowDown" => named("ArrowDown", "ArrowDown", 40, None),
            "ArrowLeft" => named("ArrowLeft", "ArrowLeft", 37, None),
            "ArrowRight" => named("ArrowRight", "ArrowRight", 39, None),
            other => {
                let mut chars = other.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    return fail(format!(
                        "unknown key `{other}`: use one character or a key name such as \
                         Escape, Enter, Tab, Space, Shift, ArrowUp"
                    ));
                };
                let upper = c.to_ascii_uppercase();
                let code = if c.is_ascii_alphabetic() {
                    format!("Key{upper}")
                } else if c.is_ascii_digit() {
                    format!("Digit{c}")
                } else {
                    String::new()
                };
                Key {
                    key: c.to_string(),
                    code,
                    virtual_key: upper as u32,
                    text: Some(c.to_string()),
                }
            }
        })
    }
}
