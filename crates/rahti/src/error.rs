//! Error boundaries.
//!
//! An `error.rs` catches failures from every page in its subtree, the way
//! Next.js nests `error.tsx`. The nearest boundary above a page wins, and the
//! rendered error takes the page's place *inside* the layouts above that
//! boundary — layouts deeper than the boundary are skipped, because the
//! subtree that owns them is what failed.
//!
//! Two things reach a boundary:
//!
//! - a page returning [`Err`], via [`Error`];
//! - a page panicking, via [`panic_response`] and `tower_http`'s
//!   `CatchPanicLayer`, which `src/routes.rs` mounts closest to the handler.

use std::any::Any;
use std::fmt::Display;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::html::{Html, PageBody};

const UNEXPECTED_ERROR: &str = "An unexpected error occurred.";

#[cfg(test)]
#[path = "tests/error.rs"]
mod tests;

/// Carried on a failed response so the boundary layer can tell a real error
/// from an ordinary 5xx a handler chose to return.
#[derive(Debug, Clone)]
pub struct ErrorSource(pub String);

/// A page failure.
///
/// Deliberately does not implement [`std::error::Error`], so that
/// `From<E: Error>` stays available for `?`.
#[derive(Debug, Clone)]
pub struct Error {
    status: StatusCode,
    message: String,
    public_message: Option<String>,
    details: Option<serde_json::Value>,
}

impl Error {
    /// An expected failure. Messages on 4xx statuses are public; messages on
    /// 5xx statuses remain private unless explicitly replaced or exposed.
    pub fn new(status: StatusCode, message: impl Display) -> Self {
        let message = message.to_string();
        Error {
            status,
            public_message: (!status.is_server_error()).then(|| message.clone()),
            message,
            details: None,
        }
    }

    /// A private 500 — the usual case for `.map_err(Error::internal)?`.
    ///
    /// The diagnostic is visible in development and server logs, but a
    /// production client receives only a generic message.
    pub fn internal(message: impl Display) -> Self {
        Error::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    /// An error whose message is intentionally safe for every client.
    ///
    /// Prefer [`Error::new`] for expected 4xx failures and [`Error::internal`]
    /// for unexpected failures. This constructor is for deliberate public
    /// 5xx explanations such as a maintenance response.
    pub fn public(status: StatusCode, message: impl Display) -> Self {
        let message = message.to_string();
        Error {
            status,
            public_message: Some(message.clone()),
            message,
            details: None,
        }
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Replace the client-facing text while retaining the original diagnostic
    /// for server logs.
    pub fn with_public_message(mut self, message: impl Display) -> Self {
        self.public_message = Some(message.to_string());
        self
    }

    /// Structured error context for an RPC response. Object fields are merged
    /// beside the wire's `error` message; page boundaries continue to receive
    /// only status and message. Unexpected production failures suppress these
    /// details with their diagnostic message.
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn details(&self) -> Option<&serde_json::Value> {
        self.details.as_ref()
    }

    pub(crate) fn client_message(&self, development: bool) -> &str {
        if development {
            &self.message
        } else {
            self.public_message.as_deref().unwrap_or(UNEXPECTED_ERROR)
        }
    }

    pub(crate) fn client_details(&self, development: bool) -> Option<&serde_json::Value> {
        (development || self.public_message.is_some())
            .then_some(self.details.as_ref())
            .flatten()
    }

    pub(crate) fn record(&self, target: &str) {
        super::dev::record_server(
            if self.status.is_server_error() {
                "error"
            } else {
                "warn"
            },
            target,
            &format!("rahti: {} — {}", self.status, self.message),
        );
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(err: E) -> Self {
        Error::internal(err)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        self.record("page");
        let client_message = self.client_message(super::dev::enabled()).to_string();
        problem_response(self.status, client_message)
    }
}

/// What `error.rs` receives.
///
/// `message` is safe client text. In development it contains the originating
/// diagnostic; unexpected production failures carry only a generic or
/// explicitly supplied public message. The full diagnostic is recorded before
/// this value is built.
#[derive(Debug, Clone)]
pub struct ErrorInfo {
    status: StatusCode,
    message: String,
    request_id: Option<super::observability::RequestId>,
}

impl ErrorInfo {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// The HTTP status as a number, for display.
    pub fn code(&self) -> u16 {
        self.status.as_u16()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    /// Correlation id for a support reference when production observability is
    /// enabled. It is not a credential and may be shown to the user.
    pub fn request_id(&self) -> Option<&super::observability::RequestId> {
        self.request_id.as_ref()
    }

    /// `"Internal Server Error"` and the like — safe to show anyone.
    pub fn reason(&self) -> &'static str {
        self.status.canonical_reason().unwrap_or("Error")
    }
}

/// The page result type. `pub async fn page() -> rahti::Result` is the
/// fallible form of `-> Html`.
pub type Result<T = Html> = std::result::Result<T, Error>;

/// Turn a panic into a response a boundary can render.
///
/// Handed to `CatchPanicLayer::custom` by generated code.
pub fn panic_response(err: Box<dyn Any + Send + 'static>) -> Response {
    let message = if let Some(s) = err.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else {
        "panicked".to_string()
    };

    let error = Error::internal(message);
    error.record("page-panic");
    let client_message = error.client_message(super::dev::enabled()).to_string();
    problem_response(StatusCode::INTERNAL_SERVER_ERROR, client_message)
}

fn problem_response(status: StatusCode, detail: String) -> Response {
    let mut body = serde_json::Map::new();
    body.insert(
        "type".into(),
        serde_json::Value::String("about:blank".into()),
    );
    body.insert(
        "title".into(),
        serde_json::Value::String(status.canonical_reason().unwrap_or("Error").into()),
    );
    body.insert("status".into(), status.as_u16().into());
    body.insert("detail".into(), serde_json::Value::String(detail.clone()));
    if let Some(request_id) = super::observability::request_id() {
        body.insert(
            "requestId".into(),
            serde_json::Value::String(request_id.to_string()),
        );
    }

    let mut response = (status, serde_json::to_vec(&body).unwrap_or_default()).into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/problem+json"),
    );
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response.extensions_mut().insert(ErrorSource(detail));
    response
}

/// Render a failed response through one error boundary.
///
/// Anything that is not a failure passes through untouched, so this layer is
/// inert on every successful request.
pub async fn apply_error(boundary: fn(ErrorInfo) -> Html, res: Response) -> Response {
    // An rpc answers the page's own URL, so it passes under this layer on its
    // way out. The caller is `fetch`, and a page rendered in place of the JSON
    // it asked for is not something it can read.
    if res.extensions().get::<super::rpc::RpcBody>().is_some() {
        return res;
    }

    let status = res.status();

    let message = match res.extensions().get::<ErrorSource>() {
        Some(source) => source.0.clone(),
        // A handler that returned a bare 5xx never went through `Error`, so
        // there is nothing to report beyond the status itself.
        None if status.is_server_error() => {
            status.canonical_reason().unwrap_or("Error").to_string()
        }
        None => return res,
    };

    let (mut parts, _) = res.into_parts();
    let body = boundary(ErrorInfo {
        status,
        message,
        request_id: super::observability::request_id(),
    });

    // Hand back a page: outer layouts wrap this exactly as they would have
    // wrapped the page that failed.
    parts.headers.remove(axum::http::header::CONTENT_LENGTH);
    parts.extensions.remove::<ErrorSource>();
    parts.extensions.insert(PageBody);

    let mut res =
        axum::response::Response::from_parts(parts, axum::body::Body::from(body.into_string()));
    res.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/html; charset=utf-8"),
    );
    res
}
