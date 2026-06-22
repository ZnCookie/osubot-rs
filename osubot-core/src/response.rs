use crate::strings::user_str;
use crate::types::{GameMode, Score, UserChange, UserStats};
use osubot_types::{
    format_accuracy, format_length, format_mods, format_number, format_play_datetime,
    trim_trailing_zeros,
};
use phf::phf_map;

/// 将用户统计数据格式化为中文个人信息文本，包含 pp、排名、国家排名、准确率、游玩次数等。
#[must_use]
pub fn format_stats(stats: &UserStats, mode: GameMode) -> String {
    let c = collect_common(stats, mode);
    let rank = stats.rank.to_string();
    let country_rank = stats.country_rank.to_string();

    let rank_change = format_change(stats.rank_change);
    let country_rank_change = format_change(stats.country_rank_change);

    let rank_str = if rank_change.is_empty() {
        rank.to_string()
    } else {
        format!("{rank} ({rank_change})")
    };
    let country = country_name(&stats.country_code);
    let country_rank_str = if country_rank_change.is_empty() {
        format!("{country} #{country_rank}")
    } else {
        format!("{country} #{country_rank} ({country_rank_change})")
    };

    format!(
        "{username}{header}{mode_name}\n\n\
         {pp}{pp_label}\n\
         #{rank_str}\n\
         {country_rank_str}\n\
         {ranked_score}{score_label}\n\
         {acc}{acc_label}\n\
         {playcount}{playcount_label}\n\
         {hits}{hits_label}\n\
         {playtime}{playtime_label}",
        username = c.username,
        header = user_str("fmt.profile_header"),
        mode_name = c.mode_name,
        pp = c.pp,
        pp_label = user_str("fmt.profile_pp"),
        rank_str = rank_str,
        country_rank_str = country_rank_str,
        ranked_score = c.ranked_score,
        score_label = user_str("fmt.profile_ranked_score"),
        acc = c.acc,
        acc_label = user_str("fmt.profile_accuracy"),
        playcount = c.playcount,
        playcount_label = user_str("fmt.profile_playcount"),
        hits = c.hits,
        hits_label = user_str("fmt.profile_hits"),
        playtime = c.playtime,
        playtime_label = user_str("fmt.profile_playtime"),
    )
}

fn format_change(change: Option<i64>) -> String {
    match change {
        Some(c) if c > 0 => format!("↑{}", c),
        Some(c) if c < 0 => format!("↓{}", c.abs()),
        _ => String::new(),
    }
}

fn format_playtime(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    let s = user_str("fmt.playtime_detail");
    let s = s.replace("{hours}", &hours.to_string());
    let s = s.replace("{minutes}", &minutes.to_string());
    s.replace("{seconds}", &secs.to_string())
}

fn format_star_rating(stars: f64) -> String {
    let clamped = stars.max(0.0);
    let full = (clamped.floor() as usize).min(30);
    let half = clamped % 1.0 >= 0.5;
    let filled: String = "\u{2605}".repeat(full);
    let stars_str = if half {
        format!("{filled}\u{2606}")
    } else {
        filled
    };
    format!("{} {:.2}*", stars_str, stars)
}

struct ScoreDisplayFields {
    length: String,
    stars: String,
    pp_str: String,
    acc: String,
}

fn compute_display_fields(score: &Score) -> ScoreDisplayFields {
    ScoreDisplayFields {
        length: format_length(score.length_seconds),
        stars: format_star_rating(score.star_rating),
        pp_str: match score.pp {
            Some(pp) => format!("{:.2}", pp),
            None => user_str("fmt.no_pp").to_string(),
        },
        acc: format_accuracy(score.accuracy), // score.accuracy is already a 0-1 fraction from the API
    }
}

