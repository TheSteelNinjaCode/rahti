//! Application calendar time over UTC-stored instants.
//!
//! The configured IANA zone decides wall-clock display, `today`, local-day
//! query bounds, and local task schedules. It never changes session expiry,
//! cache TTLs, retry delays, signatures, or any other absolute duration.
//!
//! The types are jiff's. A [`Timestamp`] is an instant, a [`ZonedDateTime`]
//! is an instant shown in a zone, and a civil [`Date`] or [`DateTime`] is a
//! wall-clock value with no instant until a zone interprets it. Zone rules
//! come from the host's IANA database when it has one, otherwise from the
//! copy embedded in the binary, so a zone resolves the same way in a slim
//! container and on Windows.

use std::{
    fmt,
    sync::{OnceLock, RwLock},
};

use jiff::{SignedDuration, tz::AmbiguousOffset};
pub use jiff::{
    Timestamp, ZonedDateTime,
    civil::{Date, DateTime},
    tz::TimeZone,
};

pub const APP_TIMEZONE_ENV: &str = "APP_TIMEZONE";
pub const DEFAULT_APP_TIMEZONE: &str = "UTC";

/// A validated application calendar timezone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppTime {
    zone: TimeZone,
}

impl Default for AppTime {
    fn default() -> Self {
        Self {
            zone: TimeZone::UTC,
        }
    }
}

impl AppTime {
    /// Resolve an IANA database name such as `America/New_York`, matched
    /// without regard to case and kept in the database's own spelling.
    pub fn named(name: &str) -> Result<Self, AppTimeError> {
        let name = name.trim().trim_matches(['"', '\'']);
        if name.is_empty() || name.eq_ignore_ascii_case(DEFAULT_APP_TIMEZONE) {
            return Ok(Self::default());
        }
        TimeZone::get(name)
            .map(|zone| Self { zone })
            .map_err(|_| AppTimeError::InvalidZone(name.to_string()))
    }

    /// Read `APP_TIMEZONE` after loading `.env`, defaulting to UTC.
    pub fn from_env() -> Result<Self, AppTimeError> {
        crate::load_env();
        let name = std::env::var(APP_TIMEZONE_ENV).unwrap_or_default();
        Self::named(&name)
    }

    pub fn timezone(&self) -> TimeZone {
        self.zone.clone()
    }

    /// The zone's IANA name, as the database spells it.
    pub fn name(&self) -> &str {
        self.zone.iana_name().unwrap_or(DEFAULT_APP_TIMEZONE)
    }

    /// Current instant rendered in the application timezone.
    pub fn now(&self) -> ZonedDateTime {
        Timestamp::now().to_zoned_datetime(self.zone.clone())
    }

    /// Current calendar date in the application timezone.
    pub fn today(&self) -> Date {
        self.now().date()
    }

    /// Render an instant in the application timezone without changing the
    /// instant it identifies.
    pub fn to_app_time(&self, value: impl Into<Timestamp>) -> ZonedDateTime {
        value.into().to_zoned_datetime(self.zone.clone())
    }

    /// Treat a driver-provided naive timestamp as UTC, then render it in the
    /// application timezone.
    pub fn from_naive_utc(&self, value: DateTime) -> Result<ZonedDateTime, AppTimeError> {
        TimeZone::UTC
            .to_timestamp(value)
            .map(|instant| self.to_app_time(instant))
            .map_err(|_| AppTimeError::UnrepresentableDate(value.date()))
    }

    /// A half-open `[start, end)` pair of UTC instants bounding one local
    /// calendar date. DST therefore produces legitimate 23- or 25-hour days.
    pub fn day_bounds_utc(&self, day: Date) -> Result<(Timestamp, Timestamp), AppTimeError> {
        let next = day
            .tomorrow()
            .map_err(|_| AppTimeError::UnrepresentableDate(day))?;
        let start =
            resolve_local(&self.zone, day, 0, 0).ok_or(AppTimeError::UnrepresentableDate(day))?;
        let end =
            resolve_local(&self.zone, next, 0, 0).ok_or(AppTimeError::UnrepresentableDate(next))?;
        if start >= end {
            return Err(AppTimeError::UnrepresentableDate(day));
        }
        Ok((start, end))
    }

    /// Convert an instant to the naive UTC shape required by database columns
    /// that store UTC without an offset.
    pub fn as_naive_utc(&self, value: impl Into<Timestamp>) -> DateTime {
        TimeZone::UTC.to_datetime(value.into())
    }
}

fn installed() -> &'static RwLock<AppTime> {
    static APP_TIME: OnceLock<RwLock<AppTime>> = OnceLock::new();
    APP_TIME.get_or_init(|| RwLock::new(AppTime::default()))
}

/// Install the application timezone during startup. Later reads use this
/// resolved value and never consult the host machine's local timezone.
pub fn configure(app_time: AppTime) {
    match installed().write() {
        Ok(mut current) => *current = app_time,
        Err(poisoned) => *poisoned.into_inner() = app_time,
    }
}

pub fn app_time() -> AppTime {
    installed()
        .read()
        .map(|value| value.clone())
        .unwrap_or_else(|poisoned| poisoned.into_inner().clone())
}

pub fn timezone() -> TimeZone {
    app_time().timezone()
}

pub fn now() -> ZonedDateTime {
    app_time().now()
}

pub fn today() -> Date {
    app_time().today()
}

pub fn to_app_time(value: impl Into<Timestamp>) -> ZonedDateTime {
    app_time().to_app_time(value)
}

pub fn from_naive_utc(value: DateTime) -> Result<ZonedDateTime, AppTimeError> {
    app_time().from_naive_utc(value)
}

