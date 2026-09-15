//! Bounded, process-local background work.
//!
//! A [`Jobs`] definition owns the handlers and execution policy. Starting it
//! returns a cheap [`JobQueue`] handle. Workers subscribe to Rahti shutdown,
//! stop accepting work, drain the bounded queue, and are then capped by the
//! configured shutdown grace period.

use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{
        Arc, Mutex as StdMutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use futures_util::FutureExt;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, Notify, mpsc, watch},
    task::JoinSet,
    time::{sleep, timeout},
};

type JobFuture = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;
type Handler = Arc<dyn Fn(Value) -> JobFuture + Send + Sync>;

const DEFAULT_CAPACITY: usize = 1_024;
const DEFAULT_WORKERS: usize = 1;
const DEFAULT_ATTEMPTS: u32 = 3;
const DEFAULT_BACKOFF: Duration = Duration::from_millis(250);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);
const DEFAULT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// A background-job definition and its execution policy.
pub struct Jobs {
    handlers: BTreeMap<String, Handler>,
    capacity: usize,
    workers: usize,
    attempts: u32,
    backoff: Duration,
    max_backoff: Duration,
    attempt_timeout: Duration,
    shutdown_grace: Duration,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            handlers: BTreeMap::new(),
            capacity: DEFAULT_CAPACITY,
            workers: DEFAULT_WORKERS,
            attempts: DEFAULT_ATTEMPTS,
            backoff: DEFAULT_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            attempt_timeout: DEFAULT_ATTEMPT_TIMEOUT,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }

    pub fn capacity(mut self, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "rahti: background-job capacity must be greater than zero"
        );
        self.capacity = capacity;
        self
    }

    pub fn workers(mut self, workers: usize) -> Self {
        assert!(
            workers > 0,
            "rahti: background-job worker count must be greater than zero"
        );
        self.workers = workers;
        self
    }

    pub fn attempts(mut self, attempts: u32) -> Self {
        assert!(
            attempts > 0,
            "rahti: background-job attempts must be greater than zero"
        );
        self.attempts = attempts;
        self
    }

    pub fn backoff(mut self, initial: Duration, maximum: Duration) -> Self {
        assert!(
            !initial.is_zero(),
            "rahti: background-job backoff must be greater than zero"
        );
        assert!(
            maximum >= initial,
            "rahti: maximum background-job backoff must not be shorter than the initial backoff"
        );
        self.backoff = initial;
        self.max_backoff = maximum;
        self
    }

    pub fn attempt_timeout(mut self, timeout: Duration) -> Self {
        assert!(
            !timeout.is_zero(),
            "rahti: background-job attempt timeout must be greater than zero"
        );
        self.attempt_timeout = timeout;
        self
    }

    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        assert!(
            !grace.is_zero(),
            "rahti: background-job shutdown grace must be greater than zero"
        );
        self.shutdown_grace = grace;
        self
    }

    /// Register one named handler and its deserialized payload type.
    pub fn handler<T, F, Fut>(mut self, name: impl Into<String>, handler: F) -> Self
    where
        T: DeserializeOwned + Send + 'static,
        F: Fn(T) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        let name = name.into();
        assert!(
            valid_name(&name),
            "rahti: invalid background-job name `{name}`"
        );
        let erased = Arc::new(move |value: Value| -> JobFuture {
            match serde_json::from_value::<T>(value) {
                Ok(payload) => Box::pin(handler(payload)),
                Err(_) => Box::pin(async move {
                    Err(JobError::discard("payload did not match its handler type"))
                }),
            }
        });
        assert!(
            self.handlers.insert(name.clone(), erased).is_none(),
            "rahti: duplicate background-job handler `{name}`"
        );
        self
    }

    /// Start workers on the current Tokio runtime.
    pub fn start(self) -> JobQueue {
        self.start_with_shutdown(true)
    }

    pub(crate) fn start_with_shutdown(self, listen_for_shutdown: bool) -> JobQueue {
        assert!(
            !self.handlers.is_empty(),
            "rahti: start background jobs only after registering a handler"
        );
        tokio::runtime::Handle::try_current()
            .expect("rahti: background jobs must start inside a Tokio runtime");

        let config = Arc::new(Config {
            attempts: self.attempts,
            backoff: self.backoff,
            max_backoff: self.max_backoff,
            attempt_timeout: self.attempt_timeout,
            shutdown_grace: self.shutdown_grace,
        });
        let handlers = Arc::new(self.handlers);
        let (sender, receiver) = mpsc::channel(self.capacity);
        let receiver = Arc::new(Mutex::new(receiver));
        let (stop, stop_receiver) = watch::channel(false);
        let state = Arc::new(State::new(stop));
        let shutdown = listen_for_shutdown.then(crate::watch_shutdown);

        let mut workers = JoinSet::new();
        for _ in 0..self.workers {
            workers.spawn(worker(
                receiver.clone(),
                handlers.clone(),
                config.clone(),
                state.clone(),
            ));
        }
        tokio::spawn(supervise(
            workers,
            config.shutdown_grace,
            state.clone(),
            stop_receiver,
            shutdown,
        ));

        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::downgrade(&state));

        JobQueue {
            sender,
            handlers,
            state,
        }
    }
}

