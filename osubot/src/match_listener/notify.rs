//! Match listener notification formatting and delivery.
//!
//! Text notifications cover lobby events (player joined/left, host changed,
//! match created, match disbanded fallback).
//! Image notifications cover completed games and match-end summary via
//! `render_match_result_card()`. On render error/timeout, falls back to
//! concise Chinese text.

use osubot_core::api::{BeatmapMetadata, LegacyMatchGame, LegacyMatchGameScore, LegacyMatchUser};
use osubot_render::{MatchResultParams, MatchTeamResultParams};

pub(crate) struct MatchResultBuildOutput {
    pub(crate) params: MatchResultParams,
    pub(crate) cover_url: Option<String>,
}

impl MatchResultBuildOutput {
    pub(crate) fn needs_beatmap_metadata(&self) -> bool {
        self.params.beatmap_title.trim().is_empty()
            || self.cover_url.is_none()
            || (self.params.is_started
                && (self.params.beatmap_bpm.is_none()
                    || self.params.beatmap_length_seconds.is_none()
                    || self.params.beatmap_max_combo.is_none()
                    || self.params.beatmap_ar.is_none()
                    || self.params.beatmap_od.is_none()
                    || self.params.beatmap_cs.is_none()
                    || self.params.beatmap_hp.is_none()))
    }

    pub(crate) fn apply_beatmap_metadata(&mut self, metadata: BeatmapMetadata) {
        if self.params.beatmap_artist.trim().is_empty() {
            self.params.beatmap_artist = metadata.artist;
        }
        if self.params.beatmap_title.trim().is_empty() {
            self.params.beatmap_title = metadata.title;
        }
        if self.params.beatmap_version.trim().is_empty() {
            self.params.beatmap_version = metadata.version;
        }
        if self.params.beatmap_mapper.trim().is_empty() {
            self.params.beatmap_mapper = metadata.creator;
        }
        if self.params.beatmap_mode.trim().is_empty() || self.params.beatmap_mode == "osu" {
            self.params.beatmap_mode = metadata.mode;
        }
        if self.params.star_rating.is_none() {
            self.params.star_rating = metadata.difficulty_rating;
        }
        if self.params.beatmap_bpm.is_none() {
            self.params.beatmap_bpm = metadata.bpm;
        }
        if self.params.beatmap_length_seconds.is_none() {
            self.params.beatmap_length_seconds = metadata.total_length;
        }
        if self.params.beatmap_max_combo.is_none() {
            self.params.beatmap_max_combo = metadata.max_combo;
        }
        if self.params.beatmap_ar.is_none() {
            self.params.beatmap_ar = metadata.ar;
        }
        if self.params.beatmap_od.is_none() {
            self.params.beatmap_od = metadata.od;
        }
        if self.params.beatmap_cs.is_none() {
            self.params.beatmap_cs = metadata.cs;
        }
        if self.params.beatmap_hp.is_none() {
            self.params.beatmap_hp = metadata.hp;
        }
        if self.cover_url.is_none() {
            self.cover_url = metadata.cover_url;
        }
    }
}

/// Format a lobby text notification (Chinese).
pub(crate) fn format_lobby_text(
    event_type: &str,
    raw_text: &str,
    users: &[LegacyMatchUser],
    match_name: &str,
) -> String {
    // For lobby events, use the API-provided text as the base.
    // Map user_id references to usernames when available.
    let text = map_user_references(raw_text, users);
    format!("【{}】{}\n{}", match_name, event_label(event_type), text)
}

