//! A request in, a page out.
//!
//! The unit tests either side of this one cover the pieces; this covers them
//! wired together the way `src/routes.rs` wires them. The handlers below are
//! written the way a `page.rs` is written, the layers are the ones codegen
//! emits, and the markup goes through the real `html!` macro — so a change
//! that breaks the contract between the macro, the runtime and the generated
//! router fails here.

// `html!` expands to `crate::rahti::` paths, so an app has to make the
// runtime reachable there. `src/main.rs` says exactly this line, and a test
// crate is an app like any other.
pub use rahti;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::middleware::map_response;
use axum::response::Response;
use axum::routing::get;
use http_body_util::BodyExt;
use rahti::{Error, ErrorInfo, Html, html};
use tower::ServiceExt;

// ------------------------------------------------------------ the app

/// The document shell, as `app/layout.rs` writes it.
fn layout(children: Html) -> Html {
    html! {
        <html lang="en">
            <body class="app">
                <slot />
            </body>
        </html>
    }
}

/// A section wrapper, as a nested `layout.rs` writes it.
fn section_layout(children: Html) -> Html {
    html! {
        <main class="section">
            <slot />
        </main>
    }
}

/// The nearest boundary, as `error.rs` writes it.
fn boundary(error: ErrorInfo) -> Html {
    html! {
        <section class="error">
            <h1>@{error.reason()}</h1>
            <p>@{error.message()}</p>
        </section>
    }
}

async fn users() -> Html {
    let name = "Ada & <Lovelace>";
    html! {
        <section class="users">
            <h1>Users</h1>
            <p title=@{name}>@{name}</p>
            <p>Seen {count} times</p>
        </section>
    }
}

async fn missing() -> rahti::Result {
    Err(Error::new(StatusCode::NOT_FOUND, "no such customer"))
}

async fn boom() -> Html {
    panic!("the database went away")
}

/// A `route.rs` handler: JSON, and no page shell around it.
async fn health() -> impl axum::response::IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json")],
        "{\"ok\":true}",
    )
}

/// The router codegen would write for the app above.
fn app() -> Router {
    // Pages under the root layout.
    let pages = Router::new()
        .route("/users", get(users))
        .layer(map_response(|res| rahti::apply_layout(layout, res)));

    // Pages under a nested layout and an error boundary at the root. The
    // panic layer sits closest to the handler; the nested layout belongs to
    // the subtree that failed, so it goes below the boundary.
    let guarded = Router::new()
        .route("/customers", get(missing))
        .route("/boom", get(boom))
        .layer(tower_http::catch_panic::CatchPanicLayer::custom(
            rahti::panic_response,
        ))
        .layer(map_response(|res| rahti::apply_layout(section_layout, res)))
        .layer(map_response(|res| rahti::apply_error(boundary, res)))
        .layer(map_response(|res| rahti::apply_layout(layout, res)));

    // A route handler inherits neither.
    let api = Router::new().route("/api/health", get(health));

    Router::new().merge(pages).merge(guarded).merge(api)
}

// ------------------------------------------------------------ driving it

async fn get_url(url: &str) -> (StatusCode, String, Response<Body>) {
    let res = app()
        .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
        .await
        .expect("the router answered");

    let status = res.status();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.expect("a body").to_bytes();
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");

    (status, text, Response::from_parts(parts, Body::empty()))
}

// ---------------------------------------------------------------- pages

#[tokio::test]
async fn a_page_is_rendered_inside_its_layout() {
    let (status, body, res) = get_url("/users").await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body.starts_with("<html lang=\"en\"><body class=\"app\">"),
        "{body}"
    );
    assert!(
        body.contains("<section class=\"users\"><h1>Users</h1>"),
        "{body}"
    );
    assert!(body.trim_end().ends_with("</body></html>"), "{body}");
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );
}

/// Rust values are escaped on the way in, in both positions.
#[tokio::test]
async fn interpolated_values_are_escaped() {
    let (_, body, _) = get_url("/users").await;

    assert!(
        body.contains("<p title=\"Ada &amp; &lt;Lovelace&gt;\">"),
        "{body}"
    );
    assert!(body.contains(">Ada &amp; &lt;Lovelace&gt;</p>"), "{body}");
    assert!(!body.contains("<Lovelace>"), "{body}");
}

/// A `{…}` binding belongs to the client runtime: it is still in the HTML,
/// as written, for the runtime to read back off the page.
#[tokio::test]
async fn a_client_binding_reaches_the_browser_intact() {
    let (_, body, _) = get_url("/users").await;
    assert!(body.contains("Seen {count} times"), "{body}");
}

// --------------------------------------------------------------- layers

/// Without the page marker, an API handler would come back wrapped in a
/// document.
#[tokio::test]
async fn an_api_route_is_not_wrapped() {
    let (status, body, res) = get_url("/api/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "{\"ok\":true}");
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
}

#[tokio::test]
async fn an_unrouted_url_is_not_answered() {
    let (status, _, _) = get_url("/nothing-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------- failures

/// A page returning `Err` is replaced by its boundary, keeping the status,
/// and the layouts above the boundary still wrap the result.
#[tokio::test]
async fn a_failed_page_is_replaced_by_its_boundary() {
    let (status, body, _) = get_url("/customers").await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.starts_with("<html lang=\"en\"><body class=\"app\">"),
        "{body}"
    );
    assert!(
        body.contains("<section class=\"error\"><h1>Not Found</h1>"),
        "{body}"
    );
    assert!(body.contains("<p>no such customer</p>"), "{body}");
}

/// The layout below the boundary belongs to the subtree that failed, so the
/// error UI replaces it rather than sitting inside it.
#[tokio::test]
async fn the_layouts_below_the_boundary_are_skipped() {
    let (_, body, _) = get_url("/customers").await;
    assert!(!body.contains("<main class=\"section\">"), "{body}");
}

/// A panicking handler becomes a rendered page rather than a dropped
/// connection.
#[tokio::test]
async fn a_panicking_page_is_caught_and_rendered() {
    let (status, body, _) = get_url("/boom").await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(body.contains("<h1>Internal Server Error</h1>"), "{body}");
    assert!(body.contains("the database went away"), "{body}");
    assert!(
        body.starts_with("<html lang=\"en\">"),
        "the boundary is still wrapped: {body}"
    );
}

// ----------------------------------------------------------- live reload

/// Injection happens on every layout in the chain, and the marker is what
/// keeps the second pass from stacking another copy.
#[tokio::test]
async fn the_dev_client_is_injected_once() {
    let (_, body, _) = get_url("/customers").await;

    let copies = body.matches("/__rahti/dev.js").count();
    if cfg!(debug_assertions) {
        assert_eq!(copies, 1, "one client, however many layouts: {body}");
        assert!(
            body.contains("</script></body></html>"),
            "it goes last: {body}"
        );
    } else {
        assert_eq!(copies, 0, "nothing is injected in a release build");
    }
}
