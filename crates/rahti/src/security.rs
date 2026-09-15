//! Baseline browser security headers for every HTTP response.
//!
//! The generated router installs [`middleware`] once, outside pages, RPCs,
//! development routes, and public files. Applications get safe defaults and
//! may replace them during startup with [`configure`]. A response's own header
//! always wins, which permits a route to apply a stricter policy.

use std::sync::{Arc, OnceLock, RwLock};

use axum::extract::Request;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;

#[cfg(test)]
#[path = "tests/security.rs"]
mod tests;

const CONTENT_SECURITY_POLICY: &str = "content-security-policy";
const PERMISSIONS_POLICY: &str = "permissions-policy";
const REFERRER_POLICY: &str = "referrer-policy";
const X_CONTENT_TYPE_OPTIONS: &str = "x-content-type-options";
const X_FRAME_OPTIONS: &str = "x-frame-options";
const STRICT_TRANSPORT_SECURITY: &str = "strict-transport-security";

/// Application-wide browser security-header policy.
///
/// Set a field to `None` to omit that header. Configuration is validated when
/// installed, before a request can observe it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityHeaders {
    pub content_security_policy: Option<String>,
    pub permissions_policy: Option<String>,
    pub referrer_policy: Option<String>,
    pub x_content_type_options: Option<String>,
    pub x_frame_options: Option<String>,
    /// Written only outside Rahti development mode. TLS normally terminates at
    /// a reverse proxy, but browsers still consume HSTS from this response.
    pub strict_transport_security: Option<String>,
}

impl Default for SecurityHeaders {
    fn default() -> Self {
        baseline(!crate::dev::enabled())
    }
}

fn baseline(production: bool) -> SecurityHeaders {
    let mut connect = "'self' ws: wss: https: blob:".to_string();
    let mut media = "'self' data: blob: https:".to_string();
    let mut frames = "'self' https:".to_string();
    if !production {
        connect.push_str(" http://localhost:* http://127.0.0.1:*");
        media.push_str(" http:");
        frames.push_str(" http:");
    }

    SecurityHeaders {
        content_security_policy: Some(format!(
            "default-src 'self'; \
             script-src 'self' 'unsafe-inline' 'unsafe-eval' https:; \
             worker-src 'self' blob:; \
             style-src 'self' 'unsafe-inline' https:; \
             img-src {media}; media-src {media}; \
             font-src 'self' data: https:; \
             connect-src {connect}; frame-src {frames}; \
             object-src 'none'; base-uri 'self'; form-action 'self'; \
             frame-ancestors 'self'"
        )),
        permissions_policy: Some(
            "camera=(self), geolocation=(self), microphone=(self), payment=(self), usb=(self)"
                .to_string(),
        ),
        referrer_policy: Some("strict-origin-when-cross-origin".to_string()),
        x_content_type_options: Some("nosniff".to_string()),
        x_frame_options: Some("SAMEORIGIN".to_string()),
        strict_transport_security: Some("max-age=31536000; includeSubDomains".to_string()),
    }
}

#[derive(Clone, Debug)]
struct Installed {
    always: HeaderMap,
    production: HeaderMap,
}

impl Installed {
    fn compile(settings: SecurityHeaders) -> Self {
        let mut always = HeaderMap::new();
        optional_header(
            &mut always,
            CONTENT_SECURITY_POLICY,
            settings.content_security_policy,
        );
        optional_header(&mut always, PERMISSIONS_POLICY, settings.permissions_policy);
        optional_header(&mut always, REFERRER_POLICY, settings.referrer_policy);
        optional_header(
            &mut always,
            X_CONTENT_TYPE_OPTIONS,
            settings.x_content_type_options,
        );
        optional_header(&mut always, X_FRAME_OPTIONS, settings.x_frame_options);

        let mut production = HeaderMap::new();
        optional_header(
            &mut production,
            STRICT_TRANSPORT_SECURITY,
            settings.strict_transport_security,
        );
        Self { always, production }
    }
}

fn optional_header(headers: &mut HeaderMap, name: &'static str, value: Option<String>) {
    let Some(value) = value else {
        return;
    };
    let value = HeaderValue::from_str(&value)
        .unwrap_or_else(|error| panic!("rahti: security header `{name}` is invalid: {error}"));
    headers.insert(HeaderName::from_static(name), value);
}

fn configured() -> &'static RwLock<Arc<Installed>> {
    static CONFIGURED: OnceLock<RwLock<Arc<Installed>>> = OnceLock::new();
    CONFIGURED.get_or_init(|| RwLock::new(Arc::new(Installed::compile(SecurityHeaders::default()))))
}

/// Replace the complete application security-header policy.
///
/// Call during application initialization before building the router. Later
/// calls replace it so in-process integration tests can install their policy.
pub fn configure(settings: SecurityHeaders) {
    let settings = Arc::new(Installed::compile(settings));
    match configured().write() {
        Ok(mut installed) => *installed = settings,
        Err(poisoned) => *poisoned.into_inner() = settings,
    }
}

fn installed() -> Arc<Installed> {
    configured()
        .read()
        .map(|settings| settings.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

/// Generated outer response middleware. Existing response headers win.
pub async fn middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    apply(response.headers_mut(), &installed(), !crate::dev::enabled());
    response
}

fn apply(headers: &mut HeaderMap, settings: &Installed, production: bool) {
    insert_missing(headers, &settings.always);
    if production {
        insert_missing(headers, &settings.production);
    }
}

fn insert_missing(target: &mut HeaderMap, source: &HeaderMap) {
    for (name, value) in source {
        if !target.contains_key(name) {
            target.insert(name.clone(), value.clone());
        }
    }
}