/// Format a fallback text notification when image rendering fails.
pub(crate) fn format_game_fallback_text(
    game: &LegacyMatchGame,
    users: &[LegacyMatchUser],
    match_name: &str,
) -> String {
    let mut lines = vec![format!("【{}】场次结束", match_name)];

    // Sort scores descending by score value
    let mut sorted_scores: Vec<&LegacyMatchGameScore> = game.scores.iter().collect();
    sorted_scores.sort_by_key(|score| std::cmp::Reverse(score.effective_score()));

    for (idx, score) in sorted_scores.iter().enumerate() {
        let username = users
            .iter()
            .find(|u| u.id == score.user_id)
            .map(|u| u.username.as_str())
            .unwrap_or("未知玩家");
        let acc_pct = score.accuracy * 100.0;
        let rank_mark = score.display_rank();
        lines.push(format!(
            "#{} {} — {} 分 | {:.2}% | {} [{}]",
            idx + 1,
            username,
            score.effective_score(),
            acc_pct,
            score.team,
            rank_mark
        ));
    }

    lines.join("\n")
}

pub(crate) fn format_game_start_fallback_text(
    game: &LegacyMatchGame,
    users: &[LegacyMatchUser],
    match_name: &str,
) -> String {
    let beatmap_id = game
        .beatmap_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "未知".to_string());
    let mut lines = vec![
        format!("【{}】场次开始", match_name),
        format!("选图：Beatmap #{}", beatmap_id),
        "参加玩家：".to_string(),
    ];

    if users.is_empty() {
        lines.push("- 暂无玩家信息".to_string());
    } else {
        lines.extend(users.iter().map(|user| format!("- {}", user.username)));
    }

    lines.join("\n")
}

/// Build `MatchResultParams` for image rendering from game data.
///
/// Returns `None` if essential data is missing (no scores or no beatmap).
pub(crate) fn build_match_result_params(
    match_id: u64,
    match_name: &str,
    event_label: &str,
    played_at: &str,
    game: &LegacyMatchGame,
    users: &[LegacyMatchUser],
) -> Option<MatchResultBuildOutput> {
    let beatmap_id = game.beatmap_id?;
    let selected_mods: Vec<String> = game
        .mods
        .iter()
        .map(|modifier| modifier.acronym().to_string())
        .collect();
    let team_type = (!game.team_type.trim().is_empty()).then(|| game.team_type.clone());
    let scoring_type = (!game.scoring_type.trim().is_empty()).then(|| game.scoring_type.clone());

    let players = if game.scores.is_empty() && game.end_time.is_none() {
        users
            .iter()
            .enumerate()
            .map(|(idx, user)| osubot_render::MatchResultPlayerParams {
                placement: idx + 1,
                username: user.username.clone(),
                avatar_url: avatar_url_for_user(user),
                avatar_image: None,
                team: None,
                score: 0,
                accuracy: 0.0,
                max_combo: 0,
                mods: Vec::new(),
                rank: String::new(),
                passed: true,
            })
            .collect()
    } else {
        if game.scores.is_empty() {
            return None;
        }
        let mut sorted_scores: Vec<&LegacyMatchGameScore> = game.scores.iter().collect();
        sorted_scores.sort_by_key(|score| std::cmp::Reverse(score.effective_score()));

        sorted_scores
            .into_iter()
            .enumerate()
            .map(|(idx, score)| {
                let username = users
                    .iter()
                    .find(|u| u.id == score.user_id)
                    .map(|u| u.username.clone())
                    .unwrap_or_else(|| "未知玩家".to_string());
                let avatar_url = users
                    .iter()
                    .find(|u| u.id == score.user_id)
                    .and_then(avatar_url_for_user);
                osubot_render::MatchResultPlayerParams {
                    placement: idx + 1,
                    username,
                    avatar_url,
                    avatar_image: None,
                    team: if score.team.is_empty() {
                        None
                    } else {
                        Some(score.team.clone())
                    },
                    score: score.effective_score().max(0) as u64,
                    accuracy: score.accuracy,
                    max_combo: score.max_combo,
                    mods: score
                        .mods
                        .iter()
                        .map(|modifier| modifier.acronym().to_string())
                        .collect(),
                    rank: score.display_rank(),
                    passed: score.passed,
                }
            })
            .collect()
    };

    let team_results = build_team_results(game);
    let beatmap = game.beatmap.as_ref();
    let beatmapset = game.beatmapset.as_ref();

    let cover_url = beatmapset.and_then(|b| b.cover_url());

    Some(MatchResultBuildOutput {
        params: MatchResultParams {
            match_id,
            match_name: match_name.to_string(),
            event_label: event_label.to_string(),
            played_at: played_at.to_string(),
            beatmap_id,
            beatmap_artist: beatmapset.map(|b| b.artist.clone()).unwrap_or_default(),
            beatmap_title: beatmapset.map(|b| b.title.clone()).unwrap_or_default(),
            beatmap_version: beatmap.map(|b| b.version.clone()).unwrap_or_default(),
            beatmap_mapper: beatmapset.map(|b| b.creator.clone()).unwrap_or_default(),
            beatmap_mode: beatmap
                .and_then(|b| (!b.mode.is_empty()).then(|| b.mode.clone()))
                .unwrap_or_else(|| "osu".to_string()),
            star_rating: beatmap.and_then(|b| b.difficulty_rating),
            beatmap_bpm: None,
            beatmap_length_seconds: None,
            beatmap_max_combo: beatmap.and_then(|b| b.max_combo),
            beatmap_ar: None,
            beatmap_od: None,
            beatmap_cs: None,
            beatmap_hp: None,
            cover_image: None,
            is_started: game.end_time.is_none(),
            selected_mods,
            team_type,
            scoring_type,
            team_results,
            players,
        },
        cover_url,
    })
}

