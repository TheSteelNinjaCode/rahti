use std::{env, sync::Arc};

use axum::{
    Json, Router,
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get},
};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::{Project, RahtiMcp};

pub const AUTH_TOKEN_ENV: &str = "RAHTI_MCP_AUTH_TOKEN";
pub const ALLOWED_HOSTS_ENV: &str = "RAHTI_MCP_ALLOWED_HOSTS";
pub const ALLOWED_ORIGINS_ENV: &str = "RAHTI_MCP_ALLOWED_ORIGINS";

pub fn routes() -> Router {
    let project = Project::open_http(".");
    let factory = move || {
        project
            .clone()
            .map(RahtiMcp::new)
            .map_err(std::io::Error::other)
    };
    let mut config = StreamableHttpServerConfig::default();
    if let Some(hosts) = csv_env(ALLOWED_HOSTS_ENV) {
        config = config.with_allowed_hosts(hosts);
    }
    if let Some(origins) = csv_env(ALLOWED_ORIGINS_ENV) {
        config = config.with_allowed_origins(origins);
    }
    let service: StreamableHttpService<RahtiMcp, LocalSessionManager> =
        StreamableHttpService::new(factory, Arc::default(), config);
    let protected = Router::new()
        .nest_service("/mcp/", service)
        .layer(middleware::from_fn(authorize));

    Router::new()
        .route("/mcp", any(canonical_redirect))
        .route("/.well-known/mcp.json", get(discovery))
        .merge(protected)
}

async fn canonical_redirect() -> Redirect {
    // Relative on purpose: an edge proxy that does not forward the original
    // scheme must never turn an HTTPS MCP request into an HTTP redirect.
    Redirect::temporary("/mcp/")
}

async fn discovery() -> Json<serde_json::Value> {
    Json(json!({
        "name": "rahti",
        "transport": "streamable-http",
        "endpoint": "/mcp/",
        "authentication": { "type": "bearer", "environment": AUTH_TOKEN_ENV },
        "siteInfo": "/llms.txt",
    }))
}

async fn authorize(request: Request, next: Next) -> Response {
    if request.method() == Method::OPTIONS {
        return next.run(request).await;
    }
    let configured = env::var(AUTH_TOKEN_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty());
    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));

    match configured.as_deref() {
        Some(expected) if token_matches(provided, expected) => next.run(request).await,
        Some(_) => unauthorized(),
        None if cfg!(debug_assertions) => next.run(request).await,
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": format!("{AUTH_TOKEN_ENV} is not configured") })),
        )
            .into_response(),
    }
}

fn token_matches(provided: Option<&str>, expected: &str) -> bool {
    provided.is_some_and(|value| bool::from(value.as_bytes().ct_eq(expected.as_bytes())))
}

fn unauthorized() -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "A valid bearer token is required" })),
    )
        .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"Rahti MCP\""),
    );
    response
}

fn csv_env(name: &str) -> Option<Vec<String>> {
    let values = env::var(name)
        .ok()?
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    (!values.is_empty()).then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn bearer_tokens_are_compared_exactly() {
        assert!(token_matches(Some("secret"), "secret"));
        assert!(!token_matches(Some("wrong"), "secret"));
        assert!(!token_matches(None, "secret"));
    }

    #[tokio::test]
    async fn canonical_redirect_is_relative_and_preserves_the_method() {
        let response = Router::new()
            .route("/mcp", any(canonical_redirect))
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/mcp")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(response.headers()[header::LOCATION], "/mcp/");
    }

    #[test]
    fn unauthorized_responses_advertise_bearer_authentication() {
        let response = unauthorized();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers()[header::WWW_AUTHENTICATE],
            "Bearer realm=\"Rahti MCP\""
        );
    }
}
