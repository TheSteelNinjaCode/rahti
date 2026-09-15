//! Process-local scheduling for named background jobs.
//!
//! Scheduling decides when a job is submitted. [`crate::jobs`] remains the
//! only execution path and therefore keeps ownership of queue bounds,
//! retries, timeouts, job context, and shutdown drain.

use std::{
    collections::HashSet,
    fmt,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::{DateTime, TimeZone as _, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::{
    sync::{Notify, watch},
    task::JoinSet,
    time::sleep,
};

use crate::jobs::{EnqueueError, JobQueue};

/// A set of schedules that submit work to one background-job queue.
pub struct Scheduler {
    queue: JobQueue,
    entries: Vec<Entry>,
    names: HashSet<String>,
}

impl Scheduler {
    pub fn new(queue: JobQueue) -> Self {
        Self {
            queue,
            entries: Vec::new(),
            names: HashSet::new(),
        }
    }

    /// Submit once per interval, with the first run one interval after start.
    /// A still-pending previous occurrence suppresses the next one.
    pub fn every<T: Serialize>(
        self,
        schedule: impl Into<String>,
        interval: Duration,
        job: impl Into<String>,
        payload: T,
    ) -> Result<Self, ScheduleError> {
        if interval.is_zero() {
            return Err(ScheduleError::InvalidInterval);
        }
        self.add(schedule, job, payload, Cadence::Every(interval), false)
    }

    /// The same interval schedule, allowing several occurrences to be queued
    /// or running at once.
    pub fn every_allowing_overlap<T: Serialize>(
        self,
        schedule: impl Into<String>,
        interval: Duration,
        job: impl Into<String>,
        payload: T,
    ) -> Result<Self, ScheduleError> {
        if interval.is_zero() {
            return Err(ScheduleError::InvalidInterval);
        }
        self.add(schedule, job, payload, Cadence::Every(interval), true)
    }

    /// Submit once at the named instant. An instant already in the past runs
    /// promptly after the scheduler starts.
    pub fn once<T: Serialize>(
        self,
        schedule: impl Into<String>,
        at: SystemTime,
        job: impl Into<String>,
        payload: T,
    ) -> Result<Self, ScheduleError> {
        self.add(schedule, job, payload, Cadence::Once(at), false)
    }

    /// Submit each day at an explicit UTC hour and minute.
    pub fn daily_utc<T: Serialize>(
        self,
        schedule: impl Into<String>,
        hour: u8,
        minute: u8,
        job: impl Into<String>,
        payload: T,
    ) -> Result<Self, ScheduleError> {
        if hour > 23 || minute > 59 {
            return Err(ScheduleError::InvalidUtcTime { hour, minute });
        }
        self.add(
            schedule,
            job,
            payload,
            Cadence::Daily {
                zone: chrono_tz::UTC,
                hour,
                minute,
            },
            false,
        )
    }

    /// Submit each local calendar day in the configured application timezone.
    pub fn daily<T: Serialize>(
        self,
        schedule: impl Into<String>,
        hour: u8,
        minute: u8,
        job: impl Into<String>,
        payload: T,
    ) -> Result<Self, ScheduleError> {
        if hour > 23 || minute > 59 {
            return Err(ScheduleError::InvalidLocalTime { hour, minute });
        }
        self.add(
            schedule,
            job,
            payload,
            Cadence::Daily {
                zone: crate::time::timezone(),
                hour,
                minute,
            },
            false,
        )
    }

    fn add<T: Serialize>(
        mut self,
        schedule: impl Into<String>,
        job: impl Into<String>,
        payload: T,
        cadence: Cadence,
        allow_overlap: bool,
    ) -> Result<Self, ScheduleError> {
        let schedule = schedule.into();
        if !valid_name(&schedule) {
            return Err(ScheduleError::InvalidName(schedule));
        }
        if !self.names.insert(schedule.clone()) {
            return Err(ScheduleError::Duplicate(schedule));
        }
        let job = job.into();
        if !self.queue.knows(&job) {
            return Err(ScheduleError::UnknownJob(job));
        }
        let payload = serde_json::to_value(payload).map_err(ScheduleError::Serialize)?;
        self.entries.push(Entry {
            schedule,
            job,
            payload,
            cadence,
            allow_overlap,
        });
        Ok(self)
    }

    /// Start all timers on the current Tokio runtime.
    pub fn start(self) -> SchedulerHandle {
        self.start_with_shutdown(true)
    }

    fn start_with_shutdown(self, listen_for_shutdown: bool) -> SchedulerHandle {
        assert!(
            !self.entries.is_empty(),
            "rahti: start task scheduling only after adding a schedule"
        );
        tokio::runtime::Handle::try_current()
            .expect("rahti: task scheduling must start inside a Tokio runtime");

        let (stop, _) = watch::channel(false);
        let state = Arc::new(State::new(stop));
        let mut tasks = JoinSet::new();
        for entry in self.entries {
            tasks.spawn(run_entry(
                self.queue.clone(),
                entry,
                state.clone(),
                state.stop.subscribe(),
                listen_for_shutdown.then(crate::watch_shutdown),
            ));
        }
        let completed = state.clone();
        tokio::spawn(async move {
            while tasks.join_next().await.is_some() {}
            completed.done.store(true, Ordering::Release);
            completed.done_notify.notify_waiters();
        });

        registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Arc::downgrade(&state));
        SchedulerHandle { state }
    }
}

/// A running set of schedule timers.
#[derive(Clone)]
pub struct SchedulerHandle {
    state: Arc<State>,
}

impl fmt::Debug for SchedulerHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SchedulerHandle")
            .field("metrics", &self.metrics())
            .finish_non_exhaustive()
    }
}

