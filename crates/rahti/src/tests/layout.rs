//! Wrapping a page in its layouts.

use super::*;
use axum::body::to_bytes;
use axum::http::header;

/// A layout, in the shape `layout.rs` exports.
fn shell(children: Html) -> Html {
    Html::from_raw(format!("<html><body>{}</body></html>", children.as_str()))
}

fn page(markup: &str) -> Response {
    Html::from_raw(markup.to_string()).into_response()
}

async fn body_of(res: Response) -> String {
    let bytes = to_bytes(res.into_body(), 16 << 20).await.expect("a body");
    String::from_utf8(bytes.into()).expect("utf-8")
}

/// The body, with the dev client taken back out.
///
/// Every page passes through injection on its way out of a layout — that is
/// the one place it can be added without an app opting in — so what a layout
/// produced is what is left once the tag is removed. In a release build there
/// is nothing to remove.
async fn wrapped(res: Response) -> String {
    let body = body_of(res).await;
    match (
        body.find("<script id=\"rahti-dev\""),
        body.find("</script>"),
    ) {
        (Some(start), Some(end)) => {
            format!("{}{}", &body[..start], &body[end + "</script>".len()..])
        }
        _ => body,
    }
}

#[tokio::test]
async fn a_page_is_wrapped() {
    let out = apply_layout(shell, page("<p>hello</p>")).await;
    assert_eq!(wrapped(out).await, "<html><body><p>hello</p></body></html>");
}

/// The marker stays, so the next layout in the chain still recognises this as
/// a page — that is what makes `app/layout.rs` wrap `(marketing)/layout.rs`.
#[tokio::test]
async fn layouts_nest() {
    fn inner(children: Html) -> Html {
        Html::from_raw(format!("<main>{}</main>", children.as_str()))
    }

    let once = apply_layout(inner, page("<p>hello</p>")).await;
    assert!(
        once.extensions().get::<PageBody>().is_some(),
        "still a page"
    );

    let twice = apply_layout(shell, once).await;
    assert_eq!(
        wrapped(twice).await,
        "<html><body><main><p>hello</p></main></body></html>"
    );
}

/// Layout layers sit above every route in their subtree, including API
/// handlers and static files. Without the marker, `route.rs` returning JSON
/// would come back inside a document.
#[tokio::test]
async fn a_response_that_is_not_a_page_is_left_alone() {
    let json = (
        [(header::CONTENT_TYPE, "application/json")],
        "{\"ok\":true}",
    )
        .into_response();
    let out = apply_layout(shell, json).await;

    assert_eq!(body_of(out).await, "{\"ok\":true}");
}

/// The body grew, so a length measured before wrapping would be wrong.
#[tokio::test]
async fn the_stale_content_length_is_dropped() {
    let mut res = page("<p>hello</p>");
    res.headers_mut()
        .insert(header::CONTENT_LENGTH, "13".parse().unwrap());

    let out = apply_layout(shell, res).await;
    assert!(out.headers().get(header::CONTENT_LENGTH).is_none());
}

/// Everything else about the response is the page's: its status, and any
/// header it set.
#[tokio::test]
async fn the_pages_own_response_survives() {
    let mut res = page("<p>gone</p>");
    *res.status_mut() = StatusCode::NOT_FOUND;
    res.headers_mut()
        .insert("x-page", "customers".parse().unwrap());

    let out = apply_layout(shell, res).await;
    assert_eq!(out.status(), StatusCode::NOT_FOUND);
    assert_eq!(out.headers().get("x-page").unwrap(), "customers");
}

#[tokio::test]
async fn an_empty_page_is_still_wrapped() {
    let out = apply_layout(shell, page("")).await;
    assert_eq!(wrapped(out).await, "<html><body></body></html>");
}

/// The cap guards against a handler streaming something huge into a route a
/// layout covers — the wrap has to read the whole body to do its job.
#[tokio::test]
async fn a_body_too_large_to_wrap_is_reported() {
    let huge = "x".repeat(9 * 1024 * 1024);
    let mut res = Html::from_raw(huge).into_response();
    res.extensions_mut().insert(PageBody);

    let out = apply_layout(shell, res).await;
    assert_eq!(out.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body_of(out).await.contains("too large"));
}

