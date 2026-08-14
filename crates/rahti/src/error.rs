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
}

impl Error {
    pub fn new(status: StatusCode, message: impl Display) -> Self {
        Error {
            status,
            message: message.to_string(),
        }
    }

    /// A 500 — the usual case for `.map_err(Error::internal)?`.
    pub fn internal(message: impl Display) -> Self {
        Error::new(StatusCode::INTERNAL_SERVER_ERROR, message)
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(err: E) -> Self {
        Error::internal(err)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        super::dev::record_server(
            "error",
            "page",
            &format!("rahti: {} — {}", self.status, self.message),
        );
        // The body is a placeholder: a boundary replaces it, and without one
        // the status line is all the client gets anyway.
        let mut res = (
            self.status,
            self.status
                .canonical_reason()
                .unwrap_or("Error")
                .to_string(),
        )
            .into_response();
        res.extensions_mut().insert(ErrorSource(self.message));
        res
    }
}

/// What `error.rs` receives.
///
/// `message` is passed through exactly as the page produced it. Whether any
/// of it reaches the browser is the boundary's decision — the full text is
/// already on stderr by the time this is built.
#[derive(Debug, Clone)]
pub struct ErrorInfo {
    status: StatusCode,
    message: String,
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

    super::dev::record_server(
        "error",
        "page-panic",
        &format!("rahti: handler panicked — {message}"),
    );
    Error::internal(message).into_response()
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
    let body = boundary(ErrorInfo { status, message });

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