/// A clonable handle used to submit work and inspect this process's counters.
#[derive(Clone)]
pub struct JobQueue {
    sender: mpsc::Sender<Envelope>,
    handlers: Arc<BTreeMap<String, Handler>>,
    state: Arc<State>,
}

impl fmt::Debug for JobQueue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JobQueue")
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .field("metrics", &self.metrics())
            .finish_non_exhaustive()
    }
}

impl JobQueue {
    pub(crate) fn knows(&self, name: &str) -> bool {
        self.handlers.contains_key(name)
    }

    /// Wait for bounded capacity, then enqueue a serializable payload.
    pub async fn enqueue<T: Serialize>(
        &self,
        name: &str,
        payload: T,
    ) -> Result<JobId, EnqueueError> {
        let envelope = self.envelope(name, payload)?;
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        let permit = self
            .sender
            .reserve()
            .await
            .map_err(|_| EnqueueError::Closed)?;
        let _gate = self
            .state
            .acceptance_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        let id = envelope.id.clone();
        self.state.queued.fetch_add(1, Ordering::Relaxed);
        permit.send(envelope);
        Ok(id)
    }

    /// Enqueue immediately or report backpressure without waiting.
    pub fn try_enqueue<T: Serialize>(&self, name: &str, payload: T) -> Result<JobId, EnqueueError> {
        self.try_enqueue_inner(name, payload, None)?
            .ok_or(EnqueueError::Closed)
    }

    /// Enqueue unless a queued or active job already owns `key`.
    ///
    /// The key is released only after the job reaches a terminal outcome.
    /// `Ok(None)` means another job already owns it.
    pub fn try_enqueue_unique<T: Serialize>(
        &self,
        key: &str,
        name: &str,
        payload: T,
    ) -> Result<Option<JobId>, EnqueueError> {
        if !valid_key(key) {
            return Err(EnqueueError::InvalidKey);
        }
        self.try_enqueue_inner(name, payload, Some(key.to_string()))
    }

