use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::IntoResponse;
use axum::routing::get;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;

static TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn test_state(policy: Observability) -> ObservabilityState {
    ObservabilityState {
        policy: Observability {
            service_name: "test-service".into(),
            ..policy
        },
        public_dir: PathBuf::from("a-public-directory-that-does-not-exist"),
    }
}

fn app(policy: Observability) -> Router {
    let state = test_state(policy);
    Router::new()
        .route(
            "/",
            get(|| async {
                let scoped = request_id().expect("the task-local request id");
                format!("{}:{}", scoped, metrics().active_requests)
            }),
        )
        .layer(from_fn_with_state(state, middleware))
}

#[tokio::test]
async fn a_valid_incoming_id_reaches_the_handler_and_response() {
    let _serial = TESTS.lock().await;
    reset_metrics();
    let response = app(Observability::default().enabled(true).request_logs(false))
        .oneshot(
            Request::get("/")
                .header(REQUEST_ID_HEADER, "edge-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.headers()[REQUEST_ID_HEADER], "edge-123");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"edge-123:1");
    assert_eq!(metrics().requests, 1);
    assert_eq!(metrics().active_requests, 0);
}

#[tokio::test]
async fn invalid_or_untrusted_ids_are_replaced() {
    let _serial = TESTS.lock().await;
    for policy in [
        Observability::default().enabled(true).request_logs(false),
        Observability::default()
            .enabled(true)
            .request_logs(false)
            .accept_request_ids(false),
    ] {
        let response = app(policy)
            .oneshot(
                Request::get("/")
                    .header(REQUEST_ID_HEADER, "bad id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let id = response.headers()[REQUEST_ID_HEADER].to_str().unwrap();
        assert_ne!(id, "bad id");
        assert!(valid_request_id(id), "{id}");
    }
}

#[tokio::test]
async fn metrics_classify_status_and_slow_responses_without_route_labels() {
    let _serial = TESTS.lock().await;
    reset_metrics();
    let state = test_state(
        Observability::default()
            .enabled(true)
            .request_logs(false)
            .slow_request(Duration::from_nanos(1)),
    );
    let app = Router::new()
        .route("/bad", get(|| async { StatusCode::UNPROCESSABLE_ENTITY }))
        .route(
            "/failed",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .layer(from_fn_with_state(state, middleware));

    for path in ["/bad", "/failed"] {
        app.clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
    }
    let metrics = metrics();
    assert_eq!(metrics.requests, 2);
    assert_eq!(metrics.client_errors, 1);
    assert_eq!(metrics.server_errors, 1);
    assert_eq!(metrics.slow_requests, 2);
    assert!(metrics.duration_micros <= metrics.max_duration_micros.saturating_mul(2));
}

#[tokio::test]
async fn health_is_live_and_readiness_reports_failed_and_timed_out_checks() {
    let _serial = TESTS.lock().await;
    let policy = Observability::default()
        .enabled(true)
        .request_logs(false)
        .readiness_timeout(Duration::from_millis(5))
        .with_readiness("database", || async { Err::<(), _>("connection refused") })
        .with_readiness("queue", || async {
            tokio::time::sleep(Duration::from_millis(25)).await;
            Ok::<(), String>(())
        });
    let state = test_state(policy);
    let app = routes(state);

    let health = app
        .clone()
        .oneshot(Request::get(HEALTH_PATH).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
    assert_eq!(health.headers()["cache-control"], "no-store");

    let ready = app
        .oneshot(Request::get(READY_PATH).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = ready.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body);
    assert!(body.contains(r#""database","status":"failed""#), "{body}");
    assert!(body.contains(r#""queue","status":"timeout""#), "{body}");
    assert!(!body.contains("connection refused"), "{body}");
}

#[test]
#[should_panic(expected = "duplicate readiness check")]
fn readiness_names_are_unique() {
    Observability::default()
        .with_readiness("database", || async { Ok::<(), String>(()) })
        .with_readiness("database", || async { Ok::<(), String>(()) });
}

#[test]
fn event_text_is_utf8_safe_when_truncated() {
    let text = "é".repeat(MAX_EVENT_TEXT);
    let truncated = limit(&text);
    assert!(truncated.ends_with('…'));
    assert!(truncated.is_char_boundary(truncated.len()));
}
