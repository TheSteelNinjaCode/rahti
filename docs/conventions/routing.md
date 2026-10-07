# Routing Conventions

## Filesystem Router

The configured `app.dir` is `src/app`. Directory segments become URL segments.
Only known convention filenames participate.

| File | Required export | Behavior |
| --- | --- | --- |
| `page.rs` | `pub async fn page(...)` | Server-rendered page |
| `layout.rs` | `pub fn layout(children: Html) -> Html` | Wraps descendant pages |
| `error.rs` | `pub fn error(error: ErrorInfo) -> Html` | Nearest descendant failure boundary |
| `loading.rs` | `pub fn loading() -> Html` | Markup shown while a navigation is in flight |
| root `not-found.rs` | `pub async fn not_found() -> Html` | Static/public fallback 404 body |
| `route.rs` | one or more HTTP method functions | Unwrapped endpoint |

A page, layout, or the root `not-found.rs` may also export
`pub fn metadata() -> Metadata`. Metadata follows the same layout chain and
is documented in `metadata-and-seo.md`.

A dynamic page may also export `pub async fn static_paths()` to name the
concrete URLs rendered by `cargo rahti build --static`. Static pages are
included automatically. The provider and exporter contract are documented in
`static-generation.md`.

When localization is configured, a root dynamic segment named `[locale]` or
`[lang]` has framework meaning: only configured locales match, and its value
selects the request's Fluent catalog. Other dynamic names remain ordinary path
parameters. See `localization.md`.

A public, deterministic page may export `pub fn cache() -> PageCache` to
cache its complete HTML response. This is opt-in, anonymous-only, and separate
from static generation. TTL, tags, bypass rules, and path/tag revalidation are
documented in `caching.md`.

A page may return `Html`, any compatible Axum response, or `rahti::Result`
when fallible. Its parameters may use normal Axum extractors:

```rust
pub async fn page(Path(id): Path<String>) -> Result {
    // ...
}
```

`route.rs` method names recognized by the build are `get`, `post`, `put`,
`patch`, `delete`, `head`, and `options`. API handlers do not inherit layouts
or page error boundaries.

A route segment must choose one public role. Defining both `page.rs` and
`route.rs` in the same directory is a build error: keep `page.rs` for UI or
`route.rs` for an API, or move one into a different segment.

An API-only segment can use normal Axum extractors and responses:

```rust
use axum::{Json, http::StatusCode};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct Message {
    id: u64,
    text: String,
}

#[derive(Deserialize)]
pub struct CreateMessage {
    text: String,
}

pub async fn get() -> Json<Vec<Message>> {
    Json(vec![Message {
        id: 1,
        text: "Hello from route.rs".to_string(),
    }])
}

pub async fn post(Json(input): Json<CreateMessage>) -> (StatusCode, Json<Message>) {
    (
        StatusCode::CREATED,
        Json(Message {
            id: 2,
            text: input.text,
        }),
    )
}
```

Placed at `src/app/api/messages/route.rs`, these functions answer
`GET /api/messages` and `POST /api/messages`.

With the server running, call them with:

```text
curl http://127.0.0.1:3000/api/messages
curl -X POST http://127.0.0.1:3000/api/messages \
  -H "content-type: application/json" \
  -d "{\"text\":\"Hello Rahti\"}"
```

## Segment Syntax

| Directory | URL behavior |
| --- | --- |
| `users` | literal `/users` |
| `[id]` | dynamic `/{id}` |
| `[...slug]` | required catch-all `/{*slug}` |
| `[[...slug]]` | optional catch-all: base URL and `/{*slug}` |
| `(marketing)` | route group; adds no URL segment |
| `_lib` | private; ignored recursively by the router |

Examples:

```text
src/app/page.rs                         -> /
src/app/customers/[id]/page.rs          -> /customers/{id}
src/app/posts/[...id]/page.rs           -> /posts/{*id}
src/app/docs/[[...slug]]/page.rs        -> /docs and /docs/{*slug}
src/app/(marketing)/adds/page.rs        -> /adds
```