    fn try_enqueue_inner<T: Serialize>(
        &self,
        name: &str,
        payload: T,
        unique_key: Option<String>,
    ) -> Result<Option<JobId>, EnqueueError> {
        let mut envelope = self.envelope(name, payload)?;
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        let _gate = self
            .state
            .acceptance_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        if let Some(key) = unique_key {
            let mut keys = self
                .state
                .unique_keys
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !keys.insert(key.clone()) {
                return Ok(None);
            }
            envelope.unique_key = Some(key);
        }
        let id = envelope.id.clone();
        self.state.queued.fetch_add(1, Ordering::Relaxed);
        match self.sender.try_send(envelope) {
            Ok(()) => Ok(Some(id)),
            Err(mpsc::error::TrySendError::Full(envelope)) => {
                self.state.queued.fetch_sub(1, Ordering::Relaxed);
                self.state.release_key(&envelope);
                Err(EnqueueError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(envelope)) => {
                self.state.queued.fetch_sub(1, Ordering::Relaxed);
                self.state.release_key(&envelope);
                Err(EnqueueError::Closed)
            }
        }
    }

    fn envelope<T: Serialize>(&self, name: &str, payload: T) -> Result<Envelope, EnqueueError> {
        if !self.handlers.contains_key(name) {
            return Err(EnqueueError::UnknownJob(name.to_string()));
        }
        Ok(Envelope {
            id: JobId::new(),
            name: name.to_string(),
            payload: serde_json::to_value(payload).map_err(EnqueueError::Serialize)?,
            origin_request_id: crate::observability::request_id().map(|id| id.to_string()),
            unique_key: None,
        })
    }

    pub fn metrics(&self) -> JobMetrics {
        self.state.metrics()
    }

    /// Stop accepting work and wait for queued and active jobs to finish.
    pub async fn shutdown(&self) {
        self.state.request_stop();
        self.state.finished().await;
    }
}

/// Ask every queue started in this process to drain and wait for its grace
/// period. Hosts call this after their HTTP server has stopped gracefully.
pub async fn finish_shutdown() {
    crate::schedule::finish_shutdown().await;
    let states = {
        let mut registered = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let states = registered
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        registered.retain(|state| state.strong_count() > 0);
        states
    };
    for state in &states {
        state.request_stop();
    }
    futures_util::future::join_all(states.iter().map(|state| state.finished())).await;
}

fn registry() -> &'static StdMutex<Vec<Weak<State>>> {
    static REGISTRY: OnceLock<StdMutex<Vec<Weak<State>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| StdMutex::new(Vec::new()))
}

#[derive(Clone)]
struct Envelope {
    id: JobId,
    name: String,
    payload: Value,
    origin_request_id: Option<String>,
    unique_key: Option<String>,
}

struct Config {
    attempts: u32,
    backoff: Duration,
    max_backoff: Duration,
    attempt_timeout: Duration,
    shutdown_grace: Duration,
}

struct State {
    acceptance_gate: StdMutex<()>,
    unique_keys: StdMutex<HashSet<String>>,
    accepting: AtomicBool,
    stopping: AtomicBool,
    done: AtomicBool,
    done_notify: Notify,
    stop: watch::Sender<bool>,
    queued: AtomicU64,
    active: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    retried: AtomicU64,
    timed_out: AtomicU64,
    panicked: AtomicU64,
    abandoned: AtomicU64,
}

impl State {
    fn new(stop: watch::Sender<bool>) -> Self {
        Self {
            acceptance_gate: StdMutex::new(()),
            unique_keys: StdMutex::new(HashSet::new()),
            accepting: AtomicBool::new(true),
            stopping: AtomicBool::new(false),
            done: AtomicBool::new(false),
            done_notify: Notify::new(),
            stop,
            queued: AtomicU64::new(0),
            active: AtomicU64::new(0),
            succeeded: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            retried: AtomicU64::new(0),
            timed_out: AtomicU64::new(0),
            panicked: AtomicU64::new(0),
            abandoned: AtomicU64::new(0),
        }
    }

    fn request_stop(&self) {
        let _gate = self
            .acceptance_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.accepting.store(false, Ordering::Release);
        if !self.stopping.swap(true, Ordering::AcqRel) {
            let _ = self.stop.send(true);
        }
    }

