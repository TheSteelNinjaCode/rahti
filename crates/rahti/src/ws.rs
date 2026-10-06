//! Holding a line open to the browser.
//!
//! An rpc is a question with one answer, and a stream is an answer that
//! arrives in pieces. A socket is the third shape: both sides may speak, at
//! any time, for as long as the page is open. The client half is
//! `pp.socket(…)` in the bundled runtime; this module is the server half,
//! and `#[socket]` is what puts a function on it:
//!
//! ```ignore
//! use rahti::ws::Socket;
//!
//! #[socket]
//! pub async fn echo(label: String, mut socket: Socket) {
//!     while let Ok(Some(text)) = socket.recv::<String>().await {
//!         if !socket.send(&format!("{label}: {text}")).await {
//!             break; // The browser is gone.
//!         }
//!     }
//! }
//! ```
//!
//! From the browser:
//!
//! ```js
//! const sock = pp.socket("echo", { label: "you" }, {
//!     onMessage: value => append(value),
//! });
//! sock.send("hello");
//! ```
//!
//! ## The wire
//!
//! Every socket connects to one endpoint, [`WS_PATH`], naming its function in
//! the `name` query parameter. The arguments do not travel in the URL — a URL
//! is logged by every proxy on the way — but as the first text frame, one
//! JSON object, exactly the payload `pp.rpc` would have posted. Every frame
//! after that is one JSON value, in either direction.
//! The browser sends `{"__pp":"ping"}` to check a quiet connection. The
//! socket pump answers `{"__pp":"pong"}` before application code sees it;
//! single-key `__pp` control frames are reserved by the wire.
//!
//! One endpoint rather than one per page because an upgrade is a GET, and the
//! page already owns GET on its own URL. The names are therefore
//! application-wide, like component rpc names, and checked for collisions the
//! same way: at build time from the scan, and again at startup over
//! everything the linker actually assembled.
//!
//! ## Who may connect
//!
//! A browser sends `Origin` on every WebSocket handshake and cookies with it,
//! and nothing in the same-origin policy stops another site from opening one.
//! So the handshake is refused when `Origin` disagrees with `Host` — the
//! WebSocket counterpart of the rpc CSRF check. A request with no `Origin` at
//! all is a non-browser client and passes; it holds no cookies worth
//! forging.
//!
//! `#[socket(auth)]` refuses the handshake with a 401 unless the request
//! carries a session, before the connection is upgraded. Inside the handler,
//! `rahti::auth::session()` answers the way it does in an rpc.
//!
//! ## Failure
//!
//! There is no status line inside an open connection, so failure is a frame:
//! `{"error": "…"}`, the same shape an rpc failure travels as, followed by a
//! close. The client runtime hands it to `onError`. A handler that returns
//! `rahti::Result<()>` gets this for free on `Err`; a handler that returns
//! nothing simply closes when it returns.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use axum::Router;
use axum::extract::Query;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use super::error::Error;
use super::rpc::json_error;

#[cfg(test)]
#[path = "tests/ws.rs"]
mod tests;

/// Where every socket connects. The function is named in the `name` query
/// parameter; everything else waits for the first frame.
// The path carries the PulsePoint runtime's name, not this framework's: the
// client half is the same `pp.socket(…)` whichever backend serves it, so
// every backend answers on the same endpoint.
pub const WS_PATH: &str = "/__pulsepoint/ws";

/// PulsePoint control frames have exactly one `__pp` key. Other objects,
/// including application values that happen to contain `__pp`, pass through.
fn control_frame(text: &str) -> Option<bool> {
    let Value::Object(map) = serde_json::from_str::<Value>(text).ok()? else {
        return None;
    };
    if map.len() != 1 {
        return None;
    }
    Some(map.get("__pp")?.as_str() == Some("ping"))
}

/// How long the server waits for the first frame — the arguments — before
/// giving up on a connection that opened and said nothing.
const ARGS_TIMEOUT: Duration = Duration::from_secs(10);

