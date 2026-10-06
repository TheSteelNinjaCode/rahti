//! Application-wide per-client request rate limiting.
//!
//! The generated router installs [`middleware`] outside all application work
//! and inside the security-header layer. Real public files, health checks,
//! development tooling, and CORS preflight requests do not consume the
//! dynamic-request budget.

use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use percent_encoding::percent_decode_str;

#[cfg(test)]
#[path = "tests/rate_limit.rs"]
mod tests;

const LIMIT_HEADER: &str = "ratelimit-limit";
const REMAINING_HEADER: &str = "ratelimit-remaining";
const RESET_HEADER: &str = "ratelimit-reset";
const RPC_HEADER: &str = "x-pp-function";

/// The default budget per client and minute in production.
pub const PRODUCTION_REQUESTS: u32 = 200;

/// The default budget per client and minute in development mode. Everything
/// on a developer's machine — the browser, an editor's preview pane polling
/// the server, test scripts, coding agents — is one client, `127.0.0.1`, and
/// the production budget is spent by the tooling alone.
pub const DEVELOPMENT_REQUESTS: u32 = 2_000;

/// The application-wide request budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateLimit {
    /// Requests admitted for one client during `window`.
    pub requests: u32,
    pub window: Duration,
    /// Maximum client buckets retained in process memory.
    pub max_clients: usize,
    /// Honor the first `X-Forwarded-For` address. Enable only behind a trusted
    /// proxy that replaces client-supplied forwarding headers.
    pub trust_forwarded_headers: bool,
    /// Exempt route subtrees. `/` matches only `/`, like auth route policies.
    pub exempt_paths: Vec<String>,
}

impl RateLimit {
    pub fn per_second(requests: u32) -> Self {
        Self {
            requests,
            window: Duration::from_secs(1),
            ..Self::default()
        }
    }

    pub fn per_minute(requests: u32) -> Self {
        Self {
            requests,
            window: Duration::from_secs(60),
            ..Self::default()
        }
    }

    pub fn per_hour(requests: u32) -> Self {
        Self {
            requests,
            window: Duration::from_secs(60 * 60),
            ..Self::default()
        }
    }

    /// Exempt one more route subtree, keeping the exemptions already there.
    ///
    /// Prefer this to assigning `exempt_paths`: assigning replaces the
    /// defaults, and `/health`, `/ready` and the development tooling start
    /// spending the budget.
    ///
    /// ```
    /// let policy = rahti::rate_limit::RateLimit::default().exempt("/api/hooks");
    /// assert!(policy.exempt_paths.iter().any(|p| p == "/health"));
    /// ```
    pub fn exempt(mut self, path: impl Into<String>) -> Self {
        let path = path.into();
        if !self.exempt_paths.contains(&path) {
            self.exempt_paths.push(path);
        }
        self
    }

    /// No request limiting. Public-file detection and bucket allocation are
    /// skipped too.
    pub fn disabled() -> Self {
        Self {
            requests: 0,
            ..Self::default()
        }
    }
}

impl Default for RateLimit {
    /// [`PRODUCTION_REQUESTS`] per minute, or [`DEVELOPMENT_REQUESTS`] in
    /// development mode.
    fn default() -> Self {
        Self {
            requests: if crate::dev::enabled() {
                DEVELOPMENT_REQUESTS
            } else {
                PRODUCTION_REQUESTS
            },
            window: Duration::from_secs(60),
            max_clients: 10_000,
            trust_forwarded_headers: false,
            exempt_paths: vec![
                "/health".to_string(),
                "/ready".to_string(),
                "/__rahti/dev".to_string(),
            ],
        }
    }
}

#[derive(Debug)]
struct Bucket {
    requests: VecDeque<Instant>,
}

#[derive(Debug)]
struct Store {
    clients: BTreeMap<IpAddr, Bucket>,
    last_cleanup: Instant,
}

#[derive(Debug)]
struct Limiter {
    policy: RateLimit,
    store: Mutex<Store>,
}

impl Limiter {
    fn new(policy: RateLimit) -> Self {
        validate(&policy);
        Self {
            policy,
            store: Mutex::new(Store {
                clients: BTreeMap::new(),
                last_cleanup: Instant::now(),
            }),
        }
    }

    fn check(&self, client: IpAddr, now: Instant) -> Outcome {
        let mut store = self
            .store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cleanup(&mut store, &self.policy, now);

        if !store.clients.contains_key(&client) && store.clients.len() >= self.policy.max_clients {
            // Avoid an LRU scan: a spoofed new address must not turn into
            // O(max_clients) CPU. BTreeMap removes one bucket in logarithmic
            // time while preserving the hard memory bound.
            store.clients.pop_first();
        }

        let bucket = store.clients.entry(client).or_insert_with(|| Bucket {
            requests: VecDeque::new(),
        });
        expire(&mut bucket.requests, self.policy.window, now);

        let limit = self.policy.requests;
        if bucket.requests.len() >= limit as usize {
            return Outcome {
                allowed: false,
                limit,
                remaining: 0,
                reset: reset_after(&bucket.requests, self.policy.window, now),
            };
        }

        bucket.requests.push_back(now);
        Outcome {
            allowed: true,
            limit,
            remaining: limit - bucket.requests.len() as u32,
            reset: reset_after(&bucket.requests, self.policy.window, now),
        }
    }
}

fn validate(policy: &RateLimit) {
    if policy.requests == 0 {
        return;
    }
    assert!(
        !policy.window.is_zero(),
        "rahti: rate-limit window must be greater than zero"
    );
    assert!(
        policy.max_clients > 0,
        "rahti: rate-limit max_clients must be greater than zero"
    );
    for path in &policy.exempt_paths {
        assert!(
            path.starts_with('/')
                && !path.contains('?')
                && !path.contains('#')
                && !path.chars().any(char::is_whitespace),
            "rahti: rate-limit exempt path `{path}` must be an absolute URL path"
        );
    }
}

