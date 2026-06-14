use chrono::{DateTime, Utc};

use crate::time_parser::TimeParser;
use crate::types::{Condition, ConditionOp};
use osubot_types::Score;

/// 对分数列表应用过滤条件，原地过滤
pub fn apply_conditions(scores: &mut Vec<Score>, conditions: &[Condition]) {
    if conditions.is_empty() {
        return;
    }
    scores.retain(|score| conditions.iter().all(|cond| matches_condition(score, cond)));
}

fn matches_condition(score: &Score, cond: &Condition) -> bool {
    let is_time_field = cond.field == "date";
    let field_value = extract_field(score, &cond.field);
    match field_value {
        Some(val) => compare_values(&val, &cond.operator, &cond.value, is_time_field),
        None => false,
    }
}

enum FieldValue {
    Str(String),
    Num(f64),
}

fn extract_field(score: &Score, field: &str) -> Option<FieldValue> {
    match field {
        // 谱面字段
        "creator" | "host" => Some(FieldValue::Str(score.creator.clone())),
        "bid" | "id" => Some(FieldValue::Num(score.beatmap_id as f64)),
        "sid" | "beatmapset" | "beatmapset_id" => Some(FieldValue::Num(score.beatmapset_id as f64)),
        "title" | "name" | "song" => Some(FieldValue::Str(score.title.clone())),
        "artist" | "singer" => Some(FieldValue::Str(score.artist.clone())),
        "difficulty" | "diff" => Some(FieldValue::Str(score.version.clone())),
        "sr" | "star" | "rating" => Some(FieldValue::Num(score.star_rating)),
        "ar" | "approach" => Some(FieldValue::Num(score.ar)),
        "cs" | "circle" | "keys" => Some(FieldValue::Num(score.cs)),
        "od" | "overall" => Some(FieldValue::Num(score.od)),
        "hp" | "health" => Some(FieldValue::Num(score.hp)),
        "length" | "long" => Some(FieldValue::Num(score.length_seconds as f64)),
        "bpm" => Some(FieldValue::Num(score.bpm)),
        // 成绩字段
        "pp" | "performance" => Some(FieldValue::Num(score.pp.unwrap_or(0.0))),
        "rank" | "ranking" => Some(FieldValue::Str(score.rank.clone())),
        "mod" | "mods" => {
            let mods_str = osubot_types::format_mods(&score.mods);
            Some(FieldValue::Str(mods_str))
        }
        "acc" | "accuracy" => Some(FieldValue::Num(score.accuracy)),
        "combo" => Some(FieldValue::Num(score.max_combo as f64)),
        "client" | "version" => Some(FieldValue::Str(
            if score.is_lazer { "lazer" } else { "stable" }.to_string(),
        )),
        // statistics 字段
        "miss" => Some(FieldValue::Num(score.statistics.count_miss as f64)),
        "perfect" => Some(FieldValue::Num(score.statistics.count_geki as f64)),
        "great" => Some(FieldValue::Num(score.statistics.count_300 as f64)),
        "good" => Some(FieldValue::Num(score.statistics.count_katu as f64)),
        "ok" | "bad" | "large droplet" => Some(FieldValue::Num(score.statistics.count_100 as f64)),
        "meh" | "poor" | "small droplet" => Some(FieldValue::Num(score.statistics.count_50 as f64)),
        "missed_fruit" | "miss fruit" => Some(FieldValue::Num(
            score.statistics.osu_large_tick_misses as f64,
        )),
        "missed_drop" | "miss drop" => Some(FieldValue::Num(
            score.statistics.osu_small_tick_misses as f64,
        )),
        "missed_droplet" | "miss droplet" => Some(FieldValue::Num(
            score.statistics.osu_small_tick_misses as f64,
        )),
        "total" | "object" => {
            let total = score.statistics.count_300
                + score.statistics.count_100
                + score.statistics.count_50
                + score.statistics.count_miss;
            Some(FieldValue::Num(total as f64))
        }
        // 新增可计算字段
        "convert" => Some(FieldValue::Num(0.0)),
        "sliders" | "long note" => Some(FieldValue::Num(
            score.statistics.osu_slider_tail_hits as f64,
        )),
        "spinners" | "rattle" => Some(FieldValue::Num(score.statistics.osu_large_tick_hits as f64)),
        "rate" => {
            let total = score.statistics.count_geki
                + score.statistics.count_300
                + score.statistics.count_katu
                + score.statistics.count_100
                + score.statistics.count_50
                + score.statistics.count_miss;
            if total == 0 {
                Some(FieldValue::Num(0.0))
            } else {
                Some(FieldValue::Num(
                    (score.statistics.count_geki as f64 / total as f64) * 100.0,
                ))
            }
        }
        "date" => Some(FieldValue::Num(parse_timestamp(&score.created_at) as f64)),
        "any" | "thing" | "y" => None,
        // 不支持的字段返回 None
        _ => None,
    }
}