Optional catch-all pages are registered for two shapes. A handler that must
serve both cannot require an extractor that is absent on the base URL; branch
using an optional-compatible handler design.

## Query Strings

A query string is not part of routing. Segments select the handler; `?a=1&b=2`
reaches it as request data, and no directory name or route registration
corresponds to it. A page reads it with the `Query` extractor, exactly as it
reads a dynamic segment with `Path`. A page may take several extractors —
each one reads the same request:

```rust
use axum::extract::{Query, RawQuery};
use serde::Deserialize;

use crate::rahti::{Html, html};

#[derive(Deserialize)]
pub struct Params {
    id: Option<u64>,
    name: Option<String>,
}

pub async fn page(
    Query(params): Query<Params>,
    Query(pairs): Query<Vec<(String, String)>>,
    RawQuery(raw): RawQuery,
) -> Html {
    // `params` is the typed view, `pairs` every pair, `raw` the unparsed text.
    html! { <p>@{format!("{:?}", params.id)}</p> }
}
```

Choose the target type by what the page needs:

| Target | Reads |
| --- | --- |
| a `Deserialize` struct | named parameters, typed |
| `Vec<(String, String)>` | every pair in URL order, repeats preserved |
| `HashMap<String, String>` | every name once — a repeated name keeps the last |
| `RawQuery` | the unparsed string, or `None` when there is no `?` |

`Query` deserialization failure is an extractor rejection, not a page error.
It returns a bare `400` with a plain-text body **before** the page body runs,
so no layout wraps it and the nearest `error.rs` never sees it. Two cases
produce one:

- a field the query string does not supply at all, unless the field is
  `Option`;
- a value that does not parse into the field's type — `Option` does not help
  here. `Option<u64>` tolerates a *missing* `id`; it still rejects `?id=abc`.

So `Option` states that a parameter is optional, and nothing more. When a bad
value deserves a rendered page instead of a bare 400, take the parameter as a
`String`, parse it in the body, and return `rahti::Result` so `?` reaches the
error boundary:

```rust
use axum::extract::Query;
use axum::http::StatusCode;
use serde::Deserialize;

use crate::rahti::{Error, Result, html};

#[derive(Deserialize)]
pub struct Params {
    id: String,
}

pub async fn page(Query(params): Query<Params>) -> Result {
    let id: u64 = params.id.parse().map_err(|_| {
        Error::new(StatusCode::BAD_REQUEST, format!("`{}` is not an id", params.id))
    })?;

    Ok(html! { <p>@{id}</p> })
}
```

The same applies to a `route.rs` endpoint, which takes `Query` the same way.

## Layouts

Layouts inherit by directory ancestry, including route groups. The descendant
page renders first, then its result climbs from the deepest layout to the root.
`<slot />` places the `children: Html`.

Only responses marked as Rahti page bodies are wrapped. JSON, redirects,
static assets, API routes, and RPC responses remain untouched.

The root layout owns the complete HTML document, stylesheet, runtime entry,
and the hydration cloak. Write its slot plainly:

```rust
<body>
    <slot />
</body>
```

Rahti supplies the reactive boundary itself. A block whose single parent
element is `<html>` is a document, and `html!` does not stamp `pp-component`
on the document element — that would make PulsePoint treat the whole document,
`<head>` included, as a component to re-render. It injects the marker around
the slot instead, so the rendered page carries:

```html
<body><template pp-component="layout_74cbcab0"><!-- the page --></template></body>
```

The name is the one derived from the layout's path, the same name the manifest
reports and the `X-PP-Root-Layout` header sends. Nothing is written by hand and
nothing has to be kept in step.

The injection anchors on `<slot />` and on nothing else. Interpolating
`@{children}` in a root layout would serve the same markup with no boundary in
it — the page renders, and client navigation breaks with nothing looking
wrong — so the build refuses a root layout whose `html!` contains no
`<slot />`.

