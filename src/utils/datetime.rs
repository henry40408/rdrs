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

/// Parse timezone offset like "+0000", "+0800", "-0500".
/// Returns offset in seconds.
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

    let hours: i32 = s[1..3].parse().ok()?;
    let minutes: i32 = s[3..5].parse().ok()?;

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

    // Expected format: "6 一月 2026 14:28:00 +0000"
    let parts: Vec<&str> = s.splitn(4, ' ').collect();
    if parts.len() < 4 {
        return None;
    }

    let day: u32 = parts[0].parse().ok()?;
    let month = parse_chinese_month(parts[1])?;
    let year: i32 = parts[2].parse().ok()?;

    // Parse time and timezone: "14:28:00 +0000"
    let time_tz = parts[3];
    let time_parts: Vec<&str> = time_tz.splitn(2, ' ').collect();
    let time_str = time_parts.first()?;

    let time = NaiveTime::parse_from_str(time_str, "%H:%M:%S").ok()?;
    let date = NaiveDate::from_ymd_opt(year, month, day)?;
    let naive_dt = NaiveDateTime::new(date, time);

    // Parse timezone offset if present
    if let Some(tz_str) = time_parts.get(1)
        && let Some(offset_secs) = parse_timezone_offset(tz_str)
    {
        let offset = FixedOffset::east_opt(offset_secs)?;
        let dt = naive_dt.and_local_timezone(offset).single()?;
        return Some(dt.with_timezone(&Utc));
    }

    Some(naive_dt.and_utc())
}

/// Normalize timezone format: convert "+08:00" to "+0800".
/// Some feeds use ISO 8601 style timezone in RFC 2822 dates, which dateparser can't handle.
pub fn normalize_timezone_format(text: &str) -> String {
    let text = text.trim();
    let len = text.len();

    // Check if ends with timezone like "+08:00" or "-05:30" (6 chars)
    if len >= 6 {
        let suffix = &text[len - 6..];
        if let Some(sign) = suffix.chars().next()
            && (sign == '+' || sign == '-')
            && suffix.chars().nth(3) == Some(':')
            && suffix[1..3].chars().all(|c| c.is_ascii_digit())
            && suffix[4..6].chars().all(|c| c.is_ascii_digit())
        {
            // Convert "+08:00" to "+0800"
            let mut result = text[..len - 6].to_string();
            result.push(sign);
            result.push_str(&suffix[1..3]);
            result.push_str(&suffix[4..6]);
            return result;
        }
    }

    text.to_string()
}

/// Parse a datetime string from the database, returning `None` if every
/// supported format fails. Tries, in order: RFC 3339, SQL datetime
/// (`%Y-%m-%d %H:%M:%S`), dateparser (RFC 2822 and other localized formats),
/// then the Chinese date format.
///
/// Prefer this over [`parse_datetime`] when an unparseable value must be
/// distinguished from a valid one (e.g. aggregates over timestamps, where the
/// `Utc::now()` fallback would silently corrupt the result).
pub fn try_parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    // Try RFC 3339 first (standard format)
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        // Then try SQL datetime format
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").map(|dt| dt.and_utc())
        })
        // Then try dateparser for various formats (RFC 2822, localized dates, etc.)
        .or_else(|_| dateparser::parse(s).map(|dt| dt.with_timezone(&Utc)))
        .ok()
        // Then try Chinese date format
        .or_else(|| parse_chinese_datetime(s))
}

/// Parse a datetime string from the database (stored as SQL datetime or RFC 3339).
/// Falls back to `Utc::now()` if parsing fails.
pub fn parse_datetime(s: &str) -> DateTime<Utc> {
    try_parse_datetime(s).unwrap_or_else(Utc::now)
}

/// Custom timestamp parser for feed-rs that handles:
/// - Standard formats (via dateparser)
/// - ISO 8601 style timezone in RFC 2822 dates (+08:00 -> +0800)
/// - Chinese date formats (e.g., "週二, 6 一月 2026 14:28:00 +0000")
pub fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    // Try standard parsing first (via dateparser)
    dateparser::parse(text)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        // Try with normalized timezone format (convert +08:00 to +0800)
        .or_else(|| {
            let normalized = normalize_timezone_format(text);
            if normalized == text {
                None
            } else {
                dateparser::parse(&normalized)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            }
        })
        // Then try Chinese date format
        .or_else(|| parse_chinese_datetime(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};

    // === parse_datetime tests (from models) ===

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

    // === Chinese datetime tests ===

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

    // === normalize_timezone_format tests (from feed_sync) ===

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

    // === parse_timestamp tests (from feed_sync) ===

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
        // Standard RFC2822
        assert!(parse_timestamp("Thu, 22 Jan 2026 15:09:47 +0800").is_some());
        // ISO 8601 / RFC 3339
        assert!(parse_timestamp("2026-01-22T15:09:47+08:00").is_some());
        // Chinese format
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
        // Should be close to now
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
