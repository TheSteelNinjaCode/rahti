use axum::http::{HeaderMap, HeaderValue};

use super::*;

fn value<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers.get(name).unwrap().to_str().unwrap()
}

#[test]
fn the_baseline_covers_browser_security_boundaries() {
    let installed = Installed::compile(baseline(true));
    let mut headers = HeaderMap::new();
    apply(&mut headers, &installed, true);

    assert_eq!(value(&headers, X_CONTENT_TYPE_OPTIONS), "nosniff");
    assert_eq!(value(&headers, X_FRAME_OPTIONS), "SAMEORIGIN");
    assert_eq!(
        value(&headers, REFERRER_POLICY),
        "strict-origin-when-cross-origin"
    );
    assert!(headers.contains_key(PERMISSIONS_POLICY));
    assert!(headers.contains_key(STRICT_TRANSPORT_SECURITY));
}

#[test]
fn the_csp_keeps_pulsepoint_working_and_locks_down_injection_targets() {
    let policy = baseline(true).content_security_policy.unwrap();

    // Same-origin script and nothing else: no eval (PulsePoint runs only the
    // compiled client registry), no inline script and no other host, so
    // injected markup cannot run script — and only PulsePoint's Trusted Types
    // policy may write markup. `src/tests/browser.rs` proves a page works
    // under it.
    assert!(policy.contains("script-src 'self';"));
    assert!(!policy.contains("unsafe-eval"));
    assert!(policy.contains("require-trusted-types-for 'script'"));
    assert!(policy.contains("trusted-types pulsepoint"));
    assert!(policy.contains("object-src 'none'"));
    assert!(policy.contains("base-uri 'self'"));
    assert!(policy.contains("form-action 'self'"));
    assert!(policy.contains("frame-ancestors 'self'"));
    assert!(policy.contains("connect-src 'self' ws: wss:"));
}

#[test]
fn development_allows_loopback_connections_but_production_does_not() {
    let development = baseline(false).content_security_policy.unwrap();
    let production = baseline(true).content_security_policy.unwrap();

    assert!(development.contains("http://localhost:*"));
    assert!(development.contains("http://127.0.0.1:*"));
    assert!(!production.contains("localhost"));
    assert!(!production.contains("127.0.0.1"));
}

#[test]
fn hsts_is_production_only() {
    let installed = Installed::compile(baseline(true));
    let mut development = HeaderMap::new();
    let mut production = HeaderMap::new();

    apply(&mut development, &installed, false);
    apply(&mut production, &installed, true);

    assert!(!development.contains_key(STRICT_TRANSPORT_SECURITY));
    // This host only: a subdomain the application does not run is not its
    // to put under HSTS.
    assert_eq!(production[STRICT_TRANSPORT_SECURITY], "max-age=31536000");
}

#[test]
fn a_response_owned_header_is_preserved() {
    let installed = Installed::compile(baseline(true));
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'"),
    );

    apply(&mut headers, &installed, true);

    assert_eq!(
        value(&headers, CONTENT_SECURITY_POLICY),
        "default-src 'none'"
    );
}

#[test]
fn an_application_can_replace_or_disable_individual_headers() {
    let settings = SecurityHeaders {
        content_security_policy: Some("default-src 'none'".to_string()),
        permissions_policy: None,
        strict_transport_security: None,
        ..baseline(true)
    };
    let installed = Installed::compile(settings);
    let mut headers = HeaderMap::new();

    apply(&mut headers, &installed, true);

    assert_eq!(
        value(&headers, CONTENT_SECURITY_POLICY),
        "default-src 'none'"
    );
    assert!(!headers.contains_key(PERMISSIONS_POLICY));
    assert!(!headers.contains_key(STRICT_TRANSPORT_SECURITY));
}

#[test]
#[should_panic(expected = "security header `content-security-policy` is invalid")]
fn malformed_configuration_fails_during_installation() {
    Installed::compile(SecurityHeaders {
        content_security_policy: Some("default-src 'self'\nset-cookie: stolen".to_string()),
        ..baseline(true)
    });
}
