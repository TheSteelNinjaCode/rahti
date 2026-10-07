# PulsePoint Performance

Read this when an input, search, filter, list, provider or panel is slow, or a
component re-renders more than it should. PulsePoint renders synchronously
when a component asks for an update, so state ownership and component
boundaries are part of the performance contract: the runtime makes
reconciliation cheap, but it cannot decide that a state change was
unnecessary.

## State Means "Render Required"

Use `pp.state(...)` when changing the value must change something rendered:

- text, attributes, lists or visibility in the markup;
- a value bound on a child component or provided through context;
- a memo, effect or other dependency that must follow the value;
- authoritative data an rpc returned.

Use `pp.ref(...)` when a value must persist but changing it must not render:

- debounce and timeout handles;
- request generations and stale-response tokens;
- cursors that are not displayed;
- previous values and handles to browser objects or libraries;
- search text used only to build a later rpc payload.

A ref change is invisible to the markup by design; never swap state for a ref
where the screen should update. A callback handed once to a library, frame
loop or subscription is `pp.event(...)` (see `pulsepoint-hooks.md`); per-frame
values live in refs, and state changes at display frequency — an FPS label
once a second, not a position every frame.

## Debounce the Work, Not a Broad Render

A debounce decides how often work starts, not how much each start costs. A
debounced setter on a large component still renders all of it:

```javascript
const [query, setQuery] = pp.state('');
function onSearch(value) {
    clearTimeout(onSearch.timer);
    onSearch.timer = setTimeout(() => setQuery(value), 300); // renders the whole owner
}
pp.effect(() => { loadRows(query); }, [query]);
```

When the query text exists only for an rpc, keep it and the timer in refs,
debounce the request itself, and put only the accepted result in state:

```rust
html! {
    <section>
        <input type="search" oninput={scheduleSearch(target.value)} />
        <template pp-for="row in rows">
            <article key={row.id}>{row.label}</article>
        </template>
        <script>r#"
            const [rows, setRows] = pp.state([]);
            const query = pp.ref('');
            const timer = pp.ref(null);
            const generation = pp.ref(0);

            function scheduleSearch(value) {
                query.current = value;
                const mine = ++generation.current;
                clearTimeout(timer.current);
                timer.current = setTimeout(() => search(mine), 300);
            }

            async function search(mine) {
                const found = await pp.rpc('search_products', { query: query.current }, { abortPrevious: true });
                if (mine !== generation.current) return; // a newer search started
                setRows(found);
            }

            pp.effect(() => () => clearTimeout(timer.current), []);
        "#</script>
    </section>
}
```

The input stays native and immediate, the bookkeeping never renders, and the
generation check stops an older answer from overwriting a newer one.

## Keep High-Frequency State Close to Its Control

A state change re-renders the component that owns the state. Put a
keystroke-frequency value in the smallest component that renders from it:

- split a search box or toolbar from the large grid it filters;
- split frequently edited form state from unrelated dialogs and panels;
- keep shared state in a parent only when several children render from it;
- do not wrap components in a parent that still owns all the fast state.

When an expensive consumer may lag one commit behind a controlled input,
feed it `pp.deferredValue(query)`. Neither `pp.deferredValue` nor
`pp.transition` makes rendering asynchronous; ownership is the fix.

## Splitting a Large Page

A page that keeps everything in one script — a game HUD, a dashboard —
re-renders all of its markup whenever any of it changes. Split it the way the
state splits:

- **Presentational pieces** that show the page's state go between component
  tags; a `{…}` binding there reads the page's scope (see "Children and
  Slots" in `rendering-and-components.md`).
- **State several components share** lives in a small store module, loaded
  before the runtime mounts, and each component reads the slice it shows with
  `pp.syncExternalStore`. A change re-renders the components whose slice
  changed, and no others:

```javascript
// public/js/office/store.js — loaded by the root layout before /js/main.js:
// <script type="module" src="/js/office/store.js"></script>
let state = { workers: [], hint: '' };
const listeners = new Set();
globalThis.officeStore = {
    subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    get: () => state,
    set(patch) { state = { ...state, ...patch }; listeners.forEach((l) => l()); },
};
```

```javascript
// In the roster component's script:
const store = globalThis.officeStore;
const workers = pp.syncExternalStore(store.subscribe, () => store.get().workers);
```

  The snapshot function must return the same value for the same state —
  select a slice the store keeps, never build a new array or object in it, or
  every store change re-renders the component.
- **A value a whole subtree reads** (a theme, the signed-in member) goes in a
  context: `pp.createContext` and `pp.context`.

Server data keeps flowing in through rpcs and sockets; the store is the
page's working state, not a cache of authority.

## When Input Edits Commit

- In a boundary of 100 or more elements, the render a text `oninput` causes
  commits on the next animation frame, so keystrokes within one frame produce
  one render. The focused input keeps its native value and caret meanwhile.
