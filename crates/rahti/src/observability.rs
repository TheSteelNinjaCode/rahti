//! Production request correlation, structured lifecycle logs, health probes,
//! and bounded-cardinality process metrics.

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::future::join_all;
use serde::Serialize;
use serde_json::{Map, Value, json};

#[cfg(test)]
#[path = "tests/observability.rs"]
mod tests;

const REQUEST_ID_HEADER: &str = "x-request-id";
const HEALTH_PATH: &str = "/health";
const READY_PATH: &str = "/ready";
const MAX_REQUEST_ID_LEN: usize = 128;
const MAX_EVENT_TEXT: usize = 16 * 1024;

type CheckFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'static>>;
type CheckFn = dyn Fn() -> CheckFuture + Send + Sync + 'static;

#[derive(Clone)]
struct ReadinessCheck {
    name: String,
    run: Arc<CheckFn>,
}

/// Production observability policy installed before the generated router is
/// built.
#[derive(Clone)]
pub struct Observability {
    enabled: bool,
    service_name: String,
    request_logs: bool,
    log_health_checks: bool,
    accept_request_ids: bool,
    slow_request: Duration,
    readiness_timeout: Duration,
    checks: Arc<[ReadinessCheck]>,
}

impl Observability {
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Override the Cargo package name used in records and probe responses.
    pub fn service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    pub fn request_logs(mut self, enabled: bool) -> Self {
        self.request_logs = enabled;
        self
    }

    /// Include `/health` and `/ready` in request lifecycle logs and metrics.
    pub fn log_health_checks(mut self, enabled: bool) -> Self {
        self.log_health_checks = enabled;
        self
    }

    /// Accept a valid caller-provided `X-Request-Id`; otherwise replace it.
    pub fn accept_request_ids(mut self, accept: bool) -> Self {
        self.accept_request_ids = accept;
        self
    }

    pub fn slow_request(mut self, threshold: Duration) -> Self {
        assert!(
            !threshold.is_zero(),
            "rahti: observability slow-request threshold must be greater than zero"
        );
        self.slow_request = threshold;
        self
    }

    pub fn readiness_timeout(mut self, timeout: Duration) -> Self {
        assert!(
            !timeout.is_zero(),
            "rahti: observability readiness timeout must be greater than zero"
        );
        self.readiness_timeout = timeout;
        self
    }

    /// Add one named asynchronous readiness dependency check.
    pub fn with_readiness<F, Fut, E>(self, name: impl Into<String>, check: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: fmt::Display + Send + 'static,
    {
        let name = name.into();
        validate_check_name(&name, &self.checks);
        let mut checks = self.checks.iter().cloned().collect::<Vec<_>>();
        checks.push(ReadinessCheck {
            name,
            run: Arc::new(move || {
                let future = check();
                Box::pin(async move { future.await.map_err(|error| error.to_string()) })
            }),
        });
        Self {
            checks: checks.into(),
            ..self
        }
    }
}

impl Default for Observability {
    fn default() -> Self {
        Self {
            enabled: !crate::dev::enabled(),
            service_name: String::new(),
            request_logs: true,
            log_health_checks: false,
            accept_request_ids: true,
            slow_request: Duration::from_secs(1),
            readiness_timeout: Duration::from_secs(2),
            checks: Arc::new([]),
        }
    }
}

impl fmt::Debug for Observability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Observability")
            .field("enabled", &self.enabled)
            .field("service_name", &self.service_name)
            .field("request_logs", &self.request_logs)
            .field("log_health_checks", &self.log_health_checks)
            .field("accept_request_ids", &self.accept_request_ids)
            .field("slow_request", &self.slow_request)
            .field("readiness_timeout", &self.readiness_timeout)
            .field("readiness_checks", &self.checks.len())
            .finish()
    }
}

fn validate_check_name(name: &str, existing: &[ReadinessCheck]) {
    assert!(
        !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')),
        "rahti: readiness check names use 1-64 ASCII letters, numbers, '-' or '_'"
    );
    assert!(
        !existing.iter().any(|check| check.name == name),
        "rahti: duplicate readiness check `{name}`"
    );
}

fn installed() -> &'static RwLock<Observability> {
    static INSTALLED: OnceLock<RwLock<Observability>> = OnceLock::new();
    INSTALLED.get_or_init(|| RwLock::new(Observability::default()))
}

fn default_service() -> &'static OnceLock<String> {
    static SERVICE: OnceLock<String> = OnceLock::new();
    &SERVICE
}

/// Install the complete policy. Build the generated router afterwards.
pub fn configure(policy: Observability) {
    assert!(
        policy.service_name.len() <= 128,
        "rahti: observability service name is limited to 128 bytes"
    );
    match installed().write() {
        Ok(mut current) => *current = policy,
        Err(poisoned) => *poisoned.into_inner() = policy,
    }
}

/// A validated correlation identifier available in request extensions and
/// through [`request_id`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

tokio::task_local! {
    static CURRENT_REQUEST_ID: RequestId;
}

