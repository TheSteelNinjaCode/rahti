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
- A payload the script builds elsewhere and that can be followed to its
  object literal (see "Payloads built in the script"): every required
  parameter must be present. The keys it brings are not checked one by one.
- An object literal with a spread that cannot be followed, or a computed key
  (`{ ...draft, id }`): the written keys are checked, and missing keys are
  not.
- No payload: the rpc must have no required parameters.
- Anything else — `Object.fromEntries(new FormData(event.currentTarget))`,
  `new FormData(form)`, a variable or a call that cannot be followed: only
  the name is checked.

#### Payloads built in the script

A script that calls several rpcs usually writes their shared keys once:

```javascript
function ids() {
    return { project_id: projectId, instance_id: serviceId() };
}
const filter = { status: "open" };

pp.rpc("list_events", { ...ids(), ...filter, page });
pp.rpc("load_service", ids());
```

Both payloads are followed to the literals they are made of, so every key
that is sent is known, and an rpc whose required parameter none of them
supplies is refused at the call: misspell `instance_id` inside `ids()` and
every call that relies on it fails to compile, naming the parameter it no
longer sends.

What is followed, in a component script:

- a function called with no arguments that is declared once, is not `async`,
  and ends in its one `return` of an object literal — a `function`, or a
  `const` arrow;
- a `const` holding an object literal that nothing else names except as a
  payload, a spread (`...filter`) or a field read (`filter.status`);
- spreads of either, to any depth.

A `let`, an object given a field afterwards (`params.extra = 1`), one handed
to a function or aliased, a function with arguments or more than one
`return`, and anything declared outside the script are not followed, and the
payload is checked as it was: by its written keys, or by name alone.

A key that arrives through a spread or a variable is not required to be a
parameter of the rpc. One helper often serves rpcs that take different
subsets of its keys, and the server ignores a key an rpc does not declare.
Only a key written in the call itself is refused when the rpc has no such
parameter.

A closed object literal also checks obvious value-shape mismatches against
standard Rust type spellings: strings/chars, booleans, numbers, vectors,
arrays, and their `Option` wrappers. For example, `name: 12` cannot satisfy
`name: String`. These are shape checks, not full JavaScript type inference:
numeric ranges, array element types, string contents, named application
types, aliases, and custom Serde representations are validated by the server.
String values are allowed through because form fields can carry JSON text:
`"42"` can deserialize as an integer, while a `String` receives the text itself.
Multipart calls do not get literal shape checks because their ordinary fields
are encoded as text. A spread that cannot be followed, or a computed key, also
prevents value-shape checks, and a key that arrives through a followed spread
is not shape-checked.
The final duplicate property is the effective value. Objects with custom
`toJSON`, prototype setters, or unknown properties have unresolved shapes.

Only a call written `pp.rpc(…)` is checked. Beyond the payloads followed
above, JavaScript variables and functions are not type-checked, and of a call's result only the reads
described under "Response Reads" and "State Reads" are. An unchecked expression is not evidence
that its value satisfies the contract.

### Response Reads

A component script that holds an awaited result in a variable has every field
it reads off that variable checked against the rpc's response shape (see
"Wire Shapes"). Given `save` answering
`Saved = { contactId: number; tags: string[]; owner: Contact | null }`:

```javascript
const result = await pp.rpc("save", { name });
result.contactId;            // checked: a field of Saved
result.owner?.name;          // checked through `| null`, into Contact
result.tags[0].trim();       // checked up to `tags`; a string's members are not
result.contact_id;           // RAHTI_RPC_RESPONSE_FIELD: Saved has no field `contact_id`
const { tags, total } = await pp.rpc("save", { name });   // `total` is refused
```

The error names the rpc, the value read from, the field, and the fields that
value has. It is the JSON Serde writes that is checked, so a
`#[serde(rename_all = "camelCase")]` response is read as `contactId`, and
renaming a field in Rust is a compile error in every script that still reads
the old name.

