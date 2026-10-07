# PulsePoint v2 Browser Runtime

## Role in Rahti

PulsePoint is Rahti's browser-side reactive runtime. Rahti emits HTML containing
component boundaries, bindings, and component-local scripts. PulsePoint mounts
those boundaries, evaluates their scripts, tracks hook state, applies bindings,
and handles SPA navigation and RPC calls.

The runtime is a prebuilt asset. There is no JavaScript build step: an
application ships

```text
public/js/pp-reactive-v2.min.js
```

and mounts it from `public/js/main.js`. `cargo rahti new` writes the same
bundle into every project it creates, and `cargo rahti upgrade` replaces it
when the framework ships a new build.

That bundle is the whole runtime. Never hand-edit the minified file.

Companion guides:

- `pulsepoint-hooks.md` — every `pp.*` hook with an example, context, refs,
  portals, error boundaries, stable callbacks (`pp.event`).
- `pulsepoint-performance.md` — render ownership, slow inputs and lists, keyed
  rows, identity, splitting a large page, diagnosing re-renders.
- `pulsepoint-web-apis.md` — WebGPU, workers, WebAssembly, media and device
  APIs, third-party libraries, DOM ownership for games and widgets.
- `pulsepoint-navigation.md` — SPA navigation, scroll, loading regions,
  `pp.redirect`.

PulsePoint's **script API** mirrors React hooks. Its **markup is plain HTML**
with `{…}` bindings, compiled by a template compiler that has no JSX support
of any kind. Conflating the two is the most common way generated PulsePoint
code fails.

## Hard Invariants

Apply these before writing any block, even without reading further:

1. One authored root per block, with its `<script>` inside that root (see
   `rendering-and-components.md` for roots and fragments).
2. Bind first-party events with native `on*` attributes in the markup. Never
   wire your own UI with ids, `querySelector`, `addEventListener` or
   `innerHTML`.
3. A form submit reads its fields with
   `Object.fromEntries(new FormData(event.currentTarget))` and sends them with
   `pp.rpc`; refs are for imperative access only.
4. Values the markup shows live in `pp.state`; template-facing bindings are
   declared at the top level of the script.
5. Talk to the server through `pp.rpc` (`#[rpc]`), `pp.socket` (`#[socket]`)
   and rpc streaming — never a hand-written `fetch`, `EventSource` or
   `WebSocket` to your own application.
6. `pp-for` goes only on a `<template>`, with a plain `key` on the repeated
   element. Context is `pp.createContext`, a lowercase `<token.provider>` tag
   and `pp.context(token)`.
7. A `{…}` binding produces text, never elements. Conditionals are
   `hidden={…}`; lists are `<template pp-for>`.
8. The lists below are closed. If an API is not in them, it does not exist:
   do not invent hooks, directives or globals.
9. Verify behavior in a browser: a browser test with `rahti::browser`
   (`testing.md`) and an empty `browser.errors()`, or a manual check of the
   page and `.rahti/dev.log`. A passing `cargo test` proves the server.

## PulsePoint Is Not JSX

The React comparison covers exactly two things: the **hook API inside
`<script>`**, and **splitting a page into components** with props. It does
not extend to markup. PulsePoint markup is HTML that the browser parses
first; a `{…}` binding is evaluated to a value and written as text.

| JSX habit | What happens | Write instead |
| --- | --- | --- |
| `{open && <div>…</div>}` | the element is always in the page, beside stray text | `<div hidden={!open}>…</div>` |
| `{cond ? <A /> : <B />}` | both branches render | two elements with opposite `hidden={…}` |
| `{items.map(i => <li>{i.name}</li>)}` | one `<li>`, plus stray text | `<template pp-for="item in items"><li key={item.id}>{item.name}</li></template>` |
| `className=`, `htmlFor=` | attributes nothing reads | `class=`, `for=` |
| `onClick={…}` | not a DOM event attribute | `onclick={…}` |
| `defaultValue=`, `defaultChecked=` | ignored | `defaultvalue=`, `defaultchecked=` |
| `style={{color: 'red'}}` | an object is not CSS text | `pp-style={"color: " + color}` — a string |
| `dangerouslySetInnerHTML` | does not exist | `@{Html::from_raw(…)}` on the server, for trusted markup only |