fn cleanup(store: &mut Store, policy: &RateLimit, now: Instant) {
    let interval = policy.window.min(Duration::from_secs(60));
    if now.saturating_duration_since(store.last_cleanup) < interval {
        return;
    }
    store.last_cleanup = now;
    store.clients.retain(|_, bucket| {
        expire(&mut bucket.requests, policy.window, now);
        !bucket.requests.is_empty()
    });
}

fn expire(requests: &mut VecDeque<Instant>, window: Duration, now: Instant) {
    while requests
        .front()
        .is_some_and(|seen| now.saturating_duration_since(*seen) >= window)
    {
        requests.pop_front();
    }
}

fn reset_after(requests: &VecDeque<Instant>, window: Duration, now: Instant) -> u64 {
    requests
        .front()
        .map(|oldest| {
            window
                .saturating_sub(now.saturating_duration_since(*oldest))
                .as_secs_f64()
                .ceil()
                .max(1.0) as u64
        })
        .unwrap_or_else(|| window.as_secs_f64().ceil().max(1.0) as u64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Outcome {
    allowed: bool,
    limit: u32,
    remaining: u32,
    reset: u64,
}

fn configured() -> &'static RwLock<Arc<Limiter>> {
    static CONFIGURED: OnceLock<RwLock<Arc<Limiter>>> = OnceLock::new();
    CONFIGURED.get_or_init(|| RwLock::new(Arc::new(Limiter::new(RateLimit::default()))))
}

/// Install the complete policy and reset all process-local buckets.
pub fn configure(policy: RateLimit) {
    let limiter = Arc::new(Limiter::new(policy));
    match configured().write() {
        Ok(mut installed) => *installed = limiter,
        Err(poisoned) => *poisoned.into_inner() = limiter,
    }
}

fn limiter() -> Arc<Limiter> {
    configured()
        .read()
        .map(|limiter| limiter.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

/// State generated beside the public-file fallback.
#[derive(Clone, Debug)]
pub struct RateLimitState {
    public_dir: PathBuf,
}

pub fn state(public_dir: PathBuf) -> RateLimitState {
    RateLimitState {
        public_dir: std::fs::canonicalize(&public_dir).unwrap_or(public_dir),
    }
}

/// Generated request middleware.
pub async fn middleware(
    State(state): State<RateLimitState>,
    request: Request,
    next: Next,
) -> Response {
    let limiter = limiter();
    if limiter.policy.requests == 0 || exempt(&limiter.policy, &state, &request) {
        return next.run(request).await;
    }

    let rpc = request.headers().contains_key(RPC_HEADER);
    let Some(client) = client_ip(&request, limiter.policy.trust_forwarded_headers) else {
        // In-process router tests and custom hosts may have no transport peer.
        // The scaffolded server always supplies ConnectInfo.
        return next.run(request).await;
    };
    let outcome = limiter.check(client, Instant::now());

    if !outcome.allowed {
        return rejected(outcome, rpc);
    }

    let mut response = next.run(request).await;
    apply_headers(response.headers_mut(), outcome);
    response
}

fn exempt(policy: &RateLimit, state: &RateLimitState, request: &Request) -> bool {
    if request.method() == Method::OPTIONS {
        return true;
    }
    let path = request.uri().path();
    // Probes must remain reachable even when an application replaces the
    // default exemption list with a custom policy.
    if path_matches("/health", path) || path_matches("/ready", path) {
        return true;
    }
    if policy
        .exempt_paths
        .iter()
        .any(|candidate| path_matches(candidate, path))
    {
        return true;
    }
    matches!(request.method(), &Method::GET | &Method::HEAD) && public_file(&state.public_dir, path)
}

fn path_matches(configured: &str, request: &str) -> bool {
    if configured == "/" {
        return request == "/";
    }
    let configured = configured.trim_end_matches('/');
    request == configured
        || request
            .strip_prefix(configured)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(crate) fn public_file(root: &Path, uri_path: &str) -> bool {
    let Ok(decoded) = percent_decode_str(uri_path).decode_utf8() else {
        return false;
    };
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let Ok(candidate) = std::fs::canonicalize(root.join(relative)) else {
        return false;
    };
    candidate.starts_with(root) && candidate.is_file()
}

fn client_ip(request: &Request, trust_forwarded: bool) -> Option<IpAddr> {
    if trust_forwarded {
        if let Some(client) = request
            .headers()
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<IpAddr>().ok())
        {
            return Some(client);
        }
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0.ip())
}

fn apply_headers(headers: &mut HeaderMap, outcome: Outcome) {
    insert_number(headers, LIMIT_HEADER, outcome.limit.into());
    insert_number(headers, REMAINING_HEADER, outcome.remaining.into());
    insert_number(headers, RESET_HEADER, outcome.reset);
}

fn insert_number(headers: &mut HeaderMap, name: &'static str, value: u64) {
    if !headers.contains_key(name) {
        headers.insert(name, HeaderValue::from_str(&value.to_string()).unwrap());
    }
}

fn rejected(outcome: Outcome, rpc: bool) -> Response {
    let mut response = if rpc {
        let body = serde_json::json!({
            "error": "Too many requests. Please slow down and try again shortly."
        });
        let mut response = (StatusCode::TOO_MANY_REQUESTS, body.to_string()).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        response
    } else {
        Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(
                "<h1>429 - Too Many Requests</h1><p>Please slow down and try again shortly.</p>",
            ))
            .unwrap()
    };
    apply_headers(response.headers_mut(), outcome);
    response.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&outcome.reset.to_string()).unwrap(),
    );
    response
}
