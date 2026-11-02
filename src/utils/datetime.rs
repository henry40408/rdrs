use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Utc};

/// Parse Chinese month names to month number.
fn parse_chinese_month(s: &str) -> Option<u32> {
    match s {
        "一月" => Some(1),
        "二月" => Some(2),
        "三月" => Some(3),
        "四月" => Some(4),
        "五月" => Some(5),
        "六月" => Some(6),
        "七月" => Some(7),
        "八月" => Some(8),
        "九月" => Some(9),
        "十月" => Some(10),
        "十一月" => Some(11),
        "十二月" => Some(12),
        _ => None,
    }
}

/// Parse an offset like "+0800" into seconds.
fn parse_timezone_offset(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.len() < 5 {
        return None;
    }

    let sign = match s.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };

    // `get`, not indexing: a multi-byte character inside the span would panic.
    let hours: i32 = s.get(1..3)?.parse().ok()?;
    let minutes: i32 = s.get(3..5)?.parse().ok()?;

    Some(sign * (hours * 3600 + minutes * 60))
}

/// Parse Chinese date format like "週二, 6 一月 2026 14:28:00 +0000".
fn parse_chinese_datetime(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    // Remove weekday prefix if present (e.g., "週二, " or "星期二, ")
    let s = if let Some(pos) = s.find(", ") {
        &s[pos + 2..]
    } else {
        s
    };

    let parts: Vec<&str> = s.splitn(4, ' ').collect();
    if parts.len() < 4 {
        return None;
    }

    let day: u32 = parts[0].parse().ok()?;
    let month = parse_chinese_month(parts[1])?;
    let year: i32 = parts[2].parse().ok()?;

    let time_tz = parts[3];
    let time_parts: Vec<&str> = time_tz.splitn(2, ' ').collect();
    let time_str = time_parts.first()?;

    let time = NaiveTime::parse_from_str(time_str, "%H:%M:%S").ok()?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let naive_dt = NaiveDateTime::new(date, time);

    if let Some(tz_str) = time_parts.get(1)
        && let Some(offset_secs) = parse_timezone_offset(tz_str)
    {
        let offset = FixedOffset::east_opt(offset_secs)?;
        let dt = naive_dt.and_local_timezone(offset).single()?;
        return Some(dt.with_timezone(&Utc));
    }

    Some(naive_dt.and_utc())
}

/// Convert a "+08:00" suffix to "+0800"; some feeds put ISO 8601 offsets in
/// RFC 2822 dates, which dateparser rejects.
pub fn normalize_timezone_format(text: &str) -> String {
    let text = text.trim();
    let Some(cut) = text.len().checked_sub(6) else {
        return text.to_string();
    };
    // Matched on bytes: a suffix that matches is all ASCII, so `cut` is a char
    // boundary; slicing by `&str` index first would panic inside a Chinese date.
    let [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] = text.as_bytes()[cut..] else {
        return text.to_string();
    };
    if ![h1, h2, m1, m2].iter().all(u8::is_ascii_digit) {
        return text.to_string();
    }
    format!(
        "{}{}{}{}{}{}",
        &text[..cut],
        char::from(sign),
        char::from(h1),
        char::from(h2),
        char::from(m1),
        char::from(m2)
    )
}

/// `dateparser` reads a timestamp with no offset in the server's local zone and
/// fills a date-only value with the time of the fetch. A feed says neither, so
/// the stable reading is UTC and midnight, whatever `TZ` the server runs in.
fn parse_as_utc(text: &str) -> Option<DateTime<Utc>> {
    dateparser::parse_with(text, &Utc, NaiveTime::MIN).ok()
}

/// Parse a stored datetime, or `None` if every format fails (RFC 3339, SQL
/// datetime, dateparser, Chinese). Use over [`parse_datetime`] where a
/// `Utc::now()` fallback would corrupt the result.
pub fn try_parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        .ok()
        .or_else(|| parse_as_utc(s))
        .or_else(|| parse_chinese_datetime(s))
}