`html!` refuses every row of this table at compile time, with the form to
write instead, and so does every directive that does not exist (`pp-if`,
`pp-show`, `pp-else`, `pp-model`, `pp-bind`, `pp-class`, `pp-text`,
`pp-html`, `pp-on`, `pp-key`, `pp-context`) and `pp-for` on anything but a
`<template>`. An attribute binding is a braced expression, `class={expr}`,
and so is a handler, `onclick={save()}`; a quoted handler is refused with the
braced form to write.

The one-line test: *would this markup be valid HTML if every `{…}` were
deleted?* If not, it is JSX, and it will not work.

## Server Values and Client Code

A server value never goes inside client code. HTML escaping is not
JavaScript escaping: the browser decodes `&#39;` back to `'` before the
runtime runs a handler or evaluates a `{…}` binding, so a value placed there
can end the string it sits in and execute. Hand the value over as data and
write its name in the client code:

```rust
html! {
    <section>
        <button onclick={pick(id)}>"Pick"</button>
        <script>r##"
            const id = @{Json(&user.id)};
            function pick(id) { /* … */ }
        "##</script>
    </section>
}
```

`html!` refuses each other form at compile time, with the form above as the
fix:

| Written | Refused because |
| --- | --- |
| `onclick=@{handler}` | a server value would run as a handler |
| `title="Hi @{name}"` | `@{…}` in a quoted value is text, never interpolated |
| `onclick="save()"` | a handler is client code, written as a binding: `onclick={save()}` |
| `{greet(@{name})}`, `title={label + @{suffix}}` | a `{…}` binding is browser source, copied through as written |

A spread that writes a native handler (`onclick`) from a string value is
refused at render (a debug assertion; dropped in release); a caller's handler
travels as `bindings: Bindings`. A value's braces are escaped wherever it
lands, so `title=@{comment}` and `<p>@{comment}</p>` are always text.

## Complete Directive and API Surface

**These lists are closed.** If something is not listed, it does not exist —
do not infer it from React, Vue, Alpine or another runtime.

### Template syntax

| Syntax | Where | Purpose |
| --- | --- | --- |
| `{expression}` | text, and attribute values | reactive text or attribute value |
| `on…={…}` (any native event: `onclick`, `oninput`, `onsubmit`, …) | any element | event handler |
| `pp-for="item in items"` / `"(item, index) in items"` | **`<template>` only** | keyed list |
| `key={expression}` | the repeated element inside `pp-for` | row identity |
| `pp-ref={ref}` | elements | element handle (`pp.ref`) or callback ref |
| `defaultvalue={expression}` | `<input>`, `<textarea>`, `<select>` | seed an **uncontrolled** field once |
| `defaultchecked={expression}` | checkbox, radio | seed an **uncontrolled** control once |
| `pp-style={cssText}` | any element | dynamic inline style, as a CSS **string** |
| `pp-spread={...object}` | any element | spread an object into attributes |
| `<token.provider value={…}>` (lowercase) | markup | context provider |
| `pp-spa="false"` | `<a>` | a normal browser navigation for that link |
| `pp-reset-scroll="true"` | a scroll container or `<body>` | reset its scroll on navigation |
| `pp-scroll-key="name"` | a scroll container | stable scroll-restoration identity |
| `pp-loading-content="true"` | a layout's content pane | the region a loading region replaces |
| `pp-loading-transition` | inside a `loading.rs` | fade timing, as JSON |

`pp-style` is rewritten to the native `style` attribute and merged with a
static `style` on the same element. `pp-spread` omits nullish values, writes
HTML booleans bare when `true` and drops them when `false`, and keeps
`aria-*`/`data-*` booleans as `"true"`/`"false"`.

### Hooks

`pp.state`, `pp.effect`, `pp.layoutEffect`, `pp.ref`, `pp.memo`,
`pp.callback`, `pp.event`, `pp.reducer`, `pp.context`, `pp.portal`, `pp.id`,
`pp.errorBoundary`, `pp.syncExternalStore`, `pp.imperativeHandle`,
`pp.transition`, `pp.deferredValue`, `pp.optimistic`, and the `pp.props`
bag. Each one, with an example, is in `pulsepoint-hooks.md`.

