//! Loading regions: what the reader sees while the next page is fetched.
//!
//! A page is rendered on the server and arrives complete, so there is nothing
//! to suspend on. The wait a `loading.rs` covers is the *navigation* — the
//! moment between a link being clicked and the response coming back. That
//! wait belongs to the browser, so this module's whole job is to put the
//! markup somewhere the client runtime can find it.
//!
//! The bundled PulsePoint runtime looks, at the start of every SPA
//! navigation, for one container element by id. Inside it, it picks the
//! `<div>` whose `pp-loading-url` matches the current path, walking up one
//! segment at a time until something matches — so a `loading.rs` at a section
//! root covers everything below it, and a root one covers the app. What it
//! finds is copied into the region the layout marked with
//! `pp-loading-content="true"`, and stays there until the new document
//! replaces the body.
//!
//! [`install`] is called once from the generated `router()`; the container is
//! injected into every root document by `layout`.

use std::sync::OnceLock;

use super::html::Html;

#[cfg(test)]
#[path = "tests/loading.rs"]
mod tests;

/// The id the client runtime looks the container up by.
///
/// Not a name anyone chose: it is the identifier the shipped bundle queries
/// for, so it is fixed until the runtime changes. Rahti writes it; an
/// application never does.
pub const REGION_ID: &str = "loading-file-1B87E";

/// The attribute that says which path a region answers for.
pub const URL_ATTR: &str = "pp-loading-url";

/// Built once, from the generated router, and read on every root document.
static REGIONS: OnceLock<String> = OnceLock::new();

/// Record the app's loading regions.
///
/// Called by generated code with one pair per `loading.rs`: the URL prefix the
/// file sits at, and its `loading` function. The markup is rendered here
/// rather than per request because a loading region takes no arguments and so
/// cannot differ between two requests.
///
/// An app with no `loading.rs` never calls this, and nothing is injected.
pub fn install(regions: &[(&str, fn() -> Html)]) {
    if regions.is_empty() {
        return;
    }
    // A second call is the same router being built twice — in a test, or by a
    // process that assembles more than one. The markup is identical, so the
    // first one standing is correct.
    let _ = REGIONS.set(container(regions));
}

/// The container, as it will be written into the document.
///
/// Split out from [`install`] so it can be tested without touching the
/// process-wide slot: a test that installed would leak into every other test
/// in the binary, since the whole point of the slot is that it is set once.
fn container(regions: &[(&str, fn() -> Html)]) -> String {
    let mut out = format!("<div id=\"{REGION_ID}\" hidden style=\"display:none\">");
    for (url, render) in regions {
        // A `<div>` specifically: the runtime's selector names the tag.
        out.push_str(&format!("<div {URL_ATTR}=\"{}\">", escape(url)));
        out.push_str(render().as_str());
        out.push_str("</div>");
    }
    out.push_str("</div>");
    out
}

/// The installed container, if this app has one.
pub(crate) fn markup() -> Option<&'static str> {
    REGIONS.get().map(String::as_str)
}

/// Put the container into a finished document.
pub(crate) fn inject(html: String) -> String {
    match markup() {
        Some(block) => place(html, block),
        None => html,
    }
}

/// Last thing in the `<body>`, so the regions are inert markup below the page
/// rather than something the reader can reach by tabbing through it.
///
/// A layout is free to render a document without a `<body>`; the parser puts
/// the container in one either way, and `getElementById` finds it there.
fn place(html: String, block: &str) -> String {
    match html.rfind("</body>") {
        Some(at) => {
            let mut out = String::with_capacity(html.len() + block.len());
            out.push_str(&html[..at]);
            out.push_str(block);
            out.push_str(&html[at..]);
            out
        }
        None => format!("{html}{block}"),
    }
}

/// Attribute-safe. URLs come from directory names, so this only matters for a
/// segment with punctuation in it.
fn escape(url: &str) -> String {
    url.replace('&', "&amp;").replace('"', "&quot;")
}