fn compare_values(
    field_val: &FieldValue,
    op: &ConditionOp,
    filter_val: &str,
    is_time_field: bool,
) -> bool {
    match field_val {
        FieldValue::Str(s) => compare_str(s, op, filter_val),
        FieldValue::Num(n) => {
            if is_time_field {
                fit_time(op, *n as i64, filter_val)
            } else {
                compare_num(*n, op, filter_val)
            }
        }
    }
}

fn compare_str(actual: &str, op: &ConditionOp, expected: &str) -> bool {
    match op {
        ConditionOp::Eq => actual.to_lowercase().contains(&expected.to_lowercase()),
        ConditionOp::XEq => actual.eq_ignore_ascii_case(expected),
        ConditionOp::Ne => !actual.to_lowercase().contains(&expected.to_lowercase()),
        ConditionOp::Gt => actual.len() > expected.len(),
        ConditionOp::Ge => actual.len() >= expected.len(),
        ConditionOp::Lt => actual.len() < expected.len(),
        ConditionOp::Le => actual.len() <= expected.len(),
    }
}

fn compare_num(actual: f64, op: &ConditionOp, expected: &str) -> bool {
    let exp: f64 = match expected.parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    match op {
        ConditionOp::Eq | ConditionOp::XEq => (actual - exp).abs() < f64::EPSILON,
        ConditionOp::Ne => (actual - exp).abs() >= f64::EPSILON,
        ConditionOp::Gt => actual > exp,
        ConditionOp::Ge => actual >= exp,
        ConditionOp::Lt => actual < exp,
        ConditionOp::Le => actual <= exp,
    }
}