A layout that needs the boundary somewhere else — around its own reactive
chrome rather than only the page — writes a `pp-component` itself. That is the
ordinary override: an author-written marker anywhere in the block turns the
injection off, and the build then reports the written name.

## Root Layouts

The outermost layout applying to a page is its *root layout*: the one that
renders `<!DOCTYPE html>`, `<html>`, `<head>`, and `<body>`. With only
`src/app/layout.rs` present, every page shares it.

An application may have more than one. Two shapes are supported:

1. No `src/app/layout.rs`, and a `layout.rs` in each top-level route group.
   The outermost layout of each group is that group's root. This is the
   Next.js arrangement.
2. `src/app/layout.rs` present, and a deeper `layout.rs` that declares itself
   a document root:

   ```rust
   pub const ROOT_LAYOUT: bool = true;

   pub fn layout(children: Html) -> Html {
       html! {
           <!DOCTYPE html>
           <html lang="en">
               <body><slot /></body>
           </html>
       }
   }
   ```

   The declaration ends layout inheritance at that file: no layout above it
   is applied to the pages below it. Without it, `app/layout.rs` would wrap a
   second `<html>` around the first.

The marker must be written on a line of its own, exactly as above; the build
text-scans for it. A layout that declares it must render a complete document.

Each root layout has an identity — the `pp-component` name the build derives
from its path, or the one written by hand in its markup. Rahti sends it two
ways on every page that layout wraps:

- the `X-PP-Root-Layout` response header;
- `<meta name="pp-root-layout" content="...">`, injected into the `<head>`.

This is what makes cross-shell navigation a full browser load rather than a
body swap; see `pulsepoint-navigation.md`. A root layout that writes the meta tag itself
keeps its own value, and is then responsible for keeping it in step with the
header.

The manifest reports `"root": true` for every layout that owns a document.

Constraints:

- Once an application has more than one root layout, every page needs one. A
  page outside them all is a build error: it would have no document of its
  own, and navigating to it would swap its markup into whichever shell was on
  screen. An application with a single root layout is not held to this — a
  page deliberately left outside it is a fragment.
- The root `not-found.rs` is rendered with the layouts at the app root. If
  there is no `src/app/layout.rs`, it renders unwrapped; keep one when a 404
  needs a document.

## Error Boundaries

A page returning `Err(Error)` or panicking is converted into a response the
nearest ancestor `error.rs` can render. The boundary receives:

- `error.code()` - numeric status;
- `error.reason()` - canonical status text;
- `error.message()` - client-safe detail;
- `error.request_id()` - optional production correlation reference.

The error UI replaces the failed subtree. Layouts below the selected boundary
are skipped; layouts above it still wrap the rendered error. RPC failures do
not pass through page error boundaries; they remain JSON errors for
`pp.rpc(...)`.

Use:

```rust
return Err(Error::new(StatusCode::BAD_REQUEST, "explanation"));
```

or let a standard error cross `?`; standard errors convert to an internal
Rahti error. Internal diagnostics and panic messages are visible in development
and server telemetry but redacted from production clients. An error with no
page boundary becomes an `application/problem+json` response. Constructor
selection, RPC behavior, cache policy, and production redaction are documented
in `error-handling.md`.

## Loading Regions

A page is rendered on the server and arrives complete, so there is nothing to
suspend on. The wait a `loading.rs` covers is the *navigation*: the moment
between a link being clicked and the response coming back. It is therefore a
browser concern, and the server's whole job is to put the markup somewhere the
runtime can reach.

`loading()` takes no arguments and no request. Rahti renders every one of them
once at startup and writes the results into every document it serves, inside a
hidden container the runtime knows the id of. Nothing is rendered per request,
and a page never waits on a loading region.

Two pieces are needed:

```rust
// src/app/dashboard/layout.rs — the region a loading file replaces. The
// marker wraps only the content pane, so the header stays on screen while
// the swap is in flight.
pub fn layout(children: Html) -> Html {
    html! {
        <div>
            <header>"Dashboard"</header>
            <main>
                <div pp-loading-content="true">
                    <slot />
                </div>
            </main>
        </div>
    }
}
```