/// 将单条成绩格式化为包含谱面信息、成绩详情和排名的中文文本。
#[must_use]
pub fn format_score(
    score: &Score,
    username: &str,
    mode: GameMode,
    position: Option<usize>,
    is_pass: bool,
) -> String {
    let fields = compute_display_fields(score);

    let mods_str = if score.mods.is_empty() {
        String::new()
    } else {
        format!(" | {}", format_mods(&score.mods))
    };

    let label = if is_pass {
        user_str("fmt.recent_pass")
    } else {
        user_str("fmt.recent_play")
    };
    let position_str = match position {
        Some(pos) => format!("#{} {}", pos + 1, label),
        None => label.to_string(),
    };

    let score_val = format_number(score.score_value);
    let play_time = format_play_datetime(&score.created_at);

    let combo_str = if score.beatmap_max_combo > 0 {
        format!("{}x/{}x", score.max_combo, score.beatmap_max_combo)
    } else {
        format!("{}x", score.max_combo)
    };

    let hits_str = if mode == GameMode::Mania {
        format!(
            "{geki}/{n300}/{katu}/{n100}/{n50}/{miss}",
            geki = score.statistics.count_geki,
            n300 = score.statistics.count_300,
            katu = score.statistics.count_katu,
            n100 = score.statistics.count_100,
            n50 = score.statistics.count_50,
            miss = score.statistics.count_miss,
        )
    } else {
        format!(
            "{n300}/{n100}/{n50}/{miss}",
            n300 = score.statistics.count_300,
            n100 = score.statistics.count_100,
            n50 = score.statistics.count_50,
            miss = score.statistics.count_miss,
        )
    };

    let mode_short = mode.short_name();

    format!(
        "{artist} - {title} [{version}]\n\
         {stars} [{length}]\n\n\
         {username} ({mode}): {pp}pp\n\
         [{rank}] {score_val}\n\n\
         {combo} // {acc}\n\
         {hits}{mods}\n\
         {play_time}\n\n\
         {position}\n\
         ID: {beatmap_id}",
        artist = score.artist,
        title = score.title,
        version = score.version,
        stars = fields.stars,
        length = fields.length,
        username = username,
        mode = mode_short,
        pp = fields.pp_str,
        rank = score.rank,
        score_val = score_val,
        combo = combo_str,
        acc = fields.acc,
        hits = hits_str,
        mods = mods_str,
        play_time = play_time,
        position = position_str,
        beatmap_id = score.beatmap_id,
    )
}

/// 将多条成绩格式化为带序号的并列中文成绩列表文本。
#[must_use]
pub fn format_scores(scores: &[Score], username: &str, mode: GameMode, is_pass: bool) -> String {
    let label = if is_pass {
        user_str("fmt.recent_pass")
    } else {
        user_str("fmt.recent_play")
    };
    let mode_name = mode.name();

    let mut lines = {
        let header = user_str("fmt.score_list_header");
        let header = header.replace("{username}", username);
        let header = header.replace("{label}", label);
        let header = header.replace("{mode}", mode_name);
        vec![header.to_string()]
    };

    for (i, score) in scores.iter().enumerate() {
        let fields = compute_display_fields(score);

        let mods_str = if score.mods.is_empty() {
            String::new()
        } else {
            format!(" // {}", format_mods(&score.mods))
        };
        let play_time = format_play_datetime(&score.created_at);

        let combo_str = if score.beatmap_max_combo > 0 {
            format!("{}x/{}x", score.max_combo, score.beatmap_max_combo)
        } else {
            format!("{}x", score.max_combo)
        };

        lines.push(format!(
            "#{idx} {artist} - {title} [{version}]\n\
                {stars} [{length}]\n\
                [{rank}] {pp}pp // {acc} // {combo}{mods} // {play_time}",
            idx = i + 1,
            artist = score.artist,
            title = score.title,
            version = score.version,
            stars = fields.stars,
            length = fields.length,
            rank = score.rank,
            pp = fields.pp_str,
            acc = fields.acc,
            combo = combo_str,
            mods = mods_str,
            play_time = play_time,
        ));
    }

    lines.join("\n\n")
}

fn format_small_change(change: Option<i64>) -> String {
    match change {
        Some(c) if c > 0 => format!("(+{})", format_number(c)),
        Some(c) if c < 0 => format!("(-{})", format_number(c.abs())),
        _ => String::new(),
    }
}

fn space_prefix(s: String) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!(" {}", s)
    }
}