/// How far either side may run ahead before the pump applies backpressure.
const CHANNEL_CAPACITY: usize = 32;

/// The shim `#[socket]` writes, type-erased so entries of different
/// signatures sit in one collection — the socket counterpart of `RpcHandler`.
pub type SocketHandler = fn(Map<String, Value>, Socket) -> Pin<Box<dyn Future<Output = ()> + Send>>;

/// One `#[socket]`, submitted by the macro at the point it is written.
///
/// Found at link time, like an `RpcEntry`: the linker is the one place every
/// socket in the binary is present at once, whichever file each was written
/// in.
pub struct SocketEntry {
    /// The name the browser connects with, which is the function's own.
    pub name: &'static str,
    /// Where it was written, for the message when two of them collide.
    pub source: &'static str,
    /// `#[socket(auth)]`: refuse the handshake without a session.
    pub auth: bool,
    pub handler: SocketHandler,
}

inventory::collect!(SocketEntry);

/// Every registered socket, in whatever order the linker produced.
fn entries() -> impl Iterator<Item = &'static SocketEntry> {
    inventory::iter::<SocketEntry>.into_iter()
}

/// Refuse a binary in which two sockets answer to one name.
///
/// The same rule, and the same reason, as `check_rpc_names`: the client sends
/// a name and nothing else, so the names have to differ across everything
/// linked into the binary.
pub fn check_socket_names() {
    let all: Vec<&SocketEntry> = entries().collect();
    if let Some((first, second)) = duplicate(&all) {
        panic!(
            "rahti: two sockets are named `{}`.\n  {}\n  {}\n  \
             The client connects with a name and nothing else, so the names \
             have to differ across everything linked into this binary.",
            first.name, first.source, second.source
        );
    }
}

/// The first pair sharing a name, if there is one. Separated from the panic
/// so the rule can be tested without a binary that refuses to start.
fn duplicate<'a>(entries: &[&'a SocketEntry]) -> Option<(&'a SocketEntry, &'a SocketEntry)> {
    let mut seen: Vec<&'a SocketEntry> = Vec::new();
    for entry in entries {
        if let Some(other) = seen.iter().find(|s| s.name == entry.name) {
            return Some((other, entry));
        }
        seen.push(entry);
    }
    None
}

/// The socket endpoint, as one merged `Router`.
///
/// Called by the generated router when the app declares a `#[socket]`
/// anywhere. Carries its own auth layer for the same two reasons the rpc
/// router does: `#[socket(auth)]` is a check against the session, and a
/// handler may want to know who connected.
pub fn routes() -> Router {
    check_socket_names();
    Router::new()
        .route(WS_PATH, get(upgrade))
        .layer(axum::middleware::from_fn(super::auth::guard))
}

/// The handshake: who is calling, whether they may, and then the upgrade.
///
/// Everything that can be refused is refused here, as an ordinary HTTP
/// response, because this is the last moment one can be sent — past the
/// upgrade there is no status line left.
async fn upgrade(
    ws: WebSocketUpgrade,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_origin(&headers) {
        return response;
    }

    let name = match params.get("name").map(String::as_str) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => {
            return json_error(
                StatusCode::BAD_REQUEST,
                "rahti: this connection named no socket.\n  \
                 Open it as `pp.socket(\"name\", { … })` — the client runtime \
                 sends the name in the `name` query parameter.",
            );
        }
    };

    let Some(entry) = entries().find(|entry| entry.name == name) else {
        return json_error(
            StatusCode::NOT_FOUND,
            format!(
                "rahti: no socket named `{name}`.\n  \
                 Mark the function `#[socket]` — the name the client connects \
                 with is the function's own."
            ),
        );
    };

    // Before the upgrade, so the refusal is a 401 the client can read rather
    // than a connection that opens and immediately closes.
    let session = super::auth::session();
    if entry.auth && session.is_none() {
        return json_error(
            StatusCode::UNAUTHORIZED,
            format!(
                "rahti: the socket `{name}` needs a signed-in session.\n  \
                 It is `#[socket(auth)]`, so it answers only while the browser \
                 is carrying one — sign in, or drop the `auth` if the socket \
                 is meant to be open."
            ),
        );
    }

    let handler = entry.handler;
    ws.on_upgrade(move |socket| {
        // The upgrade callback runs outside the request that carried the
        // cookie, so the session read above is scoped back in — which is what
        // lets a handler call `rahti::auth::session()` and get the visitor
        // who opened the connection.
        super::auth::scope_session(session, run(name, handler, socket))
    })
}

