# PulsePoint Hooks

The `pp.*` API inside a component `<script>`. It mirrors React hooks; the
markup around it never does — see `pulsepoint.md` for the template contract.
Examples are `html! { … }` templates, whose `<script>r##"…"##</script>` body
is plain JavaScript (see "Templates" in `rendering-and-components.md`).

## The Whole List

Hooks, called at the top level of a component script in a stable order on
every render:

| Hook | Returns | Use |
| --- | --- | --- |
| `pp.state(initial)` | `[value, setValue]` | A value the markup shows. The setter takes a value or `(previous) => next`; equal values (`Object.is`) do not re-render |
| `pp.effect(callback, deps?)` | — | Work after commit; may return a synchronous cleanup |
| `pp.layoutEffect(callback, deps?)` | — | Synchronous work after DOM mutation, ref binding and portals, before `pp.effect` — measurement |
| `pp.ref(initial?)` | `{ current }` | A value that persists without re-rendering; an element handle through `pp-ref` |
| `pp.memo(factory, deps)` | value | A derived value recomputed only when `deps` change |
| `pp.callback(fn, deps)` | function | A function whose identity changes only when `deps` change |
| `pp.event(fn)` | function | A function whose identity never changes and that always runs the latest render's `fn` |
| `pp.reducer(reducer, initial, init?)` | `[state, dispatch]` | Multi-field update logic |
| `pp.context(token)` | value | The nearest provided value of a context |
| `pp.portal(ref, target?)` | `{ sourceParent }` | Render a ref'd element under another parent (default `document.body`) |
| `pp.id()` | string | A stable, DOM-safe unique id for `id`/`for`/`aria-*` |
| `pp.syncExternalStore(subscribe, getSnapshot)` | snapshot | Read a mutable source PulsePoint does not own |
| `pp.imperativeHandle(ref, create, deps?)` | — | Publish a child-defined API on a parent's ref |
| `pp.transition()` | `[isPending, start]` | A pending flag around synchronous or promise-returning work |
| `pp.deferredValue(value, initial?)` | value | A copy of `value` that catches up one commit later |
| `pp.optimistic(passthrough, reducer?)` | `[value, add]` | Show a guess until the confirmed value changes |
| `pp.errorBoundary()` | `[error, reset]` | Catch render and effect failures below and in this component |
| `pp.props` | object | The component's props |

Utilities, on the same `pp` and on the global one: `pp.createContext`,
`pp.mount`, `pp.redirect`, `pp.rpc`, `pp.socket`, `pp.enablePerf`,
`pp.disablePerf`, `pp.getPerfStats`, `pp.resetPerfStats`.

Rules that hold for every hook:

- Effects return a synchronous cleanup or nothing. A promise returned from an
  effect is ignored with a warning; start async work inside the effect and
  guard it with a `cancelled` flag.
- Pass an explicit dependency array to `pp.effect`, `pp.layoutEffect`,
  `pp.memo` and `pp.callback`. Without one, an effect runs after every
  render. Dependencies compare by identity, so an object or function rebuilt
  each render belongs in `pp.memo`/`pp.callback` before it is a dependency.
- Top-level declarations reach the markup — every destructuring form
  included: `const [isPending, start] = pp.transition()` and
  `const [error, reset] = pp.errorBoundary()` bind like `pp.state` does.
  Declarations inside functions and blocks do not.
- A `pp-ref` callback may return a synchronous cleanup, run when the ref is
  replaced or detached; a callback without one is called with `null` on
  detach.

## Stable Callbacks That Read Current State

Use `pp.event` whenever a game loop, library, timer, observer or socket keeps
a callback registered across renders. A plain function captured by a `[]`
effect reads the state of the render that created it, and so does
`pp.callback(fn, [])`. `pp.event(fn)` keeps one identity and calls the latest
`fn`, passing its arguments, return value and `this` through.

```rust
html! {
    <section>
        <button onclick={setPaused(!paused)}>{paused ? "Resume" : "Pause"}</button>
        <script>r##"
            const [paused, setPaused] = pp.state(false);
            const engine = pp.ref(null);
            const tick = pp.event((time) => {
                if (!paused) engine.current?.step(time);
            });
            pp.effect(() => {
                let id = requestAnimationFrame(function frame(time) {
                    tick(time);
                    id = requestAnimationFrame(frame);
                });
                return () => cancelAnimationFrame(id);
            }, [tick]);
        "##</script>
    </section>
}
```

- Call `pp.event` at the top level, like every hook; invoke the returned
  function from the integration.
- Use `pp.callback(fn, deps)` when changing dependencies should change the
  identity. Keep dependencies that recreate a resource (a room id, a document
  id) in the setup effect: `pp.event` does not re-run an effect.
- An async callback still needs a cancellation or request-generation check
  after each `await`: the latest `fn` is chosen when the call starts.

## Effects

Attach in the body, detach in the returned cleanup. The cleanup also runs on
unmount, including the unmount an SPA navigation causes.