fn format_float_change(change: Option<f64>, suffix: &str) -> String {
    match change {
        Some(c) if c != 0.0 => {
            let abs = c.abs();
            let sign = if c > 0.0 { "+" } else { "-" };
            let formatted = format!("{:.2}", abs);
            let trimmed = trim_trailing_zeros(&formatted);
            if trimmed.is_empty() || trimmed == "0" {
                return format!("({sign})");
            }
            let stripped = if trimmed.starts_with("0.") {
                trimmed.strip_prefix('0').unwrap_or(&trimmed).to_string()
            } else {
                trimmed
            };
            format!("({sign}{}{suffix})", stripped)
        }
        _ => String::new(),
    }
}

static COUNTRY_NAMES: phf::Map<&'static str, &'static str> = phf_map! {
    "A1" => "Anonymous Proxy",
    "A2" => "Satellite Provider",
    "AD" => "Andorra",
    "AE" => "United Arab Emirates",
    "AF" => "Afghanistan",
    "AG" => "Antigua and Barbuda",
    "AI" => "Anguilla",
    "AL" => "Albania",
    "AM" => "Armenia",
    "AN" => "Netherlands Antilles",
    "AO" => "Angola",
    "AP" => "Asia/Pacific Region",
    "AQ" => "Antarctica",
    "AR" => "Argentina",
    "AS" => "American Samoa",
    "AT" => "Austria",
    "AU" => "Australia",
    "AW" => "Aruba",
    "AX" => "Aland Islands",
    "AZ" => "Azerbaijan",
    "BA" => "Bosnia and Herzegovina",
    "BB" => "Barbados",
    "BD" => "Bangladesh",
    "BE" => "Belgium",
    "BF" => "Burkina Faso",
    "BG" => "Bulgaria",
    "BH" => "Bahrain",
    "BI" => "Burundi",
    "BJ" => "Benin",
    "BL" => "Saint Barthelemy",
    "BM" => "Bermuda",
    "BN" => "Brunei",
    "BO" => "Bolivia",
    "BR" => "Brazil",
    "BS" => "Bahamas",
    "BT" => "Bhutan",
    "BV" => "Bouvet Island",
    "BW" => "Botswana",
    "BY" => "Belarus",
    "BZ" => "Belize",
    "CA" => "Canada",
    "CC" => "Cocos (Keeling) Islands",
    "CD" => "Congo, The Democratic Republic of the",
    "CF" => "Central African Republic",
    "CG" => "Congo",
    "CH" => "Switzerland",
    "CI" => "Cote D'Ivoire",
    "CK" => "Cook Islands",
    "CL" => "Chile",
    "CM" => "Cameroon",
    "CN" => "China",
    "CO" => "Colombia",
    "CR" => "Costa Rica",
    "CU" => "Cuba",
    "CV" => "Cabo Verde",
    "CX" => "Christmas Island",
    "CY" => "Cyprus",
    "CZ" => "Czechia",
    "DE" => "Germany",
    "DJ" => "Djibouti",
    "DK" => "Denmark",
    "DM" => "Dominica",
    "DO" => "Dominican Republic",
    "DZ" => "Algeria",
    "EC" => "Ecuador",
    "EE" => "Estonia",
    "EG" => "Egypt",
    "EH" => "Western Sahara",
    "ER" => "Eritrea",
    "ES" => "Spain",
    "ET" => "Ethiopia",
    "EU" => "Europe",
    "FI" => "Finland",
    "FJ" => "Fiji",
    "FK" => "Falkland Islands (Malvinas)",
    "FM" => "Micronesia, Federated States of",
    "FO" => "Faroe Islands",
    "FR" => "France",
    "FX" => "France, Metropolitan",
    "GA" => "Gabon",
    "GB" => "United Kingdom",
    "GD" => "Grenada",
    "GE" => "Georgia",
    "GF" => "French Guiana",
    "GG" => "Guernsey",
    "GH" => "Ghana",
    "GI" => "Gibraltar",
    "GL" => "Greenland",
    "GM" => "Gambia",
    "GN" => "Guinea",
    "GP" => "Guadeloupe",
    "GQ" => "Equatorial Guinea",
    "GR" => "Greece",
    "GS" => "South Georgia and the South Sandwich Islands",
    "GT" => "Guatemala",
    "GU" => "Guam",
    "GW" => "Guinea-Bissau",
    "GY" => "Guyana",
    "HK" => "Hong Kong",
    "HM" => "Heard Island and McDonald Islands",
    "HN" => "Honduras",
    "HR" => "Croatia",
    "HT" => "Haiti",
    "HU" => "Hungary",
    "ID" => "Indonesia",
    "IE" => "Ireland",
    "IL" => "Israel",
    "IM" => "Isle of Man",
    "IN" => "India",
    "IO" => "British Indian Ocean Territory",
    "IQ" => "Iraq",
    "IR" => "Iran, Islamic Republic of",
    "IS" => "Iceland",
    "IT" => "Italy",
    "JE" => "Jersey",
    "JM" => "Jamaica",
    "JO" => "Jordan",
    "JP" => "Japan",
    "KE" => "Kenya",
    "KG" => "Kyrgyzstan",
    "KH" => "Cambodia",
    "KI" => "Kiribati",
    "KM" => "Comoros",
    "KN" => "Saint Kitts and Nevis",
    "KP" => "Korea, Democratic People's Republic of",
    "KR" => "South Korea",
    "KW" => "Kuwait",
    "KY" => "Cayman Islands",
    "KZ" => "Kazakhstan",
    "LA" => "Lao People's Democratic Republic",
    "LB" => "Lebanon",
    "LC" => "Saint Lucia",
    "LI" => "Liechtenstein",
    "LK" => "Sri Lanka",
    "LR" => "Liberia",
    "LS" => "Lesotho",
    "LT" => "Lithuania",
    "LU" => "Luxembourg",
    "LV" => "Latvia",
    "LY" => "Libya",
    "MA" => "Morocco",
    "MC" => "Monaco",
    "MD" => "Moldova",
    "ME" => "Montenegro",
    "MF" => "Saint Martin",
    "MG" => "Madagascar",
    "MH" => "Marshall Islands",
    "MK" => "North Macedonia",
    "ML" => "Mali",
    "MM" => "Myanmar",
    "MN" => "Mongolia",
    "MO" => "Macau",
    "MP" => "Northern Mariana Islands",
    "MQ" => "Martinique",
    "MR" => "Mauritania",
    "MS" => "Montserrat",
    "MT" => "Malta",
    "MU" => "Mauritius",
    "MV" => "Maldives",
    "MW" => "Malawi",
    "MX" => "Mexico",
    "MY" => "Malaysia",
    "MZ" => "Mozambique",
    "NA" => "Namibia",
    "NC" => "New Caledonia",
    "NE" => "Niger",
    "NF" => "Norfolk Island",
    "NG" => "Nigeria",
    "NI" => "Nicaragua",
    "NL" => "Netherlands",
    "NO" => "Norway",
    "NP" => "Nepal",
    "NR" => "Nauru",
    "NU" => "Niue",
    "NZ" => "New Zealand",
    "O1" => "Other",
    "OM" => "Oman",
    "PA" => "Panama",
    "PE" => "Peru",
    "PF" => "French Polynesia",
    "PG" => "Papua New Guinea",
    "PH" => "Philippines",
    "PK" => "Pakistan",
    "PL" => "Poland",
    "PM" => "Saint Pierre and Miquelon",
    "PN" => "Pitcairn",
    "PR" => "Puerto Rico",
    "PS" => "Palestine, State of",
    "PT" => "Portugal",
    "PW" => "Palau",
    "PY" => "Paraguay",
    "QA" => "Qatar",
    "RE" => "Reunion",
    "RO" => "Romania",
    "RS" => "Serbia",
    "RU" => "Russian Federation",
    "RW" => "Rwanda",
    "SA" => "Saudi Arabia",
    "SB" => "Solomon Islands",
    "SC" => "Seychelles",
    "SD" => "Sudan",
    "SE" => "Sweden",
    "SG" => "Singapore",
    "SH" => "Saint Helena",
    "SI" => "Slovenia",
    "SJ" => "Svalbard and Jan Mayen",
    "SK" => "Slovakia",
    "SL" => "Sierra Leone",
    "SM" => "San Marino",
    "SN" => "Senegal",
    "SO" => "Somalia",
    "SR" => "Suriname",
    "ST" => "Sao Tome and Principe",
    "SV" => "El Salvador",
    "SY" => "Syrian Arab Republic",
    "SZ" => "Eswatini",
    "TC" => "Turks and Caicos Islands",
    "TD" => "Chad",
    "TF" => "French Southern Territories",
    "TG" => "Togo",
    "TH" => "Thailand",
    "TJ" => "Tajikistan",
    "TK" => "Tokelau",
    "TL" => "Timor-Leste",
    "TM" => "Turkmenistan",
    "TN" => "Tunisia",
    "TO" => "Tonga",
    "TR" => "Türkiye",
    "TT" => "Trinidad and Tobago",
    "TV" => "Tuvalu",
    "TW" => "Taiwan",
    "TZ" => "Tanzania, United Republic of",
    "UA" => "Ukraine",
    "UG" => "Uganda",
    "UM" => "United States Minor Outlying Islands",
    "US" => "United States",
    "UY" => "Uruguay",
    "UZ" => "Uzbekistan",
    "VA" => "Holy See (Vatican City State)",
    "VC" => "Saint Vincent and the Grenadines",
    "VE" => "Venezuela",
    "VG" => "Virgin Islands, British",
    "VI" => "Virgin Islands, U.S.",
    "VN" => "Vietnam",
    "VU" => "Vanuatu",
    "WF" => "Wallis and Futuna",
    "WS" => "Samoa",
    "XX" => "",
    "YE" => "Yemen",
    "YT" => "Mayotte",
    "ZA" => "South Africa",
    "ZM" => "Zambia",
    "ZW" => "Zimbabwe",
};