### Utilities

`pp.createContext`, `pp.mount`, `pp.redirect`, `pp.rpc`, `pp.socket`,
`pp.enablePerf`, `pp.disablePerf`, `pp.getPerfStats`, `pp.resetPerfStats`.

### Runtime-managed — never author

`pp-component`, `pp-owner`, `pp-event-owner`, `pp-ref-owner`,
`pp-ref-forward`, `<pp-context-provider>`, `<pp-fragment>`, every `data-pp-*`
attribute, and `pp-keep*`. `html!` and the runtime write them — including the
`<template pp-component="…">` around a root layout's `<slot />` and the
`<!--pp:…-->`/`<!--/pp-->` pair around a `<>…</>` fragment.

### React APIs with no equivalent

`forwardRef`, component-wrapper `memo`, `lazy`, `Suspense`,
`useInsertionEffect`, `useDebugValue`, `useActionState`, `useFormStatus`, and
a free-standing `startTransition`. Nor does PulsePoint wrap browser APIs:
there is no `pp.gpu`, `pp.worker`, `pp.audio` or `pp.storage` — call the
browser directly (`pulsepoint-web-apis.md`).

## Authoring Model

PulsePoint scripts are plain JavaScript inside the reactive root:

```rust
html! {
    <section>
        <p>{count}</p>
        <button onclick={setCount(value => value + 1)}>"Increment"</button>
        <script>r##"
            const [count, setCount] = pp.state(0);
        "##</script>
    </section>
}
```

The root and script are one component scope. Hooks must run in a stable order
during every render, as in React. Do not call hooks conditionally or after the
component is disposed.

## Component Boundaries

PulsePoint reads the served document back and mounts one scope per boundary.
A boundary arrives in one of three serialized shapes, all written by `html!`
and never by hand:

- **An element** carrying `pp-component="…"` — the ordinary case: a page's
  root, a `#[component]`'s root, a single-rooted child fragment. The
  element anchors the scope: its script, its bindings, its re-renders.
- **A template** — `<template pp-component="…">` around a root layout's
  `<slot />`, and around server-deferred content generally. The runtime
  materializes it into live DOM before mounting.
- **A comment pair** — `<!--pp:…-->` … `<!--/pp-->` around a run of
  siblings: a `<>…</>` fragment. At mount the runtime raises the pair into
  a live layout-invisible boundary element that carries the name; the
  siblings sit in the parent's flex, grid, or flow as direct items. A close
  marker pairs with the nearest unclosed open among its siblings, so
  fragments nest. Under a table-model or `<select>` parent the markers stay
  as served and no boundary is created — the fragment is grouping only
  there, and its contents belong to the enclosing scope.

The runtime compiles a boundary that has no `<script>` in its parent's
scope. That is what lets a script-less component take expressions from the
page that renders it, the way rahti-ui's parts do. A `#[component]` whose own
markup below the root writes a client `{…}` binding and has no script is
compiled in its own scope instead: `html!` adds an empty `<script></script>`
before the root closes, so `<p>{pp.props.label}</p>` reads that component's
props. A block with a spread below its root keeps the parent's scope, because
the spread may carry the caller's `bindings`.

Two things are deliberately *not* boundaries. A block whose root is a
component tag (composition) contributes no markup of its own, so it adds no
scope — the inner component's boundary is what mounts. And several instances
of one component on a page are separate scopes even though they share a
name: the runtime derives a unique identity per instance, which is why two
copies of the same counter count independently.

## First-Party UI Is PulsePoint From the Start

PulsePoint is the event, state and reactivity model for a block's own UI, not
an enhancement added afterwards:

- put the button, form, input, menu, filter or list in the `html!` of the
  block that owns it;
- bind its events with `on…={…}`, keep its values in `pp.state`, render
  conditions with `hidden={…}` and lists with `pp-for`;
- reach for `pp.ref` + `pp-ref` only for real imperative access: focus,
  measurement, media, canvas, third-party widgets, clearing a file input;
- do lifecycle work in `pp.effect`, and call the server with `pp.rpc`.

