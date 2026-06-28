use osubot_types::{format_accuracy, format_length, format_number, Score};

const SCORE_CSS: &str = include_str!("../styles/score.css");

#[derive(serde::Serialize)]
pub struct ScoreCardData {
    pub score: Score,
    pub username: String,
    pub mode: osubot_types::GameMode,
    pub user_pp: f64,
    pub user_global_rank: Option<i64>,
    pub user_country_rank: Option<i64>,
    pub country_code: String,
    pub avatar_data_uri: String,
    pub bg_data_uri: String,
    pub thumb_data_uri: String,
    pub play_time: String,
    pub hue: u16,
    pub sat: u16,
    pub fav_count: Option<i64>,
    pub play_count: Option<i64>,
    pub pp_change: Option<f64>,
    pub global_rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
    pub ranked_status: String,
    pub ur_value: Option<f64>,
    pub ar_eff: Option<f64>,
    pub od_eff: Option<f64>,
    pub cs_eff: Option<f64>,
    pub hp_eff: Option<f64>,
}

fn pp_breakdown_sum(b: &osubot_types::PpBreakdown) -> f64 {
    b.aim.filter(|&v| v > 0.0).unwrap_or(0.0)
        + b.speed.filter(|&v| v > 0.0).unwrap_or(0.0)
        + if b.accuracy > 0.0 { b.accuracy } else { 0.0 }
        + b.flashlight.filter(|&v| v > 0.0).unwrap_or(0.0)
        + b.difficulty.filter(|&v| v > 0.0).unwrap_or(0.0)
}

fn fmt_pct(v: f64) -> i64 {
    (v / 10.0 * 100.0).min(100.0).round() as i64
}

fn fmt1(v: f64) -> String {
    format!("{:.1}", v)
}

fn fmt0(v: f64) -> String {
    format!("{:.0}", v)
}

