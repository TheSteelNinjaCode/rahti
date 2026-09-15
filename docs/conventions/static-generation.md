# Static Site Generation

## Build Command

Run the static exporter from the application root:

```text
cargo rahti build --static
cargo rahti build --static --out site
```

The default output directory is `dist/`. The command compiles CSS through the
normal build, disables development-script injection, starts the application's
normal initialization without binding a listener, renders pages through the
generated Axum router, and copies the configured public directory into the
output. This is build-time rendering by the real application, not a separate
template renderer.

The generated router includes every non-dynamic `page.rs`. API `route.rs`
handlers are not exported. A successful page is written as:

```text
/                  -> dist/index.html
/about             -> dist/about/index.html
/customers/42      -> dist/customers/42/index.html
public/css/app.css -> dist/css/app.css
```

Page routes take precedence over an asset that maps to the same output path,
the same precedence they have in the live router.

## Dynamic Paths

A dynamic page opts paths into the build by exporting this exact async
function beside `page()`:

```rust
use axum::extract::Path;

use crate::rahti::{Html, html};

pub async fn static_paths() -> Vec<String> {
    vec!["/products/first".into(), "/products/second".into()]
}

pub async fn page(Path(slug): Path<String>) -> Html {
    html! { <h1>@{slug}</h1> }
}
```

`static_paths()` returns concrete URL paths, not parameter values or path
fragments. Each path must start with `/`, contain no query string or fragment,
and match one of that page's generated route patterns. A catch-all path must
contain at least one segment. The return item can be `String`, `&str`, or any
type convertible into `String`.

The function is async so it may read application data after normal startup.
That also means the static build needs every service the function and page
need: environment variables, database access, migrations the application
chooses to run, and external services. A dynamic page with no
`static_paths()` is reported and skipped rather than guessed.

Do not declare `static_paths()` on a non-dynamic page, layout, API route, or
other convention file. The build rejects that shape because only a dynamic
page needs concrete URLs.

With localization configured, a page whose only dynamic segment is a root
`[locale]` or `[lang]` is expanded once per configured locale automatically.
A page with locale plus another dynamic segment still declares complete URLs,
including the locale, in `static_paths()`. See `localization.md`.

## What Is Exportable

The exporter writes a response only when the router returns `200 OK` with a
`text/html` content type. It reports and skips:

- redirects, including authentication redirects;
- non-success responses and extractor rejections;
- non-HTML responses;
- undeclared or invalid dynamic paths;
- two routes that would write the same HTML file.

A page containing PulsePoint RPC or WebSocket calls is still rendered, but
the report warns that those interactions require a running Rahti server. The
generated HTML and browser reactivity can be hosted as static files; backend
RPCs, WebSockets, sessions, runtime APIs, and request-time data cannot become
static merely because the initial document was exported.

Metadata is resolved through the same layout and page chain as a live request,
so static output includes the request-specific title, description, canonical,
robots, Open Graph, and Twitter tags supported by `metadata-and-seo.md`.

## Output Ownership and Safety

`dist/` is gitignored in a new project. The exporter places
`.rahti-static-output` inside its output directory. On a later build it clears
only a non-empty directory carrying that marker; it refuses to erase an
existing directory it does not own. It also refuses the project root, a
filesystem root, any output nested inside the public directory, any public
directory nested inside the output, and symlinked public assets.

If `src/main.rs` predates static generation, the command stops before running
the application. Use `cargo rahti upgrade --force src/main.rs` to adopt the
current scaffold handoff, reviewing the forced replacement first with
`--dry-run` when that file contains application changes.
