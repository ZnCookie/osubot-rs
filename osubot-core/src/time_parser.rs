use chrono::{Duration, NaiveDate, NaiveDateTime, TimeZone, Timelike, Utc};
use regex::Regex;
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TimeParseError {
    #[error("无效的时间格式")]
    InvalidFormat,
    #[error("无效的数字")]
    InvalidNumber,
    #[error("无效的时间单位")]
    InvalidUnit,
    #[error("无效的日期")]
    InvalidDate,
    #[error("不能使用未来时间")]
    FutureTime,
}

#[derive(Debug, Clone)]
pub struct ParsedTime {
    pub timestamp: i64,
    pub is_relative: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Period {
    pub years: i64,
    pub months: i64,
    pub days: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TimeDuration {
    pub hours: i64,
    pub minutes: i64,
    pub seconds: i64,
}

static RELATIVE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^-?\s*(\d+)\s*(y|year|years|mo|month|months|w|week|weeks|d|day|days|h|hour|hours|m|min|minute|minutes|s|sec|second|seconds)(\s+\d+\s*(y|year|years|mo|month|months|w|week|weeks|d|day|days|h|hour|hours|m|min|minute|minutes|s|sec|second|seconds))*$").unwrap()
});

static ABSOLUTE_PATTERN_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}[-/]\d{1,2}[-/]\d{1,2}$").unwrap());

static ABSOLUTE_PATTERN_DATETIME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}[-/]\d{1,2}[-/]\d{1,2}\s+\d{1,2}:\d{2}(:\d{2})?$").unwrap()
});

static ABSOLUTE_PATTERN_COMPACT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{8}$").unwrap());

static SINGLE_TOKEN_RELATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(-?\d+)\s*(y|year|years|mo|month|months|w|week|weeks|d|day|days|h|hour|hours|m|min|minute|minutes|s|sec|second|seconds)$").unwrap()
});

static UNIT_MAP: LazyLock<phf::Map<&'static str, &'static str>> = LazyLock::new(|| {
    phf::phf_map! {
        "y" => "y",
        "year" => "y",
        "years" => "y",
        "mo" => "mo",
        "month" => "mo",
        "months" => "mo",
        "w" => "w",
        "week" => "w",
        "weeks" => "w",
        "d" => "d",
        "day" => "d",
        "days" => "d",
        "h" => "h",
        "hour" => "h",
        "hours" => "h",
        "m" => "m",
        "min" => "m",
        "minute" => "m",
        "minutes" => "m",
        "s" => "s",
        "sec" => "s",
        "second" => "s",
        "seconds" => "s",
    }
});

pub struct TimeParser;

impl TimeParser {
    pub fn process(input: &str) -> Result<ParsedTime, TimeParseError> {
        let input = input.trim();
        if input.is_empty() {
            return Err(TimeParseError::InvalidFormat);
        }

        if Self::is_relative_time(input) {
            Self::parse_relative(input)
        } else {
            Self::parse_absolute(input)
        }
    }

    pub fn is_relative_time(input: &str) -> bool {
        RELATIVE_PATTERN.is_match(input.trim())
    }