fn build_team_results(game: &LegacyMatchGame) -> Vec<MatchTeamResultParams> {
    if game.end_time.is_none() || game.scores.is_empty() {
        return Vec::new();
    }

    let mut totals = std::collections::BTreeMap::<String, u64>::new();
    for score in &game.scores {
        let team = score.team.trim();
        if team.is_empty() {
            continue;
        }
        *totals.entry(team.to_string()).or_default() += score.effective_score().max(0) as u64;
    }

    if totals.len() < 2 {
        return Vec::new();
    }

    let max_score = totals.values().copied().max().unwrap_or(0);
    let mut results: Vec<MatchTeamResultParams> = totals
        .into_iter()
        .map(|(team, score)| MatchTeamResultParams {
            team,
            score,
            is_winner: score == max_score,
        })
        .collect();
    results.sort_by_key(|team| std::cmp::Reverse(team.score));
    results
}

/// Map user ID references in API text to usernames.
fn map_user_references(text: &str, users: &[LegacyMatchUser]) -> String {
    let result = text.to_string();
    for user in users {
        // osu! API text sometimes references users by ID; we don't have a
        // reliable pattern so we pass through. v1 keeps raw API text.
        let _ = user;
    }
    result
}

fn avatar_url_for_user(user: &LegacyMatchUser) -> Option<String> {
    user.avatar_url
        .as_ref()
        .filter(|url| !url.trim().is_empty())
        .cloned()
        .or_else(|| (user.id > 0).then(|| format!("https://a.ppy.sh/{}", user.id)))
}

