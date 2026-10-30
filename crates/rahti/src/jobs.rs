//! Bounded background work, in memory or in a durable store.
//!
//! A [`Jobs`] definition owns the handlers and execution policy. `#[job]`
//! registers typed handlers at link time, and [`Jobs::install`] starts one
//! process-wide queue for them; the function's own `enqueue` reaches that
//! queue. Workers subscribe to Rahti shutdown, stop accepting work, and are
//! capped by the configured shutdown grace period.
//!
//! Without a store, accepted work lives in a bounded channel and is drained on
//! shutdown. With a [`JobStore`], it lives in the store: workers claim it under
//! a renewed lease, so a crash loses nothing and several processes can share
//! one queue. Both paths run one attempt through the same code.

mod store;

pub use store::{
    Claim, ClaimedJob, FailedJob, Inserted, JobCounts, JobStore, JobStoreError, JobStoreFuture,
    MemoryJobStore, NewJob, Terminal, conformance,
};

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
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
use serde_json::{Map, Value, json};
use tokio::{
    sync::{Mutex, Notify, mpsc, watch},
    task::JoinSet,
    time::{interval_at, sleep, timeout},
};

use store::now_ms;

/// What a job handler returns.
pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;
type Handler = Arc<dyn Fn(Value) -> JobFuture + Send + Sync>;

const DEFAULT_CAPACITY: usize = 1_024;
const DEFAULT_STORED_CAPACITY: usize = 10_000;
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DEFAULT_LEASE: Duration = Duration::from_secs(300);
const RECENT_FAILURES: usize = 20;
const DEFAULT_WORKERS: usize = 1;
const DEFAULT_ATTEMPTS: u32 = 3;
const DEFAULT_BACKOFF: Duration = Duration::from_millis(250);
const DEFAULT_MAX_BACKOFF: Duration = Duration::from_secs(30);
const DEFAULT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

/// A background-job definition and its execution policy.
pub struct Jobs {
    handlers: BTreeMap<String, (Handler, Overrides)>,
    store: Option<Arc<dyn JobStore>>,
    capacity: Option<usize>,
    workers: usize,
    attempts: u32,
    backoff: Duration,
    max_backoff: Duration,
    attempt_timeout: Duration,
    shutdown_grace: Duration,
    poll_interval: Duration,
    lease: Duration,
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
            store: None,
            capacity: None,
            workers: DEFAULT_WORKERS,
            attempts: DEFAULT_ATTEMPTS,
            backoff: DEFAULT_BACKOFF,
            max_backoff: DEFAULT_MAX_BACKOFF,
            attempt_timeout: DEFAULT_ATTEMPT_TIMEOUT,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            poll_interval: DEFAULT_POLL_INTERVAL,
            lease: DEFAULT_LEASE,
        }
    }

    /// Keep accepted work in `store` instead of in this process's memory.
    pub fn store(mut self, store: impl JobStore) -> Self {
        self.store = Some(Arc::new(store));
        self
    }

    /// How often idle durable workers look for work another process stored.
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        assert!(
            !interval.is_zero(),
            "rahti: background-job poll interval must be greater than zero"
        );
        self.poll_interval = interval;
        self
    }

    /// How long a durable worker holds a job between renewals. A job whose
    /// worker died runs again once this much time has passed.
    pub fn lease(mut self, lease: Duration) -> Self {
        assert!(
            lease >= Duration::from_secs(1),
            "rahti: background-job lease must be at least one second"
        );
        self.lease = lease;
        self
    }

    /// The bound on queued work: 1,024 in memory, 10,000 in a store.
    pub fn capacity(mut self, capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "rahti: background-job capacity must be greater than zero"
        );
        self.capacity = Some(capacity);
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
        self.register(name, erased, Overrides::default());
        self
    }

    fn register(&mut self, name: String, handler: Handler, overrides: Overrides) {
        assert!(
            self.handlers
                .insert(name.clone(), (handler, overrides))
                .is_none(),
            "rahti: duplicate background-job handler `{name}`"
        );
    }

    /// Start workers for every `#[job]` linked into this binary and make this
    /// the queue their `enqueue` reaches.
    ///
    /// Call it once from `initialize_application`. A project with no jobs
    /// starts nothing, and a static export starts nothing: rendering a page
    /// must not run side effects. Installing again replaces the queue, which
    /// is what a test binary with one runtime per test needs.
    pub fn install(mut self) -> Option<JobQueue> {
        if crate::static_output().is_some() {
            return None;
        }
        for entry in inventory::iter::<JobEntry> {
            self.register(
                entry.name.to_string(),
                Arc::new(entry.handler),
                Overrides {
                    attempts: entry.attempts,
                    backoff: entry.backoff_ms.map(Duration::from_millis),
                    max_backoff: entry.max_backoff_ms.map(Duration::from_millis),
                    attempt_timeout: entry.timeout_secs.map(Duration::from_secs),
                },
            );
        }
        if self.handlers.is_empty() {
            return None;
        }
        let queue = self.start();
        let previous = installed()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(queue.clone());
        if let Some(previous) = previous {
            previous.state.request_stop();
        }
        Some(queue)
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

        let defaults = Policy {
            attempts: self.attempts,
            backoff: self.backoff,
            max_backoff: self.max_backoff,
            attempt_timeout: self.attempt_timeout,
        };
        let handlers = Arc::new(
            self.handlers
                .into_iter()
                .map(|(name, (handler, overrides))| {
                    let policy = defaults.with(overrides);
                    (name, Registered { handler, policy })
                })
                .collect::<BTreeMap<_, _>>(),
        );
        let (stop, stop_receiver) = watch::channel(false);
        let state = Arc::new(State::new(stop));
        let shutdown = listen_for_shutdown.then(crate::watch_shutdown);

        let mut workers = JoinSet::new();
        let backend = match self.store {
            None => {
                let capacity = self.capacity.unwrap_or(DEFAULT_CAPACITY);
                let (sender, receiver) = mpsc::channel(capacity);
                let receiver = Arc::new(Mutex::new(receiver));
                for _ in 0..self.workers {
                    workers.spawn(worker(receiver.clone(), handlers.clone(), state.clone()));
                }
                Backend::Memory(sender)
            }
            Some(store) => {
                let durable = Arc::new(Durable {
                    store,
                    wake: Notify::new(),
                    capacity: self.capacity.unwrap_or(DEFAULT_STORED_CAPACITY),
                    poll_interval: self.poll_interval,
                    lease: self.lease,
                    leased: StdMutex::new(HashMap::new()),
                    names: handlers.keys().cloned().collect(),
                });
                let process = JobId::new();
                for index in 0..self.workers {
                    workers.spawn(durable_worker(
                        format!("{process}-{index}"),
                        durable.clone(),
                        handlers.clone(),
                        state.clone(),
                    ));
                }
                Backend::Durable(durable)
            }
        };
        tokio::spawn(supervise(
            workers,
            self.shutdown_grace,
            state.clone(),
            backend.durable(),
            stop_receiver,
            shutdown,
        ));

        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::downgrade(&state));

        JobQueue {
            backend,
            handlers,
            state,
        }
    }
}

