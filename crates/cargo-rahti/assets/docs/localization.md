# Localization

Rahti localizes server-rendered application content with build-validated Fluent
catalogs. Localization is optional: a project with no `i18n` object has no
catalog scan, generated localization layer, or request locale.

## Configuration

Declare supported Unicode locale identifiers and the default in
`rahti.config.json`:

```json
{
  "i18n": {
    "locales": ["en", "es", "es-NI"],
    "default": "en",
    "fallbacks": {
      "es": ["en"],
      "es-NI": ["es", "en"]
    },
    "dir": "lang"
  }
}
```

`locales` must be non-empty and unique after canonicalization. `default`
and every fallback must appear in it. A fallback chain cannot repeat a locale
or contain its own source. `dir` defaults to `lang`.

Each locale owns a directory of `.ftl` files:

```text
lang/
├── en/
│   ├── app.ftl
│   └── validation.ftl
└── es/
    ├── app.ftl
    └── validation.ftl
```

Files may be nested further. The build watches, parses, and embeds them in the
application binary. Invalid Fluent and duplicate message identifiers within
one locale fail the build. Runtime deployment and native packaging therefore
do not read catalogs from the installation directory.

## Routes and Locale Selection

Put localized pages below a root dynamic segment named exactly `[locale]` or
`[lang]`:

```text
src/app/
├── page.rs
└── [locale]/
    ├── layout.rs
    ├── page.rs
    └── account/
        └── page.rs
```

On those routes the segment is authoritative. `/es/account` uses `es`;
an unsupported value such as `/fr/account` answers 404 even though the
filesystem route is dynamic.

A route without either segment negotiates `Accept-Language`, then uses the
configured default. This is useful for an unprefixed entry page that redirects
to a canonical localized URL:

```rust
use axum::response::Redirect;
use rahti::i18n::localized_path;

pub async fn page() -> Redirect {
    Redirect::temporary(&localized_path("/"))
}
```

Once a URL has a locale segment, request headers never replace it. Links
within localized content should retain the explicit prefix.

## Translation

Catalog messages use Fluent syntax:

```ftl
welcome = Welcome, { $name }!

cart-items =
    { $count ->
        [one] One item
       *[other] { $count } items
    }
```

Read the active locale and translate in any page, component, layout, metadata
function, error boundary, API, or RPC reached during the request:

```rust
use rahti::i18n::{Argument, locale, t, t_with};

let current = locale();
let heading = t_with("welcome", &[("name", Argument::from("Ada"))]);
let items = t_with("cart-items", &[("count", Argument::from(3_u32))]);
let label = t("navigation-account");
```

`Argument` accepts strings and Rust integer or floating-point values. Fluent
selects the locale's plural category. Interpolated values carry Unicode
directionality isolation.

Lookup checks the active locale, its configured fallback chain, and finally
the default locale. A missing message returns its key. Translation results are
ordinary strings: `@{t("key")}` is escaped by `html!` like every other
runtime value. A catalog is never trusted raw HTML.

The active request locale is task-local, not process-global. Concurrent
requests cannot change one another's language. Framework rendering outside a
matched dynamic route, such as the root 404 fallback, uses the immutable
configured default locale.

## Documents, Direction, and Metadata

Set the document language and direction in the root layout:

```rust
pub fn layout(children: Html) -> Html {
    let locale = rahti::i18n::locale();

    html! {r##"
        <!DOCTYPE html>
        <html lang=@{locale.as_str()} dir=@{locale.direction()}>
            <head></head>
            <body><slot /></body>
        </html>
    "##}
}
```

Use the real scaffold root-layout shape; the abbreviated example only shows
the locale attributes. Common right-to-left language subtags return `rtl`;
the rest return `ltr`.

The localization layer surrounds metadata and layouts, so `metadata()` may
call `t`. Every localized dynamic response receives `Content-Language`.

## Links, Authentication, RPCs, and Caching

`localized_path("/account")` produces `/es/account` in an `es` request
and leaves an already supported locale prefix unchanged.

Authentication policies remain application-relative. A policy entry
`/account` also protects `/en/account` and `/es/account`; sign-in and
post-authentication targets are localized on the way out. Keep `next=`
pointing at the concrete localized URL.

A page RPC posts to the page's current URL, so it inherits the same locale and
may translate its response. Components rendered by the server use the same
request context. Browser-only PulsePoint expressions receive no translation
catalog automatically; seed the particular translated strings they need from
Rust.

Locale-prefixed URLs naturally partition page-cache keys because the complete
URI is part of the key. An i18n request whose locale came from
`Accept-Language` bypasses page caching; only an explicit locale URL is safe
to cache. Use an uncacheable negotiation redirect, then cache that URL.

## Static Export

A page whose only dynamic segment is `[locale]` or `[lang]` is expanded for
every configured locale automatically:

```text
src/app/[locale]/about/page.rs
    -> /en/about
    -> /es/about
```

If the page has other dynamic segments, its `static_paths()` still returns
complete concrete URLs, including the locale:

```rust
pub async fn static_paths() -> Vec<&'static str> {
    vec!["/en/products/1", "/es/products/1"]
}
```

This keeps product slugs and locale-specific content under application
control while removing repetition for locale-only pages.

## Failure Modes

- Invalid configuration, locale identifiers, Fluent syntax, duplicate message
  identifiers, or missing locale directories fail the build.
- An unsupported locale on a locale-segment route is a 404.
- A missing message walks the configured fallback chain and ultimately
  renders its key.
- Translation strings are text. Deliberately trusted markup must still use
  `Html::from_raw` at the application boundary that established trust.
