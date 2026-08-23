//! The rpcs found at link time: what may answer, and what may not.

use super::*;

fn entry(name: &'static str, source: &'static str) -> RpcEntry {
    RpcEntry {
        name,
        source,
        handler: |_req| Box::pin(async { crate::rpc_unknown("unused") }),
    }
}

// ---------------------------------------------------------------- duplicates

/// Two files can each declare `submit` without either author doing anything
/// wrong. Whichever the linker placed first would answer for both, silently,
/// and possibly differently in the next build.
#[test]
fn two_rpcs_of_one_name_are_found() {
    let a = entry("submit", "src/components/form.rs:12");
    let b = entry("submit", "src/components/contact.rs:30");
    let c = entry("bump", "src/components/counter.rs:51");

    let found = duplicate(&[&a, &c, &b]).expect("the clash");
    assert_eq!(found.0.name, "submit");
    assert_eq!(found.1.name, "submit");
    assert_eq!(found.0.source, "src/components/form.rs:12");
    assert_eq!(found.1.source, "src/components/contact.rs:30");
}

#[test]
fn distinct_names_are_not_a_clash() {
    let a = entry("bump", "a.rs:1");
    let b = entry("like", "b.rs:1");
    let c = entry("greet", "c.rs:1");
    assert!(duplicate(&[&a, &b, &c]).is_none());
}

#[test]
fn an_empty_registry_is_fine() {
    assert!(duplicate(&[]).is_none());
}

// ----------------------------------------------------------------- reserved

/// A page's rpcs are in the registry too, because `#[rpc]` cannot tell where
/// it was written. They must not be answerable here: a page's rpc belongs to
/// that page's URL, and every page falls through to this dispatcher.
#[tokio::test]
async fn a_page_rpc_is_not_answered_by_the_fallback() {
    let req = axum::extract::Request::new(axum::body::Body::empty());
    let res = dispatch("describe", &["describe"], req).await;

    assert_eq!(
        res.status(),
        axum::http::StatusCode::NOT_FOUND,
        "a reserved name must not be dispatched from the registry"
    );
}

#[tokio::test]
async fn an_unknown_name_is_unknown() {
    let req = axum::extract::Request::new(axum::body::Body::empty());
    let res = dispatch("nothing-declares-this", &[], req).await;
    assert_eq!(res.status(), axum::http::StatusCode::NOT_FOUND);
}
