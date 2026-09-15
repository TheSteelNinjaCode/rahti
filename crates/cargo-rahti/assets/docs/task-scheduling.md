# Task Scheduling

Rahti's scheduler submits named work to `rahti::jobs::JobQueue` at a specified
time. It does not execute application logic itself: the background-job queue
continues to own payload decoding, capacity, retries, timeouts, observability,
and graceful completion.

Schedules are process-local and in memory. They are appropriate for periodic
maintenance and best-effort automation. They are not a durable promise that an
occurrence will run.

## Define the Schedule

Start the background-job queue first, then build one scheduler for it during
`initialize_application`:

```rust
use std::{sync::OnceLock, time::{Duration, SystemTime}};
use rahti::schedule::{Scheduler, SchedulerHandle};

static SCHEDULER: OnceLock<SchedulerHandle> = OnceLock::new();

pub fn start_schedule(queue: rahti::jobs::JobQueue) {
    let scheduler = Scheduler::new(queue)
        .every(
            "refresh-search-index",
            Duration::from_secs(5 * 60),
            "refresh-index",
            (),
        )
        .expect("valid interval schedule")
        .daily(
            "send-daily-summary",
            8,
            30,
            "send-summary",
            SummaryInput { locale: "en".into() },
        )
        .expect("valid daily schedule")
        .once(
            "finish-import",
            SystemTime::now() + Duration::from_secs(60),
            "finish-import",
            ImportInput { import_id: 42 },
        )
        .expect("valid one-shot schedule")
        .start();

    SCHEDULER.set(scheduler).expect("task scheduler configured once");
}
```

Schedule names are stable identifiers containing 1–64 ASCII letters, numbers,
`-`, `_`, or `.`. Names must be unique within one `Scheduler`. The target
job must already be registered on the supplied queue, so misspelled handlers
fail during startup rather than at the scheduled time. Payload serialization
also happens while defining the schedule.

The supported timing forms are:

- `every`: fixed delay, with the first occurrence one interval after startup;
- `daily`: every calendar day in the configured application timezone;
- `daily_utc`: every day at an explicit UTC hour and minute;
- `once`: one occurrence at a `SystemTime`; a past instant runs promptly;
- `every_allowing_overlap`: an explicit overlapping form for work designed
  to run concurrently.

Intervals must be greater than zero. Hours are 0–23 and minutes are 0–59.
`daily` snapshots the installed `APP_TIMEZONE` when the schedule is defined
and follows its daylight-saving rules. `daily_utc` is the explicit choice for
infrastructure work tied to UTC. Neither form reads the server's local
timezone. See `application-timezone.md`.

## Overlap and Backpressure

Recurring schedules prevent overlap by default. A schedule-specific unique key
is held while its prior job is queued, running, retrying, or backing off. If the
next occurrence arrives during that period, it is skipped and counted; it is
not delayed into an accumulating backlog.

Use `every_allowing_overlap` only when concurrent occurrences are safe and
the queue budget can absorb them. This choice does not make a handler
idempotent and does not lift the queue's capacity.

Scheduled dispatch never waits for queue space. A full queue skips that
occurrence and records it. This keeps a scheduler timer from becoming an
unbounded producer. Unknown, closed, or otherwise rejected dispatches are
recorded as failures.

When a process sleeps, stalls, or starts late, interval occurrences are
coalesced. Rahti schedules the next interval from the current observation time
rather than replaying every missed tick in a burst. A daily occurrence missed
while the process is stopped is not replayed.

## Shutdown

Each timer subscribes to Rahti shutdown before it starts. Shutdown stops future
dispatches immediately. `rahti::jobs::finish_shutdown()` first waits for all
schedulers to stop and only then closes and drains job queues, so a timer cannot
race the queue boundary. Existing web and static-export scaffolds already call
that function.

A test can stop only one scheduler with:

```rust
scheduler.shutdown().await;
queue.shutdown().await;
```

Always stop the scheduler before its queue. Starting a scheduler during static
export is permitted for shared startup structure, but static rendering must
not define an already-due one-shot or otherwise depend on scheduled side
effects.

## Observability

`scheduler.metrics()` returns process-local counters for:

- due occurrences;
- successfully enqueued jobs;
- occurrences skipped to prevent overlap;
- occurrences skipped because the queue was full;
- other enqueue failures.

Production events and development diagnostics use the `schedule` target and
include the stable schedule name, target job name, and job id when dispatch
succeeds. Payload values are never logged.

Metrics have fixed cardinality and reset on restart. Alert on queue-full and
dispatch-failure growth when scheduled work matters operationally.

## Deployment Boundary

Every application process that starts a scheduler owns an independent copy.
With three web replicas, the same daily schedule can therefore enqueue three
jobs. The built-in overlap guard coordinates only within one process.

Run scheduling in exactly one designated application replica when one
best-effort occurrence is sufficient. Use a durable external scheduler and
queue when execution must survive downtime, coordinate across replicas, catch
up missed occurrences, use cron/calendar expressions, observe a dead-letter
queue, or provide an exactly-once business guarantee. Even with external
coordination, handlers must remain idempotent because a crash can happen after
a side effect and before acknowledgement.
