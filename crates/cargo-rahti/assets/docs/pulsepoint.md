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

## Authoring Model

PulsePoint scripts are plain JavaScript inside the reactive root:

```rust
html! {
    <section>
        <p>{count}</p>
        <button onclick={setCount(value => value + 1)}>"Increment"</button>
        <script>
            const [count, setCount] = pp.state(0);
        </script>
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

Two things are deliberately *not* boundaries. A block whose root is a
component tag (composition) contributes no markup of its own, so it adds no
scope — the inner component's boundary is what mounts. And several instances
of one component on a page are separate scopes even though they share a
name: the runtime derives a unique identity per instance, which is why two
copies of the same counter count independently.

## Binding Surface

PulsePoint supports:

| Syntax | Purpose |
| --- | --- |
| `{expression}` in text | Escaped reactive text/value |
| `{expression}` in a quoted emitted attribute | Reactive attribute/property |
| any native `on*` attribute | Event handler expression |
| `pp-for="item in items"` | Loop on a `<template>` |
| `pp-for="(item, index) in items"` | Loop with index |
| `key="{expression}"` | Repeated row identity |
| `pp-ref={ref}` | Bind a native element/ref |
| `pp-style="{cssText}"` | Dynamic inline style |
| `pp-spread="{object}"` | Spread dynamic attributes |
| lowercase `<token.provider value="{value}">` | Context provider |
| `pp-spa="false"` on `<a>` | Opt out of SPA interception |
| `pp-reset-scroll="true"` | Reset a scroll container on navigation |
| `pp-scroll-key="name"` | Stable scroll-restoration identity |
| `pp-loading-content="true"` | The region a `loading.rs` replaces |
| `pp-loading-transition` | Fade timing for that swap, as JSON |

There is no `pp-if`, `pp-show`, `pp-else`, or `pp-key`. Use boolean attributes
such as `hidden={condition}`, ternary expressions, and plain `key`.

Do not handwrite runtime internals including `pp-component`, `pp-owner`,
`pp-ref-owner`, `pp-ref-forward`, or `data-pp-*`. Rahti's macros and the
runtime manage them — including the `<template pp-component="...">` a root
layout's page is wrapped in, which `html!` injects around the layout's
`<slot />`, and the `<!--pp:…-->`/`<!--/pp-->` comment pair a `<>…</>`
fragment ships between. The same goes for the `<pp-fragment>` element the
runtime raises a pair into at mount: never author one.

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
<p>"Admin: "<span>{admin}</span></p>   // admin === false → renders nothing
```

Spell the display out in the expression instead of binding the raw value:

```rust
<p>"Admin: "<span>{admin ? "yes" : "no"}</span></p>
<p>"Role: "<span>{role ? role : "none"}</span></p>
```

The same rule is why the React `&&` idiom leaks a zero. `{items.length && "…"}`
renders `0` when the list is empty, because `0` prints and only booleans and
nullish values vanish. Use a ternary, or `{items.length > 0 && "…"}`.

There is no escape hatch to raw HTML from a `{…}` binding. Text bindings are
always escaped, and an attribute value is escaped such that it cannot close its
quote or introduce an event handler. Trusted markup is the server's job —
`Html::from_raw` in Rust.

**Attribute position.** `name="{expression}"` follows the same family of rules,
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
<img src="{avatar ? avatar : placeholder}" />
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
            …
            <script>function handleSubmit(e) { e.preventDefault(); }</script>
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

### Forms

`value={state}` and `checked={state}` are controlled bindings. Default values
are uncontrolled. Do not switch a control between controlled and uncontrolled
after mount. PulsePoint preserves selection/focus and handles input, textarea,
select, checkbox/radio, and form-reset behavior.

Use `pp.ref` plus `pp-ref` for file inputs and imperative DOM access:

```rust
<input type="file" pp-ref={file} />
<script>
    const file = pp.ref(null);
</script>
```

### Lists

For browser-owned reactive collections, use a template:

```rust
<ul>
    <template pp-for="(item, index) in items">
        <li key="{item.id}">{index}: {item.label}</li>
    </template>
</ul>
```

The collection must be iterable. Stable keys preserve row identity and local
state. For server-owned lists rendered once in Rust, use `Html::concat`
instead.

## Hooks

### `pp.state(initial)`

Returns `[value, setter]`. The initial value may be a value or lazy function.
The setter accepts a replacement or `(previous) => next`. Equal values use
`Object.is` and do not rerender.

### `pp.effect(callback, deps?)`

Runs after commit. It may return a synchronous cleanup function. Start async
work inside the callback; do not return a Promise. Omitted dependencies rerun,
`[]` runs once per mount, and a dependency array uses `Object.is`.

### `pp.layoutEffect(callback, deps?)`

Runs synchronously after DOM mutation and before normal effects. Reserve it for
measurement or synchronous DOM coordination.

### `pp.ref(initial?)`