```rust
html! {
    <section>
        <p>"Elapsed: "{seconds}"s"</p>
        <button onclick={setRunning(!running)}>{running ? "Pause" : "Resume"}</button>
        <script>r##"
            const [seconds, setSeconds] = pp.state(0);
            const [running, setRunning] = pp.state(true);
            pp.effect(() => {
                if (!running) return;
                const id = setInterval(() => setSeconds((s) => s + 1), 1000);
                return () => clearInterval(id);
            }, [running]);
        "##</script>
    </section>
}
```

- When a render or a navigation removes a subtree, every `pp.layoutEffect`
  cleanup in that subtree runs before any `pp.effect` cleanup in it, as in
  React. A cleanup that throws during that teardown is logged with
  `[PP-ERROR]` and the remaining cleanups still run.
- An effect that sets state on every run re-renders forever: effects and
  state flushes are both microtasks, so the page would freeze. After 50
  renders in a row driven only by state set inside `pp.effect`, PulsePoint
  drops the next render and logs `[PP-WARN] Maximum update depth exceeded
  for component "…"`, which reaches `.rahti/dev.log`. The cause is a missing
  dependency array or a dependency rebuilt each render. Any other update — an
  event, a parent re-render, an async callback — starts the count again.

## Derived Values and Reducers

A plain top-level `const` recomputed each render is right for cheap
derivations. `pp.memo` is for heavy ones, and for values whose identity
matters as a dependency or a context value:

```javascript
const visible = pp.memo(
    () => items.filter((item) => item.label.toLowerCase().includes(query.toLowerCase())),
    [items, query]
);
```

`pp.reducer` keeps multi-field updates in one place:

```javascript
const [cart, dispatch] = pp.reducer((state, action) => {
    if (action.type === 'add') return { count: state.count + 1, total: state.total + action.price };
    if (action.type === 'clear') return { count: 0, total: 0 };
    return state;
}, { count: 0, total: 0 });
```

## Portals

`pp.portal(ref, target?)` moves a ref'd element to `document.body`, or to
`target`, for overlays that must escape a clipping ancestor. Portaled content
keeps its logical component ancestry: context, props and events keep working.

```rust
html! {
    <div>
        <button onclick={setOpen(true)}>"Open"</button>
        <div pp-ref={dialog} hidden={!open} class="modal">
            <button onclick={setOpen(false)}>"Close"</button>
        </div>
        <script>r##"
            const [open, setOpen] = pp.state(false);
            const dialog = pp.ref(null);
            pp.portal(dialog);
        "##</script>
    </div>
}
```

## Stable Ids

`pp.id()` is derived from the component and the hook slot: stable across
renders, unique across instances. Use it for every `id`/`for` pair and
`aria-*` reference; never derive ids from an index or a counter.

```rust
html! {
    <div>
        <label for={emailId}>"Email"</label>
        <input id={emailId} type="email" aria-describedby={hintId} />
        <p id={hintId}>"We never share this."</p>
        <script>r##"
            const emailId = pp.id();
            const hintId = pp.id();
        "##</script>
    </div>
}
```

## External Stores

`pp.syncExternalStore` reads a source the component does not own — a media
query, a store module, a library's state — during render, and closes the gap
between render and subscription. `subscribe` must keep its identity: wrap an
inline one in `pp.callback(…, [])`, or pass a function the store already
holds. `getSnapshot` must return the same value for the same state; select a
slice the store keeps, never a freshly built array or object. On mount
PulsePoint calls it twice and logs `[PP-WARN] pp.syncExternalStore
getSnapshot for component "…" returned a different value on two calls` when
the results differ by `Object.is`, because such a snapshot re-renders on every
store notification.

```javascript
const subscribe = pp.callback((onChange) => {
    const query = window.matchMedia('(max-width: 640px)');
    query.addEventListener('change', onChange);
    return () => query.removeEventListener('change', onChange);
}, []);
const isNarrow = pp.syncExternalStore(subscribe, () => window.matchMedia('(max-width: 640px)').matches);
```

Sharing one store between components is in "Splitting a Large Page" in
`pulsepoint-performance.md`.

## Imperative Handles

A child publishes an API on a ref the parent owns, instead of exposing its
DOM. The handle is published in the layout-effect phase and cleared on
unmount, so call it from handlers and effects, not during render.

```javascript
// The child's script; `controlRef` is the parent's ref, passed as a prop.
const [open, setOpen] = pp.state(false);
pp.imperativeHandle(pp.props.controlRef, () => ({
    open: () => setOpen(true),
    close: () => setOpen(false),
}), []);
```

`pp-ref` on an element binds that element; `pp.imperativeHandle` is for a
child-defined API. Never author `pp-ref-forward`.

## Pending, Deferred and Optimistic Values

