# RPC, Streaming, and Uploads

An rpc answers once, and an `RpcStream` answers in pieces. When both sides
need to speak for as long as the page is open, that is a WebSocket — see
`websockets.md` for `#[socket]` and `pp.socket`, which are optional and live
behind rahti's `ws` cargo feature.

## RPC Placement and Dispatch

The browser posts to the page URL it is currently on with an RPC function
header. The generated router dispatches page-owned names first, then falls
through to the global component RPC registry.

- Page-owned RPC: place `#[rpc]` in the calling `page.rs`.
- Application component RPC: place it in `src/components/<component>.rs`.
- Do not place `#[rpc]` in layouts, errors, `not-found.rs`, or `route.rs`.
  Expose an ordinary HTTP method from `route.rs` instead.

Page RPCs are reachable only on their page URLs, so two different pages may
reuse a page RPC name. Component RPC names are application-wide because a
component can render on any page. All linked component RPC names must be
unique, and cannot collide with any page RPC name.

## Rust Signature

```rust
#[rpc]
pub async fn greet(name: String) -> Greeting {
    // ...
}
```

Rules:

- `#[rpc]` takes `auth`, or nothing. `#[rpc(auth)]` answers `401` unless the
  browser carries a session — see `authentication.md`. It does not authorize
  an action or resource; require a named ability inside the handler as
  described in `authorization.md`. The attribute has no other argument, and
  in particular no role.
- Functions may be sync or async, but cannot be methods or generic functions.
- Parameters need plain names and owned deserializable types.
- The payload key must match the Rust parameter name.
- Return a serializable value or `rahti::Result<T>`.
- `Option<T>` accepts an absent/null argument.
- The original function remains callable from Rust.

From PulsePoint:

```javascript
const result = await pp.rpc("greet", { name });
```

Successful values resolve from JSON. `Err(Error)` becomes an HTTP status plus
JSON error message and causes the Promise to reject. Expected messages remain
public; unexpected 5xx diagnostics are generic in production and stay in
server telemetry. The optional `requestId` correlates the response with that
telemetry. A
`ValidationErrors`-backed error uses status 422 and also returns its named
`errors` map; see `validation.md` and `error-handling.md`.

## Database Errors

`rahti::Error` implements `From<E>` for every `E: std::error::Error`, so
`sea_orm::DbErr` works with `?` inside `rahti::Result<T>` with no conversion
written anywhere:

```rust
#[rpc]
pub async fn list() -> Result<Vec<todo::Model>> {
    Ok(todo::Entity::find().all(db()).await?)
}
```

That produces a 500. Return a real status where one exists — a missing row is
a 404 — and validate before querying rather than relying on the database to
refuse. See `validation.md` and `database.md`.

## Client Options

```javascript
pp.rpc(name, data?, optionsOrAbort?)
```

Options:

- `abortPrevious`
- `url`
- `csrfUrl`
- `credentials`
- `onStream`
- `onStreamError`
- `onStreamComplete`
- `onUploadProgress`
- `onUploadComplete`

Passing `true` as the third argument is shorthand for
`{ abortPrevious: true }`.

By default the URL is the current route. A CSRF cookie is established by a
page GET and returned as `X-CSRF-Token`. The client includes Rahti/PulsePoint
wire headers and honors same-origin server redirects. Do not replace
`pp.rpc(...)` with raw `fetch` unless implementing a distinct external API.

## Server Streaming

Return `RpcStream` to produce `text/event-stream`:

```rust
#[rpc]
pub async fn count(to: u32) -> RpcStream {
    RpcStream::from_iter(1..=to)
}
```

Other construction forms:

- `RpcStream::new(stream)` for an asynchronous stream of serializable values;
- `RpcStream::from_iter(values)` for an iterator;
- `RpcStream::channel(capacity)` for `(RpcSender, RpcStream)`;
- `RpcSender::send(value).await` sends one serialized event.

Client:

```javascript
await pp.rpc("count", { to: 5 }, {
    onStream: value => append(value),
    onStreamError: error => showError(error),
    onStreamComplete: () => finish(),
});
```

Each event is one JSON value. The request remains active until the stream
closes and can be cancelled by `abortPrevious` or component disposal.

## Buffered/Spilled Files: `RpcFile`

Including a browser `File` or non-empty `FileList` makes `pp.rpc` use
`multipart/form-data`. Use:

- `RpcFile` for exactly one file;
- `Option<RpcFile>` for zero or one;
- `Vec<RpcFile>` for several fields under the same payload key.

Useful APIs:

- metadata: `name`, `safe_name`, `content_type`, `len`, `is_empty`,
  `is_spilled`;
- content: `bytes`, `into_bytes`, `text`, `reader`, `into_stream`;
- persistence: consuming `save`, non-consuming `copy_to`.

The reported filename is user input. Use `safe_name()` before constructing a
path, and still choose the destination directory yourself.

Small files remain in memory; larger files spill to a temporary file while
arriving. Clones keep a spilled file alive, and the temporary file is removed
when no owner remains.

## Direct Streaming Uploads: `RpcUpload`

Use `RpcUpload` when the server must process bytes as they arrive without
spooling the complete upload:

```rust
#[rpc]
pub async fn store(note: String, upload: RpcUpload) -> Result<Stored> {
    let bytes = upload.save(path).await?;
    // ...
}
```

Rules:

- at most one `RpcUpload` parameter;
- it must be the final parameter;
- the browser's file value must be last on the multipart wire. The current
  PulsePoint client deliberately appends all non-file fields before files;
- a missing or non-multipart file is an RPC error.

APIs include `name`, `safe_name`, `content_type`, `read_so_far`, `chunk`,
`save`, `copy_to`, and `collect`. `collect` converts the remaining stream to an
`RpcFile` with a chosen spill threshold.

## Limits and Environment

Defaults:

- total request: 128 MiB;
- per-file: unlimited except by total request (`0`);
- spill threshold: 256 KiB;
- spill directory: operating-system temporary directory.

Overrides:

- `RAHTI_MAX_UPLOAD`
- `RAHTI_MAX_FILE`
- `RAHTI_SPILL_THRESHOLD`
- `RAHTI_SPILL_DIR`

Values are byte counts. The effective per-file limit never exceeds the total
request limit. Keep application-level content validation separate from
transport limits.