/// One connection, from first frame to close.
async fn run(name: String, handler: SocketHandler, mut ws: WebSocket) {
    let args = match first_frame(&mut ws).await {
        Ok(Some(args)) => args,
        // Closed before saying anything — a refresh mid-handshake. Not an
        // error, and nobody left to tell.
        Ok(None) => return,
        Err(message) => {
            super::dev::record_server("error", "socket", &message);
            let _ = ws.send(Message::Text(error_frame(&message).into())).await;
            let _ = ws.send(Message::Close(None)).await;
            return;
        }
    };

    handler(args, Socket::drive(name, ws)).await;
}

/// The first text frame: the arguments, as one JSON object.
///
/// `Ok(None)` is a connection that closed first. `Err` is a frame that was
/// sent and could not be read — worth an error frame back, because someone
/// wrote a client that almost speaks the wire.
async fn first_frame(ws: &mut WebSocket) -> Result<Option<Map<String, Value>>, String> {
    let frame = tokio::time::timeout(ARGS_TIMEOUT, async {
        loop {
            match ws.recv().await {
                Some(Ok(Message::Text(text))) => return Some(text),
                Some(Ok(Message::Close(_))) | None => return None,
                // Ping and pong are answered below this API; binary has no
                // place in a JSON wire and is ignored the way stray frames
                // are everywhere else in it.
                Some(Ok(_)) => continue,
                Some(Err(_)) => return None,
            }
        }
    })
    .await
    .map_err(|_| {
        "rahti: a socket opened and sent no arguments.\n  \
         The first frame is the payload — one JSON object, `{}` when the \
         function takes nothing. `pp.socket` sends it on open."
            .to_string()
    })?;

    let Some(text) = frame else {
        return Ok(None);
    };

    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(Some(map)),
        Ok(_) | Err(_) => Err(
            "rahti: the first frame of a socket is not a JSON object.\n  \
             Arguments are named, so they arrive as `{ \"room\": … }` — \
             `pp.socket(\"name\", { room })` is what sends them."
                .to_string(),
        ),
    }
}

/// Read one argument out of the first frame, by the parameter's own name.
///
/// The socket half of `RpcCall::arg`, minus the multipart fallback: the wire
/// is JSON end to end, so a value either fits or the message says why not.
/// Called by the shim `#[socket]` writes; `func` is only here for the error.
pub fn arg<T: DeserializeOwned>(
    args: &Map<String, Value>,
    func: &str,
    name: &str,
) -> Result<T, String> {
    let value = args.get(name).cloned().unwrap_or(Value::Null);
    serde_json::from_value::<T>(value).map_err(|e| {
        format!(
            "rahti: `{func}` could not read its `{name}` argument — {e}.\n  \
             The browser opens `pp.socket(\"{func}\", {{ {name}: … }})`, and \
             the value has to match the parameter's type."
        )
    })
}

/// Refuse a connection whose arguments could not be read: the error frame,
/// then the close. Takes the socket because the conversation is over.
pub async fn refuse(socket: Socket, message: String) {
    super::dev::record_server("error", "socket", &message);
    socket.sender.error(&message).await;
}