/// The current request's correlation id, including inside ordinary pages and
/// RPCs that take no request parameter.
pub fn request_id() -> Option<RequestId> {
    CURRENT_REQUEST_ID.try_with(Clone::clone).ok()
}

#[derive(Clone, Debug)]
pub struct ObservabilityState {
    policy: Observability,
    public_dir: PathBuf,
}

/// Snapshot startup configuration for the generated router.
#[doc(hidden)]
pub fn state(default_service_name: &str, public_dir: PathBuf) -> ObservabilityState {
    let _ = default_service().set(default_service_name.to_string());
    let mut policy = installed()
        .read()
        .map(|policy| policy.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    if policy.service_name.trim().is_empty() {
        policy.service_name = default_service_name.to_string();
    }
    ObservabilityState {
        policy,
        public_dir: std::fs::canonicalize(&public_dir).unwrap_or(public_dir),
    }
}

/// Framework-owned liveness and readiness endpoints.
#[doc(hidden)]
pub fn routes(state: ObservabilityState) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(health))
        .route(READY_PATH, get(ready))
        .with_state(state)
}

async fn health(State(state): State<ObservabilityState>) -> Response {
    probe_response(
        StatusCode::OK,
        json!({ "status": "ok", "service": state.policy.service_name }),
    )
}

async fn ready(State(state): State<ObservabilityState>) -> Response {
    let timeout = state.policy.readiness_timeout;
    let results = join_all(state.policy.checks.iter().cloned().map(|check| async move {
        let result = tokio::time::timeout(timeout, (check.run)()).await;
        let (status, detail) = match result {
            Ok(Ok(())) => ("ok", None),
            Ok(Err(error)) => ("failed", Some(error)),
            Err(_) => (
                "timeout",
                Some(format!("exceeded {} ms", timeout.as_millis())),
            ),
        };
        (check.name, status, detail)
    }))
    .await;

    let mut healthy = true;
    let checks = results
        .into_iter()
        .map(|(name, status, detail)| {
            if status != "ok" {
                healthy = false;
                record_with(
                    "warn",
                    "readiness",
                    "readiness check did not pass",
                    json!({ "check": name, "status": status, "detail": detail }),
                );
            }
            json!({ "name": name, "status": status })
        })
        .collect::<Vec<_>>();

    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    probe_response(
        status,
        json!({
            "status": if healthy { "ready" } else { "not_ready" },
            "service": state.policy.service_name,
            "checks": checks,
        }),
    )
}

fn probe_response(status: StatusCode, body: Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    response
}

/// Generated outer request lifecycle middleware.
#[doc(hidden)]
pub async fn middleware(
    State(state): State<ObservabilityState>,
    mut request: Request,
    next: Next,
) -> Response {
    if !state.policy.enabled {
        return next.run(request).await;
    }

    let request_id = choose_request_id(request.headers().get(REQUEST_ID_HEADER), &state.policy);
    let header = HeaderValue::from_str(request_id.as_str()).expect("a validated request id");
    request
        .headers_mut()
        .insert(REQUEST_ID_HEADER, header.clone());
    request.extensions_mut().insert(request_id.clone());

    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let observed = should_observe(&state, &method, &path);
    let started = Instant::now();
    let _active = observed.then(ActiveRequest::start);
    let mut response = CURRENT_REQUEST_ID
        .scope(request_id.clone(), next.run(request))
        .await;
    let elapsed = started.elapsed();
    response.headers_mut().insert(REQUEST_ID_HEADER, header);

    if observed {
        METRICS.finish(response.status(), elapsed, state.policy.slow_request);
        if state.policy.request_logs {
            request_record(
                &state.policy,
                &request_id,
                &method,
                &path,
                response.status(),
                elapsed,
            );
        }
    }
    response
}

fn should_observe(state: &ObservabilityState, method: &Method, path: &str) -> bool {
    if !state.policy.log_health_checks && matches!(path, HEALTH_PATH | READY_PATH) {
        return false;
    }
    !(matches!(method, &Method::GET | &Method::HEAD)
        && crate::rate_limit::public_file(&state.public_dir, path))
}

fn choose_request_id(value: Option<&HeaderValue>, policy: &Observability) -> RequestId {
    if policy.accept_request_ids {
        if let Some(value) = value
            .and_then(|value| value.to_str().ok())
            .filter(|value| valid_request_id(value))
        {
            return RequestId(value.to_string());
        }
    }
    RequestId(generated_request_id())
}

fn valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_REQUEST_ID_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn generated_request_id() -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_ok() {
        return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    }

    static FALLBACK: AtomicU64 = AtomicU64::new(0);
    let sequence = FALLBACK.fetch_add(1, Ordering::Relaxed);
    format!("{:x}-{sequence:x}", now_ms())
}

struct ActiveRequest;

impl ActiveRequest {
    fn start() -> Self {
        METRICS.active.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        METRICS.active.fetch_sub(1, Ordering::Relaxed);
    }
}

struct MetricCounters {
    total: AtomicU64,
    active: AtomicU64,
    client_errors: AtomicU64,
    server_errors: AtomicU64,
    slow: AtomicU64,
    duration_micros: AtomicU64,
    max_duration_micros: AtomicU64,
}