/// A clonable handle used to submit work and inspect this process's counters.
#[derive(Clone)]
pub struct JobQueue {
    backend: Backend,
    handlers: Arc<BTreeMap<String, Registered>>,
    state: Arc<State>,
}

#[derive(Clone)]
enum Backend {
    Memory(mpsc::Sender<Envelope>),
    Durable(Arc<Durable>),
}

impl Backend {
    fn durable(&self) -> Option<Arc<Durable>> {
        match self {
            Self::Memory(_) => None,
            Self::Durable(durable) => Some(durable.clone()),
        }
    }
}

struct Durable {
    store: Arc<dyn JobStore>,
    wake: Notify,
    capacity: usize,
    poll_interval: Duration,
    lease: Duration,
    /// What this process holds, so a grace expiry can hand it back.
    leased: StdMutex<HashMap<String, String>>,
    names: Vec<String>,
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
    ///
    /// A durable queue never waits: at capacity it answers
    /// [`EnqueueError::Full`], because the bound belongs to a shared store.
    pub async fn enqueue<T: Serialize>(
        &self,
        name: &str,
        payload: T,
    ) -> Result<JobId, EnqueueError> {
        self.submit(self.job(name, payload)?).await
    }

    /// Enqueue immediately or report backpressure without waiting.
    pub async fn try_enqueue<T: Serialize>(
        &self,
        name: &str,
        payload: T,
    ) -> Result<JobId, EnqueueError> {
        self.submit_now(self.job(name, payload)?)
            .await?
            .ok_or(EnqueueError::Closed)
    }

    /// Enqueue unless a queued or active job already owns `key`.
    ///
    /// The key is released only after the job reaches a terminal outcome.
    /// `Ok(None)` means another job already owns it.
    pub async fn try_enqueue_unique<T: Serialize>(
        &self,
        key: &str,
        name: &str,
        payload: T,
    ) -> Result<Option<JobId>, EnqueueError> {
        self.submit_now(self.job(name, payload)?.unique(key)).await
    }

    fn job<T: Serialize>(&self, name: &str, payload: T) -> Result<NewJob, EnqueueError> {
        if !self.handlers.contains_key(name) {
            return Err(EnqueueError::UnknownJob(name.to_string()));
        }
        NewJob::new(name, payload).map_err(EnqueueError::Serialize)
    }

