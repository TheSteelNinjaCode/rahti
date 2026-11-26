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

assert_eq!(local_now.time_zone(), &zone);
```

The types are jiff's, re-exported from `rahti::time` so application code
names them without depending on the crate itself:

| Type | What it is |
| --- | --- |
| `Timestamp` | An instant, with no zone. What is stored and compared. |
| `ZonedDateTime` | An instant shown in a zone. What `now()` returns. |
| `Date` | A calendar date. What `today()` returns. |
| `DateTime` | A wall-clock date and time with no zone and no instant. |
| `TimeZone` | An IANA zone. What `timezone()` returns. |

A `Date` or `DateTime` has no instant until a zone interprets it.
`TimeZone::get("Europe/Berlin")` resolves a zone by name and `iana_name()`
returns it. A `ZonedDateTime` displays as
`2026-01-14T21:30:00-05:00[America/New_York]`; `value.strftime("%Z")` gives
the zone's abbreviation (`EST`), and `strftime` formats any of these types
for display. `year()`, `month()`, `day()`, `hour()` and `minute()` read the
wall-clock fields.

`Timestamp` and `ZonedDateTime` serialize as strings, so an `#[rpc]` returns
them directly.

## Stored Timestamps

Store instants in UTC. Convert a stored instant for presentation without
changing the instant:

```rust
let shown = rahti::time::to_app_time(stored);
```

`to_app_time` takes a `Timestamp` or a `ZonedDateTime`. A database driver
that returns its own date type hands over the instant as Unix seconds:

```rust
use rahti::time::Timestamp;

let stored = Timestamp::from_second(unix_seconds)?;
let shown = rahti::time::to_app_time(stored);
```

`from_second` carries whole seconds. A column that keeps fractions of a
second passes them too, as the nanoseconds past that second (0 to
999,999,999), or the value shown and compared is rounded down to the second:

```rust
let stored = Timestamp::new(unix_seconds, subsecond_nanoseconds)?;
```

SeaORM's `DateTime` column value is one such type: a wall-clock value with no
offset, stored as UTC. Keep the conversion in one application function and
call it wherever a stored time is shown:

```rust
use rahti::time::{Timestamp, ZonedDateTime};
use sea_orm::prelude::DateTime;

/// A stored UTC column value in the application timezone.
pub fn local(value: DateTime) -> rahti::Result<ZonedDateTime> {
    let utc = value.and_utc();
    let nanoseconds = utc.timestamp_subsec_nanos() as i32;
    let stored = Timestamp::new(utc.timestamp(), nanoseconds)?;
    Ok(rahti::time::to_app_time(stored))
}
```

`local(row.created_at)?.strftime("%b %d, %Y %H:%M").to_string()` is then the
text a page shows.

Some columns hold a wall-clock value with no offset whose application
contract is still UTC. Make that assumption explicit:

```rust
let shown = rahti::time::from_naive_utc(stored_naive_utc)?;
```

`from_naive_utc` takes a `DateTime` and returns
`AppTimeError::UnrepresentableDate` for a value outside the supported range
(years -9999 to 9999). Do not use it for user-entered local wall time. Local
input can be nonexistent during spring-forward or ambiguous during fall-back
and needs an application-facing validation policy.

For a column that requires a naive UTC value, convert before stripping the
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
use rahti::time::Date;

let day = Date::new(2026, 3, 8)?;
let (start, end) = rahti::time::day_bounds_utc(day)?;

let rows = query_created_at_gte_lt(start, end).await?;
```

`start` and `end` are `Timestamp`s; `as_second()` gives each as Unix seconds
for a driver that takes a number.

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