/// A handler that returned `Err`: the same error frame, sent on the sender
/// the shim kept, because the handler consumed the socket itself.
///
/// Logged the way an rpc failure is: an expected 4xx refusal ("that worker is
/// not running") as a warning, a 5xx as an error. The frame carries the
/// client-safe message, so an unexpected production failure is redacted here
/// exactly as it is in an rpc response.
pub async fn failed(sender: &SocketSender, error: Error) {
    error.record("socket");
    sender
        .error(error.client_message(super::dev::enabled()))
        .await;
}

/// `{"error": "…"}` — the frame shape the client runtime hands to `onError`.
/// Reserved on this wire the way the key is reserved in an rpc failure.
fn error_frame(message: &str) -> String {
    serde_json::json!({ "error": message }).to_string()
}

/// Refuse a handshake whose `Origin` disagrees with `Host`.
///
/// The same-origin policy does not cover WebSockets: another site's page can
/// open one here and the browser will attach the cookies. What that page
/// cannot do is forge its `Origin` header. Comparing authorities — not
/// schemes, which a TLS-terminating proxy rewrites — is the whole check.
#[allow(clippy::result_large_err)]
fn check_origin(headers: &HeaderMap) -> Result<(), Response> {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        // No `Origin` is a non-browser client: it carries no ambient cookies
        // for anyone to abuse, so there is nothing here to defend.
        return Ok(());
    };

    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let origin_host = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);

    if !host.is_empty() && origin_host.eq_ignore_ascii_case(host) {
        return Ok(());
    }

    Err(json_error(
        StatusCode::FORBIDDEN,
        format!(
            "rahti: this socket handshake came from `{origin}`, and this \
             server is `{host}`.\n  \
             Sockets are same-origin: the page that opens one has to be \
             served by the host it connects to."
        ),
    ))
}

// ------------------------------------------------------------------ socket

/// What the pump is told to do, from however many senders exist.
enum Outgoing {
    Frame(String),
    Close,
}

/// One open connection, as the handler holds it.
///
/// The last parameter of every `#[socket]` function. Receiving needs `&mut`;
/// sending does not, and [`Socket::sender`] hands out a clone that another
/// task may hold — which is how a chat room reaches the people in it.
///
/// Dropping it closes the connection, so a handler that returns is a
/// conversation that ends.
pub struct Socket {
    /// The function this connection is speaking to, for error messages.
    name: String,
    sender: SocketSender,
    incoming: mpsc::Receiver<String>,
}

