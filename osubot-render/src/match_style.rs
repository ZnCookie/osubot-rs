use maud::{html, Markup, PreEscaped};
use osubot_types::{format_accuracy, format_number};

const MATCH_CSS: &str = include_str!("../styles/match.css");

#[derive(Clone, Debug)]
pub struct MatchResultCardData {
    pub match_id: u64,
    pub match_name: String,
    pub event_label: String,
    pub played_at: String,
    pub beatmap_id: u64,
    pub beatmap_artist: String,
    pub beatmap_title: String,
    pub beatmap_version: String,
    pub beatmap_mapper: String,
    pub beatmap_mode: String,
    pub star_rating: Option<f64>,
    pub beatmap_bpm: Option<f64>,
    pub beatmap_length_seconds: Option<u32>,
    pub beatmap_max_combo: Option<u32>,
    pub beatmap_ar: Option<f64>,
    pub beatmap_od: Option<f64>,
    pub beatmap_cs: Option<f64>,
    pub beatmap_hp: Option<f64>,
    pub cover_data_uri: String,
    pub is_started: bool,
    pub selected_mods: Vec<String>,
    pub team_type: Option<String>,
    pub scoring_type: Option<String>,
    pub team_results: Vec<MatchTeamResultData>,
    pub players: Vec<MatchPlayerRowData>,
}

#[derive(Clone, Debug)]
pub struct MatchTeamResultData {
    pub team: String,
    pub score: u64,
    pub is_winner: bool,
}

#[derive(Clone, Debug)]
pub struct MatchPlayerRowData {
    pub placement: usize,
    pub username: String,
    pub avatar_data_uri: String,
    pub team: Option<String>,
    pub score: u64,
    pub accuracy: f64,
    pub max_combo: u32,
    pub mods: Vec<String>,
    pub rank: String,
    pub passed: bool,
}

fn render_player_row(row: &MatchPlayerRowData, is_started: bool) -> Markup {
    if is_started {
        render_started_player_row(row)
    } else {
        render_result_player_row(row)
    }
}

fn render_avatar(row: &MatchPlayerRowData) -> Markup {
    let avatar_initial = row.username.chars().next().unwrap_or('?');

    html! {
        div.match-player-avatar {
            @if row.avatar_data_uri.is_empty() {
                span { (avatar_initial) }
            } @else {
                img src=(row.avatar_data_uri) alt="";
            }
        }
    }
}

fn render_started_player_row(row: &MatchPlayerRowData) -> Markup {
    let rank_class = match row.placement {
        1 => "rank-1",
        2 => "rank-2",
        3 => "rank-3",
        _ => "rank-n",
    };

    html! {
        div.match-player-row.match-player-row--started {
            div class=(format!("match-player-rank {rank_class}")) {
                "#" (row.placement)
            }
            (render_avatar(row))
            div.match-player-main {
                div.match-player-name { (row.username) }
                div.match-player-participant { "PARTICIPANT" }
            }
        }
    }
}

fn derive_hue_sat(beatmap_id: u64) -> (u16, u16) {
    let hue = (beatmap_id.wrapping_mul(37) % 280 + 40) as u16;
    let sat = (30 + beatmap_id.wrapping_mul(17) % 40) as u16;
    (hue, sat)
}

fn format_score(score: u64) -> String {
    if score > i64::MAX as u64 {
        score.to_string()
    } else {
        format_number(score as i64)
    }
}

