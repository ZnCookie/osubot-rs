use crate::commands::parse_command;
use crate::types::{Command, CommandGroup, GameMode};

#[test]
fn test_help_command() {
    let cmd = parse_command("!help", None).unwrap();
    assert_eq!(cmd, Command::Help);
}

#[test]
fn test_pass_self() {
    let cmd = parse_command("!p", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_mode() {
    let cmd = parse_command("!p :1", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Taiko),
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_username() {
    let cmd = parse_command("!p ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_username_mode() {
    let cmd = parse_command("!p ZnCookie :2", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Catch),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_recent_self() {
    let cmd = parse_command("!r", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_recent_multiple() {
    let cmd = parse_command("!rs", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 20,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_mention() {
    let cmd = parse_command("!p", Some(123456)).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: Some(123456),
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_qq_in_text() {
    let cmd = parse_command("!p @123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: Some(123456),
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_qq_in_text_non_numeric_returns_none() {
    assert!(parse_command("!p @ZnCookie", None).is_none());
}

#[test]
fn test_pass_qq_equals() {
    let cmd = parse_command("!p qq=123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: Some(123456),
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_qq_equals_and_username_mutually_exclusive() {
    assert!(parse_command("!p ZnCookie qq=123456", None).is_none());
}

#[test]
fn test_beatmap_audio_qq_equals() {
    let cmd = parse_command("!a qq=123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: Some(123456),
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        }
    );
}

#[test]
fn test_pass_no_conflict_with_profile() {
    let cmd = parse_command("!profile", None).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: None,
            qq: None
        }
    );
}

#[test]
fn test_pass_mode_no_space() {
    let cmd = parse_command("!p:3", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Mania),
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_with_hash() {
    let cmd = parse_command("!p #2", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 2,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_multiple_with_hash() {
    let cmd = parse_command("!ps #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_multiple_username_hash() {
    let cmd = parse_command("!ps ZnCookie #3", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 3,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_pass_multiple_username_mode_hash() {
    let cmd = parse_command("!ps ZnCookie :2 #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Catch),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_hash_clamp_max() {
    let cmd = parse_command("!ps #2000", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 200,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_hash_zero_ignored() {
    let cmd = parse_command("!p #0", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_hash_garbage_ignored_with_username() {
    let cmd = parse_command("!p ZnCookie #xyz", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_hash_garbage_ignored_self() {
    let cmd = parse_command("!p #xyz", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_recent_with_hash() {
    let cmd = parse_command("!r #3", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 3,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_recent_multiple_with_hash() {
    let cmd = parse_command("!rs #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_ps_invalid_mode_returns_none() {
    let result = parse_command("!ps :xyz", None).unwrap();
    assert_eq!(
        result,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 20,
            limit_end: None,
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_fullwidth_exclamation() {
    let cmd = parse_command("！profile", None).unwrap();
    assert_eq!(
        cmd,
        Command::ProfileCard {
            username: None,
            qq: None,
        }
    );

    let cmd = parse_command("！p", None).unwrap();
    assert!(matches!(cmd, Command::Pass { .. }));

    let cmd = parse_command("！rs", None).unwrap();
    assert!(matches!(cmd, Command::Recent { .. }));
}

#[test]
fn parse_ps_empty_mode_returns_none() {
    let result = parse_command("!ps :", None);
    assert_eq!(
        result,
        Some(Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 20,
            is_summary: true,
            limit_end: None,
            filters: None,
        })
    );
}

#[test]
fn test_command_group_name() {
    assert_eq!(
        Command::QuerySelf { mode: None }.group_name(),
        CommandGroup::Query
    );
    assert_eq!(
        Command::QueryUser {
            username: "x".into(),
            mode: None
        }
        .group_name(),
        CommandGroup::Query
    );
    assert_eq!(
        Command::QueryMentionedUser { qq: 1, mode: None }.group_name(),
        CommandGroup::Query
    );
    assert_eq!(
        Command::Bind {
            username: "x".into()
        }
        .group_name(),
        CommandGroup::Bind
    );
    assert_eq!(Command::Unbind.group_name(), CommandGroup::Bind);
    assert_eq!(
        Command::Highlight { mode: None }.group_name(),
        CommandGroup::Highlight
    );
    assert_eq!(
        Command::ProfileCard {
            username: None,
            qq: None
        }
        .group_name(),
        CommandGroup::Profile
    );
    assert_eq!(
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
        .group_name(),
        CommandGroup::Score
    );
    assert_eq!(
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
        .group_name(),
        CommandGroup::Score
    );
    assert_eq!(
        Command::SetDefaultMode { mode: None }.group_name(),
        CommandGroup::Mode
    );
}

#[test]
fn test_pv_not_matched() {
    assert!(parse_command("!pv", None).is_none());
}

#[test]
fn test_rabc_not_matched() {
    assert!(parse_command("!rabc", None).is_none());
}

#[test]
fn test_punderscore_not_matched() {
    assert!(parse_command("!p_test", None).is_none());
}

#[test]
fn test_phyphen_not_matched() {
    assert!(parse_command("!r-test", None).is_none());
}

#[test]
fn test_profile_not_matched_as_p() {
    let cmd = parse_command("!profile", None).unwrap();
    assert!(matches!(cmd, Command::ProfileCard { .. }));
}

#[test]
fn test_ps_not_affected() {
    let cmd = parse_command("!ps", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 20,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_basic() {
    let cmd = parse_command("!s 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_with_mods() {
    let cmd = parse_command("!s 123456 +HDDT", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: Some(vec!["mod=HDDT".to_string()]),
            limit: 1,
            is_all: false,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_with_mode_and_username() {
    let cmd = parse_command("!s :2 123456 ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Catch),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_score_id() {
    let cmd = parse_command("!s 12345678901", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: Some(12345678901),
            limit: 1,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_all() {
    let cmd = parse_command("!ss 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 20,
            is_all: true,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_no_conflict_with_ps() {
    let cmd = parse_command("!ps", None).unwrap();
    assert!(matches!(cmd, Command::Pass { .. }));
}

#[test]
fn test_score_on_beatmap_with_limit() {
    let cmd = parse_command("!s 123456 #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 5,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_qq_in_text() {
    let cmd = parse_command("!s @123456 789012", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: Some(123456),
            beatmap_id: Some(789012),
            score_id: None,
            limit: 1,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_username_and_qq_mutually_exclusive() {
    assert!(parse_command("!s 123 ZnCookie @999", None).is_none());
    assert!(parse_command("!s 123 @999 ZnCookie", None).is_none());
}

#[test]
fn test_score_on_beatmap_at_non_numeric_returns_none() {
    assert!(parse_command("!s 123456 @ZnCookie", None).is_none());
}

#[test]
fn test_score_on_beatmap_multi_word_username() {
    let cmd = parse_command("!s 123456 My Name", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: Some("My Name".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_multi_word_username_with_mode() {
    let cmd = parse_command("!s :2 123456 Zhang San #3 +HD", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Catch),
            username: Some("Zhang San".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: Some(vec!["mod=HD".to_string()]),
            limit: 3,
            is_all: false,
            limit_end: None,
        }
    );
}

#[test]
fn test_score_on_beatmap_single_word_username_still_works() {
    let cmd = parse_command("!s 123456 ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            is_all: false,
            filters: None,
            limit_end: None,
        }
    );
}

// === Unified format tests ===

#[test]
fn test_pass_new_format_mode_user() {
    let cmd = parse_command("!p :1 ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Taiko),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_mode_user_hash() {
    let cmd = parse_command("!p :2 ZnCookie #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Catch),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_filters() {
    let cmd = parse_command("!ps ZnCookie miss=1,combo=500", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 20,
            limit_end: None,
            is_summary: true,
            filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
        }
    );
}

#[test]
fn test_pass_new_format_hash_without_hash() {
    let cmd = parse_command("!r :3 miss=1,combo=500 5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: Some(GameMode::Mania),
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
        }
    );
}

#[test]
fn test_pass_new_format_mode_alias_rejected() {
    let cmd = parse_command("!p :std ZnCookie", None).unwrap();
    match cmd {
        Command::Pass { mode, .. } => assert_eq!(mode, None),
        _ => panic!("expected Command::Pass"),
    }
}

#[test]
fn test_pass_new_format_invalid_mode_returns_none() {
    let cmd = parse_command("!p :99", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_range_ps() {
    let cmd = parse_command("!ps #2-10", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 2,
            limit_end: Some(10),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_multi_word_username() {
    let cmd = parse_command("!ps Zhang San miss=1", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: Some("Zhang San".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 20,
            limit_end: None,
            is_summary: true,
            filters: Some(vec!["miss=1".to_string()]),
        }
    );
}

#[test]
fn test_pass_new_format_qq_user() {
    let cmd = parse_command("!ps @123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: Some(123456),
            beatmap_id: None,
            score_id: None,
            limit: 20,
            limit_end: None,
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_mods_and_filters() {
    let cmd = parse_command("!p +HDHR,miss=1", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: Some(vec!["miss=1".to_string(), "mod=HDHR".to_string()]),
        }
    );
}

#[test]
fn test_pass_new_format_mods_only() {
    let cmd = parse_command("!p +HDHR", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: Some(vec!["mod=HDHR".to_string()]),
        }
    );
}

#[test]
fn test_pass_new_format_bare_command() {
    let cmd = parse_command("!p", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_mode_user_limit() {
    let cmd = parse_command("!p :1 ZnCookie #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Taiko),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_new_format_mention_fallback() {
    let cmd = parse_command("!p :1", Some(999999)).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: Some(GameMode::Taiko),
            username: None,
            qq: Some(999999),
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

// === Unified !s/!ss format tests ===

#[test]
fn test_s_new_format_basic() {
    let cmd = parse_command("!s 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: None,
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_mode_beatmap_user() {
    let cmd = parse_command("!s :2 123456 ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Catch),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: None,
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_mods_filters() {
    let cmd = parse_command("!s :1 123456 ZnCookie +HDHR,miss=1 #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Taiko),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: Some(vec!["miss=1".to_string(), "mod=HDHR".to_string()]),
            limit: 5,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_score_id() {
    let cmd = parse_command("!s :3 1234567890", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Mania),
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: Some(1234567890),
            filters: None,
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_ss_new_format_range() {
    let cmd = parse_command("!ss 123456 #2-10", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: None,
            limit: 2,
            limit_end: Some(10),
            is_all: true,
        }
    );
}

#[test]
fn test_s_new_format_multi_word_username() {
    let cmd = parse_command("!s :2 123456 Zhang San +DT,miss=1", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Catch),
            username: Some("Zhang San".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: Some(vec!["miss=1".to_string(), "mod=DT".to_string()]),
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_qq_user() {
    let cmd = parse_command("!s :2 123456 @123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: Some(GameMode::Catch),
            username: None,
            qq: Some(123456),
            beatmap_id: Some(123456),
            score_id: None,
            filters: None,
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_inline_filters_no_plus() {
    let cmd = parse_command("!s 123456 ZnCookie miss=1,combo=500", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: Some(vec!["miss=1".to_string(), "combo=500".to_string()]),
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_bare_command() {
    let cmd = parse_command("!s", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            filters: None,
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_mode_alias_rejected() {
    let cmd = parse_command("!s :mania 123456", None).unwrap();
    match cmd {
        Command::ScoreOnBeatmap { mode, .. } => assert_eq!(mode, None),
        _ => panic!("expected Command::ScoreOnBeatmap"),
    }
}

#[test]
fn test_s_new_format_invalid_mode_returns_none() {
    let cmd = parse_command("!s :99 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: None,
            limit: 1,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_new_format_implicit_hash() {
    let cmd = parse_command("!s 123456 5", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            filters: None,
            limit: 5,
            limit_end: None,
            is_all: false,
        }
    );
}

#[test]
fn test_s_two_beatmap_level_numbers_rejected() {
    // 第二个 >200 的纯数字与既有 beatmap_id 冲突，应被拒绝（不再静默 clamp 成位置）。
    assert!(parse_command("!s 123456 999999", None).is_none());
}

#[test]
fn test_ss_new_format_bare_command() {
    let cmd = parse_command("!ss", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            filters: None,
            limit: 20,
            limit_end: None,
            is_all: true,
        }
    );
}

// === !mode tests ===

#[test]
fn test_get_default_mode() {
    let cmd = parse_command("!mode", None).unwrap();
    assert_eq!(cmd, Command::SetDefaultMode { mode: None });
}

#[test]
fn test_set_default_mode_osu() {
    let cmd = parse_command("!mode 0", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Osu)
        }
    );
}

#[test]
fn test_set_default_mode_mania() {
    let cmd = parse_command("!mode 3", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Mania)
        }
    );
}

#[test]
fn test_set_default_mode_invalid_is_query() {
    let cmd = parse_command("!mode 5", None).unwrap();
    assert_eq!(cmd, Command::SetDefaultMode { mode: None });
}

#[test]
fn test_set_default_mode_trailing_space() {
    let cmd = parse_command("!mode ", None).unwrap();
    assert_eq!(cmd, Command::SetDefaultMode { mode: None });
}

#[test]
fn test_set_default_mode_multi_spaces() {
    let cmd = parse_command("!mode  0", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Osu)
        }
    );
}

#[test]
fn test_set_default_mode_fullwidth_exclamation() {
    let cmd = parse_command("！mode 0", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Osu)
        }
    );
}

#[test]
fn test_set_default_mode_string_name_gives_query() {
    let cmd = parse_command("!mode osu", None).unwrap();
    assert_eq!(cmd, Command::SetDefaultMode { mode: None });
}

#[test]
fn test_set_default_mode_taiko() {
    let cmd = parse_command("!mode 1", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Taiko)
        }
    );
}

#[test]
fn test_set_default_mode_catch() {
    let cmd = parse_command("!mode 2", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Catch)
        }
    );
}

#[test]
fn test_set_default_mode_extra_args_gives_query() {
    let cmd = parse_command("!mode 0 extra", None).unwrap();
    assert_eq!(cmd, Command::SetDefaultMode { mode: None });
}

#[test]
fn test_mode_newline_only() {
    let cmd = parse_command("!mode\n", None).unwrap();
    assert_eq!(cmd, Command::SetDefaultMode { mode: None });
}

#[test]
fn test_mode_tab_separator() {
    let cmd = parse_command("!mode\t0", None).unwrap();
    assert_eq!(
        cmd,
        Command::SetDefaultMode {
            mode: Some(GameMode::Osu)
        }
    );
}

#[test]
fn test_mode_no_space_not_mode_command() {
    assert!(parse_command("!mode1", None).is_none());
}

// === Misc scoring tests ===

#[test]
fn test_r_with_beatmap_id() {
    let cmd = parse_command("!r 12345", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(12345),
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_p_with_hash_and_mode_order_independent() {
    let cmd1 = parse_command("!p :1 #5", None);
    let cmd2 = parse_command("!p #5 :1", None);
    assert_eq!(cmd1, cmd2);
    if let Some(Command::Pass { mode, limit, .. }) = cmd1 {
        assert_eq!(mode, Some(GameMode::Taiko));
        assert_eq!(limit, 5);
    } else {
        panic!("expected Pass command");
    }
}

// === !b / !bs tests ===

#[test]
fn test_best_self() {
    let cmd = parse_command("!b", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_list_self() {
    let cmd = parse_command("!bs", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 20,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_mode() {
    let cmd = parse_command("!b :1", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: Some(GameMode::Taiko),
            username: None,
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_username() {
    let cmd = parse_command("!b ZnCookie", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_mention() {
    let cmd = parse_command("!b", Some(123456)).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: Some(123456),
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_qq_in_text() {
    let cmd = parse_command("!b @123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: Some(123456),
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_with_hash() {
    let cmd = parse_command("!b #3", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 3,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_list_with_hash() {
    let cmd = parse_command("!bs #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 5,
            is_summary: true,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_list_range() {
    let cmd = parse_command("!bs #2-10", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 2,
            limit_end: Some(10),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_best_with_mods() {
    let cmd = parse_command("!b +HDHR", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: Some(vec!["mod=HDHR".to_string()]),
        }
    );
}

#[test]
fn test_best_with_filters() {
    let cmd = parse_command("!bs pp>=200,miss=0", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 20,
            is_summary: true,
            limit_end: None,
            filters: Some(vec!["pp>=200".to_string(), "miss=0".to_string()]),
        }
    );
}

#[test]
fn test_best_full_args() {
    let cmd = parse_command("!bs :2 ZnCookie +DT #5-10", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: Some(GameMode::Catch),
            username: Some("ZnCookie".to_string()),
            qq: None,
            limit: 5,
            limit_end: Some(10),
            is_summary: true,
            filters: Some(vec!["mod=DT".to_string()]),
        }
    );
}

#[test]
fn test_best_no_conflict_with_profile() {
    let cmd = parse_command("!b", None).unwrap();
    assert!(matches!(cmd, Command::Best { .. }));
}

#[test]
fn test_best_list_not_matched_as_best() {
    let cmd = parse_command("!bs", None).unwrap();
    assert!(matches!(
        cmd,
        Command::Best {
            is_summary: true,
            ..
        }
    ));
}

#[test]
fn test_best_mode_no_space() {
    let cmd = parse_command("!b:3", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: Some(GameMode::Mania),
            username: None,
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
    );
}

#[test]
fn test_best_group_name() {
    assert_eq!(
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
        .group_name(),
        CommandGroup::Score
    );
}

#[test]
fn test_best_command_name() {
    assert_eq!(
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 1,
            is_summary: false,
            limit_end: None,
            filters: None,
        }
        .command_name(),
        "!b"
    );
}

#[test]
fn test_beatmap_audio_self() {
    let cmd = parse_command("!a", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        }
    );
}

#[test]
fn test_beatmap_audio_username_mode() {
    let cmd = parse_command("!a ZnCookie :1", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: Some(GameMode::Taiko),
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        }
    );
}

#[test]
fn test_beatmap_audio_beatmap_id() {
    let cmd = parse_command("!a 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        }
    );
}

#[test]
fn test_beatmap_audio_group_is_beatmap_preview() {
    let cmd = parse_command("!a", None).unwrap();
    assert_eq!(cmd.group_name(), CommandGroup::BeatmapPreview);
}

#[test]
fn test_beatmap_audio_with_filters() {
    let cmd = parse_command("!a ZnCookie +HD", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: Some("ZnCookie".to_string()),
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: Some(vec!["mod=HD".to_string()]),
            explicit_position: false,
        }
    );
}

#[test]
fn test_pass_bare_number_limit() {
    let cmd = parse_command("!p5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_bare_number_with_space() {
    let cmd = parse_command("!p 5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_best_bare_number_limit() {
    let cmd = parse_command("!b5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 5,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_best_list_bare_range() {
    let cmd = parse_command("!bs2-10", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 2,
            limit_end: Some(10),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_best_list_bare_range_with_space() {
    let cmd = parse_command("!bs 2-10", None).unwrap();
    assert_eq!(
        cmd,
        Command::Best {
            mode: None,
            username: None,
            qq: None,
            limit: 2,
            limit_end: Some(10),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_pass_large_number_still_beatmap_id() {
    let cmd = parse_command("!p 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_score_large_number_still_beatmap_id() {
    let cmd = parse_command("!s 123456", None).unwrap();
    assert_eq!(
        cmd,
        Command::ScoreOnBeatmap {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(123456),
            score_id: None,
            limit: 1,
            limit_end: None,
            is_all: false,
            filters: None,
        }
    );
}

#[test]
fn test_two_beatmap_level_numbers_rejected() {
    // 两个 >200 的纯数字同时出现：beatmap_id 语义冲突，应被拒绝而非静默当成位置。
    assert!(parse_command("!p 123456 789", None).is_none());
    assert!(parse_command("!p 123456 12345", None).is_none());
}

#[test]
fn test_single_command_range_rejected() {
    // single 命令不接受区间（区间请用 summary），不再静默吞尾取第 1 条。
    assert!(parse_command("!p1-100", None).is_none());
    assert!(parse_command("!p 1-100", None).is_none());
    assert!(parse_command("!b1-5", None).is_none());
    assert!(parse_command("!p #2-10", None).is_none());
    // summary 命令的区间仍正常。
    assert!(parse_command("!ps1-100", None).is_some());
}

#[test]
fn test_summary_range_start_exceeds_max_rejected() {
    // 区间起点超出 MAX_LIMIT 直接拒绝，不静默当成用户名。
    assert!(parse_command("!ps300-400", None).is_none());
    assert!(parse_command("!ps 300-400", None).is_none());
    assert!(parse_command("!rs300-400", None).is_none());
    assert!(parse_command("!bs300-400", None).is_none());
    // 起点在范围内、终点超出的区间仍正常 clamp。
    let cmd = parse_command("!ps1-300", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: Some(200),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_score_id_and_beatmap_id_coexist_rejected() {
    // score_id 与 beatmap_id 不可共存，解析期即拒绝。
    assert!(parse_command("!a 12345678 456", None).is_none());
    assert!(parse_command("!s 12345678 456", None).is_none());
}

#[test]
fn test_recent_bare_number_limit() {
    let cmd = parse_command("!r5", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_recent_list_bare_range() {
    let cmd = parse_command("!rs3-8", None).unwrap();
    assert_eq!(
        cmd,
        Command::Recent {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 3,
            limit_end: Some(8),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_pass_bare_number_clamp_max() {
    let cmd = parse_command("!p2000", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: Some(2000),
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_pass_summary_bare_range_clamp() {
    let cmd = parse_command("!ps1-300", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: Some(200),
            is_summary: true,
            filters: None,
        }
    );
}

#[test]
fn test_pass_bare_zero_clamps_to_one() {
    let cmd = parse_command("!p0", None).unwrap();
    assert_eq!(
        cmd,
        Command::Pass {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            limit_end: None,
            is_summary: false,
            filters: None,
        }
    );
}

#[test]
fn test_beatmap_audio_bare_number_position() {
    let cmd = parse_command("!a 5", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            filters: None,
            explicit_position: true,
        }
    );
}

#[test]
fn test_beatmap_audio_hash_index() {
    let cmd = parse_command("!a #5", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 5,
            filters: None,
            explicit_position: true,
        }
    );
}

#[test]
fn test_beatmap_audio_bare_range_rejected() {
    assert!(parse_command("!a 5-10", None).is_none());
}

#[test]
fn test_beatmap_audio_hash_range_rejected() {
    assert!(parse_command("!a #5-10", None).is_none());
}

#[test]
fn test_beatmap_audio_score_id() {
    let cmd = parse_command("!a 12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: Some(12345678),
            limit: 1,
            filters: None,
            explicit_position: false,
        }
    );
}

#[test]
fn test_beatmap_audio_hash_one_explicit_position() {
    let cmd = parse_command("!a #1", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: true,
        }
    );
}

#[test]
fn test_beatmap_audio_bare_one_explicit_position() {
    let cmd = parse_command("!a 1", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: None,
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: true,
        }
    );
}

#[test]
fn test_beatmap_audio_mode_only_skips_cache() {
    let cmd = parse_command("!a :1", None).unwrap();
    assert_eq!(
        cmd,
        Command::BeatmapAudio {
            mode: Some(GameMode::Taiko),
            username: None,
            qq: None,
            beatmap_id: None,
            score_id: None,
            limit: 1,
            filters: None,
            explicit_position: false,
        }
    );
}