fn country_name(code: &str) -> &'static str {
    COUNTRY_NAMES.get(code).copied().unwrap_or("Unknown")
}

/// 将用户统计数据与变化量格式化为带变化标注（↑↓/加减）的中文个人信息文本。
#[must_use]
pub fn format_stats_with_change(
    stats: &UserStats,
    change: &Option<UserChange>,
    mode: GameMode,
) -> String {
    let c = collect_common(stats, mode);
    let rank = stats.rank.to_string();

    let rank_str = match change {
        Some(c) if c.rank_change != Some(0) => {
            let change_str = format_change(c.rank_change);
            format!("{rank} ({change_str})")
        }
        _ => rank.to_string(),
    };

    let country_rank = stats.country_rank.to_string();
    let country = country_name(&stats.country_code);
    let country_rank_change_str = match change {
        Some(c) if c.country_rank_change != Some(0) => {
            let change_str = format_change(c.country_rank_change);
            format!("{country} #{country_rank} ({change_str})")
        }
        _ => format!("{country} #{country_rank}"),
    };

    let pp_change_str = format_float_change(
        change
            .as_ref()
            .and_then(|c| c.pp_change.filter(|&v| v != 0.0)),
        "",
    );
    let acc_change_str = format_float_change(
        change
            .as_ref()
            .and_then(|c| c.accuracy_change.filter(|&v| v != 0.0)),
        "%",
    );

    let playcount_change_str = format_small_change(
        change
            .as_ref()
            .and_then(|c| c.playcount_change.filter(|&v| v != 0)),
    );
    let hits_change_str = format_small_change(
        change
            .as_ref()
            .and_then(|c| c.hits_change.filter(|&v| v != 0)),
    );
    let playtime_change_str = format_small_change(
        change
            .as_ref()
            .and_then(|c| c.playtime_change.filter(|&v| v != 0)),
    );

    let pp_change_str = space_prefix(pp_change_str);
    let acc_change_str = space_prefix(acc_change_str);
    let playcount_change_str = space_prefix(playcount_change_str);
    let hits_change_str = space_prefix(hits_change_str);
    let playtime_change_str = space_prefix(playtime_change_str);

    format!(
        "{username}{header}{mode_name}\n\n\
         {pp}{pp_label}{pp_change_str}\n\
         #{rank_str}\n\
         {country_rank_change_str}\n\
         {ranked_score}{score_label}\n\
         {acc}{acc_label}{acc_change_str}\n\
         {playcount}{playcount_label}{playcount_change_str}\n\
         {hits}{hits_label}{hits_change_str}\n\
         {playtime}{playtime_label}{playtime_change_str}",
        username = c.username,
        header = user_str("fmt.profile_header"),
        mode_name = c.mode_name,
        pp = c.pp,
        pp_label = user_str("fmt.profile_pp"),
        pp_change_str = pp_change_str,
        rank_str = rank_str,
        country_rank_change_str = country_rank_change_str,
        ranked_score = c.ranked_score,
        score_label = user_str("fmt.profile_ranked_score"),
        acc = c.acc,
        acc_label = user_str("fmt.profile_accuracy"),
        acc_change_str = acc_change_str,
        playcount = c.playcount,
        playcount_label = user_str("fmt.profile_playcount"),
        playcount_change_str = playcount_change_str,
        hits = c.hits,
        hits_label = user_str("fmt.profile_hits"),
        hits_change_str = hits_change_str,
        playtime = c.playtime,
        playtime_label = user_str("fmt.profile_playtime"),
        playtime_change_str = playtime_change_str,
    )
}

