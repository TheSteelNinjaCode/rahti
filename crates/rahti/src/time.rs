//! Application calendar time over UTC-stored instants.
//!
//! The configured IANA zone decides wall-clock display, `today`, local-day
//! query bounds, and local task schedules. It never changes session expiry,
//! cache TTLs, retry delays, signatures, or any other absolute duration.

use std::{
    collections::HashSet,
    fmt,
    str::FromStr,
    sync::{Mutex, OnceLock, RwLock},
};

pub use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use chrono::{Duration, FixedOffset, LocalResult, MappedLocalTime, NaiveTime, TimeZone as _};

pub const APP_TIMEZONE_ENV: &str = "APP_TIMEZONE";
pub const DEFAULT_APP_TIMEZONE: &str = "UTC";

/// An IANA time zone, usable wherever chrono takes a zone: `DateTime<Tz>`,
/// `with_timezone(&zone)`, `zone.with_ymd_and_hms(…)`. Its rules come from
/// jiff's time zone database: the host's own when it has one, otherwise the
/// copy embedded in the binary, so a zone resolves the same way in a slim
/// container and on Windows. `Tz` is `Copy`, and so is `DateTime<Tz>`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tz {
    name: &'static str,
}

impl Tz {
    pub const UTC: Tz = Tz { name: "UTC" };

    /// The zone's IANA name, as the database spells it (`America/New_York`).
    pub fn name(self) -> &'static str {
        self.name
    }

    /// The zone's rules. The database caches them, so this is a lookup, and a
    /// name that resolved once keeps resolving.
    fn rules(self) -> jiff::tz::TimeZone {
        if self.name == Self::UTC.name {
            return jiff::tz::TimeZone::UTC;
        }
        jiff::tz::db()
            .get(self.name)
            .unwrap_or(jiff::tz::TimeZone::UTC)
    }

    fn offset_at(self, rules: &jiff::tz::TimeZone, instant: jiff::Timestamp) -> TzOffset {
        let info = rules.to_offset_info(instant);
        TzOffset::new(self, info.offset().seconds(), info.abbreviation())
    }
}

/// A zone name, kept for the life of the process: there are a few hundred,
/// and each is stored once however often it is parsed.
fn interned(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = names.get(name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.insert(leaked);
    leaked
}

impl FromStr for Tz {
    type Err = AppTimeError;

    /// An IANA name, matched without regard to case and kept in the
    /// database's own spelling.
    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let zone = jiff::tz::db()
            .get(name)
            .map_err(|_| AppTimeError::InvalidZone(name.to_string()))?;
        let canonical = zone.iana_name().unwrap_or(name);
        Ok(Self {
            name: interned(canonical),
        })
    }
}

impl fmt::Debug for Tz {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

impl fmt::Display for Tz {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name)
    }
}

/// The offset of a `Tz` at one instant, with the zone it belongs to and its
/// abbreviation (`EST`, `CEST`, `-03`), which is how it displays.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TzOffset {
    zone: Tz,
    fixed: FixedOffset,
    abbreviation: [u8; 8],
    length: u8,
}

impl TzOffset {
    fn new(zone: Tz, seconds: i32, abbreviation: &str) -> Self {
        let fixed = FixedOffset::east_opt(seconds)
            .unwrap_or_else(|| FixedOffset::east_opt(0).expect("zero is a valid offset"));
        let mut bytes = [0u8; 8];
        let mut length = 0;
        for letter in abbreviation.chars() {
            let width = letter.len_utf8();
            if length + width > bytes.len() {
                break;
            }
            letter.encode_utf8(&mut bytes[length..]);
            length += width;
        }
        Self {
            zone,
            fixed,
            abbreviation: bytes,
            length: length as u8,
        }
    }

    pub fn abbreviation(&self) -> &str {
        std::str::from_utf8(&self.abbreviation[..usize::from(self.length)]).unwrap_or("")
    }

    pub fn zone(&self) -> Tz {
        self.zone
    }
}

impl chrono::Offset for TzOffset {
    fn fix(&self) -> FixedOffset {
        self.fixed
    }
}

impl fmt::Debug for TzOffset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.abbreviation())
    }
}

impl fmt::Display for TzOffset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.abbreviation())
    }
}

/// A chrono UTC wall time as a jiff instant, or `None` outside jiff's range
/// (years -9999 to 9999).
fn instant_of(utc: &NaiveDateTime) -> Option<jiff::Timestamp> {
    let nanos = utc.and_utc().timestamp_subsec_nanos().min(999_999_999);
    jiff::Timestamp::new(utc.and_utc().timestamp(), nanos as i32).ok()
}