`pp.transition()` gives a correct pending flag around synchronous and
promise-returning work — the common use is disabling a submit button while
`pp.rpc` resolves. It does not time-slice: PulsePoint renders synchronously.
Overlapping scopes stay pending until the last one settles.

```javascript
const [isSaving, startSaving] = pp.transition();
function save() {
    startSaving(() => pp.rpc('save_profile', { name }));
}
```

`pp.deferredValue(value)` lags one commit, so a responsive input can feed an
expensive consumer that catches up a frame later.

`pp.optimistic(confirmed, reducer?)` shows a guess on top of `confirmed`. A
guess is dropped when `confirmed` changes — the server answered, so its value
never double-counts — and, when the guess was added inside a `pp.transition`
scope, when that transition settles. Add guesses inside a transition: a
refused `pp.rpc` then rolls the guess back instead of showing it forever.

```rust
html! {
    <section>
        <button disabled={liking} onclick={like()}>"Like ("{likes}")"</button>
        <script>r##"
            const [confirmedLikes, setConfirmedLikes] = pp.state(@{likes});
            const [likes, addLike] = pp.optimistic(confirmedLikes, (state, delta) => state + delta);
            const [liking, startLiking] = pp.transition();
            function like() {
                startLiking(async () => {
                    addLike(1);
                    try {
                        const result = await pp.rpc('like_post', { post: @{post_id} });
                        setConfirmedLikes(result.likes);
                    } catch (failure) {
                        // Refused: the guess is dropped when the transition settles.
                    }
                });
            }
        "##</script>
    </section>
}
```

- A guess added before the scope's first `await` belongs to that transition.
  A guess added after an `await` cannot be traced to one, so it lasts until
  every transition in flight at that moment has settled.
- A guess added with no transition in flight lasts until `confirmed`
  changes; if the request fails and nothing changes `confirmed`, it stays.

## Error Boundaries

`pp.errorBoundary()` makes a component the recovery point for render and
effect failures:

```rust
html! {
    <section>
        <div hidden={!error}>
            <p>"Something went wrong: "{error?.message}</p>
            <button onclick={reset()}>"Try again"</button>
        </div>
        <div hidden={!!error}>
            <ReportChart />
        </div>
        <script>r##"
            const [error, reset] = pp.errorBoundary();
        "##</script>
    </section>
}
```

- A throw in a descendant's render or effects bubbles up the logical
  component ancestry to the nearest boundary. Uncaught, the runtime logs
  `[PP-ERROR] Render Cycle Failed`.
- A boundary also catches throws from its own render and effects, since its
  fallback markup lives in the same component.
- Effect bodies and effect cleanups take the same path as render failures.
- The boundary holds the error until `reset()`. It absorbs at most five errors
  without a `reset()`, then logs and stops catching.
- A throw from a boundary's own render while it shows its fallback (it holds
  an error) is a broken fallback, not a new error to show: it goes to the next
  boundary up, as in React. Keep fallback markup simple and guarded
  (`{error?.message}`).
- Event-handler errors are not routed to boundaries: use `try`/`catch` in the
  handler, as in React.

## Context

A token, a provider tag in markup, and `pp.context(token)` in descendants.
The provider tag is the token's name in lowercase with `.provider` —
`<themecontext.provider>` for `ThemeContext` — because markup is HTML:

```rust
html! {
    <section>
        <themecontext.provider value={theme}>
            <ThemeBadge />
        </themecontext.provider>
        <script>r##"
            const ThemeContext = pp.createContext("light");
            const [theme, setTheme] = pp.state("dark");
        "##</script>
    </section>
}
```

- Resolution walks the logical component ancestry, not the live DOM, so
  portaled descendants still find their provider.
- Consumers re-render when the provided value changes identity, and only
  then. Provide a memoized object, not one built each render (see
  `pulsepoint-performance.md`).
- A component does not consume the value it provides in the same render.
- Provider and consumer must share the token object. When they live in
  different component scripts, put the token in a store module both read, or
  pass it down.
- When a provider stops providing or is destroyed, consumers fall back to the
  next ancestor provider, then to the token's default.
- There is no `pp-context` attribute and no `pp.provideContext`.

## Refs

- Refs are for imperative access: focus, measurement, media, canvas,
  third-party widgets, resetting a file input. They are not the way to read
  form fields on submit — that is `FormData` (see "Forms" in `pulsepoint.md`).
- `pp-ref={name}` binds an element to the ref or callback `name` in scope;
  `pp-ref={(el) => setNode(el)}` binds a callback expression.
- Refs attach before layout effects and passive effects run, and detach with
  `null` (or the callback's returned cleanup) when the element unmounts.
- Ref bindings survive re-renders, including re-renders that change nothing.
- The runtime writes `data-pp-ref` internally; never author it.

```rust
html! {
    <div>
        <input pp-ref={nameInput} />
        <button onclick={nameInput.current?.focus()}>"Focus"</button>
        <script>r##"
            const nameInput = pp.ref(null);
        "##</script>
    </div>
}
```