pub fn day_bounds_utc(day: Date) -> Result<(Timestamp, Timestamp), AppTimeError> {
    app_time().day_bounds_utc(day)
}

pub fn as_naive_utc(value: impl Into<Timestamp>) -> DateTime {
    app_time().as_naive_utc(value)
}

/// Resolve one local wall-clock minute. Ambiguous fall-back times choose the
/// earlier instant; nonexistent spring-forward times advance to the first
/// valid minute, never more than three hours later.
pub(crate) fn resolve_local(zone: &TimeZone, day: Date, hour: u8, minute: u8) -> Option<Timestamp> {
    let requested = day.at(i8::try_from(hour).ok()?, i8::try_from(minute).ok()?, 0, 0);
    for adjustment in 0..=180 {
        let local = requested
            .checked_add(SignedDuration::from_mins(adjustment))
            .ok()?;
        let candidate = zone.to_ambiguous_timestamp(local);
        if matches!(candidate.offset(), AmbiguousOffset::Gap { .. }) {
            continue;
        }
        return candidate.earlier().ok();
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppTimeError {
    InvalidZone(String),
    UnrepresentableDate(Date),
}

impl fmt::Display for AppTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidZone(name) => write!(
                formatter,
                "{APP_TIMEZONE_ENV}={name:?} is not a valid IANA timezone; use UTC or a name such as America/New_York"
            ),
            Self::UnrepresentableDate(day) => write!(
                formatter,
                "{day} has no representable calendar boundary in the application timezone"
            ),
        }
    }
}

impl std::error::Error for AppTimeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn new_york() -> AppTime {
        AppTime::named("America/New_York").unwrap()
    }

    #[test]
    fn names_default_validate_and_strip_common_env_quotes() {
        assert_eq!(AppTime::named("").unwrap().name(), "UTC");
        assert_eq!(AppTime::named("utc").unwrap(), AppTime::default());
        assert_eq!(
            AppTime::named("\"America/Santo_Domingo\"").unwrap().name(),
            "America/Santo_Domingo"
        );
        assert!(matches!(
            AppTime::named("America/Santo_Domingoo"),
            Err(AppTimeError::InvalidZone(_))
        ));
    }

    #[test]
    fn a_zone_name_is_matched_without_case_and_kept_in_its_own_spelling() {
        let app = AppTime::named("america/new_york").unwrap();
        assert_eq!(app.name(), "America/New_York");
        assert_eq!(app, new_york());
        assert!(AppTime::named("Not/A_Zone").is_err());
    }

    #[test]
    fn an_aware_time_shows_its_offset_and_abbreviation() {
        let zone = new_york().timezone();
        let winter = date(2026, 1, 14)
            .at(21, 30, 0, 0)
            .to_zoned_datetime(zone.clone())
            .unwrap();
        let summer = date(2026, 7, 14)
            .at(21, 30, 0, 0)
            .to_zoned_datetime(zone)
            .unwrap();
        assert_eq!(
            winter.to_string(),
            "2026-01-14T21:30:00-05:00[America/New_York]"
        );
        assert_eq!(summer.timestamp().to_string(), "2026-07-15T01:30:00Z");
        assert_eq!(winter.strftime("%Z").to_string(), "EST");
        assert_eq!(summer.strftime("%Z").to_string(), "EDT");
    }

    #[test]
    fn a_skipped_wall_time_advances_and_a_repeated_one_is_the_earlier() {
        let zone = new_york().timezone();
        // 02:30 does not exist on the day clocks spring forward: 03:00 EDT.
        let skipped = resolve_local(&zone, date(2026, 3, 8), 2, 30).unwrap();
        assert_eq!(skipped.to_string(), "2026-03-08T07:00:00Z");
        // 01:30 happens twice on the day clocks fall back: the EDT one.
        let repeated = resolve_local(&zone, date(2026, 11, 1), 1, 30).unwrap();
        assert_eq!(repeated.to_string(), "2026-11-01T05:30:00Z");
    }

    #[test]
    fn utc_instants_convert_without_being_relabelled() {
        let stored: Timestamp = "2026-01-15T02:30:00Z".parse().unwrap();
        let local = new_york().to_app_time(stored);
        assert_eq!(
            (local.year(), local.month(), local.day(), local.hour()),
            (2026, 1, 14, 21)
        );
        assert_eq!(local.timestamp(), stored);
    }

    #[test]
    fn naive_database_values_are_treated_as_utc() {
        let stored = date(2026, 1, 15).at(2, 30, 0, 0);
        assert_eq!(new_york().from_naive_utc(stored).unwrap().day(), 14);
    }

    #[test]
    fn local_day_bounds_follow_dst_and_are_half_open() {
        let app = new_york();
        let spring = date(2026, 3, 8);
        let fall = date(2026, 11, 1);
        let (spring_start, spring_end) = app.day_bounds_utc(spring).unwrap();
        let (next_start, _) = app.day_bounds_utc(spring.tomorrow().unwrap()).unwrap();
        let (fall_start, fall_end) = app.day_bounds_utc(fall).unwrap();
        assert_eq!(
            spring_end.duration_since(spring_start),
            SignedDuration::from_hours(23)
        );
        assert_eq!(
            fall_end.duration_since(fall_start),
            SignedDuration::from_hours(25)
        );
        assert_eq!(spring_end, next_start);
    }

    #[test]
    fn naive_utc_conversion_converts_before_removing_the_offset() {
        let app = new_york();
        let local = date(2026, 1, 15)
            .at(21, 30, 0, 0)
            .to_zoned_datetime(app.timezone())
            .unwrap();
        assert_eq!(app.as_naive_utc(&local), date(2026, 1, 16).at(2, 30, 0, 0));
    }
}