/// UTC timestamp as shown to the user. Same pattern as the DB text format but
/// independent of it: cursor and `read_after` values must keep matching
/// `datetime('now')` (see `snapshot_now`), so do not reuse this for them.
pub fn format_display(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Parse a stored datetime, falling back to `Utc::now()`.
pub fn parse_datetime(s: &str) -> DateTime<Utc> {
    try_parse_datetime(s).unwrap_or_else(Utc::now)
}

/// feed-rs timestamp parser: dateparser, plus "+08:00" offsets in RFC 2822 and
/// Chinese dates (e.g. "週二, 6 一月 2026 14:28:00 +0000").
pub fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    parse_as_utc(text)
        .or_else(|| {
            let normalized = normalize_timezone_format(text);
            if normalized == text {
                None
            } else {
                parse_as_utc(&normalized)
            }
        })
        .or_else(|| parse_chinese_datetime(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    #[test]
    fn test_parse_datetime_rfc3339() {
        let dt = parse_datetime("2026-01-06T14:28:00Z");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 28);
    }

    #[test]
    fn test_try_parse_datetime_supported_formats() {
        // SQL datetime and RFC 3339 both parse to the same instant.
        assert!(try_parse_datetime("2026-01-06 14:28:00").is_some());
        assert!(try_parse_datetime("2026-01-06T14:28:00Z").is_some());
        let a = try_parse_datetime("2026-01-06 14:28:00").unwrap();
        let b = try_parse_datetime("2026-01-06T14:28:00Z").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn test_try_parse_datetime_garbage_is_none() {
        // Unparseable input returns None (no Utc::now() fallback).
        assert!(try_parse_datetime("not a date").is_none());
        assert!(try_parse_datetime("").is_none());
    }

    #[test]
    fn test_parse_datetime_sql_format() {
        let dt = parse_datetime("2026-01-06 14:28:00");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
    }

    #[test]
    fn test_parse_chinese_datetime_with_weekday() {
        let dt = parse_chinese_datetime("週二, 6 一月 2026 14:28:00 +0000");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
        assert_eq!(dt.hour(), 14);
        assert_eq!(dt.minute(), 28);
    }

    #[test]
    fn test_parse_chinese_datetime_different_months() {
        assert!(parse_chinese_datetime("週一, 15 三月 2026 10:00:00 +0800").is_some());
        assert!(parse_chinese_datetime("週五, 25 十二月 2026 23:59:59 +0000").is_some());
        assert!(parse_chinese_datetime("週日, 1 七月 2026 00:00:00 -0500").is_some());
    }

    #[test]
    fn test_parse_chinese_month() {
        assert_eq!(parse_chinese_month("一月"), Some(1));
        assert_eq!(parse_chinese_month("六月"), Some(6));
        assert_eq!(parse_chinese_month("十二月"), Some(12));
        assert_eq!(parse_chinese_month("invalid"), None);
    }

    #[test]
    fn test_parse_timezone_offset() {
        assert_eq!(parse_timezone_offset("+0000"), Some(0));
        assert_eq!(parse_timezone_offset("+0800"), Some(8 * 3600));
        assert_eq!(parse_timezone_offset("-0500"), Some(-5 * 3600));
    }

    #[test]
    fn test_normalize_timezone_format() {
        assert_eq!(
            normalize_timezone_format("Thu, 22 Jan 2026 15:09:47 +08:00"),
            "Thu, 22 Jan 2026 15:09:47 +0800"
        );
        assert_eq!(
            normalize_timezone_format("Mon, 01 Jan 2026 12:00:00 -05:30"),
            "Mon, 01 Jan 2026 12:00:00 -0530"
        );
        assert_eq!(
            normalize_timezone_format("Thu, 22 Jan 2026 15:09:47 +0800"),
            "Thu, 22 Jan 2026 15:09:47 +0800"
        );
        assert_eq!(
            normalize_timezone_format("Thu, 22 Jan 2026 15:09:47 +08:00  "),
            "Thu, 22 Jan 2026 15:09:47 +0800"
        );
    }

    #[test]
    fn test_parse_timestamp_colon_timezone() {
        let result = parse_timestamp("Thu, 22 Jan 2026 15:09:47 +08:00");
        assert!(
            result.is_some(),
            "Should parse RFC2822-like format with colon timezone"
        );

        let dt = result.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 22);
        // The time should be converted to UTC (15:09:47 +08:00 = 07:09:47 UTC)
        assert_eq!(dt.hour(), 7);
        assert_eq!(dt.minute(), 9);
    }

    #[test]
    fn test_parse_timestamp_various_formats() {
        assert!(parse_timestamp("Thu, 22 Jan 2026 15:09:47 +0800").is_some());
        assert!(parse_timestamp("2026-01-22T15:09:47+08:00").is_some());
        assert!(parse_timestamp("週四, 22 一月 2026 15:09:47 +0800").is_some());
    }

    #[test]
    fn test_parse_timestamp_returns_none_for_garbage() {
        assert!(parse_timestamp("not a date").is_none());
    }

    #[test]
    fn test_parse_chinese_datetime_without_weekday() {
        // No weekday prefix — goes through the else branch
        let dt = parse_chinese_datetime("6 一月 2026 14:28:00 +0000");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 6);
    }

    #[test]
    fn test_parse_chinese_datetime_without_timezone() {
        // No timezone — falls through to naive_dt.and_utc()
        let dt = parse_chinese_datetime("週二, 6 一月 2026 14:28:00");
        assert!(dt.is_some());
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.hour(), 14);
    }

    #[test]
    fn test_parse_chinese_datetime_too_few_parts() {
        assert!(parse_chinese_datetime("6 一月").is_none());
    }

    #[test]
    fn test_parse_chinese_datetime_invalid_month() {
        assert!(parse_chinese_datetime("週二, 6 invalid 2026 14:28:00 +0000").is_none());
    }

    #[test]
    fn test_parse_timezone_offset_with_multibyte_characters_is_none() {
        assert!(parse_timezone_offset("+08中").is_none());
        assert!(parse_timezone_offset("+中0800").is_none());
    }

    #[test]
    fn test_normalize_timezone_format_cut_inside_a_multibyte_character() {
        // 7 bytes: the last six start in the middle of the first character.
        assert_eq!(normalize_timezone_format("日期x"), "日期x");
        assert_eq!(normalize_timezone_format("2026年1月6日"), "2026年1月6日");
    }

    #[test]
    fn test_parse_timestamp_without_offset_is_utc() {
        let dt = parse_timestamp("2026-01-06 14:28:00").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-06T14:28:00+00:00");
    }

    #[test]
    fn test_parse_timestamp_date_only_is_midnight_utc_and_stable() {
        let first = parse_timestamp("2026-01-06").unwrap();
        assert_eq!(first.to_rfc3339(), "2026-01-06T00:00:00+00:00");
        assert_eq!(parse_timestamp("2026-01-06"), Some(first));
    }

    #[test]
    fn test_parse_timestamp_keeps_an_explicit_offset() {
        let dt = parse_timestamp("Tue, 06 Jan 2026 14:28:00 +0800").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-06T06:28:00+00:00");
        let dt = parse_timestamp("Tue, 06 Jan 2026 14:28:00 +08:00").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-06T06:28:00+00:00");
    }

    #[test]
    fn test_try_parse_datetime_without_offset_is_utc() {
        let dt = try_parse_datetime("Jan 6, 2026 14:28:00").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-06T14:28:00+00:00");
    }

    #[test]
    fn test_parse_timezone_offset_short_string() {
        assert!(parse_timezone_offset("+08").is_none());
        assert!(parse_timezone_offset("").is_none());
    }

    #[test]
    fn test_parse_timezone_offset_invalid_sign() {
        assert!(parse_timezone_offset("X0800").is_none());
    }

    #[test]
    fn test_normalize_timezone_format_short_input() {
        // Input shorter than 6 chars — just returns as-is
        assert_eq!(normalize_timezone_format("abc"), "abc");
        assert_eq!(normalize_timezone_format("12:00"), "12:00");
    }

    #[test]
    fn test_normalize_timezone_format_no_colon() {
        // 6+ chars but no timezone-like suffix
        assert_eq!(normalize_timezone_format("foobar"), "foobar");
    }

    #[test]
    fn test_parse_datetime_falls_back_to_utc_now() {
        // Completely unparsable string — falls back to Utc::now()
        let dt = parse_datetime("totally unparsable");
        let diff = (Utc::now() - dt).num_seconds().abs();
        assert!(diff < 5);
    }

    #[test]
    fn test_parse_chinese_month_all_months() {
        assert_eq!(parse_chinese_month("二月"), Some(2));
        assert_eq!(parse_chinese_month("四月"), Some(4));
        assert_eq!(parse_chinese_month("五月"), Some(5));
        assert_eq!(parse_chinese_month("七月"), Some(7));
        assert_eq!(parse_chinese_month("八月"), Some(8));
        assert_eq!(parse_chinese_month("九月"), Some(9));
        assert_eq!(parse_chinese_month("十月"), Some(10));
        assert_eq!(parse_chinese_month("十一月"), Some(11));
    }
}
