# Application Timezone

Rahti separates absolute time from application calendar time.

Absolute instants stay UTC: database timestamps, session expiry, cache TTLs,
rate-limit windows, job retry delays, and signatures must not move when an
operator changes a display preference. The application timezone decides only
which wall-clock date and time an instant belongs to, what “today” means, local
calendar query boundaries, and when a local daily schedule is due.

## Configuration

Set one IANA timezone name in the environment or the ignored `.env`:

```ini
APP_TIMEZONE="America/Santo_Domingo"
```

An absent or blank value means `UTC`. Names are IANA database identifiers
such as `UTC`, `America/New_York`, `Europe/Berlin`, or `Asia/Tokyo`, matched
without regard to case and reported in the database's own spelling
(`america/new_york` resolves to `America/New_York`). A misspelling fails
application startup and includes the offending value; Rahti never silently
falls back from an invalid configured name.

The scaffold resolves and installs the setting once in
`initialize_application`:

```rust
let app_time = rahti::time::AppTime::from_env()
    .map_err(|error| StartupError::new("timezone", error.to_string()))?;
rahti::time::configure(app_time);
```

Resolve it before starting schedulers or serving requests. Changing the
environment requires a process restart. Zone rules come from the host's IANA
database where it has one and from a copy embedded in the Rahti binary
otherwise, so named zones resolve on Windows and in slim containers that ship
no `/usr/share/zoneinfo`. `AppTime::named("Europe/Berlin")`
resolves a name without reading the environment, which is how a test installs
a zone; quotes around either value are ignored.

`APP_TIMEZONE` is application behavior rather than a credential. It stays in
the environment so deployments can use different calendars without compiling
different binaries. Do not set the process-wide `TZ` variable or read the
server's local clock; those make behavior depend on the host.

## Current Time and Calendar Date

Use the installed application calendar:

```rust
let local_now = rahti::time::now();
let local_date = rahti::time::today();
let zone = rahti::time::timezone();

assert_eq!(local_now.timezone(), zone);
```

`now()` returns an aware `DateTime<Tz>`. `today()` returns a
`NaiveDate` because a calendar date has no instant until it is interpreted in
a timezone.

`Tz` is Rahti's zone type and a chrono `TimeZone`, so it works wherever
chrono takes one: `value.with_timezone(&zone)`,
`zone.with_ymd_and_hms(2026, 1, 14, 21, 30, 0)`, `zone.from_local_datetime(…)`.
It is `Copy`, `"Europe/Berlin".parse::<Tz>()` resolves one, and `name()`
returns its IANA name. An aware value displays the zone's abbreviation
(`2026-01-14 21:30:00 EST`), and `value.offset().abbreviation()` returns it.
A wall time a transition skips has no instant (`LocalResult::None`); one it
repeats has two (`LocalResult::Ambiguous`, the earlier first).

The module re-exports `DateTime`, `NaiveDate`, `NaiveDateTime`, `Tz`,
and `Utc` for application code that does not otherwise depend directly on
the time crates.

## Stored Timestamps

Store instants in UTC. Convert an aware stored value for presentation without
changing the instant:

```rust
let shown = rahti::time::to_app_time(stored_utc);
```

Some database drivers return a naive timestamp for a column whose application
contract is still UTC. Make that assumption explicit:

```rust
let shown = rahti::time::from_naive_utc(stored_naive_utc);
```

Do not use `from_naive_utc` for user-entered local wall time. Local input can
be nonexistent during spring-forward or ambiguous during fall-back and needs
an application-facing validation policy.

For a driver that requires naive UTC values, convert before stripping the
offset:

```rust
let database_value = rahti::time::as_naive_utc(aware_value);
```

Removing an offset without first converting to UTC changes the represented
instant.

## Local-Day Database Queries

A report for one application calendar day needs UTC instants for its database
predicate:

```rust
use rahti::time::NaiveDate;

let day = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
let (start, end) = rahti::time::day_bounds_utc(day)?;

let rows = query_created_at_gte_lt(start, end).await?;
```

The range is half-open: `[start, end)`. Use greater-than-or-equal for the
start and strictly-less-than for the end. Adjacent dates then share one
boundary without double-counting it.

The calculation anchors successive local midnights. A daylight-saving
spring-forward day can therefore span 23 hours in UTC and a fall-back day 25
hours. A calendar date with no representable boundaries returns
`AppTimeError::UnrepresentableDate` instead of inventing a range.

## Task Scheduling

`Scheduler::daily(...)` uses the installed application timezone:

```rust
let scheduler = rahti::schedule::Scheduler::new(queue)
    .daily("morning-summary", 8, 30, "send-summary", payload)?
    .start();
```

The time remains 08:30 local when the UTC offset changes. On an ambiguous
fall-back time Rahti selects the earlier occurrence and runs once for that
local date. If a requested minute is skipped by a spring-forward transition,
it advances to the first valid minute after the gap.

Use `daily_utc(...)` when the schedule is intentionally tied to UTC rather
than the business calendar. Interval and one-shot schedules are absolute and
never change with `APP_TIMEZONE`.

As with every built-in schedule, local daily timing is process-local and not
durable. See `task-scheduling.md` for overlap, missed-occurrence, replica,
and shutdown behavior.

## Security and Expiration Boundary

Never use application-local wall time for:

- session issuance or expiry;
- cache expiration;
- signed-token timestamps;
- request budgets;
- retry and timeout durations;
- uniqueness or ordering of stored events.

Those are absolute or monotonic concerns and Rahti keeps them independent of
`APP_TIMEZONE`. Changing from UTC to UTC+14 must change a report's calendar
date when appropriate; it must not add fourteen hours to an authenticated
session or cached value.