    async fn finished(&self) {
        loop {
            let notified = self.done_notify.notified();
            if self.done.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }

    fn metrics(&self) -> JobMetrics {
        JobMetrics {
            queued: self.queued.load(Ordering::Relaxed),
            active: self.active.load(Ordering::Relaxed),
            succeeded: self.succeeded.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            retried: self.retried.load(Ordering::Relaxed),
            timed_out: self.timed_out.load(Ordering::Relaxed),
            panicked: self.panicked.load(Ordering::Relaxed),
            abandoned: self.abandoned.load(Ordering::Relaxed),
        }
    }

    fn release_key(&self, envelope: &Envelope) {
        self.release_unique_key(envelope.unique_key.as_deref());
    }

    fn release_unique_key(&self, key: Option<&str>) {
        if let Some(key) = key {
            self.unique_keys
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(key);
        }
    }
}

async fn supervise(
    mut workers: JoinSet<()>,
    grace: Duration,
    state: Arc<State>,
    mut stop: watch::Receiver<bool>,
    shutdown: Option<crate::Shutdown>,
) {
    if let Some(shutdown) = shutdown {
        tokio::select! {
            _ = shutdown.fired() => state.request_stop(),
            _ = stop.changed() => state.request_stop(),
        }
    } else {
        let _ = stop.changed().await;
        state.request_stop();
    }

    let drained = timeout(grace, async {
        while workers.join_next().await.is_some() {}
    })
    .await
    .is_ok();

    if !drained {
        let abandoned =
            state.queued.swap(0, Ordering::Relaxed) + state.active.load(Ordering::Relaxed);
        state.abandoned.fetch_add(abandoned, Ordering::Relaxed);
        workers.abort_all();
        while workers.join_next().await.is_some() {}
        record(
            "error",
            "background-job shutdown grace expired",
            json!({ "abandoned": abandoned, "graceMs": grace.as_secs_f64() * 1000.0 }),
        );
    }
    state.done.store(true, Ordering::Release);
    state.done_notify.notify_waiters();
}

async fn worker(
    receiver: Arc<Mutex<mpsc::Receiver<Envelope>>>,
    handlers: Arc<BTreeMap<String, Handler>>,
    config: Arc<Config>,
    state: Arc<State>,
) {
    let mut stop = state.stop.subscribe();
    loop {
        let envelope = {
            let mut receiver = receiver.lock().await;
            if state.stopping.load(Ordering::Acquire) && receiver.is_empty() {
                return;
            }
            tokio::select! {
                envelope = receiver.recv() => envelope,
                _ = stop.changed() => continue,
            }
        };
        let Some(envelope) = envelope else { return };
        state.queued.fetch_sub(1, Ordering::Relaxed);
        let _active = ActiveJob::start(&state, &envelope);
        run(envelope, &handlers, &config, &state).await;
    }
}

struct ActiveJob<'a> {
    state: &'a State,
    unique_key: Option<String>,
}

impl<'a> ActiveJob<'a> {
    fn start(state: &'a State, envelope: &Envelope) -> Self {
        state.active.fetch_add(1, Ordering::Relaxed);
        Self {
            state,
            unique_key: envelope.unique_key.clone(),
        }
    }
}

impl Drop for ActiveJob<'_> {
    fn drop(&mut self) {
        self.state.active.fetch_sub(1, Ordering::Relaxed);
        self.state.release_unique_key(self.unique_key.as_deref());
    }
}

async fn run(
    envelope: Envelope,
    handlers: &BTreeMap<String, Handler>,
    config: &Config,
    state: &State,
) {
    let handler = handlers.get(&envelope.name).expect("registered handler");
    for attempt in 1..=config.attempts {
        let context = JobContext {
            id: envelope.id.clone(),
            name: envelope.name.clone(),
            attempt,
            origin_request_id: envelope.origin_request_id.clone(),
        };
        let started = Instant::now();
        let future =
            CURRENT_JOB.scope(context, async { (handler)(envelope.payload.clone()).await });
        let result = timeout(
            config.attempt_timeout,
            AssertUnwindSafe(future).catch_unwind(),
        )
        .await;
        let failure = match result {
            Ok(Ok(Ok(()))) => {
                state.succeeded.fetch_add(1, Ordering::Relaxed);
                record_job(
                    "info",
                    "background job completed",
                    &envelope,
                    attempt,
                    started.elapsed(),
                );
                return;
            }
            Ok(Ok(Err(error))) => Failure::Handler(error),
            Ok(Err(panic)) => {
                state.panicked.fetch_add(1, Ordering::Relaxed);
                Failure::Panic(panic_message(panic))
            }
            Err(_) => {
                state.timed_out.fetch_add(1, Ordering::Relaxed);
                Failure::Timeout
            }
        };

        let retryable = failure.retryable() && attempt < config.attempts;
        if retryable {
            state.retried.fetch_add(1, Ordering::Relaxed);
            record_job(
                "warn",
                &format!("background job attempt failed; retrying: {failure}"),
                &envelope,
                attempt,
                started.elapsed(),
            );
            sleep(backoff(config, attempt)).await;
        } else {
            state.failed.fetch_add(1, Ordering::Relaxed);
            record_job(
                "error",
                &format!("background job failed: {failure}"),
                &envelope,
                attempt,
                started.elapsed(),
            );
            return;
        }
    }
}