static METRICS: MetricCounters = MetricCounters {
    total: AtomicU64::new(0),
    active: AtomicU64::new(0),
    client_errors: AtomicU64::new(0),
    server_errors: AtomicU64::new(0),
    slow: AtomicU64::new(0),
    duration_micros: AtomicU64::new(0),
    max_duration_micros: AtomicU64::new(0),
};

impl MetricCounters {
    fn finish(&self, status: StatusCode, elapsed: Duration, slow: Duration) {
        self.total.fetch_add(1, Ordering::Relaxed);
        if status.is_client_error() {
            self.client_errors.fetch_add(1, Ordering::Relaxed);
        }
        if status.is_server_error() {
            self.server_errors.fetch_add(1, Ordering::Relaxed);
        }
        if elapsed >= slow {
            self.slow.fetch_add(1, Ordering::Relaxed);
        }
        let micros = elapsed.as_micros().min(u64::MAX as u128) as u64;
        self.duration_micros.fetch_add(micros, Ordering::Relaxed);
        self.max_duration_micros
            .fetch_max(micros, Ordering::Relaxed);
    }
}

/// Aggregate, label-free process metrics. Values reset when the process exits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub requests: u64,
    pub active_requests: u64,
    pub client_errors: u64,
    pub server_errors: u64,
    pub slow_requests: u64,
    pub duration_micros: u64,
    pub max_duration_micros: u64,
}

pub fn metrics() -> MetricsSnapshot {
    MetricsSnapshot {
        requests: METRICS.total.load(Ordering::Relaxed),
        active_requests: METRICS.active.load(Ordering::Relaxed),
        client_errors: METRICS.client_errors.load(Ordering::Relaxed),
        server_errors: METRICS.server_errors.load(Ordering::Relaxed),
        slow_requests: METRICS.slow.load(Ordering::Relaxed),
        duration_micros: METRICS.duration_micros.load(Ordering::Relaxed),
        max_duration_micros: METRICS.max_duration_micros.load(Ordering::Relaxed),
    }
}

fn request_record(
    policy: &Observability,
    request_id: &RequestId,
    method: &Method,
    path: &str,
    status: StatusCode,
    elapsed: Duration,
) {
    let slow = elapsed >= policy.slow_request;
    let level = if status.is_server_error() {
        "error"
    } else if status.is_client_error() || slow {
        "warn"
    } else {
        "info"
    };
    write_record(json!({
        "kind": "request",
        "timestampMs": now_ms(),
        "level": level,
        "service": policy.service_name,
        "requestId": request_id.as_str(),
        "method": method.as_str(),
        "path": path,
        "status": status.as_u16(),
        "durationMs": elapsed.as_secs_f64() * 1000.0,
        "slow": slow,
    }));
}

/// Write one application event to the same JSONL stream as request records.
pub fn record(level: &str, target: &str, message: &str) {
    record_with(level, target, message, Value::Null);
}

/// Write one application event with structured fields nested under `fields`.
pub fn record_with(level: &str, target: &str, message: &str, fields: Value) {
    let policy = installed()
        .read()
        .map(|policy| policy.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone());
    if !policy.enabled {
        return;
    }

    let mut record = Map::new();
    record.insert("kind".into(), Value::String("event".into()));
    record.insert("timestampMs".into(), now_ms().into());
    record.insert("level".into(), Value::String(limit(level)));
    let service = if policy.service_name.trim().is_empty() {
        default_service()
            .get()
            .cloned()
            .unwrap_or_else(|| "rahti-app".to_string())
    } else {
        policy.service_name
    };
    record.insert("service".into(), Value::String(service));
    record.insert("target".into(), Value::String(limit(target)));
    record.insert("message".into(), Value::String(limit(message)));
    if let Some(request_id) = request_id() {
        record.insert("requestId".into(), Value::String(request_id.0));
    }
    if !fields.is_null() {
        record.insert("fields".into(), fields);
    }
    write_record(Value::Object(record));
}

pub(crate) fn enabled() -> bool {
    installed()
        .read()
        .map(|policy| policy.enabled)
        .unwrap_or_else(|poisoned| poisoned.into_inner().enabled)
}

fn limit(value: &str) -> String {
    if value.len() <= MAX_EVENT_TEXT {
        return value.to_string();
    }
    let mut end = MAX_EVENT_TEXT;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

fn write_record(record: Value) {
    static WRITE: Mutex<()> = Mutex::new(());
    let _guard = WRITE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    eprintln!("{record}");
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
fn reset_metrics() {
    METRICS.total.store(0, Ordering::Relaxed);
    METRICS.active.store(0, Ordering::Relaxed);
    METRICS.client_errors.store(0, Ordering::Relaxed);
    METRICS.server_errors.store(0, Ordering::Relaxed);
    METRICS.slow.store(0, Ordering::Relaxed);
    METRICS.duration_micros.store(0, Ordering::Relaxed);
    METRICS.max_duration_micros.store(0, Ordering::Relaxed);
}