- Handlers always read committed state: a blur, and every handler other than
  the control's own value events (`input`, `change`, `blur`, a checkbox or
  radio `click`), commits pending edits before it runs, so a `submit`,
  `click` or `keydown` right after typing sees the typed value.
- On `date`, `datetime-local`, `time`, `month` and `week` inputs, an
  `oninput` handler runs on `change` and `blur`, so a half-typed date is
  never written back.
- A hidden tab runs no animation frames. A test driving a hidden browser can
  read state one frame behind; `rahti::browser` runs a visible headless page.

## Avoid Invisible and Duplicate Commits

Every setter that changes a value requests a render.

- Do not set a loading flag for a background refresh when no indicator
  changes.
- Set related state in the same synchronous turn; scheduled updates coalesce.
- Never mirror one value in two state hooks.
- Keep cursors in refs when moving them only starts work; put returned rows
  and visible metadata in state.
- `pp.memo` is for genuinely expensive derivations and for identities that
  matter. It does not make an unnecessary render free.

## Keyed Rows Are Reconciled Per Row

A render rebuilds the component's markup and reconciles it against the live
DOM, but keyed rows are remembered: a row that renders byte-identically again
is not re-parsed or re-diffed — its node, attributes, text and handlers are
kept, and runs of unchanged rows collapse together. Changing a few rows of a
long list costs about what those rows cost. A row participates when:

- the loop body renders **exactly one root element** per row;
- that element has a non-empty, unique `key`;
- the loop is not nested inside another `pp-for`;
- the row contains no nested component boundary, owned slot content or
  context provider.

A keyed list of simple rows that closes and reopens (an accordion, a filter
that matched nothing) revives the same rows — same nodes, same identities —
instead of rebuilding them. Derive row output from data, never from side
effects that expect a re-run.

So: always key rows; keep the row body single-rooted (wrap two siblings in
one element); keep values feeding a row stable — an identical row is free, a
row that differs by one character is full work.

## A Mounted Child Is Reconciled by Its Attributes

A child component's internals belong to the child. Once it is mounted, the
parent re-emits only the child's root — the attributes that carry its
props — with an empty body, so a parent render does not re-serialize subtrees
it would ignore. The first render of a boundary emits its full markup, as
does a boundary whose content the parent owns (slot content, a provider, a
parent-owned ref inside it). A bound prop is written to the child's root only
when its value changes, and a child whose props did not change is not
re-walked. If the runtime ever finds a reused boundary unmounted it logs
`[PP-WARN] Nested boundary content reuse was abandoned…` and renders from
full markup — report that warning rather than working around it.

## Identity Decides Who Re-Renders

Bound values crossing a component boundary, provided context values, and
effect dependencies all compare **by identity**. An expression that builds an
array, object or function builds a new one every render:

- derive arrays and objects with `pp.memo(() => …, [deps])`;
- create handlers that cross a boundary with `pp.callback(fn, [deps])`, or
  `pp.event(fn)` when the identity should never change;
- primitives (`count={items.length}`, `active={selected === row.id}`)
  compare by value and need nothing;
- inside one component's own markup an inline object or arrow function is
  harmless — identity only matters where something compares it.

A provider's consumers re-render whenever the provided value's identity
changes, so provide a memoized value, and split unrelated values into
separate contexts when they change at different rates:

```javascript
const [theme, setTheme] = pp.state('light');
const themeValue = pp.memo(() => ({ theme, setTheme }), [theme]);
```

```rust
html! { <themecontext.provider value={themeValue}>"…"</themecontext.provider> }
```

## Diagnose the Owner Before Blaming the Runtime

| Evidence | Classification | First action |
| --- | --- | --- |
| A setter fires but the value is not rendered | Authoring | Move the value to a ref, or call the action directly |
| Typing stalls when a debounce fires | Ownership | Debounce the rpc, not a setter on a broad owner |
| Loading and result setters cause two large commits | Authoring | Drop the invisible loading commit, or isolate the indicator |
| A fast control owns a large list, provider or dialog | Boundary | Move the state into a smaller component |
| A child re-renders though its props look unchanged | Authoring | A bound value is a new identity each render — memoize it |
| Every context consumer re-renders with the provider | Authoring | The provided value is rebuilt each render — memoize it |
| One changed row costs as much as all rows | Authoring | Key the row, single-root it, keep boundaries out of it |
| Byte-identical output still reconciles a large subtree | Possibly runtime | Capture `pp.getPerfStats()` and report it with the shape |

Measure only while profiling, around one reproduced interaction:

```javascript
pp.enablePerf();
pp.resetPerfStats();
// reproduce the interaction, then:
console.log(pp.getPerfStats());
pp.disablePerf();
```

Compare the same interaction, data and DOM size before and after a change.
Never "fix" performance with `querySelector`, hand-added listeners,
`innerHTML` or parallel DOM state; those bypass ownership instead of
correcting it.