fn render_result_player_row(row: &MatchPlayerRowData) -> Markup {
    let row_class = if row.passed {
        "match-player-row"
    } else {
        "match-player-row match-player-row--failed"
    };
    let rank_class = match row.placement {
        1 => "rank-1",
        2 => "rank-2",
        3 => "rank-3",
        _ => "rank-n",
    };
    let rank_display = if row.rank.is_empty() {
        if row.passed {
            "PASS"
        } else {
            "F"
        }
    } else {
        row.rank.as_str()
    };
    html! {
        div class=(row_class) {
            div class=(format!("match-player-rank {rank_class}")) {
                "#" (row.placement)
            }
            (render_avatar(row))
            div.match-player-main {
                div.match-player-name { (row.username) }
                div.match-player-mods {
                    @if row.mods.is_empty() {
                        span.chip.chip-filled { "NM" }
                    } @else {
                        @for m in &row.mods {
                            span.chip.chip-filled { (m) }
                        }
                    }
                }
            }
            div.match-player-stats {
                div.stat-block.stat-block-score {
                    div.stat-val { (format_score(row.score)) }
                    div.stat-label { "SCORE" }
                }
                div.stat-block.stat-block-acc {
                    div.stat-val { (format_accuracy(row.accuracy)) }
                    div.stat-label { "ACC" }
                }
                div.stat-block.stat-block-combo {
                    div.stat-val { (row.max_combo) "x" }
                    div.stat-label { "COMBO" }
                }
            }
            div.match-player-status-col {
                @if row.passed {
                    span.chip.chip-status-pass { (rank_display) }
                } @else {
                    span.chip.chip-status-fail { (rank_display) }
                }
                @if let Some(ref team) = row.team {
                    span.chip.chip-info { (team) }
                }
            }
        }
    }
}

fn render_mod_chips(mods: &[String]) -> Markup {
    html! {
        span.chip.chip-info.chip-mods {
            span.chip-label { "Mods: " }
            span.chip-num {
                @if mods.is_empty() {
                    "NM"
                } @else {
                    (mods.join(" "))
                }
            }
        }
    }
}

fn format_optional_decimal(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.1}"))
        .unwrap_or_else(|| "--".to_string())
}

fn format_length(length_seconds: Option<u32>) -> String {
    length_seconds
        .map(|seconds| format!("{}:{:02}", seconds / 60, seconds % 60))
        .unwrap_or_else(|| "--:--".to_string())
}

fn render_started_beatmap_stats(data: &MatchResultCardData) -> Markup {
    html! {
        @if data.is_started {
            div.started-beatmap-stats {
                div.started-stat { span.started-stat-label { "BPM" } span.started-stat-value { (data.beatmap_bpm.map(|v| format!("{v:.0}")).unwrap_or_else(|| "--".to_string())) } }
                div.started-stat { span.started-stat-label { "Length" } span.started-stat-value { (format_length(data.beatmap_length_seconds)) } }
                div.started-stat { span.started-stat-label { "Max Combo" } span.started-stat-value { (data.beatmap_max_combo.map(|v| v.to_string()).unwrap_or_else(|| "--".to_string())) } }
                div.started-stat { span.started-stat-label { "Players" } span.started-stat-value { (data.players.len()) } }
                div.started-stat { span.started-stat-label { "AR" } span.started-stat-value { (format_optional_decimal(data.beatmap_ar)) } }
                div.started-stat { span.started-stat-label { "OD" } span.started-stat-value { (format_optional_decimal(data.beatmap_od)) } }
                div.started-stat { span.started-stat-label { "CS" } span.started-stat-value { (format_optional_decimal(data.beatmap_cs)) } }
                div.started-stat { span.started-stat-label { "HP" } span.started-stat-value { (format_optional_decimal(data.beatmap_hp)) } }
            }
        }
    }
}

fn render_team_results(teams: &[MatchTeamResultData]) -> Markup {
    html! {
        @if !teams.is_empty() {
            div.match-team-results {
                @for team in teams {
                    div class=(if team.is_winner { "team-result team-result--win" } else { "team-result team-result--lose" }) {
                        span.team-result-state { @if team.is_winner { "WIN" } @else { "LOSE" } }
                        span.team-result-name { (team.team.to_uppercase()) }
                        span.team-result-score { (format_score(team.score)) }
                    }
                }
            }
        }
    }
}

fn team_sections(data: &MatchResultCardData) -> Vec<(&str, Option<&MatchTeamResultData>)> {
    let mut sections = Vec::new();

    for team in &data.team_results {
        sections.push((team.team.as_str(), Some(team)));
    }

    for player in &data.players {
        let Some(team) = player.team.as_deref() else {
            continue;
        };
        if sections
            .iter()
            .any(|(existing, _)| existing.eq_ignore_ascii_case(team))
        {
            continue;
        }
        sections.push((team, None));
    }

    sections
}