    /// Accept a built job, waiting for capacity on a memory queue.
    pub async fn submit(&self, job: NewJob) -> Result<JobId, EnqueueError> {
        self.check(&job)?;
        let Backend::Memory(sender) = &self.backend else {
            return self.submit_now(job).await?.ok_or(EnqueueError::Closed);
        };
        if job.unique_key.is_some() {
            return self.submit_now(job).await?.ok_or(EnqueueError::Closed);
        }
        let permit = sender.reserve().await.map_err(|_| EnqueueError::Closed)?;
        let _gate = self
            .state
            .acceptance_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        let id = JobId(job.id.clone());
        self.state.queued.fetch_add(1, Ordering::Relaxed);
        permit.send(job.into());
        Ok(id)
    }

    /// Accept a built job or report backpressure. `Ok(None)` means another
    /// job already owns its unique key.
    pub async fn submit_now(&self, job: NewJob) -> Result<Option<JobId>, EnqueueError> {
        self.check(&job)?;
        match &self.backend {
            Backend::Memory(sender) => self.submit_to_memory(sender, job),
            Backend::Durable(durable) => self.submit_to_store(durable, job).await,
        }
    }

    fn check(&self, job: &NewJob) -> Result<(), EnqueueError> {
        if !self.handlers.contains_key(&job.name) {
            return Err(EnqueueError::UnknownJob(job.name.clone()));
        }
        if job.unique_key.as_deref().is_some_and(|key| !valid_key(key)) {
            return Err(EnqueueError::InvalidKey);
        }
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        Ok(())
    }

    fn submit_to_memory(
        &self,
        sender: &mpsc::Sender<Envelope>,
        job: NewJob,
    ) -> Result<Option<JobId>, EnqueueError> {
        let _gate = self
            .state
            .acceptance_gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !self.state.accepting.load(Ordering::Acquire) {
            return Err(EnqueueError::Closed);
        }
        if let Some(key) = &job.unique_key {
            let mut keys = self
                .state
                .unique_keys
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !keys.insert(key.clone()) {
                return Ok(None);
            }
        }
        let id = JobId(job.id.clone());
        self.state.queued.fetch_add(1, Ordering::Relaxed);
        match sender.try_send(job.into()) {
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

    async fn submit_to_store(
        &self,
        durable: &Durable,
        job: NewJob,
    ) -> Result<Option<JobId>, EnqueueError> {
        let counts = durable.store.counts().await.map_err(EnqueueError::Store)?;
        if counts.pending + counts.running >= durable.capacity as u64 {
            return Err(EnqueueError::Full);
        }
        let id = JobId(job.id.clone());
        match durable
            .store
            .insert(job)
            .await
            .map_err(EnqueueError::Store)?
        {
            Inserted::Accepted => {
                durable.wake.notify_one();
                Ok(Some(id))
            }
            Inserted::Duplicate => Ok(None),
        }
    }

    pub fn metrics(&self) -> JobMetrics {
        self.state.metrics()
    }

    /// What this queue is doing, for diagnostics. Payloads never appear here.
    pub async fn status(&self) -> JobStatus {
        let (stored, recent_failures, store_error) = match self.backend.durable() {
            None => (None, self.state.failures(), None),
            Some(durable) => {
                let limit = RECENT_FAILURES as u32;
                match (
                    durable.store.counts().await,
                    durable.store.failures(limit).await,
                ) {
                    (Ok(counts), Ok(failures)) => (Some(counts), failures, None),
                    (Err(error), _) | (_, Err(error)) => {
                        (None, Vec::new(), Some(error.to_string()))
                    }
                }
            }
        };
        JobStatus {
            durable: self.backend.durable().is_some(),
            handlers: self.handlers.keys().cloned().collect(),
            metrics: self.metrics(),
            stored,
            recent_failures,
            store_error,
        }
    }

    /// Stop accepting work and wait for queued and active jobs to finish.
    pub async fn shutdown(&self) {
        self.state.request_stop();
        self.state.finished().await;
    }
}

/// What [`JobQueue::status`] reports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatus {
    pub durable: bool,
    pub handlers: Vec<String>,
    pub metrics: JobMetrics,
    /// Rows by state, when a store holds the work.
    pub stored: Option<JobCounts>,
    /// The newest jobs that ended without succeeding.
    pub recent_failures: Vec<FailedJob>,
    pub store_error: Option<String>,
}

/// One `#[job]`, submitted at link time and started by [`Jobs::install`].
#[doc(hidden)]
pub struct JobEntry {
    pub name: &'static str,
    pub source: &'static str,
    pub attempts: Option<u32>,
    pub timeout_secs: Option<u64>,
    pub backoff_ms: Option<u64>,
    pub max_backoff_ms: Option<u64>,
    pub handler: fn(Value) -> JobFuture,
}

inventory::collect!(JobEntry);

fn installed() -> &'static StdMutex<Option<JobQueue>> {
    static INSTALLED: OnceLock<StdMutex<Option<JobQueue>>> = OnceLock::new();
    INSTALLED.get_or_init(|| StdMutex::new(None))
}

