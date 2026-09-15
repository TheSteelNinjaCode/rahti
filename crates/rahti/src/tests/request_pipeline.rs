use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::{RequestPipeline, configure, middleware, state};

fn app(pipeline: RequestPipeline) -> Router {
    Router::new()
        .route("/", get(|| async { "handler" }))
        .layer(from_fn_with_state(pipeline, middleware))
}

#[tokio::test]
async fn steps_wrap_the_handler_in_registration_order() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let outer_events = events.clone();
    let inner_events = events.clone();

    let pipeline = RequestPipeline::new()
        .with(move |request, next| {
            let events = outer_events.clone();
            async move {
                events.lock().unwrap().push("outer request");
                let response = next.run(request).await;
                events.lock().unwrap().push("outer response");
                response
            }
        })
        .with(move |request, next| {
            let events = inner_events.clone();
            async move {
                events.lock().unwrap().push("inner request");
                let response = next.run(request).await;
                events.lock().unwrap().push("inner response");
                response
            }
        });

    let response = app(pipeline)
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        *events.lock().unwrap(),
        [
            "outer request",
            "inner request",
            "inner response",
            "outer response"
        ]
    );
}

#[tokio::test]
async fn a_step_can_short_circuit_before_the_handler() {
    let called = Arc::new(AtomicBool::new(false));
    let handler_called = called.clone();
    let pipeline = RequestPipeline::new()
        .with(|_request, _next| async { (StatusCode::IM_A_TEAPOT, "stopped").into_response() });
    let app = Router::new()
        .route(
            "/",
            get(move || {
                let called = handler_called.clone();
                async move {
                    called.store(true, Ordering::SeqCst);
                    "handler"
                }
            }),
        )
        .layer(from_fn_with_state(pipeline, middleware));

    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert!(!called.load(Ordering::SeqCst));
}

#[tokio::test]
async fn request_extensions_flow_in_and_response_headers_flow_out() {
    let pipeline = RequestPipeline::new().with(|mut request, next| async move {
        request.extensions_mut().insert(42_u32);
        let mut response = next.run(request).await;
        response.headers_mut().insert(
            "x-pipeline",
            axum::http::HeaderValue::from_static("observed"),
        );
        response
    });
    let app = Router::new()
        .route(
            "/",
            get(|request: axum::extract::Request| async move {
                request
                    .extensions()
                    .get::<u32>()
                    .copied()
                    .unwrap_or_default()
                    .to_string()
            }),
        )
        .layer(from_fn_with_state(pipeline, middleware));

    let response = app
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.headers()["x-pipeline"], "observed");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"42");
}

#[tokio::test]
async fn an_empty_pipeline_is_transparent() {
    let response: Response = app(RequestPipeline::new())
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_middleware_panic_becomes_the_callers_500_shape() {
    let pipeline = RequestPipeline::new().with(|_request, _next| async {
        panic!("middleware failed");
    });

    let page = app(pipeline.clone())
        .oneshot(Request::get("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(page.headers()["content-type"], "application/problem+json");

    let rpc = app(pipeline)
        .oneshot(
            Request::get("/")
                .header("x-pp-function", "work")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(rpc.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(rpc.headers()["content-type"], "application/json");
}

#[test]
fn routers_take_an_immutable_snapshot_of_configuration() {
    configure(RequestPipeline::new().with(|request, next| async move { next.run(request).await }));
    let first_router = state();
    configure(RequestPipeline::new());

    assert_eq!(first_router.len(), 1);
    assert!(state().is_empty());
}
