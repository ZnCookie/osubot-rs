use crate::log_fmt;
use crate::types::{GameMode, Score, ScoreStatistics, ScoreUser};

use osubot_types::to_rosu_game_mode;

use super::stable_grade;
use super::{fullsize_cover_url, OsuApiMod, OsuApiScore};

fn api_mods_to_game_mods(api_mods: &[OsuApiMod], mode: GameMode) -> rosu_mods::GameMods {
    use rosu_mods::serde::GameModSeed;
    use serde::de::DeserializeSeed;
    let ros_mode = to_rosu_game_mode(mode);
    let seed = GameModSeed::Mode {
        mode: ros_mode,
        deny_unknown_fields: false,
    };
    let mut mods = rosu_mods::GameMods::new();
    for m in api_mods {
        let gamemod = match m {
            OsuApiMod::String(s) => {
                let json_str = format!("\"{s}\"");
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|e| {
                        tracing::warn!(mod_str = %s, error = %e, "{}", log_fmt!("api.mod_deserialize_failed"));
                        rosu_mods::GameMod::new(s, ros_mode)
                    })
            }
            OsuApiMod::Object {
                acronym,
                settings: Some(settings),
            } => {
                let json = serde_json::json!({"acronym": acronym, "settings": settings});
                let json_str = json.to_string();
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|e| {
                        tracing::warn!(acronym = %acronym, error = %e, "{}", log_fmt!("api.mod_settings_deserialize_failed"));
                        rosu_mods::GameMod::new(acronym, ros_mode)
                    })
            }
            OsuApiMod::Object {
                acronym,
                settings: None,
            } => {
                let json_str = serde_json::json!({"acronym": acronym}).to_string();
                let mut de = serde_json::Deserializer::from_str(&json_str);
                seed.deserialize(&mut de)
                    .unwrap_or_else(|e| {
                        tracing::warn!(acronym = %acronym, error = %e, "{}", log_fmt!("api.mod_deserialize_failed"));
                        rosu_mods::GameMod::new(acronym, ros_mode)
                    })
            }
        };
        mods.insert(gamemod);
    }
    mods
}