Never build a parallel JavaScript layer for it: no ids added so a script can
find an element, no `data-*` attributes as private state or as click targets
for a scanning script, no `addEventListener` for ordinary clicks and inputs,
no `innerHTML` repaints of lists, no hand-rolled store or event bus for state
one component owns. Direct DOM APIs are the narrow escape hatch for
integrations, and even then they live behind `pp.ref` + `pp.effect` in the
owning component.

```rust
html! {
    <section>
        <input value={query} oninput={setQuery(target.value)} />
        <button onclick={setQuery("")} disabled={query.length === 0}>"Clear"</button>
        <ul>
            <template pp-for="item in shown">
                <li key={item.id}>{item.label}</li>
            </template>
        </ul>
        <script>r##"
            const [query, setQuery] = pp.state('');
            const items = @{Json(&items)};
            const shown = items.filter((item) => item.label.toLowerCase().includes(query.toLowerCase()));
        "##</script>
    </section>
}
```

## Bindings

### Value Rendering

A `{expression}` does not print its value with `String(...)`. PulsePoint
serializes a bound value the way React serializes a JSX child, and the rules
below are the runtime's behavior, not an accident. Read them before binding a
value whose type is not a plain string or number — most surprises reported as
"my binding is blank" are one of these rules working as designed.

**Text position.** A `{expression}` between tags renders as:

| Value | Rendered text | Note |
| --- | --- | --- |
| `"text"`, `42`, `12n` | the value, HTML-escaped | |
| `true` / `false` | nothing | *both* booleans, not only `false` |
| `null` / `undefined` | nothing | |
| `""` | nothing | it is the empty string, not a blank |
| `0` | `0` | falsy but printed |
| `NaN` | `NaN` | |
| an array | each item rendered by these same rules, concatenated with no separator | `[1, null, 2]` → `12` |
| an object, function, or symbol | nothing, plus a `[PP-WARN]` console line | React throws here; PulsePoint omits and warns |

The boolean row is the one that costs time. A value seeded from Rust as
`false`, or an `Option::None` seeded as `null`, arrives in the browser
correctly and then renders as an empty span:

```rust
<p>Admin: <span>{admin}</span></p>   // admin === false → renders nothing
```

Spell the display out in the expression instead of binding the raw value:

```rust
<p>Admin: <span>{admin ? "yes" : "no"}</span></p>
<p>Role: <span>{role ? role : "none"}</span></p>
```

The same rule is why the React `&&` idiom leaks a zero. `{items.length && "…"}`
renders `0` when the list is empty, because `0` prints and only booleans and
nullish values vanish. Use a ternary, or `{items.length > 0 && "…"}`.

There is no escape hatch to raw HTML from a `{…}` binding. Text bindings are
always escaped, and an attribute value is escaped such that it cannot close its
quote or introduce an event handler. Trusted markup is the server's job —
`Html::from_raw` in Rust.

**Attribute position.** `name={expression}` follows the same family of rules,
split by the kind of attribute:

| Attribute kind | Value | Result |
| --- | --- | --- |
| HTML boolean (`disabled`, `hidden`, `checked`, `readonly`, …) | `true` | attribute present |
| | `false`, `null`, `undefined` | attribute absent |
| | falsy primitive (`""`, `0`, `NaN`) | attribute absent |
| | truthy primitive | present, carrying that value |
| anything else (`class`, `title`, `aria-*`, `data-*`) | `true` / `false` | the literal strings `"true"` / `"false"` |
| | `null` / `undefined` | attribute present with an empty value |
| | `0`, `NaN` | `"0"`, `"NaN"` |

Two of those rows differ from React and are worth reading twice.

A boolean on a non-boolean attribute is serialized rather than dropped, because
`aria-pressed` and `data-` flags are string-valued in the DOM and an author
writing `aria-expanded={open}` means `"true"`/`"false"`, not presence.

A nullish value on a non-boolean attribute leaves the attribute present and
empty — `title={null}` emits `title=""`, where React would omit the attribute.
It is harmless on `class` or `title` and is not harmless on a URL attribute:
`src=""` re-requests the current document and `href=""` links to it. Guard the
expression when the attribute is a URL:

