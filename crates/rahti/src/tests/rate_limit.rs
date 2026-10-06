use std::fs;
use std::net::{IpAddr, Ipv4Addr};

use axum::Router;
use axum::body::to_bytes;
use axum::routing::get;
use tower::ServiceExt;

use super::*;

fn ip(last: u8) -> IpAddr {
    IpAddr::V4(Ipv4Addr::new(192, 0, 2, last))
}

#[test]
fn the_budget_allows_then_refuses_one_client() {
    let limiter = Limiter::new(RateLimit::per_minute(2));
    let now = Instant::now();

    assert_eq!(limiter.check(ip(1), now).remaining, 1);
    assert_eq!(limiter.check(ip(1), now).remaining, 0);
    assert!(!limiter.check(ip(1), now).allowed);
}

#[test]
fn clients_have_independent_buckets() {
    let limiter = Limiter::new(RateLimit::per_minute(1));
    let now = Instant::now();

    assert!(limiter.check(ip(1), now).allowed);
    assert!(limiter.check(ip(2), now).allowed);
}

#[test]
fn a_sliding_window_releases_an_expired_request() {
    let limiter = Limiter::new(RateLimit::per_second(1));
    let now = Instant::now();

    assert!(limiter.check(ip(1), now).allowed);
    assert!(
        !limiter
            .check(ip(1), now + Duration::from_millis(999))
            .allowed
    );
    assert!(limiter.check(ip(1), now + Duration::from_secs(1)).allowed);
}

#[test]
fn the_client_table_stays_bounded() {
    let limiter = Limiter::new(RateLimit {
        max_clients: 2,
        ..RateLimit::per_minute(1)
    });
    let now = Instant::now();

    limiter.check(ip(1), now);
    limiter.check(ip(2), now + Duration::from_millis(1));
    limiter.check(ip(3), now + Duration::from_millis(2));

    assert_eq!(limiter.store.lock().unwrap().clients.len(), 2);
}

fn request(client: Option<IpAddr>, forwarded: Option<&str>) -> Request {
    let mut request = Request::builder().uri("/").body(Body::empty()).unwrap();
    if let Some(client) = client {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(client, 3000)));
    }
    if let Some(forwarded) = forwarded {
        request
            .headers_mut()
            .insert("x-forwarded-for", forwarded.parse().unwrap());
    }
    request
}

#[test]
fn forwarded_addresses_are_opt_in_and_must_be_ips() {
    let direct = ip(1);
    let forwarded = ip(9);
    let forwarded_request = request(Some(direct), Some("192.0.2.9, 10.0.0.1"));

    assert_eq!(client_ip(&forwarded_request, false), Some(direct));
    assert_eq!(client_ip(&forwarded_request, true), Some(forwarded));

    let malformed = request(Some(direct), Some("attacker-controlled"));
    assert_eq!(client_ip(&malformed, true), Some(direct));
}

#[test]
fn exempt_paths_cover_subtrees_but_root_does_not() {
    assert!(path_matches("/health", "/health"));
    assert!(path_matches("/health", "/health/live"));
    assert!(!path_matches("/health", "/healthy"));
    assert!(path_matches("/", "/"));
    assert!(!path_matches("/", "/account"));
}

#[test]
fn framework_probes_cannot_be_removed_from_the_exemptions() {
    let policy = RateLimit {
        exempt_paths: Vec::new(),
        ..RateLimit::per_minute(1)
    };
    let state = state(std::env::temp_dir().join("rahti-no-public-files-here"));

    for path in ["/health", "/ready"] {
        let request = Request::builder().uri(path).body(Body::empty()).unwrap();
        assert!(exempt(&policy, &state, &request));
    }
}

#[test]
fn only_real_files_beneath_public_are_exempt() {
    let root = std::env::temp_dir().join(format!("rahti-rate-limit-{}", std::process::id()));
    let outside = root.with_extension("secret");
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_file(&outside);
    fs::create_dir_all(root.join("css")).unwrap();
    fs::write(root.join("css/app.css"), "body{}").unwrap();
    fs::write(&outside, "secret").unwrap();
    let root = fs::canonicalize(&root).unwrap();

    assert!(public_file(&root, "/css/app.css"));
    assert!(!public_file(&root, "/css/missing.css"));
    assert!(!public_file(&root, "/../secret"));
    assert!(!public_file(&root, "/%2e%2e/secret"));

    fs::remove_dir_all(&root).unwrap();
    fs::remove_file(&outside).unwrap();
}

#[tokio::test]
async fn a_rejected_rpc_gets_json_and_retry_headers() {
    let response = rejected(
        Outcome {
            allowed: false,
            limit: 1,
            remaining: 0,
            reset: 30,
        },
        true,
    );

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::RETRY_AFTER], "30");
    assert_eq!(response.headers()[LIMIT_HEADER], "1");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains(r#""error""#));
}

#[tokio::test]
async fn the_middleware_stops_an_exhausted_client_before_the_handler() {
    configure(RateLimit::per_minute(1));
    let state = state(std::env::temp_dir().join("rahti-no-public-files-here"));
    let app = Router::new()
        .route("/", get(|| async { "reached" }))
        .layer(axum::middleware::from_fn_with_state(state, middleware));

    let first = app
        .clone()
        .oneshot(request(Some(ip(20)), None))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[LIMIT_HEADER], "1");

    let mut second = request(Some(ip(20)), None);
    second
        .headers_mut()
        .insert(RPC_HEADER, HeaderValue::from_static("save"));
    let second = app.oneshot(second).await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(second.headers().contains_key(header::RETRY_AFTER));
    let body = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains(r#""error""#));

    configure(RateLimit::default());
}

#[test]
#[should_panic(expected = "window must be greater than zero")]
fn a_zero_window_is_rejected_at_configuration() {
    Limiter::new(RateLimit {
        window: Duration::ZERO,
        ..RateLimit::default()
    });
}

#[test]
fn exempt_adds_to_the_default_exemptions() {
    let policy = RateLimit::default().exempt("/api/hooks").exempt("/api/hooks");
    for kept in ["/health", "/ready", "/__rahti/dev"] {
        assert!(policy.exempt_paths.iter().any(|p| p == kept), "{kept} dropped");
    }
    assert_eq!(policy.exempt_paths.iter().filter(|p| *p == "/api/hooks").count(), 1);
}

#[test]
fn the_default_budget_follows_development_mode() {
    let expected = if crate::dev::enabled() {
        DEVELOPMENT_REQUESTS
    } else {
        PRODUCTION_REQUESTS
    };
    assert_eq!(RateLimit::default().requests, expected);
}
