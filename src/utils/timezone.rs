//! A user's timezone, which decides where the statistics page cuts one day from
//! the next. Instants are stored in UTC everywhere; this only maps calendar
//! dates to and from them.

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;

/// What a user who never chose a timezone gets.
pub const DEFAULT_TIMEZONE: &str = "UTC";

/// Common choices offered in the settings form; any IANA name is accepted.
pub const TIMEZONE_SUGGESTIONS: &[&str] = &[
    "UTC",
    "America/Los_Angeles",
    "America/New_York",
    "America/Sao_Paulo",
    "Europe/London",
    "Europe/Berlin",
    "Asia/Dubai",
    "Asia/Kolkata",
    "Asia/Shanghai",
    "Asia/Taipei",
    "Asia/Tokyo",
    "Australia/Sydney",
];

/// An IANA name such as `Asia/Taipei`; anything else is `None`.
pub fn parse(name: &str) -> Option<Tz> {
    name.parse().ok()
}

/// A stored value that no longer parses falls back to UTC, so a display
/// preference can never break a page render.
pub fn parse_or_utc(name: &str) -> Tz {
    parse(name).unwrap_or(Tz::UTC)
}

/// Whether `a` and `b` cut days at the same instants for the next year.
///
/// Compared by behaviour because the database lists aliases as separate zones
/// (`Asia/Calcutta` and `Asia/Kolkata`, `Etc/UTC` and `UTC`), and browsers
/// report either spelling. Offsets are compared every day for a year, so
/// daylight-saving rules that differ for only a few weeks still count.
pub fn same_rules(a: Tz, b: Tz, now: DateTime<Utc>) -> bool {
    let offset_at = |tz: Tz, at: DateTime<Utc>| at.with_timezone(&tz).offset().fix();
    a == b
        || (0..=366).all(|day| {
            let at = now + Duration::days(day);
            offset_at(a, at) == offset_at(b, at)
        })
}

/// The calendar date `instant` falls on in `tz`.
pub fn date_at(instant: DateTime<Utc>, tz: Tz) -> NaiveDate {
    instant.with_timezone(&tz).date_naive()
}

pub fn today_in(tz: Tz) -> NaiveDate {
    date_at(Utc::now(), tz)
}

/// The UTC instant at which `date` begins in `tz`.
///
/// Some zones skip midnight on a DST change (São Paulo before 2019); the day
/// then begins at its first hour that exists. A few skipped a whole day (Apia,
/// 2011-12-30); that date then begins where the next one does.
pub fn start_of_day_utc(date: NaiveDate, tz: Tz) -> NaiveDateTime {
    let first_hour = (0..24).find_map(|hour| {
        let local = date.and_hms_opt(hour, 0, 0)?;
        tz.from_local_datetime(&local).earliest()
    });
    if let Some(start) = first_hour {
        return start.with_timezone(&Utc).naive_utc();
    }
    match date.succ_opt() {
        Some(next) => start_of_day_utc(next, tz),
        None => date.and_time(chrono::NaiveTime::MIN),
    }
}

