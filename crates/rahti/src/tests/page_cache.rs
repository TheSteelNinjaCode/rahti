use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{Request, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tower::ServiceExt;

use super::*;
use crate::cache::MemoryStore;
use crate::html::Html;

#[derive(Clone)]
struct TestState {
    policy: PageCache,
    store: Arc<dyn CacheStore>,
}

async fn forced_cache(
    State(state): State<TestState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    serve(state.policy, request, next, state.store).await
}

fn app(
    policy: PageCache,
    store: Arc<dyn CacheStore>,
    calls: Arc<AtomicUsize>,
    set_cookie: bool,
) -> Router {
    let state = TestState { policy, store };
    Router::new()
        .route(
            "/",
            get(move || {
                let calls = calls.clone();
                async move {
                    let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    let mut response = Html::from_raw(format!("<p>{call}</p>")).into_response();
                    if set_cookie {
                        response
                            .headers_mut()
                            .insert(header::SET_COOKIE, HeaderValue::from_static("theme=dark"));
                    }
                    response
                }
            }),
        )
        .layer(from_fn_with_state(state, forced_cache))
}

async fn request(app: &Router, uri: &str, method: Method) -> (Response, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, 1024).await.unwrap();
    (
        Response::from_parts(parts, Body::empty()),
        String::from_utf8(body.to_vec()).unwrap(),
    )
}

fn cache_status(response: &Response) -> &str {
    response
        .headers()
        .get(CACHE_HEADER)
        .unwrap()
        .to_str()
        .unwrap()
}

#[tokio::test]
async fn a_complete_page_is_reused_after_the_first_render() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(16));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(PageCache::seconds(60), store, calls.clone(), false);

    let (first, first_body) = request(&app, "/", Method::GET).await;
    let (second, second_body) = request(&app, "/", Method::GET).await;
    assert_eq!(cache_status(&first), "MISS");
    assert_eq!(cache_status(&second), "HIT");
    assert_eq!(first_body, "<p>1</p>");
    assert_eq!(second_body, first_body);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn query_variants_do_not_share_a_cached_response() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(16));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(PageCache::seconds(60), store, calls.clone(), false);

    assert_eq!(
        cache_status(&request(&app, "/?page=1", Method::GET).await.0),
        "MISS"
    );
    assert_eq!(
        cache_status(&request(&app, "/?page=2", Method::GET).await.0),
        "MISS"
    );
    assert_eq!(
        cache_status(&request(&app, "/?page=1", Method::GET).await.0),
        "HIT"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn responses_that_set_a_cookie_are_never_stored() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(16));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(PageCache::seconds(60), store, calls.clone(), true);

    assert_eq!(
        cache_status(&request(&app, "/", Method::GET).await.0),
        "BYPASS"
    );
    assert_eq!(
        cache_status(&request(&app, "/", Method::GET).await.0),
        "BYPASS"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_request_cache_bypass_renders_without_storing() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(16));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(PageCache::seconds(60), store, calls.clone(), false);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cache_status(&response), "BYPASS");
    assert_eq!(
        cache_status(&request(&app, "/", Method::GET).await.0),
        "MISS"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_corrupt_page_is_removed_and_replaced() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(16));
    let calls = Arc::new(AtomicUsize::new(0));
    let policy = PageCache::seconds(60);
    let key = page_key(store.as_ref(), &policy, "", "/", "/")
        .await
        .unwrap();
    let corrupt = StoredPage {
        status: 200,
        headers: Vec::new(),
        body: "not base64".to_string(),
    };
    store
        .put(
            &key,
            serde_json::to_vec(&corrupt).unwrap(),
            Expiration::After(Duration::from_secs(60)),
        )
        .await
        .unwrap();
    let app = app(policy, store, calls.clone(), false);

    let (first, first_body) = request(&app, "/", Method::GET).await;
    let (second, second_body) = request(&app, "/", Method::GET).await;
    assert_eq!(cache_status(&first), "MISS");
    assert_eq!(cache_status(&second), "HIT");
    assert_eq!(first_body, "<p>1</p>");
    assert_eq!(second_body, first_body);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn path_and_tag_revalidation_change_the_generation_key() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(32));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(
        PageCache::seconds(60).tags(["reports"]),
        store.clone(),
        calls.clone(),
        false,
    );

    request(&app, "/?year=2026", Method::GET).await;
    assert_eq!(
        cache_status(&request(&app, "/?year=2026", Method::GET).await.0),
        "HIT"
    );
    revalidate(store.as_ref(), PATH_GENERATION_PREFIX, "/", valid_path)
        .await
        .unwrap();
    assert_eq!(
        cache_status(&request(&app, "/?year=2026", Method::GET).await.0),
        "MISS"
    );

    assert_eq!(
        cache_status(&request(&app, "/?year=2026", Method::GET).await.0),
        "HIT"
    );
    revalidate(store.as_ref(), TAG_GENERATION_PREFIX, "reports", valid_tag)
        .await
        .unwrap();
    assert_eq!(
        cache_status(&request(&app, "/?year=2026", Method::GET).await.0),
        "MISS"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn head_reuses_get_headers_without_sending_the_body() {
    let store: Arc<dyn CacheStore> = Arc::new(MemoryStore::with_capacity(16));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = app(PageCache::seconds(60), store, calls.clone(), false);

    request(&app, "/", Method::GET).await;
    let (head, body) = request(&app, "/", Method::HEAD).await;
    assert_eq!(cache_status(&head), "HIT");
    assert!(body.is_empty());
    assert_eq!(head.headers().get(header::CONTENT_LENGTH).unwrap(), "8");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
#[should_panic(expected = "greater than zero")]
fn a_zero_page_lifetime_is_not_a_disabled_policy() {
    PageCache::seconds(0);
}
