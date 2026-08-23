# WebSockets

An rpc is a question with one answer, and a stream is an answer that arrives
in pieces. A socket is the third shape: both sides may speak, at any time,
for as long as the page is open.

WebSockets are optional. The `#[socket]` attribute and the `rahti::ws` module
exist only when the application's `rahti` dependency enables the `ws` cargo
feature:

```toml
rahti = { version = "0.1", features = ["ws"] }
```

`cargo rahti new --ws` (or answering yes to "Use WebSockets?") writes that
line and records `"ws": true` in `rahti.config.json`. A project that did not
ask compiles none of the socket wire — the feature is what keeps
tokio-tungstenite out of an app that never opens a connection.

`cargo rahti upgrade --ws` adds WebSockets to a project that started without
them: it records the key and folds `features = ["ws"]` into the `rahti` line
already in `Cargo.toml`, keeping the version or path that is there and any
features beside it. The config key is what it reads to know the feature is
wanted, so a project whose config says `"ws": true` and whose manifest does
not name the feature is repaired by running `upgrade` again.

## The Rust Signature

```rust
use rahti::ws::Socket;

#[socket]
pub async fn chat(room: String, mut socket: Socket) -> rahti::Result<()> {
    while let Some(message) = socket.recv::<String>().await? {
        if !socket.send(&message).await {
            break; // The browser is gone.
        }
    }
    Ok(())
}
```

Rules:

- `#[socket]` takes `auth`, or nothing. `#[socket(auth)]` refuses the
  handshake with a 401 unless the browser carries a session. There is no
  other argument.
- The function must be `async` — the conversation only exists behind
  `.await`.
- The last parameter is the `Socket`. Everything before it arrives in the
  connection's first frame, read by the parameter's own name under the rpc
  payload rules: owned deserializable types, `Option<T>` for an optional
  argument.
- Return nothing, or `rahti::Result<()>`. A socket answers through its
  frames, so a returned value would have nowhere to go — the macro refuses
  one. An `Err` reaches the browser as an error frame, then the close.
- The original function remains callable from Rust, but only the wire builds
  a `Socket` — there is no way to construct one by hand.

Placement is the rpc rule: beside the page whose script opens it in that
`page.rs`, or in the component file that owns the conversation. Not in
layouts, errors, or `route.rs`.

Unlike page rpcs, **socket names are application-wide** wherever they are
written. Every connection arrives on one endpoint and is dispatched by name
alone, so a page's socket and a component's share one namespace. Two files
claiming one name is a build error, and the whole linked set is checked again
at startup, the way component rpc names are.

## The Socket

- `socket.recv::<T>().await` — the next frame as `T`. `Ok(None)` is the
  connection closing, which is how every conversation eventually ends; the
  natural loop is `while let Some(v) = socket.recv().await?`. `Err` is a
  frame that does not fit `T`, and `?` in a fallible handler sends it back
  as the error frame.
- `socket.send(&value).await` — one frame out. Returns `false` once the
  browser is gone; that is the signal to stop producing, not an error, the
  same contract as `RpcSender::send`.
- `socket.text().await` — the next frame as it arrived, for a handler that
  reads it itself.
- `socket.sender()` — a cloneable `SocketSender` another task may hold. A
  chat room is a list of these: keep one per connection in shared state, and
  a send that returns `false` is a connection to forget.
- `socket.close().await` — say goodbye first. Dropping the socket closes the
  connection too, so a handler that returns is a conversation that ends.
- `rahti::auth::session()` answers inside a handler the way it does in an
  rpc: the session is read at the handshake and scoped around the
  conversation. `sign_in`/`sign_out` do not work here — there is no response
  left to write a cookie onto.

Connections close themselves when the process is told to stop, so an open
socket never holds Ctrl+C.

## The Client

```javascript
const sock = pp.socket("chat", { room: "lobby" }, {
    onOpen: () => …,
    onMessage: value => …,
    onError: error => …,
    onClose: ({ code, reason, wasClean }) => …,
});

sock.send({ text: "hello" });
sock.close();
```

`pp.socket` returns its handle synchronously. Frames sent before the
connection opens are buffered and flushed after the argument frame, so
opening and sending on consecutive lines works. `sock.send` returns `false`
once the connection is closed.

## The Wire

- The connection is `GET /__pulsepoint/ws?name=<function>` upgraded to a
  WebSocket. Only the name travels in the URL; the arguments are the first
  text frame — one JSON object, exactly the payload `pp.rpc` would have
  posted — because a URL is logged by every proxy on the way and an argument
  is data.
- Every frame after that is one JSON value, in either direction.
- A frame of the shape `{"error": "…"}` — that key alone — is reserved: it is
  how the server reports failure inside an open connection, where no HTTP
  status line exists any more. The client routes it to `onError`, not
  `onMessage`.
- Everything refusable is refused before the upgrade, as HTTP: an unnamed or
  unknown socket, a cross-origin page, a missing session on
  `#[socket(auth)]`.

## Who May Connect

The same-origin policy does not cover WebSockets: another site's page can
open one here and the browser attaches the cookies. What that page cannot do
is forge its `Origin` header, so the handshake is refused when `Origin`
disagrees with `Host` — the socket counterpart of the rpc CSRF check.
Schemes are not compared (a TLS-terminating proxy rewrites them); a request
with no `Origin` at all is a non-browser client carrying nothing to forge,
and passes.

`#[socket(auth)]` is the call-side check, exactly as `#[rpc(auth)]` is:
listing a page in `private_routes` decides who may load the page, and the
attribute decides who may open the socket. Mark every socket that touches
something private.

The generated manifest records `auth: true` on that socket. This lets local
tooling describe the handshake requirement; it does not authenticate a tool
or replace the runtime session check.

## Broadcast: One Message, Every Browser

A handler's `Socket` speaks to the one browser that connected, so an echo
answers privately and two tabs never hear each other. That is the default on
purpose — a socket is a conversation. A chat room is the other shape, and it
is built out of `socket.sender()`: keep one cloned sender per connection in
shared state, and a broadcast is a walk down the list.

```rust
fn room() -> &'static Mutex<Vec<SocketSender>> {
    static ROOM: OnceLock<Mutex<Vec<SocketSender>>> = OnceLock::new();
    ROOM.get_or_init(|| Mutex::new(Vec::new()))
}

#[socket]
pub async fn chat_room(name: String, mut socket: Socket) -> Result<()> {
    room().lock().unwrap().push(socket.sender());
    while let Some(said) = socket.recv::<Said>().await? {
        broadcast(&name, said.text).await; // send on every sender in the room
    }
    Ok(())
}
```

Two rules keep it honest:

- Clone the list out from under the lock before sending — a `Mutex` guard
  must not be held across an `.await`.
- A send that returns `false` is a browser that left; prune on that answer
  (`senders.retain(SocketSender::is_open)`) rather than keeping a ledger.

The room state is the application's, like the database: Rahti provides the
senders and says nothing about how they are grouped. Rooms-by-name is a
`HashMap<String, Vec<SocketSender>>` keyed by an argument from the first
frame; the shape above is the one-room case.