fn should_split_team_sections(data: &MatchResultCardData) -> bool {
    let mut distinct_teams = std::collections::BTreeSet::new();
    for player in &data.players {
        if let Some(team) = player.team.as_deref() {
            let team = team.trim();
            if !team.is_empty() {
                distinct_teams.insert(team.to_ascii_lowercase());
            }
        }
    }

    distinct_teams.len() >= 2
        || (!distinct_teams.is_empty()
            && data
                .team_type
                .as_deref()
                .is_some_and(|team_type| team_type.contains("team")))
}

fn render_player_list(rows: &[&MatchPlayerRowData], is_started: bool) -> Markup {
    html! {
        div.match-player-list {
            @for row in rows {
                (render_player_row(row, is_started))
            }
        }
    }
}

fn render_player_groups(data: &MatchResultCardData) -> Markup {
    if !should_split_team_sections(data) {
        let rows: Vec<&MatchPlayerRowData> = data.players.iter().collect();
        return render_player_list(&rows, data.is_started);
    }

    let sections = team_sections(data);
    html! {
        div.team-player-sections {
            @for (team_name, team_result) in sections {
                @let rows: Vec<&MatchPlayerRowData> = data
                    .players
                    .iter()
                    .filter(|player| player.team.as_deref().is_some_and(|team| team.eq_ignore_ascii_case(team_name)))
                    .collect();
                @if !rows.is_empty() {
                    div.team-player-section {
                        div.team-player-section-header {
                            div.team-player-section-title { (team_name.to_uppercase()) " TEAM" }
                            @if let Some(team_result) = team_result {
                                div.team-player-section-meta {
                                    span class=(if team_result.is_winner { "team-player-section-state team-player-section-state--win" } else { "team-player-section-state team-player-section-state--lose" }) {
                                        @if team_result.is_winner { "WIN" } @else { "LOSE" }
                                    }
                                    span.team-player-section-score { (format_score(team_result.score)) }
                                }
                            }
                        }
                        (render_player_list(&rows, data.is_started))
                    }
                }
            }
        }
    }
}

fn display_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.trim().is_empty() {
        fallback
    } else {
        value
    }
}

