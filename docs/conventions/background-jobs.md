# Background Jobs

Rahti runs non-request work through typed, named handlers on a bounded
process-local queue. The queue is useful for work such as sending a
notification, updating a derived search index, or performing an integration
call after the HTTP response no longer needs its result.

This is an in-memory execution facility, not a durable broker. A process crash,
forced deployment, or machine failure can lose queued and active jobs. Use an
external durable queue when every accepted job must survive those failures.

## Define and Start a Queue

Keep the queue handle in application-owned state, beside other process-wide
services. For example, `src/jobs.rs` can contain:

```rust
use std::{sync::OnceLock, time::Duration};
use rahti::jobs::{JobError, JobQueue, Jobs};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct SendReceipt {
    pub order_id: i64,
}

static QUEUE: OnceLock<JobQueue> = OnceLock::new();

pub fn start() {
    let queue = Jobs::new()
        .capacity(1_024)
        .workers(4)
        .attempts(3)
        .backoff(Duration::from_millis(250), Duration::from_secs(30))
        .attempt_timeout(Duration::from_secs(20))
        .shutdown_grace(Duration::from_secs(30))
        .handler("send-receipt", |job: SendReceipt| async move {
            send_receipt(job.order_id)
                .await
                .map_err(|error| JobError::retry(error.to_string()))
        })
        .start();

    QUEUE.set(queue).expect("background jobs configured once");
}

pub fn queue() -> &'static JobQueue {
    QUEUE.get().expect("background jobs started")
}
```

Call `jobs::start()` from `initialize_application` before building the
router. `Jobs::start` must run inside the Tokio runtime and requires at least
one handler. A job name is 1–64 ASCII letters, numbers, `-`, `_`, or `.`;
duplicate names are a startup error.

Defaults are one worker, capacity 1,024, three attempts, 250 ms initial
exponential backoff capped at 30 seconds, a 30-second attempt timeout, and a
30-second shutdown grace. Configure these in code because they are application
policy, not framework environment variables.

## Enqueue After the Write

Payloads implement `Serialize` when enqueued and `DeserializeOwned` at the
handler. Enqueue only after the authoritative transaction commits:

```rust
let order = save_order(input).await?;

jobs::queue()
    .enqueue("send-receipt", SendReceipt { order_id: order.id })
    .await
    .map_err(|error| rahti::Error::internal(error.to_string()))?;
```

`enqueue` waits for bounded capacity and returns a `JobId`.
`try_enqueue` returns `EnqueueError::Full` immediately instead. Both reject
unknown names and return `EnqueueError::Closed` after shutdown starts.

Do not enqueue before a database commit: a fast worker may observe state that
later rolls back. Do not put sessions, secrets, password material, access
tokens, or unnecessary personal data in a payload. Payloads are serialized in
memory, not encrypted, and the process may retain them until execution.

## Failure and Retry

A handler returns one of:

- `Ok(())` for completion;
- `Err(JobError::retry(message))` for a transient failure;
- `Err(JobError::discard(message))` for a permanent input or policy failure.

Retryable errors, panics, and attempt timeouts retry up to the configured total
attempt count with capped exponential backoff. A discarded error stops
immediately. The attempt timeout drops the handler future; external side
effects may already have happened, so every retryable handler must be
idempotent. Use a database uniqueness key, provider idempotency key, or a
recorded completion marker based on the job's business identity.

A panic is isolated to its job attempt rather than taking down a worker.
Exhausted attempts are recorded and discarded; this process-local queue has no
dead-letter store. Error messages enter diagnostics, so keep them useful but
free of credentials and sensitive payload values.

The current handler can inspect `rahti::jobs::context()` to obtain its job id,
stable name, one-based attempt, and the request id that originated the job when
it was enqueued inside an observed request. Background tasks do not inherit the
request's authentication session or authorization decision. Put a stable
resource identifier in the payload and authorize the write before enqueueing;
re-check mutable business rules in the handler when necessary.

## Shutdown

Rahti workers subscribe when they start. Once shutdown begins they reject new
jobs, close the receiver, finish queued and active work, and stop at the
configured grace deadline. Work still queued or active at that deadline is
counted as abandoned and its tasks are aborted.

The scaffolded web host first lets HTTP drain and then calls:

```rust
rahti::jobs::finish_shutdown().await;
```

Every custom or native host must do the same after `begin_shutdown` and HTTP
graceful shutdown. A test can call `queue.shutdown().await` to drain one
queue without stopping the entire process.

Starting workers during static export is allowed, but static rendering should
not enqueue side effects. The scaffold still finishes all started queues before
the export process returns.

## Observability

`queue.metrics()` returns fixed-cardinality process counters for queued,
active, succeeded, failed, retried, timed-out, panicking, and abandoned work.
They reset on restart and carry no user-controlled labels.

Production JSONL events and development diagnostics use the `jobs` target.
They include job id, registered name, attempt, duration, and originating request
id. Rahti never logs the serialized payload. Export counters through an
approved internal metrics collector; Rahti does not publish a metrics route.

## Operational Boundary

The built-in queue is intentionally bounded and process-local:

- several application processes each own independent workers and counters;
- the queue itself does not own timers; `rahti::schedule` provides
  process-local interval, local/UTC daily, and one-shot dispatch;
- there is no persistence, lease, distributed uniqueness, dead-letter queue,
  dashboard, or cross-process cancellation;
- graceful shutdown reduces routine deployment loss but cannot protect against
  process or machine failure.

Choose a durable external broker for billing, irreversible fulfillment,
compliance delivery, long-running work, scheduled work, or any workflow whose
acceptance promise must survive a crash. The handler and payload shapes can
remain application-owned; only their transport and worker host need to change.