/// The queue [`Jobs::install`] started.
pub fn queue() -> Result<JobQueue, EnqueueError> {
    installed()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
        .ok_or(EnqueueError::NotStarted)
}

/// The installed queue's status, when one is running.
pub async fn status() -> Option<JobStatus> {
    match queue() {
        Ok(queue) => Some(queue.status().await),
        Err(_) => None,
    }
}

/// What a `#[job]` function may return. Anything that is not success is a
/// retry, unless it is a [`JobError::discard`].
pub trait IntoJobResult {
    fn into_job_result(self) -> Result<(), JobError>;
}

impl IntoJobResult for () {
    fn into_job_result(self) -> Result<(), JobError> {
        Ok(())
    }
}

impl IntoJobResult for Result<(), JobError> {
    fn into_job_result(self) -> Result<(), JobError> {
        self
    }
}

impl IntoJobResult for crate::Result<()> {
    fn into_job_result(self) -> Result<(), JobError> {
        self.map_err(|error| JobError::retry(error.message))
    }
}

/// The argument object a `#[job]` stored, or a refusal. A payload that no
/// longer matches its function can never succeed, so it is never retried.
#[doc(hidden)]
pub fn arguments(payload: Value, job: &str) -> Result<Map<String, Value>, JobError> {
    match payload {
        Value::Object(arguments) => Ok(arguments),
        _ => Err(JobError::discard(format!(
            "job `{job}` expected an object of named arguments"
        ))),
    }
}

