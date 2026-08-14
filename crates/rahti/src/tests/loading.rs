//! The container the client runtime reads its loading markup out of.
//!
//! The id, the tag and the attribute are all named by the shipped bundle, so
//! these tests pin the exact shape rather than something equivalent: a `<span>`
//! instead of a `<div>`, or a renamed attribute, compiles and ships and then
//! silently never matches.

use super::*;

fn spinner() -> Html {
    Html::from_raw("<p>loading</p>".to_string())
}

fn skeleton() -> Html {
    Html::from_raw("<p>rows</p>".to_string())
}

#[test]
fn the_container_carries_the_id_the_runtime_looks_up() {
    let out = container(&[("/", spinner as fn() -> Html)]);
    assert!(
        out.starts_with(&format!("<div id=\"{REGION_ID}\"")),
        "{out}"
    );
}

#[test]
fn the_container_is_hidden() {
    let out = container(&[("/", spinner as fn() -> Html)]);
    assert!(out.contains("hidden"), "{out}");
    assert!(out.contains("display:none"), "{out}");
}

#[test]
fn each_region_is_a_div_tagged_with_its_url() {
    let out = container(&[
        ("/", spinner as fn() -> Html),
        ("/customers", skeleton as fn() -> Html),
    ]);
    assert!(
        out.contains("<div pp-loading-url=\"/\"><p>loading</p></div>"),
        "{out}"
    );
    assert!(
        out.contains("<div pp-loading-url=\"/customers\"><p>rows</p></div>"),
        "{out}"
    );
}

#[test]
fn no_regions_means_no_container() {
    // `install` returns before building anything, so an app without a
    // `loading.rs` never has the element — which is what keeps view
    // transitions available to it.
    install(&[]);
    assert!(markup().is_none());
}

#[test]
fn the_container_goes_last_in_the_body() {
    let out = place(
        "<html><body><p>page</p></body></html>".to_string(),
        "<div/>",
    );
    assert_eq!(out, "<html><body><p>page</p><div/></body></html>");
}

#[test]
fn a_document_without_a_body_gets_it_appended() {
    let out = place("<p>page</p>".to_string(), "<div/>");
    assert_eq!(out, "<p>page</p><div/>");
}

#[test]
fn the_innermost_body_close_is_the_one_used() {
    // `</body>` can appear inside the page as escaped text or in a script;
    // the document's own close is the last one.
    let out = place(
        "<html><body><pre>&lt;/body&gt;</pre></body></html>".to_string(),
        "<div/>",
    );
    assert!(out.ends_with("<div/></body></html>"), "{out}");
}

#[test]
fn a_url_with_a_quote_in_it_cannot_break_out_of_the_attribute() {
    assert_eq!(escape("/a\"b"), "/a&quot;b");
}

#[test]
fn without_an_installed_container_a_document_is_untouched() {
    let html = "<html><body><p>page</p></body></html>".to_string();
    assert_eq!(inject(html.clone()), html);
}