Returns a stable `{ current }` object. Pair it with `pp-ref`, portals, or
imperative handles.

### `pp.memo(factory, deps?)`

Memoizes a computed value.

### `pp.callback(function, deps?)`

Memoizes a function identity. Use for stable subscriptions and callbacks.

### `pp.reducer(reducer, initial, init?)`

Returns `[state, dispatch]`; supports an optional initializer.

### `pp.createContext(defaultValue)` and `pp.context(token)`

Creates and reads logical ancestor context. Providers are rendered with the
lowercase token-derived `.provider` tag. Reads belong in the component script
render phase, not inside a later event callback.

### `pp.portal(ref, target?)`

Moves the referenced content to another DOM target while preserving logical
component ancestry, context, and lifecycle.

### `pp.id()`

Returns a stable component-local identifier for `id`, `for`, and ARIA pairs.

### `pp.errorBoundary()`

Returns `[error, reset]`. It catches descendant render/effect failures. Call
`reset()` to clear the latched error and re-arm it. It also catches the
declaring component's own render and effect failures; without a boundary,
errors walk to the nearest ancestor boundary. After five failures without a
reset, a boundary stops catching to avoid a render loop and logs the error.
Event-handler exceptions are logged by the event system, so handle expected
async failures in the event handler itself. This is a client boundary,
distinct from Rahti's server-side `error.rs`.

### `pp.syncExternalStore(subscribe, getSnapshot)`

Subscribes to browser/external state. Memoize `subscribe` when its identity
would otherwise change each render.

### `pp.imperativeHandle(ref, createHandle, deps?)`

Publishes a controlled imperative API through an object or callback ref.

### `pp.transition()`

Returns `[isPending, startTransition]`. The transition remains pending until a
synchronous scope or returned Promise settles.

### `pp.deferredValue(value, initial?)`

Returns a value that updates after the current commit.

### `pp.optimistic(passthrough, reducer?)`

Returns `[optimisticValue, addOptimistic]`. Pending optimistic actions replay
over the confirmed value and reset when the passthrough value changes.

### `pp.props`

Contains component-boundary props resolved from root attributes. Rahti Rust
components normally pass server props through rendered markup or seed them in
the script. Do not assume a Rust function argument automatically appears in
`pp.props`.

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

## SPA Navigation

PulsePoint intercepts eligible same-origin links after mount, fetches the next
document, morphs owned DOM, preserves component identity where possible, and
manages focus, scroll, history, redirects, and loading regions. Use
`pp-spa="false"` for a hard navigation.

Do not add a second client router. Rahti remains the URL and response authority.

### Crossing Root Layouts

A body swap only makes sense while the document stays the same. When an
application has more than one root layout, the two shells have different
`<head>` contents — different stylesheets, different scripts, possibly a
different runtime setup — and swapping one body into the other would leave
the page running the wrong shell.

PulsePoint therefore compares the `X-PP-Root-Layout` header of the navigation
response with the `<meta name="pp-root-layout">` of the current document. When
both are present and they differ, it abandons the SPA path and sets
`window.location.href` to the target, which is an ordinary full load. Equal
values navigate in place as usual.

Rahti sends both halves for every page a root layout wraps, including the root
404, so nothing has to be written by hand. See "Root Layouts" in `routing.md`
for how the identity is derived and how a second root layout is declared.

A navigation between two pages under the same root layout is unaffected: the
identities match, and the body is swapped as usual.

### Loading Regions

While a navigation is in flight the runtime replaces the first element in the
document marked `pp-loading-content="true"` — or the whole `<body>` when
nothing is marked — with the markup of the nearest loading region, then
restores it from the response. Marking a section layout's content pane rather
than a root layout's slot keeps the chrome around it on screen for the wait. The regions come from the application's
`loading.rs` files; Rahti renders them once and writes them into every
document inside a hidden container, so nothing about this is authored by hand
beyond the two attributes above.

The region is chosen from the path in the address bar when the navigation
starts, which is the page being left. Ownership, the literal-path walk, the
build-time refusal of a region under a dynamic segment, and the transition
JSON are all in "Loading Regions" in `routing.md`.

Defining any `loading.rs` opts the application out of the View Transitions
path: the runtime uses the loading region instead. An application with no
`loading.rs` writes no container and keeps view transitions.

## Development Diagnostics

In development mode Rahti injects its classic diagnostic script before
deferred module scripts execute. That ordering lets it wrap `console.warn` and
`console.error` and install global error listeners before PulsePoint mounts.

Captured events are mirrored to the server terminal and written as JSON Lines
to `.rahti/dev.log`. See `diagnostics.md` for the schema, limits, privacy
boundary, and AI debugging workflow.

## APIs Not Present

Do not invent React compatibility. PulsePoint v2 has no direct equivalent for
`forwardRef`, component-wrapper `memo`, `lazy`, `Suspense`,
`useInsertionEffect`, `useActionState`, `useFormStatus`, or a free-standing
`startTransition`.