fn parse_timestamp(created_at: &str) -> i64 {
    created_at
        .parse::<DateTime<Utc>>()
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

fn fit_time(op: &ConditionOp, score_timestamp: i64, time_str: &str) -> bool {
    let parsed = match TimeParser::process(time_str) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let time = parsed.timestamp;
    let is_relative = parsed.is_relative;
    match op {
        ConditionOp::Gt => {
            if is_relative {
                score_timestamp < time
            } else {
                score_timestamp > time
            }
        }
        ConditionOp::Ge => {
            if is_relative {
                score_timestamp <= time
            } else {
                score_timestamp >= time
            }
        }
        ConditionOp::Lt => {
            if is_relative {
                score_timestamp > time
            } else {
                score_timestamp < time
            }
        }
        ConditionOp::Le => {
            if is_relative {
                score_timestamp >= time
            } else {
                score_timestamp <= time
            }
        }
        ConditionOp::Eq => {
            let start = TimeParser::start_of_day(time);
            score_timestamp >= start && score_timestamp < start + 86400
        }
        ConditionOp::XEq => {
            let start = TimeParser::start_of_hour(time);
            score_timestamp >= start && score_timestamp < start + 3600
        }
        ConditionOp::Ne => {
            let start = TimeParser::start_of_day(time);
            score_timestamp < start || score_timestamp >= start + 86400
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_types::{Score, ScoreStatistics, ScoreUser};
    use rosu_mods::GameMods;

    fn make_score(pp: Option<f64>, acc: f64, rank: &str, star: f64) -> Score {
        Score {
            score_id: 1,
            beatmap_id: 123,
            beatmapset_id: 456,
            artist: "Test".into(),
            title: "Song".into(),
            version: "Insane".into(),
            creator: "Mapper".into(),
            star_rating: star,
            bpm: 180.0,
            ar: 9.5,
            od: 9.0,
            cs: 4.0,
            hp: 5.0,
            length_seconds: 120,
            score_value: 1000000,
            accuracy: acc,
            max_combo: 1000,
            beatmap_max_combo: 1200,
            pp,
            pp_breakdown: None,
            pp_if_acc: None,
            perfect_pp: None,
            rank: rank.into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            is_lazer: false,
            mods: GameMods::new(),
            is_perfect: false,
            has_replay: false,
            legacy_score_id: None,
            statistics: ScoreStatistics {
                count_geki: 0,
                count_300: 500,
                count_katu: 0,
                count_100: 0,
                count_50: 0,
                count_miss: 0,
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
            },
            cover_url: "".into(),
            user: ScoreUser {
                avatar_url: "".into(),
                country_code: "JP".into(),
                user_id: Some(1),
                username: Some("player".into()),
                global_rank: Some(100),
                country_rank: Some(10),
                pp: 5000.0,
            },
            fav_count: None,
            play_count: None,
            status: "ranked".into(),
            passed: true,
        }
    }

    #[test]
    fn filter_pp_greater_than() {
        let mut scores = vec![
            make_score(Some(300.0), 98.0, "A", 6.5),
            make_score(Some(200.0), 95.0, "B", 5.0),
        ];
        let cond = Condition {
            field: "pp".into(),
            operator: ConditionOp::Gt,
            value: "250".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].pp, Some(300.0));
    }

    #[test]
    fn filter_rank_equals() {
        let mut scores = vec![
            make_score(Some(300.0), 98.0, "A", 6.5),
            make_score(Some(200.0), 95.0, "B", 5.0),
        ];
        let cond = Condition {
            field: "rank".into(),
            operator: ConditionOp::Eq,
            value: "A".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn filter_star_rating_range() {
        let mut scores = vec![
            make_score(Some(300.0), 98.0, "A", 7.5),
            make_score(Some(200.0), 95.0, "B", 5.0),
            make_score(Some(150.0), 90.0, "C", 6.0),
        ];
        let cond = Condition {
            field: "star".into(),
            operator: ConditionOp::Ge,
            value: "6.0".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 2);
    }

    #[test]
    fn filter_multi_condition() {
        let mut scores = vec![
            make_score(Some(300.0), 98.0, "A", 7.5),
            make_score(Some(200.0), 95.0, "B", 5.0),
            make_score(Some(150.0), 90.0, "A", 6.0),
        ];
        let conds = vec![
            Condition {
                field: "rank".into(),
                operator: ConditionOp::Eq,
                value: "A".into(),
            },
            Condition {
                field: "star".into(),
                operator: ConditionOp::Ge,
                value: "6.5".into(),
            },
        ];
        apply_conditions(&mut scores, &conds);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn filter_no_conditions_does_nothing() {
        let mut scores = vec![make_score(Some(300.0), 98.0, "A", 6.5)];
        apply_conditions(&mut scores, &[]);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn filter_unknown_field_returns_none() {
        let mut scores = vec![make_score(Some(300.0), 98.0, "A", 6.5)];
        let cond = Condition {
            field: "nonexistent".into(),
            operator: ConditionOp::Eq,
            value: "x".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 0);
    }

    #[test]
    fn filter_ne_operator() {
        let mut scores = vec![
            make_score(Some(300.0), 98.0, "XH", 6.5),
            make_score(Some(200.0), 95.0, "A", 5.0),
        ];
        let cond = Condition {
            field: "rank".into(),
            operator: ConditionOp::Ne,
            value: "XH".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
        assert_eq!(scores[0].rank, "A");
    }

    #[test]
    fn filter_acc_gt() {
        let mut scores = vec![
            make_score(Some(300.0), 98.5, "A", 6.5),
            make_score(Some(200.0), 95.0, "B", 5.0),
        ];
        let cond = Condition {
            field: "acc".into(),
            operator: ConditionOp::Gt,
            value: "97.0".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn filter_rank_xeq() {
        let mut scores = vec![
            make_score(Some(300.0), 98.0, "SS", 6.5),
            make_score(Some(200.0), 95.0, "S", 5.0),
        ];
        let cond = Condition {
            field: "rank".into(),
            operator: ConditionOp::XEq,
            value: "SS".into(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn test_filter_by_date_absolute() {
        let mut scores = vec![{
            let mut s = make_score(None, 100.0, "SS", 5.0);
            s.created_at = "2024-01-15T12:00:00Z".into();
            s
        }];
        let cond = Condition {
            field: "date".to_string(),
            operator: ConditionOp::Gt,
            value: "2024-01-01".to_string(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn test_filter_by_date_relative() {
        let mut scores = vec![{
            let mut s = make_score(None, 100.0, "SS", 5.0);
            s.created_at = "2025-12-01T12:00:00Z".into();
            s
        }];
        let cond = Condition {
            field: "date".to_string(),
            operator: ConditionOp::Lt,
            value: "365d".to_string(),
        };
        apply_conditions(&mut scores, &[cond]);
        assert_eq!(scores.len(), 1);
    }

    #[test]
    fn date_aliases_are_not_supported() {
        for field in ["created time", "score time", "ct", "ca", "ti", "st"] {
            let mut scores = vec![{
                let mut s = make_score(None, 100.0, "SS", 5.0);
                s.created_at = "2025-12-01T12:00:00Z".into();
                s
            }];
            let cond = Condition {
                field: field.to_string(),
                operator: ConditionOp::Lt,
                value: "365d".to_string(),
            };
            apply_conditions(&mut scores, &[cond]);
            assert_eq!(scores.len(), 0, "field {field} should not be supported");
        }
    }

    #[test]
    fn filter_readme_statistics_fields_and_aliases() {
        for field in [
            "perfect",
            "great",
            "good",
            "ok",
            "bad",
            "large droplet",
            "meh",
            "poor",
            "small droplet",
            "missed_fruit",
            "miss fruit",
            "missed_drop",
            "miss drop",
            "missed_droplet",
            "miss droplet",
        ] {
            let mut score = make_score(None, 100.0, "SS", 5.0);
            score.statistics.count_geki = 1;
            score.statistics.count_300 = 1;
            score.statistics.count_katu = 1;
            score.statistics.count_100 = 1;
            score.statistics.count_50 = 1;
            score.statistics.osu_large_tick_misses = 1;
            score.statistics.osu_small_tick_misses = 1;
            let mut scores = vec![score];
            let cond = Condition {
                field: field.to_string(),
                operator: ConditionOp::Eq,
                value: "1".to_string(),
            };
            apply_conditions(&mut scores, &[cond]);
            assert_eq!(scores.len(), 1, "field {field} should match README");
        }
    }
}
