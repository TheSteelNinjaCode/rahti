//! Ordered application middleware for matched dynamic requests.
//!
//! Rahti's generated router places this chain inside authentication and
//! outside handlers, layouts, page caching, and RPC dispatch. Framework-owned
//! CSRF, rate limiting, and security-header layers remain outside it.

use std::fmt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, RwLock};

use axum::extract::{Request, State};
use axum::middleware::Next as AxumNext;
use axum::response::Response;
use futures_util::FutureExt;

type ResponseFuture = Pin<Box<dyn Future<Output = Response> + Send + 'static>>;
type Step = dyn Fn(Request, Next) -> ResponseFuture + Send + Sync + 'static;

/// An ordered chain of application-defined request middleware.
///
/// Steps run in registration order on the way to the handler and unwind in
/// reverse order on the response. A step may return a response without
/// calling [`Next::run`] to stop dispatch deliberately.
#[derive(Clone, Default)]
pub struct RequestPipeline {
    steps: Arc<[Arc<Step>]>,
}

impl RequestPipeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one asynchronous middleware function.
    pub fn with<F, Fut>(self, middleware: F) -> Self
    where
        F: Fn(Request, Next) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let mut steps = self.steps.iter().cloned().collect::<Vec<_>>();
        steps.push(Arc::new(move |request, next| {
            Box::pin(middleware(request, next))
        }));
        Self {
            steps: steps.into(),
        }
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

impl fmt::Debug for RequestPipeline {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RequestPipeline")
            .field("steps", &self.steps.len())
            .finish()
    }
}

/// The remainder of the application chain.
///
/// Pass the request to [`run`](Self::run) exactly once. Middleware owns the
/// request while it runs, so it can change headers and extensions before
/// continuing, then inspect or change the returned response.
pub struct Next {
    steps: Arc<[Arc<Step>]>,
    index: usize,
    terminal: Option<AxumNext>,
}

impl Next {
    pub async fn run(mut self, request: Request) -> Response {
        if let Some(step) = self.steps.get(self.index).cloned() {
            self.index += 1;
            return step(request, self).await;
        }

        self.terminal
            .take()
            .expect("rahti: request-pipeline next called after the terminal handler")
            .run(request)
            .await
    }
}

impl fmt::Debug for Next {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Next")
            .field("remaining", &self.steps.len().saturating_sub(self.index))
            .finish_non_exhaustive()
    }
}

fn installed() -> &'static RwLock<RequestPipeline> {
    static INSTALLED: OnceLock<RwLock<RequestPipeline>> = OnceLock::new();
    INSTALLED.get_or_init(|| RwLock::new(RequestPipeline::new()))
}

/// Install the complete application pipeline.
///
/// Build the generated router only after this call. Each router snapshots the
/// installed chain, so replacing it later does not mutate an already-running
/// application.
pub fn configure(pipeline: RequestPipeline) {
    match installed().write() {
        Ok(mut current) => *current = pipeline,
        Err(poisoned) => *poisoned.into_inner() = pipeline,
    }
}

/// Snapshot the installed chain for a generated dynamic-route group.
#[doc(hidden)]
pub fn state() -> RequestPipeline {
    installed()
        .read()
        .map(|pipeline| pipeline.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

/// Generated middleware entry point.
#[doc(hidden)]
pub async fn middleware(
    State(pipeline): State<RequestPipeline>,
    request: Request,
    next: AxumNext,
) -> Response {
    let rpc = request.headers().contains_key("x-pp-function");
    let dispatch = Next {
        steps: pipeline.steps,
        index: 0,
        terminal: Some(next),
    }
    .run(request);

    match AssertUnwindSafe(dispatch).catch_unwind().await {
        Ok(response) => response,
        Err(panic) if rpc => crate::rpc_panic_response(panic),
        Err(panic) => crate::panic_response(panic),
    }
}

#[cfg(test)]
#[path = "tests/request_pipeline.rs"]
mod tests;
