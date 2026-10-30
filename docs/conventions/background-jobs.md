# Background Jobs

A job is work that runs after the response: sending a receipt, updating a
derived search index, calling an integration. It is a typed Rust function, run
by named workers with retries, timeouts, and graceful shutdown.

By default the queue is bounded and in memory. Give it a store and accepted
jobs live in the application's database, where they survive a crash and any
process on that database can run them.

## Write a Job

```bash
cargo rahti generate job send_receipt --params order_id:i64
```

This adds the function to `src/jobs.rs`, declares `pub mod jobs;` in
`src/lib.rs`, and makes sure workers start. A parameter with no type is a
`String`. The result is an ordinary function with an attribute:

```rust
use rahti::job;
use rahti::jobs::JobError;

#[job(attempts = 5, timeout_secs = 20)]
pub async fn send_receipt(order_id: i64) -> Result<(), JobError> {
    let order = load_order(order_id)
        .await
        .map_err(|error| JobError::retry(error.to_string()))?;
    let Some(order) = order else {
        return Err(JobError::discard(format!("order {order_id} no longer exists")));
    };
    deliver_receipt(&order)
        .await
        .map_err(|error| JobError::retry(error.to_string()))
}
```

Jobs live in `src/jobs.rs` or in files under `src/jobs/`. `#[job]` refuses to
compile anywhere else, because the build reads those files into the manifest
and a job outside them would run invisibly.

A job is `async`, takes owned parameters that implement `Serialize` and
`DeserializeOwned`, and returns nothing, `Result<(), JobError>`, or
`rahti::Result<()>`. Generics, `self`, borrowed parameters, and returned values
are compile errors that name the form to write.

| `#[job(…)]` option | Default | Meaning |
| --- | --- | --- |
| `name = "billing.receipt"` | the function's name | the stored name; 1–64 ASCII letters, numbers, `-`, `_`, or `.` |
| `attempts = 5` | the queue's, 3 | total attempts including the first |
| `timeout_secs = 20` | the queue's, 30 | limit on one attempt |
| `backoff_ms = 250` | the queue's, 250 | delay after the first failure, doubling |
| `max_backoff_ms = 30000` | the queue's, 30000 | largest delay |

The stored name is a contract with work already in the queue. Renaming a
function leaves stored jobs under the old name, where no handler will claim
them, so pin the old value with `name = "…"` before renaming a job in a durable
application. Two jobs sharing a name are a build error.

## Enqueue a Job

The attribute gives the function's name a second meaning. The name still calls
the function, so a test calls `send_receipt(7).await` directly, and
`send_receipt::` reaches the queue:

```rust
let order = save_order(input).await?;

crate::jobs::send_receipt::enqueue(order.id)
    .await
    .map_err(|error| rahti::Error::internal(error.to_string()))?;
```

A misspelled job, a missing argument, and a wrong type are compile errors on
this line.

| Call | Result |
| --- | --- |
| `send_receipt::enqueue(id).await` | `JobId`, waiting for capacity on a memory queue |
| `send_receipt::enqueue_unique("key", id).await` | `Some(JobId)`, or `None` while a pending or running job holds `key` |
| `send_receipt::request(id)` | a `NewJob` value, for a store to insert in a transaction |
| `send_receipt::payload(id)` | the stored arguments, for a schedule |
| `send_receipt::NAME` | the stored name |

Enqueue after the authoritative transaction commits. A fast worker may
otherwise observe state that later rolls back. With a store, enqueue inside the
transaction instead; see below.

Do not put sessions, secrets, password material, access tokens, or unnecessary
personal data in a parameter. Pass an id and load the rest in the handler.
Payloads are not encrypted, and a store keeps them on disk until the job ends.

## Start the Workers

`initialize_application` starts the queue once, after the database and before
the router:

```rust
rahti::jobs::Jobs::new().install();
```

`install` starts workers for every `#[job]` linked into the binary and makes
this the queue `enqueue` reaches. With no jobs it starts nothing. During a
static export it also starts nothing, because rendering must not run side
effects. An `enqueue` with no installed queue returns
`EnqueueError::NotStarted`, whose message names this line.

Configure the queue in code, because this is application policy:

```rust
use std::time::Duration;

rahti::jobs::Jobs::new()
    .workers(4)
    .capacity(1_024)
    .attempts(3)
    .backoff(Duration::from_millis(250), Duration::from_secs(30))
    .attempt_timeout(Duration::from_secs(20))
    .shutdown_grace(Duration::from_secs(30))
    .install();
```

Defaults are one worker, capacity 1,024 in memory and 10,000 in a store, three
attempts, 250 ms initial exponential backoff capped at 30 seconds, a 30-second
attempt timeout, and a 30-second shutdown grace. `#[job]` options override the
attempt policy for that job.

A test reinstalls freely: the newest `install` replaces the previous queue,
which is what a test binary with one runtime per test needs.

## Durable Jobs

In a project with a database:

```bash
cargo rahti generate job-store
```

This writes three application-owned files and changes `src/lib.rs`:

| File | Purpose |
| --- | --- |
| `src/job_store.rs` | `SeaJobStore`, the adapter, and `enqueue_in` |
| `src/models/job.rs` | the `job` table's entity |
| `src/migrations/m…_create_job.rs` | the migration that creates it |

Apply the migration, and add a test that holds the adapter to the contract:

```rust
#[tokio::test]
async fn the_job_store_keeps_the_contract() {
    setup().await;
    rahti::jobs::conformance(&crate::job_store::SeaJobStore).await;
}
```

`conformance` needs an empty `job` table and panics on the first broken rule.
The queue now starts with `Jobs::new().store(job_store::SeaJobStore).install()`.
Workers wake at once for work this process accepts, and poll for the rest; set
the interval with `.poll_interval(…)`, one second by default.

What a store changes:

- **A crash loses nothing.** A job is committed before `enqueue` returns.
- **Several processes share one queue.** On Postgres or MySQL that includes
  several machines. On SQLite it means one machine.
- **A dead worker's job runs again.** A worker holds a five-minute lease and
  renews it while it runs. When the lease expires, another worker claims the
  job, and the interrupted attempt counts. Set the length with `.lease(…)`.
- **A retry survives a restart.** Backoff is a stored due time, not a sleep.
- **A failure stays.** Failed and discarded jobs keep their last error for
  inspection until `store.purge(before_ms)` deletes them. Successful jobs are
  deleted.
- **Shutdown does not drain.** Pending jobs simply stay stored. Running jobs get
  the shutdown grace, and the rest return to pending without spending an
  attempt.
- **`enqueue` does not wait.** At capacity it returns `EnqueueError::Full`,
  because the bound belongs to a shared table. The bound is checked before the
  insert, so concurrent callers can pass it by a few rows.

A job for a name this process has no handler for stays pending, so a rolling
deployment never fails work that only the other version can run.

### In the Same Transaction

A job and the write it follows can commit together. Nothing can then leave one
without the other, including a crash between them:

```rust
use sea_orm::TransactionTrait;

let transaction = crate::db::db().begin().await?;
let order = save_order(&transaction, input).await?;
crate::job_store::enqueue_in(
    &transaction,
    crate::jobs::send_receipt::request(order.id)?,
)
.await?;
transaction.commit().await?;
```

A rollback removes the job with the row. Workers find a job inserted this way on
their next poll.

### Another Store

`rahti::jobs::JobStore` is a small trait, and `Jobs::store` takes any
implementation. `MemoryJobStore` is the reference implementation and a test
double. `conformance` states the contract in executable form: claiming is
atomic, a lease belongs to its owner, an expired lease moves on, and unique keys
are held until a job ends.

## Failure and Retry

| The handler | Outcome |
| --- | --- |
| returns `Ok(())` | succeeded |
| returns `Err(JobError::retry(message))` | retried with capped exponential backoff |
| returns `rahti::Error` | retried |
| returns `Err(JobError::discard(message))` | discarded at once |
| panics | contained, then retried |
| exceeds its timeout | dropped, then retried |
| uses every attempt | failed |

A stored payload that no longer reads as the function's parameters is discarded,
because it can never succeed. An added `Option` parameter reads as `None`, so it
accepts jobs stored before it existed.

The attempt timeout drops the handler's future, and a lost lease means another
worker may run the job beside this one, so external side effects may already
have happened. **Every retryable handler must be idempotent.** Use a database
uniqueness key, a provider idempotency key, or a recorded completion marker
based on the job's business identity. Error messages enter diagnostics, so keep
them useful and free of credentials and payload values.

`rahti::jobs::context()` inside a handler gives its job id, stable name,
one-based attempt, and the request id that enqueued it. Background tasks do not
inherit the request's authentication session or authorization decision.
Authorize the write before enqueueing, and re-check mutable business rules in
the handler.

## Shutdown

Workers subscribe when they start. Once shutdown begins they reject new jobs and
finish the work they hold. A memory queue also drains what is queued. Work still
running at the grace deadline is aborted and counted as abandoned; in a store it
returns to pending.

The scaffolded web host lets HTTP drain and then calls:

```rust
rahti::jobs::finish_shutdown().await;
```

Every custom or native host does the same after `begin_shutdown` and HTTP
graceful shutdown. A test can call `queue.shutdown().await` to stop one queue
without stopping the process.

## Observability

During development, `cargo rahti status` and the MCP `server_status` tool report
handlers, counters, stored counts, and the newest failures with their errors.
`list_jobs` and `jobs` in `.rahti/manifest.json` give every job's parameters,
policy, source location, and the exact call that enqueues it.

`queue.metrics()` returns fixed-cardinality process counters for queued, active,
succeeded, failed, retried, timed-out, panicking, and abandoned work. They reset
on restart and carry no user-controlled labels. `queue.status().await` adds
stored counts and recent failures, and never carries a payload.

Production JSONL events and development diagnostics use the `jobs` target. They
include job id, registered name, attempt, duration, and originating request id.
Rahti never logs a payload. Export counters through an approved internal metrics
collector; Rahti does not publish a metrics route.

## Programmatic Handlers

`Jobs::handler("name", closure)` registers a closure on a queue from `start()`,
for a handler that captures state. Its name is a string, so nothing checks a call
at compile time, and it appears in no manifest. Prefer `#[job]`.

## Operational Boundary

- Without a store, a process crash, forced deployment, or machine failure loses
  queued and active jobs. Use a store when an accepted job must survive.
- With a store, delivery is at least once, never exactly once.
- The queue owns no timers. `rahti::schedule` provides process-local interval,
  local/UTC daily, and one-shot dispatch, and those timers are not durable even
  when the jobs they submit are.
- There is no dashboard, cross-process cancellation, or built-in cleanup of
  failed rows.
