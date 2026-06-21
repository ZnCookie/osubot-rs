use crate::commands::parse_command;
use crate::types::{Command, GameMode};

#[test]
fn test_where_qq_basic() {
    let cmd = parse_command("where qq=1234567", None).unwrap();
    assert_eq!(
        cmd,
        Command::QueryMentionedUser {
            qq: 1234567,
            mode: None,
        }
    );
}

#[test]
fn test_where_qq_with_mode() {
    let cmd = parse_command("where qq=1234567,1", None).unwrap();
    assert_eq!(
        cmd,
        Command::QueryMentionedUser {
            qq: 1234567,
            mode: Some(GameMode::Taiko),
        }
    );
}

#[test]
fn test_where_qq_invalid_number() {
    assert!(parse_command("where qq=abc", None).is_none());
}

#[test]
fn test_where_qq_empty() {
    assert!(parse_command("where qq=", None).is_none());
}

#[test]
fn test_where_qq_invalid_mode() {
    assert!(parse_command("where qq=123,99", None).is_none());
}

#[test]
fn test_where_qq_in_text() {
    let cmd = parse_command("where @1234567", None).unwrap();
    assert_eq!(
        cmd,
        Command::QueryMentionedUser {
            qq: 1234567,
            mode: None,
        }
    );
}

#[test]
fn test_where_qq_in_text_with_mode() {
    let cmd = parse_command("where @1234567,1", None).unwrap();
    assert_eq!(
        cmd,
        Command::QueryMentionedUser {
            qq: 1234567,
            mode: Some(GameMode::Taiko),
        }
    );
}

#[test]
fn test_where_qq_in_text_non_numeric_returns_none() {
    assert!(parse_command("where @ZnCookie", None).is_none());
}