A result that comes back through a helper of the script's own is checked the
same way. A helper is a function the script declares that returns what its
callback returns, or an empty value (`null`, `undefined`, `[]`, `{}`, `""`),
and nothing else:

```javascript
async function act(request) {
    setBusy(true);
    try {
        return await request();
    } catch (error) {
        setFailure(error.message);
        return null;
    } finally {
        setBusy(false);
    }
}

const result = await act(() => pp.rpc("save", { name }));
result.contact_id;           // RAHTI_RPC_RESPONSE_FIELD, as above
```

The callback is written at the call and does nothing but return one
`pp.rpc`: `() => pp.rpc(…)`, `async () => await pp.rpc(…)`, or a body of one
`return`. A helper that returns anything else — a fallback object, the result
after `normalize(…)`, `result.data`, a variable the result was put in — is not
followed, and neither is a callback that does more than return the call. Write
the helper to return `await request()` directly to have its results checked.

The first parameter of a `.then` callback written on the call is the response
too, and is checked the same way:

```javascript
pp.rpc("save", { name }).then((result) => show(result.contact_id));   // refused
```

Only that callback is: a second `.then` is handed what the first returned,
and `.catch` and `.finally` are handed something else.

A read is refused only when the value is an object whose every field is
known and the name is not one of them. These are left alone:

- a result that is not held as `const x = await pp.rpc("name", …)`,
  `const x = await helper(() => pp.rpc("name", …))`, destructured from
  either, or taken as the parameter of the call's own `.then` callback: one
  returned, passed down a longer promise chain, or handed to a function and
  read there (a result stored in state is covered by "State Reads");
- a variable that is assigned again, or given fields (`result.extra = …`,
  `Object.assign(result, …)`, `delete result.field`);
- what an array, a string or a number answers to (`length`, `map`, `trim`),
  and a computed key (`result[key]`);
- a shape that is not fully known: `unknown`, a flattened map, a type the
  build could not follow (`schemaStatus` other than `resolved`), a stream;
- bindings and handlers, which hold no awaited result.

To read a field the shape does not declare, read it through another
variable: `const loose = result; loose.anything`.

### State Reads

A `pp.state` value has no declared type, but a state that is only ever given
one rpc's response holds that response. `html!` then checks every field read
off it — in the script, in a `{binding}`, in a handler, and in the rows of a
`pp-for` over it — against the response's shape. Given `list_domains`
answering `Domain[]`, where
`Domain = { id: number; name: string; records: DnsRecord[] }`:

```rust
html! {
    <section>
        <p hidden={domains.length > 0}>"No domains yet."</p>
        <ul>
            <template pp-for="domain in domains">
                <li key={domain.id}>
                    {domain.name}
                    <template pp-for="record in domain.records">
                        <span key={record.kind}>{record.value}</span>
                    </template>
                </li>
            </template>
        </ul>
        <script>r##"
            const [domains, setDomains] = pp.state([]);
            async function load() {
                setDomains(await pp.rpc("list_domains", {}));
            }
            pp.effect(() => { load(); }, []);
        "##</script>
    </section>
}
```

`{domain.nmae}` in that template is `RAHTI_STATE_FIELD`: the error lands on
the field, names the state and the rpc whose response it holds, and lists the
fields the value has. Renaming a field in the Rust struct is a compile error
in every row that still reads the old name.

A state has a shape when **everything** the block puts into it is one of:

