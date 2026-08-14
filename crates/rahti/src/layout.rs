//! Wrapping a page in its layouts.
//!
//! `src/routes.rs` groups routes by the layout chain they inherit and hangs
//! one [`apply_layout`] layer per layout on the group, deepest layout first.
//! A response therefore climbs out through `(marketing)/layout.rs` and then
//! `app/layout.rs`, which is the nesting order Next.js uses.
//!
//! The outermost layout of a chain is the *root layout*: the one that owns
//! the document. It is applied by [`apply_root_layout`] instead, which is the
//! same wrap plus the identity the client runtime navigates by — see
//! [`ROOT_LAYOUT_HEADER`].
//!
//! Composition happens on the response rather than at the call site because
//! the build script only text-scans `app/` — it cannot know that
//! `customers/[id]/page.rs` takes a `Path<String>` and so cannot generate a
//! wrapper with the right signature.

use axum::body::{Body, to_bytes};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::html::{Html, PageBody};

#[cfg(test)]
#[path = "tests/layout.rs"]
mod tests;

/// Bodies are built in memory by `html!`, so the whole response is already
/// resident. The cap only guards against a handler streaming something huge
/// into a route that a layout covers.
const MAX_PAGE_BYTES: usize = 8 * 1024 * 1024;

/// Which root layout rendered this response.
///
/// PulsePoint reads it off the navigation `fetch` and compares it with the
/// `<meta name="pp-root-layout">` of the document already on screen. Equal
/// means the two pages share a document shell and the body can be swapped in
/// place; different means they do not, and the runtime falls back to a full
/// browser load so the new shell's stylesheets, scripts and `<head>` actually
/// take effect.
///
/// Lowercase because `HeaderValue`/`HeaderName` want it that way; HTTP header
/// names are case-insensitive and the client reads `X-PP-Root-Layout`.
pub const ROOT_LAYOUT_HEADER: HeaderName = HeaderName::from_static("x-pp-root-layout");

/// The same identity, in the document itself.
const ROOT_LAYOUT_META: &str = "name=\"pp-root-layout\"";

/// Feed a response body through one layout.
///
/// Responses without a [`PageBody`] marker are returned untouched, which is
/// what keeps `route.rs` handlers and files served out of `public/` from
/// being wrapped in a page shell.
pub async fn apply_layout(layout: fn(Html) -> Html, res: Response) -> Response {
    wrap(layout, res, None).await
}

/// The same, for the layout that owns the document.
///
/// `id` names the root layout — a value the build derives from the layout
/// file, so two root layouts never share one. It is written into the response
/// header and into the document's `<head>`, which is the pair the client
/// navigation compares.
pub async fn apply_root_layout(
    layout: fn(Html) -> Html,
    id: &'static str,
    res: Response,
) -> Response {
    wrap(layout, res, Some(id)).await
}

/// A page the router assembled by calling its layouts directly, marked as the
/// document of `id`.
///
/// The generated 404 handler is the one caller: it has no layer to hang
/// [`apply_root_layout`] on, because it renders below the static-file
/// fallback rather than inside a route group.
pub fn root_document(id: &'static str, html: Html) -> Response {
    let mut res =
        Html::from_raw(super::loading::inject(mark(html.into_string(), id))).into_response();
    set_root_header(&mut res, id);
    res
}

async fn wrap(layout: fn(Html) -> Html, res: Response, root: Option<&'static str>) -> Response {
    if res.extensions().get::<PageBody>().is_none() {
        return res;
    }

    let (mut parts, body) = res.into_parts();

    let bytes = match to_bytes(body, MAX_PAGE_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "rahti: page body too large to wrap in its layout",
            )
                .into_response();
        }
    };

    let inner = match String::from_utf8(bytes.into()) {
        Ok(inner) => inner,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "rahti: page body was not valid UTF-8",
            )
                .into_response();
        }
    };

    let wrapped = layout(Html::from_raw(inner));

    // Every page passes through here, so this is the one place the dev
    // client can be added without an app having to opt in. No-op outside
    // dev mode.
    let mut body = super::dev::inject(wrapped.into_string());

    // The root layout owns the document, so it is the one wrap that can carry
    // the app-wide furniture: the identity the client navigates by, and the
    // loading regions it reads at the start of a navigation.
    if let Some(id) = root {
        body = super::loading::inject(mark(body, id));
    }

    // The length changed, and the marker stays so any outer layout in the
    // chain still recognises this as a page.
    parts.headers.remove(header::CONTENT_LENGTH);
    let mut res = Response::from_parts(parts, Body::from(body));
    if let Some(id) = root {
        set_root_header(&mut res, id);
    }
    res
}

fn set_root_header(res: &mut Response, id: &str) {
    if let Ok(value) = HeaderValue::from_str(id) {
        res.headers_mut().insert(ROOT_LAYOUT_HEADER, value);
    }
}

/// Put `<meta name="pp-root-layout">` into a finished document.
///
/// A layout that already carries one wrote it by hand; leave it, so an author
/// who wants to pin the identity themselves can.
fn mark(html: String, id: &str) -> String {
    if html.contains(ROOT_LAYOUT_META) {
        return html;
    }

    let tag = format!("<meta {ROOT_LAYOUT_META} content=\"{}\" />", escape(id));

    // Straight after the opening `<head>`, ahead of the stylesheet and the
    // runtime entry: the client only ever queries for it, but a document
    // reads better with its identity at the top.
    if let Some(at) = open_tag_end(&html, "<head") {
        return insert(html, at, &tag);
    }

    // A layout is free to render a document without a head. The parser keeps
    // a `<meta>` in the body where it finds it, and `querySelector` finds it
    // there, so the pair still matches.
    if let Some(at) = open_tag_end(&html, "<body") {
        return insert(html, at, &tag);
    }

    format!("{tag}{html}")
}

/// The index just past the `>` of the first `name` tag, if the document has
/// one.
fn open_tag_end(html: &str, name: &str) -> Option<usize> {
    // `<header>` starts with `<head`, so the character after the name has to
    // be one an open tag may continue with: whitespace, `>` or `/`.
    html.match_indices(name).find_map(|(at, _)| {
        let rest = &html[at + name.len()..];
        let next = rest.chars().next()?;
        if !(next.is_whitespace() || next == '>' || next == '/') {
            return None;
        }
        rest.find('>').map(|end| at + name.len() + end + 1)
    })
}

fn insert(html: String, at: usize, tag: &str) -> String {
    let mut out = String::with_capacity(html.len() + tag.len());
    out.push_str(&html[..at]);
    out.push_str(tag);
    out.push_str(&html[at..]);
    out
}

/// Attribute-safe. Ids are build-derived identifiers, so this only matters
/// for a `pp-component` name an author wrote by hand.
fn escape(id: &str) -> String {
    id.replace('&', "&amp;").replace('"', "&quot;")
}