impl SchedulerHandle {
    pub fn metrics(&self) -> ScheduleMetrics {
        self.state.metrics()
    }

    /// Stop future dispatches and wait for all timer tasks to finish.
    pub async fn shutdown(&self) {
        self.state.stop();
        self.state.finished().await;
    }
}

/// Stop every scheduler in the process. Background-job shutdown calls this
/// before closing queues so no timer can race the drain boundary.
pub async fn finish_shutdown() {
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
        state.stop();
    }
    futures_util::future::join_all(states.iter().map(|state| state.finished())).await;
}

fn registry() -> &'static Mutex<Vec<Weak<State>>> {
    static REGISTRY: OnceLock<Mutex<Vec<Weak<State>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Clone)]
struct Entry {
    schedule: String,
    job: String,
    payload: Value,
    cadence: Cadence,
    allow_overlap: bool,
}

#[derive(Clone)]
enum Cadence {
    Every(Duration),
    Once(SystemTime),
    Daily {
        zone: chrono_tz::Tz,
        hour: u8,
        minute: u8,
    },
}

impl Cadence {
    fn first(&self, now: SystemTime) -> SystemTime {
        match self {
            Self::Every(interval) => now.checked_add(*interval).unwrap_or(now),
            Self::Once(at) => *at,
            Self::Daily { zone, hour, minute } => next_daily(now, *zone, *hour, *minute),
        }
    }

    fn next(&self, previous: SystemTime, now: SystemTime) -> Option<SystemTime> {
        match self {
            Self::Once(_) => None,
            Self::Daily { zone, hour, minute } => {
                Some(next_daily_after(previous, *zone, *hour, *minute))
            }
            // Fixed delay from the observation time deliberately coalesces
            // missed occurrences. A sleeping process must not replay a burst.
            Self::Every(interval) => Some(now.checked_add(*interval).unwrap_or(now)),
        }
    }
}

