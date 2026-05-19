#![deny(clippy::all)]
#![allow(clippy::derive_partial_eq_without_eq)]

use crate::types::{GameMode, UserChange, UserStats};

/// 格式化用户数据为响应字符串
pub fn format_stats(stats: &UserStats, mode: GameMode) -> String {
    let username = &stats.username;
    let mode_name = mode.name();

    let pp = trim_trailing_zeros(format!("{:.2}", stats.pp));
    let rank = stats.rank.to_string();
    let country_rank = stats.country_rank.to_string();
    let ranked_score = format_number((stats.ranked_score as f64 / 1_000_000.0).round() as i64); // 转成 m
    let acc = trim_trailing_zeros(format!("{:.2}", stats.accuracy));
    let playcount = stats.playcount.to_string();
    let hits = format_number(stats.hits);
    let playtime = format_playtime(stats.playtime);

    let rank_change = format_change(stats.rank_change);
    let country_rank_change = format_change(stats.country_rank_change);

    let rank_str =
        if rank_change.is_empty() { rank.to_string() } else { format!("{rank} ({rank_change})") };
    let country = country_name(&stats.country_code);
    let country_rank_str = if country_rank_change.is_empty() {
        format!("{country} #{country_rank}")
    } else {
        format!("{country} #{country_rank} ({country_rank_change})")
    };

    format!(
        "{username}的个人信息—{mode_name}\n\n\
         {pp}pp 表现\n\
         #{rank_str}\n\
         {country_rank_str}\n\
         {ranked_score}m Ranked谱面总分\n\
         {acc}% 准确率\n\
         {playcount} 游玩次数\n\
         {hits} 总命中次数\n\
         {playtime}游玩时间",
        username = username,
        mode_name = mode_name,
        pp = pp,
        rank_str = rank_str,
        country_rank_str = country_rank_str,
        ranked_score = ranked_score,
        acc = acc,
        playcount = playcount,
        hits = hits,
        playtime = playtime,
    )
}

/// 格式化整数，加千位分隔符
fn format_number<T: itoa::Integer>(value: T) -> String {
    let mut buf = itoa::Buffer::new();
    let formatted = buf.format(value);

    let chars: Vec<char> = formatted.chars().collect();
    let len = chars.len();

    let mut result = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(*c);
    }

    result
}

/// 格式化浮点数，加千位分隔符和小数位
#[allow(dead_code)]
fn format_float(value: f64, decimals: usize) -> String {
    let int_part = value as i64;

    // 格式整数部分
    let mut int_buf = itoa::Buffer::new();
    let int_formatted = int_buf.format(int_part);

    let chars: Vec<char> = int_formatted.chars().collect();
    let len = chars.len();

    let mut int_result = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            int_result.push(',');
        }
        int_result.push(*c);
    }

    // 格式小数部分
    let multiplier = 10_f64.powi(decimals as i32);
    let dec_part = ((value - int_part as f64) * multiplier).round() as i64;

    let mut dec_buf = itoa::Buffer::new();
    let dec_formatted = dec_buf.format(dec_part);

    format!("{}.{:0>width$}", int_result, dec_formatted, width = decimals)
}

/// 去除数字字符串末尾的无效零（如 33570.10 → 33570.1）
fn trim_trailing_zeros(s: String) -> String {
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 格式化排名变化
fn format_change(change: Option<i64>) -> String {
    match change {
        Some(c) if c > 0 => format!("↑{}", c),
        Some(c) if c < 0 => format!("↓{}", c.abs()),
        _ => String::new(),
    }
}

/// 格式化游玩时间
fn format_playtime(seconds: i64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;
    format!("{} 小时 {} 分钟 {} 秒", hours, minutes, secs)
}

/// 格式化小变化（用于 playcount, hits, playtime）
fn format_small_change(change: Option<i64>) -> String {
    match change {
        Some(c) if c > 0 => format!("(+{})", format_number(c)),
        Some(c) if c < 0 => format!("(-{})", format_number(c.abs())),
        _ => String::new(),
    }
}

/// 非空字符串加空格前缀
fn space_prefix(s: String) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!(" {}", s)
    }
}

