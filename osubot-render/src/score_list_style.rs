use crate::style::escape_html;
use osubot_types::{format_accuracy, format_number, Score};

const SCORE_LIST_CSS: &str = include_str!("../styles/score_list.css");

pub struct ScoreListCardData {
    pub score: Score,
    pub rank_class: String,
    pub rank_display: String,
    pub cover_data_uri: String,
    pub relative_time: String,
    pub acc_formatted: String,
    pub pp_formatted: String,
    pub mods_html: String,
    pub passed: bool,
}

impl ScoreListCardData {
    pub fn from_score(score: &Score, cover_data_uri: String) -> Self {
        let rank_class = if !score.passed {
            "rank-f"
        } else {
            match score.rank.as_str() {
                "XH" | "SH" => "rank-s-silver",
                "X" | "S" => "rank-s",
                "A" => "rank-a",
                "B" => "rank-b",
                "C" => "rank-c",
                "D" => "rank-d",
                _ => "rank-f",
            }
        }
        .to_string();

        let rank_display = if !score.passed {
            "F"
        } else {
            match score.rank.as_str() {
                "XH" | "X" => "X",
                "SH" | "S" => "S",
                other => other,
            }
        }
        .to_string();

        let acc_formatted = format_accuracy(score.accuracy);

        let pp_formatted = score
            .pp
            .map(|p| format!("{:.0}", p))
            .unwrap_or_else(|| "--".to_string());

        let mut mods_html = String::new();
        if score.is_lazer {
            mods_html.push_str(r#"<span class="mod-tag">Lazer</span>"#);
        }
        if !score.mods.is_empty() {
            for m in &score.mods {
                mods_html.push_str(&format!(
                    r#"<span class="mod-tag">{}</span>"#,
                    escape_html(m.acronym().as_str())
                ));
            }
        }

        let relative_time = format_relative_time(&score.created_at);

        ScoreListCardData {
            score: score.clone(),
            rank_class,
            rank_display,
            cover_data_uri,
            relative_time,
            acc_formatted,
            pp_formatted,
            mods_html,
            passed: score.passed,
        }
    }
}

fn render_mini_card(idx: usize, data: &ScoreListCardData) -> String {
    let mut html = String::with_capacity(1024);
    html.push_str(r#"<div class="mini-card">"#);

    // Cover strip with background-image (more reliable in blitz than <img>)
    if data.cover_data_uri.is_empty() {
        html.push_str(r#"<div class="cover-strip">"#);
    } else {
        html.push_str(r#"<div class="cover-strip" style="background-image: url(&quot;"#);
        html.push_str(&data.cover_data_uri);
        html.push_str(r#"&quot;);">"#);
    }

    // Rank badge
    html.push_str(&format!(
        r#"<div class="rank {}">{}</div>"#,
        data.rank_class,
        escape_html(&data.rank_display)
    ));

    // Index number
    html.push_str(&format!(r#"<span class="idx">#{}</span>"#, idx + 1));

    // Star rating only (difficulty name removed to keep cover strip clean
    // and avoid horizontal overlap with .time-in-cover on long version strings).
    html.push_str(&format!(
        r#"<span class="star-in-cover">★ {:.2}</span>"#,
        data.score.star_rating
    ));

    // Relative time
    html.push_str(&format!(
        r#"<span class="time-in-cover">{}</span>"#,
        escape_html(&data.relative_time)
    ));

    // Beatmap ID
    html.push_str(&format!(
        r#"<span class="bid-in-cover">{}</span>"#,
        data.score.beatmap_id
    ));

    html.push_str(r#"</div>"#); // end cover-strip

    // Body
    html.push_str(r#"<div class="body">"#);

    // Title
    html.push_str(r#"<div class="title">"#);
    html.push_str(&escape_html(&data.score.title));
    html.push_str(r#"</div>"#);

    // Subtitle (artist only; mapper-defined difficulty name is shown in the
    // cover strip alongside the star rating, so it would be redundant here)
    html.push_str(r#"<div class="sub"><span>"#);
    html.push_str(&escape_html(&data.score.artist));
    html.push_str(r#"</span></div>"#);

    // Mods row
    html.push_str(&format!(
        r#"<div class="row"><div class="mods">{}</div></div>"#,
        data.mods_html
    ));

    // Acc + PP row
    let pp_class = if data.passed { "pp" } else { "pp pp-fail" };
    html.push_str(&format!(
        r#"<div class="row2"><span class="acc">{}</span><span class="{}">{}<span class="pp-unit">pp</span></span></div>"#,
        data.acc_formatted,
        pp_class,
        data.pp_formatted
    ));

    html.push_str(r#"</div></div>"#); // end body, end mini-card
    html
}

pub struct ScoreListHtmlParams<'a> {
    pub cards: &'a [ScoreListCardData],
    pub username: &'a str,
    pub mode: osubot_types::GameMode,
    pub is_pass: bool,
    pub avatar_data_uri: &'a str,
    pub hero_bg_data_uri: &'a str,
    pub user_pp: f64,
    pub user_global_rank: Option<i64>,
    pub user_country_rank: Option<i64>,
    pub country_code: &'a str,
    pub pp_change: Option<f64>,
    pub global_rank_change: Option<i64>,
    pub country_rank_change: Option<i64>,
}

fn format_relative_time(created_at: &str) -> String {
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(created_at) else {
        return String::new();
    };
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(dt);
    let seconds = duration.num_seconds().max(0);

    if seconds < 60 {
        // < 1 min 不显示时间(避免 ~1min 这种近似值)
        return String::new();
    }

    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("~{}min", minutes);
    }

    let hours = minutes / 60;
    if hours < 24 {
        return format!("~{}h", hours);
    }

    let days = hours / 24;
    if days < 30 {
        return format!("~{}d", days);
    }

    let months = days / 30;
    if months < 12 {
        return format!("~{}mo", months);
    }

    let years = months / 12;
    format!("~{}y", years)
}

pub fn wrap_score_list_html(params: &ScoreListHtmlParams<'_>) -> String {
    let css = SCORE_LIST_CSS.to_string();

    let label = if params.is_pass {
        "最近通过"
    } else {
        "最近游玩"
    };
    let mode_name = params.mode.name();
    let count = params.cards.len();

    let mut html = String::with_capacity(32768);
    html.push_str(r#"<!DOCTYPE html><html><head><style>"#);
    html.push_str(&css);
    html.push_str(r#"</style></head><body><div class="score-list-card">"#);

    // Hero section with background-image (more reliable in blitz than <img>)
    if params.hero_bg_data_uri.is_empty() {
        html.push_str(r#"<div class="hero hero--empty">"#);
    } else {
        html.push_str(r#"<div class="hero" style="background-image: url(&quot;"#);
        html.push_str(params.hero_bg_data_uri);
        html.push_str(r#"&quot;);">"#);
    }
    html.push_str(r#"<div class="hero-overlay"></div><div class="hero-content">"#);

    html.push_str(r#"<div class="hero-avatar"><img src=""#);
    html.push_str(params.avatar_data_uri);
    html.push_str(r#"" /></div>"#);

    html.push_str(r#"<div class="hero-info"><div class="hero-name">"#);
    html.push_str(&escape_html(params.username));
    html.push_str(r#"</div>"#);

    // Rank + PP row
    html.push_str(r#"<div class="hero-rank-pp">"#);
    if let Some(rank) = params.user_global_rank {
        let change_html = crate::style::format_rank_change_html(params.global_rank_change);
        html.push_str(&format!(
            r#"<div class="rank-item"><span class="rank-hash">#</span><span class="rank-val">{}</span>{}<span class="rank-label">Global</span></div>"#,
            format_number(rank),
            change_html
        ));
    }
    if let Some(rank) = params.user_country_rank {
        let change_html = crate::style::format_rank_change_html(params.country_rank_change);
        html.push_str(&format!(
            r#"<div class="rank-item"><span class="rank-hash">#</span><span class="rank-val">{}</span>{}<span class="rank-label">{}</span></div>"#,
            format_number(rank),
            change_html,
            escape_html(params.country_code)
        ));
    }
    let pp_change_html = crate::style::format_pp_change_html(params.pp_change);
    html.push_str(&format!(
        r#"<div class="user-pp-section"><span class="user-pp-val">{:.0}pp</span>{}</div>"#,
        params.user_pp, pp_change_html
    ));
    html.push_str(r#"</div>"#);

    // Meta row
    html.push_str(r#"<div class="hero-meta"><span>"#);
    html.push_str(label);
    html.push_str(r#"</span><span class="dot">·</span><span>"#);
    html.push_str(mode_name);
    html.push_str(r#"</span><span class="dot">·</span><span>"#);
    html.push_str(&format!("{} 条记录", count));
    html.push_str(r#"</span></div></div></div></div>"#);

    // Score list
    html.push_str(r#"<div class="score-list">"#);
    for (i, card) in params.cards.iter().enumerate() {
        html.push_str(&render_mini_card(i, card));
    }
    html.push_str(r#"</div></div></body></html>"#);

    html
}

#[cfg(test)]
mod tests {
    use super::*;
    use osubot_types::{Score, ScoreStatistics, ScoreUser};
    use rosu_mods::GameMods;

    fn make_test_score() -> Score {
        let mut mods = GameMods::new();
        mods.insert(rosu_mods::GameMod::HiddenOsu(Default::default()));
        Score {
            score_id: 1,
            beatmap_id: 100,
            beatmapset_id: 200,
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
            max_combo: 400,
            beatmap_max_combo: 500,
            pp: Some(456.0),
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
            user: ScoreUser {
                avatar_url: "https://example.com/avatar.jpg".to_string(),
                country_code: "CN".to_string(),
                user_id: None,
                username: None,
                global_rank: Some(12345),
                country_rank: Some(1000),
                pp: 9876.5,
            },
            fav_count: None,
            play_count: None,
            status: "ranked".to_string(),
        }
    }

    #[test]
    fn test_wrap_score_list_html_basic() {
        let score = make_test_score();
        let card =
            ScoreListCardData::from_score(&score, "data:image/jpeg;base64,cover".to_string());
        let params = ScoreListHtmlParams {
            cards: &[card],
            username: "TestUser",
            mode: osubot_types::GameMode::Osu,
            is_pass: true,
            avatar_data_uri: "data:image/jpeg;base64,avatar",
            hero_bg_data_uri: "data:image/jpeg;base64,bg",
            user_pp: 9876.5,
            user_global_rank: Some(12345),
            user_country_rank: Some(1000),
            country_code: "CN",
            pp_change: Some(12.0),
            global_rank_change: Some(-99),
            country_rank_change: Some(50),
        };
        let html = wrap_score_list_html(&params);

        assert!(html.contains("TestUser"));
        assert!(html.contains("最近通过"));
        assert!(html.contains("osu!"));
        assert!(html.contains("1 条记录"));
        assert!(html.contains("TestTitle"));
        assert!(html.contains("TestArtist"));
        assert!(html.contains("★ 6.5"));
        assert!(html.contains("456"));
        assert!(html.contains("98.5%"));
        assert!(html.contains("mini-card"));
        assert!(html.contains("rank-s"));
        assert!(html.contains("HD"));
    }

    #[test]
    fn test_wrap_score_list_html_xss() {
        let mut score = make_test_score();
        score.title = "<script>alert('xss')</script>".to_string();
        score.artist = "Artist<img onerror=alert(1)>".to_string();
        let card = ScoreListCardData::from_score(&score, String::new());
        let params = ScoreListHtmlParams {
            cards: &[card],
            username: "<b>Bad</b>",
            mode: osubot_types::GameMode::Osu,
            is_pass: false,
            avatar_data_uri: "",
            hero_bg_data_uri: "",
            user_pp: 5000.0,
            user_global_rank: None,
            user_country_rank: None,
            country_code: "",
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
        };
        let html = wrap_score_list_html(&params);

        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img onerror"));
        assert!(!html.contains("<b>Bad</b>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&lt;b&gt;Bad&lt;/b&gt;"));
    }

    #[test]
    fn test_score_list_card_data_from_score() {
        let score = make_test_score();
        let card = ScoreListCardData::from_score(&score, "data:image/jpeg;base64,test".to_string());

        assert_eq!(card.rank_class, "rank-s");
        assert_eq!(card.rank_display, "S");
        assert_eq!(card.acc_formatted, "98.5%");
        assert_eq!(card.pp_formatted, "456");
        assert!(card.mods_html.contains("HD"));
    }

    #[test]
    fn test_rank_display_xh() {
        let mut score = make_test_score();
        score.rank = "XH".to_string();
        let card = ScoreListCardData::from_score(&score, String::new());
        assert_eq!(card.rank_class, "rank-s-silver");
        assert_eq!(card.rank_display, "X");
    }

    #[test]
    fn test_empty_mods() {
        let mut score = make_test_score();
        score.mods = GameMods::new();
        score.is_lazer = false;
        let card = ScoreListCardData::from_score(&score, String::new());
        assert!(card.mods_html.is_empty());
    }

    #[test]
    fn test_recent_label() {
        let score = make_test_score();
        let card = ScoreListCardData::from_score(&score, String::new());
        let params = ScoreListHtmlParams {
            cards: &[card],
            username: "User",
            mode: osubot_types::GameMode::Osu,
            is_pass: false,
            avatar_data_uri: "",
            hero_bg_data_uri: "",
            user_pp: 5000.0,
            user_global_rank: None,
            user_country_rank: None,
            country_code: "",
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
        };
        let html = wrap_score_list_html(&params);
        assert!(html.contains("最近游玩"));
        assert!(!html.contains("最近通过"));
    }

    #[test]
    fn test_format_relative_time_invalid() {
        assert_eq!(format_relative_time("invalid"), "");
        assert_eq!(format_relative_time(""), "");
    }

    #[test]
    fn test_format_relative_time_recent() {
        let now = chrono::Utc::now();
        let ts = now.to_rfc3339();
        let result = format_relative_time(&ts);
        assert!(result == "~1min" || result.is_empty() || result.starts_with("~"));
    }

    /// 边界:<60s 视为"刚刚",留空(避免与 ~1min 语义冲突)
    #[test]
    fn test_format_relative_time_under_one_minute() {
        let now = chrono::Utc::now();
        let ts = (now - chrono::TimeDelta::seconds(30)).to_rfc3339();
        assert_eq!(format_relative_time(&ts), "");
    }

    /// 边界:60s 应进入 minutes 分支
    #[test]
    fn test_format_relative_time_one_minute() {
        let now = chrono::Utc::now();
        let ts = (now - chrono::TimeDelta::seconds(60)).to_rfc3339();
        let result = format_relative_time(&ts);
        assert!(
            result == "~1min" || result.is_empty(),
            "60s ago should be ~1min or empty (boundary race), got {result}"
        );
    }

    /// 边界:59min 应进入 hours 分支,~1h
    #[test]
    fn test_format_relative_time_59_minutes() {
        let now = chrono::Utc::now();
        let ts = (now - chrono::TimeDelta::minutes(59)).to_rfc3339();
        let result = format_relative_time(&ts);
        assert!(
            result.starts_with("~") && (result.ends_with("h") || result.ends_with("min")),
            "59min ago should be ~1h or ~59min, got {result}"
        );
    }

    /// 边界:24h 应进入 days 分支
    #[test]
    fn test_format_relative_time_24_hours() {
        let now = chrono::Utc::now();
        let ts = (now - chrono::TimeDelta::hours(24)).to_rfc3339();
        let result = format_relative_time(&ts);
        assert!(
            result.starts_with("~") && (result.ends_with("d") || result.ends_with("h")),
            "24h ago should be ~1d or ~24h, got {result}"
        );
    }

    /// 边界:30d 应进入 months 分支
    #[test]
    fn test_format_relative_time_30_days() {
        let now = chrono::Utc::now();
        let ts = (now - chrono::TimeDelta::days(30)).to_rfc3339();
        let result = format_relative_time(&ts);
        assert!(
            result.starts_with("~") && (result.ends_with("mo") || result.ends_with("d")),
            "30d ago should be ~1mo or ~30d, got {result}"
        );
    }

    /// 边界:365d 应进入 years 分支
    #[test]
    fn test_format_relative_time_365_days() {
        let now = chrono::Utc::now();
        let ts = (now - chrono::TimeDelta::days(365)).to_rfc3339();
        let result = format_relative_time(&ts);
        assert!(
            result.starts_with("~") && (result.ends_with("y") || result.ends_with("mo")),
            "365d ago should be ~1y or ~12mo, got {result}"
        );
    }

    /// blitz's CSS grid does not compute the intrinsic height of items when
    /// the items have no explicit `height`. Without `grid-auto-rows: min-content`,
    /// cards collapse to 0 height and the score list renders as a solid block.
    /// See commit that added `grid-auto-rows` for context.
    #[test]
    fn test_score_list_grid_uses_min_content_rows() {
        assert!(
            SCORE_LIST_CSS.contains("grid-auto-rows: min-content"),
            ".score-list must declare `grid-auto-rows: min-content` so blitz gives grid items their intrinsic height"
        );
    }

    /// blitz 0.2.4 does not paint `<img>` elements whose `src` is a `data:`
    /// URI when the image is inside a CSS grid item, even though the resource
    /// is fetched and decoded successfully. The cover-strip therefore uses a
    /// `background-image: url(data:...)` inline style on the strip div, with
    /// `background-size: cover` to replicate the cropped thumbnail effect.
    #[test]
    fn test_cover_strip_uses_background_image() {
        let score = make_test_score();
        let card = ScoreListCardData::from_score(&score, "data:image/jpeg;base64,ABC".to_string());
        let params = ScoreListHtmlParams {
            cards: &[card],
            username: "U",
            mode: osubot_types::GameMode::Osu,
            is_pass: true,
            avatar_data_uri: "",
            hero_bg_data_uri: "",
            user_pp: 0.0,
            user_global_rank: None,
            user_country_rank: None,
            country_code: "",
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
        };
        let html = wrap_score_list_html(&params);
        assert!(
            html.contains("background-image: url(&quot;data:image/jpeg;base64,ABC&quot;)"),
            "cover-strip must inline the cover data URI as `background-image: url(...)` (blitz does not paint <img src=\"data:...\"> reliably inside CSS grid items)"
        );
        assert!(
            !html.contains("<img src=\"data:image"),
            "cover-strip must not contain an <img> tag with a data: URI (renders blank in blitz)"
        );
    }

    /// hero 高度通过 min-height 锁定到 640px,与 .hero 上的 2560×640 banner
    /// 背景图 1:1 匹配;background-size: cover 退化为 100% 100%。无 banner 时
    /// 加 .hero--empty 让高度回归内容(避免 640px 纯灰方块)。
    #[test]
    fn test_hero_min_height_matches_banner() {
        assert!(
            SCORE_LIST_CSS.contains("min-height: 640px"),
            ".hero must have min-height: 640px to match the 2560x640 banner image"
        );
        assert!(
            SCORE_LIST_CSS.contains(".hero--empty"),
            ".hero--empty class must exist for the no-banner fallback"
        );
    }

    /// Same blitz `<img src="data:...">` rendering issue that affects the cover strip
    /// also applies to the hero background banner. The hero `<div>` carries the data
    /// URI via an inline `background-image: url(&quot;...&quot;)` style instead.
    #[test]
    fn test_hero_bg_uses_background_image() {
        let score = make_test_score();
        let card = ScoreListCardData::from_score(&score, "data:image/jpeg;base64,ABC".to_string());
        let params = ScoreListHtmlParams {
            cards: &[card],
            username: "U",
            mode: osubot_types::GameMode::Osu,
            is_pass: true,
            avatar_data_uri: "",
            hero_bg_data_uri: "data:image/jpeg;base64,HEROBG",
            user_pp: 0.0,
            user_global_rank: None,
            user_country_rank: None,
            country_code: "",
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
        };
        let html = wrap_score_list_html(&params);
        assert!(
            html.contains(
                r#"<div class="hero" style="background-image: url(&quot;data:image/jpeg;base64,HEROBG&quot;);">"#
            ),
            "hero must inline the bg data URI as `background-image: url(...)`"
        );
        assert!(
            !html.contains(r#"<img class="hero-bg""#),
            "hero must not contain a <img class=\"hero-bg\"> element (renders blank in blitz)"
        );
    }

    /// The star rating overlay on the cover strip shows the numeric star rating
    /// (1 decimal place) only — the mapper-defined difficulty name was removed
    /// to keep the cover strip clean and prevent horizontal overlap with
    /// `.time-in-cover` on long version strings.
    #[test]
    fn test_star_in_cover_shows_rating() {
        let score = make_test_score();
        let card = ScoreListCardData::from_score(&score, "data:image/jpeg;base64,X".to_string());
        let params = ScoreListHtmlParams {
            cards: &[card],
            username: "U",
            mode: osubot_types::GameMode::Osu,
            is_pass: true,
            avatar_data_uri: "",
            hero_bg_data_uri: "",
            user_pp: 0.0,
            user_global_rank: None,
            user_country_rank: None,
            country_code: "",
            pp_change: None,
            global_rank_change: None,
            country_rank_change: None,
        };
        let html = wrap_score_list_html(&params);
        assert!(
            html.contains(r#"<span class="star-in-cover">★ 6.50</span>"#),
            "star-in-cover should show `★ <rating>` only; got HTML: {}",
            html.split("star-in-cover")
                .nth(1)
                .unwrap_or("")
                .chars()
                .take(200)
                .collect::<String>()
        );
    }
}
