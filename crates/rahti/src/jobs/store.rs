//! Where accepted jobs wait when they have to outlive the process.
//!
//! A [`JobStore`] is the contract between the queue and a database. The queue
//! owns policy — which handler runs, how long a lease is, when a retry is
//! due — and a store owns only the row: it inserts, hands a due job to exactly
//! one worker, and records how that job ended. Times are Unix milliseconds
//! passed in by the caller, so an adapter needs no clock of its own and every
//! backend compares the same integers.
//!
//! Rahti ships [`MemoryJobStore`] for tests and no database adapter: the
//! framework depends on no ORM. `cargo rahti generate job-store` writes a
//! SeaORM adapter into the application, and [`conformance`] proves any adapter
//! keeps this contract.

use std::{
    collections::HashMap,
    fmt,
    future::Future,
    pin::Pin,
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;

use super::JobId;

/// The future every [`JobStore`] operation returns.
pub type JobStoreFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, JobStoreError>> + Send + 'a>>;

/// A store could not answer. The message enters diagnostics, so it names what
/// failed and never carries a payload or a connection string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobStoreError(pub String);

impl JobStoreError {
    pub fn new(message: impl fmt::Display) -> Self {
        Self(message.to_string())
    }
}

impl fmt::Display for JobStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for JobStoreError {}

/// A job accepted for execution, before any queue or store has seen it.
#[derive(Clone, Debug, PartialEq)]
pub struct NewJob {
    pub id: String,
    pub name: String,
    pub payload: Value,
    pub origin_request_id: Option<String>,
    pub unique_key: Option<String>,
    /// Unix milliseconds before which no worker may claim it.
    pub run_at_ms: i64,
}

impl NewJob {
    /// A job due now, carrying the current request id when there is one.
    pub fn new<T: Serialize>(name: &str, payload: T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id: JobId::new().0,
            name: name.to_string(),
            payload: serde_json::to_value(payload)?,
            origin_request_id: crate::observability::request_id().map(|id| id.to_string()),
            unique_key: None,
            run_at_ms: now_ms(),
        })
    }

    /// Refuse this job while another job holding `key` is pending or running.
    pub fn unique(mut self, key: impl Into<String>) -> Self {
        self.unique_key = Some(key.into());
        self
    }
}

/// What a store answers when it is handed a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inserted {
    Accepted,
    /// Another pending or running job already holds the unique key.
    Duplicate,
}

/// One worker asking for its next job.
#[derive(Clone, Copy, Debug)]
pub struct Claim<'a> {
    /// Identifies the worker; only this owner may settle what it claims.
    pub owner: &'a str,
    /// Handlers this process has. A job for any other name stays pending, so
    /// a rolling deployment never fails work the old version still owns.
    pub names: &'a [String],
    pub now_ms: i64,
    pub lease_until_ms: i64,
}

/// A job handed to one worker. `attempt` is one-based and counts this claim,
/// so an attempt a crash interrupted still counts.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaimedJob {
    pub id: String,
    pub name: String,
    pub payload: Value,
    pub origin_request_id: Option<String>,
    pub attempt: u32,
}

/// How a job that will not run again ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Terminal {
    /// Every attempt was used.
    Failed,
    /// A handler or policy refused it permanently.
    Discarded,
}

impl Terminal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Discarded => "discarded",
        }
    }
}

/// Rows by state, as a store counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCounts {
    pub pending: u64,
    pub running: u64,
    pub failed: u64,
    pub discarded: u64,
}

/// A job that ended without succeeding. It never carries the payload: this is
/// what diagnostics and agent tooling read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailedJob {
    pub id: String,
    pub name: String,
    pub outcome: Terminal,
    pub attempt: u32,
    pub error: String,
    pub at_ms: i64,
}