fn backoff(config: &Config, failed_attempt: u32) -> Duration {
    let multiplier = 1_u32
        .checked_shl(failed_attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    config
        .backoff
        .saturating_mul(multiplier)
        .min(config.max_backoff)
}

fn record_job(level: &str, message: &str, envelope: &Envelope, attempt: u32, elapsed: Duration) {
    record(
        level,
        message,
        json!({
            "jobId": envelope.id.as_str(),
            "job": envelope.name,
            "attempt": attempt,
            "durationMs": elapsed.as_secs_f64() * 1000.0,
            "originRequestId": envelope.origin_request_id,
        }),
    );
}

fn record(level: &str, message: &str, fields: Value) {
    crate::observability::record_with(level, "jobs", message, fields);
    if crate::dev::enabled() {
        crate::dev::record_server(level, "jobs", message);
    }
}

enum Failure {
    Handler(JobError),
    Timeout,
    Panic(String),
}

impl Failure {
    fn retryable(&self) -> bool {
        !matches!(self, Self::Handler(error) if !error.retryable)
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handler(error) => error.fmt(formatter),
            Self::Timeout => formatter.write_str("attempt timed out"),
            Self::Panic(message) => write!(formatter, "handler panicked: {message}"),
        }
    }
}

fn panic_message(value: Box<dyn std::any::Any + Send>) -> String {
    value
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| value.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 128 && !key.bytes().any(|byte| byte == 0)
}

/// Stable identity for one enqueued job.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct JobId(String);

impl JobId {
    fn new() -> Self {
        let mut bytes = [0_u8; 16];
        if getrandom::fill(&mut bytes).is_ok() {
            return Self(bytes.iter().map(|byte| format!("{byte:02x}")).collect());
        }
        static FALLBACK: AtomicU64 = AtomicU64::new(0);
        let sequence = FALLBACK.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Self(format!("{millis:x}-{sequence:x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Context visible while a handler is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobContext {
    pub id: JobId,
    pub name: String,
    pub attempt: u32,
    pub origin_request_id: Option<String>,
}

tokio::task_local! {
    static CURRENT_JOB: JobContext;
}

pub fn context() -> Option<JobContext> {
    CURRENT_JOB.try_with(Clone::clone).ok()
}

/// A handler failure. Retryable failures use the queue's retry policy;
/// discarded failures stop immediately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobError {
    message: String,
    retryable: bool,
}

impl JobError {
    pub fn retry(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    pub fn discard(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for JobError {}

#[derive(Debug)]
pub enum EnqueueError {
    UnknownJob(String),
    Full,
    Closed,
    InvalidKey,
    Serialize(serde_json::Error),
}

impl fmt::Display for EnqueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownJob(name) => write!(formatter, "unknown background job `{name}`"),
            Self::Full => formatter.write_str("background-job queue is full"),
            Self::Closed => formatter.write_str("background-job queue is shutting down"),
            Self::InvalidKey => formatter.write_str("background-job unique key is invalid"),
            Self::Serialize(error) => write!(
                formatter,
                "background-job payload could not be serialized: {error}"
            ),
        }
    }
}

impl std::error::Error for EnqueueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            _ => None,
        }
    }
}