#[must_use]
pub fn wrap_match_result_html(data: &MatchResultCardData) -> String {
    let (hue, sat) = derive_hue_sat(data.beatmap_id);
    let runtime_css = format!(":root {{ --match-hue: {hue}; --match-sat: {sat}%; }}");
    let player_count_class = match data.players.len() {
        0..=6 => "player-count--normal",
        7..=8 => "player-count--many",
        _ => "player-count--crowded",
    };
    let state_class = if data.is_started {
        "match-card--started"
    } else {
        "match-card--finished"
    };

    let star_text = data
        .star_rating
        .map(|stars| format!("{stars:.2}"))
        .unwrap_or_else(|| "--".to_string());
    let fallback_title = format!("Beatmap #{}", data.beatmap_id);
    let beatmap_title = display_or(&data.beatmap_title, &fallback_title);
    let beatmap_artist = display_or(&data.beatmap_artist, "未知曲师");
    let beatmap_mapper = display_or(&data.beatmap_mapper, "未知作者");
    let beatmap_version = display_or(&data.beatmap_version, "未知难度");
    let beatmap_mode = display_or(&data.beatmap_mode, "osu");
    let has_cover = !data.cover_data_uri.is_empty();

    html! {
        (PreEscaped("<!DOCTYPE html>"))
        html {
            head {
                meta charset="utf-8";
                style { (PreEscaped(MATCH_CSS)) }
                style { (PreEscaped(runtime_css)) }
            }
            body {
                div class=(format!("match-card {player_count_class} {state_class}")) {
                    @if has_cover {
                        img.match-bg src=(data.cover_data_uri);
                    }
                    div.match-bg-overlay {}
                    div.match-content {
                        div.match-top-row {
                            div.beatmap-card.surface {
                                div.cover-wrap {
                                    @if has_cover {
                                        img src=(data.cover_data_uri);
                                    }
                                }
                                div.beatmap-text {
                                    div.beatmap-title { (beatmap_title) }
                                    div.beatmap-artist { (beatmap_artist) }
                                    div.bottom-chips {
                                        span.chip.chip-diff {
                                            span.star { "★ " (star_text) }
                                            span.diff-name { (beatmap_version) }
                                        }
                                        (render_mod_chips(&data.selected_mods))
                                        @if let Some(ref team_type) = data.team_type {
                                            span.chip.chip-info {
                                                span.chip-label { "Team: " }
                                                span.chip-num { (team_type) }
                                            }
                                        }
                                        @if let Some(ref scoring_type) = data.scoring_type {
                                            span.chip.chip-info {
                                                span.chip-label { "Scoring: " }
                                                span.chip-num { (scoring_type) }
                                            }
                                        }
                                        span.chip.chip-info {
                                            span.chip-label { "Mode: " }
                                            span.chip-num { (beatmap_mode) }
                                        }
                                        span.chip.chip-info {
                                            span.chip-label { "Mapper: " }
                                            span.chip-num { (beatmap_mapper) }
                                        }
                                        span.chip.chip-info {
                                            span.chip-label { "BID: " }
                                            span.chip-num { (data.beatmap_id) }
                                        }
                                    }
                                    (render_started_beatmap_stats(data))
                                }
                            }
                            div.meta-card.surface {
                                div.match-kicker { @if data.is_started { "MATCH START" } @else { "MATCH RESULT" } }
                                div.match-title { (data.match_name) }
                                div.match-meta {
                                    span { "MP #" (data.match_id) }
                                    span.dot { "·" }
                                    span { (data.event_label) }
                                    span.dot { "·" }
                                    span { (data.played_at) }
                                }
                                div.match-player-count {
                                    (data.players.len()) " players"
                                }
                            }
                        }
                        div.match-players.surface {
                            (render_team_results(&data.team_results))
                            div.match-player-list-header {
                                span { "PLAYERS" }
                                span { (data.players.len()) " players" }
                            }
                            (render_player_groups(data))
                        }
                    }
                }
            }
        }
    }
    .into_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_data() -> MatchResultCardData {
        MatchResultCardData {
            match_id: 12_345_678,
            match_name: "Test Lobby".to_string(),
            event_label: "场次结束".to_string(),
            played_at: "2026/06/26 20:00:00".to_string(),
            beatmap_id: 987_654,
            beatmap_artist: "xi".to_string(),
            beatmap_title: "Blue Zenith".to_string(),
            beatmap_version: "FOUR DIMENSIONS".to_string(),
            beatmap_mapper: "Asphyxia".to_string(),
            beatmap_mode: "osu!".to_string(),
            star_rating: Some(7.85),
            beatmap_bpm: Some(190.0),
            beatmap_length_seconds: Some(260),
            beatmap_max_combo: Some(2429),
            beatmap_ar: Some(9.7),
            beatmap_od: Some(9.8),
            beatmap_cs: Some(4.0),
            beatmap_hp: Some(6.0),
            cover_data_uri: "data:image/jpeg;base64,COVER".to_string(),
            is_started: false,
            selected_mods: vec!["HD".to_string(), "HR".to_string()],
            team_type: Some("team-vs".to_string()),
            scoring_type: Some("score".to_string()),
            team_results: vec![
                MatchTeamResultData {
                    team: "red".to_string(),
                    score: 2_000_000,
                    is_winner: true,
                },
                MatchTeamResultData {
                    team: "blue".to_string(),
                    score: 1_500_000,
                    is_winner: false,
                },
            ],
            players: vec![
                MatchPlayerRowData {
                    placement: 1,
                    username: "Alice".to_string(),
                    avatar_data_uri: "data:image/png;base64,AVATAR".to_string(),
                    team: Some("Red".to_string()),
                    score: 1_234_567,
                    accuracy: 0.9876,
                    max_combo: 1234,
                    mods: vec!["HD".to_string(), "HR".to_string()],
                    rank: "A".to_string(),
                    passed: true,
                },
                MatchPlayerRowData {
                    placement: 2,
                    username: "Bob".to_string(),
                    avatar_data_uri: String::new(),
                    team: Some("Blue".to_string()),
                    score: 987_654,
                    accuracy: 0.9654,
                    max_combo: 876,
                    mods: Vec::new(),
                    rank: "F".to_string(),
                    passed: false,
                },
            ],
        }
    }

    #[test]
    fn renders_match_card_structure() {
        let html = wrap_match_result_html(&make_test_data());

        assert!(html.contains(r#"class="match-card player-count--normal match-card--finished""#));
        assert!(html.contains(r#"class="match-bg""#));
        assert!(html.contains("data:image/jpeg;base64,COVER"));
        assert!(html.contains(r#"class="match-player-row""#));
        assert!(html.contains(r#"class="match-player-avatar""#));
        assert!(html.contains("data:image/png;base64,AVATAR"));
        assert!(html.matches(r#"<div class="match-player-row"#).count() >= 2);
        assert!(html.contains("MATCH RESULT"));
        assert!(html.contains("Mods: "));
        assert!(html.contains("Team: "));
        assert!(html.contains("Scoring: "));
        assert!(html.contains("WIN"));
        assert!(html.contains("LOSE"));
        assert!(html.contains("RED TEAM"));
        assert!(html.contains("BLUE TEAM"));
        assert!(html.contains("Test Lobby"));
        assert!(html.contains("Blue Zenith"));
        assert!(html.contains("1,234,567"));
        assert!(html.contains("98.76%"));
        assert!(html.contains(r#"<meta charset="utf-8">"#));
        assert!(MATCH_CSS.contains("width: 1920px"));
        assert!(MATCH_CSS.contains("min-height: 1080px"));
    }

    #[test]
    fn renders_started_match_as_participant_list() {
        let mut data = make_test_data();
        data.event_label = "场次开始".to_string();
        data.is_started = true;
        data.players[0].score = 0;
        data.players[0].accuracy = 0.0;
        data.players[0].max_combo = 0;

        let html = wrap_match_result_html(&data);

        assert!(html.contains("match-card--started"));
        assert!(html.contains("MATCH START"));
        assert!(html.contains("Alice"));
        assert!(html.contains("BPM"));
        assert!(html.contains("Length"));
        assert!(html.contains("Max Combo"));
        assert!(!html.contains("SCORE"));
        assert!(!html.contains("PASS"));
    }

    #[test]
    fn renders_team_vs_players_in_separate_sections() {
        let html = wrap_match_result_html(&make_test_data());

        assert!(html.contains("team-player-sections"));
        assert!(html.contains("RED TEAM"));
        assert!(html.contains("BLUE TEAM"));
    }

    #[test]
    fn renders_team_sections_even_without_team_type_when_player_teams_exist() {
        let mut data = make_test_data();
        data.team_type = None;

        let html = wrap_match_result_html(&data);

        assert!(html.contains("team-player-sections"));
        assert!(html.contains("RED TEAM"));
        assert!(html.contains("BLUE TEAM"));
    }

    #[test]
    fn escapes_match_text() {
        let mut data = make_test_data();
        data.match_name = "<b>Bad Match</b>".to_string();
        data.beatmap_title = "Song<img onerror=alert(1)>".to_string();
        data.players[0].username = "<script>alert(1)</script>".to_string();
        data.players[0].team = Some("Red<script>".to_string());
        data.players[0].mods = vec!["HD<script>".to_string()];

        let html = wrap_match_result_html(&data);

        assert!(!html.contains("<script>"));
        assert!(!html.contains("</script>"));
        assert!(!html.contains("<img onerror"));
        assert!(!html.contains("<b>Bad Match</b>"));
        assert!(html.contains("&lt;b&gt;Bad Match&lt;/b&gt;"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("Song&lt;img onerror=alert(1)&gt;"));
        assert!(html.contains("HD&lt;script&gt;"));
    }

    #[test]
    fn renders_readable_beatmap_fallbacks_for_missing_metadata() {
        let mut data = make_test_data();
        data.beatmap_title.clear();
        data.beatmap_artist.clear();
        data.beatmap_version.clear();
        data.beatmap_mapper.clear();
        data.beatmap_mode.clear();

        let html = wrap_match_result_html(&data);

        assert!(html.contains("Beatmap #987654"));
        assert!(html.contains("未知曲师"));
        assert!(html.contains("未知作者"));
        assert!(html.contains("未知难度"));
        assert!(html.contains("Mode: "));
        assert!(html.contains(r#"<span class="chip-num">osu</span>"#));
        assert!(!html.contains(r#"class="beatmap-title"></div>"#));
        assert!(!html.contains(r#"class="chip-num"></span>"#));
    }
}