- `await pp.rpc("name", …)`, or a result a helper hands back (see "Response
  Reads"), passed straight to the setter;
- the setter handed to the call's own `.then`: `pp.rpc("name",
  …).then(setRows)`, or `.then((rows) => setRows(rows ?? []))`;
- a variable holding such a result, or a part of one: `setRows(page.rows)`,
  `setOwner(page.owner)`, `setRows(page.rows ?? [])`;
- an empty value, which contradicts nothing: `[]`, `{}`, `null`,
  `undefined`, `""` — as the initial value or in a setter call;
- either branch of `condition ? a : b`, when both are one of the above.

Every such value must have the same shape: two rpcs that both answer `Domain`
agree, `Domain` and `Domain[]` do not. The setter calls are read from the
script and from the markup's handlers alike.

A state has no shape, and none of its reads are checked, when any one of
these is true (a state that starts from a server value is covered below):

- its setter is called with anything else: an object or array literal, a
  spread, a `.map(…)`, an updater function, an event value, another state;
- its setter is named without being called, other than as a `pp.rpc`
  call's own `.then`: passed to a function, forwarded to a component
  (`update={setRows}`), handed to a second `.then` or to a promise that is
  not a `pp.rpc` call;
- a field is written on it or on one of its rows in place
  (`rows[0].open = true`, `onclick={row.open = !row.open}`);
- the block has no script, or more than one.

#### A state that starts from a server value

A state usually starts from data the page already loaded and is reloaded by
an rpc:

```rust
pub async fn page() -> Html {
    let domains: Vec<Domain> = list_domains().await;
    html! {
        <section>
            <template pp-for="domain in domains">
                <li key={domain.id}>{domain.name}</li>
            </template>
            <script>r##"
                const [domains, setDomains] = pp.state(@{Json(&domains)});
                async function reload() {
                    setDomains(await pp.rpc("list_domains", {}));
                }
            "##</script>
        </section>
    }
}
```

Such a state holds the server value until the rpc answers, and the response
after. A field read off it is refused when **neither** has it: the response,
whose shape `html!` knows, and the server value, whose struct the compiler
knows (see "Server Value Reads" for which structs are checked). The two need
not be the same Rust type, and the state may be given a part of a response,
`setRows(page.rows)`.

A field only one of them has is not refused: a read that works before the
reload and yields `undefined` after it, or the reverse, is the page's to
handle. A server value whose struct has no shape — a private struct, a
`serde_json::Value` — leaves the read unrefused too, since nothing certain
can be said about it.

A state that is given nothing but the server value it starts from — or
`[]`, `null` and the like — holds that value, and is checked as a server
constant is: every field read off it, against the struct it is made of.

```javascript
const [filter, setFilter] = pp.state(@{Json(&filter)});
// …
setFilter(null);            // contradicts nothing: `filter` is still checked
```

The error for one of these states is reported by the compiler rather than by
`html!`. It carries a `RAHTI_STATE_FIELD` message on the field, followed by a
line of the compiler's own naming a trait that is not implemented; read the
first line.

#### Loop rows

A `pp-for` row has a shape when its collection comes from a shaped state, or
from an outer row, through steps that cannot change what the rows are:

- a path: `domain in domains`, `record in domain.records`;
- a fallback to an empty value: `rows || []`, `detail?.variables ?? []`;
- a choice whose branches agree: `query ? matches : domains`,
  `plan ? plan.phases : []`;
- an array method that returns the same elements: `filter`, `slice`, `sort`,
  `toSorted`, `reverse`, `toReversed`;
- an element: `[0]`, `find`, `findLast`, `at`;
- a script function called with no arguments, or a top-level `const`, whose
  value is made of the steps above:

```javascript
function shown() {
    const all = domains || [];
    if (!query) return all;
    return all.filter((domain) => domain.name.includes(query));
}
```

`<template pp-for="domain in shown()">` has `domains`' rows, so
`{domain.nmae}` inside it is refused. Every `return` of the function has to
be one of those steps: the function is not `async`, is declared once at the
script's top level, and reads states and its own `const`s.

Rows have no shape, and are not checked, when the list is built rather than
picked: `map`, an array or object literal, a spread, `concat`, `flat`,
`Object.values(…)`, a function called with arguments, or a function with a
`return` of any of those. To check the rows of such a list, loop over the
state and compute the extra value in the row: `{label(domain)}`.

A read is refused only when it is certain. In addition to what "Response
Reads" leaves alone, a field name the block assigns anywhere
(`item.open = false` in any function) is never refused, on any value, because
a value reached through a callback's parameter or an alias can be given that
field in place; and a block that calls `Object.assign` or
`Object.defineProperty` on an existing value has no state read refused.

To keep fields of your own beside the response's, give the state a value
built in the script: `setDomains(list.map((domain) => ({ ...domain, open:
false })))`. That state has no shape and every read of it compiles. Markup
written in another `html!` block — a fragment a Rust function returns — is
compiled separately and is not checked against this block's states.

### Server Value Reads

A script also takes server values that are not state: data rendered once
with the page and never reloaded.

```rust
pub async fn page() -> Html {
    let projects = crate::services::projects::list().await;
    html! {
        <section>
            <p hidden={projects.length > 0}>"No projects yet."</p>
            <ul>
                <template pp-for="project in projects">
                    <li key={project.id}>{project.name}</li>
                </template>
            </ul>
            <script>r##"
                const projects = @{Json(&projects)};
            "##</script>
        </section>
    }
}
```

No rpc gives `projects` a shape, and nothing has to be written for it to have
one. The build reads the application's structs, the compiler knows the type
of `Json(&projects)`, and each field read off a top-level
`const name = @{…}` — in the script, in a binding or handler, and in the
rows of a `pp-for` over it — is checked against the struct the value is made
of. `{project.nmae}` above is `RAHTI_SERVER_VALUE_FIELD`, reported by the
compiler on the field. Loops are followed as in "Loop rows":
`projects || []`, `projects.filter(…)`, a script function that returns such a
list, and a list inside a row (`database in project.databases`).

The value may be the struct, a `Vec`, a slice or an `Option` of it, owned or
borrowed, behind a `Box`, `Rc` or `Arc`, with or without `Json`. Nested
structs are followed through their fields.

A struct is checked when all of these hold:

- it derives `Serialize`, has named fields, and is not generic;
- it is `pub` or `pub(crate)`, and so is every module on the way to it —
  except that a module declared in the crate root, and a page's or a
  layout's own file, may be private;
- neither it nor a module above it is behind a `#[cfg]`.