/// Fixed-cardinality, process-local queue counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobMetrics {
    pub queued: u64,
    pub active: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub retried: u64,
    pub timed_out: u64,
    pub panicked: u64,
    pub abandoned: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Semaphore;

    #[derive(Serialize, Deserialize)]
    struct Payload {
        value: usize,
    }

    #[tokio::test]
    async fn runs_typed_jobs_and_exposes_context() {
        let seen = Arc::new(AtomicUsize::new(0));
        let output = seen.clone();
        let queue = Jobs::new()
            .handler("count", move |payload: Payload| {
                let output = output.clone();
                async move {
                    let context = context().expect("job context");
                    assert_eq!(context.name, "count");
                    assert_eq!(context.attempt, 1);
                    output.fetch_add(payload.value, Ordering::Relaxed);
                    Ok(())
                }
            })
            .start_with_shutdown(false);
        queue.enqueue("count", Payload { value: 7 }).await.unwrap();
        queue.shutdown().await;
        assert_eq!(seen.load(Ordering::Relaxed), 7);
        assert_eq!(queue.metrics().succeeded, 1);
    }

    #[tokio::test]
    async fn retries_retryable_failures_and_discards_permanent_ones() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let seen = attempts.clone();
        let queue = Jobs::new()
            .attempts(3)
            .backoff(Duration::from_millis(1), Duration::from_millis(2))
            .handler("retry", move |_: ()| {
                let seen = seen.clone();
                async move {
                    if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                        Err(JobError::retry("temporary"))
                    } else {
                        Ok(())
                    }
                }
            })
            .handler("discard", |_: ()| async {
                Err(JobError::discard("permanent"))
            })
            .start_with_shutdown(false);
        queue.enqueue("retry", ()).await.unwrap();
        queue.enqueue("discard", ()).await.unwrap();
        queue.shutdown().await;
        let metrics = queue.metrics();
        assert_eq!(metrics.succeeded, 1);
        assert_eq!(metrics.failed, 1);
        assert_eq!(metrics.retried, 1);
    }

    #[tokio::test]
    async fn reports_unknown_jobs_and_closes_on_shutdown() {
        let queue = Jobs::new()
            .handler("known", |_: ()| async { Ok(()) })
            .start_with_shutdown(false);
        assert!(matches!(
            queue.try_enqueue("missing", ()),
            Err(EnqueueError::UnknownJob(_))
        ));
        queue.shutdown().await;
        assert!(matches!(
            queue.try_enqueue("known", ()),
            Err(EnqueueError::Closed)
        ));
    }

    #[tokio::test]
    async fn contains_panics_and_times_out_attempts() {
        let panics = Arc::new(AtomicUsize::new(0));
        let seen = panics.clone();
        let queue = Jobs::new()
            .attempts(2)
            .backoff(Duration::from_millis(1), Duration::from_millis(1))
            .attempt_timeout(Duration::from_millis(5))
            .handler("panic-once", move |_: ()| {
                let seen = seen.clone();
                async move {
                    if seen.fetch_add(1, Ordering::Relaxed) == 0 {
                        panic!("contained");
                    }
                    Ok(())
                }
            })
            .handler("timeout", |_: ()| async {
                sleep(Duration::from_millis(50)).await;
                Ok(())
            })
            .start_with_shutdown(false);
        queue.enqueue("panic-once", ()).await.unwrap();
        queue.enqueue("timeout", ()).await.unwrap();
        queue.shutdown().await;
        let metrics = queue.metrics();
        assert_eq!(metrics.succeeded, 1);
        assert_eq!(metrics.failed, 1);
        assert_eq!(metrics.retried, 2);
        assert_eq!(metrics.panicked, 1);
        assert_eq!(metrics.timed_out, 2);
    }

    #[tokio::test]
    async fn enforces_shutdown_grace() {
        let queue = Jobs::new()
            .attempts(1)
            .shutdown_grace(Duration::from_millis(5))
            .handler("slow", |_: ()| async {
                sleep(Duration::from_secs(1)).await;
                Ok(())
            })
            .start_with_shutdown(false);
        queue.enqueue("slow", ()).await.unwrap();
        queue.shutdown().await;
        let metrics = queue.metrics();
        assert_eq!(metrics.abandoned, 1);
        assert_eq!(metrics.active, 0);
        assert_eq!(metrics.queued, 0);
    }

    #[tokio::test]
    async fn try_enqueue_reports_bounded_backpressure() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let handler_started = started.clone();
        let handler_release = release.clone();
        let queue = Jobs::new()
            .capacity(1)
            .handler("blocked", move |_: ()| {
                let started = handler_started.clone();
                let release = handler_release.clone();
                async move {
                    started.notify_one();
                    let _permit = release.acquire().await.expect("release semaphore");
                    Ok(())
                }
            })
            .start_with_shutdown(false);

        let first_started = started.notified();
        queue.enqueue("blocked", ()).await.unwrap();
        first_started.await;
        queue.enqueue("blocked", ()).await.unwrap();
        assert!(matches!(
            queue.try_enqueue("blocked", ()),
            Err(EnqueueError::Full)
        ));
        release.add_permits(2);
        queue.shutdown().await;
        assert_eq!(queue.metrics().succeeded, 2);
    }
}
