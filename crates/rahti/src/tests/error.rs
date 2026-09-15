//! Failures, and the boundary that turns one into a page.

use super::*;
use axum::body::to_bytes;
use axum::http::header;

/// A boundary that renders everything it was told, so the tests can see it.
fn boundary(info: ErrorInfo) -> Html {
    Html::from_raw(format!(
        "<section>{} {} {}</section>",
        info.code(),
        info.reason(),
        info.message()
    ))
}

async fn body_of(res: Response) -> String {
    let bytes = to_bytes(res.into_body(), 1 << 20).await.expect("a body");
    String::from_utf8(bytes.into()).expect("utf-8")
}

// ------------------------------------------------------------------- error

#[test]
fn an_error_carries_a_status_and_a_message() {
    let err = Error::new(StatusCode::NOT_FOUND, "no such customer");
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    assert_eq!(err.message(), "no such customer");
}

/// The usual case for `.map_err(Error::internal)?`.
#[test]
fn internal_is_a_500() {
    assert_eq!(
        Error::internal("boom").status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
}

#[test]
fn production_hides_unexpected_server_diagnostics() {
    for err in [
        Error::internal("database password appeared here"),
        Error::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "database password appeared here",
        ),
    ] {
        assert_eq!(err.client_message(false), UNEXPECTED_ERROR);
        assert_eq!(err.client_message(true), "database password appeared here");
    }
}

#[test]
fn expected_client_errors_and_explicit_public_server_errors_keep_their_message() {
    let missing = Error::new(StatusCode::NOT_FOUND, "no such customer");
    assert_eq!(missing.client_message(false), "no such customer");

    let maintenance = Error::public(StatusCode::SERVICE_UNAVAILABLE, "Back shortly");
    assert_eq!(maintenance.client_message(false), "Back shortly");

    let mapped = Error::internal("upstream connection refused")
        .with_public_message("The service is temporarily unavailable");
    assert_eq!(
        mapped.client_message(false),
        "The service is temporarily unavailable"
    );
    assert_eq!(mapped.message(), "upstream connection refused");
}

#[test]
fn private_server_errors_do_not_expose_structured_details() {
    let err = Error::internal("database failed")
        .with_details(serde_json::json!({ "connection": "secret" }));
    assert!(err.client_details(false).is_none());
    assert!(err.client_details(true).is_some());
}