/// Durable job storage. Every method is safe to call from several processes
/// at once, and [`conformance`] states the behavior in executable form.
pub trait JobStore: Send + Sync + 'static {
    /// Store a job, or answer [`Inserted::Duplicate`] when its unique key is
    /// held by a pending or running job.
    fn insert(&self, job: NewJob) -> JobStoreFuture<'_, Inserted>;

    /// Atomically take the earliest due job whose name is in `claim.names`: a
    /// pending job at or past its `run_at_ms`, or a running job whose lease
    /// has expired. Increments its attempt and records the lease and owner.
    fn claim<'a>(&'a self, claim: Claim<'a>) -> JobStoreFuture<'a, Option<ClaimedJob>>;

    /// Extend a lease. `false` means this owner no longer holds the job.
    fn renew<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        lease_until_ms: i64,
    ) -> JobStoreFuture<'a, bool>;

    /// A successful job is removed. Ignored unless `owner` holds it.
    fn complete<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()>;

    /// Return a failed attempt to pending, due at `run_at_ms`, recording the
    /// error. Ignored unless `owner` holds it.
    fn reschedule<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        run_at_ms: i64,
        error: &'a str,
    ) -> JobStoreFuture<'a, ()>;

    /// Keep a finished job for inspection and release its unique key. Ignored
    /// unless `owner` holds it.
    fn finish<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        outcome: Terminal,
        error: &'a str,
        now_ms: i64,
    ) -> JobStoreFuture<'a, ()>;

    /// Return a running job to pending without spending its attempt. Graceful
    /// shutdown does this. Ignored unless `owner` holds it.
    fn release<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()>;

    fn counts(&self) -> JobStoreFuture<'_, JobCounts>;

    /// The most recently finished failed or discarded jobs, newest first.
    fn failures(&self, limit: u32) -> JobStoreFuture<'_, Vec<FailedJob>>;

    /// Delete failed and discarded jobs that finished before `before_ms`,
    /// returning how many went.
    fn purge(&self, before_ms: i64) -> JobStoreFuture<'_, u64>;
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[derive(Clone, Debug)]
enum RowState {
    Pending,
    Running { owner: String, lease_until_ms: i64 },
    Ended { outcome: Terminal, at_ms: i64 },
}

#[derive(Clone, Debug)]
struct Row {
    job: NewJob,
    attempt: u32,
    state: RowState,
    error: Option<String>,
}

/// An in-process [`JobStore`]: the reference implementation of the contract
/// and a test double. It survives a queue restart, not a process restart.
#[derive(Debug, Default)]
pub struct MemoryJobStore {
    rows: Mutex<HashMap<String, Row>>,
}

impl MemoryJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with<T>(&self, change: impl FnOnce(&mut HashMap<String, Row>) -> T) -> T {
        change(&mut self.rows.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn held<'a>(rows: &'a mut HashMap<String, Row>, id: &str, owner: &str) -> Option<&'a mut Row> {
        rows.get_mut(id).filter(
            |row| matches!(&row.state, RowState::Running { owner: holder, .. } if holder == owner),
        )
    }
}