/// `[start, end)` of `date` in `tz` as UTC instants; a DST day is not 24 hours.
pub fn day_bounds_utc(date: NaiveDate, tz: Tz) -> (DateTime<Utc>, DateTime<Utc>) {
    let next = date.succ_opt().unwrap_or(date);
    (
        start_of_day_utc(date, tz).and_utc(),
        start_of_day_utc(next, tz).and_utc(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, min, 0).unwrap()
    }

    #[test]
    fn day_bounds_span_the_local_day() {
        let taipei = parse("Asia/Taipei").unwrap();
        let (start, end) = day_bounds_utc(ymd(2026, 5, 1), taipei);
        assert_eq!(start, utc(2026, 4, 30, 16, 0));
        assert_eq!(end, utc(2026, 5, 1, 16, 0));
    }

    #[test]
    fn day_bounds_are_not_24_hours_across_dst() {
        let new_york = parse("America/New_York").unwrap();
        let hours = |date| {
            let (start, end) = day_bounds_utc(date, new_york);
            (end - start).num_hours()
        };
        assert_eq!(hours(ymd(2026, 3, 8)), 23, "spring forward");
        assert_eq!(hours(ymd(2026, 11, 1)), 25, "fall back");
        assert_eq!(hours(ymd(2026, 6, 1)), 24);
    }

    #[test]
    fn same_rules_sees_through_aliases_but_not_real_differences() {
        let zone = |name| parse(name).unwrap();
        let now = utc(2026, 11, 4, 0, 0);
        let same = |a, b| same_rules(zone(a), zone(b), now);

        assert!(same("Asia/Calcutta", "Asia/Kolkata"), "alias");
        assert!(same("Etc/UTC", "UTC"), "alias");
        assert!(same("Asia/Taipei", "Asia/Shanghai"), "same offset all year");
        assert!(!same("Asia/Taipei", "Asia/Tokyo"), "one hour apart");
        assert!(
            !same("Europe/London", "Africa/Abidjan"),
            "equal in winter, apart in summer"
        );
        assert!(
            !same("Africa/Cairo", "Asia/Jerusalem"),
            "differ only for a few weeks around the transitions"
        );
    }

    #[test]
    fn parse_accepts_iana_names_only() {
        assert_eq!(parse("Asia/Taipei"), Some(Tz::Asia__Taipei));
        assert_eq!(parse("UTC"), Some(Tz::UTC));
        assert_eq!(parse(""), None);
        assert_eq!(parse("Mars/Olympus_Mons"), None);
        assert_eq!(parse("CST-8"), None);
    }

    #[test]
    fn every_suggestion_parses() {
        for name in TIMEZONE_SUGGESTIONS {
            assert!(parse(name).is_some(), "{name}");
        }
        assert!(TIMEZONE_SUGGESTIONS.contains(&DEFAULT_TIMEZONE));
    }

    #[test]
    fn parse_or_utc_falls_back_for_a_stale_value() {
        assert_eq!(parse_or_utc("Asia/Taipei"), Tz::Asia__Taipei);
        assert_eq!(parse_or_utc("Not/AZone"), Tz::UTC);
    }

    #[test]
    fn date_at_follows_the_zone_across_midnight() {
        let late_evening_utc = utc(2026, 1, 5, 20, 0);
        assert_eq!(date_at(late_evening_utc, Tz::UTC), ymd(2026, 1, 5));
        // 04:00 the next morning in Taipei (UTC+8).
        assert_eq!(date_at(late_evening_utc, Tz::Asia__Taipei), ymd(2026, 1, 6));
        // 12:00 the same day in Los Angeles (UTC-8).
        assert_eq!(
            date_at(late_evening_utc, Tz::America__Los_Angeles),
            ymd(2026, 1, 5)
        );
    }

    #[test]
    fn start_of_day_is_the_previous_evening_east_of_greenwich() {
        let start = start_of_day_utc(ymd(2026, 1, 6), Tz::Asia__Taipei);
        assert_eq!(start, utc(2026, 1, 5, 16, 0).naive_utc());
    }

    #[test]
    fn start_of_day_is_the_same_morning_west_of_greenwich() {
        let start = start_of_day_utc(ymd(2026, 1, 6), Tz::America__Los_Angeles);
        assert_eq!(start, utc(2026, 1, 6, 8, 0).naive_utc());
    }

    #[test]
    fn start_of_day_is_midnight_in_utc() {
        let start = start_of_day_utc(ymd(2026, 1, 6), Tz::UTC);
        assert_eq!(start, utc(2026, 1, 6, 0, 0).naive_utc());
    }

    #[test]
    fn start_of_day_tracks_daylight_saving() {
        // New York is UTC-5 in January and UTC-4 in July.
        assert_eq!(
            start_of_day_utc(ymd(2026, 1, 15), Tz::America__New_York),
            utc(2026, 1, 15, 5, 0).naive_utc()
        );
        assert_eq!(
            start_of_day_utc(ymd(2026, 7, 15), Tz::America__New_York),
            utc(2026, 7, 15, 4, 0).naive_utc()
        );
    }

    #[test]
    fn start_of_day_uses_the_first_hour_when_midnight_is_skipped() {
        // São Paulo moved its clocks from 00:00 to 01:00 on 2018-11-04, so that
        // day began at 01:00 local (UTC-2), i.e. 03:00 UTC.
        let start = start_of_day_utc(ymd(2018, 11, 4), Tz::America__Sao_Paulo);
        assert_eq!(start, utc(2018, 11, 4, 3, 0).naive_utc());
    }

    #[test]
    fn a_skipped_calendar_day_begins_where_the_next_one_does() {
        // Samoa jumped from 2011-12-29 to 2011-12-31, so the 30th never existed.
        let skipped = start_of_day_utc(ymd(2011, 12, 30), Tz::Pacific__Apia);
        let next = start_of_day_utc(ymd(2011, 12, 31), Tz::Pacific__Apia);
        assert_eq!(skipped, next);
        assert_eq!(next, utc(2011, 12, 30, 10, 0).naive_utc());
    }

    #[test]
    fn consecutive_days_are_adjacent_across_a_dst_change() {
        // 2026-03-08 is 23 hours long in New York.
        let day = start_of_day_utc(ymd(2026, 3, 8), Tz::America__New_York);
        let next = start_of_day_utc(ymd(2026, 3, 9), Tz::America__New_York);
        assert_eq!((next - day).num_hours(), 23);
    }
}