/// Human-readable Chinese label for an event type.
fn event_label(event_type: &str) -> &'static str {
    match event_type {
        "match-created" => "比赛创建",
        "match-disbanded" => "比赛解散",
        "player-joined" => "玩家加入",
        "player-left" => "玩家离开",
        "player-kicked" => "玩家被踢出",
        "host-changed" => "房主变更",
        "beatmap-changed" => "选图变更",
        "match-started" => "比赛开始",
        "other" => "场次结束",
        _ => "事件",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_core::api::{LegacyMatchBeatmap, LegacyMatchBeatmapset, LegacyMatchMod};

    fn make_user(id: u64, name: &str) -> LegacyMatchUser {
        LegacyMatchUser {
            id,
            username: name.to_string(),
            avatar_url: None,
        }
    }

    fn make_user_with_avatar(id: u64, name: &str, avatar_url: &str) -> LegacyMatchUser {
        LegacyMatchUser {
            id,
            username: name.to_string(),
            avatar_url: Some(avatar_url.to_string()),
        }
    }

    fn make_beatmap() -> LegacyMatchBeatmap {
        LegacyMatchBeatmap {
            id: Some(987),
            version: "Extra".to_string(),
            mode: "osu".to_string(),
            difficulty_rating: Some(6.25),
            max_combo: Some(1200),
        }
    }

    fn make_beatmapset() -> LegacyMatchBeatmapset {
        LegacyMatchBeatmapset {
            artist: "xi".to_string(),
            title: "Blue Zenith".to_string(),
            creator: "Asphyxia".to_string(),
            covers: Some(serde_json::json!({
                "list": "https://assets.ppy.sh/beatmaps/987/covers/list@2x.jpg"
            })),
        }
    }

    #[test]
    fn formats_player_joined_as_text() {
        let users = vec![make_user(11, "Alpha")];
        let text = format_lobby_text("player-joined", "Alpha joined", &users, "OWC Finals");
        assert!(text.contains("OWC Finals"));
        assert!(text.contains("玩家加入"));
        assert!(text.contains("Alpha joined"));
    }

    #[test]
    fn formats_match_disbanded_as_text() {
        let text = format_lobby_text("match-disbanded", "Match disbanded", &[], "Test");
        assert!(text.contains("比赛解散"));
        assert!(text.contains("Match disbanded"));
    }

    #[test]
    fn formats_map_change_and_match_start_labels() {
        let map = format_lobby_text("beatmap-changed", "Beatmap changed", &[], "Test");
        let start = format_lobby_text("match-started", "Match started", &[], "Test");

        assert!(map.contains("选图变更"));
        assert!(map.contains("Beatmap changed"));
        assert!(start.contains("比赛开始"));
        assert!(start.contains("Match started"));
    }

    #[test]
    fn formats_game_fallback_with_scores() {
        let users = vec![make_user(11, "Alpha"), make_user(22, "Bravo")];
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: None,
            beatmapset: None,
            end_time: Some("2026-06-01T12:00:00Z".to_string()),
            mods: Vec::new(),
            team_type: String::new(),
            scoring_type: String::new(),
            scores: vec![
                LegacyMatchGameScore {
                    user_id: 11,
                    score: 500000,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.98,
                    max_combo: 0,
                    mods: Vec::new(),
                    team: "blue".to_string(),
                    rank: "A".to_string(),
                    passed: true,
                },
                LegacyMatchGameScore {
                    user_id: 22,
                    score: 300000,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.92,
                    max_combo: 0,
                    mods: Vec::new(),
                    team: "red".to_string(),
                    rank: "F".to_string(),
                    passed: false,
                },
            ],
        };
        let text = format_game_fallback_text(&game, &users, "OWC");
        assert!(text.contains("OWC"));
        assert!(text.contains("Alpha"));
        assert!(text.contains("500000"));
        assert!(text.contains("A"));
        assert!(text.contains("Bravo"));
        assert!(text.contains("F"));
    }

    #[test]
    fn formats_game_start_fallback_with_participants() {
        let users = vec![make_user(11, "Alpha"), make_user(22, "Bravo")];
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: None,
            beatmapset: None,
            end_time: None,
            mods: Vec::new(),
            team_type: String::new(),
            scoring_type: String::new(),
            scores: Vec::new(),
        };

        let text = format_game_start_fallback_text(&game, &users, "OWC");

        assert!(text.contains("场次开始"));
        assert!(text.contains("选图：Beatmap #987"));
        assert!(text.contains("参加玩家"));
        assert!(text.contains("Alpha"));
        assert!(text.contains("Bravo"));
        assert!(!text.contains("场次结束"));
    }

    #[test]
    fn builds_params_for_completed_game() {
        let users = vec![make_user(11, "Alpha")];
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: Some(make_beatmap()),
            beatmapset: Some(make_beatmapset()),
            end_time: Some("2026-06-01T12:00:00Z".to_string()),
            mods: vec![LegacyMatchMod::String("DT".to_string())],
            team_type: "team-vs".to_string(),
            scoring_type: "score".to_string(),
            scores: vec![LegacyMatchGameScore {
                user_id: 11,
                score: 500000,
                total_score: None,
                legacy_total_score: None,
                classic_total_score: None,
                accuracy: 0.98,
                max_combo: 900,
                mods: vec![
                    LegacyMatchMod::String("HD".to_string()),
                    LegacyMatchMod::Object {
                        acronym: "HR".to_string(),
                    },
                ],
                team: "blue".to_string(),
                rank: "A".to_string(),
                passed: true,
            }],
        };
        let output = build_match_result_params(42, "Test", "场次结束", "12:00", &game, &users);
        assert!(output.is_some());
        let output = output.unwrap();
        assert_eq!(
            output.cover_url.as_deref(),
            Some("https://assets.ppy.sh/beatmaps/987/covers/fullsize.jpg")
        );
        let p = output.params;
        assert_eq!(p.match_id, 42);
        assert_eq!(p.beatmap_artist, "xi");
        assert_eq!(p.beatmap_title, "Blue Zenith");
        assert_eq!(p.beatmap_version, "Extra");
        assert_eq!(p.beatmap_mapper, "Asphyxia");
        assert_eq!(p.beatmap_mode, "osu");
        assert_eq!(p.star_rating, Some(6.25));
        assert_eq!(p.players.len(), 1);
        assert_eq!(p.players[0].username, "Alpha");
        assert_eq!(p.players[0].max_combo, 900);
        assert_eq!(p.players[0].mods, vec!["HD", "HR"]);
        assert_eq!(p.selected_mods, vec!["DT"]);
        assert_eq!(p.team_type.as_deref(), Some("team-vs"));
        assert_eq!(p.scoring_type.as_deref(), Some("score"));
    }

    #[test]
    fn builds_params_for_started_game_label() {
        let users = vec![make_user(11, "Alpha")];
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: Some(make_beatmap()),
            beatmapset: Some(make_beatmapset()),
            end_time: None,
            mods: vec![LegacyMatchMod::String("NF".to_string())],
            team_type: "team-vs".to_string(),
            scoring_type: "score".to_string(),
            scores: vec![LegacyMatchGameScore {
                user_id: 11,
                score: 0,
                total_score: None,
                legacy_total_score: None,
                classic_total_score: None,
                accuracy: 0.0,
                max_combo: 0,
                mods: Vec::new(),
                team: "blue".to_string(),
                rank: String::new(),
                passed: false,
            }],
        };

        let output = build_match_result_params(
            42,
            "Test",
            "场次开始",
            "2026-06-01T12:30:00Z",
            &game,
            &users,
        )
        .expect("in-progress game should render start card");
        let params = output.params;

        assert_eq!(params.event_label, "场次开始");
        assert_eq!(params.played_at, "2026-06-01T12:30:00Z");
        assert_eq!(params.beatmap_title, "Blue Zenith");
        assert_eq!(params.players[0].username, "Alpha");
        assert_eq!(params.selected_mods, vec!["NF"]);
    }

    #[test]
    fn builds_params_for_started_game_without_scores_from_match_users() {
        let users = vec![
            make_user_with_avatar(11, "Alpha", "https://example.com/alpha.png"),
            make_user(22, "Bravo"),
        ];
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: Some(make_beatmap()),
            beatmapset: Some(make_beatmapset()),
            end_time: None,
            mods: vec![
                LegacyMatchMod::String("DT".to_string()),
                LegacyMatchMod::String("NF".to_string()),
            ],
            team_type: "team-vs".to_string(),
            scoring_type: "score".to_string(),
            scores: Vec::new(),
        };

        let output = build_match_result_params(
            42,
            "Test",
            "场次开始",
            "2026-06-01T12:30:00Z",
            &game,
            &users,
        )
        .expect("start card should render from match users even before scores exist");
        let params = output.params;

        assert_eq!(params.event_label, "场次开始");
        assert!(params.is_started);
        assert_eq!(params.beatmap_title, "Blue Zenith");
        assert_eq!(params.players.len(), 2);
        assert_eq!(params.players[0].username, "Alpha");
        assert_eq!(
            params.players[0].avatar_url.as_deref(),
            Some("https://example.com/alpha.png")
        );
        assert_eq!(params.players[0].score, 0);
        assert!(params.players[0].passed);
        assert_eq!(params.players[1].username, "Bravo");
        assert_eq!(params.selected_mods, vec!["DT", "NF"]);
        assert_eq!(params.team_type.as_deref(), Some("team-vs"));
        assert_eq!(params.scoring_type.as_deref(), Some("score"));
        assert_eq!(
            params.players[1].avatar_url.as_deref(),
            Some("https://a.ppy.sh/22")
        );
    }

    #[test]
    fn build_params_sorts_players_by_score_descending() {
        let users = vec![make_user(1, "Winner"), make_user(2, "RunnerUp")];
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: Some(make_beatmap()),
            beatmapset: Some(make_beatmapset()),
            end_time: Some("2026-06-01T12:00:00Z".to_string()),
            mods: Vec::new(),
            team_type: "team-vs".to_string(),
            scoring_type: "score".to_string(),
            scores: vec![
                LegacyMatchGameScore {
                    user_id: 2,
                    score: 100,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.5,
                    max_combo: 10,
                    mods: Vec::new(),
                    team: "blue".to_string(),
                    rank: "F".to_string(),
                    passed: false,
                },
                LegacyMatchGameScore {
                    user_id: 1,
                    score: 999,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.9,
                    max_combo: 99,
                    mods: Vec::new(),
                    team: "red".to_string(),
                    rank: "A".to_string(),
                    passed: true,
                },
            ],
        };

        let output = build_match_result_params(42, "Test", "场次结束", "12:00", &game, &users)
            .expect("params");

        assert_eq!(output.params.players[0].placement, 1);
        assert_eq!(output.params.players[0].username, "Winner");
        assert_eq!(output.params.players[1].placement, 2);
        assert_eq!(output.params.players[1].username, "RunnerUp");
        assert_eq!(output.params.team_results.len(), 2);
        assert!(output.params.team_results[0].is_winner);
        assert_eq!(output.params.team_results[0].team, "red");
    }

    #[test]
    fn build_output_applies_fallback_beatmap_metadata() {
        let users = vec![make_user(11, "Alpha")];
        let game = LegacyMatchGame {
            beatmap_id: Some(796622),
            beatmap: None,
            beatmapset: None,
            end_time: Some("2026-06-01T12:00:00Z".to_string()),
            mods: Vec::new(),
            team_type: String::new(),
            scoring_type: String::new(),
            scores: vec![LegacyMatchGameScore {
                user_id: 11,
                score: 123,
                total_score: None,
                legacy_total_score: None,
                classic_total_score: None,
                accuracy: 0.98,
                max_combo: 456,
                mods: Vec::new(),
                team: String::new(),
                rank: "A".to_string(),
                passed: true,
            }],
        };

        let mut output = build_match_result_params(42, "Test", "场次结束", "12:00", &game, &users)
            .expect("params");
        assert!(output.needs_beatmap_metadata());

        output.apply_beatmap_metadata(BeatmapMetadata {
            beatmap_id: 796622,
            beatmapset_id: 1234,
            artist: "Rhapsody".to_string(),
            title: "Power of the Dragonflame".to_string(),
            version: "Normal".to_string(),
            creator: "nebuyuwa".to_string(),
            mode: "osu".to_string(),
            difficulty_rating: Some(2.55),
            cover_url: Some("https://assets.ppy.sh/beatmaps/1234/covers/fullsize.jpg".to_string()),
            bpm: Some(190.0),
            total_length: Some(260),
            max_combo: Some(1337),
            ar: Some(9.7),
            od: Some(9.8),
            cs: Some(4.0),
            hp: Some(6.0),
        });

        assert_eq!(output.params.beatmap_artist, "Rhapsody");
        assert_eq!(output.params.beatmap_title, "Power of the Dragonflame");
        assert_eq!(output.params.beatmap_version, "Normal");
        assert_eq!(output.params.beatmap_mapper, "nebuyuwa");
        assert_eq!(output.params.star_rating, Some(2.55));
        assert_eq!(output.params.beatmap_bpm, Some(190.0));
        assert_eq!(output.params.beatmap_length_seconds, Some(260));
        assert_eq!(output.params.beatmap_max_combo, Some(1337));
        assert_eq!(output.params.beatmap_ar, Some(9.7));
        assert_eq!(output.params.beatmap_od, Some(9.8));
        assert_eq!(output.params.beatmap_cs, Some(4.0));
        assert_eq!(output.params.beatmap_hp, Some(6.0));
        assert_eq!(
            output.cover_url.as_deref(),
            Some("https://assets.ppy.sh/beatmaps/1234/covers/fullsize.jpg")
        );
        assert!(!output.needs_beatmap_metadata());
    }

    #[test]
    fn returns_none_for_empty_scores() {
        let game = LegacyMatchGame {
            beatmap_id: Some(987),
            beatmap: None,
            beatmapset: None,
            end_time: Some("2026-06-01T12:00:00Z".to_string()),
            mods: Vec::new(),
            team_type: String::new(),
            scoring_type: String::new(),
            scores: vec![],
        };
        assert!(build_match_result_params(42, "Test", "场次结束", "12:00", &game, &[]).is_none());
    }

    #[test]
    fn returns_none_for_missing_beatmap() {
        let game = LegacyMatchGame {
            beatmap_id: None,
            beatmap: None,
            beatmapset: None,
            end_time: Some("2026-06-01T12:00:00Z".to_string()),
            mods: Vec::new(),
            team_type: String::new(),
            scoring_type: String::new(),
            scores: vec![LegacyMatchGameScore {
                user_id: 11,
                score: 500000,
                total_score: None,
                legacy_total_score: None,
                classic_total_score: None,
                accuracy: 0.98,
                max_combo: 0,
                mods: Vec::new(),
                team: "blue".to_string(),
                rank: "A".to_string(),
                passed: true,
            }],
        };
        assert!(build_match_result_params(42, "Test", "场次结束", "12:00", &game, &[]).is_none());
    }

    #[test]
    fn fallback_text_sorts_by_score_descending() {
        let users = vec![make_user(1, "A"), make_user(2, "B")];
        let game = LegacyMatchGame {
            beatmap_id: Some(1),
            beatmap: None,
            beatmapset: None,
            end_time: Some("t".to_string()),
            mods: Vec::new(),
            team_type: "team-vs".to_string(),
            scoring_type: "score".to_string(),
            scores: vec![
                LegacyMatchGameScore {
                    user_id: 2,
                    score: 100,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.5,
                    max_combo: 0,
                    mods: Vec::new(),
                    team: "red".to_string(),
                    rank: "F".to_string(),
                    passed: false,
                },
                LegacyMatchGameScore {
                    user_id: 1,
                    score: 999,
                    total_score: None,
                    legacy_total_score: None,
                    classic_total_score: None,
                    accuracy: 0.9,
                    max_combo: 0,
                    mods: Vec::new(),
                    team: "blue".to_string(),
                    rank: "A".to_string(),
                    passed: true,
                },
            ],
        };
        let text = format_game_fallback_text(&game, &users, "M");
        let a_pos = text.find("A").unwrap();
        let b_pos = text.find("B").unwrap();
        assert!(a_pos < b_pos, "higher score should be listed first");
    }
}