#[must_use]
pub fn wrap_score_html(data: &ScoreCardData) -> String {
    let score = &data.score;
    let mode_str = data.mode.to_string();

    let status_lower = data.ranked_status.to_lowercase();
    let status_class = match status_lower.as_str() {
        "ranked" => "chip-status-ranked",
        "loved" => "chip-status-loved",
        "qualified" => "chip-status-qualified",
        "approved" => "chip-status-approved",
        "graveyard" => "chip-status-graveyard",
        "wip" => "chip-status-wip",
        "pending" => "chip-status-pending",
        _ => "chip-status-ranked",
    };
    let status_display = {
        let mut chars = data.ranked_status.chars();
        match chars.next() {
            None => String::new(),
            Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        }
    };

    let fav_count_formatted = data.fav_count.map(format_number);
    let play_count_formatted = data.play_count.map(format_plays);

    let global_rank = data
        .user_global_rank
        .map(format_number)
        .unwrap_or_else(|| "-".to_string());
    let country_rank = data
        .user_country_rank
        .map(format_number)
        .unwrap_or_else(|| "-".to_string());

    let pp_change_html = crate::style::format_pp_change_html(data.pp_change);
    let global_rank_change_html = crate::style::format_rank_change_html(data.global_rank_change);
    let country_rank_change_html = crate::style::format_rank_change_html(data.country_rank_change);

    let pp_val_display = format!("{:.0}", data.user_pp);

    let no_ar_cs = matches!(
        data.mode,
        osubot_types::GameMode::Taiko | osubot_types::GameMode::Mania
    );

    let score_formatted = if score.score_value > 0 {
        format_number(score.score_value)
    } else {
        "--".to_string()
    };
    let acc_formatted = format_accuracy(score.accuracy);

    let rank_class = match score.rank.as_str() {
        "XH" | "SH" | "X" | "S" => {
            if score.rank.ends_with('H') {
                "rank-s-silver"
            } else {
                "rank-s"
            }
        }
        "A" => "rank-a",
        "B" => "rank-b",
        "C" => "rank-c",
        "D" => "rank-d",
        _ => "rank-f",
    };
    let rank_display = match score.rank.as_str() {
        "XH" | "X" => "X",
        "SH" | "S" => "S",
        other => other,
    };

    let pp_str = score
        .pp
        .map(|p| format!("{:.0}", p))
        .unwrap_or_else(|| "--".to_string());

    let has_breakdown = score
        .pp_breakdown
        .as_ref()
        .map(pp_breakdown_sum)
        .unwrap_or(0.0)
        > 0.0;
    let has_if_acc = score.pp_if_acc.is_some();

    // Pre-compute stat bar values
    let make_stat =
        |base: f64, eff: Option<f64>| -> (i64, String, i64, String, bool, bool, bool, i64, i64) {
            let base_pct = fmt_pct(base);
            let base_val = fmt1(base);
            match eff {
                Some(e) if (e - base).abs() < 0.01 => (
                    base_pct,
                    base_val,
                    base_pct,
                    fmt1(e),
                    false,
                    false,
                    true,
                    0,
                    0,
                ),
                Some(e) => {
                    let e_pct = fmt_pct(e);
                    let diff = e_pct - base_pct;
                    (
                        base_pct,
                        base_val,
                        e_pct,
                        fmt1(e),
                        e > base,
                        e < base,
                        true,
                        diff.max(0),
                        (-diff).max(0),
                    )
                }
                None => (
                    base_pct,
                    base_val,
                    0,
                    String::new(),
                    false,
                    false,
                    false,
                    0,
                    0,
                ),
            }
        };

    let (
        ar_base_pct,
        ar_base_val,
        ar_eff_pct,
        ar_eff_val,
        ar_eff_gt,
        ar_eff_lt,
        ar_has_eff,
        ar_fill_diff_up,
        ar_fill_diff_down,
    ) = make_stat(score.ar, data.ar_eff);
    let (
        od_base_pct,
        od_base_val,
        od_eff_pct,
        od_eff_val,
        od_eff_gt,
        od_eff_lt,
        od_has_eff,
        od_fill_diff_up,
        od_fill_diff_down,
    ) = make_stat(score.od, data.od_eff);
    let (
        cs_base_pct,
        cs_base_val,
        cs_eff_pct,
        cs_eff_val,
        cs_eff_gt,
        cs_eff_lt,
        cs_has_eff,
        cs_fill_diff_up,
        cs_fill_diff_down,
    ) = make_stat(score.cs, data.cs_eff);
    let (
        hp_base_pct,
        hp_base_val,
        hp_eff_pct,
        hp_eff_val,
        hp_eff_gt,
        hp_eff_lt,
        hp_has_eff,
        hp_fill_diff_up,
        hp_fill_diff_down,
    ) = make_stat(score.hp, data.hp_eff);

    // Pre-compute hit counts
    let hgeki = score.statistics.count_geki;
    let h300 = score.statistics.count_300;
    let hkatu = score.statistics.count_katu;
    let h100 = score.statistics.count_100;
    let h50 = score.statistics.count_50;
    let hmiss = score.statistics.count_miss;

    // Pre-compute breakdown values
    let (bd_aim, bd_speed, bd_acc, bd_fl, bd_diff) = match score.pp_breakdown {
        Some(ref b) => (
            b.aim.filter(|&v| v > 0.0).map(fmt0),
            b.speed.filter(|&v| v > 0.0).map(fmt0),
            if b.accuracy > 0.0 {
                Some(fmt0(b.accuracy))
            } else {
                None
            },
            b.flashlight.filter(|&v| v > 0.0).map(fmt0),
            b.difficulty.filter(|&v| v > 0.0).map(fmt0),
        ),
        None => (None, None, None, None, None),
    };

    let if_acc_items: Vec<serde_json::Value> = score
        .pp_if_acc
        .as_ref()
        .map(|if_acc| {
            vec![
                serde_json::json!({"label": "95%", "val": fmt0(if_acc.acc_95)}),
                serde_json::json!({"label": "97%", "val": fmt0(if_acc.acc_97)}),
                serde_json::json!({"label": "98%", "val": fmt0(if_acc.acc_98)}),
                serde_json::json!({"label": "99%", "val": fmt0(if_acc.acc_99)}),
                serde_json::json!({"label": "100%", "val": fmt0(if_acc.acc_100)}),
            ]
        })
        .unwrap_or_default();

    let if_fc_pp = score
        .pp_if_acc
        .as_ref()
        .map(|a| fmt0(a.if_fc))
        .unwrap_or_default();

    let combo_pct_raw =
        (score.max_combo as f64 / score.beatmap_max_combo.max(1) as f64 * 100.0).min(100.0);
    let combo_pct = format!("{:.0}", combo_pct_raw);
    let combo_classes = if score.max_combo == score.beatmap_max_combo {
        "subcard-combo combo-fc"
    } else {
        "subcard-combo"
    };

    let mods_vec: Vec<String> = score.mods.iter().map(|m| m.acronym().to_string()).collect();

    let star_rating_fmt = format!("{:.2}", score.star_rating);
    let bpm_fmt = format!("{:.0}", score.bpm);

    let ur_value_display = data.ur_value.map(|ur| format!("{:.0}", ur));

    let mut ctx = tera::Context::new();
    ctx.insert("css", SCORE_CSS);
    ctx.insert("hue", &data.hue);
    ctx.insert("sat", &data.sat);
    ctx.insert("bg_data_uri", &data.bg_data_uri);
    ctx.insert("thumb_data_uri", &data.thumb_data_uri);
    ctx.insert("avatar_data_uri", &data.avatar_data_uri);
    ctx.insert("score", &data.score);
    ctx.insert("username", &data.username);
    ctx.insert("mode", &mode_str);
    ctx.insert("star_rating_fmt", &star_rating_fmt);
    ctx.insert("bpm_fmt", &bpm_fmt);
    ctx.insert("global_rank", &global_rank);
    ctx.insert("country_rank", &country_rank);
    ctx.insert("country_code", &data.country_code);
    ctx.insert("pp_change_html", &pp_change_html);
    ctx.insert("global_rank_change_html", &global_rank_change_html);
    ctx.insert("country_rank_change_html", &country_rank_change_html);
    ctx.insert("pp_val_display", &pp_val_display);
    ctx.insert("no_ar_cs", &no_ar_cs);
    ctx.insert("status_class", &status_class);
    ctx.insert("status_display", &status_display);
    ctx.insert("fav_count", &data.fav_count);
    ctx.insert("fav_count_formatted", &fav_count_formatted);
    ctx.insert("play_count", &data.play_count);
    ctx.insert("play_count_formatted", &play_count_formatted);
    ctx.insert("play_time", &data.play_time);
    ctx.insert("length", &format_length(score.length_seconds));
    ctx.insert("rank_class", &rank_class);
    ctx.insert("rank_display", &rank_display);
    ctx.insert("score_formatted", &score_formatted);
    ctx.insert("acc_formatted", &acc_formatted);
    ctx.insert("pp_str", &pp_str);
    ctx.insert("has_breakdown", &has_breakdown);
    ctx.insert("has_if_acc", &has_if_acc);
    ctx.insert("if_acc_items", &if_acc_items);
    ctx.insert("if_fc_pp", &if_fc_pp);
    ctx.insert("combo_pct", &combo_pct);
    ctx.insert("combo_classes", &combo_classes);
    ctx.insert("ur_value", &ur_value_display);
    ctx.insert("mods", &mods_vec);

    // Stat bars
    ctx.insert("ar_base_pct", &ar_base_pct);
    ctx.insert("ar_base_val", &ar_base_val);
    ctx.insert("ar_eff_pct", &ar_eff_pct);
    ctx.insert("ar_eff_val", &ar_eff_val);
    ctx.insert("ar_eff_gt", &ar_eff_gt);
    ctx.insert("ar_eff_lt", &ar_eff_lt);
    ctx.insert("ar_has_eff", &ar_has_eff);
    ctx.insert("ar_fill_diff_up", &ar_fill_diff_up);
    ctx.insert("ar_fill_diff_down", &ar_fill_diff_down);
    ctx.insert("od_base_pct", &od_base_pct);
    ctx.insert("od_base_val", &od_base_val);
    ctx.insert("od_eff_pct", &od_eff_pct);
    ctx.insert("od_eff_val", &od_eff_val);
    ctx.insert("od_eff_gt", &od_eff_gt);
    ctx.insert("od_eff_lt", &od_eff_lt);
    ctx.insert("od_has_eff", &od_has_eff);
    ctx.insert("od_fill_diff_up", &od_fill_diff_up);
    ctx.insert("od_fill_diff_down", &od_fill_diff_down);
    ctx.insert("cs_base_pct", &cs_base_pct);
    ctx.insert("cs_base_val", &cs_base_val);
    ctx.insert("cs_eff_pct", &cs_eff_pct);
    ctx.insert("cs_eff_val", &cs_eff_val);
    ctx.insert("cs_eff_gt", &cs_eff_gt);
    ctx.insert("cs_eff_lt", &cs_eff_lt);
    ctx.insert("cs_has_eff", &cs_has_eff);
    ctx.insert("cs_fill_diff_up", &cs_fill_diff_up);
    ctx.insert("cs_fill_diff_down", &cs_fill_diff_down);
    ctx.insert("hp_base_pct", &hp_base_pct);
    ctx.insert("hp_base_val", &hp_base_val);
    ctx.insert("hp_eff_pct", &hp_eff_pct);
    ctx.insert("hp_eff_val", &hp_eff_val);
    ctx.insert("hp_eff_gt", &hp_eff_gt);
    ctx.insert("hp_eff_lt", &hp_eff_lt);
    ctx.insert("hp_has_eff", &hp_has_eff);
    ctx.insert("hp_fill_diff_up", &hp_fill_diff_up);
    ctx.insert("hp_fill_diff_down", &hp_fill_diff_down);

    // Hit counts
    ctx.insert("hgeki", &hgeki);
    ctx.insert("h300", &h300);
    ctx.insert("hkatu", &hkatu);
    ctx.insert("h100", &h100);
    ctx.insert("h50", &h50);
    ctx.insert("hmiss", &hmiss);

    // Breakdown
    ctx.insert("bd_aim", &bd_aim);
    ctx.insert("bd_speed", &bd_speed);
    ctx.insert("bd_acc", &bd_acc);
    ctx.insert("bd_fl", &bd_fl);
    ctx.insert("bd_diff", &bd_diff);

    crate::template::render("score.html", &ctx)
}