/// A chrono local wall time as a jiff civil time, or `None` outside its range.
fn civil_of(local: &NaiveDateTime) -> Option<jiff::civil::DateTime> {
    use chrono::{Datelike, Timelike};
    let nanos = local.nanosecond().min(999_999_999);
    jiff::civil::DateTime::new(
        i16::try_from(local.year()).ok()?,
        local.month() as i8,
        local.day() as i8,
        local.hour() as i8,
        local.minute() as i8,
        local.second() as i8,
        nanos as i32,
    )
    .ok()
}

impl chrono::TimeZone for Tz {
    type Offset = TzOffset;

    fn from_offset(offset: &TzOffset) -> Self {
        offset.zone
    }

    fn offset_from_local_date(&self, local: &NaiveDate) -> MappedLocalTime<TzOffset> {
        self.offset_from_local_datetime(&local.and_time(NaiveTime::MIN))
    }

    /// A wall time a transition skips is `None`; one it repeats is
    /// `Ambiguous`, the earlier instant first.
    fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> MappedLocalTime<TzOffset> {
        use jiff::tz::AmbiguousOffset;
        let Some(civil) = civil_of(local) else {
            return LocalResult::None;
        };
        let rules = self.rules();
        let at = |offset: jiff::tz::Offset| {
            offset
                .to_timestamp(civil)
                .ok()
                .map(|instant| self.offset_at(&rules, instant))
        };
        match rules.to_ambiguous_timestamp(civil).offset() {
            AmbiguousOffset::Unambiguous { offset } => {
                at(offset).map_or(LocalResult::None, LocalResult::Single)
            }
            AmbiguousOffset::Gap { .. } => LocalResult::None,
            AmbiguousOffset::Fold { before, after } => match (at(before), at(after)) {
                (Some(earlier), Some(later)) => LocalResult::Ambiguous(earlier, later),
                (Some(only), None) | (None, Some(only)) => LocalResult::Single(only),
                (None, None) => LocalResult::None,
            },
        }
    }

    fn offset_from_utc_date(&self, utc: &NaiveDate) -> TzOffset {
        self.offset_from_utc_datetime(&utc.and_time(NaiveTime::MIN))
    }

    fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> TzOffset {
        match instant_of(utc) {
            Some(instant) => self.offset_at(&self.rules(), instant),
            // Outside the database's range a zone has no rules to apply.
            None => TzOffset::new(*self, 0, "UTC"),
        }
    }
}

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
        Tz::from_str(name).map(|zone| Self { zone })
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
    fn a_zone_name_is_matched_without_case_and_kept_in_its_own_spelling() {
        let zone: Tz = "america/new_york".parse().unwrap();
        assert_eq!(zone.name(), "America/New_York");
        assert_eq!(zone, "America/New_York".parse::<Tz>().unwrap());
        assert_eq!(Tz::UTC.name(), "UTC");
        assert!("Not/A_Zone".parse::<Tz>().is_err());
    }

    #[test]
    fn an_aware_time_shows_its_offset_and_abbreviation() {
        let zone: Tz = "America/New_York".parse().unwrap();
        let winter = zone.with_ymd_and_hms(2026, 1, 14, 21, 30, 0).unwrap();
        let summer = zone.with_ymd_and_hms(2026, 7, 14, 21, 30, 0).unwrap();
        assert_eq!(winter.to_string(), "2026-01-14 21:30:00 EST");
        assert_eq!(summer.to_rfc3339(), "2026-07-14T21:30:00-04:00");
        assert_eq!(summer.offset().abbreviation(), "EDT");
        // A `DateTime<Tz>` is `Copy`, as it was with the zone it replaces.
        let copied = winter;
        assert_eq!(copied, winter);
    }

    #[test]
    fn a_skipped_wall_time_has_no_instant_and_a_repeated_one_has_two() {
        let zone: Tz = "America/New_York".parse().unwrap();
        let skipped = NaiveDate::from_ymd_opt(2026, 3, 8)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        assert!(matches!(
            zone.from_local_datetime(&skipped),
            LocalResult::None
        ));
        let repeated = NaiveDate::from_ymd_opt(2026, 11, 1)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();
        let LocalResult::Ambiguous(earlier, later) = zone.from_local_datetime(&repeated) else {
            panic!("1:30 repeats on the day clocks fall back");
        };
        assert_eq!(later - earlier, Duration::hours(1));
        assert_eq!(
            (
                earlier.offset().abbreviation(),
                later.offset().abbreviation()
            ),
            ("EDT", "EST")
        );
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