```rust
// src/app/dashboard/loading.rs — what shows there while the fetch is out.
use crate::rahti::{Html, html};

pub fn loading() -> Html {
    html! {
        <div class="spinner">"Loading…"</div>
    }
}
```

The runtime swaps the **first** `pp-loading-content="true"` element in the
document, so write the marker in the deepest layout that wraps exactly what
the swap should replace. In a root layout it replaces the whole page; in a
section layout it replaces only that layout's content pane, which is how a
dashboard keeps its chrome during the wait. That firstness is also a trap: a
root layout that writes the marker around its whole `<slot />` is earlier in
document order and takes the swap away from every layout below it, so leave
the root layout unmarked when any section scopes its own. Without any marker
the runtime falls back to swapping the whole `<body>`.

### Which Region Shows

The browser chooses. At the start of a navigation it takes the path in the
address bar and walks it up one segment at a time, taking the first region it
finds — so `/slow/report` tries `/slow/report`, then `/slow`, then `/`.

The path in the address bar at that moment is the page being **left**, not the
one being loaded: history is updated after the response arrives. A link from
`/` to `/slow` therefore shows `app/loading.rs`, and a link from `/slow` to
anywhere shows `app/slow/loading.rs`. In practice this means a region belongs
to the section a reader is navigating *within*, and a root `app/loading.rs` is
what covers arrivals from elsewhere.

A region covers every URL below it, including URLs that do not exist yet and
dynamic segments below a static ancestor. There is no chain and no nesting:
exactly one region is used, and layouts have no say in which.

Because the walk is over literal path segments, a `loading.rs` under a dynamic
segment is a build error — `/customers/{id}` is a string no browser is ever
on, so the file would ship and never once match. Put it on a static ancestor,
which covers the dynamic children anyway. Route groups are fine: they
contribute no URL segment, so `(marketing)/loading.rs` covers `/`.

### Transition Timing

The runtime fades the content region out, swaps the markup in, and fades it
back. Both halves default to 250ms. To change them, put
`pp-loading-transition` on an element inside the loading markup, holding JSON
with `fadeIn` and `fadeOut` — a number of milliseconds, or a string with a
`ms`, `s` or `m` suffix.

JSON is braces, and braces in an authored attribute are a PulsePoint
expression, so send it as a server value:

```rust
const TRANSITION: &str = r#"{"fadeIn":"120ms","fadeOut":"120ms"}"#;

pub fn loading() -> Html {
    html! {
        <div pp-loading-transition=@{TRANSITION}>"Loading…"</div>
    }
}
```

An app that defines at least one `loading.rs` opts out of the browser's View
Transitions path: the runtime uses the loading region instead. An app with no
`loading.rs` writes no container and keeps view transitions.

## Not Found and Static Fallback

Explicit routes are matched first. Unclaimed paths fall through to the
configured public directory. If no public file exists, the root
`not-found.rs` is rendered with HTTP 404 and wrapped by the root layout.
Nested `not-found.rs` files are not currently routed.

## Generated Router

`rahti-build` groups pages sharing the same layout/error chain into Axum
subrouters. It adds page RPC POST routes, component RPC fallback, upload body
limits, static fallback, CSRF, dev routes, revalidation, and the configured
application request pipeline.

`/health` and `/ready` are framework-owned production probes. They are merged
outside application route groups and the application request pipeline; an
application convention file resolving to either exact URL is a build error.

It also installs the auth guard as the outermost layer of every group and of
the RPC router — but not over the static fallback, so `public/` is served
without a session. Which routes that guard protects is not a property of the
file tree: it is the policy the application installs from `main`. See
`authentication.md`.

Never modify `src/routes.rs`. Change the convention files or build generator
and rebuild.

Application middleware runs only around matched pages, APIs, and RPCs, after
the auth/session guard and before page or RPC execution. Static files, the 404
fallback, and framework-owned endpoints do not enter it. Its precise order,
short-circuit behavior, and configuration contract are documented in
`request-pipeline.md`.
