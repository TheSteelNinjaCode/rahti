//! Application calendar time over UTC-stored instants.
//!
//! The configured IANA zone decides wall-clock display, `today`, local-day
//! query bounds, and local task schedules. It never changes session expiry,
//! cache TTLs, retry delays, signatures, or any other absolute duration.

use std::{
    fmt,
    str::FromStr,
    sync::{OnceLock, RwLock},
};

pub use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use chrono::{Duration, LocalResult, TimeZone as _};
pub use chrono_tz::Tz;

pub const APP_TIMEZONE_ENV: &str = "APP_TIMEZONE";
pub const DEFAULT_APP_TIMEZONE: &str = "UTC";

/// A validated application calendar timezone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppTime {
    zone: Tz,
}

impl Default for AppTime {
    fn default() -> Self {
        Self { zone: Tz::UTC }
    }
}

impl AppTime {
    /// Resolve an IANA database name such as `America/New_York`.
    pub fn named(name: &str) -> Result<Self, AppTimeError> {
        let name = name.trim().trim_matches(['"', '\'']);
        let name = if name.is_empty() {
            DEFAULT_APP_TIMEZONE
        } else {
            name
        };
        Tz::from_str(name)
            .map(|zone| Self { zone })
            .map_err(|_| AppTimeError::InvalidZone(name.to_string()))
    }

    /// Read `APP_TIMEZONE` after loading `.env`, defaulting to UTC.
    pub fn from_env() -> Result<Self, AppTimeError> {
        crate::load_env();
        let name = std::env::var(APP_TIMEZONE_ENV).unwrap_or_default();
        Self::named(&name)
    }

    pub fn timezone(self) -> Tz {
        self.zone
    }

    pub fn name(self) -> &'static str {
        self.zone.name()
    }

    /// Current instant rendered in the application timezone.
    pub fn now(self) -> DateTime<Tz> {
        Utc::now().with_timezone(&self.zone)
    }

    /// Current calendar date in the application timezone.
    pub fn today(self) -> NaiveDate {
        self.now().date_naive()
    }

    /// Render an aware instant in the application timezone without changing
    /// the instant it identifies.
    pub fn to_app_time<Z: chrono::TimeZone>(self, value: DateTime<Z>) -> DateTime<Tz> {
        value.with_timezone(&self.zone)
    }

    /// Treat a driver-provided naive timestamp as UTC, then render it in the
    /// application timezone.
    pub fn from_naive_utc(self, value: NaiveDateTime) -> DateTime<Tz> {
        Utc.from_utc_datetime(&value).with_timezone(&self.zone)
    }

    /// A half-open `[start, end)` pair of UTC instants bounding one local
    /// calendar date. DST therefore produces legitimate 23- or 25-hour days.
    pub fn day_bounds_utc(
        self,
        day: NaiveDate,
    ) -> Result<(DateTime<Utc>, DateTime<Utc>), AppTimeError> {
        let next = day
            .succ_opt()
            .ok_or(AppTimeError::UnrepresentableDate(day))?;
        let start =
            resolve_local(self.zone, day, 0, 0).ok_or(AppTimeError::UnrepresentableDate(day))?;
        let end =
            resolve_local(self.zone, next, 0, 0).ok_or(AppTimeError::UnrepresentableDate(next))?;
        if start >= end {
            return Err(AppTimeError::UnrepresentableDate(day));
        }
        Ok((start, end))
    }

    /// Convert an aware instant to the naive UTC shape required by database
    /// columns that store UTC without an offset.
    pub fn as_naive_utc<Z: chrono::TimeZone>(self, value: DateTime<Z>) -> NaiveDateTime {
        value.with_timezone(&Utc).naive_utc()
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
        .map(|value| *value)
        .unwrap_or_else(|poisoned| *poisoned.into_inner())
}

pub fn timezone() -> Tz {
    app_time().timezone()
}

pub fn now() -> DateTime<Tz> {
    app_time().now()
}

pub fn today() -> NaiveDate {
    app_time().today()
}

pub fn to_app_time<Z: chrono::TimeZone>(value: DateTime<Z>) -> DateTime<Tz> {
    app_time().to_app_time(value)
}

pub fn from_naive_utc(value: NaiveDateTime) -> DateTime<Tz> {
    app_time().from_naive_utc(value)
}

pub fn day_bounds_utc(day: NaiveDate) -> Result<(DateTime<Utc>, DateTime<Utc>), AppTimeError> {
    app_time().day_bounds_utc(day)
}

pub fn as_naive_utc<Z: chrono::TimeZone>(value: DateTime<Z>) -> NaiveDateTime {
    app_time().as_naive_utc(value)
}

/// Resolve one local wall-clock minute. Ambiguous fall-back times choose the
/// earlier instant; nonexistent spring-forward times advance to the first
/// valid minute, never more than three hours later.
pub(crate) fn resolve_local(
    zone: Tz,
    day: NaiveDate,
    hour: u8,
    minute: u8,
) -> Option<DateTime<Utc>> {
    let requested = day.and_hms_opt(u32::from(hour), u32::from(minute), 0)?;
    for adjustment in 0..=180 {
        let local = requested.checked_add_signed(Duration::minutes(adjustment))?;
        match zone.from_local_datetime(&local) {
            LocalResult::Single(value) => return Some(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                return Some(first.min(second).with_timezone(&Utc));
            }
            LocalResult::None => {}
        }
    }
    None
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AppTimeError {
    InvalidZone(String),
    UnrepresentableDate(NaiveDate),
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
    use chrono::{Datelike, Timelike};

    #[test]
    fn names_default_validate_and_strip_common_env_quotes() {
        assert_eq!(AppTime::named("").unwrap().name(), "UTC");
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
    fn utc_instants_convert_without_being_relabelled() {
        let app = AppTime::named("America/New_York").unwrap();
        let stored = Utc.with_ymd_and_hms(2026, 1, 15, 2, 30, 0).unwrap();
        let local = app.to_app_time(stored);
        assert_eq!(
            (local.year(), local.month(), local.day(), local.hour()),
            (2026, 1, 14, 21)
        );
        assert_eq!(local.with_timezone(&Utc), stored);
    }

    #[test]
    fn naive_database_values_are_treated_as_utc() {
        let app = AppTime::named("America/New_York").unwrap();
        let stored = NaiveDate::from_ymd_opt(2026, 1, 15)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        assert_eq!(app.from_naive_utc(stored).day(), 14);
    }

    #[test]
    fn local_day_bounds_follow_dst_and_are_half_open() {
        let app = AppTime::named("America/New_York").unwrap();
        let spring = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let fall = NaiveDate::from_ymd_opt(2026, 11, 1).unwrap();
        let (spring_start, spring_end) = app.day_bounds_utc(spring).unwrap();
        let (next_start, _) = app.day_bounds_utc(spring.succ_opt().unwrap()).unwrap();
        let (fall_start, fall_end) = app.day_bounds_utc(fall).unwrap();
        assert_eq!(spring_end - spring_start, Duration::hours(23));
        assert_eq!(fall_end - fall_start, Duration::hours(25));
        assert_eq!(spring_end, next_start);
    }

    #[test]
    fn naive_utc_conversion_converts_before_removing_the_offset() {
        let app = AppTime::named("America/New_York").unwrap();
        let local = app
            .timezone()
            .with_ymd_and_hms(2026, 1, 15, 21, 30, 0)
            .unwrap();
        assert_eq!(
            app.as_naive_utc(local),
            NaiveDate::from_ymd_opt(2026, 1, 16)
                .unwrap()
                .and_hms_opt(2, 30, 0)
                .unwrap()
        );
    }
}
