use crate::commands::common::parse_time_token;

#[test]
fn test_parse_time_token_valid() {
    assert_eq!(parse_time_token("01:30:000"), Some(90_000));
    assert_eq!(parse_time_token("00:00:500"), Some(500));
    assert_eq!(parse_time_token("02:00:319"), Some(120_319));
    assert_eq!(parse_time_token("0:00:000"), Some(0));
    assert_eq!(
        parse_time_token("99:59:999"),
        Some(99 * 60_000 + 59_000 + 999)
    );
}

#[test]
fn test_parse_time_token_invalid() {
    assert_eq!(parse_time_token("abc"), None);
    assert_eq!(parse_time_token("01:30:00"), None);
    assert_eq!(parse_time_token("1:30:0000"), None);
    assert_eq!(parse_time_token(""), None);
    assert_eq!(parse_time_token("01:30:000 "), None);
    assert_eq!(parse_time_token("01:60:000"), None);
    assert_eq!(parse_time_token("01:30:1000"), None);
}
