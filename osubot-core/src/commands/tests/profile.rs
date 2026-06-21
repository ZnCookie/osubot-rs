use crate::commands::parse_command;
use crate::types::Command;

#[test]
fn test_profile_self() {
    let cmd = parse_command("!profile", None).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: None,
            qq: None,
        }
    );
}

#[test]
fn test_profile_mention() {
    let cmd = parse_command("!profile", Some(123456)).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: None,
            qq: Some(123456),
        }
    );
}

#[test]
fn test_profile_with_username() {
    let cmd = parse_command("!profile ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: Some("ZnCookie".to_string()),
            qq: None,
        }
    );
}

#[test]
fn test_profile_username_with_mention() {
    let cmd = parse_command("!profile ZnCookie", Some(123456)).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: Some("ZnCookie".to_string()),
            qq: None,
        }
    );
}

#[test]
fn test_profile_with_spaces_around_username() {
    let cmd = parse_command("!profile  ZnCookie  ", None).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: Some("ZnCookie".to_string()),
            qq: None,
        }
    );
}

#[test]
fn test_profile_qq_in_text() {
    let cmd = parse_command("!profile @123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: None,
            qq: Some(123456),
        }
    );
}

#[test]
fn test_profile_qq_in_text_non_numeric_returns_none() {
    assert!(parse_command("!profile @ZnCookie", None).is_none());
}

#[test]
fn test_profile_not_matched_as_p() {
    let cmd = parse_command("!profile", None).unwrap();
    assert!(matches!(cmd, Command::ProfileCard { .. }));
}