/// 格式化浮点变化（用于 pp, accuracy）(+X.XX) / (-X.XX)
fn format_float_change(change: Option<f64>, suffix: &str) -> String {
    match change {
        Some(c) if c != 0.0 => {
            let abs = c.abs();
            let sign = if c > 0.0 { "+" } else { "-" };
            let formatted = format!("{:.2}", abs);
            let trimmed = trim_trailing_zeros(formatted);
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

// country_code to full name mapping (complete list from osu!)
fn country_name(code: &str) -> &'static str {
    match code {
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
        _ => "Unknown", // Return Unknown if code not found
    }
}

/// 格式化带变化统计的用户数据
pub fn format_stats_with_change(
    stats: &UserStats,
    change: &Option<UserChange>,
    mode: GameMode,
) -> String {
    let username = &stats.username;
    let mode_name = mode.name();

    let pp = trim_trailing_zeros(format!("{:.2}", stats.pp));
    let rank = stats.rank.to_string();
    let ranked_score = format_number((stats.ranked_score as f64 / 1_000_000.0).round() as i64); // 转成 m
    let acc = trim_trailing_zeros(format!("{:.2}", stats.accuracy));
    let playcount = stats.playcount.to_string();
    let hits = format_number(stats.hits);
    let playtime = format_playtime(stats.playtime);

    // 从 change 中提取，如果没有 change 或 change 为 0 则显示 —
    let rank_str = match change {
        Some(c) if c.rank_change != Some(0) => {
            let change_str = format_change(c.rank_change);
            format!("{rank} ({change_str})")
        }
        _ => rank.to_string(),
    };

    // country_rank 显示为 {Country Name} #数字 格式，有变化时显示 (change)，无变化时不显示括号
    let country_rank = stats.country_rank.to_string();
    let country = country_name(&stats.country_code);
    let country_rank_change_str = match change {
        Some(c) if c.country_rank_change != Some(0) => {
            let change_str = format_change(c.country_rank_change);
            format!("{country} #{country_rank} ({change_str})")
        }
        _ => format!("{country} #{country_rank}"),
    };

    // pp 和 accuracy 变化格式化
    let pp_change_str =
        format_float_change(change.as_ref().and_then(|c| c.pp_change.filter(|&v| v != 0.0)), "");
    let acc_change_str = format_float_change(
        change.as_ref().and_then(|c| c.accuracy_change.filter(|&v| v != 0.0)),
        "%",
    );

    let playcount_change_str =
        format_small_change(change.as_ref().and_then(|c| c.playcount_change.filter(|&v| v != 0)));
    let hits_change_str =
        format_small_change(change.as_ref().and_then(|c| c.hits_change.filter(|&v| v != 0)));
    let playtime_change_str =
        format_small_change(change.as_ref().and_then(|c| c.playtime_change.filter(|&v| v != 0)));

    // Add space before change string if non-empty
    let pp_change_str = space_prefix(pp_change_str);
    let acc_change_str = space_prefix(acc_change_str);
    let playcount_change_str = space_prefix(playcount_change_str);
    let hits_change_str = space_prefix(hits_change_str);
    let playtime_change_str = space_prefix(playtime_change_str);

    format!(
        "{username}的个人信息—{mode_name}\n\n\
         {pp}pp 表现{pp_change_str}\n\
         #{rank_str}\n\
         {country_rank_change_str}\n\
         {ranked_score}m Ranked谱面总分\n\
         {acc}% 准确率{acc_change_str}\n\
         {playcount} 游玩次数{playcount_change_str}\n\
         {hits} 总命中次数{hits_change_str}\n\
         {playtime}游玩时间{playtime_change_str}",
        username = username,
        mode_name = mode_name,
        pp = pp,
        pp_change_str = pp_change_str,
        rank_str = rank_str,
        country_rank_change_str = country_rank_change_str,
        ranked_score = ranked_score,
        acc = acc,
        acc_change_str = acc_change_str,
        playcount = playcount,
        playcount_change_str = playcount_change_str,
        hits = hits,
        hits_change_str = hits_change_str,
        playtime = playtime,
        playtime_change_str = playtime_change_str,
    )
}