/// One parameter of a `#[job]`, read by its own name. An absent key reads as
/// `null`, so an added `Option` parameter accepts jobs stored before it.
#[doc(hidden)]
pub fn argument<T: DeserializeOwned>(
    arguments: &Map<String, Value>,
    job: &str,
    name: &str,
) -> Result<T, JobError> {
    let value = arguments.get(name).cloned().unwrap_or(Value::Null);
    serde_json::from_value(value).map_err(|_| {
        JobError::discard(format!(
            "job `{job}` could not read its `{name}` argument from the stored payload"
        ))
    })
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
    crate::dev::retire();
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

impl From<NewJob> for Envelope {
    fn from(job: NewJob) -> Self {
        Self {
            id: JobId(job.id),
            name: job.name,
            payload: job.payload,
            origin_request_id: job.origin_request_id,
            unique_key: job.unique_key,
        }
    }
}

/// How one job runs: the queue's defaults under its `#[job]` overrides.
#[derive(Clone, Copy)]
struct Policy {
    attempts: u32,
    backoff: Duration,
    max_backoff: Duration,
    attempt_timeout: Duration,
}

impl Policy {
    fn with(self, overrides: Overrides) -> Self {
        let backoff = overrides.backoff.unwrap_or(self.backoff);
        Self {
            attempts: overrides.attempts.unwrap_or(self.attempts),
            backoff,
            max_backoff: overrides
                .max_backoff
                .unwrap_or(self.max_backoff)
                .max(backoff),
            attempt_timeout: overrides.attempt_timeout.unwrap_or(self.attempt_timeout),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Overrides {
    attempts: Option<u32>,
    backoff: Option<Duration>,
    max_backoff: Option<Duration>,
    attempt_timeout: Option<Duration>,
}

struct Registered {
    handler: Handler,
    policy: Policy,
}

struct State {
    acceptance_gate: StdMutex<()>,
    unique_keys: StdMutex<HashSet<String>>,
    failures: StdMutex<VecDeque<FailedJob>>,
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
            failures: StdMutex::new(VecDeque::new()),
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

    /// Keep a memory job's failure for diagnostics; a store keeps its own.
    fn remember_failure(&self, failure: FailedJob) {
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        failures.push_front(failure);
        failures.truncate(RECENT_FAILURES);
    }

    fn failures(&self) -> Vec<FailedJob> {
        self.failures
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
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
    durable: Option<Arc<Durable>>,
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
        if let Some(durable) = durable {
            release_leased(&durable).await;
        }
        record(
            "error",
            "background-job shutdown grace expired",
            json!({ "abandoned": abandoned, "graceMs": grace.as_secs_f64() * 1000.0 }),
        );
    }
    state.done.store(true, Ordering::Release);
    state.done_notify.notify_waiters();
}

/// Hand back what aborted workers held, so the next start runs it again
/// without spending an attempt. A lease expiry would recover it anyway.
async fn release_leased(durable: &Durable) {
    let leased = std::mem::take(
        &mut *durable
            .leased
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
    for (id, owner) in leased {
        if let Err(error) = durable.store.release(&id, &owner).await {
            store_failed("release", &error);
        }
    }
}

fn store_failed(operation: &str, error: &JobStoreError) {
    record(
        "error",
        &format!("background-job store {operation} failed: {error}"),
        json!({}),
    );
}

async fn worker(
    receiver: Arc<Mutex<mpsc::Receiver<Envelope>>>,
    handlers: Arc<BTreeMap<String, Registered>>,
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
        run(envelope, &handlers, &state).await;
    }
}

/// Claim and run stored jobs until shutdown. Stored work stays stored, so
/// stopping means finishing what this worker holds and claiming nothing more.
async fn durable_worker(
    owner: String,
    durable: Arc<Durable>,
    handlers: Arc<BTreeMap<String, Registered>>,
    state: Arc<State>,
) {
    let mut stop = state.stop.subscribe();
    let lease = i64::try_from(durable.lease.as_millis()).unwrap_or(i64::MAX);
    loop {
        if state.stopping.load(Ordering::Acquire) {
            return;
        }
        let now = now_ms();
        let claim = Claim {
            owner: &owner,
            names: &durable.names,
            now_ms: now,
            lease_until_ms: now.saturating_add(lease),
        };
        match durable.store.claim(claim).await {
            Ok(Some(job)) => run_stored(job, &owner, &durable, &handlers, &state).await,
            Ok(None) => {
                tokio::select! {
                    _ = durable.wake.notified() => {}
                    _ = sleep(durable.poll_interval) => {}
                    _ = stop.changed() => {}
                }
            }
            Err(error) => {
                store_failed("claim", &error);
                tokio::select! {
                    _ = sleep(durable.poll_interval) => {}
                    _ = stop.changed() => {}
                }
            }
        }
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

/// One attempt, the only place a handler is called: its context, its timeout,
/// and panic containment are the same in memory and in a store.
async fn attempt_once(
    envelope: &Envelope,
    attempt: u32,
    registered: &Registered,
    state: &State,
) -> Result<(), Failure> {
    let context = JobContext {
        id: envelope.id.clone(),
        name: envelope.name.clone(),
        attempt,
        origin_request_id: envelope.origin_request_id.clone(),
    };
    let future = CURRENT_JOB.scope(context, async {
        (registered.handler)(envelope.payload.clone()).await
    });
    let result = timeout(
        registered.policy.attempt_timeout,
        AssertUnwindSafe(future).catch_unwind(),
    )
    .await;
    match result {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(error))) => Err(Failure::Handler(error)),
        Ok(Err(panic)) => {
            state.panicked.fetch_add(1, Ordering::Relaxed);
            Err(Failure::Panic(panic_message(panic)))
        }
        Err(_) => {
            state.timed_out.fetch_add(1, Ordering::Relaxed);
            Err(Failure::Timeout)
        }
    }
}

fn job_succeeded(state: &State, envelope: &Envelope, attempt: u32, elapsed: Duration) {
    state.succeeded.fetch_add(1, Ordering::Relaxed);
    record_job(
        "info",
        "background job completed",
        envelope,
        attempt,
        elapsed,
    );
}

fn job_retrying(
    state: &State,
    envelope: &Envelope,
    attempt: u32,
    failure: &Failure,
    elapsed: Duration,
) {
    state.retried.fetch_add(1, Ordering::Relaxed);
    record_job(
        "warn",
        &format!("background job attempt failed; retrying: {failure}"),
        envelope,
        attempt,
        elapsed,
    );
}

fn job_failed(
    state: &State,
    envelope: &Envelope,
    attempt: u32,
    failure: &Failure,
    elapsed: Duration,
) -> FailedJob {
    state.failed.fetch_add(1, Ordering::Relaxed);
    record_job(
        "error",
        &format!("background job failed: {failure}"),
        envelope,
        attempt,
        elapsed,
    );
    FailedJob {
        id: envelope.id.to_string(),
        name: envelope.name.clone(),
        outcome: failure.terminal(),
        attempt,
        error: failure.to_string(),
        at_ms: now_ms(),
    }
}

async fn run(envelope: Envelope, handlers: &BTreeMap<String, Registered>, state: &State) {
    let registered = handlers.get(&envelope.name).expect("registered handler");
    let policy = registered.policy;
    for attempt in 1..=policy.attempts {
        let started = Instant::now();
        let failure = match attempt_once(&envelope, attempt, registered, state).await {
            Ok(()) => {
                job_succeeded(state, &envelope, attempt, started.elapsed());
                return;
            }
            Err(failure) => failure,
        };

        if failure.retryable() && attempt < policy.attempts {
            job_retrying(state, &envelope, attempt, &failure, started.elapsed());
            sleep(backoff(&policy, attempt)).await;
        } else {
            let failed = job_failed(state, &envelope, attempt, &failure, started.elapsed());
            state.remember_failure(failed);
            return;
        }
    }
}

/// One attempt of a stored job under a renewed lease, then settle the row. A
/// failed settle is logged and left to the lease: the job runs again, which
/// every retryable handler already has to tolerate.
async fn run_stored(
    job: ClaimedJob,
    owner: &str,
    durable: &Durable,
    handlers: &BTreeMap<String, Registered>,
    state: &State,
) {
    let attempt = job.attempt;
    let envelope = Envelope {
        id: JobId(job.id),
        name: job.name,
        payload: job.payload,
        origin_request_id: job.origin_request_id,
        unique_key: None,
    };
    let id = envelope.id.as_str();
    let registered = handlers.get(&envelope.name).expect("claimed by name");
    let policy = registered.policy;

    // The attempts ran out while a worker died holding the last one.
    if attempt > policy.attempts {
        let failed = job_failed(
            state,
            &envelope,
            attempt - 1,
            &Failure::Abandoned,
            Duration::ZERO,
        );
        settle(
            durable
                .store
                .finish(id, owner, failed.outcome, &failed.error, failed.at_ms),
            "finish",
        )
        .await;
        return;
    }

    durable
        .leased
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id.to_string(), owner.to_string());
    let _active = ActiveJob::start(state, &envelope);
    let started = Instant::now();
    let renew_every = durable.lease / 3;
    let lease = i64::try_from(durable.lease.as_millis()).unwrap_or(i64::MAX);
    let mut renewal = interval_at(tokio::time::Instant::now() + renew_every, renew_every);
    let result = {
        let running = attempt_once(&envelope, attempt, registered, state);
        tokio::pin!(running);
        loop {
            tokio::select! {
                result = &mut running => break Some(result),
                _ = renewal.tick() => {
                    let until = now_ms().saturating_add(lease);
                    match durable.store.renew(id, owner, until).await {
                        Ok(true) => {}
                        // Another worker owns it now, so its outcome is theirs.
                        Ok(false) => break None,
                        Err(error) => store_failed("renew", &error),
                    }
                }
            }
        }
    };
    durable
        .leased
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(id);

    let Some(result) = result else {
        record_job(
            "warn",
            "background job lost its lease; another worker owns it",
            &envelope,
            attempt,
            started.elapsed(),
        );
        return;
    };
    match result {
        Ok(()) => {
            job_succeeded(state, &envelope, attempt, started.elapsed());
            settle(durable.store.complete(id, owner), "complete").await;
        }
        Err(failure) if failure.retryable() && attempt < policy.attempts => {
            job_retrying(state, &envelope, attempt, &failure, started.elapsed());
            let delay = i64::try_from(backoff(&policy, attempt).as_millis()).unwrap_or(i64::MAX);
            let due = now_ms().saturating_add(delay);
            let error = failure.to_string();
            settle(
                durable.store.reschedule(id, owner, due, &error),
                "reschedule",
            )
            .await;
        }
        Err(failure) => {
            let failed = job_failed(state, &envelope, attempt, &failure, started.elapsed());
            settle(
                durable
                    .store
                    .finish(id, owner, failed.outcome, &failed.error, failed.at_ms),
                "finish",
            )
            .await;
        }
    }
}

async fn settle(future: JobStoreFuture<'_, ()>, operation: &str) {
    if let Err(error) = future.await {
        store_failed(operation, &error);
    }
}

fn backoff(policy: &Policy, failed_attempt: u32) -> Duration {
    let multiplier = 1_u32
        .checked_shl(failed_attempt.saturating_sub(1))
        .unwrap_or(u32::MAX);
    policy
        .backoff
        .saturating_mul(multiplier)
        .min(policy.max_backoff)
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
    Abandoned,
}

impl Failure {
    fn retryable(&self) -> bool {
        !matches!(self, Self::Handler(error) if !error.retryable)
    }

    fn terminal(&self) -> Terminal {
        if self.retryable() {
            Terminal::Failed
        } else {
            Terminal::Discarded
        }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Handler(error) => error.fmt(formatter),
            Self::Timeout => formatter.write_str("attempt timed out"),
            Self::Panic(message) => write!(formatter, "handler panicked: {message}"),
            Self::Abandoned => {
                formatter.write_str("attempts used up: a worker stopped during the last attempt")
            }
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
    Store(JobStoreError),
    NotStarted,
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
            Self::Store(error) => write!(formatter, "background-job store failed: {error}"),
            Self::NotStarted => formatter.write_str(
                "no background-job queue is running: call \
                 `rahti::jobs::Jobs::new().install()` in `initialize_application`",
            ),
        }
    }
}

impl std::error::Error for EnqueueError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            Self::Store(error) => Some(error),
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
            queue.try_enqueue("missing", ()).await,
            Err(EnqueueError::UnknownJob(_))
        ));
        queue.shutdown().await;
        assert!(matches!(
            queue.try_enqueue("known", ()).await,
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
            queue.try_enqueue("blocked", ()).await,
            Err(EnqueueError::Full)
        ));
        release.add_permits(2);
        queue.shutdown().await;
        assert_eq!(queue.metrics().succeeded, 2);
    }

    #[tokio::test]
    async fn a_memory_queue_reports_its_failures_without_payloads() {
        let queue = Jobs::new()
            .attempts(1)
            .handler("refuse", |_: Payload| async {
                Err(JobError::discard("no such order"))
            })
            .start_with_shutdown(false);
        queue.enqueue("refuse", Payload { value: 7 }).await.unwrap();
        queue.shutdown().await;
        let status = queue.status().await;
        assert!(!status.durable);
        assert_eq!(status.recent_failures[0].error, "no such order");
        assert_eq!(status.recent_failures[0].outcome, Terminal::Discarded);
    }

    /// A store shared by the two "processes" a test starts in turn.
    #[derive(Clone, Default)]
    struct Shared(Arc<MemoryJobStore>);

    impl JobStore for Shared {
        fn insert(&self, job: NewJob) -> JobStoreFuture<'_, Inserted> {
            self.0.insert(job)
        }
        fn claim<'a>(&'a self, claim: Claim<'a>) -> JobStoreFuture<'a, Option<ClaimedJob>> {
            self.0.claim(claim)
        }
        fn renew<'a>(
            &'a self,
            id: &'a str,
            owner: &'a str,
            lease_until_ms: i64,
        ) -> JobStoreFuture<'a, bool> {
            self.0.renew(id, owner, lease_until_ms)
        }
        fn complete<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()> {
            self.0.complete(id, owner)
        }
        fn reschedule<'a>(
            &'a self,
            id: &'a str,
            owner: &'a str,
            run_at_ms: i64,
            error: &'a str,
        ) -> JobStoreFuture<'a, ()> {
            self.0.reschedule(id, owner, run_at_ms, error)
        }
        fn finish<'a>(
            &'a self,
            id: &'a str,
            owner: &'a str,
            outcome: Terminal,
            error: &'a str,
            now_ms: i64,
        ) -> JobStoreFuture<'a, ()> {
            self.0.finish(id, owner, outcome, error, now_ms)
        }
        fn release<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()> {
            self.0.release(id, owner)
        }
        fn counts(&self) -> JobStoreFuture<'_, JobCounts> {
            self.0.counts()
        }
        fn failures(&self, limit: u32) -> JobStoreFuture<'_, Vec<FailedJob>> {
            self.0.failures(limit)
        }
        fn purge(&self, before_ms: i64) -> JobStoreFuture<'_, u64> {
            self.0.purge(before_ms)
        }
    }

    async fn eventually(check: impl AsyncFn() -> bool) {
        for _ in 0..500 {
            if check().await {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
        panic!("the condition never held");
    }

    #[tokio::test]
    async fn a_durable_queue_runs_retries_and_keeps_the_failure() {
        let store = Shared::default();
        let seen = Arc::new(AtomicUsize::new(0));
        let attempts = seen.clone();
        let queue = Jobs::new()
            .store(store.clone())
            .attempts(2)
            .backoff(Duration::from_millis(1), Duration::from_millis(1))
            .poll_interval(Duration::from_millis(5))
            .handler("count", move |_: Payload| {
                let attempts = attempts.clone();
                async move {
                    let context = context().expect("job context");
                    attempts.fetch_add(1, Ordering::Relaxed);
                    Err(JobError::retry(format!("attempt {}", context.attempt)))
                }
            })
            .start_with_shutdown(false);
        queue.enqueue("count", Payload { value: 1 }).await.unwrap();

        eventually(async || store.counts().await.unwrap().failed == 1).await;
        queue.shutdown().await;

        assert_eq!(seen.load(Ordering::Relaxed), 2);
        let status = queue.status().await;
        assert!(status.durable);
        assert_eq!(status.metrics.retried, 1);
        assert_eq!(status.recent_failures[0].error, "attempt 2");
        assert_eq!(status.recent_failures[0].attempt, 2);
    }

    #[tokio::test]
    async fn stored_work_survives_the_queue_that_accepted_it() {
        let store = Shared::default();
        store
            .insert(NewJob::new("count", Payload { value: 3 }).unwrap())
            .await
            .unwrap();

        let seen = Arc::new(AtomicUsize::new(0));
        let output = seen.clone();
        let queue = Jobs::new()
            .store(store.clone())
            .poll_interval(Duration::from_millis(5))
            .handler("count", move |payload: Payload| {
                let output = output.clone();
                async move {
                    output.fetch_add(payload.value, Ordering::Relaxed);
                    Ok(())
                }
            })
            .start_with_shutdown(false);
        eventually(async || store.counts().await.unwrap().pending == 0).await;
        queue.shutdown().await;
        assert_eq!(seen.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn a_job_for_a_missing_handler_stays_pending() {
        let store = Shared::default();
        store
            .insert(NewJob::new("from-the-next-release", ()).unwrap())
            .await
            .unwrap();
        let queue = Jobs::new()
            .store(store.clone())
            .poll_interval(Duration::from_millis(1))
            .handler("known", |_: ()| async { Ok(()) })
            .start_with_shutdown(false);
        sleep(Duration::from_millis(20)).await;
        queue.shutdown().await;
        assert_eq!(store.counts().await.unwrap().pending, 1);
    }

    #[tokio::test]
    async fn a_dead_workers_job_runs_again_and_spends_its_attempts() {
        let store = Shared::default();
        let job = NewJob::new("count", ()).unwrap();
        let id = job.id.clone();
        store.insert(job).await.unwrap();
        let names = ["count".to_string()];
        let now = now_ms();
        store
            .claim(Claim {
                owner: "dead",
                names: &names,
                now_ms: now,
                lease_until_ms: now,
            })
            .await
            .unwrap()
            .unwrap();

        let queue = Jobs::new()
            .store(store.clone())
            .attempts(1)
            .poll_interval(Duration::from_millis(5))
            .handler("count", |_: ()| async { Ok(()) })
            .start_with_shutdown(false);
        eventually(async || store.counts().await.unwrap().failed == 1).await;
        queue.shutdown().await;

        let failure = &store.failures(1).await.unwrap()[0];
        assert_eq!(failure.id, id);
        assert_eq!(failure.outcome, Terminal::Failed);
        assert!(
            failure.error.contains("attempts used up"),
            "{}",
            failure.error
        );
    }

    #[tokio::test]
    async fn a_grace_expiry_returns_running_work_without_spending_it() {
        let store = Shared::default();
        let started = Arc::new(Notify::new());
        let running = started.clone();
        let queue = Jobs::new()
            .store(store.clone())
            .shutdown_grace(Duration::from_millis(5))
            .poll_interval(Duration::from_millis(5))
            .handler("slow", move |_: ()| {
                let running = running.clone();
                async move {
                    running.notify_one();
                    sleep(Duration::from_secs(60)).await;
                    Ok(())
                }
            })
            .start_with_shutdown(false);
        queue.enqueue("slow", ()).await.unwrap();
        started.notified().await;
        queue.shutdown().await;

        let counts = store.counts().await.unwrap();
        assert_eq!((counts.pending, counts.running), (1, 0));
        assert_eq!(queue.metrics().abandoned, 1);
    }

    #[tokio::test]
    async fn a_durable_queue_is_bounded_and_honours_unique_keys() {
        let store = Shared::default();
        let queue = Jobs::new()
            .store(store.clone())
            .capacity(1)
            .poll_interval(Duration::from_secs(60))
            .handler("keyed", |_: ()| async { Ok(()) })
            .start_with_shutdown(false);
        store
            .insert(NewJob::new("unclaimed", ()).unwrap().unique("held"))
            .await
            .unwrap();
        assert!(matches!(
            queue.try_enqueue("keyed", ()).await,
            Err(EnqueueError::Full)
        ));
        queue.shutdown().await;

        let queue = Jobs::new()
            .store(store.clone())
            .poll_interval(Duration::from_secs(60))
            .handler("keyed", |_: ()| async { Ok(()) })
            .start_with_shutdown(false);
        assert_eq!(
            queue.try_enqueue_unique("held", "keyed", ()).await.unwrap(),
            None
        );
        queue.shutdown().await;
    }

    #[test]
    fn arguments_refuse_rather_than_retry_a_stale_payload() {
        let refused = arguments(json!([1]), "send").unwrap_err();
        assert!(!refused.is_retryable());

        let object = arguments(json!({ "id": 1 }), "send").unwrap();
        assert_eq!(argument::<i64>(&object, "send", "id").unwrap(), 1);
        assert_eq!(
            argument::<Option<i64>>(&object, "send", "later").unwrap(),
            None
        );
        let refused = argument::<String>(&object, "send", "id").unwrap_err();
        assert!(!refused.is_retryable());
        assert!(refused.to_string().contains("`id`"));
    }
}
