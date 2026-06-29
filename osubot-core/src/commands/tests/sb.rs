use crate::commands::parse_command;
use crate::types::{Command, GameMode, Server};

#[test]
fn test_sb_empty() {
    let cmd = parse_command("?", None).unwrap();
    assert_eq!(
        cmd,
        Command::QuerySelf {
            mode: None,
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_tilde_only() {
    let cmd = parse_command("?~", None).unwrap();
    assert_eq!(
        cmd,
        Command::QuerySelf {
            mode: None,
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_tilde_mode() {
    let cmd = parse_command("?~0", None).unwrap();
    assert_eq!(
        cmd,
        Command::QuerySelf {
            mode: Some(GameMode::Osu),
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_digit_shortcut() {
    let cmd = parse_command("?0", None).unwrap();
    assert_eq!(
        cmd,
        Command::QuerySelf {
            mode: Some(GameMode::Osu),
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_digit_shortcut_mania() {
    let cmd = parse_command("?3", None).unwrap();
    assert_eq!(
        cmd,
        Command::QuerySelf {
            mode: Some(GameMode::Mania),
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_where() {
    let cmd = parse_command("?where ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::QueryUser {
            username: "ZnCookie".to_string(),
            mode: None,
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_bind() {
    let cmd = parse_command("?bind ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::Bind {
            username: "ZnCookie".to_string(),
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_unbind() {
    let cmd = parse_command("?unbind", None).unwrap();
    assert_eq!(
        cmd,
        Command::Unbind {
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_help() {
    let cmd = parse_command("?help", None).unwrap();
    assert_eq!(cmd, Command::Help);
}

#[test]
fn test_sb_mode() {
    let cmd = parse_command("?mode", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: None,
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_mode_with_value() {
    let cmd = parse_command("?mode 0", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Osu),
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_highlight() {
    let cmd = parse_command("?今日高光", None).unwrap();
    assert_eq!(
        cmd,
        Command::Highlight {
            mode: None,
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_highlight_with_mode() {
    let cmd = parse_command("?今日高光 0", None).unwrap();
    assert_eq!(
        cmd,
        Command::Highlight {
            mode: Some(GameMode::Osu),
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_scoring_pass() {
    let cmd = parse_command("?p", None).unwrap();
    assert_eq!(cmd.server(), Server::PpsySb);
}

#[test]
fn test_sb_scoring_recent_with_mode() {
    let cmd = parse_command("?r :1", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: Some(GameMode::Taiko),
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
            server: Server::PpsySb,
        }
    );
}

#[test]
fn test_sb_scoring_best() {
    let cmd = parse_command("?b", None).unwrap();
    assert_eq!(cmd.server(), Server::PpsySb);
}