async fn run_entry(
    queue: JobQueue,
    entry: Entry,
    state: Arc<State>,
    mut stop: watch::Receiver<bool>,
    shutdown: Option<crate::Shutdown>,
) {
    let shutdown = async move {
        match shutdown {
            Some(shutdown) => shutdown.fired().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(shutdown);
    let mut due = entry.cadence.first(SystemTime::now());
    loop {
        let wait = due.duration_since(SystemTime::now()).unwrap_or_default();
        tokio::select! {
            biased;
            _ = stop.changed() => return,
            _ = &mut shutdown => return,
            _ = sleep(wait) => {}
        }

        dispatch(&queue, &entry, &state);
        let now = SystemTime::now();
        let Some(next) = entry.cadence.next(due, now) else {
            return;
        };
        due = next;
    }
}

fn dispatch(queue: &JobQueue, entry: &Entry, state: &State) {
    state.due.fetch_add(1, Ordering::Relaxed);
    let result = if entry.allow_overlap {
        queue
            .try_enqueue(&entry.job, entry.payload.clone())
            .map(Some)
    } else {
        queue.try_enqueue_unique(
            &format!("schedule:{}", entry.schedule),
            &entry.job,
            entry.payload.clone(),
        )
    };

    match result {
        Ok(Some(job_id)) => {
            state.enqueued.fetch_add(1, Ordering::Relaxed);
            record(
                "info",
                "scheduled job enqueued",
                json!({ "schedule": entry.schedule, "job": entry.job, "jobId": job_id.as_str() }),
            );
        }
        Ok(None) => {
            state.skipped_overlap.fetch_add(1, Ordering::Relaxed);
            record(
                "warn",
                "scheduled occurrence skipped because the previous job is pending",
                json!({ "schedule": entry.schedule, "job": entry.job }),
            );
        }
        Err(EnqueueError::Full) => {
            state.queue_full.fetch_add(1, Ordering::Relaxed);
            record(
                "warn",
                "scheduled occurrence skipped because the job queue is full",
                json!({ "schedule": entry.schedule, "job": entry.job }),
            );
        }
        Err(error) => {
            state.failed.fetch_add(1, Ordering::Relaxed);
            record(
                "error",
                &format!("scheduled occurrence could not be enqueued: {error}"),
                json!({ "schedule": entry.schedule, "job": entry.job }),
            );
        }
    }
}

fn next_daily(now: SystemTime, zone: chrono_tz::Tz, hour: u8, minute: u8) -> SystemTime {
    let now_utc: DateTime<Utc> = now.into();
    let mut day = now_utc.with_timezone(&zone).date_naive();
    for _ in 0..=2 {
        if let Some(candidate) = crate::time::resolve_local(zone, day, hour, minute)
            && candidate > now_utc
        {
            return candidate.into();
        }
        day = day.succ_opt().unwrap_or(day);
    }
    now.checked_add(Duration::from_secs(24 * 60 * 60))
        .unwrap_or(now)
}

fn next_daily_after(previous: SystemTime, zone: chrono_tz::Tz, hour: u8, minute: u8) -> SystemTime {
    let previous_utc: DateTime<Utc> = previous.into();
    let mut day = previous_utc
        .with_timezone(&zone)
        .date_naive()
        .succ_opt()
        .unwrap_or_else(|| previous_utc.date_naive());
    for _ in 0..=2 {
        if let Some(candidate) = crate::time::resolve_local(zone, day, hour, minute) {
            return candidate.into();
        }
        day = day.succ_opt().unwrap_or(day);
    }
    previous
        .checked_add(Duration::from_secs(24 * 60 * 60))
        .unwrap_or(previous)
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn record(level: &str, message: &str, fields: Value) {
    crate::observability::record_with(level, "schedule", message, fields);
    if crate::dev::enabled() {
        crate::dev::record_server(level, "schedule", message);
    }
}

struct State {
    stopping: AtomicBool,
    stop: watch::Sender<bool>,
    done: AtomicBool,
    done_notify: Notify,
    due: AtomicU64,
    enqueued: AtomicU64,
    skipped_overlap: AtomicU64,
    queue_full: AtomicU64,
    failed: AtomicU64,
}

impl State {
    fn new(stop: watch::Sender<bool>) -> Self {
        Self {
            stopping: AtomicBool::new(false),
            stop,
            done: AtomicBool::new(false),
            done_notify: Notify::new(),
            due: AtomicU64::new(0),
            enqueued: AtomicU64::new(0),
            skipped_overlap: AtomicU64::new(0),
            queue_full: AtomicU64::new(0),
            failed: AtomicU64::new(0),
        }
    }

    fn stop(&self) {
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

    fn metrics(&self) -> ScheduleMetrics {
        ScheduleMetrics {
            due: self.due.load(Ordering::Relaxed),
            enqueued: self.enqueued.load(Ordering::Relaxed),
            skipped_overlap: self.skipped_overlap.load(Ordering::Relaxed),
            queue_full: self.queue_full.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

/// Fixed-cardinality, process-local schedule counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleMetrics {
    pub due: u64,
    pub enqueued: u64,
    pub skipped_overlap: u64,
    pub queue_full: u64,
    pub failed: u64,
}

#[derive(Debug)]
pub enum ScheduleError {
    InvalidName(String),
    Duplicate(String),
    UnknownJob(String),
    InvalidInterval,
    InvalidUtcTime { hour: u8, minute: u8 },
    InvalidLocalTime { hour: u8, minute: u8 },
    Serialize(serde_json::Error),
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "invalid schedule name `{name}`"),
            Self::Duplicate(name) => write!(formatter, "duplicate schedule `{name}`"),
            Self::UnknownJob(name) => write!(formatter, "unknown scheduled job `{name}`"),
            Self::InvalidInterval => {
                formatter.write_str("schedule interval must be greater than zero")
            }
            Self::InvalidUtcTime { hour, minute } => {
                write!(formatter, "invalid UTC schedule time {hour:02}:{minute:02}")
            }
            Self::InvalidLocalTime { hour, minute } => {
                write!(
                    formatter,
                    "invalid local schedule time {hour:02}:{minute:02}"
                )
            }
            Self::Serialize(error) => write!(
                formatter,
                "scheduled payload could not be serialized: {error}"
            ),
        }
    }
}

impl std::error::Error for ScheduleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::{JobError, Jobs};
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::{Notify, Semaphore};

    #[tokio::test]
    async fn interval_schedules_dispatch_through_the_job_queue() {
        let count = Arc::new(AtomicUsize::new(0));
        let ran = Arc::new(Notify::new());
        let output = count.clone();
        let signal = ran.clone();
        let queue = Jobs::new()
            .handler("tick", move |value: usize| {
                let output = output.clone();
                let signal = signal.clone();
                async move {
                    output.fetch_add(value, Ordering::Relaxed);
                    signal.notify_one();
                    Ok(())
                }
            })
            .start_with_shutdown(false);
        let scheduler = Scheduler::new(queue.clone())
            .every("clock", Duration::from_millis(5), "tick", 2)
            .unwrap()
            .start_with_shutdown(false);

        ran.notified().await;
        scheduler.shutdown().await;
        queue.shutdown().await;
        assert!(count.load(Ordering::Relaxed) >= 2);
        assert!(scheduler.metrics().enqueued >= 1);
    }

    #[tokio::test]
    async fn overlap_is_suppressed_until_the_previous_job_finishes() {
        let started = Arc::new(Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let handler_started = started.clone();
        let handler_release = release.clone();
        let queue = Jobs::new()
            .handler("slow", move |_: ()| {
                let started = handler_started.clone();
                let release = handler_release.clone();
                async move {
                    started.notify_one();
                    let _permit = release.acquire().await.expect("release semaphore");
                    Ok::<_, JobError>(())
                }
            })
            .start_with_shutdown(false);
        let scheduler = Scheduler::new(queue.clone())
            .every("single", Duration::from_millis(2), "slow", ())
            .unwrap()
            .start_with_shutdown(false);

        let first = started.notified();
        first.await;
        sleep(Duration::from_millis(50)).await;
        assert!(scheduler.metrics().skipped_overlap > 0);
        release.add_permits(1);
        scheduler.shutdown().await;
        queue.shutdown().await;
        assert_eq!(queue.metrics().succeeded, 1);
    }

    #[tokio::test]
    async fn a_past_one_shot_runs_once() {
        let count = Arc::new(AtomicUsize::new(0));
        let output = count.clone();
        let queue = Jobs::new()
            .handler("once", move |_: ()| {
                let output = output.clone();
                async move {
                    output.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                }
            })
            .start_with_shutdown(false);
        let scheduler = Scheduler::new(queue.clone())
            .once("startup", UNIX_EPOCH, "once", ())
            .unwrap()
            .start_with_shutdown(false);
        while scheduler.metrics().due == 0 {
            tokio::task::yield_now().await;
        }
        scheduler.shutdown().await;
        queue.shutdown().await;
        assert_eq!(count.load(Ordering::Relaxed), 1);
        assert_eq!(scheduler.metrics().due, 1);
    }

    #[test]
    fn daily_utc_moves_to_the_next_matching_minute() {
        let now = UNIX_EPOCH + Duration::from_secs(12 * 3_600 + 30 * 60);
        assert_eq!(
            next_daily(now, chrono_tz::UTC, 13, 15),
            UNIX_EPOCH + Duration::from_secs(13 * 3_600 + 15 * 60)
        );
        assert_eq!(
            next_daily(now, chrono_tz::UTC, 12, 30),
            now + Duration::from_secs(24 * 60 * 60)
        );
    }

    #[test]
    fn application_daily_time_tracks_dst() {
        let zone = "America/New_York".parse().unwrap();
        let before_spring: SystemTime = Utc.with_ymd_and_hms(2026, 3, 7, 13, 0, 0).unwrap().into();
        let first = next_daily(before_spring, zone, 8, 30);
        let second = next_daily_after(first, zone, 8, 30);
        let elapsed = second.duration_since(first).unwrap();
        assert_eq!(elapsed, Duration::from_secs(23 * 60 * 60));
    }

    #[tokio::test]
    async fn invalid_and_unknown_definitions_fail_before_start() {
        let queue = Jobs::new()
            .handler("known", |_: ()| async { Ok(()) })
            .start_with_shutdown(false);
        assert!(matches!(
            Scheduler::new(queue.clone()).every("bad", Duration::ZERO, "known", ()),
            Err(ScheduleError::InvalidInterval)
        ));
        assert!(matches!(
            Scheduler::new(queue.clone()).daily_utc("bad", 24, 0, "known", ()),
            Err(ScheduleError::InvalidUtcTime { .. })
        ));
        assert!(matches!(
            Scheduler::new(queue.clone()).once("bad", SystemTime::now(), "missing", ()),
            Err(ScheduleError::UnknownJob(_))
        ));
        queue.shutdown().await;
    }
}
