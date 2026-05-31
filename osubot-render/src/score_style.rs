use osubot_types::{format_accuracy, format_length, format_number, Score};

use crate::style::escape_html;

const SCORE_CSS: &str = include_str!("../styles/score.css");

pub struct ScoreCardData {
    pub score: Score,
    pub username: String,
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
    pub pp_change: Option<i64>,
    pub global_rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
    pub ranked_status: String,
    pub ur_value: Option<f64>,
    pub ar_eff: Option<f64>,
    pub od_eff: Option<f64>,
    pub cs_eff: Option<f64>,
    pub hp_eff: Option<f64>,
}

const BPM_ICON: &str = "♫";
const LENGTH_ICON: &str = "◷";

pub fn wrap_score_html(data: &ScoreCardData) -> String {
    let css = SCORE_CSS
        .replace("{{SCORE_HUE}}", &data.hue.to_string())
        .replace("{{SCORE_SAT}}", &data.sat.to_string());

    let score = &data.score;

    let h300 = score.statistics.count_300;
    let h100 = score.statistics.count_100;
    let h50 = score.statistics.count_50;
    let miss = score.statistics.count_miss;

    let mods_html = if score.mods.is_empty() {
        String::new()
    } else {
        score
            .mods
            .iter()
            .map(|m| {
                format!(
                    r#"<span class="chip chip-filled">{}</span>"#,
                    escape_html(m.acronym().as_str())
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };

    let pp_str = score
        .pp
        .map(|p| format!("{:.0}", p))
        .unwrap_or_else(|| "--".to_string());

    let global_rank = data
        .user_global_rank
        .map(format_number)
        .unwrap_or_else(|| "-".to_string());
    let country_rank = data
        .user_country_rank
        .map(format_number)
        .unwrap_or_else(|| "-".to_string());

    let length = format_length(score.length_seconds);
    let score_formatted = if score.score_value > 0 {
        format_number(score.score_value)
    } else {
        "--".to_string()
    };

    fn stat_bar(label: &str, base: f64, eff: Option<f64>) -> (String, String) {
        let base_pct = (base / 10.0 * 100.0).min(100.0);
        let (track_html, val_str) = match eff {
            Some(e) if (e - base).abs() < 0.01 => {
                let track = format!(r#"<div class="fill" style="width:{base_pct:.0}%"></div>"#);
                (track, format!("{:.1}", base))
            }
            Some(e) if e > base => {
                let eff_pct = (e / 10.0 * 100.0).min(100.0);
                let overflow_pct = eff_pct - base_pct;
                let track = format!(
                    r#"<div class="fill" style="width:{base_pct:.0}%"></div><div class="fill-over" style="left:calc({base_pct:.0}% - 2px); width:calc({overflow_pct:.0}% + 2px)"></div>"#
                );
                (track, format!("{:.1}[{:.1}]", e, base))
            }
            Some(e) => {
                let eff_pct = (e / 10.0 * 100.0).min(100.0);
                let under_pct = base_pct - eff_pct;
                let track = format!(
                    r#"<div class="fill" style="width:{eff_pct:.0}%"></div><div class="fill-under" style="left:calc({eff_pct:.0}% - 2px); width:calc({under_pct:.0}% + 2px)"></div>"#
                );
                (track, format!("{:.1}[{:.1}]", e, base))
            }
            None => {
                let track = format!(r#"<div class="fill" style="width:{base_pct:.0}%"></div>"#);
                (track, format!("{:.1}", base))
            }
        };
        let row = format!(
            r#"<div class="stat-row"><span class="label">{label}</span><div class="track">{track_html}</div><span class="val">{val_str}</span></div>"#
        );
        (row, val_str)
    }

    let fav_chip = data
        .fav_count
        .map(|c| {
            format!(
                r#"<span class="chip chip-fav"><span class="chip-icon">♥</span><span class="chip-num">{}</span></span>"#,
                format_number(c)
            )
        })
        .unwrap_or_default();
    let plays_chip = data
        .play_count
        .map(|c| {
            format!(
                r#"<span class="chip chip-plays"><span class="chip-icon"></span><span class="chip-num">{}</span></span>"#,
                format_plays(c)
            )
        })
        .unwrap_or_default();

    let pp_change_html = match data.pp_change {
        Some(delta) if delta > 0 => {
            format!(r#"<span class="user-pp-change up">+{delta}</span>"#)
        }
        Some(delta) if delta < 0 => {
            format!(r#"<span class="user-pp-change down">{delta}</span>"#)
        }
        Some(0) => r#"<span class="user-pp-change zero">±0</span>"#.to_string(),
        _ => String::new(),
    };

    let global_rank_change_html = match data.global_rank_change {
        Some(delta) if delta > 0 => {
            format!(r#"<span class="rank-change up">+{delta}</span>"#)
        }
        Some(delta) if delta < 0 => {
            format!(r#"<span class="rank-change down">{delta}</span>"#)
        }
        Some(0) => r#"<span class="rank-change zero">±0</span>"#.to_string(),
        _ => String::new(),
    };
    let country_rank_change_html = match data.country_rank_change {
        Some(delta) if delta > 0 => {
            format!(r#"<span class="rank-change up">+{delta}</span>"#)
        }
        Some(delta) if delta < 0 => {
            format!(r#"<span class="rank-change down">{delta}</span>"#)
        }
        Some(0) => r#"<span class="rank-change zero">±0</span>"#.to_string(),
        _ => String::new(),
    };

    let combo_pct = if score.beatmap_max_combo > 0 {
        (score.max_combo as f64 / score.beatmap_max_combo as f64 * 100.0).min(100.0)
    } else {
        0.0
    };

    let pp_val_display = format!("{:.0}", data.user_pp);
    let user_pp_change_section = if data.pp_change.is_some() {
        pp_change_html
    } else {
        String::new()
    };

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

    let mut html = String::with_capacity(16384);
    html.push_str(
        r#"<!DOCTYPE html>
<html><head><style>"#,
    );
    html.push_str(&css);
    html.push_str(
        r#"</style></head><body>
<div class="score-card">
  <img class="bg-img" src=""#,
    );
    html.push_str(&data.bg_data_uri);
    html.push_str(
        r#"" />
  <div class="bg-gradient"></div>
  <div class="content">
    <div class="top-row">
      <div class="beatmap-card surface">
        <div class="cover-wrap"><img src=""#,
    );
    html.push_str(&data.thumb_data_uri);
    html.push_str(
        r#"" /></div>
        <div class="beatmap-text">
          <div class="beatmap-title">"#,
    );
    html.push_str(&escape_html(&score.title));
    html.push_str(
        r#"</div>
          <div class="beatmap-artist">"#,
    );
    html.push_str(&escape_html(&score.artist));
    html.push_str(
        r#"</div>
          <div class="bottom-chips">
            <span class="chip chip-status "#,
    );
    html.push_str(status_class);
    html.push_str(r#"">"#);
    html.push_str(&escape_html(&status_display));
    html.push_str(
        r#"</span>
            <span class="chip chip-diff"><span class="star">★ "#,
    );
    html.push_str(&format!("{:.2}", score.star_rating));
    html.push_str(r#"</span><span class="diff-name">"#);
    html.push_str(&escape_html(&score.version));
    html.push_str(
        r#"</span></span>
            "#,
    );
    html.push_str(&fav_chip);
    html.push_str(&plays_chip);
    html.push_str(
        r#"
          </div>
        </div>
        <div class="info-modules">
          <div class="info-mod surface"><span class="mod-icon">"#,
    );
    html.push_str(BPM_ICON);
    html.push_str(r#"</span><span class="mod-val">"#);
    html.push_str(&format!("{:.0}", score.bpm));
    html.push_str(
        r#"</span><span class="mod-label">BPM</span></div>
          <div class="info-mod surface"><span class="mod-icon">"#,
    );
    html.push_str(LENGTH_ICON);
    html.push_str(r#"</span><span class="mod-val">"#);
    html.push_str(&length);
    html.push_str(
        r#"</span><span class="mod-label">Length</span></div>
        </div>
      </div>
      <div class="meta-card surface">
        <div class="meta-line"><span class="meta-label">Mapper</span><span class="meta-val">"#,
    );
    html.push_str(&escape_html(&score.creator));
    html.push_str(
        r#"</span></div>
        <div class="meta-line"><span class="meta-label">BID</span><span class="meta-val">"#,
    );
    html.push_str(&score.beatmap_id.to_string());
    html.push_str(r#"</span></div>"#);
    let play_display = if data.play_time.is_empty() {
        "--"
    } else {
        &data.play_time
    };
    html.push_str(
        r#"<div class="meta-line"><span class="meta-label">Played</span><span class="meta-val">"#,
    );
    html.push_str(play_display);
    html.push_str(
        r#"</span></div>
      </div>
    </div>

    <div class="middle-row">
      <div class="user-card surface">
        <div class="user-avatar"><img src=""#,
    );
    html.push_str(&data.avatar_data_uri);
    html.push_str(
        r#"" /></div>
        <div class="user-info-mid">
          <div class="user-name">"#,
    );
    html.push_str(&escape_html(&data.username));
    html.push_str(
        r#"</div>
          <div class="user-ranks">
            <div class="rank-item"><span class="rank-hash">#</span><span class="rank-val">"#,
    );
    html.push_str(&global_rank);
    html.push_str(r#"</span>"#);
    html.push_str(&global_rank_change_html);
    html.push_str(
        r#"<span class="rank-label">Global</span></div>
            <div class="rank-item"><span class="rank-hash">#</span><span class="rank-val">"#,
    );
    html.push_str(&country_rank);
    html.push_str(r#"</span>"#);
    html.push_str(&country_rank_change_html);
    html.push_str(r#"<span class="rank-label">"#);
    html.push_str(&escape_html(&data.country_code));
    html.push_str(
        r#"</span></div>
          </div>
        </div>
        <div class="user-pp-section">
          <span class="user-pp-val">"#,
    );
    html.push_str(&pp_val_display);
    html.push_str(
        r#"pp</span>
          "#,
    );
    html.push_str(&user_pp_change_section);
    html.push_str(
        r#"
        </div>
      </div>
      <div class="stats-card surface">"#,
    );

    let (ar_row, _) = stat_bar("AR", score.ar, data.ar_eff);
    html.push_str(&ar_row);
    let (od_row, _) = stat_bar("OD", score.od, data.od_eff);
    html.push_str(&od_row);
    let (cs_row, _) = stat_bar("CS", score.cs, data.cs_eff);
    html.push_str(&cs_row);
    let (hp_row, _) = stat_bar("HP", score.hp, data.hp_eff);
    html.push_str(&hp_row);

    html.push_str(
        r#"</div>
    </div>

    <div class="score-row">
      <div class="hits-card surface">
        <div class="hits-row">
          <div class="hit-card hit-300"><div class="num">"#,
    );
    html.push_str(&h300.to_string());
    html.push_str(
        r#"×</div><div class="label">300</div></div>
          <div class="hit-card hit-100"><div class="num">"#,
    );
    html.push_str(&h100.to_string());
    html.push_str(
        r#"×</div><div class="label">100</div></div>
          <div class="hit-card hit-50"><div class="num">"#,
    );
    html.push_str(&h50.to_string());
    html.push_str(
        r#"×</div><div class="label">50</div></div>
          <div class="hit-card hit-miss"><div class="num">"#,
    );
    html.push_str(&miss.to_string());
    html.push_str(
        r#"×</div><div class="label">miss</div></div>
        </div>
        <div class="score-acc-row">
          <div class="rank-badge "#,
    );
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
    html.push_str(rank_class);
    html.push_str(r#"">"#);
    let rank_display = match score.rank.as_str() {
        "XH" | "X" => "X",
        "SH" | "S" => "S",
        other => other,
    };
    html.push_str(rank_display);
    html.push_str(
        r#"</div>
          <div class="score-acc-stack">
            <div class="stat-mod stat-mod-score"><div class="stat-val">"#,
    );
    html.push_str(&score_formatted);
    html.push_str(
        r#"</div><div class="stat-label">SCORE</div></div>
            <div class="stat-mod stat-mod-acc"><div class="stat-val">"#,
    );
    html.push_str(&format_accuracy(score.accuracy));
    html.push_str(
        r#"</div><div class="stat-label">ACC</div></div>
          </div>
        </div>
        <div class="mod-chips">"#,
    );
    html.push_str(&mods_html);

    html.push_str(
        r#"</div>
      </div>
      <div class="detail-card surface">"#,
    );

    html.push_str(
        r#"<div class="detail-middle">
          <div class="score-pp-row"><div class="score-pp-big">"#,
    );
    html.push_str(&pp_str);
    html.push_str(r#"pp</div>"#);

    let is_fc = score.max_combo == score.beatmap_max_combo;
    let combo_color = if is_fc { "#c0f0c8" } else { "#938f99" };
    html.push_str(&format!(
        r#"<span class="combo-group" style="--combo-pct:{:.1}%;--combo-color:{}"><span class="combo-current">"#,
        combo_pct, combo_color
    ));
    html.push_str(&score.max_combo.to_string());
    html.push_str(r#"×</span><span class="combo-divider">/</span><span class="combo-total">"#);
    html.push_str(&score.beatmap_max_combo.to_string());
    html.push_str(r#"×</span></span></div>"#);

    if let Some(ref breakdown) = score.pp_breakdown {
        let total = breakdown.aim.filter(|&v| v > 0.0).unwrap_or(0.0)
            + breakdown.speed.filter(|&v| v > 0.0).unwrap_or(0.0)
            + if breakdown.accuracy > 0.0 {
                breakdown.accuracy
            } else {
                0.0
            }
            + breakdown.flashlight.filter(|&v| v > 0.0).unwrap_or(0.0)
            + breakdown.difficulty.filter(|&v| v > 0.0).unwrap_or(0.0);
        if total > 0.0 {
            html.push_str(r#"<div class="pp-breakdown"><div class="pp-labels">"#);
            if let Some(aim) = breakdown.aim.filter(|&v| v > 0.0) {
                html.push_str(&format!(
                    r#"<span class="chip pp-chip-aim">AIM {:.0}</span>"#,
                    aim
                ));
            }
            if let Some(speed) = breakdown.speed.filter(|&v| v > 0.0) {
                html.push_str(&format!(
                    r#"<span class="chip pp-chip-speed">SPD {:.0}</span>"#,
                    speed
                ));
            }
            if breakdown.accuracy > 0.0 {
                html.push_str(&format!(
                    r#"<span class="chip pp-chip-acc">ACC {:.0}</span>"#,
                    breakdown.accuracy
                ));
            }
            if let Some(fl) = breakdown.flashlight.filter(|&v| v > 0.0) {
                html.push_str(&format!(
                    r#"<span class="chip pp-chip-fl">FL {:.0}</span>"#,
                    fl
                ));
            }
            if let Some(diff) = breakdown.difficulty.filter(|&v| v > 0.0) {
                html.push_str(&format!(
                    r#"<span class="chip pp-chip-diff">DIFF {:.0}</span>"#,
                    diff
                ));
            }
            html.push_str(r#"</div></div>"#);
        }
    }

    if let Some(ref if_acc) = score.pp_if_acc {
        html.push_str(r#"<div class="if-acc">"#);
        for (label, val) in [
            ("95%", if_acc.acc_95),
            ("97%", if_acc.acc_97),
            ("98%", if_acc.acc_98),
            ("99%", if_acc.acc_99),
            ("100%", if_acc.acc_100),
        ] {
            html.push_str(&format!(
                r#"<div class="if-acc-item"><span class="val">{:.0}</span><span class="label">{}</span></div>"#,
                val, label
            ));
        }
        html.push_str(r#"</div>"#);
    }

    html.push_str(
        r#"</div>
        <div class="detail-bottom">"#,
    );

    if let Some(ref if_acc) = score.pp_if_acc {
        html.push_str(&format!(
            r#"<span class="if-fc-line">IF FC: {:.0}pp</span>"#,
            if_acc.if_fc
        ));
    } else {
        html.push_str(r#"<span></span>"#);
    }

    let ur_html = data
        .ur_value
        .map(|v| format!(r#"<span class="ur-value">UR: {:.0}</span>"#, v))
        .unwrap_or_default();
    html.push_str(&ur_html);

    html.push_str(
        r#"</div>
      </div>
    </div>
  </div>
</div>
</body></html>"#,
    );

    html
}

fn format_plays(val: i64) -> String {
    if val >= 1_000_000 {
        format!("{:.1}M", val as f64 / 1_000_000.0)
    } else if val >= 1_000 {
        format!("{:.1}K", val as f64 / 1_000.0)
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
            max_combo: 1121,
            beatmap_max_combo: 1500,
            pp: Some(456.7),
            pp_breakdown: None,
            pp_if_acc: None,
            rank: "S".to_string(),
            mods,
            is_perfect: false,
            created_at: "2025-05-27T14:30:22Z".to_string(),
            is_lazer: false,
            has_replay: true,
            legacy_score_id: None,
            statistics: ScoreStatistics {
                count_300: 856,
                count_100: 45,
                count_50: 12,
                count_miss: 2,
            },
            cover_url: "https://example.com/cover.jpg".to_string(),
            user: osubot_types::ScoreUser {
                avatar_url: "https://example.com/avatar.jpg".to_string(),
                country_code: "CN".to_string(),
                global_rank: Some(1234),
                country_rank: Some(56),
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
            user_pp: 9876.5,
            user_global_rank: Some(1234),
            user_country_rank: Some(56),
            country_code: "CN".to_string(),
            avatar_data_uri: "data:image/jpeg;base64,avatar".to_string(),
            bg_data_uri: "data:image/jpeg;base64,bg".to_string(),
            thumb_data_uri: "data:image/jpeg;base64,thumb".to_string(),
            play_time: "2025/05/27 14:30:22".to_string(),
            hue: 200,
            sat: 60,
            fav_count: Some(1234),
            play_count: Some(56700),
            pp_change: Some(12),
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

        // Top row
        assert!(html.contains("TestTitle"));
        assert!(html.contains("TestArtist"));
        assert!(html.contains("★ 6.50"));
        assert!(html.contains("Expert"));
        // Middle row
        assert!(html.contains("TestPlayer"));
        assert!(html.contains(">1,234<"));
        assert!(html.contains(">56<"));
        assert!(html.contains("9876pp"));
        assert!(html.contains(">+12<"));
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
        assert!(html.contains("combo-group"), "combo-group class missing");
        assert!(
            html.contains("combo-current"),
            "combo-current class missing"
        );
        assert!(html.contains("1121×"), "combo value missing");
        assert!(html.contains("1500×"), "combo total missing");
        assert!(
            !html.contains("combo-bar-track"),
            "combo bar should be removed"
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
        });
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            user_pp: 9876.5,
            user_global_rank: Some(1234),
            user_country_rank: Some(56),
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
            html.contains(r#"class="pp-breakdown""#),
            "pp-breakdown div missing"
        );
        assert!(html.contains("pp-chip-aim"), "aim chip missing");
        assert!(html.contains("pp-chip-speed"), "speed chip missing");
        assert!(html.contains("pp-chip-acc"), "acc chip missing");
        assert!(html.contains("pp-chip-fl"), "fl chip missing");
        assert!(html.contains("AIM 180"), "AIM label missing");
        assert!(html.contains("SPD 95"), "SPD label missing");
        assert!(html.contains("ACC 42"), "ACC label missing");
        assert!(html.contains("FL 10"), "FL label missing");
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
        });
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            user_pp: 9876.5,
            user_global_rank: Some(1234),
            user_country_rank: Some(56),
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
            html.contains(r#"class="pp-breakdown""#),
            "pp-breakdown div missing"
        );
        assert!(html.contains("pp-chip-diff"), "diff chip missing");
        assert!(html.contains("DIFF 200"), "DIFF label missing");
        assert!(html.contains("ACC 80"), "ACC label missing");
        assert!(!html.contains("AIM"), "AIM should not appear for taiko");
    }

    #[test]
    fn test_pp_breakdown_none() {
        let score = make_test_score();
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            user_pp: 9876.5,
            user_global_rank: Some(1234),
            user_country_rank: Some(56),
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
            !html.contains(r#"class="pp-breakdown""#),
            "pp-breakdown div should not appear when None"
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
        });
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            user_pp: 9876.5,
            user_global_rank: Some(1234),
            user_country_rank: Some(56),
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
        assert!(html.contains(r#"class="if-acc""#), "if-acc div missing");
        assert!(html.contains(">320<"), "95% PP value missing");
        assert!(html.contains(">380<"), "97% PP value missing");
        assert!(html.contains(">410<"), "98% PP value missing");
        assert!(html.contains(">440<"), "99% PP value missing");
        assert!(html.contains(">480<"), "100% PP value missing");
        assert!(html.contains("IF FC: 520pp"), "IF FC line missing");
        assert!(html.contains("if-acc-item"), "if-acc-item class missing");
    }

    #[test]
    fn test_if_acc_card_none() {
        let score = make_test_score();
        let data = ScoreCardData {
            score,
            username: "TestPlayer".to_string(),
            user_pp: 9876.5,
            user_global_rank: Some(1234),
            user_country_rank: Some(56),
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
            !html.contains(r#"class="if-acc""#),
            "if-acc should not appear when None"
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
                rank: "A".to_string(),
                mods,
                is_perfect: false,
                created_at: "2025-01-01T00:00:00Z".to_string(),
                is_lazer: false,
                has_replay: true,
                legacy_score_id: None,
                statistics: ScoreStatistics {
                    count_300: 400,
                    count_100: 50,
                    count_50: 10,
                    count_miss: 5,
                },
                cover_url: String::new(),
                user: ScoreUser {
                    avatar_url: String::new(),
                    country_code: "CN".to_string(),
                    global_rank: Some(1000),
                    country_rank: Some(50),
                    pp: 5000.0,
                },
                fav_count: None,
                play_count: None,
                status: "ranked".to_string(),
            },
            username: "TestUser".to_string(),
            user_pp: 5000.0,
            user_global_rank: Some(1000),
            user_country_rank: Some(50),
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
