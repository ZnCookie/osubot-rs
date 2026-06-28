use crate::commands::parse_command;
use crate::types::{Command, CommandGroup, MatchListenAction};

#[test]
fn parse_ml_start_numeric() {
    let cmd = parse_command("!ml 12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_start_mp() {
    let cmd = parse_command("!ml mp12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_start_url() {
    let cmd = parse_command("!ml https://osu.ppy.sh/community/matches/12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_start_url_trailing_slash() {
    let cmd = parse_command("!ml https://osu.ppy.sh/community/matches/12345678/", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_stop_numeric() {
    let cmd = parse_command("!ml stop 12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Stop { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_stop_mp() {
    let cmd = parse_command("!ml stop mp12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Stop { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_stop_url() {
    let cmd = parse_command(
        "!ml stop https://osu.ppy.sh/community/matches/12345678",
        None,
    )
    .unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Stop { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_stop_all() {
    let cmd = parse_command("!ml stop all", None).unwrap();
    assert_eq!(cmd, Command::MatchListen(MatchListenAction::StopAll));
}

#[test]
fn parse_ml_list() {
    let cmd = parse_command("!ml list", None).unwrap();
    assert_eq!(cmd, Command::MatchListen(MatchListenAction::List));
}

#[test]
fn parse_ml_status_numeric() {
    let cmd = parse_command("!ml status 12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Status { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_status_mp() {
    let cmd = parse_command("!ml status mp12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Status { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_status_url() {
    let cmd = parse_command(
        "!ml status https://osu.ppy.sh/community/matches/12345678",
        None,
    )
    .unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Status { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_invalid_empty() {
    assert!(parse_command("!ml", None).is_none());
}

#[test]
fn parse_ml_invalid_bare_stop() {
    assert!(parse_command("!ml stop", None).is_none());
}

#[test]
fn parse_ml_invalid_bare_status() {
    assert!(parse_command("!ml status", None).is_none());
}

#[test]
fn parse_ml_invalid_garbage() {
    assert!(parse_command("!ml asdf", None).is_none());
}

#[test]
fn parse_ml_invalid_malformed_mp() {
    assert!(parse_command("!ml mp", None).is_none());
}

#[test]
fn parse_ml_invalid_negative_id() {
    assert!(parse_command("!ml -1", None).is_none());
}

#[test]
fn parse_ml_rejects_lazer_room() {
    // ponytail: v1 rejects lazer room URLs intentionally; they use a different API.
    let cmd = parse_command("!ml https://osu.ppy.sh/multiplayer/rooms/12345678", None);
    assert!(cmd.is_none());
}

#[test]
fn parse_ml_no_conflict_with_scoring() {
    // Ensure !p, !r, !b, etc. still work
    assert!(parse_command("!p", None).is_some());
    assert!(parse_command("!r", None).is_some());
    assert!(parse_command("!b", None).is_some());
    assert!(parse_command("!s 123", None).is_some());
    assert!(parse_command("!mode 3", None).is_some());
}

#[test]
fn parse_ml_no_false_positive_on_prefix() {
    assert!(parse_command("!mls", None).is_none());
    assert!(parse_command("!mlfoo", None).is_none());
}

#[test]
fn parse_ml_fullwidth_exclamation() {
    let cmd = parse_command("！ml 12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_li_alias_start() {
    let cmd = parse_command("!li 12345678", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_start_operation_suffix() {
    let cmd = parse_command("!ml 12345678 start", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 0,
        })
    );
}

#[test]
fn parse_ml_stop_operation_suffix_alias() {
    let cmd = parse_command("!ml 12345678 p", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Stop { match_id: 12345678 })
    );
}

#[test]
fn parse_ml_list_aliases() {
    assert_eq!(
        parse_command("!ml info", None).unwrap(),
        Command::MatchListen(MatchListenAction::List)
    );
    assert_eq!(
        parse_command("!ml l", None).unwrap(),
        Command::MatchListen(MatchListenAction::List)
    );
    assert_eq!(
        parse_command("!ml 12345678 i", None).unwrap(),
        Command::MatchListen(MatchListenAction::List)
    );
}

#[test]
fn parse_ml_start_with_skip_suffix() {
    let cmd = parse_command("!ml 12345678 #3", None).unwrap();
    assert_eq!(
        cmd,
        Command::MatchListen(MatchListenAction::Start {
            match_id: 12345678,
            skip_rounds: 3,
        })
    );
}

#[test]
fn parse_ml_invalid_skip_suffix() {
    assert!(parse_command("!ml 12345678 #0", None).is_none());
    assert!(parse_command("!ml 12345678 #101", None).is_none());
    assert!(parse_command("!ml 12345678 #abc", None).is_none());
}

#[test]
fn parse_ml_group_name() {
    assert_eq!(
        Command::MatchListen(MatchListenAction::List).group_name(),
        CommandGroup::MatchListen
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::Start {
            match_id: 1,
            skip_rounds: 0,
        })
        .group_name(),
        CommandGroup::MatchListen
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::Stop { match_id: 1 }).group_name(),
        CommandGroup::MatchListen
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::StopAll).group_name(),
        CommandGroup::MatchListen
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::Status { match_id: 1 }).group_name(),
        CommandGroup::MatchListen
    );
}

#[test]
fn parse_ml_command_name() {
    assert_eq!(
        Command::MatchListen(MatchListenAction::List).command_name(),
        "!ml"
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::Start {
            match_id: 1,
            skip_rounds: 0,
        })
        .command_name(),
        "!ml"
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::Stop { match_id: 1 }).command_name(),
        "!ml"
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::StopAll).command_name(),
        "!ml"
    );
    assert_eq!(
        Command::MatchListen(MatchListenAction::Status { match_id: 1 }).command_name(),
        "!ml"
    );
}