impl JobStore for MemoryJobStore {
    fn insert(&self, job: NewJob) -> JobStoreFuture<'_, Inserted> {
        Box::pin(async move {
            Ok(self.with(|rows| {
                let taken = job.unique_key.as_ref().is_some_and(|key| {
                    rows.values().any(|row| {
                        row.job.unique_key.as_ref() == Some(key)
                            && !matches!(row.state, RowState::Ended { .. })
                    })
                });
                if taken {
                    return Inserted::Duplicate;
                }
                rows.insert(
                    job.id.clone(),
                    Row {
                        job,
                        attempt: 0,
                        state: RowState::Pending,
                        error: None,
                    },
                );
                Inserted::Accepted
            }))
        })
    }

    fn claim<'a>(&'a self, claim: Claim<'a>) -> JobStoreFuture<'a, Option<ClaimedJob>> {
        Box::pin(async move {
            Ok(self.with(|rows| {
                let row = rows
                    .values_mut()
                    .filter(|row| claim.names.contains(&row.job.name))
                    .filter(|row| match &row.state {
                        RowState::Pending => row.job.run_at_ms <= claim.now_ms,
                        RowState::Running { lease_until_ms, .. } => *lease_until_ms <= claim.now_ms,
                        RowState::Ended { .. } => false,
                    })
                    .min_by_key(|row| row.job.run_at_ms)?;
                row.attempt += 1;
                row.state = RowState::Running {
                    owner: claim.owner.to_string(),
                    lease_until_ms: claim.lease_until_ms,
                };
                Some(ClaimedJob {
                    id: row.job.id.clone(),
                    name: row.job.name.clone(),
                    payload: row.job.payload.clone(),
                    origin_request_id: row.job.origin_request_id.clone(),
                    attempt: row.attempt,
                })
            }))
        })
    }

    fn renew<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        lease_until_ms: i64,
    ) -> JobStoreFuture<'a, bool> {
        Box::pin(async move {
            Ok(self.with(|rows| {
                let Some(row) = Self::held(rows, id, owner) else {
                    return false;
                };
                row.state = RowState::Running {
                    owner: owner.to_string(),
                    lease_until_ms,
                };
                true
            }))
        })
    }

    fn complete<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            self.with(|rows| {
                if Self::held(rows, id, owner).is_some() {
                    rows.remove(id);
                }
            });
            Ok(())
        })
    }

    fn reschedule<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        run_at_ms: i64,
        error: &'a str,
    ) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            self.with(|rows| {
                if let Some(row) = Self::held(rows, id, owner) {
                    row.state = RowState::Pending;
                    row.job.run_at_ms = run_at_ms;
                    row.error = Some(error.to_string());
                }
            });
            Ok(())
        })
    }

    fn finish<'a>(
        &'a self,
        id: &'a str,
        owner: &'a str,
        outcome: Terminal,
        error: &'a str,
        now_ms: i64,
    ) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            self.with(|rows| {
                if let Some(row) = Self::held(rows, id, owner) {
                    row.state = RowState::Ended {
                        outcome,
                        at_ms: now_ms,
                    };
                    row.error = Some(error.to_string());
                }
            });
            Ok(())
        })
    }

    fn release<'a>(&'a self, id: &'a str, owner: &'a str) -> JobStoreFuture<'a, ()> {
        Box::pin(async move {
            self.with(|rows| {
                if let Some(row) = Self::held(rows, id, owner) {
                    row.state = RowState::Pending;
                    row.attempt = row.attempt.saturating_sub(1);
                }
            });
            Ok(())
        })
    }

    fn counts(&self) -> JobStoreFuture<'_, JobCounts> {
        Box::pin(async move {
            Ok(self.with(|rows| {
                let mut counts = JobCounts::default();
                for row in rows.values() {
                    match row.state {
                        RowState::Pending => counts.pending += 1,
                        RowState::Running { .. } => counts.running += 1,
                        RowState::Ended {
                            outcome: Terminal::Failed,
                            ..
                        } => counts.failed += 1,
                        RowState::Ended {
                            outcome: Terminal::Discarded,
                            ..
                        } => counts.discarded += 1,
                    }
                }
                counts
            }))
        })
    }

    fn failures(&self, limit: u32) -> JobStoreFuture<'_, Vec<FailedJob>> {
        Box::pin(async move {
            Ok(self.with(|rows| {
                let mut failures = rows
                    .values()
                    .filter_map(|row| match row.state {
                        RowState::Ended { outcome, at_ms } => Some(FailedJob {
                            id: row.job.id.clone(),
                            name: row.job.name.clone(),
                            outcome,
                            attempt: row.attempt,
                            error: row.error.clone().unwrap_or_default(),
                            at_ms,
                        }),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                failures.sort_by_key(|failure| std::cmp::Reverse(failure.at_ms));
                failures.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
                failures
            }))
        })
    }

    fn purge(&self, before_ms: i64) -> JobStoreFuture<'_, u64> {
        Box::pin(async move {
            Ok(self.with(|rows| {
                let before = rows.len();
                rows.retain(|_, row| {
                    !matches!(row.state, RowState::Ended { at_ms, .. } if at_ms < before_ms)
                });
                (before - rows.len()) as u64
            }))
        })
    }
}