    pub fn parse_relative(input: &str) -> Result<ParsedTime, TimeParseError> {
        let input = input.trim();
        let now = Utc::now();
        let mut period = Period::default();
        let mut duration = TimeDuration::default();

        let parts: Vec<&str> = input.split_whitespace().collect();
        let mut i = 0;

        while i < parts.len() {
            let part = parts[i];

            if part.starts_with('-') {
                if part.len() > 1 {
                    if let Some(num) = part.strip_prefix('-') {
                        if let Ok(n) = num.parse::<i64>() {
                            if i + 1 < parts.len() {
                                let unit_str = parts[i + 1];
                                match UNIT_MAP.get(unit_str.to_lowercase().as_str()) {
                                    Some(&"y") => period.years -= n,
                                    Some(&"mo") => period.months -= n,
                                    Some(&"w") => period.days -= n * 7,
                                    Some(&"d") => period.days -= n,
                                    Some(&"h") => duration.hours -= n,
                                    Some(&"m") => duration.minutes -= n,
                                    Some(&"s") => duration.seconds -= n,
                                    _ => return Err(TimeParseError::InvalidUnit),
                                }
                                i += 2;
                                continue;
                            }
                        }
                    }

                    if let Some(caps) = SINGLE_TOKEN_RELATIVE.captures(part) {
                        let n: i64 = caps[1].parse().unwrap();
                        let unit_str = &caps[2];
                        match UNIT_MAP.get(unit_str.to_lowercase().as_str()) {
                            Some(&"y") => period.years += n,
                            Some(&"mo") => period.months += n,
                            Some(&"w") => period.days += n * 7,
                            Some(&"d") => period.days += n,
                            Some(&"h") => duration.hours += n,
                            Some(&"m") => duration.minutes += n,
                            Some(&"s") => duration.seconds += n,
                            _ => return Err(TimeParseError::InvalidUnit),
                        }
                        i += 1;
                        continue;
                    }
                }
                i += 1;
                continue;
            }

            if let Ok(n) = part.parse::<i64>() {
                if i + 1 < parts.len() {
                    let unit_str = parts[i + 1];
                    match UNIT_MAP.get(unit_str.to_lowercase().as_str()) {
                        Some(&"y") => period.years += n,
                        Some(&"mo") => period.months += n,
                        Some(&"w") => period.days += n * 7,
                        Some(&"d") => period.days += n,
                        Some(&"h") => duration.hours += n,
                        Some(&"m") => duration.minutes += n,
                        Some(&"s") => duration.seconds += n,
                        _ => return Err(TimeParseError::InvalidUnit),
                    }
                    i += 2;
                    continue;
                }
            }

            if let Some(caps) = SINGLE_TOKEN_RELATIVE.captures(part) {
                let n: i64 = caps[1].parse().unwrap();
                let unit_str = &caps[2];
                match UNIT_MAP.get(unit_str.to_lowercase().as_str()) {
                    Some(&"y") => period.years += n,
                    Some(&"mo") => period.months += n,
                    Some(&"w") => period.days += n * 7,
                    Some(&"d") => period.days += n,
                    Some(&"h") => duration.hours += n,
                    Some(&"m") => duration.minutes += n,
                    Some(&"s") => duration.seconds += n,
                    _ => return Err(TimeParseError::InvalidUnit),
                }
                i += 1;
                continue;
            }

            i += 1;
        }

        let mut result = now;

        if period.years != 0 || period.months != 0 || period.days != 0 {
            result -= Duration::days(period.days);
            result -= Duration::days(period.months * 30);
            result -= Duration::days(period.years * 365);
        }

        if duration.hours != 0 || duration.minutes != 0 || duration.seconds != 0 {
            result -= Duration::hours(duration.hours);
            result -= Duration::minutes(duration.minutes);
            result -= Duration::seconds(duration.seconds);
        }

        let timestamp = result.timestamp();

        if timestamp > now.timestamp() {
            return Err(TimeParseError::FutureTime);
        }

        Ok(ParsedTime {
            timestamp,
            is_relative: true,
        })
    }

    pub fn parse_absolute(input: &str) -> Result<ParsedTime, TimeParseError> {
        let input = input.trim();
        let now = Utc::now();

        let datetime = if ABSOLUTE_PATTERN_DATETIME.is_match(input) {
            let formats = [
                "%Y-%m-%d %H:%M:%S",
                "%Y-%m-%d %H:%M",
                "%Y/%m/%d %H:%M:%S",
                "%Y/%m/%d %H:%M",
            ];
            let mut parsed = None;
            for fmt in formats {
                if let Ok(dt) = NaiveDateTime::parse_from_str(input, fmt) {
                    parsed = Some(dt);
                    break;
                }
            }
            parsed.ok_or(TimeParseError::InvalidDate)?
        } else if ABSOLUTE_PATTERN_DATE.is_match(input) {
            let formats = ["%Y-%m-%d", "%Y/%m/%d"];
            let mut parsed = None;
            for fmt in formats {
                if let Ok(d) = NaiveDate::parse_from_str(input, fmt) {
                    parsed = Some(d.and_hms_opt(0, 0, 0).unwrap());
                    break;
                }
            }
            parsed.ok_or(TimeParseError::InvalidDate)?
        } else if ABSOLUTE_PATTERN_COMPACT.is_match(input) {
            NaiveDate::parse_from_str(input, "%Y%m%d")
                .map_err(|_| TimeParseError::InvalidDate)?
                .and_hms_opt(0, 0, 0)
                .ok_or(TimeParseError::InvalidDate)?
        } else {
            return Err(TimeParseError::InvalidFormat);
        };

        let result = Utc.from_utc_datetime(&datetime);
        let timestamp = result.timestamp();

        if timestamp > now.timestamp() {
            return Err(TimeParseError::FutureTime);
        }

        Ok(ParsedTime {
            timestamp,
            is_relative: false,
        })
    }

