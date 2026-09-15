use super::*;
use axum::Router;
use axum::body::Body;
use axum::http::Request;
use axum::routing::get;
use http_body_util::BodyExt;
use tower::ServiceExt;

static RESOURCES: &[ResourceSource] = &[
    ResourceSource {
        locale: "en",
        source: "hello = Hello, { $name }\nitems = { $count ->\n [one] One item\n*[other] { $count } items\n}",
    },
    ResourceSource {
        locale: "es",
        source: "hello = Hola, { $name }",
    },
];
static FALLBACKS: &[FallbackSource] = &[FallbackSource {
    locale: "es",
    fallbacks: &["en"],
}];

fn router() -> Router {
    let state = state("en", &["en", "es"], FALLBACKS, RESOURCES);
    Router::new()
        .route(
            "/{locale}",
            get(|| async {
                format!(
                    "{}|{}|{}|{}",
                    locale().as_str(),
                    t_with("hello", &[("name", "Ada".into())]),
                    t_with("items", &[("count", 2.into())]),
                    localized_path("/account")
                )
            }),
        )
        .route("/plain", get(|| async { locale().0 }))
        .layer(axum::middleware::from_fn_with_state(state, middleware))
}

#[tokio::test]
async fn an_unprefixed_route_negotiates_accept_language() {
    let response = router()
        .oneshot(
            Request::get("/plain")
                .header(header::ACCEPT_LANGUAGE, "fr;q=0.9, es;q=0.8")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "es");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(String::from_utf8(body.to_vec()).unwrap(), "es");
}

#[tokio::test]
async fn route_locale_selects_catalog_and_falls_back() {
    let response = router()
        .oneshot(Request::get("/es").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()[header::CONTENT_LANGUAGE], "es");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert_eq!(
        body,
        "es|Hola, \u{2068}Ada\u{2069}|\u{2068}2\u{2069} items|/es/account"
    );
}

#[tokio::test]
async fn unsupported_route_locale_is_not_found() {
    let response = router()
        .oneshot(Request::get("/fr").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn direction_uses_the_language_subtag() {
    assert_eq!(direction_for("ar-EG"), "rtl");
    assert_eq!(direction_for("en-US"), "ltr");
}

#[test]
fn only_an_explicit_locale_is_safe_for_page_caching() {
    let mut request = Request::get("/").body(Body::empty()).unwrap();
    assert!(request_cache_safe(&request));
    request
        .extensions_mut()
        .insert(LocaleSelection { explicit: false });
    assert!(!request_cache_safe(&request));
    request
        .extensions_mut()
        .insert(LocaleSelection { explicit: true });
    assert!(request_cache_safe(&request));
}