struct CommonStats<'a> {
    username: &'a str,
    mode_name: String,
    pp: String,
    ranked_score: String,
    acc: String,
    playcount: String,
    hits: String,
    playtime: String,
}

fn collect_common(stats: &UserStats, mode: GameMode) -> CommonStats<'_> {
    CommonStats {
        username: &stats.username,
        mode_name: mode.name().to_string(),
        pp: trim_trailing_zeros(&format!("{:.2}", stats.pp)),
        ranked_score: format_number((stats.ranked_score as f64 / 1_000_000.0).round() as i64),
        acc: format_accuracy((stats.accuracy / 100.0 * 10000.0).round() / 10000.0),
        playcount: stats.playcount.to_string(),
        hits: format_number(stats.hits),
        playtime: format_playtime(stats.playtime),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Score, ScoreStatistics, ScoreUser};
    use rosu_mods::GameMods;

    fn make_score(is_perfect: bool) -> Score {
        let mut mods = GameMods::new();
        mods.insert(rosu_mods::GameMod::HiddenOsu(Default::default()));
        mods.insert(rosu_mods::GameMod::HardRockOsu(Default::default()));
        Score {
            score_id: 88888,
            beatmap_id: 100,
            beatmapset_id: 200,
            artist: "Artist".to_string(),
            title: "Song".to_string(),
            version: "Expert".to_string(),
            creator: "Mapper".to_string(),
            star_rating: 6.5,
            bpm: 200.0,
            ar: 9.5,
            od: 8.5,
            cs: 4.0,
            hp: 5.0,
            length_seconds: 180,
            score_value: 900000,
            accuracy: 0.985,
            max_combo: 400,
            beatmap_max_combo: 500,
            pp: Some(250.0),
            pp_breakdown: None,
            pp_if_acc: None,
            perfect_pp: None,
            rank: "S".to_string(),
            passed: true,
            mods,
            is_perfect,
            created_at: "2024-01-01T00:00:00Z".to_string(),
            is_lazer: false,
            has_replay: true,
            legacy_score_id: None,
            statistics: ScoreStatistics {
                count_geki: 0,
                count_300: 300,
                count_katu: 0,
                count_100: 5,
                count_50: 0,
                count_miss: 0,
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
            },
            cover_url: String::new(),
            user: ScoreUser {
                avatar_url: String::new(),
                country_code: String::new(),
                user_id: None,
                username: None,
                global_rank: None,
                country_rank: None,
                pp: 0.0,
            },
            fav_count: None,
            play_count: None,
            status: "ranked".to_string(),
        }
    }

    #[test]
    fn test_format_score_single_pass() {
        let score = make_score(false);
        let output = format_score(&score, "TestUser", GameMode::Osu, Some(0), true);
        assert!(output.contains("Artist - Song [Expert]"));
        assert!(output.contains("★★★★★★☆ 6.50* [3:00]"));
        assert!(output.contains("TestUser (osu): 250.00pp"));
        assert!(output.contains("[S] 900,000"));
        assert!(output.contains("400x/500x // 98.5%"));
        assert!(output.contains("300/5/0/0 | HD HR"));
        assert!(output.contains("#1 最近通过"));
        assert!(output.contains("ID: 100"));
    }

    #[test]
    fn test_format_score_single_recent() {
        let score = make_score(false);
        let output = format_score(&score, "TestUser", GameMode::Osu, Some(2), false);
        assert!(output.contains("#3 最近游玩"));
        assert!(output.contains("ID: 100"));
    }

    #[test]
    fn test_format_score_pp_null() {
        let mut score = make_score(false);
        score.pp = None;
        let output = format_score(&score, "TestUser", GameMode::Osu, None, true);
        assert!(output.contains("TestUser (osu): --pp"));
    }

    #[test]
    fn test_format_score_no_mods() {
        let mut score = make_score(false);
        score.mods = GameMods::new();
        let output = format_score(&score, "TestUser", GameMode::Osu, None, true);
        assert!(output.contains("300/5/0/0\n"));
    }

    #[test]
    fn test_format_score_is_perfect() {
        let score = make_score(true);
        let output = format_score(&score, "TestUser", GameMode::Osu, None, true);
        assert!(output.contains("400x/500x"));
    }

    #[test]
    fn test_format_score_lazer_tag_omitted() {
        let mut score = make_score(false);
        score.is_lazer = true;
        let output = format_score(&score, "TestUser", GameMode::Osu, None, true);
        assert!(!output.contains("(lazer)"));
    }

    #[test]
    fn test_format_score_play_time() {
        let score = make_score(false);
        let output = format_score(&score, "TestUser", GameMode::Osu, None, true);
        assert!(output.contains("2024/01/01 08:00:00"));
    }

    #[test]
    fn test_format_scores_mods_and_time() {
        let scores = vec![make_score(false), make_score(true)];
        let output = format_scores(&scores, "TestUser", GameMode::Osu, true);
        assert!(output.contains("HD HR"));
        assert!(output.contains("2024/01/01 08:00:00"));
        assert!(output.contains("#1 Artist - Song [Expert]"));
        assert!(output.contains("#2 Artist - Song [Expert]"));
        assert!(output.contains("400x/500x"));
    }

    #[test]
    fn test_format_star_rating_boundaries() {
        assert_eq!(format_star_rating(0.0), " 0.00*");
        assert_eq!(format_star_rating(5.0), "★★★★★ 5.00*");
        assert_eq!(format_star_rating(5.49), "★★★★★ 5.49*");
        assert_eq!(format_star_rating(5.5), "★★★★★☆ 5.50*");
        assert_eq!(format_star_rating(5.99), "★★★★★☆ 5.99*");
        assert_eq!(format_star_rating(14.3), "★★★★★★★★★★★★★★ 14.30*");
    }

    #[test]
    fn test_format_change_positive_shows_up() {
        assert_eq!(format_change(Some(2)), "↑2");
    }

    #[test]
    fn test_format_change_negative_shows_down() {
        assert_eq!(format_change(Some(-3)), "↓3");
    }

    #[test]
    fn test_format_change_zero_shows_empty() {
        assert_eq!(format_change(Some(0)), "");
        assert_eq!(format_change(None), "");
    }

    #[test]
    fn test_format_score_mania_hits() {
        let mut score = make_score(false);
        score.statistics = ScoreStatistics {
            count_geki: 100,
            count_300: 200,
            count_katu: 50,
            count_100: 30,
            count_50: 10,
            count_miss: 5,
            osu_large_tick_hits: 0,
            osu_small_tick_hits: 0,
            osu_slider_tail_hits: 0,
            osu_large_tick_misses: 0,
            osu_small_tick_misses: 0,
        };
        let output = format_score(&score, "TestUser", GameMode::Mania, None, true);
        assert!(output.contains("100/200/50/30/10/5"));
    }

    #[test]
    fn format_star_rating_negative_stars() {
        let result = format_star_rating(-1.5);
        assert!(result.contains("0.0") || result.contains("-1.5"));
        assert!(!result.contains("\u{2605}"));
    }

    #[test]
    fn format_star_rating_extreme_stars() {
        let result = format_star_rating(999.9);
        let star_count = result.matches('\u{2605}').count();
        assert!(star_count <= 30);
    }
}