```rust
<img src={avatar ? avatar : placeholder} />
```

The same binding on a component boundary *does* remove the attribute when the
value is nullish, so a nullish `src` behaves differently on a plain `<img>`
than on a component that forwards it. Do not rely on either; write the guard.

**`pp-for` collections.** A non-iterable collection renders no rows and logs a
`[PP-WARN]`; `null` and `undefined` render no rows silently. Binding a value
that is an object rather than an array is therefore a blank list, not an error.

### Events

Rahti authoring commonly uses:

```rust
onclick={save()}
oninput={setName(target.value)}
```

Within an event expression PulsePoint exposes:

- `event`, `e`, and `$event`;
- `target`;
- `currentTarget`;
- `el`.

Use native lowercase HTML event names, not JSX names such as `onClick`.

A handler on a block's root element runs in that block's scope, as in React:

```rust
#[component]
pub fn SubmitForm() -> Html {
    html! {
        <form onsubmit={handleSubmit(event)}>
            "…"
            <script>r##"function handleSubmit(e) { e.preventDefault(); }"##</script>
        </form>
    }
}
```

The runtime evaluates a root's attributes in the caller's scope, because that
is where forwarded values come from, so `html!` names the block as the owner
of the handlers written literally on its root: the tag ships with
`pp-event-owner` beside `pp-component`. Each instance resolves to itself, even
when several instances of one component share a `pp-component` name. A
caller's handler forwarded through `bindings` keeps running in the caller's
scope. One element has one event owner, so a root cannot combine its own
handlers with a caller's: an `on…=@{…}` beside an owned handler is a compile
error, and a spread that brings an event handler onto such a root is refused
at render (a debug assertion; dropped in release). Put the caller's handler on
an inner element, or call it from the block's own handler.

Handlers are the only client expressions a component writes on its own root.
Every other root attribute is evaluated in the caller's scope, so a `{…}` the
component writes there can never read the component's own state or
`pp.props`. `html!` refuses it at compile time in a `#[component]`:

```rust
<section hidden={!open}>              // compile error in a component
<section><div hidden={!open}>…</div>  // bind an element inside the root
```

A value the caller passes as a `{…}` prop and the component writes on its
root through `bindings` is the caller's expression, evaluated where it was
written. Server `@{…}` values, `pp-*` attributes, and JSON data on the root
compile as usual. Page and layout roots are not checked.

A handler in children written between component tags runs in the scope of
the block that **wrote** it, not the component it renders inside, so the
function it calls must be declared in that block's script. `ReferenceError: fn
is not defined` from a handler that clearly fired means the function lives in
the wrong script — moving the element into a wrapper component does not
change its owner, and neither does `pp.portal`.

### Forms

A form submit reads its own fields: `FormData` from the submitted form, sent
with `pp.rpc`. The input `name`s are the payload, the rpc's parameter names
receive them, and validation happens in Rust:

```rust
html! {
    <form onsubmit={save(event)} novalidate>
        <input name="name" value=@{user.name} autocomplete="name" />
        <p hidden={!errors.name}>{errors.name}</p>
        <input name="email" type="email" value=@{user.email} autocomplete="email" />
        <p hidden={!errors.email}>{errors.email}</p>
        <button disabled={saving}>{saving ? "Saving…" : "Save"}</button>
        <script>r##"
            const [saving, setSaving] = pp.state(false);
            const [errors, setErrors] = pp.state({});

            async function save(event) {
                event.preventDefault();
                if (saving) return;
                const data = Object.fromEntries(new FormData(event.currentTarget));
                setSaving(true);
                try {
                    await pp.rpc('save_profile', data);
                    setErrors({});
                } catch (failure) {
                    setErrors(failure.errors ?? {}); // 422 field messages from Validator
                } finally {
                    setSaving(false);
                }
            }
        "##</script>
    </form>
}
```

```rust
#[rpc(auth)]
pub async fn save_profile(name: String, email: String) -> Result<()> {
    let mut validator = Validator::new();
    validator.required("name", &name).required("email", &email).email("email", &email);
    validator.finish()?;
    // the signed-in member comes from the session, never from the payload
    Ok(())
}
```

- `FormData` values are strings: declare `String` parameters and parse or
  validate in Rust. A field the rpc does not declare is not read.
- `event.currentTarget` is the form that owns the handler, wherever the
  submit came from.
- A rejected `pp.rpc` is an `RpcError`; `failure.errors` is the field map a
  `Validator` produced (HTTP 422). Expected 4xx refusals are not logged as
  console errors — handle them and show them.
- Do not read a form through a ref per input, or attach a submit listener in
  an effect.

**A control is controlled or uncontrolled for its whole life.**
`value={state}` / `checked={state}` with an `oninput` handler is controlled;
`defaultvalue={…}` / `defaultchecked={…}` is uncontrolled, applied once and
then left to the user. Switching between them — usually by binding `value` to
state that starts `undefined` — logs
`[PP-WARN] <input#email> changed from uncontrolled to controlled.` Give the
state a defined initial value; never write both forms.

