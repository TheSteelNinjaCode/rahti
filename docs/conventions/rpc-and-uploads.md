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
JSON error message and rejects with a PulsePoint `RpcError`. Its `message` is
the server's `error` text; `status`, `errors`, `requestId`, and `body` expose
the HTTP status, validation field map, optional correlation id, and full parsed
JSON response. `errors` is `{}` and `requestId` is `null` when absent. Expected
messages remain public; unexpected 5xx diagnostics are generic in production
and stay in server telemetry. A `ValidationErrors`-backed error uses status
422 and named field messages; see `validation.md` and `error-handling.md`.

For 401 and 403, PulsePoint keeps its stable messages `Authentication
required` and `Permission denied` while preserving the response in `body`.
When `onStreamError` is supplied, PulsePoint calls it for a failed request or
stream instead of rejecting the outer Promise; handle the error there.

## Compile-Time Call Checking

`html!` checks every `pp.rpc` call in a template's client code — component
scripts, handlers and bindings — against the `#[rpc]` it names. A misspelled
name or payload key fails `cargo check` rather than answering 404 or 400 in
the browser.

The name resolves through the Rust module the `html!` is written in, so the
rpc has to be declared in that file: a page calls the `#[rpc]`s in its own
`page.rs`, and a component calls the ones in its own file (see "RPC Placement
and Dispatch"). Given

```rust
#[rpc]
pub async fn rename(id: i32, name: String, note: Option<String>) -> Result<user::Public> {
    // ...
}
```

each of these calls is a compile error:

```javascript
pp.rpc("renam", { id, name });     // cannot find value `__RAHTI_RPC_renam`;
                                   // rustc names `__RAHTI_RPC_rename` as similar
pp.rpc("rename", { id, nmae });    // `rename` has no parameter `nmae`
pp.rpc("rename", { id });          // this call to `rename` does not send `name`
pp.rpc("rename");                  // this call to `rename` sends no arguments
pp.rpc(which, { id, name });       // `pp.rpc` names its `#[rpc]` with a string literal
pp.rpc("rename", { id: 1, name: 12 }); // RAHTI_RPC_ARGUMENT_TYPE: name expects String
```

The name is always a string literal — `"rename"`, `'rename'` or
`` `rename` `` — and anything else is refused. A call that chooses between rpcs
is one literal call per branch:
`editing ? pp.rpc("update", { … }) : pp.rpc("create", { … })`.

What is checked depends on the payload:

- An object literal: every key must be a parameter, and every required
  parameter must be present. `Option<T>`, `Option<RpcFile>` and
  `Vec<RpcFile>` are optional; every other parameter is required.
- An object literal with a spread or a computed key (`{ ...draft, id }`): the
  written keys are checked, and missing keys are not.
- No payload: the rpc must have no required parameters.
- Anything else — `Object.fromEntries(new FormData(event.currentTarget))`,
  `new FormData(form)`, a variable, a call: only the name is checked.

A closed object literal also checks obvious value-shape mismatches against
standard Rust type spellings: strings/chars, booleans, numbers, vectors,
arrays, and their `Option` wrappers. For example, `name: 12` cannot satisfy
`name: String`. These are shape checks, not full JavaScript type inference:
numeric ranges, array element types, string contents, named application
types, aliases, and custom Serde representations are validated by the server.
String values are allowed through because form fields can carry JSON text:
`"42"` can deserialize as an integer, while a `String` receives the text itself.
Multipart calls do not get literal shape checks because their ordinary fields
are encoded as text. A spread or computed key also prevents value-shape checks.
The final duplicate property is the effective value. Objects with custom
`toJSON`, prototype setters, or unknown properties have unresolved shapes.

Only a call written `pp.rpc(…)` is checked. JavaScript variables, functions,
response-field accesses, and return-value usage are not type-checked.
An unchecked expression is not evidence that its value satisfies the contract.

In a binding or handler the error lands on the token: the name string, the
key, or the payload object. A `<script>` body is one Rust string literal,
which a compiler error cannot point inside, so a script's errors point at its
first character. A key error names the file line of the call; an unknown
name is identified by the name in rustc's message.

The check is a developer aid, not a security boundary. The server deserializes
every payload, and the handler validates business rules before side effects
(see `validation.md`).

## Agent RPC Workflow

1. Run `cargo check` after editing Rust RPC declarations. It regenerates
   `.rahti/manifest.json`; never edit that file.
2. Read `rpcContracts` in that manifest, or use the optional MCP server's
   `list_rpcs`. Match the RPC's `file`, `scope`, and `urls`, not its name alone:
   separate page URLs may use the same name. A component RPC has no fixed URL;
   it is called from the page rendering that component.
3. Read `parameters` (`name`, `rustType`, `required`, `transport`) and
   `response` (`rustType`, `fallible`, `transport`). `line` and `column` locate
   the declaration. `rustType` is the authored Rust spelling, not a resolved
   JSON schema: `schemaStatus: "unresolved"` means inspect the type and its
   Serde representation before using fields. `checks` names the checking limits.
4. Keep the caller and `#[rpc]` together and use the existing
   `pp.rpc("name", { ... })` shape. Prefer explicit object keys when the data
   is already available; submit forms with the documented `FormData` pattern.
5. Run `cargo check` and then the relevant router/browser tests. Compilation
   does not prove that a dynamic payload or response-field access is correct.

The manifest declares `rpcContractVersion: 1`. MCP requires this contract
inventory and reports `RAHTI_RPC_CONTRACT_MANIFEST` with a regeneration
instruction if it is missing or unsupported. It never reconstructs contracts
from route-name lists. Discovery remains read-only and never invokes an RPC.
`authRequired` describes the RPC session requirement, not route privacy or
resource authorization.

## Contract Diagnostics

Compile-time diagnostic codes are `RAHTI_RPC_UNKNOWN_ARGUMENT`,
`RAHTI_RPC_MISSING_ARGUMENT`, `RAHTI_RPC_ARGUMENT_TYPE`,
`RAHTI_RPC_NAME_LITERAL`, and `RAHTI_RPC_NAME_MISSING`. Unknown literal function
names use Rust's unresolved-signature diagnostic. Each diagnostic names the
problem and the supported form to write.

A missing or mistyped JSON argument that fails server deserialization returns
HTTP 400 with `error`, `code`, `rpc`, `argument`, `expectedRustType`,
`receivedType`, and `hint`. `code` is `RAHTI_RPC_MISSING_ARGUMENT` or
`RAHTI_RPC_ARGUMENT_TYPE`. `receivedType` is a shape such as `string` or
`missing`, never the input value. A successful custom deserializer remains
valid even if it accepts an absent argument as null. Business validation
remains HTTP 422 with named field messages. Streaming-upload ordering errors
and file errors retain their transport-specific explanations.

Read these fields through `failure.body`, for example:

```javascript
try {
    await pp.rpc("rename", draft);
} catch (failure) {
    if (failure.body.code === "RAHTI_RPC_ARGUMENT_TYPE") {
        console.warn(failure.body.argument, failure.body.expectedRustType);
    }
}
```

Contract deserialization failures are recorded under `rpc-contract` in
`.rahti/dev.log` during development. These diagnostics identify names and
types without echoing payload values.

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