/// `From<E: std::error::Error>` is what makes `?` work in a page, and is the
/// reason `Error` deliberately does not implement `Error` itself.
#[test]
fn any_ordinary_error_converts() {
    let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
    let err: Error = io.into();
    assert_eq!(err.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(err.message().contains("missing"));
}

#[test]
fn a_failed_page_responds_with_its_status_and_a_source() {
    let res = Error::new(StatusCode::BAD_GATEWAY, "upstream said no").into_response();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    let source = res
        .extensions()
        .get::<ErrorSource>()
        .expect("the message is carried");
    assert_eq!(source.0, "upstream said no");
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(
        res.headers()[header::CONTENT_TYPE],
        "application/problem+json"
    );
}

#[tokio::test]
async fn an_unbounded_error_is_a_problem_details_response() {
    let res = Error::new(StatusCode::CONFLICT, "already exists").into_response();
    let body = body_of(res).await;
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(body["type"], "about:blank");
    assert_eq!(body["title"], "Conflict");
    assert_eq!(body["status"], 409);
    assert_eq!(body["detail"], "already exists");
}

// ------------------------------------------------------------------ panics

#[test]
fn a_panic_becomes_a_failure_the_boundary_can_render() {
    let res = panic_response(Box::new("it broke"));
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(res.extensions().get::<ErrorSource>().unwrap().0, "it broke");
}

#[test]
fn a_formatted_panic_message_is_kept() {
    let res = panic_response(Box::new(String::from("index 4 out of range")));
    assert_eq!(
        res.extensions().get::<ErrorSource>().unwrap().0,
        "index 4 out of range"
    );
}

/// `panic!(SomeStruct)` carries no text to report.
#[test]
fn a_panic_with_no_message_still_answers() {
    let res = panic_response(Box::new(42u8));
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(res.extensions().get::<ErrorSource>().unwrap().0, "panicked");
}

// -------------------------------------------------------------- boundaries

/// The layer sits above every route in its subtree, so it has to be inert on
/// every successful request.
#[tokio::test]
async fn a_successful_page_passes_through() {
    let res = Html::from_raw("<p>fine</p>".to_string()).into_response();
    let out = apply_error(boundary, res).await;

    assert_eq!(out.status(), StatusCode::OK);
    assert_eq!(body_of(out).await, "<p>fine</p>");
}

#[tokio::test]
async fn a_failed_page_is_replaced_by_the_boundary() {
    let res = Error::new(StatusCode::NOT_FOUND, "no such customer").into_response();
    let out = apply_error(boundary, res).await;

    assert_eq!(out.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_of(out).await,
        "<section>404 Not Found no such customer</section>"
    );
}

/// A handler that returned a bare 5xx never went through `Error`, so there is
/// nothing to report beyond the status itself — but it is still a failure.
#[tokio::test]
async fn a_bare_server_error_is_caught_too() {
    let res = (StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    let out = apply_error(boundary, res).await;

    assert_eq!(
        body_of(out).await,
        "<section>500 Internal Server Error Internal Server Error</section>"
    );
}

/// A 404 a handler chose to return is an answer, not a failure. Only the
/// server's own errors, and pages that failed, reach the boundary.
#[tokio::test]
async fn a_client_error_without_a_source_passes_through() {
    let res = (StatusCode::NOT_FOUND, "nothing here").into_response();
    let out = apply_error(boundary, res).await;

    assert_eq!(body_of(out).await, "nothing here");
}

/// What comes back is a page: outer layouts wrap it exactly as they would
/// have wrapped the page that failed.
#[tokio::test]
async fn the_rendered_error_is_a_page() {
    let res = Error::internal("boom").into_response();
    let out = apply_error(boundary, res).await;

    assert!(
        out.extensions().get::<PageBody>().is_some(),
        "layouts still wrap it"
    );
    assert_eq!(
        out.headers().get(header::CONTENT_TYPE).unwrap(),
        "text/html; charset=utf-8"
    );
}

/// The body changed, and a boundary further out must not catch this again.
#[tokio::test]
async fn the_failure_is_not_carried_any_further() {
    let res = Error::internal("boom").into_response();
    let out = apply_error(boundary, res).await;

    assert!(out.extensions().get::<ErrorSource>().is_none());
    assert!(out.headers().get(header::CONTENT_LENGTH).is_none());
}

/// The nearest boundary wins, and the one above it sees a rendered page.
#[tokio::test]
async fn a_second_boundary_leaves_the_first_ones_work_alone() {
    fn outer(_: ErrorInfo) -> Html {
        Html::from_raw("<p>outer</p>".to_string())
    }

    let res = Error::new(StatusCode::NOT_FOUND, "gone").into_response();
    let once = apply_error(boundary, res).await;
    let twice = apply_error(outer, once).await;

    assert_eq!(
        body_of(twice).await,
        "<section>404 Not Found gone</section>"
    );
}

// ------------------------------------------------------------- error info

/// `reason` is the status text — safe to show anyone — while `message` is
/// whatever the page produced, and showing it is the boundary's decision.
#[tokio::test]
async fn a_boundary_is_told_the_status_and_the_message() {
    fn capture(info: ErrorInfo) -> Html {
        assert_eq!(info.status(), StatusCode::IM_A_TEAPOT);
        assert_eq!(info.code(), 418);
        assert_eq!(info.reason(), "I'm a teapot");
        assert_eq!(info.message(), "no coffee");
        Html::default()
    }

    let res = Error::new(StatusCode::IM_A_TEAPOT, "no coffee").into_response();
    apply_error(capture, res).await;
}
