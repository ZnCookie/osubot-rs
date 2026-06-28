use crate::rate_limiter::RateLimiter;

use super::http;
use super::{ApiError, OauthTokenCache};

const MAX_MATCH_LIMIT: u16 = 101;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct LegacyMatchResponse {
    #[serde(rename = "match")]
    pub match_info: LegacyMatchInfo,
    #[serde(default)]
    pub users: Vec<LegacyMatchUser>,
    #[serde(default)]
    pub first_event_id: Option<u64>,
    pub latest_event_id: u64,
    #[serde(default)]
    pub cursor_string: Option<String>,
    #[serde(default)]
    pub events: Vec<LegacyMatchEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LegacyMatchInfo {
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub start_time: Option<String>,
    #[serde(default)]
    pub end_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LegacyMatchUser {
    pub id: u64,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct LegacyMatchEvent {
    pub id: u64,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub user_id: Option<u64>,
    #[serde(default)]
    pub detail: LegacyMatchEventDetail,
    #[serde(default)]
    pub game: Option<LegacyMatchGame>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct LegacyMatchEventDetail {
    #[serde(rename = "type", default)]
    pub event_type: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct LegacyMatchGame {
    #[serde(default)]
    pub beatmap_id: Option<u64>,
    #[serde(default)]
    pub beatmap: Option<LegacyMatchBeatmap>,
    #[serde(default)]
    pub beatmapset: Option<LegacyMatchBeatmapset>,
    #[serde(default)]
    pub end_time: Option<String>,
    #[serde(default)]
    pub scores: Vec<LegacyMatchGameScore>,
    #[serde(default)]
    pub mods: Vec<LegacyMatchMod>,
    #[serde(default)]
    pub team_type: String,
    #[serde(default)]
    pub scoring_type: String,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct LegacyMatchBeatmap {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub difficulty_rating: Option<f64>,
    #[serde(default)]
    pub max_combo: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct LegacyMatchBeatmapset {
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub creator: String,
    #[serde(default)]
    pub covers: Option<serde_json::Value>,
}

impl LegacyMatchBeatmapset {
    #[must_use]
    pub fn cover_url(&self) -> Option<String> {
        let covers = self.covers.as_ref()?;
        if let Some(list_url) = covers.get("list").and_then(|v| v.as_str()) {
            return Some(list_url.replace("@2x", "").replace("list", "fullsize"));
        }
        covers
            .get("cover")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct LegacyMatchGameScore {
    pub user_id: u64,
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub total_score: Option<i64>,
    #[serde(default)]
    pub legacy_total_score: Option<i64>,
    #[serde(default)]
    pub classic_total_score: Option<i64>,
    #[serde(default)]
    pub accuracy: f64,
    #[serde(default)]
    pub max_combo: u32,
    #[serde(default)]
    pub mods: Vec<LegacyMatchMod>,
    #[serde(default)]
    pub team: String,
    #[serde(default)]
    pub rank: String,
    #[serde(rename = "pass", default)]
    pub passed: bool,
}

impl LegacyMatchGameScore {
    #[must_use]
    pub fn effective_score(&self) -> i64 {
        if self.score > 0 {
            return self.score;
        }

        self.total_score
            .filter(|&v| v > 0)
            .or(self.legacy_total_score.filter(|&v| v > 0))
            .or(self.classic_total_score.filter(|&v| v > 0))
            .unwrap_or(self.score)
    }

    #[must_use]
    pub fn display_rank(&self) -> String {
        match self.rank.trim() {
            "XH" => "X".to_string(),
            "SH" => "S".to_string(),
            "" => {
                if self.passed {
                    "PASS".to_string()
                } else {
                    "F".to_string()
                }
            }
            rank => rank.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(untagged)]
pub enum LegacyMatchMod {
    Object { acronym: String },
    String(String),
}

impl LegacyMatchMod {
    #[must_use]
    pub fn acronym(&self) -> &str {
        match self {
            Self::Object { acronym } | Self::String(acronym) => acronym,
        }
    }
}

fn normalize_match_limit(limit: Option<u16>) -> Option<u16> {
    limit.map(|value| value.min(MAX_MATCH_LIMIT))
}

fn build_match_url(match_id: u64, after: Option<u64>, limit: Option<u16>) -> String {
    let mut url = format!("https://osu.ppy.sh/api/v2/matches/{match_id}");
    let mut query = Vec::new();

    if let Some(after) = after {
        query.push(format!("after={after}"));
    }

    if let Some(limit) = normalize_match_limit(limit) {
        query.push(format!("limit={limit}"));
    }

    if !query.is_empty() {
        url.push('?');
        url.push_str(&query.join("&"));
    }

    url
}

pub async fn fetch_match(
    rate_limiter: &RateLimiter,
    oauth: &OauthTokenCache,
    match_id: u64,
    after: Option<u64>,
    limit: Option<u16>,
) -> Result<LegacyMatchResponse, ApiError> {
    let url = build_match_url(match_id, after, limit);
    let resp = http::authenticated_get(&url, rate_limiter, oauth).await?;
    http::json_body(resp).await
}

#[must_use]
pub fn reconstruct_match_roster(response: &LegacyMatchResponse) -> Vec<LegacyMatchUser> {
    let mut roster: Vec<LegacyMatchUser> = response.users.clone();

    for event in &response.events {
        match event.detail.event_type.as_str() {
            "player-joined" => {
                if let Some(user) = event_user(event, &response.users) {
                    if !roster.iter().any(|existing| existing.id == user.id) {
                        roster.push(user);
                    }
                }
            }
            "player-left" => {
                if let Some(user_id) = event.user_id {
                    roster.retain(|user| user.id != user_id);
                } else if let Some(name) = username_from_lobby_text(&event.detail.text, " left") {
                    roster.retain(|user| user.username != name);
                }
            }
            _ => {}
        }
    }

    roster
}

fn event_user(event: &LegacyMatchEvent, users: &[LegacyMatchUser]) -> Option<LegacyMatchUser> {
    if let Some(user_id) = event.user_id {
        if let Some(user) = users.iter().find(|user| user.id == user_id) {
            return Some(user.clone());
        }

        let username = username_from_lobby_text(&event.detail.text, " joined")
            .unwrap_or_else(|| format!("User {user_id}"));
        return Some(LegacyMatchUser {
            id: user_id,
            username,
            avatar_url: None,
        });
    }

    username_from_lobby_text(&event.detail.text, " joined").map(|username| LegacyMatchUser {
        id: 0,
        username,
        avatar_url: None,
    })
}

fn username_from_lobby_text(text: &str, suffix: &str) -> Option<String> {
    text.strip_suffix(suffix)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod match_api_tests {
    use super::*;

    const MATCH_FIXTURE: &str = r#"
    {
      "match": {
        "id": 424242,
        "name": "OWC Finals",
        "start_time": "2026-06-01T12:00:00Z",
        "end_time": "2026-06-01T13:00:00Z"
      },
      "users": [
        {
          "id": 11,
          "username": "Alpha"
        },
        {
          "id": 22,
          "username": "Bravo"
        }
      ],
      "first_event_id": 10,
      "latest_event_id": 12,
      "cursor_string": "cursor:12",
      "events": [
        {
          "id": 10,
          "timestamp": "2026-06-01T12:00:00Z",
          "user_id": 11,
          "detail": {
            "type": "match-created",
            "text": "Alpha created the match"
          }
        },
        {
          "id": 11,
          "timestamp": "2026-06-01T12:30:00Z",
          "detail": {
            "type": "other",
            "text": "Blue Team won by 250000"
          },
          "game": {
            "beatmap_id": 987654,
            "beatmap": {
              "id": 987654,
              "version": "Extra",
              "mode": "osu",
              "difficulty_rating": 6.25,
              "max_combo": 1200
            },
            "beatmapset": {
              "artist": "xi",
              "title": "Blue Zenith",
              "creator": "Asphyxia",
              "covers": {
                "cover": "https://assets.ppy.sh/beatmaps/987/covers/cover.jpg"
              }
            },
            "end_time": "2026-06-01T12:29:58Z",
            "scores": [
              {
                "user_id": 11,
                "score": 654321,
                "accuracy": 0.9825,
                "max_combo": 900,
                "mods": ["HD", {"acronym": "HR"}],
                "team": "blue",
                "pass": true
              },
              {
                "user_id": 22,
                "score": 432100,
                "accuracy": 0.956,
                "max_combo": 650,
                "mods": [],
                "team": "red",
                "pass": false
              }
            ]
          }
        },
        {
          "id": 12,
          "timestamp": "2026-06-01T13:00:00Z",
          "detail": {
            "type": "match-disbanded",
            "text": "Match disbanded"
          }
        }
      ]
    }
    "#;

    #[test]
    fn deserialize_legacy_match_fixture() {
        let response: LegacyMatchResponse = serde_json::from_str(MATCH_FIXTURE).unwrap();

        assert_eq!(response.match_info.id, 424242);
        assert_eq!(response.match_info.name, "OWC Finals");
        assert_eq!(response.latest_event_id, 12);
        assert_eq!(response.cursor_string.as_deref(), Some("cursor:12"));
        assert_eq!(response.users.len(), 2);
        assert_eq!(response.events.len(), 3);
        assert_eq!(response.events[0].detail.event_type, "match-created");
        assert_eq!(response.events[0].detail.text, "Alpha created the match");
        assert_eq!(response.events[2].detail.event_type, "match-disbanded");
    }

    #[test]
    fn deserialize_completed_game_event_scores() {
        let response: LegacyMatchResponse = serde_json::from_str(MATCH_FIXTURE).unwrap();
        let game = response.events[1].game.as_ref().unwrap();

        assert_eq!(response.events[1].detail.event_type, "other");
        assert_eq!(game.beatmap_id, Some(987654));
        let beatmap = game.beatmap.as_ref().unwrap();
        assert_eq!(beatmap.id, Some(987654));
        assert_eq!(beatmap.version, "Extra");
        assert_eq!(beatmap.mode, "osu");
        assert_eq!(beatmap.difficulty_rating, Some(6.25));
        assert_eq!(beatmap.max_combo, Some(1200));
        let beatmapset = game.beatmapset.as_ref().unwrap();
        assert_eq!(beatmapset.artist, "xi");
        assert_eq!(beatmapset.title, "Blue Zenith");
        assert_eq!(beatmapset.creator, "Asphyxia");
        assert_eq!(
            beatmapset.cover_url().as_deref(),
            Some("https://assets.ppy.sh/beatmaps/987/covers/cover.jpg")
        );
        assert_eq!(game.end_time.as_deref(), Some("2026-06-01T12:29:58Z"));
        assert_eq!(game.scores.len(), 2);
        assert_eq!(game.scores[0].user_id, 11);
        assert_eq!(game.scores[0].score, 654321);
        assert!((game.scores[0].accuracy - 0.9825).abs() < f64::EPSILON);
        assert_eq!(game.scores[0].max_combo, 900);
        assert_eq!(game.scores[0].mods[0].acronym(), "HD");
        assert_eq!(game.scores[0].mods[1].acronym(), "HR");
        assert_eq!(game.scores[0].team, "blue");
        assert!(game.scores[0].passed);
        assert_eq!(game.scores[1].user_id, 22);
        assert_eq!(game.scores[1].team, "red");
        assert!(!game.scores[1].passed);
    }

    #[test]
    fn display_rank_prefers_api_rank_over_pass_flag() {
        let score = LegacyMatchGameScore {
            user_id: 11,
            score: 123,
            total_score: None,
            legacy_total_score: None,
            classic_total_score: None,
            accuracy: 1.0,
            max_combo: 1,
            mods: Vec::new(),
            team: String::new(),
            rank: "A".to_string(),
            passed: false,
        };

        assert_eq!(score.display_rank(), "A");
    }

    #[test]
    fn display_rank_falls_back_to_pass_or_fail_when_rank_missing() {
        let passed = LegacyMatchGameScore {
            user_id: 11,
            score: 123,
            total_score: None,
            legacy_total_score: None,
            classic_total_score: None,
            accuracy: 1.0,
            max_combo: 1,
            mods: Vec::new(),
            team: String::new(),
            rank: String::new(),
            passed: true,
        };
        let failed = LegacyMatchGameScore {
            passed: false,
            ..passed.clone()
        };

        assert_eq!(passed.display_rank(), "PASS");
        assert_eq!(failed.display_rank(), "F");
    }

    #[test]
    fn beatmapset_cover_url_prefers_fullsize_from_list() {
        let beatmapset = LegacyMatchBeatmapset {
            artist: String::new(),
            title: String::new(),
            creator: String::new(),
            covers: Some(serde_json::json!({
                "list": "https://assets.ppy.sh/beatmaps/1/covers/list@2x.jpg",
                "cover": "https://assets.ppy.sh/beatmaps/1/covers/cover.jpg"
            })),
        };

        assert_eq!(
            beatmapset.cover_url().as_deref(),
            Some("https://assets.ppy.sh/beatmaps/1/covers/fullsize.jpg")
        );
    }

    #[test]
    fn build_match_url_without_after_for_initial_fetch() {
        assert_eq!(
            build_match_url(424242, None, None),
            "https://osu.ppy.sh/api/v2/matches/424242"
        );
    }

    #[test]
    fn build_match_url_with_after_for_incremental_fetch() {
        assert_eq!(
            build_match_url(424242, Some(12), Some(50)),
            "https://osu.ppy.sh/api/v2/matches/424242?after=12&limit=50"
        );
    }

    #[test]
    fn build_match_url_caps_limit_to_101() {
        assert_eq!(
            build_match_url(424242, Some(12), Some(500)),
            "https://osu.ppy.sh/api/v2/matches/424242?after=12&limit=101"
        );
    }

    #[test]
    fn reconstruct_match_roster_replays_join_and_left_events() {
        let response: LegacyMatchResponse = serde_json::from_str(
            r#"
            {
              "match": { "id": 1, "name": "Test" },
              "users": [
                { "id": 11, "username": "Alpha", "avatar_url": "https://example.com/a.png" },
                { "id": 22, "username": "Bravo" },
                { "id": 33, "username": "Charlie" }
              ],
              "latest_event_id": 4,
              "events": [
                { "id": 1, "user_id": 11, "detail": { "type": "player-joined", "text": "Alpha joined" } },
                { "id": 2, "user_id": 22, "detail": { "type": "player-joined", "text": "Bravo joined" } },
                { "id": 3, "user_id": 11, "detail": { "type": "player-left", "text": "Alpha left" } },
                { "id": 4, "user_id": 33, "detail": { "type": "player-joined", "text": "Charlie joined" } }
              ]
            }
            "#,
        )
        .unwrap();

        let roster = reconstruct_match_roster(&response);

        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].username, "Bravo");
        assert_eq!(roster[1].username, "Charlie");
    }

    #[test]
    fn reconstruct_match_roster_uses_lobby_text_when_users_mapping_missing() {
        let response: LegacyMatchResponse = serde_json::from_str(
            r#"
            {
              "match": { "id": 1, "name": "Test" },
              "users": [],
              "latest_event_id": 2,
              "events": [
                { "id": 1, "user_id": 11, "detail": { "type": "player-joined", "text": "Alpha joined" } },
                { "id": 2, "detail": { "type": "player-joined", "text": "Bravo joined" } }
              ]
            }
            "#,
        )
        .unwrap();

        let roster = reconstruct_match_roster(&response);

        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].id, 11);
        assert_eq!(roster[0].username, "Alpha");
        assert_eq!(roster[1].id, 0);
        assert_eq!(roster[1].username, "Bravo");
    }

    #[test]
    fn reconstruct_match_roster_seeds_from_response_users() {
        let response: LegacyMatchResponse = serde_json::from_str(
            r#"
            {
              "match": { "id": 1, "name": "Test" },
              "users": [
                { "id": 11, "username": "Alpha" },
                { "id": 22, "username": "Bravo" }
              ],
              "latest_event_id": 1,
              "events": [
                { "id": 1, "detail": { "type": "match-started", "text": "Match started" } }
              ]
            }
            "#,
        )
        .unwrap();

        let roster = reconstruct_match_roster(&response);

        assert_eq!(roster.len(), 2);
        assert_eq!(roster[0].username, "Alpha");
        assert_eq!(roster[1].username, "Bravo");
    }

    #[test]
    fn reconstruct_match_roster_seeds_then_applies_left_events() {
        let response = LegacyMatchResponse {
            match_info: LegacyMatchInfo {
                id: 1,
                name: "Test".to_string(),
                start_time: None,
                end_time: None,
            },
            users: vec![
                LegacyMatchUser {
                    id: 11,
                    username: "Alpha".to_string(),
                    avatar_url: None,
                },
                LegacyMatchUser {
                    id: 22,
                    username: "Bravo".to_string(),
                    avatar_url: None,
                },
            ],
            first_event_id: Some(1),
            latest_event_id: 3,
            cursor_string: None,
            events: vec![LegacyMatchEvent {
                id: 3,
                timestamp: String::new(),
                user_id: Some(22),
                detail: LegacyMatchEventDetail {
                    event_type: "player-left".to_string(),
                    text: "Bravo left the game".to_string(),
                },
                game: None,
            }],
        };

        let roster = reconstruct_match_roster(&response);

        assert_eq!(roster.len(), 1);
        assert_eq!(roster[0].username, "Alpha");
    }
}