#[must_use]
fn format_plays(val: i64) -> String {
    if val >= 1_000_000 {
        let f = val as f64 / 1_000_000.0;
        if f == f.floor() {
            format!("{:.0}M", f)
        } else {
            format!("{:.1}M", f)
        }
    } else if val >= 1_000 {
        let f = val as f64 / 1_000.0;
        if f == f.floor() {
            format!("{:.0}K", f)
        } else {
            format!("{:.1}K", f)
        }
    } else {
        val.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_types::{Score, ScoreStatistics, ScoreUser};
    use rosu_mods::GameMods;

    fn make_test_score_data() -> ScoreCardData {
        ScoreCardData {
            score: make_test_score(),
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: Some(1234),
            play_count: Some(56700),
            pp_change: Some(12.0),
            global_rank_change: Some(-99999),
            country_rank_change: Some(-99999),
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        }
    }

    fn make_test_score() -> Score {
        let mut mods = GameMods::new();
        mods.insert(rosu_mods::GameMod::HiddenOsu(Default::default()));
        mods.insert(rosu_mods::GameMod::DoubleTimeOsu(Default::default()));
        Score {
            score_id: 99999,
            beatmap_id: 12345,
            beatmapset_id: 6789,
            artist: "TestArtist".to_string(),
            title: "TestTitle".to_string(),
            version: "Expert".to_string(),
            creator: "Mapper".to_string(),
            star_rating: 6.50,
            bpm: 180.0,
            ar: 9.3,
            od: 8.5,
            cs: 4.0,
            hp: 6.0,
            length_seconds: 222,
            score_value: 1234567,
            accuracy: 0.985,
            max_combo: 4000,
            beatmap_max_combo: 9999,
            pp: Some(456.7),
            pp_breakdown: None,
            pp_if_acc: None,
            perfect_pp: None,
            rank: "S".to_string(),
            passed: true,
            mods,
            is_perfect: false,
            created_at: "2025-05-27T14:30:22Z".to_string(),
            is_lazer: false,
            has_replay: true,
            legacy_score_id: None,
            statistics: ScoreStatistics {
                count_geki: 0,
                count_300: 856,
                count_katu: 0,
                count_100: 45,
                count_50: 12,
                count_miss: 2,
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
            },
            cover_url: "https://example.com/cover.jpg".to_string(),
            user: osubot_types::ScoreUser {
                avatar_url: "https://example.com/avatar.jpg".to_string(),
                country_code: "CN".to_string(),
                user_id: None,
                username: None,
                global_rank: Some(999999),
                country_rank: Some(999999),
                pp: 9876.5,
            },
            fav_count: Some(1234),
            play_count: Some(56700),
            status: "ranked".to_string(),
        }
    }

    #[test]
    fn test_wrap_score_html_contains_key_elements() {
        let data = ScoreCardData {
            score: make_test_score(),
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: Some(1234),
            play_count: Some(56700),
            pp_change: Some(12.0),
            global_rank_change: Some(-99999),
            country_rank_change: Some(-99999),
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);

        // Top row
        assert!(html.contains("TestTitle"));
        assert!(html.contains("TestArtist"));
        assert!(html.contains("★ 6.50"));
        assert!(html.contains("Expert"));
        // Middle row
        assert!(html.contains("TestPlayer"));
        assert!(html.contains(">999,999<"));
        assert!(html.contains("9876pp"));
        assert!(html.contains(">-99999<"));
        assert!(html.contains(">9.3<"), "AR value missing");
        assert!(html.contains(">8.5<"), "OD value missing");
        assert!(html.contains(">4.0<"), "CS value missing");
        assert!(html.contains(">6.0<"), "HP value missing");
        // Hits
        assert!(html.contains("856×"));
        assert!(html.contains("45×"));
        assert!(html.contains("12×"));
        assert!(html.contains("2×"));
        // New layout structure
        assert!(html.contains("hits-row"), "hits-row wrapper missing");
        assert!(html.contains("score-acc-row"), "score-acc-row missing");
        // Rank badge in score-acc-row
        assert!(html.contains("rank-s"), "rank badge missing");
        // Score + ACC modules
        assert!(html.contains("stat-mod-score"), "score module missing");
        assert!(html.contains("1,234,567"), "score value missing");
        assert!(html.contains("stat-mod-acc"), "acc module missing");
        assert!(html.contains("98.5%"), "acc value missing");
        // Mod chips
        assert!(html.contains("HD"));
        assert!(html.contains("DT"));
        assert!(html.contains("chip-filled"), "mod chip missing");
        // Combo
        assert!(html.contains("subcard-pp"), "PP subcard class missing");
        assert!(
            html.contains("subcard-combo"),
            "Combo subcard class missing"
        );
        assert!(html.contains("4000"), "combo value missing");
        assert!(html.contains("9999"), "combo total missing");
        assert!(html.contains("COMBO"), "combo label missing");
        assert!(
            !html.contains("MAX COMBO"),
            "old MAX COMBO label should be gone"
        );
        // Meta
        assert!(html.contains("2025/05/27 14:30:22"));
        assert!(html.contains("--score-hue: 200"));
        assert!(html.contains("chip-status"), "chip-status class missing");
        assert!(html.contains(">Ranked<"), "Ranked status text missing");
        assert!(
            html.contains("chip-fav") && html.contains("1,234"),
            "fav_count chip missing"
        );
        assert!(
            html.contains("chip-plays") && html.contains("56.7K"),
            "play_count chip missing"
        );
        assert!(
            html.contains("user-pp-change up"),
            "pp-change up class missing"
        );
    }

    #[test]
    fn test_wrap_score_html_uses_css_variables() {
        let data = make_test_score_data();
        let html = wrap_score_html(&data);
        assert!(html.contains("--score-hue: 200"), "missing hue CSS var");
        assert!(html.contains("--score-sat: 60%"), "missing sat CSS var");
        assert!(
            !html.contains("{{SCORE_HUE}}"),
            "should not have template placeholder"
        );
        assert!(
            !html.contains("{{SCORE_SAT}}"),
            "should not have template placeholder"
        );
        let style_count = html.matches("<style>").count();
        assert!(
            style_count >= 2,
            "expected at least 2 <style> tags (SCORE_CSS + runtime vars), got {style_count}",
        );
    }

    #[test]
    fn test_score_css_has_no_template_placeholders() {
        const SCORE_CSS_LOCAL: &str = include_str!("../styles/score.css");
        assert!(
            !SCORE_CSS_LOCAL.contains("{{SCORE_HUE}}"),
            "score.css should not have {{SCORE_HUE}} placeholder"
        );
        assert!(
            !SCORE_CSS_LOCAL.contains("{{SCORE_SAT}}"),
            "score.css should not have {{SCORE_SAT}} placeholder"
        );
    }

    #[test]
    fn test_format_length() {
        assert_eq!(format_length(222), "3:42");
        assert_eq!(format_length(60), "1:00");
        assert_eq!(format_length(0), "0:00");
    }

    #[test]
    fn test_format_number() {
        assert_eq!(format_number(1234567), "1,234,567");
        assert_eq!(format_number(100), "100");
        assert_eq!(format_number(0), "0");
        assert_eq!(format_number(1234), "1,234");
        assert_eq!(format_number(56), "56");
    }

    #[test]
    fn test_tera_render_error_detail() {
        let data = make_test_score_data();
        let html = wrap_score_html(&data);
        if html.starts_with("Tera render error") {
            panic!("Render failed: {html}");
        }
    }

    #[test]
    fn test_format_plays() {
        assert_eq!(format_plays(56700), "56.7K");
        assert_eq!(format_plays(1234567), "1.2M");
        assert_eq!(format_plays(500), "500");
        assert_eq!(format_plays(1000), "1K");
        assert_eq!(format_plays(1000000), "1M");
        assert_eq!(format_plays(1500), "1.5K");
    }

    #[test]
    fn test_pp_breakdown_osu_standard() {
        use osubot_types::PpBreakdown;

        let mut score = make_test_score();
        score.pp_breakdown = Some(PpBreakdown {
            aim: Some(180.0),
            speed: Some(95.0),
            accuracy: 42.0,
            flashlight: Some(10.0),
            difficulty: None,
            total_pp: 327.0,
            star_rating: None,
        });
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);
        assert!(
            html.contains(r#"class="subcard-breakdown""#),
            "subcard-breakdown div missing"
        );
        assert!(html.contains("pp-chip-aim"), "aim chip missing");
        assert!(html.contains("pp-chip-speed"), "speed chip missing");
        assert!(html.contains("pp-chip-acc"), "acc chip missing");
        assert!(html.contains("pp-chip-fl"), "fl chip missing");
        assert!(html.contains("AIM"), "AIM label missing");
        assert!(html.contains("180"), "AIM value missing");
        assert!(html.contains("SPD"), "SPD label missing");
        assert!(html.contains("95"), "SPD value missing");
        assert!(html.contains("ACC"), "ACC label missing");
        assert!(html.contains("42"), "ACC value missing");
        assert!(html.contains("FL"), "FL label missing");
        assert!(html.contains("10"), "FL value missing");
    }

    #[test]
    fn test_pp_breakdown_taiko() {
        use osubot_types::PpBreakdown;

        let mut score = make_test_score();
        score.pp_breakdown = Some(PpBreakdown {
            aim: None,
            speed: None,
            accuracy: 80.0,
            flashlight: None,
            difficulty: Some(200.0),
            total_pp: 280.0,
            star_rating: None,
        });
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);
        assert!(
            html.contains(r#"class="subcard-breakdown""#),
            "subcard-breakdown div missing"
        );
        assert!(html.contains("pp-chip-diff"), "diff chip missing");
        assert!(html.contains("DIFF"), "DIFF label missing");
        assert!(html.contains("200"), "DIFF value missing");
        assert!(html.contains("ACC"), "ACC label missing");
        assert!(html.contains("80"), "ACC value missing");
        assert!(
            !html.contains(r#"class="chip pp-chip-aim""#),
            "AIM chip should not appear for taiko"
        );
    }

    #[test]
    fn test_pp_breakdown_none() {
        let score = make_test_score();
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);
        assert!(
            !html.contains(r#"class="subcard-breakdown""#),
            "subcard-breakdown div should not appear when None"
        );
    }

    #[test]
    fn test_if_acc_card() {
        use osubot_types::PpIfAcc;

        let mut score = make_test_score();
        score.pp_if_acc = Some(PpIfAcc {
            acc_95: 320.0,
            acc_97: 380.0,
            acc_98: 410.0,
            acc_99: 440.0,
            acc_100: 480.0,
            if_fc: 520.0,
            perfect_pp: 600.0,
        });
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);
        assert!(
            html.contains(r#"class="subcard-if-acc""#),
            "subcard-if-acc div missing"
        );
        assert!(html.contains(">320<"), "95% PP value missing");
        assert!(html.contains(">380<"), "97% PP value missing");
        assert!(html.contains(">410<"), "98% PP value missing");
        assert!(html.contains(">440<"), "99% PP value missing");
        assert!(html.contains(">480<"), "100% PP value missing");
        assert!(html.contains("IF FC"), "IF FC missing");
        assert!(html.contains("520"), "IF FC value missing");
        assert!(html.contains("pp-unit"), "IF FC pp-unit missing");
        assert!(html.contains("if-acc-item"), "if-acc-item class missing");
    }

    #[test]
    fn test_if_acc_card_none() {
        let score = make_test_score();
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "Ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);
        assert!(
            !html.contains(r#"class="subcard-if-acc""#),
            "subcard-if-acc should not appear when None"
        );
    }

    fn make_score_with_xss() -> ScoreCardData {
        let mut mods = GameMods::new();
        mods.insert(rosu_mods::GameMod::HiddenOsu(Default::default()));
        ScoreCardData {
            score: Score {
                score_id: 1,
                beatmap_id: 1,
                beatmapset_id: 1,
                artist: "<script>alert('xss')</script>".to_string(),
                title: "Title <img onerror=alert(1)>".to_string(),
                version: "Normal & Hard".to_string(),
                creator: "User\"onmouseover=alert(1)".to_string(),
                star_rating: 5.0,
                bpm: 180.0,
                ar: 9.0,
                od: 8.0,
                cs: 4.0,
                hp: 6.0,
                length_seconds: 120,
                score_value: 1000000,
                accuracy: 0.98,
                max_combo: 500,
                beatmap_max_combo: 600,
                pp: Some(200.0),
                pp_breakdown: None,
                pp_if_acc: None,
                perfect_pp: None,
                rank: "A".to_string(),
                passed: true,
                mods,
                is_perfect: false,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                is_lazer: false,
                has_replay: true,
                legacy_score_id: None,
                statistics: ScoreStatistics {
                    count_geki: 0,
                    count_300: 400,
                    count_katu: 0,
                    count_100: 50,
                    count_50: 10,
                    count_miss: 5,
                    osu_large_tick_hits: 0,
                    osu_small_tick_hits: 0,
                    osu_slider_tail_hits: 0,
                    osu_large_tick_misses: 0,
                    osu_small_tick_misses: 0,
                },
                cover_url: String::new(),
                user: ScoreUser {
                    avatar_url: String::new(),
                    country_code: "CN".to_string(),
                    user_id: None,
                    username: None,
                    global_rank: Some(999999),
                    country_rank: Some(999999),
                    pp: 5000.0,
                },
                fav_count: None,
                play_count: None,
                status: "ranked".to_string(),
            },
            username: "TestUser".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 5000.0,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: String::new(),
            bg_data_uri: String::new(),
            thumb_data_uri: String::new(),
            play_time: "2025/01/01 08:00:00".to_string(),
            hue: 200,
            sat: 50,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "ranked".to_string(),
            ur_value: None,
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        }
    }

    #[test]
    fn test_xss_escaping_in_score_html() {
        let data = make_score_with_xss();
        let html = wrap_score_html(&data);

        // Raw script tags must not appear
        assert!(
            !html.contains("<script>"),
            "HTML should not contain raw <script> tags"
        );
        assert!(
            !html.contains("<img onerror"),
            "HTML should not contain unescaped <img onerror"
        );
        assert!(
            !html.contains("<img onmouseover"),
            "HTML should not contain unescaped onmouseover attribute"
        );

        // Escaped versions should be present
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;img onerror"));
        assert!(html.contains("&amp;"));
    }

    #[test]
    fn test_ur_value_rounded_to_integer() {
        let score = make_test_score();
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            mode: osubot_types::GameMode::Osu,
            user_pp: 9876.5,
            user_global_rank: Some(999999),
            user_country_rank: Some(999999),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: None,
            play_count: None,
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
            ranked_status: "Ranked".to_string(),
            ur_value: Some(999.9),
            ar_eff: None,
            od_eff: None,
            cs_eff: None,
            hp_eff: None,
        };
        let html = wrap_score_html(&data);

        // UR should be rounded to integer (999.9 → 1000)
        assert!(
            html.contains(">1000<"),
            "UR value 999.9 should be rounded to 1000"
        );
        assert!(
            !html.contains("999.9"),
            "UR value should not contain decimal point"
        );
    }
}