impl Socket {
    /// Put the raw connection on a pump and hand back the two ends.
    ///
    /// One task owns the `WebSocket` and both directions cross it, which is
    /// what lets [`SocketSender`] be a clonable mpsc handle rather than half
    /// a split — and gives shutdown one place to close every connection from.
    fn drive(name: String, mut ws: WebSocket) -> Socket {
        let (out_tx, mut out_rx) = mpsc::channel::<Outgoing>(CHANNEL_CAPACITY);
        let (in_tx, in_rx) = mpsc::channel::<String>(CHANNEL_CAPACITY);

        tokio::spawn(async move {
            // Subscribed before the loop: a receiver only hears what is sent
            // after it subscribes.
            let shutdown = super::shutdown::watch();
            tokio::pin! {
                let stopped = shutdown.fired();
            }

            loop {
                tokio::select! {
                    out = out_rx.recv() => match out {
                        Some(Outgoing::Frame(text)) => {
                            if ws.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        // `None` is every sender dropped — the handler
                        // returned. Either way the close is said out loud.
                        Some(Outgoing::Close) | None => {
                            let _ = ws.send(Message::Close(None)).await;
                            break;
                        }
                    },
                    msg = ws.recv() => match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Some(is_ping) = control_frame(&text) {
                                if is_ping
                                    && ws.send(Message::Text(r#"{"__pp":"pong"}"#.into())).await.is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                            // A handler that stopped reading is not an error;
                            // the frame simply has nowhere to go.
                            let _ = in_tx.send(text.to_string()).await;
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => {}
                        Some(Err(_)) => break,
                    },
                    // `axum::serve` waits for open connections on shutdown,
                    // and an open socket never finishes on its own — the same
                    // reason every long-lived response in this crate watches.
                    _ = &mut stopped => {
                        let _ = ws.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
        });

        Socket {
            name,
            sender: SocketSender { tx: out_tx },
            incoming: in_rx,
        }
    }

    /// Send one value. `false` means the browser is gone — navigated away, or
    /// closed the tab — and is the signal to stop, not an error to report.
    /// The same contract as `RpcSender::send`.
    pub async fn send<T: Serialize>(&self, value: &T) -> bool {
        self.sender.send(value).await
    }

    /// The next value the browser sent.
    ///
    /// `Ok(None)` is the connection closing, which is how every socket
    /// conversation eventually ends — the natural loop is
    /// `while let Some(v) = socket.recv().await?`. `Err` is a frame that
    /// arrived and does not fit `T`; returned rather than skipped because a
    /// frame the handler cannot read is a client bug worth surfacing, and
    /// `?` in a fallible handler sends it back as the error frame.
    pub async fn recv<T: DeserializeOwned>(&mut self) -> crate::Result<Option<T>> {
        let Some(text) = self.incoming.recv().await else {
            return Ok(None);
        };
        match serde_json::from_str::<T>(&text) {
            Ok(value) => Ok(Some(value)),
            Err(e) => Err(Error::new(
                StatusCode::BAD_REQUEST,
                format!(
                    "rahti: `{}` could not read a frame — {e}.\n  \
                     Each frame is one JSON value, and it has to match the \
                     type `recv` asks for.",
                    self.name
                ),
            )),
        }
    }

    /// The next frame as it arrived, for a handler that would rather read it
    /// itself. `None` is the connection closing.
    pub async fn text(&mut self) -> Option<String> {
        self.incoming.recv().await
    }

    /// A sending handle another task may hold.
    ///
    /// Cloneable, so a broadcast is a list of these: keep one per connection
    /// in shared state, and a send that returns `false` is a connection to
    /// forget.
    pub fn sender(&self) -> SocketSender {
        self.sender.clone()
    }

    /// Whether the connection is still open, without sending anything.
    pub fn is_open(&self) -> bool {
        self.sender.is_open()
    }

    /// Say goodbye first. Dropping the socket closes it too; this is for
    /// closing while the value is still in scope.
    pub async fn close(self) {
        let _ = self.sender.tx.send(Outgoing::Close).await;
    }
}

impl std::fmt::Debug for Socket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Socket").field("name", &self.name).finish()
    }
}

/// The sending half of a [`Socket`], detached from the conversation.
#[derive(Clone)]
pub struct SocketSender {
    tx: mpsc::Sender<Outgoing>,
}

impl SocketSender {
    /// Send one value. `false` means nobody is listening any more.
    pub async fn send<T: Serialize>(&self, value: &T) -> bool {
        let json = match serde_json::to_string(value) {
            Ok(json) => json,
            Err(e) => {
                super::dev::record_server(
                    "error",
                    "socket",
                    &format!("rahti: a socket frame cannot be written as JSON — {e}"),
                );
                return false;
            }
        };
        self.tx.send(Outgoing::Frame(json)).await.is_ok()
    }

    /// Whether the connection is still open, without sending anything.
    pub fn is_open(&self) -> bool {
        !self.tx.is_closed()
    }

    /// The error frame, then the close. What [`refuse`] and [`failed`] send.
    async fn error(&self, message: &str) {
        let _ = self.tx.send(Outgoing::Frame(error_frame(message))).await;
        let _ = self.tx.send(Outgoing::Close).await;
    }
}

impl std::fmt::Debug for SocketSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SocketSender")
    }
}