On a large block PulsePoint may defer the render an `input` event causes to
the next frame. A discrete event — `submit`, `click`, `keydown` — commits any
pending render before its handler runs, so a handler always reads the state
every earlier edit produced: typing then pressing Enter, or an autofill
followed by a submit in the same frame, never submits stale state. Events that
carry their control's own new value (`input`, `change`, `blur`, `focusout`,
and a checkbox or radio `click`) are not preceded by that commit.

### Conditional Rendering

There is no conditional directive. In order of preference:

1. **`hidden={…}`** — the default. The element stays in the DOM with its
   state; only visibility changes. Guard expressions inside it
   (`{target?.name}`), because a hidden subtree still renders.
   `hidden` is `display: none` from the browser's own stylesheet, and any
   `display` rule on the element overrides it; Tailwind's preflight makes it
   `display: none !important`, so it wins over `flex` or `grid` there.
2. **A ternary in a binding** — for text, classes and attributes:
   `{saving ? "Saving…" : "Save"}`, `class={"badge " + (admin ? "badge-admin" : "badge-user")}`.
3. **`pp-for` over a zero-or-one list** — when the node must truly leave the
   DOM (an expensive child, a duplicate field name, media that must stop):

```rust
<template pp-for="item in selected ? [selected] : []">
    <section key={item.id}>…</section>
</template>
```

An empty state is a `hidden` placeholder beside the list, not a branch:

```rust
<tbody>
    <tr hidden={rows.length !== 0}><td colspan="5">"No rows."</td></tr>
    <template pp-for="row in rows">
        <tr key={row.id}>…</tr>
    </template>
</tbody>
```

### Lists

For browser-owned reactive collections, use a template:

```rust
<ul>
    <template pp-for="(item, index) in items">
        <li key={item.id}>{index}": "{item.label}</li>
    </template>
</ul>
```

The collection must be iterable. Stable keys preserve row identity and local
state. For server-owned lists rendered once in Rust, use `Html::concat`
instead.

A component rendered inside a `pp-for` keeps its own loops. When a row
component with a `<script>` writes its own `<template pp-for>`, that loop is
compiled in the component's scope, not the surrounding row's, at any depth of
nesting:

```rust
#[component]
pub fn Code() -> Html {
    html! {
        <p>
            <template pp-for="(letter, index) in letters">
                <span key={index}>{letter}</span>
            </template>
            <script>r##"const letters = ["K", "7", "Q"];"##</script>
        </p>
    }
}

// In a page:
<template pp-for="row in rows">
    <article key={row.id}><Code /></article>
</template>
```

`letters` resolves in `Code` for every row the page's loop renders, even
though the page never defines it.

- Collections may be arrays or any synchronous iterable: `Set`, `Map`, typed
  arrays, strings, generators. A non-iterable value warns and renders no rows.
- Handlers inside a loop capture their own row, so later collection changes
  do not retarget an existing row's handler.
- Keys must be unique among siblings; duplicates warn and weaken diffing.
  Keyed rows that render identically are reused without being re-parsed —
  see "Keyed Rows Are Reconciled Per Row" in `pulsepoint-performance.md`.

## Hooks