So a struct a page declares for its own data is written `pub struct Row`,
not `struct Row`. A value whose struct does not qualify is not checked, and
nothing says so. These are also left alone:

- an enum, a `#[serde(flatten)]` field, a map, a `serde_json::Value`, a
  struct with a hand-written `Serialize`: these have no fields to check a
  read against, here or nested inside a checked struct;
- a struct from another crate;
- a field name the block assigns anywhere, and what an array, a string or
  any object answers to (`length`, `map`, `toString`);
- a server value that is not a top-level `const`: one used inside an
  expression (`@{count} + 1`), or declared with `let`.

Each field of a read is checked on its own, so the error names the step that
failed: for `project.owner.emial` it is `` `project.owner` has no field
`emial` ``. The struct is shown as its name followed by its first fields and a
count of the rest, `SessionView__id__name__ip_address__and_3_more`:

```text
error[E0277]: rahti[RAHTI_SERVER_VALUE_FIELD]: `project` has no field `nmae`: it comes
              from the server value `projects`, whose Rust struct has no such field
   |   {project.nmae}
   |            ^^^^ `Missing<Project__id__name__databases>` names the struct this is, then its fields
```

The shapes live in a module `rahti::shapes!()` expands to at the end of the
generated `src/routes.rs`. It is regenerated with the routes; never edit it,
and do not name it from application code.

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
3. Read `parameters` (`name`, `rustType`, `required`, `transport`,
   `typescript`) and `response` (`rustType`, `fallible`, `transport`,
   `typescript`). `line` and `column` locate the declaration. `typescript` is
   the JSON the browser sends or reads, and `payload.typescript` is the whole
   object a call passes; write the caller's keys and field accesses from them
   (see "Wire Shapes"). `schemaStatus` says how far to trust a shape: anything
   but `resolved` means inspect the type and its Serde representation before
   using fields. `checks` names the checking limits.