    pub fn start_of_day(timestamp: i64) -> i64 {
        let dt = Utc.timestamp_opt(timestamp, 0).unwrap();
        let start = dt.date_naive().and_hms_opt(0, 0, 0).unwrap();
        Utc.from_utc_datetime(&start).timestamp()
    }

    pub fn start_of_hour(timestamp: i64) -> i64 {
        let dt = Utc.timestamp_opt(timestamp, 0).unwrap();
        let start = dt.date_naive().and_hms_opt(dt.hour(), 0, 0).unwrap();
        Utc.from_utc_datetime(&start).timestamp()
    }

    pub fn now_timestamp() -> i64 {
        Utc::now().timestamp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_relative_time() {
        assert!(TimeParser::is_relative_time("1d"));
        assert!(TimeParser::is_relative_time("7d"));
        assert!(TimeParser::is_relative_time("1y 2mo 3d"));
        assert!(TimeParser::is_relative_time("-1d"));
        assert!(!TimeParser::is_relative_time("2024-01-15"));
        assert!(!TimeParser::is_relative_time("2024/01/15 14:30"));
    }

    #[test]
    fn test_parse_absolute_date() {
        let result = TimeParser::parse_absolute("2024-01-15").unwrap();
        assert!(!result.is_relative);
        assert!(result.timestamp > 0);
    }

    #[test]
    fn test_parse_absolute_compact() {
        let result = TimeParser::parse_absolute("20240115").unwrap();
        assert!(!result.is_relative);
        assert!(result.timestamp > 0);
    }

    #[test]
    fn test_start_of_day() {
        let timestamp = Utc::now().timestamp();
        let start = TimeParser::start_of_day(timestamp);
        let dt = Utc.timestamp_opt(start, 0).unwrap();
        assert_eq!(dt.hour(), 0);
        assert_eq!(dt.minute(), 0);
        assert_eq!(dt.second(), 0);
    }

    #[test]
    fn test_start_of_hour() {
        let timestamp = Utc::now().timestamp();
        let start = TimeParser::start_of_hour(timestamp);
        let dt = Utc.timestamp_opt(start, 0).unwrap();
        assert_eq!(dt.minute(), 0);
        assert_eq!(dt.second(), 0);
    }

    #[test]
    fn test_relative_time_days() {
        let result = TimeParser::process("7d").unwrap();
        assert!(result.is_relative);
        let now = TimeParser::now_timestamp();
        assert!(result.timestamp > now - 8 * 86400);
        assert!(result.timestamp < now - 6 * 86400);
    }

    #[test]
    fn test_relative_time_hours() {
        let result = TimeParser::process("24h").unwrap();
        assert!(result.is_relative);
        let now = TimeParser::now_timestamp();
        assert!(result.timestamp > now - 25 * 3600);
        assert!(result.timestamp < now - 23 * 3600);
    }

    #[test]
    fn test_relative_time_months() {
        let result = TimeParser::process("1mo").unwrap();
        assert!(result.is_relative);
        let now = TimeParser::now_timestamp();
        assert!(result.timestamp > now - 31 * 86400);
        assert!(result.timestamp < now - 29 * 86400);
    }

    #[test]
    fn test_absolute_time() {
        let result = TimeParser::process("2024-01-01").unwrap();
        assert!(!result.is_relative);
        assert!(result.timestamp >= 1704067200);
        assert!(result.timestamp < 1704153600);
    }

    #[test]
    fn test_absolute_time_with_slash() {
        let result = TimeParser::process("2024/01/15 14:30");
        assert!(result.is_ok());
    }

    #[test]
    fn test_future_time_rejected() {
        let result = TimeParser::process("2099-01-01");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_unit() {
        let result = TimeParser::process("7x");
        assert!(result.is_err());
    }

    #[test]
    fn chinese_time_units_are_not_supported() {
        for input in [
            "1年", "1月", "1周", "1星期", "1天", "1日", "1小时", "1分", "1分钟", "1秒",
        ] {
            assert!(
                TimeParser::process(input).is_err(),
                "{input} should be rejected"
            );
        }
    }
}