The whole list is in "Complete Directive and API Surface" above, and every
hook with an example is in `pulsepoint-hooks.md`. Hooks run at the top level
of the script, in the same order on every render, as in React; never call one
conditionally or after the component is disposed.

### `pp.props`

`pp.props` holds the props of the block that wrote the expression, read from
that block's root attributes, with kebab-case names in camelCase:

- a server value the component writes on its root —
  `<form user-name=@{user_name}>` gives `pp.props.userName`, as a string;
- a client expression the caller passed as a `{…}` prop, which the component
  writes on its root through `bindings` — evaluated in the caller's scope,
  with its real type.

A Rust argument is in `pp.props` only when the component writes it on its
root.

`pp.props` reads the same in the script, in markup, and in inline handlers,
like `props.x` in JSX:

```rust
#[component]
pub fn Greeting(user_name: &str) -> Html {
    html! {
        <div user-name=@{user_name}>
            <b>{pp.props.userName}</b>
            <p>{name}</p>
            <button type="button" onclick={setName(pp.props.userName)}>"Reset"</button>
            <script>r##"const [name, setName] = pp.state(pp.props.userName ?? "");"##</script>
        </div>
    }
}
```

Handlers read the props current when the event fires. Children written
between component tags read the props of the block that wrote them. Derive
values from `pp.props` on every render; `pp.state(pp.props.x)` seeds
independent state, as `useState(props.x)` does.

## Runtime Utilities

- `pp.mount()` mounts the document; `public/js/main.js` calls it.
- `pp.redirect(url)` performs PulsePoint-aware navigation.
- `pp.rpc(name, data?, optionsOrAbort?)` calls Rahti; see
  `rpc-and-uploads.md`.
- `pp.socket(name, args?, handlers?)` opens a WebSocket to a `#[socket]`
  function and returns `{ send, close, readyState }`. It keeps quiet
  connections alive and reconnects after unexpected closes; see
  `websockets.md` for recovery and callback rules.
  The server half exists only when the app enables rahti's `ws` feature.
- `pp.enablePerf()` / `pp.disablePerf()` toggle runtime performance sampling.
- `pp.getPerfStats()` returns per-component render-phase aggregates.
- `pp.resetPerfStats()` clears the aggregates.

## Typed Modules

`public/js/pp.d.ts` declares the runtime for the application's own modules,
and `jsconfig.json` at the project root makes `public/js/` a checkable
JavaScript project: `/js/…` imports resolve to the served files and
`pp.d.ts` is in scope. Nothing is compiled. A module opts in with
`// @ts-check` on its first line; editors check it as you type, and
`npx tsc -p jsconfig.json` checks it from a terminal. `pp.d.ts` is generated
from the same build as the runtime bundle and is replaced with it by
`cargo rahti upgrade`; never hand-edit it.

```javascript
// @ts-check
/** @param {PulsePoint.RuntimeSocketHandle} socket */
export function hello(socket) {
    socket.send({ type: "hello" });
}
```

In a module the global `pp` is the page-level runtime — `pp.rpc`,
`pp.socket`, `pp.redirect` — and the `PulsePoint` namespace names its types
for JSDoc. Hooks (`pp.state`, `pp.effect`, …) belong to a component's own
`<script>`, so the declarations do not offer them to a module. A module that
imports a library without type information (a vendored three.js build) needs
that library's own declarations before it can be checked.

## Development Diagnostics

In development mode Rahti injects its classic diagnostic script before
deferred module scripts execute. That ordering lets it wrap `console.warn` and
`console.error` and install global error listeners before PulsePoint mounts.

Captured events are mirrored to the server terminal and written as JSON Lines
to `.rahti/dev.log`. See `diagnostics.md` for the schema, limits, privacy
boundary, and AI debugging workflow.

A component's script is compiled with a name, so an error thrown inside it
has a stack frame of the form `pp://component/<boundary-id>.js:<line>:<col>`
rather than an anonymous eval. The boundary id is the component's
`pp-component` name, which `.rahti/manifest.json` maps back to its source
file; the line counts from the first line of the script body (engines add a
constant two-line header above it).

## APIs Not Present

See "React APIs with no equivalent" above. Do not invent React compatibility,
and do not invent wrappers around browser APIs.