/// Markup is text; a page body that is not UTF-8 cannot be a page.
#[tokio::test]
async fn a_body_that_is_not_text_is_reported() {
    let mut res = axum::body::Body::from(vec![0xff, 0xfe, 0xfd]).into_response();
    res.extensions_mut().insert(PageBody);

    let out = apply_layout(shell, res).await;
    assert_eq!(out.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body_of(out).await.contains("not valid UTF-8"));
}

// ------------------------------------------------------ multiple root layouts

/// A document with a head, in the shape a root layout renders one.
fn document(children: Html) -> Html {
    Html::from_raw(format!(
        "<!DOCTYPE html><html><head><title>x</title></head><body>{}</body></html>",
        children.as_str()
    ))
}

/// The pair the client compares: the header off the navigation `fetch`, and
/// the meta of the document already on screen.
#[tokio::test]
async fn a_root_layout_names_the_document_it_rendered() {
    let out = apply_root_layout(document, "layout_portal", page("<p>hello</p>")).await;

    assert_eq!(
        out.headers().get(ROOT_LAYOUT_HEADER).unwrap(),
        "layout_portal"
    );
    assert!(
        wrapped(out)
            .await
            .contains("<meta name=\"pp-root-layout\" content=\"layout_portal\" />"),
        "the document names its root layout"
    );
}

/// Ahead of the stylesheet and the runtime entry, which is where a document
/// reads best — and inside the head, where a `<meta>` belongs.
#[tokio::test]
async fn the_marker_goes_at_the_top_of_the_head() {
    let out = apply_root_layout(document, "layout_portal", page("")).await;
    let body = wrapped(out).await;

    let head = body.find("<head>").expect("a head");
    let meta = body.find("pp-root-layout").expect("the marker");
    let title = body.find("<title>").expect("a title");
    assert!(head < meta && meta < title, "{body}");
}

/// `<header>` starts with `<head`. Skipping that check would put the marker
/// inside an element the layout wrote.
#[tokio::test]
async fn a_document_without_a_head_is_marked_in_its_body() {
    fn headless(children: Html) -> Html {
        Html::from_raw(format!(
            "<html><body><header>{}</header></body></html>",
            children.as_str()
        ))
    }

    let body = wrapped(apply_root_layout(headless, "layout_bare", page("x")).await).await;
    assert!(
        body.starts_with("<html><body><meta name=\"pp-root-layout\" content=\"layout_bare\" />"),
        "{body}"
    );
}

/// An author who writes the marker by hand keeps it: two would leave the
/// client to pick, and the one in the source is the deliberate one.
#[tokio::test]
async fn a_hand_written_marker_is_left_alone() {
    fn pinned(children: Html) -> Html {
        Html::from_raw(format!(
            "<html><head><meta name=\"pp-root-layout\" content=\"mine\" /></head><body>{}</body></html>",
            children.as_str()
        ))
    }

    let out = apply_root_layout(pinned, "layout_generated", page("")).await;
    // The header still reports what the build derived. An author who pins the
    // markup is taking on keeping the two the same.
    assert_eq!(
        out.headers().get(ROOT_LAYOUT_HEADER).unwrap(),
        "layout_generated"
    );

    let body = wrapped(out).await;
    assert_eq!(body.matches("pp-root-layout").count(), 1, "{body}");
    assert!(body.contains("content=\"mine\""), "{body}");
}

/// The layout layer sits above every route in its subtree. A `route.rs`
/// answering JSON is not a document, and naming one would be a lie.
#[tokio::test]
async fn a_response_that_is_not_a_page_is_never_marked() {
    let json = (
        [(header::CONTENT_TYPE, "application/json")],
        "{\"ok\":true}",
    )
        .into_response();
    let out = apply_root_layout(document, "layout_portal", json).await;

    assert!(out.headers().get(ROOT_LAYOUT_HEADER).is_none());
    assert_eq!(body_of(out).await, "{\"ok\":true}");
}

/// The 404 has no layer to be marked by: it is rendered below the static-file
/// fallback, with its layouts called directly.
#[tokio::test]
async fn a_document_assembled_by_hand_can_be_marked() {
    let out = root_document(
        "layout_root",
        document(Html::from_raw("<p>gone</p>".into())),
    );

    assert_eq!(
        out.headers().get(ROOT_LAYOUT_HEADER).unwrap(),
        "layout_root"
    );
    assert!(body_of(out).await.contains("content=\"layout_root\""));
}
