# Metadata and SEO

## Ownership

Rahti resolves document metadata on the server. A root layout supplies site
defaults, nested layouts refine a section, and the page supplies the most
specific values. The finished values are escaped and written into the root
document's `<head>`.

Do not write a managed title, description, canonical link, robots directive,
Open Graph field, or Twitter field by hand in the root layout. Keep structural
head entries there — charset, viewport, stylesheets, icons, and scripts — and
put route-dependent values in `metadata()`.

## Static Metadata

A layout or page opts in by exporting this exact function:

```rust
use crate::rahti::Metadata;

pub fn metadata() -> Metadata {
    Metadata::new()
        .title("Account")
        .description("Manage your account")
}
```

The generated router detects that export and merges it in this order:

```text
root layout -> nested layouts -> page
```

A child replaces a scalar field it supplies and inherits every scalar it
omits. Open Graph, Twitter, and extra named fields merge by key.

A typical root layout owns the defaults:

```rust
use crate::rahti::{Html, Metadata, html};

pub fn metadata() -> Metadata {
    Metadata::new()
        .default_title("Acme")
        .title_template("{title} | Acme")
        .description("Tools for Acme customers")
        .open_graph("site_name", "Acme")
        .twitter("card", "summary")
}

pub fn layout(children: Html) -> Html {
    html! {r##"
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="UTF-8" />
                <meta name="viewport"
                      content="width=device-width, initial-scale=1.0" />
                <link href="/css/styles.css" rel="stylesheet" />
                <script type="module" src="/js/main.js"></script>
            </head>
            <body><slot /></body>
        </html>
    "##}
}
```

A page below it can override only its title:

```rust
pub fn metadata() -> Metadata {
    Metadata::new().title("Billing")
}
```

The resolved title is `Billing | Acme`; the root description and social
defaults remain.

## Titles

`title_template` uses `{title}` as its placeholder. The nearest inherited
template applies to the final ordinary title:

```rust
Metadata::new().title_template("{title} | Acme")
```

Use an absolute title to bypass every inherited template:

```rust
Metadata::new().absolute_title("Acme system status")
```

A template without `{title}` is ignored. It never erases a valid page title.

## Dynamic Metadata

When metadata depends on an extractor, database record, or session, return a
`Page` containing the rendered content and request-specific metadata:

```rust
use axum::extract::Path;
use crate::rahti::{Html, Metadata, Page, html};

pub async fn page(Path(slug): Path<String>) -> Page<Html> {
    let post_title = load_title(&slug).await;

    Page::new(
        html! {r##"<article><h1>@{&post_title}</h1></article>"##},
        Metadata::new()
            .title(&post_title)
            .description(format!("Read {post_title}")),
    )
}
```

Dynamic metadata has the highest priority: it overrides the static
`metadata()` in the same page, which overrides its layouts. It is attached
to the response rather than stored globally, so concurrent requests cannot
see one another's titles.

## SEO Fields

The supported builders are:

| Builder | Output |
| --- | --- |
| `.title(value)` | templated `<title>` |
| `.default_title(value)` | fallback `<title>` when descendants omit one |
| `.absolute_title(value)` | untemplated `<title>` |
| `.title_template(value)` | inherited title rule |
| `.description(value)` | `meta name="description"` |
| `.keywords(values)` | comma-separated keywords |
| `.canonical(url)` | `link rel="canonical"` |
| `.robots(Robots::new()...)` | index/follow directives |
| `.open_graph(name, value)` | `meta property="og:name"` |
| `.twitter(name, value)` | `meta name="twitter:name"` |
| `.meta(name, value)` | another named meta field |

Do not include the namespace prefix in the two social builders:

```rust
use crate::rahti::{Metadata, Robots};

Metadata::new()
    .canonical("https://example.com/products/widget")
    .robots(Robots::new().index(true).follow(true))
    .open_graph("type", "product")
    .open_graph("title", "Widget")
    .open_graph("image", "https://example.com/og/widget.png")
    .twitter("card", "summary_large_image")
    .twitter("image", "https://example.com/og/widget.png")
    .meta("author", "Acme")
```

Metadata values are untrusted text. Rahti escapes element and attribute
content before it reaches the document. Passing a value to metadata never
makes it raw HTML.

Canonical and social-image URLs are not automatically made absolute. Supply
absolute public URLs when a crawler outside the application must fetch them.

## Client Navigation

Pages sharing a root layout navigate by replacing the body instead of loading
a new document. Every root response therefore carries the resolved managed
head in a hidden `rahti-metadata` template. The scaffolded client entry
copies that set into `document.head` after navigation.

Keep `public/js/main.js` when customizing client startup, or preserve its
metadata synchronization. A full browser load needs no JavaScript for SEO:
the same tags are already rendered directly in the server response's head.

Crossing into a different root layout performs a full load as before. Each
root layout resolves only the metadata inside its own document tree.

## Boundaries

- Metadata applies to `page.rs`, layouts inherited by pages, and the root
  `not-found.rs`. A `route.rs` is an API endpoint and has no document head.
- Components do not own route metadata. The page rendering a component owns
  the URL and its SEO contract.
- Metadata does not replace charset, viewport, stylesheet, icon, preload, or
  script tags authored by the root layout.
- Metadata is request state, not session state. It is never persisted in an
  authentication cookie.