4. Keep the caller and `#[rpc]` together and use the existing
   `pp.rpc("name", { ... })` shape. Prefer explicit object keys when the data
   is already available; submit forms with the documented `FormData` pattern.
5. Run `cargo check` and then the relevant router/browser tests. Compilation
   proves the fields read directly off an awaited result, including one a
   helper hands back (see "Response Reads"), and the fields read off a state
   that only ever holds one rpc's response, including one that starts from a
   server value of that response's type (see "State Reads"), and the fields
   read off a server constant made of the application's structs (see "Server Value
   Reads"). It does not
   prove a dynamic payload, a state given anything else, or a result passed
   on to other code.

## Wire Shapes

The build reads each `#[rpc]`'s parameter and return types from source and
writes the JSON they become as TypeScript. Given

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub contact_id: i32,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[rpc]
pub async fn save(name: String, note: Option<String>) -> Result<Saved> {
    // ...
}
```

the contract for `save` carries

```json
"payload": { "typescript": "{ name: string; note?: string | null }", "schemaStatus": "resolved" },
"response": { "rustType": "Saved", "typescript": "Saved", "schemaStatus": "resolved" }
```

and the manifest's `rpcTypes`, keyed by the file the rpc is written in, holds
the declaration: `"Saved": "{ contactId: number; tags: string[]; note?: string | null }"`.
The caller reads `result.contactId`, not `result.contact_id`. A type name is
unique within its file, not across files; MCP `list_rpcs` puts the
declarations a contract names under that contract's `types`.

A type is followed through the file's own items, its `use` declarations,
`crate::`/`self::`/`super::` paths, and `mod` files, to a `struct`, `enum` or
`type` alias that derives `Serialize` (a response) or `Deserialize` (a
parameter). The shape honors `rename`, `rename_all`, `rename_all_fields`,
`skip`, `skip_serializing`, `skip_deserializing`, `skip_serializing_if`,
`default`, `flatten`, `transparent`, `tag`, `content`, `untagged`, `other`,
`into`, `from` and `try_from`. Generic types keep their parameters:
`Page<Hit>` names `"Page<Row>": "{ rows: Row[]; total: number }"`.

| Rust | TypeScript |
| --- | --- |
| `String`, `&str`, `char`, dates and times, `Uuid`, `Decimal` | `string` |
| integers, floats | `number` |
| `bool` | `boolean` |
| `()` | `null` |
| `Option<T>` | `T \| null` |
| `Vec<T>`, arrays, sets | `T[]` |
| `HashMap<K, V>`, `BTreeMap<K, V>` | `Record<string, V>` |
| tuples | `[A, B]` |
| `serde_json::Value` | `unknown` |
| `RpcFile`, `RpcUpload` | `File` |

`schemaStatus` is one of:

- `resolved` — every part was followed to a definition or a known type.
- `partial` — some part is `unknown`: a type from another crate, a
  hand-written `Serialize` or `Deserialize`, or a `with = "…"` field.
- `unresolved` — nothing about the type could be read.
- `dynamic` — JSON of any shape, such as `serde_json::Value`.
- `stream` — an `RpcStream`, whose events no type declares.

A shape is read from source, not from the compiler, so it describes what the
derive writes: a macro-generated type, a `#[path]` the scan cannot follow, or
a type shadowing a standard name is reported as `unknown` or under the
standard name's shape. `html!` checks a script's direct reads of an awaited
result against these shapes (see "Response Reads"); everything else about a
result is described here and checked by nothing.

The shapes `html!` reads are in `.rahti/rpc-shapes.tsv`, written by the build
before the crate compiles. It is generated beside the manifest: never edit
it, and a project without it compiles with no response check.

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