/// Prove that `store` keeps the [`JobStore`] contract, panicking on the first
/// broken rule. Run it from a test against an empty store: it uses the job
/// name `rahti.conformance`, and its last step purges finished jobs.
pub async fn conformance<S: JobStore>(store: &S) {
    const NAME: &str = "rahti.conformance";
    const LEASE: i64 = 60_000;

    fn job(run_at_ms: i64) -> NewJob {
        NewJob {
            run_at_ms,
            ..NewJob::new(NAME, serde_json::json!({ "value": 7 })).expect("a JSON payload")
        }
    }
    async fn take<S: JobStore>(
        store: &S,
        names: &[String],
        owner: &str,
        now_ms: i64,
    ) -> Option<ClaimedJob> {
        store
            .claim(Claim {
                owner,
                names,
                now_ms,
                lease_until_ms: now_ms + LEASE,
            })
            .await
            .expect("claim")
    }

    let names = vec![NAME.to_string()];
    let now = now_ms();

    // Claiming: once, intact, and only when due and named.
    let first = job(now);
    assert_eq!(
        store.insert(first.clone()).await.expect("insert"),
        Inserted::Accepted
    );
    assert!(
        take(store, &["another".to_string()], "a", now)
            .await
            .is_none(),
        "a job for a name this process lacks stays pending"
    );
    let claimed = take(store, &names, "a", now).await.expect("a due job");
    assert_eq!(claimed.id, first.id);
    assert_eq!(claimed.payload, first.payload);
    assert_eq!(claimed.attempt, 1, "the attempt counts this claim");
    assert!(
        take(store, &names, "b", now).await.is_none(),
        "a leased job belongs to its owner"
    );
    assert!(
        store
            .renew(&first.id, "a", now + LEASE)
            .await
            .expect("renew")
    );
    assert!(
        !store
            .renew(&first.id, "b", now + LEASE)
            .await
            .expect("renew")
    );

    // Settling: a stranger changes nothing, and success removes the row.
    store.complete(&first.id, "b").await.expect("complete");
    assert_eq!(store.counts().await.expect("counts").running, 1);
    store.complete(&first.id, "a").await.expect("complete");
    let counts = store.counts().await.expect("counts");
    assert_eq!((counts.pending, counts.running), (0, 0));

    // Scheduling: a future job waits, and a retry waits for its delay.
    let later = job(now + 1_000);
    store.insert(later.clone()).await.expect("insert");
    assert!(take(store, &names, "a", now).await.is_none(), "not due yet");
    take(store, &names, "a", now + 1_000)
        .await
        .expect("due now");
    store
        .reschedule(&later.id, "a", now + 5_000, "temporary")
        .await
        .expect("reschedule");
    assert!(take(store, &names, "a", now + 4_999).await.is_none());
    let retried = take(store, &names, "a", now + 5_000)
        .await
        .expect("the retry");
    assert_eq!(retried.attempt, 2, "a retry keeps the attempt count");

    // Recovery: an expired lease moves to another worker, spends an attempt,
    // and the old owner can no longer settle it.
    let expired = now + 5_000 + LEASE;
    let recovered = take(store, &names, "b", expired)
        .await
        .expect("an expired lease");
    assert_eq!(recovered.attempt, 3);
    store.complete(&later.id, "a").await.expect("complete");
    assert_eq!(store.counts().await.expect("counts").running, 1);

    // Release: a graceful shutdown hands the job back without spending it.
    store.release(&later.id, "b").await.expect("release");
    let again = take(store, &names, "c", expired).await.expect("released");
    assert_eq!(again.attempt, 3, "release does not spend an attempt");
    store.complete(&later.id, "c").await.expect("complete");

    // Uniqueness: held while pending or running, free once finished.
    let keyed = job(now).unique("conformance");
    assert_eq!(
        store.insert(keyed.clone()).await.expect("insert"),
        Inserted::Accepted
    );
    assert_eq!(
        store
            .insert(job(now).unique("conformance"))
            .await
            .expect("insert"),
        Inserted::Duplicate
    );
    take(store, &names, "a", now).await.expect("keyed");
    assert_eq!(
        store
            .insert(job(now).unique("conformance"))
            .await
            .expect("insert"),
        Inserted::Duplicate,
        "a running job still holds its key"
    );
    store
        .finish(&keyed.id, "a", Terminal::Failed, "gave up", now)
        .await
        .expect("finish");
    let successor = job(now).unique("conformance");
    assert_eq!(
        store.insert(successor.clone()).await.expect("insert"),
        Inserted::Accepted,
        "a finished job releases its key"
    );

    // Inspection: finished jobs stay, newest first, until purged.
    take(store, &names, "a", now).await.expect("successor");
    store
        .finish(&successor.id, "a", Terminal::Discarded, "refused", now + 1)
        .await
        .expect("finish");
    let counts = store.counts().await.expect("counts");
    assert_eq!((counts.failed, counts.discarded), (1, 1));
    let failures = store.failures(10).await.expect("failures");
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0].id, successor.id);
    assert_eq!(failures[0].outcome, Terminal::Discarded);
    assert_eq!(failures[0].error, "refused");
    assert_eq!(failures[1].id, keyed.id);
    assert_eq!(store.purge(now + 1).await.expect("purge"), 1);
    assert_eq!(store.purge(now + 2).await.expect("purge"), 1);

    // Atomicity: of many simultaneous claims, exactly one wins.
    store.insert(job(now)).await.expect("insert");
    let owners = (0..8)
        .map(|index| format!("racer-{index}"))
        .collect::<Vec<_>>();
    let claims =
        futures_util::future::join_all(owners.iter().map(|owner| take(store, &names, owner, now)))
            .await;
    let winners = claims.iter().flatten().collect::<Vec<_>>();
    assert_eq!(winners.len(), 1, "exactly one worker claims a job");
    let winner = owners
        .iter()
        .zip(&claims)
        .find_map(|(owner, claim)| claim.as_ref().map(|_| owner))
        .expect("a winner");
    store
        .complete(&winners[0].id, winner)
        .await
        .expect("complete");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_memory_store_keeps_the_contract() {
        conformance(&MemoryJobStore::new()).await;
    }
}
