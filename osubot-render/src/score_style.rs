use maud::{html, Markup, PreEscaped};
use osubot_types::{format_accuracy, format_length, format_number, Score};

const SCORE_CSS: &str = include_str!("../styles/score.css");

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

fn stat_bar(label: &str, base: f64, eff: Option<f64>) -> Markup {
    let base_pct = (base / 10.0 * 100.0).min(100.0);
    let (track_html, val_str) = match eff {
        Some(e) if (e - base).abs() < 0.01 => {
            let track = format!(r#"<div class="fill" style="width:{base_pct:.0}%"></div>"#);
            (track, format!("<span>{:.1}</span>", base))
        }
        Some(e) if e > base => {
            let eff_pct = (e / 10.0 * 100.0).min(100.0);
            let overflow_pct = eff_pct - base_pct;
            let track = format!(
                r#"<div class="fill" style="width:{base_pct:.0}%"></div><div class="fill-over" style="left:calc({base_pct:.0}% - 2px); width:calc({overflow_pct:.0}% + 2px)"></div>"#
            );
            (
                track,
                format!(
                    r#"<span class="val-eff-up">{:.1}</span><span>[{:.1}]</span>"#,
                    e, base
                ),
            )
        }
        Some(e) => {
            let eff_pct = (e / 10.0 * 100.0).min(100.0);
            let under_pct = base_pct - eff_pct;
            let track = format!(
                r#"<div class="fill" style="width:{eff_pct:.0}%"></div><div class="fill-under" style="left:calc({eff_pct:.0}% - 2px); width:calc({under_pct:.0}% + 2px)"></div>"#
            );
            (
                track,
                format!(
                    r#"<span class="val-eff-down">{:.1}</span><span>[{:.1}]</span>"#,
                    e, base
                ),
            )
        }
        None => {
            let track = format!(r#"<div class="fill" style="width:{base_pct:.0}%"></div>"#);
            (track, format!("<span>{:.1}</span>", base))
        }
    };
    html! {
        div.stat-row {
            span.label { (label) }
            div.track { (PreEscaped(track_html)) }
            span.val { (PreEscaped(val_str)) }
        }
    }
}

fn render_top_row(data: &ScoreCardData) -> Markup {
    let score = &data.score;
    let length = format_length(score.length_seconds);

    let fav_chip = data.fav_count.map(|c| {
        html! {
            span.chip.chip-fav {
                span.chip-icon { "\u{2665}" }
                span.chip-num { (format_number(c)) }
            }
        }
    });
    let plays_chip = data.play_count.map(|c| {
        html! {
            span.chip.chip-plays {
                span.chip-icon {}
                span.chip-num { (format_plays(c)) }
            }
        }
    });

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

    html! {
        div.top-row {
            div.beatmap-card.surface {
                div.cover-wrap {
                    img src=(data.thumb_data_uri);
                }
                div.beatmap-text {
                    div.beatmap-title { (score.title) }
                    div.beatmap-artist { (score.artist) }
                    div.bottom-chips {
                        span class=(format!("chip chip-status {}", status_class)) { (status_display) }
                        span.chip.chip-diff {
                            span.star { "\u{2605} " (format!("{:.2}", score.star_rating)) }
                            span.diff-name { (score.version) }
                        }
                        @if let Some(ref chip) = fav_chip {
                            (chip)
                        }
                        @if let Some(ref chip) = plays_chip {
                            (chip)
                        }
                    }
                    div.info-chips {
                        span.chip.chip-info {
                            span.chip-icon { "\u{266B}" }
                            span.chip-label { "BPM" }
                            span.chip-num { (format!("{:.0}", score.bpm)) }
                        }
                        span.chip.chip-info {
                            span.chip-icon { "\u{25F7}" }
                            span.chip-label { "Length" }
                            span.chip-num { (length) }
                        }
                        span.chip.chip-info {
                            span.chip-icon { "\u{270E}" }
                            span.chip-label { "Mapper" }
                            span.chip-num { (score.creator) }
                        }
                    }
                }
            }
            div.meta-card.surface {
                div.meta-line {
                    span.meta-label { "BID" }
                    span.meta-val.meta-val-big { (score.beatmap_id) }
                }
                div.meta-line {
                    span.meta-label { "Played" }
                    span.meta-val {
                        @if data.play_time.is_empty() {
                            "--"
                        } @else {
                            (data.play_time)
                        }
                    }
                }
            }
        }
    }
}

fn render_middle_row(data: &ScoreCardData) -> Markup {
    let score = &data.score;

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
    let ar_row = if no_ar_cs {
        html! {
            div.stat-row {
                span.label { "AR" }
                div.track {}
                span.val { span { "-" } }
            }
        }
    } else {
        stat_bar("AR", score.ar, data.ar_eff)
    };
    let od_row = if matches!(data.mode, osubot_types::GameMode::Catch) {
        html! {
            div.stat-row {
                span.label { "OD" }
                div.track {}
                span.val { span { "-" } }
            }
        }
    } else {
        stat_bar("OD", score.od, data.od_eff)
    };
    let cs_row = if no_ar_cs {
        html! {
            div.stat-row {
                span.label { "CS" }
                div.track {}
                span.val { span { "-" } }
            }
        }
    } else {
        stat_bar("CS", score.cs, data.cs_eff)
    };
    let hp_row = stat_bar("HP", score.hp, data.hp_eff);

    html! {
        div.middle-row {
            div.user-card.surface {
                div.user-avatar {
                    img src=(data.avatar_data_uri);
                }
                div.user-info-mid {
                    div.user-name { (data.username) }
                    div.user-ranks {
                        div.rank-item {
                            span.rank-hash { "#" }
                            span.rank-val { (global_rank) }
                            (PreEscaped(global_rank_change_html))
                            span.rank-label { "Global" }
                        }
                        div.rank-item {
                            span.rank-hash { "#" }
                            span.rank-val { (country_rank) }
                            (PreEscaped(country_rank_change_html))
                            span.rank-label { (data.country_code) }
                        }
                    }
                }
                div.user-pp-section {
                    span.user-pp-val { (pp_val_display) "pp" }
                    @if data.pp_change.is_some() {
                        (PreEscaped(pp_change_html))
                    }
                }
            }
            div.stats-card.surface {
                (ar_row)
                (od_row)
                (cs_row)
                (hp_row)
            }
        }
    }
}

fn render_score_row(data: &ScoreCardData) -> Markup {
    let score = &data.score;
    let is_mania = data.mode == osubot_types::GameMode::Mania;
    let is_taiko = data.mode == osubot_types::GameMode::Taiko;

    let hgeki = score.statistics.count_geki;
    let h300 = score.statistics.count_300;
    let hkatu = score.statistics.count_katu;
    let h100 = score.statistics.count_100;
    let h50 = score.statistics.count_50;
    let miss = score.statistics.count_miss;

    let score_formatted = if score.score_value > 0 {
        format_number(score.score_value)
    } else {
        "--".to_string()
    };

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

    html! {
        div.hits-card.surface {
            div.hits-row {
                @if is_mania {
                    div.hit-card.hit-300-mania {
                        div.num.mania-geki { (hgeki) "\u{00D7}" }
                        div.num.mania-300 { (h300) "\u{00D7}" }
                    }
                    div.hit-card.hit-katu {
                        div.num { (hkatu) "\u{00D7}" }
                        div.label { "200" }
                    }
                } @else {
                    div.hit-card.hit-300 {
                        div.num { (h300) "\u{00D7}" }
                        div.label { "300" }
                    }
                }
                @if !is_taiko {
                    div.hit-card.hit-100 {
                        div.num { (h100) "\u{00D7}" }
                        div.label { "100" }
                    }
                }
                @if is_taiko {
                    div.hit-card.hit-100 {
                        div.num { (h100) "\u{00D7}" }
                        div.label { "150" }
                    }
                }
                @if is_mania {
                    div.hit-card.hit-50-mania {
                        div.num.mania-50 { (h50) "\u{00D7}" }
                        div.num.mania-miss { (miss) "\u{00D7}" }
                    }
                } @else if is_taiko {
                    div.hit-card.hit-miss {
                        div.num { (miss) "\u{00D7}" }
                        div.label { "miss" }
                    }
                } @else {
                    div.hit-card.hit-50 {
                        div.num { (h50) "\u{00D7}" }
                        div.label { "50" }
                    }
                    div.hit-card.hit-miss {
                        div.num { (miss) "\u{00D7}" }
                        div.label { "miss" }
                    }
                }
            }
            div.score-acc-row {
                div class=(format!("rank-badge {}", rank_class)) {
                    (rank_display)
                }
                div.score-acc-stack {
                    div.stat-mod.stat-mod-score {
                        div.stat-val { (score_formatted) }
                        div.stat-label { "SCORE" }
                    }
                    div.stat-mod.stat-mod-acc {
                        div.stat-val { (format_accuracy(score.accuracy)) }
                        div.stat-label { "ACC" }
                    }
                }
            }
            div.mod-chips {
                @if score.is_lazer {
                    span.chip.chip-filled { "Lazer" }
                }
                @for m in &score.mods {
                    span.chip.chip-filled { (m.acronym()) }
                }
            }
        }
    }
}

fn render_detail_cards(data: &ScoreCardData) -> Markup {
    let score = &data.score;

    let pp_str = score
        .pp
        .map(|p| format!("{:.0}", p))
        .unwrap_or_else(|| "--".to_string());

    fn pp_breakdown_sum(b: &osubot_types::PpBreakdown) -> f64 {
        b.aim.filter(|&v| v > 0.0).unwrap_or(0.0)
            + b.speed.filter(|&v| v > 0.0).unwrap_or(0.0)
            + if b.accuracy > 0.0 { b.accuracy } else { 0.0 }
            + b.flashlight.filter(|&v| v > 0.0).unwrap_or(0.0)
            + b.difficulty.filter(|&v| v > 0.0).unwrap_or(0.0)
    }

    let has_breakdown = score
        .pp_breakdown
        .as_ref()
        .map(pp_breakdown_sum)
        .unwrap_or(0.0)
        > 0.0;
    let has_if_acc = score.pp_if_acc.is_some();

    let combo_pct =
        (score.max_combo as f64 / score.beatmap_max_combo.max(1) as f64 * 100.0).min(100.0);
    let combo_classes = if score.max_combo == score.beatmap_max_combo {
        "subcard-combo combo-fc"
    } else {
        "subcard-combo"
    };

    html! {
        div.detail-card.surface {
            div.detail-subcard-row {
                div.subcard-pp {
                    div.subcard-valwrap {
                        div.pp-val { (pp_str) span.pp-unit { "pp" } }
                    }
                    div.pp-label { "PERFORMANCE" }
                }
            }
            @if has_breakdown || has_if_acc {
                div.subcard-pp-predict {
                    @if let Some(ref breakdown) = score.pp_breakdown {
                        @if pp_breakdown_sum(breakdown) > 0.0 {
                            div.subcard-breakdown {
                                @if let Some(aim) = breakdown.aim.filter(|&v| v > 0.0) {
                                    span.chip.pp-chip-aim {
                                        span.chip-label { "AIM" }
                                        " " (format!("{:.0}", aim))
                                        span.chip-unit { "pp" }
                                    }
                                }
                                @if let Some(speed) = breakdown.speed.filter(|&v| v > 0.0) {
                                    span.chip.pp-chip-speed {
                                        span.chip-label { "SPD" }
                                        " " (format!("{:.0}", speed))
                                        span.chip-unit { "pp" }
                                    }
                                }
                                @if breakdown.accuracy > 0.0 {
                                    span.chip.pp-chip-acc {
                                        span.chip-label { "ACC" }
                                        " " (format!("{:.0}", breakdown.accuracy))
                                        span.chip-unit { "pp" }
                                    }
                                }
                                @if let Some(fl) = breakdown.flashlight.filter(|&v| v > 0.0) {
                                    span.chip.pp-chip-fl {
                                        span.chip-label { "FL" }
                                        " " (format!("{:.0}", fl))
                                        span.chip-unit { "pp" }
                                    }
                                }
                                @if let Some(diff) = breakdown.difficulty.filter(|&v| v > 0.0) {
                                    span.chip.pp-chip-diff {
                                        span.chip-label { "DIFF" }
                                        " " (format!("{:.0}", diff))
                                        span.chip-unit { "pp" }
                                    }
                                }
                            }
                        }
                    }
                    @if let Some(ref if_acc) = score.pp_if_acc {
                        div.subcard-if-acc {
                            @for (label, val) in [
                                ("95%", if_acc.acc_95),
                                ("97%", if_acc.acc_97),
                                ("98%", if_acc.acc_98),
                                ("99%", if_acc.acc_99),
                                ("100%", if_acc.acc_100),
                            ] {
                                div.if-acc-item {
                                    span.val { (format!("{:.0}", val)) span.val-unit { "pp" } }
                                    span.label { (label) }
                                }
                            }
                        }
                    }
                }
            }
            div.detail-subcard-footer-row {
                @if let Some(ref if_acc) = score.pp_if_acc {
                    div.subcard-if-fc {
                        span.fc-label { "IF FC" }
                        span.fc-val { (format!("{:.0}", if_acc.if_fc)) span.pp-unit { "pp" } }
                    }
                }
                div class=(combo_classes) style=(format!("--combo-pct: {combo_pct:.0}%")) {
                    span.fc-label { "COMBO" }
                    span.fc-val {
                        span.combo-cur { (score.max_combo) span.combo-unit { "x" } }
                        span.combo-sep { " / " }
                        span.combo-max { (score.beatmap_max_combo) span.combo-unit { "x" } }
                    }
                }
                @if let Some(ur) = data.ur_value {
                    div.subcard-ur {
                        span.ur-label { "UR" }
                        span.ur-val { (format!("{:.0}", ur)) }
                    }
                }
            }
        }
    }
}

pub fn wrap_score_html(data: &ScoreCardData) -> String {
    let css = SCORE_CSS
        .replace("{{SCORE_HUE}}", &data.hue.to_string())
        .replace("{{SCORE_SAT}}", &data.sat.to_string());

    html! {
        (PreEscaped("<!DOCTYPE html>"))
        html {
            head {
                style { (PreEscaped(css)) }
            }
            body {
                div.score-card {
                    img.bg-img src=(data.bg_data_uri);
                    div.bg-gradient {}
                    div.content {
                        (render_top_row(data))
                        (render_middle_row(data))
                        div.score-row {
                            (render_score_row(data))
                            (render_detail_cards(data))
                        }
                    }
                }
            }
        }
    }
    .into_string()
}

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
}
