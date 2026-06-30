use crate::commands::parse_command;
use crate::types::{Command, CommandGroup, GameMode, Server};

#[test]
fn test_rv_bare() {
    let cmd = parse_command("!rv", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_beatmap_id() {
    let cmd = parse_command("!rv 12345", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: Some(12345),
            mode: None,
            mods: None,
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_score_id() {
    let cmd = parse_command("!rv 12345678901", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: Some(12345678901),
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mods() {
    let cmd = parse_command("!rv +HD", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: Some(vec!["HD".to_string()]),
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: Some(vec!["mod=HD".to_string()]),
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mods_and_beatmap() {
    let cmd = parse_command("!rv 99999 +HDDT", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: Some(99999),
            mode: None,
            mods: Some(vec!["HD".to_string(), "DT".to_string()]),
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: Some(vec!["mod=HDDT".to_string()]),
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mode() {
    let cmd = parse_command("!rv :1", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: Some(GameMode::Taiko),
            mods: None,
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mods_and_mode() {
    let cmd = parse_command("!rv +HD :3", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: Some(GameMode::Mania),
            mods: Some(vec!["HD".to_string()]),
            gif: false,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: Some(vec!["mod=HD".to_string()]),
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_rejected_suffix_alphanumeric() {
    assert!(parse_command("!rva", None).is_none());
}

#[test]
fn test_rv_rejected_suffix_hyphen() {
    assert!(parse_command("!rv-bid", None).is_none());
}

#[test]
fn test_rv_rejected_suffix_underscore() {
    assert!(parse_command("!rv_bid", None).is_none());
}

#[test]
fn test_rv_with_username() {
    let cmd = parse_command("!rv abc", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: None,
            username: Some("abc".to_string()),
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_gif() {
    let cmd = parse_command("!rv --gif", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: true,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_gif_short() {
    let cmd = parse_command("!rv -g", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: true,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_beatmap_id_and_gif() {
    let cmd = parse_command("!rv 12345 --gif", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: Some(12345),
            mode: None,
            mods: None,
            gif: true,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mods_and_gif() {
    let cmd = parse_command("!rv +HD --gif", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: Some(vec!["HD".to_string()]),
            gif: true,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: Some(vec!["mod=HD".to_string()]),
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mode_and_gif() {
    let cmd = parse_command("!rv :1 --gif", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: Some(GameMode::Taiko),
            mods: None,
            gif: true,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_all_args_and_gif() {
    let cmd = parse_command("!rv 99999 +DT :3 --gif", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: Some(99999),
            mode: Some(GameMode::Mania),
            mods: Some(vec!["DT".to_string()]),
            gif: true,
            times: None,
            username: None,
            qq: None,
            limit: 1,
            filters: Some(vec!["mod=DT".to_string()]),
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_username_dash_prefix() {
    let cmd = parse_command("!rv -1", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: None,
            username: Some("-1".to_string()),
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_group_is_beatmap_preview() {
    let cmd = parse_command("!rv", None).unwrap();
    assert_eq!(cmd.group_name(), CommandGroup::BeatmapPreview);
}

#[test]
fn test_rv_single_time() {
    let cmd = parse_command("!rv 01:30:000", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: Some(vec![90_000]),
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_two_times() {
    let cmd = parse_command("!rv 01:00:000 02:00:000", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: Some(vec![60_000, 120_000]),
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_beatmap_id_with_time() {
    let cmd = parse_command("!rv 12345 01:30:000", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: Some(12345),
            mode: None,
            mods: None,
            gif: false,
            times: Some(vec![90_000]),
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_time_with_mods_and_mode() {
    let cmd = parse_command("!rv :1 +HD 01:30:000", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: Some(GameMode::Taiko),
            mods: Some(vec!["HD".to_string()]),
            gif: false,
            times: Some(vec![90_000]),
            username: None,
            qq: None,
            limit: 1,
            filters: Some(vec!["mod=HD".to_string()]),
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_two_times_invalid_order() {
    let cmd = parse_command("!rv 02:00:000 01:00:000", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: Some(vec![120_000, 60_000]),
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_three_times_rejected() {
    assert!(parse_command("!rv 01:00:000 02:00:000 03:00:000", None).is_none());
}

#[test]
fn test_rv_username_with_time() {
    let cmd = parse_command("!rv abc 01:30:000", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: false,
            times: Some(vec![90_000]),
            username: Some("abc".to_string()),
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_time_with_gif() {
    let cmd = parse_command("!rv 01:30:000 --gif", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            mods: None,
            gif: true,
            times: Some(vec![90_000]),
            username: None,
            qq: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_qq_equals() {
    let cmd = parse_command("!rv qq=123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            username: None,
            qq: Some(123456),
            mods: None,
            gif: false,
            times: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_mention() {
    let cmd = parse_command("!rv @123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            username: None,
            qq: Some(123456),
            mods: None,
            gif: false,
            times: None,
            limit: 1,
            filters: None,
            explicit_position: false,
            server: Server::Official,
        }
    );
}

#[test]
fn test_rv_with_explicit_position() {
    let cmd = parse_command("!rv 5", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapPreview {
            score_id: None,
            beatmap_id: None,
            mode: None,
            username: None,
            qq: None,
            mods: None,
            gif: false,
            times: None,
            limit: 5,
            filters: None,
            explicit_position: true,
            server: Server::Official,
        }
    );
}