pub(crate) fn api_score_to_score(api: OsuApiScore, mode: GameMode) -> Score {
    let bmap = api.beatmap.as_ref();
    let is_lazer = api
        .is_lazer
        .unwrap_or_else(|| api.build_id.is_some_and(|id| id > 0));
    let has_hidden = api.mods.iter().any(|m| {
        let acronym = match m {
            OsuApiMod::String(s) => s.as_str(),
            OsuApiMod::Object { acronym, .. } => acronym.as_str(),
        };
        acronym == "HD" || acronym == "FL" || acronym == "PF"
    });

    let cover_url = api
        .beatmapset
        .as_ref()
        .and_then(|bs| fullsize_cover_url(bs.covers.as_ref()))
        .unwrap_or_default();
    let artist = api
        .beatmapset
        .as_ref()
        .map(|bs| bs.artist.clone())
        .unwrap_or_default();
    let title = api
        .beatmapset
        .as_ref()
        .map(|bs| bs.title.clone())
        .unwrap_or_default();
    let creator = api
        .beatmapset
        .as_ref()
        .map(|bs| bs.creator.clone())
        .unwrap_or_default();
    let fav_count = api
        .beatmapset
        .as_ref()
        .and_then(|bs| Some(bs.favourite_count).filter(|&v| v > 0));
    let play_count = api
        .beatmapset
        .as_ref()
        .and_then(|bs| Some(bs.play_count).filter(|&v| v > 0));

    let score_value = if api.score > 0 {
        api.score
    } else if !is_lazer {
        // filter 丢弃 legacy_total_score=Some(0)，回退到 total_score。
        // Option::or 遇 Some(0) 直接返回 0，会丢正确分数。
        api.legacy_total_score
            .filter(|&v| v > 0)
            .or(api.total_score)
            .unwrap_or(0)
    } else {
        api.total_score
            .filter(|&v| v > 0)
            .or(api.legacy_total_score)
            .unwrap_or(0)
    };

    let user = api
        .user
        .and_then(|v| {
            let u: super::OsuApiScoreUser = match serde_json::from_value(v.clone()) {
                Ok(u) => u,
                Err(e) => {
                    tracing::warn!(error = %e, user_json = %v, "{}", log_fmt!("api.parse_user_from_score_failed"));
                    return None;
                }
            };
            Some(ScoreUser {
                avatar_url: u.avatar_url.unwrap_or_default(),
                country_code: u.country_code.unwrap_or_default(),
                user_id: u.id,
                username: u.username,
                global_rank: u.statistics.as_ref().and_then(|s| s.global_rank),
                country_rank: u.statistics.as_ref().and_then(|s| s.country_rank),
                pp: u.statistics.as_ref().and_then(|s| s.pp).unwrap_or(0.0),
            })
        })
        .unwrap_or(ScoreUser {
            avatar_url: String::new(),
            country_code: String::new(),
            user_id: None,
            username: None,
            global_rank: None,
            country_rank: None,
            pp: 0.0,
        });

    Score {
        score_id: api.id,
        beatmap_id: bmap.map_or(api.beatmap_id, |b| b.id),
        beatmapset_id: bmap.map_or(api.beatmapset_id, |b| b.beatmapset_id),
        artist,
        title,
        version: bmap.map_or(String::new(), |b| b.version.clone()),
        creator,
        star_rating: bmap.map_or(0.0, |b| b.difficulty_rating),
        bpm: bmap.map_or(0.0, |b| b.bpm),
        ar: bmap.map_or(0.0, |b| b.ar),
        od: bmap.map_or(0.0, |b| b.od),
        cs: bmap.map_or(0.0, |b| b.cs),
        hp: bmap.map_or(0.0, |b| b.hp),
        length_seconds: bmap.map_or(0, |b| b.total_length),
        score_value,
        accuracy: if is_lazer {
            api.accuracy
        } else {
            let stable_acc = stable_grade::get_stable_accuracy(&api.statistics, mode, api.passed);
            if stable_acc > 0.0 {
                stable_acc
            } else {
                api.accuracy
            }
        },
        max_combo: api.max_combo,
        beatmap_max_combo: bmap.map_or(0, |b| b.max_combo),
        pp: api.pp,
        pp_breakdown: None,
        pp_if_acc: None,
        perfect_pp: None,
        rank: if is_lazer {
            if api.passed {
                api.rank
            } else {
                "F".to_string()
            }
        } else {
            stable_grade::get_stable_rank(&api.statistics, mode, api.passed, has_hidden)
        },
        passed: api.passed,
        mods: if is_lazer {
            api_mods_to_game_mods(&api.mods, mode)
        } else {
            let filtered_mods: Vec<OsuApiMod> = api
                .mods
                .into_iter()
                .filter(|m| {
                    let acr = match m {
                        OsuApiMod::String(s) => s.as_str(),
                        OsuApiMod::Object { acronym, .. } => acronym.as_str(),
                    };
                    acr != "CL"
                })
                .collect();
            api_mods_to_game_mods(&filtered_mods, mode)
        },
        is_perfect: api.perfect,
        created_at: api.ended_at,
        is_lazer,
        has_replay: api.has_replay,
        legacy_score_id: api.legacy_score_id,
        statistics: ScoreStatistics {
            count_geki: api.statistics.count_geki,
            count_300: api.statistics.count_300,
            count_katu: if mode == GameMode::Mania {
                if api.statistics.count_katu != 0 {
                    api.statistics.count_katu
                } else {
                    api.statistics.ok
                }
            } else {
                api.statistics.count_katu
            },
            count_100: if mode == GameMode::Catch {
                if api.statistics.osu_large_tick_hits != 0 {
                    api.statistics.osu_large_tick_hits
                } else {
                    api.statistics.count_100
                }
            } else if mode == GameMode::Mania {
                if api.statistics.ok != 0 {
                    api.statistics.ok
                } else {
                    api.statistics.count_100
                }
            } else if api.statistics.ok != 0 {
                api.statistics.ok
            } else {
                api.statistics.count_100
            },
            count_50: if mode == GameMode::Catch {
                if api.statistics.osu_small_tick_hits != 0 {
                    api.statistics.osu_small_tick_hits
                } else {
                    api.statistics.count_50
                }
            } else {
                api.statistics.count_50
            },
            count_miss: api.statistics.count_miss,
            osu_large_tick_hits: api.statistics.osu_large_tick_hits,
            osu_small_tick_hits: api.statistics.osu_small_tick_hits,
            osu_slider_tail_hits: api.statistics.osu_slider_tail_hits,
            osu_large_tick_misses: api.statistics.osu_large_tick_misses,
            osu_small_tick_misses: api.statistics.osu_small_tick_misses,
        },
        cover_url,
        user,
        fav_count,
        play_count,
        status: bmap.map_or(String::new(), |b| b.status.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_score() -> super::OsuApiScore {
        super::OsuApiScore {
            id: 1001,
            score: 1234567,
            total_score: None,
            legacy_total_score: None,
            accuracy: 0.9876,
            max_combo: 543,
            pp: Some(300.5),
            rank: "S".to_string(),
            passed: true,
            perfect: false,
            ended_at: "2024-01-01T00:00:00Z".to_string(),
            is_lazer: None,
            build_id: None,
            has_replay: true,
            legacy_score_id: None,
            beatmap_id: 0,
            beatmapset_id: 0,
            beatmap: Some(super::super::OsuApiBeatmap {
                id: 2001,
                beatmapset_id: 3001,
                version: "Insane".to_string(),
                difficulty_rating: 5.5,
                bpm: 180.0,
                ar: 9.0,
                od: 8.0,
                cs: 4.0,
                hp: 5.0,
                total_length: 200,
                max_combo: 800,
                passcount: 100,
                playcount: 500,
                status: "ranked".to_string(),
            }),
            beatmapset: Some(super::super::OsuApiBeatmapset {
                artist: "TestArtist".to_string(),
                title: "TestTitle".to_string(),
                creator: "Mapper".to_string(),
                covers: None,
                favourite_count: 100,
                play_count: 5000,
            }),
            mods: vec![
                super::super::OsuApiMod::String("HD".to_string()),
                super::super::OsuApiMod::String("DT".to_string()),
            ],
            statistics: super::super::OsuApiScoreStatistics {
                count_geki: 0,
                count_300: 500,
                count_katu: 0,
                count_100: 10,
                count_50: 0,
                count_miss: 1,
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
                ok: 0,
            },
            ruleset_id: 0,
            user: None,
        }
    }

    #[test]
    fn test_api_score_to_score_happy_path() {
        let api = make_score();
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 2001);
        assert_eq!(score.beatmapset_id, 3001);
        assert_eq!(score.artist, "TestArtist");
        assert_eq!(score.title, "TestTitle");
        assert_eq!(score.version, "Insane");
        assert_eq!(score.creator, "Mapper");
        assert!((score.star_rating - 5.5).abs() < 0.0001);
        assert!((score.bpm - 180.0).abs() < 0.0001);
        assert!((score.ar - 9.0).abs() < 0.0001);
        assert!((score.od - 8.0).abs() < 0.0001);
        assert!((score.cs - 4.0).abs() < 0.0001);
        assert!((score.hp - 5.0).abs() < 0.0001);
        assert_eq!(score.length_seconds, 200);
        assert_eq!(score.score_value, 1234567);
        assert!((score.accuracy - 0.9850).abs() < 0.0001);
        assert_eq!(score.max_combo, 543);
        assert_eq!(score.beatmap_max_combo, 800);
        assert_eq!(score.pp, Some(300.5));
        assert_eq!(score.rank, "A");
        let mod_acronyms: Vec<String> = score
            .mods
            .iter()
            .map(|m| m.acronym().as_str().to_string())
            .collect();
        assert!(mod_acronyms.iter().any(|m| m == "HD"));
        assert!(mod_acronyms.iter().any(|m| m == "DT"));
        assert_eq!(mod_acronyms.len(), 2);
        assert!(!score.is_perfect);
        assert_eq!(score.created_at, "2024-01-01T00:00:00Z");
        assert!(!score.is_lazer);
        assert_eq!(score.statistics.count_300, 500);
        assert_eq!(score.statistics.count_100, 10);
        assert_eq!(score.statistics.count_50, 0);
        assert_eq!(score.statistics.count_miss, 1);
        assert_eq!(score.cover_url, "");
        assert_eq!(score.user.avatar_url, "");
        assert_eq!(score.user.country_code, "");
        assert_eq!(score.user.global_rank, None);
        assert_eq!(score.user.country_rank, None);
        assert!((score.user.pp - 0.0).abs() < 0.0001);
        assert_eq!(score.status, "ranked");
    }

    #[test]
    fn test_api_score_to_score_pp_null() {
        let mut api = make_score();
        api.pp = None;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.pp, None);
    }

    #[test]
    fn test_api_score_to_score_is_perfect() {
        let mut api = make_score();
        api.perfect = true;
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.is_perfect);
    }

    #[test]
    fn test_api_score_to_score_empty_mods() {
        let mut api = make_score();
        api.mods = vec![];
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.mods.is_empty());
    }

    #[test]
    fn test_api_score_to_score_nested_user_data() {
        let mut api = make_score();
        api.beatmapset.as_mut().unwrap().covers = Some(serde_json::json!({
            "cover": "https://example.com/cover.jpg"
        }));
        api.user = Some(serde_json::json!({
            "id": 1001,
            "username": "TestPlayer",
            "avatar_url": "https://example.com/avatar.png",
            "country_code": "CN",
            "statistics": {
                "global_rank": 1234,
                "country_rank": 56,
                "pp": 9876.5
            }
        }));
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.cover_url, "https://example.com/cover.jpg");
        assert_eq!(score.user.avatar_url, "https://example.com/avatar.png");
        assert_eq!(score.user.country_code, "CN");
        assert_eq!(score.user.user_id, Some(1001));
        assert_eq!(score.user.username.as_deref(), Some("TestPlayer"));
        assert_eq!(score.user.global_rank, Some(1234));
        assert_eq!(score.user.country_rank, Some(56));
        assert!((score.user.pp - 9876.5).abs() < 0.0001);
    }

    #[test]
    fn test_api_score_to_score_lazer_by_build_id() {
        let mut api = make_score();
        api.build_id = Some(12345);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_stable_when_api_is_lazer_false() {
        let mut api = make_score();
        api.is_lazer = Some(false);
        api.build_id = Some(12345);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(!score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_lazer_when_api_is_lazer_true() {
        let mut api = make_score();
        api.is_lazer = Some(true);
        api.build_id = None;
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_lazer_by_legacy_total_score_zero() {
        let mut api = make_score();
        api.is_lazer = None;
        api.build_id = None;
        api.legacy_total_score = Some(0);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(
            !score.is_lazer,
            "legacy_total_score=0 should no longer trigger is_lazer"
        );
    }

    #[test]
    fn test_api_score_to_score_not_lazer_when_build_id_zero() {
        let mut api = make_score();
        api.is_lazer = None;
        api.build_id = Some(0);
        api.legacy_total_score = Some(5000);
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(!score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_not_lazer_all_conditions_false() {
        let mut api = make_score();
        api.is_lazer = None;
        api.build_id = None;
        api.legacy_total_score = None;
        let score = api_score_to_score(api, GameMode::Osu);
        assert!(!score.is_lazer);
    }

    #[test]
    fn test_api_score_to_score_solo_score_no_beatmap() {
        let mut api = make_score();
        api.beatmap = None;
        api.beatmapset = None;
        api.beatmap_id = 9999;
        api.beatmapset_id = 8888;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 9999);
        assert_eq!(score.beatmapset_id, 8888);
        assert!(score.artist.is_empty());
        assert!(score.title.is_empty());
        assert!(score.version.is_empty());
        assert!((score.ar - 0.0).abs() < 0.0001);
        assert!((score.od - 0.0).abs() < 0.0001);
        assert_eq!(score.beatmap_max_combo, 0);
        assert!(score.status.is_empty());
        assert!(score.cover_url.is_empty());
    }

    #[test]
    fn test_api_score_to_score_solo_score_beatmap_id_zero() {
        let mut api = make_score();
        api.beatmap = None;
        api.beatmapset = None;
        api.beatmap_id = 0;
        api.beatmapset_id = 0;
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(score.beatmap_id, 0);
        assert_eq!(score.beatmapset_id, 0);
    }

    #[test]
    fn test_api_score_to_score_solo_score_covers_fullsize() {
        let mut api = make_score();
        api.beatmap = None;
        api.beatmapset = Some(super::super::OsuApiBeatmapset {
            artist: "Artist".to_string(),
            title: "Title".to_string(),
            creator: "Creator".to_string(),
            covers: Some(serde_json::json!({
                "cover": "https://a.ppy.sh/thumb/1.jpg",
                "cover@2x": "https://a.ppy.sh/thumb@2x/1.jpg",
                "card": "https://a.ppy.sh/card/1.jpg",
                "card@2x": "https://a.ppy.sh/card@2x/1.jpg",
                "list": "https://assets.ppy.sh/beatmaps/1/covers/list.jpg",
                "list@2x": "https://assets.ppy.sh/beatmaps/1/covers/list@2x.jpg",
                "slimcover": "https://a.ppy.sh/slim/1.jpg",
                "slimcover@2x": "https://a.ppy.sh/slim@2x/1.jpg",
            })),
            favourite_count: 0,
            play_count: 0,
        });
        let score = api_score_to_score(api, GameMode::Osu);
        assert_eq!(
            score.cover_url,
            "https://assets.ppy.sh/beatmaps/1/covers/fullsize.jpg"
        );
    }
}
