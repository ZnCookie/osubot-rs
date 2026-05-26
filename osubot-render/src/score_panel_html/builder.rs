use crate::pp_calc::PpBreakdown;
use osubot_core::api::{BeatmapInfo, BeatmapSetInfo, RecentPlay};

pub fn build_score_panel_html(
    score: &RecentPlay,
    username: &str,
    avatar_url: &str,
    flag_emoji: &str,
    global_rank: i64,
    country_rank: i64,
    user_pp: Option<f64>,
    mode: &str,
    beatmap: &BeatmapInfo,
    beatmapset: Option<&BeatmapSetInfo>,
    cover_data_uri: Option<&str>,
    pp_breakdown: &Option<PpBreakdown>,
) -> String {
    let cover_html = match cover_data_uri {
        Some(uri) => format!(r#"<img class="score-panel-bg" src="{}">"#, uri),
        None => String::new(),
    };

    let (title, artist, creator) = match beatmapset {
        Some(set) => (
            set.title.as_str(),
            set.artist.as_str(),
            set.creator.as_str(),
        ),
        None => ("Unknown", "Unknown", "Unknown"),
    };

    let rank_color = grade_color(&score.rank);
    let formatted_score = format_score(score.score);
    let formatted_time = format_time(beatmap.total_length);
    let accuracy_pct = format!("{:.2}", score.accuracy * 100.0);
    let pp_value = score
        .pp
        .map(|p| (p as i64).to_string())
        .unwrap_or_else(|| "—".to_string());
    let fc_pp = pp_breakdown
        .as_ref()
        .map(|b| (b.full_pp as i64).to_string())
        .unwrap_or_else(|| "—".to_string());
    let mod_html = build_mod_html(&score.mods);
    let breakdown_html = build_pp_breakdown_html(pp_breakdown);
    let hit_bar_html = build_hit_bar_html(
        score.statistics.count_300,
        score.statistics.count_100,
        score.statistics.count_50,
        score.statistics.count_miss,
    );
    let max_combo_str = score
        .max_combo
        .map(|c| c.to_string())
        .unwrap_or_else(|| "0".to_string());
    let stars = format!("{:.2}", beatmap.difficulty_rating);
    let cs = format!("{:.1}", beatmap.cs);
    let ar = format!("{:.1}", beatmap.ar);
    let od = format!("{:.1}", beatmap.od);

    let totalpp_html = match user_pp {
        Some(pp) => format!(
            r#"<div class="panel-totalpp">{}pp</div>"#,
            pp.round() as i64
        ),
        None => String::new(),
    };

    format!(
        r#"<div class="score-panel">
  {cover_html}
  <div class="score-panel-overlay"></div>
  <div class="score-panel-content">
    <div class="score-panel-spacer"></div>
    <div class="score-panel-card">
      <div class="panel-left">
        <img class="panel-avatar" src="{avatar_url}">
        <div class="panel-username"><span class="panel-flag">{flag}</span> {username}</div>
        <div class="panel-ranks"><span class="rank-global">#{global_rank}</span> · <span class="rank-country">#{country_rank}</span></div>
        {totalpp_html}
      </div>
      <div class="panel-center">
        <div class="panel-title">{title}</div>
        <div class="panel-artist">{artist}</div>
        <div class="panel-meta"><span class="panel-stars">{stars}★</span> · <span class="panel-version">{version}</span> · <span class="panel-creator">mapped by {creator}</span></div>
        <div class="panel-pp-row">
          <div><div class="panel-pp-label">PP</div><div class="panel-pp-value">{pp_value}</div></div>
          <div><div class="panel-pp-label">FC</div><div class="panel-pp-fc">{fc_pp}pp</div></div>
          {breakdown_html}
        </div>
        <div><div class="panel-score-label">SCORE</div><div class="panel-score-value">{formatted_score}</div></div>
        <div class="panel-hits">
          <span class="hit-label">HITS</span>
          <span class="hit-great">{count_300}</span> · <span class="hit-ok">{count_100}</span>
          <span class="hit-meh">{count_50}</span> · <span class="hit-miss">{count_miss}</span>
        </div>
        {hit_bar_html}
        <div class="panel-hits-stats">
          <span><span class="stat-label">Combo</span> <strong>x{max_combo_str}</strong></span>
          <span><span class="stat-label">Acc</span> <strong>{accuracy_pct}%</strong></span>
        </div>
      </div>
      <div class="panel-right">
        <div class="panel-grade" style="background: linear-gradient(135deg, {rank_color}, {rank_color}cc);">{rank}</div>
        {mod_html}
        <div class="panel-beatmap-info">{bpm}bpm · {formatted_time} · {cs}cs/{ar}ar/{od}od · <span class="mode">{mode}</span> · <span class="status">{status}</span></div>
      </div>
    </div>
    <div class="score-panel-spacer"></div>
  </div>
</div>"#,
        cover_html = cover_html,
        avatar_url = avatar_url,
        flag = flag_emoji,
        username = username,
        global_rank = global_rank,
        country_rank = country_rank,
        totalpp_html = totalpp_html,
        title = title,
        artist = artist,
        stars = stars,
        version = beatmap.version,
        creator = creator,
        pp_value = pp_value,
        fc_pp = fc_pp,
        breakdown_html = breakdown_html,
        formatted_score = formatted_score,
        count_300 = score.statistics.count_300,
        count_100 = score.statistics.count_100,
        count_50 = score.statistics.count_50,
        count_miss = score.statistics.count_miss,
        hit_bar_html = hit_bar_html,
        max_combo_str = max_combo_str,
        accuracy_pct = accuracy_pct,
        rank = score.rank,
        rank_color = rank_color,
        mod_html = mod_html,
        bpm = beatmap.bpm.round() as i64,
        formatted_time = formatted_time,
        cs = cs,
        ar = ar,
        od = od,
        mode = mode,
        status = beatmap.status,
    )
}

fn format_time(seconds: i64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn grade_color(rank: &str) -> &str {
    match rank {
        "X" | "XH" => "#ffd700",
        "S" | "SH" => "#ffd700",
        "A" => "#5f5",
        "B" => "#5cf",
        "C" => "#f5f",
        _ => "#f55",
    }
}

fn build_mod_html(mods: &[String]) -> String {
    if mods.is_empty() {
        return String::new();
    }
    let max_visible = 3;
    let visible_mods = if mods.len() > max_visible {
        &mods[..max_visible]
    } else {
        mods
    };
    let mut html = String::from(r#"<div class="panel-mods">"#);
    for m in visible_mods {
        html.push_str(&format!(r#"<span class="panel-mod">{}</span>"#, m));
    }
    if mods.len() > max_visible {
        html.push_str(&format!(
            r#"<span class="panel-mods-overflow">+{}</span>"#,
            mods.len() - max_visible
        ));
    }
    html.push_str("</div>");
    html
}

fn build_pp_breakdown_html(pp_breakdown: &Option<PpBreakdown>) -> String {
    let bd = match pp_breakdown {
        Some(b) => b,
        None => return String::new(),
    };

    let aim_pp = bd.aim.round() as i64;
    let speed_pp = bd.speed.round() as i64;
    let acc_pp = bd.accuracy.round() as i64;
    let fl_pp = bd.flashlight.round() as i64;

    let total = aim_pp + speed_pp + acc_pp + fl_pp;
    if total == 0 {
        return String::new();
    }

    let components = [
        (aim_pp, "AIM", "panel-breakdown-aim", "#5cf"),
        (speed_pp, "SPD", "panel-breakdown-spd", "#5f5"),
        (acc_pp, "ACC", "panel-breakdown-acc", "#f55"),
        (fl_pp, "FL", "panel-breakdown-fl", "#f5f"),
    ];

    let mut labels = Vec::new();
    let mut bars: Vec<(i64, &str)> = Vec::new();

    for &(pp_val, label, class, color) in &components {
        let pct = (pp_val as f64 / total as f64 * 100.0).round() as i64;
        if pp_val > 0 {
            labels.push(format!("<span class=\"{class}\">{label} {pp_val}</span>"));
            bars.push((pct, color));
        }
    }

    if bars.is_empty() {
        return String::new();
    }

    let labels_html = labels.join(" ");

    let mut bar_html = String::from(r#"<div class="panel-breakdown-bar">"#);
    for (i, &(pct, color)) in bars.iter().enumerate() {
        let radius = if bars.len() == 1 {
            "4px"
        } else if i == 0 {
            "4px 0 0 4px"
        } else if i == bars.len() - 1 {
            "0 4px 4px 0"
        } else {
            "0"
        };
        bar_html.push_str(&format!(
            r#"<div style="height:100%;flex:{pct};background:{color};border-radius:{radius}"></div>"#,
        ));
    }
    bar_html.push_str("</div>");

    format!(
        r#"<div class="panel-breakdown">
  <div class="panel-breakdown-labels">{labels}</div>
  {bar_html}
</div>"#,
        labels = labels_html,
        bar_html = bar_html
    )
}

fn build_hit_bar_html(count_300: i64, count_100: i64, count_50: i64, count_miss: i64) -> String {
    let total = (count_300 + count_100 + count_50 + count_miss) as f64;
    if total == 0.0 {
        return String::new();
    }

    let mut segments: Vec<(i64, &str)> = Vec::new();

    if count_300 > 0 {
        segments.push(((count_300 as f64 / total * 100.0).round() as i64, "#5cf"));
    }
    if count_100 > 0 {
        segments.push(((count_100 as f64 / total * 100.0).round() as i64, "#5f5"));
    }
    if count_50 > 0 {
        segments.push(((count_50 as f64 / total * 100.0).round() as i64, "#f55"));
    }
    if count_miss > 0 {
        segments.push(((count_miss as f64 / total * 100.0).round() as i64, "#888"));
    }

    if segments.is_empty() {
        return String::new();
    }

    let mut html = String::from(r#"<div class="panel-hits-bar">"#);
    for (i, &(pct, color)) in segments.iter().enumerate() {
        let pct = pct.max(1);
        let radius = if segments.len() == 1 {
            "4px"
        } else if i == 0 {
            "4px 0 0 4px"
        } else if i == segments.len() - 1 {
            "0 4px 4px 0"
        } else {
            "0"
        };
        html.push_str(&format!(
            r#"<div style="height:100%;flex:{pct};background:{color};border-radius:{radius}"></div>"#,
        ));
    }
    html.push_str("</div>");
    html
}

fn format_score(score: i64) -> String {
    let s = score.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pp_calc::PpBreakdown;
    use osubot_core::api::{
        BeatmapCovers, BeatmapInfo, BeatmapRaw, BeatmapSetInfo, PlayStatistics, RecentPlay,
    };

    #[test]
    fn test_build_html_contains_title() {
        let play = RecentPlay {
            id: 123,
            beatmap: BeatmapRaw {
                id: 456,
                version: "Another".to_string(),
                difficulty_rating: 7.5,
                cs: 4.0,
                ar: 9.5,
                od: 8.8,
                hp: 5.0,
                bpm: Some(180.0),
                total_length: 145,
                max_combo: Some(1234),
                status: "ranked".to_string(),
            },
            beatmapset: Some(BeatmapSetInfo {
                id: 789,
                artist: "IU".to_string(),
                title: "COLORFUL DAYS!!".to_string(),
                creator: "Niva".to_string(),
                covers: BeatmapCovers {
                    cover: "https://example.com/cover.jpg".to_string(),
                    cover_2x: "".to_string(),
                },
            }),
            score: 999999,
            accuracy: 0.9775,
            mods: vec!["HD".to_string(), "DT".to_string()],
            rank: "S".to_string(),
            pp: Some(277.5),
            statistics: PlayStatistics {
                count_300: 800,
                count_100: 50,
                count_50: 5,
                count_miss: 2,
            },
            max_combo: Some(1050),
            perfect: false,
            passed: true,
            created_at: "2026-01-01".to_string(),
        };

        let beatmap_info: BeatmapInfo = play.beatmap.clone().into();
        let breakdown = PpBreakdown {
            aim: 100.0,
            speed: 80.0,
            accuracy: 50.0,
            flashlight: 20.0,
            effective_miss_count: 0,
            full_pp: 277.5,
            perfect_pp: 310.0,
        };

        let html = build_score_panel_html(
            &play,
            "ZnCookie",
            "https://a.ppy.sh/18230719",
            "🇨🇳",
            12345,
            678,
            Some(5678.0),
            "osu",
            &beatmap_info,
            play.beatmapset.as_ref(),
            None,
            &Some(breakdown),
        );

        assert!(
            html.contains("COLORFUL DAYS!!"),
            "Should contain beatmap title"
        );
        assert!(html.contains("ZnCookie"), "Should contain username");
        assert!(html.contains("277"), "Should contain PP value");
        assert!(html.contains("S"), "Should contain rank");
        assert!(html.contains("HD"), "Should contain mod HD");
        assert!(html.contains("DT"), "Should contain mod DT");
        assert!(html.contains("97.75"), "Should contain accuracy");
        assert!(html.contains("999,999"), "Should contain formatted score");
        assert!(html.contains("AIM"), "Should contain PP breakdown label");
    }
}
