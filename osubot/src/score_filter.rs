use osubot_core::types::Score;

pub(crate) struct ScoreQueryParams<'a> {
    pub username: &'a Option<String>,
    pub qq: &'a Option<i64>,
    pub is_pass: bool,
    pub beatmap_id: Option<u32>,
    pub score_id: Option<u64>,
    pub limit: u32,
    pub is_single: bool,
    pub limit_end: Option<u32>,
    pub filters: Option<&'a [String]>,
}

/// Comparison operator extracted from a `key<op>value` filter token.
/// `=` maps to `Eq` and `==` maps to `EqEq`. For numeric keys the two
/// have identical semantics (equality), but for the `mod` key `Eq` means
/// "subset" (score's mods must include all required mods) and `EqEq`
/// means "exact set" (score's mods must match the required set exactly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterOp {
    Eq,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
}

/// Parse a single filter token of the form `key<op>value` where `<op>`
/// is one of `=`, `==`, `!=`, `<`, `<=`, `>`, `>=`. The operator must
/// be glued to both key and value (no surrounding spaces).
///
/// Returns `None` for malformed input (empty key, empty value, or no
/// recognized operator).
pub(crate) fn parse_filter_token(token: &str) -> Option<(String, FilterOp, String)> {
    // Two-character operators must be tried before single-character ones
    // to avoid `>=` being misread as `>` with value `=5`.
    const TWO_CHAR_OPS: &[(&str, FilterOp)] = &[
        ("==", FilterOp::EqEq),
        (">=", FilterOp::GtEq),
        ("<=", FilterOp::LtEq),
        ("!=", FilterOp::NotEq),
    ];
    for (op_str, op) in TWO_CHAR_OPS {
        if let Some(idx) = token.find(op_str) {
            let key = &token[..idx];
            let value = &token[idx + op_str.len()..];
            if key.is_empty() {
                continue;
            }
            if value.is_empty() {
                return None;
            }
            return Some((key.to_string(), *op, value.to_string()));
        }
    }

    const ONE_CHAR_OPS: &[(char, FilterOp)] = &[
        ('=', FilterOp::Eq),
        ('>', FilterOp::Gt),
        ('<', FilterOp::Lt),
    ];
    for (op_char, op) in ONE_CHAR_OPS {
        if let Some(idx) = token.find(*op_char) {
            let key = &token[..idx];
            let value = &token[idx + 1..];
            if key.is_empty() {
                continue;
            }
            if value.is_empty() {
                return None;
            }
            return Some((key.to_string(), *op, value.to_string()));
        }
    }

    None
}

/// Strict integer comparison.
pub(crate) fn cmp_i64(a: i64, b: i64, op: FilterOp) -> bool {
    match op {
        FilterOp::Eq | FilterOp::EqEq => a == b,
        FilterOp::NotEq => a != b,
        FilterOp::Lt => a < b,
        FilterOp::LtEq => a <= b,
        FilterOp::Gt => a > b,
        FilterOp::GtEq => a >= b,
    }
}

/// Float comparison. `Eq` / `NotEq` use the given `tol` tolerance
/// (to handle display precision and FP rounding). Ordering operators
/// (`<`, `<=`, `>`, `>=`) are strict — tolerance on inequality would
/// degrade `pp>500` into `pp>=499.5`, which is surprising.
pub(crate) fn cmp_f64(a: f64, b: f64, op: FilterOp, tol: f64) -> bool {
    match op {
        FilterOp::Eq | FilterOp::EqEq => (a - b).abs() < tol,
        FilterOp::NotEq => (a - b).abs() >= tol,
        FilterOp::Lt => a < b,
        FilterOp::LtEq => a <= b,
        FilterOp::Gt => a > b,
        FilterOp::GtEq => a >= b,
    }
}

/// Parse a `mod=` filter value (e.g. `HDDT`, `HD,DT`) into a list of
/// 2-character mod acronyms. Returns `None` if the input has an odd
/// number of characters after splitting on commas (treat as parse error).
/// An empty input returns `Some(vec![])`.
pub(crate) fn parse_mod_filter(value: &str) -> Option<Vec<String>> {
    if value.is_empty() {
        return Some(Vec::new());
    }
    // "NM" is treated as a "no mod" marker (label only; osu! has no NM mod).
    // It produces an empty required set so `mod==NM` matches scores with zero mods.
    if value.trim().eq_ignore_ascii_case("NM") {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for part in value.split(',') {
        let upper = part.trim().to_uppercase();
        let chars: Vec<char> = upper.chars().collect();
        if !chars.len().is_multiple_of(2) {
            return None;
        }
        for chunk in chars.chunks(2) {
            out.push(chunk.iter().collect::<String>());
        }
    }
    Some(out)
}

pub(crate) fn score_matches_filters(score: &Score, filters: &[String]) -> bool {
    for filter in filters {
        // Special case: "mod=" with empty value means "no required mods" → passes.
        // parse_filter_token rejects empty values, so we handle this here to
        // preserve the legacy behavior (split_once('=') used to allow it).
        if filter == "mod=" {
            continue;
        }
        let Some((key, op, value)) = parse_filter_token(filter) else {
            return false;
        };
        if !apply_filter(score, &key, op, &value) {
            return false;
        }
    }
    true
}

pub(crate) fn apply_filter(score: &Score, key: &str, op: FilterOp, value: &str) -> bool {
    match key {
        "miss" => value
            .parse::<i64>()
            .is_ok_and(|v| cmp_i64(score.statistics.count_miss, v, op)),
        "combo" => value
            .parse::<i64>()
            .is_ok_and(|v| cmp_i64(score.max_combo, v, op)),
        "pp" => value
            .parse::<f64>()
            .ok()
            .and_then(|v| score.pp.map(|p| cmp_f64(p, v, op, 0.5)))
            .unwrap_or(false),
        "score" => value
            .parse::<i64>()
            .is_ok_and(|v| cmp_i64(score.score_value, v, op)),
        "acc" | "accuracy" => value
            .parse::<f64>()
            .is_ok_and(|v| cmp_f64(score.accuracy * 100.0, v, op, 0.5)),
        "mod" => apply_mod_filter(score, op, value),
        _ => true, // 未知 key 静默忽略（与现有行为一致）
    }
}

pub(crate) fn apply_mod_filter(score: &Score, op: FilterOp, value: &str) -> bool {
    // Comparison operators (>, <, >=, <=) on the mod key have no
    // set-membership semantics, so they silently pass without
    // touching the score.
    if matches!(
        op,
        FilterOp::Gt | FilterOp::Lt | FilterOp::GtEq | FilterOp::LtEq
    ) {
        return true;
    }
    let required = match parse_mod_filter(value) {
        Some(v) => v,
        None => return false, // 奇数长度等解析失败
    };
    let present: Vec<String> = score
        .mods
        .iter()
        .map(|m| m.acronym().as_str().to_string())
        .collect();
    let subset = required.iter().all(|r| present.contains(r));
    match op {
        FilterOp::Eq => subset, // = 子集（必须包含所有列出的 mod）
        FilterOp::EqEq => {
            // == 精确集合（分数的 mod 集必须恰好等于）
            subset && present.len() == required.len()
        }
        FilterOp::NotEq => !subset, // != 子集取反
        FilterOp::Gt | FilterOp::Lt | FilterOp::GtEq | FilterOp::LtEq => unreachable!(),
    }
}

#[cfg(test)]
mod filter_tests {
    use super::*;
    use osubot_core::types::{ScoreStatistics, ScoreUser};
    use rosu_mods::GameMods;

    fn make_score(mods: GameMods) -> Score {
        Score {
            score_id: 1,
            beatmap_id: 1,
            beatmapset_id: 1,
            artist: String::new(),
            title: String::new(),
            version: String::new(),
            creator: String::new(),
            star_rating: 0.0,
            bpm: 0.0,
            ar: 0.0,
            od: 0.0,
            cs: 0.0,
            hp: 0.0,
            length_seconds: 0,
            score_value: 0,
            accuracy: 1.0,
            max_combo: 0,
            beatmap_max_combo: 0,
            pp: None,
            pp_breakdown: None,
            pp_if_acc: None,
            perfect_pp: None,
            rank: String::new(),
            passed: true,
            mods,
            is_perfect: false,
            created_at: String::new(),
            is_lazer: false,
            has_replay: false,
            legacy_score_id: None,
            statistics: ScoreStatistics {
                count_geki: 0,
                count_300: 0,
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
            status: String::new(),
        }
    }

    fn mods_with(acronyms: &[&str]) -> GameMods {
        let mut mods = GameMods::new();
        for a in acronyms {
            let m = rosu_mods::GameMod::new(*a, rosu_mods::GameMode::Osu);
            mods.insert(m);
        }
        mods
    }

    #[test]
    fn parse_token_eq() {
        let r = parse_filter_token("miss=0");
        assert_eq!(r, Some(("miss".to_string(), FilterOp::Eq, "0".to_string())));
    }

    #[test]
    fn parse_token_eqeq() {
        let r = parse_filter_token("miss==0");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::EqEq, "0".to_string()))
        );
    }

    #[test]
    fn parse_token_noteq() {
        let r = parse_filter_token("miss!=0");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::NotEq, "0".to_string()))
        );
    }

    #[test]
    fn parse_token_gt() {
        let r = parse_filter_token("miss>0");
        assert_eq!(r, Some(("miss".to_string(), FilterOp::Gt, "0".to_string())));
    }

    #[test]
    fn parse_token_lt() {
        let r = parse_filter_token("miss<10");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::Lt, "10".to_string()))
        );
    }

    #[test]
    fn parse_token_gteq() {
        // >= must be matched before > alone
        let r = parse_filter_token("miss>=5");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::GtEq, "5".to_string()))
        );
    }

    #[test]
    fn parse_token_lteq() {
        let r = parse_filter_token("miss<=10");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::LtEq, "10".to_string()))
        );
    }

    #[test]
    fn parse_token_negative_value() {
        let r = parse_filter_token("miss>-5");
        assert_eq!(
            r,
            Some(("miss".to_string(), FilterOp::Gt, "-5".to_string()))
        );
    }

    #[test]
    fn parse_token_mod() {
        let r = parse_filter_token("mod=HDDT");
        assert_eq!(
            r,
            Some(("mod".to_string(), FilterOp::Eq, "HDDT".to_string()))
        );
    }

    #[test]
    fn parse_token_mod_eqeq() {
        let r = parse_filter_token("mod==DT");
        assert_eq!(
            r,
            Some(("mod".to_string(), FilterOp::EqEq, "DT".to_string()))
        );
    }

    #[test]
    fn parse_token_mod_noteq() {
        let r = parse_filter_token("mod!=DT");
        assert_eq!(
            r,
            Some(("mod".to_string(), FilterOp::NotEq, "DT".to_string()))
        );
    }

    #[test]
    fn parse_token_empty_value_rejected() {
        assert_eq!(parse_filter_token("miss="), None);
        assert_eq!(parse_filter_token("miss>="), None);
        assert_eq!(parse_filter_token("miss=="), None);
    }

    #[test]
    fn parse_token_empty_key_rejected() {
        assert_eq!(parse_filter_token("=0"), None);
        assert_eq!(parse_filter_token(">0"), None);
        assert_eq!(parse_filter_token("==0"), None);
    }

    #[test]
    fn parse_token_no_operator_rejected() {
        assert_eq!(parse_filter_token("miss0"), None);
        assert_eq!(parse_filter_token(""), None);
    }

    // === Integer key × 6 operators ===

    fn score_with_miss(miss: i64) -> Score {
        let mut s = make_score(GameMods::new());
        s.statistics.count_miss = miss;
        s
    }

    fn score_with_combo(combo: i64) -> Score {
        let mut s = make_score(GameMods::new());
        s.max_combo = combo;
        s
    }

    fn score_with_pp(pp: f64) -> Score {
        let mut s = make_score(GameMods::new());
        s.pp = Some(pp);
        s
    }

    fn score_with_acc(acc: f64) -> Score {
        let mut s = make_score(GameMods::new());
        s.accuracy = acc;
        s
    }

    fn score_with_score_value(v: i64) -> Score {
        let mut s = make_score(GameMods::new());
        s.score_value = v;
        s
    }

    #[test]
    fn miss_eq() {
        let s = score_with_miss(5);
        assert!(score_matches_filters(&s, &["miss=5".to_string()]));
        assert!(score_matches_filters(&s, &["miss==5".to_string()]));
        assert!(!score_matches_filters(&s, &["miss=4".to_string()]));
    }

    #[test]
    fn miss_noteq() {
        let s = score_with_miss(5);
        assert!(score_matches_filters(&s, &["miss!=4".to_string()]));
        assert!(!score_matches_filters(&s, &["miss!=5".to_string()]));
    }

    #[test]
    fn miss_ordering() {
        let s = score_with_miss(5);
        assert!(score_matches_filters(&s, &["miss>4".to_string()]));
        assert!(score_matches_filters(&s, &["miss>=5".to_string()]));
        assert!(!score_matches_filters(&s, &["miss>5".to_string()]));
        assert!(score_matches_filters(&s, &["miss<6".to_string()]));
        assert!(score_matches_filters(&s, &["miss<=5".to_string()]));
        assert!(!score_matches_filters(&s, &["miss<5".to_string()]));
    }

    #[test]
    fn combo_eq() {
        let s = score_with_combo(500);
        assert!(score_matches_filters(&s, &["combo=500".to_string()]));
        assert!(!score_matches_filters(&s, &["combo=501".to_string()]));
    }

    #[test]
    fn combo_noteq() {
        let s = score_with_combo(500);
        assert!(score_matches_filters(&s, &["combo!=501".to_string()]));
    }

    #[test]
    fn combo_ordering() {
        let s = score_with_combo(500);
        assert!(score_matches_filters(&s, &["combo>499".to_string()]));
        assert!(score_matches_filters(&s, &["combo>=500".to_string()]));
        assert!(score_matches_filters(&s, &["combo<501".to_string()]));
        assert!(score_matches_filters(&s, &["combo<=500".to_string()]));
    }

    #[test]
    fn score_value_eq() {
        let s = score_with_score_value(1_000_000);
        assert!(score_matches_filters(&s, &["score=1000000".to_string()]));
        assert!(!score_matches_filters(&s, &["score=999999".to_string()]));
    }

    #[test]
    fn score_value_ordering() {
        let s = score_with_score_value(1_000_000);
        assert!(score_matches_filters(&s, &["score>999999".to_string()]));
        assert!(!score_matches_filters(&s, &["score<999999".to_string()]));
    }

    // === Float key (pp) — tolerance 0.5 ===

    #[test]
    fn pp_eq_tolerance() {
        let s = score_with_pp(500.4);
        // |500.4 - 500| = 0.4 < 0.5
        assert!(score_matches_filters(&s, &["pp=500".to_string()]));
        assert!(score_matches_filters(&s, &["pp==500".to_string()]));
        let s = score_with_pp(500.6);
        // |500.6 - 500| = 0.6 >= 0.5
        assert!(!score_matches_filters(&s, &["pp=500".to_string()]));
    }

    #[test]
    fn pp_noteq_tolerance() {
        let s = score_with_pp(500.6);
        assert!(score_matches_filters(&s, &["pp!=500".to_string()]));
        let s = score_with_pp(500.4);
        assert!(!score_matches_filters(&s, &["pp!=500".to_string()]));
    }

    #[test]
    fn pp_ordering_strict() {
        let s = score_with_pp(500.0);
        // Strict: 500.0 is NOT > 500.0
        assert!(!score_matches_filters(&s, &["pp>500".to_string()]));
        assert!(score_matches_filters(&s, &["pp>=500".to_string()]));
        assert!(!score_matches_filters(&s, &["pp<500".to_string()]));
        assert!(score_matches_filters(&s, &["pp<=500".to_string()]));
    }

    #[test]
    fn pp_ordering_tolerance_does_not_apply() {
        // 499.6 is within == tolerance of 500 (|499.6-500|=0.4 < 0.5),
        // so it matches `pp=500`. But strict ordering must reject
        // `pp>500`: 499.6 is not strictly > 500. If `>` were degraded
        // to `a > b - tol = a > 499.5`, the assertion would fail.
        let s = score_with_pp(499.6);
        assert!(score_matches_filters(&s, &["pp=500".to_string()]));
        assert!(!score_matches_filters(&s, &["pp>500".to_string()]));

        // Symmetric case: 500.4 is within == tolerance of 500 and
        // strictly < 501. Strict `<500` must reject it. If `<` were
        // degraded to `a < b + tol = a < 500.5`, the assertion would fail.
        let s2 = score_with_pp(500.4);
        assert!(score_matches_filters(&s2, &["pp=500".to_string()]));
        assert!(!score_matches_filters(&s2, &["pp<500".to_string()]));
    }

    // === Float key (acc) ===

    #[test]
    fn acc_eq_tolerance() {
        // accuracy is stored as fraction; 95.5% → 0.955
        let s = score_with_acc(0.954);
        // 0.954 * 100 = 95.4, |95.4 - 95.5| = 0.1 < 0.5
        assert!(score_matches_filters(&s, &["acc=95.5".to_string()]));
        let s = score_with_acc(0.946);
        // 94.6, |94.6 - 95.5| = 0.9 >= 0.5
        assert!(!score_matches_filters(&s, &["acc=95.5".to_string()]));
    }

    #[test]
    fn acc_alias_accuracy() {
        let s = score_with_acc(0.955);
        assert!(score_matches_filters(&s, &["accuracy=95.5".to_string()]));
    }

    #[test]
    fn acc_ordering_strict() {
        let s = score_with_acc(0.95);
        assert!(score_matches_filters(&s, &["acc>90".to_string()]));
        assert!(score_matches_filters(&s, &["acc>=95".to_string()]));
        assert!(!score_matches_filters(&s, &["acc>95".to_string()]));
    }

    #[test]
    fn mod_filter_matches_single_mod() {
        let score = make_score(mods_with(&["HD"]));
        assert!(score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_does_not_match_missing_mod() {
        let score = make_score(mods_with(&["DT"]));
        assert!(!score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_subset_match() {
        // score has HDDT; mod=HD should still match (subset)
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_combined_concat() {
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&score, &["mod=HDDT".to_string()]));
    }

    #[test]
    fn mod_filter_combined_concat_no_match() {
        // score has only HD; mod=HDDT requires DT too
        let score = make_score(mods_with(&["HD"]));
        assert!(!score_matches_filters(&score, &["mod=HDDT".to_string()]));
    }

    #[test]
    fn mod_filter_comma_separated() {
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&score, &["mod=HD,DT".to_string()]));
    }

    #[test]
    fn mod_filter_no_mods_score() {
        let score = make_score(GameMods::new());
        assert!(!score_matches_filters(&score, &["mod=HD".to_string()]));
    }

    #[test]
    fn mod_filter_combines_with_other_keys() {
        // AND semantics: mod=HD AND miss=0
        let score = make_score(mods_with(&["HD"]));
        assert!(score_matches_filters(
            &score,
            &["mod=HD".to_string(), "miss=0".to_string()]
        ));
    }

    #[test]
    fn mod_filter_odd_length_fails_match() {
        // mod=HDT (odd length) → entry cannot be parsed → false
        let score = make_score(mods_with(&["HD", "DT"]));
        assert!(!score_matches_filters(&score, &["mod=HDT".to_string()]));
    }

    #[test]
    fn mod_filter_empty_value_no_op() {
        // mod= with no mods → no required mods → passes
        let score = make_score(GameMods::new());
        assert!(score_matches_filters(&score, &["mod=".to_string()]));
    }

    #[test]
    fn mod_eqeq_exact_match() {
        // 纯 DT 分数匹配 mod==DT
        let s = make_score(mods_with(&["DT"]));
        assert!(score_matches_filters(&s, &["mod==DT".to_string()]));
    }

    #[test]
    fn mod_eqeq_does_not_match_superset() {
        // HDDT 不匹配 mod==DT
        let s = make_score(mods_with(&["HD", "DT"]));
        assert!(!score_matches_filters(&s, &["mod==DT".to_string()]));
    }

    #[test]
    fn mod_eqeq_empty_required() {
        // mod==NM: required set is empty → exact match means score has no mods
        // (and no extras). Spec README example: !ps mod==NM.

        // No mods → matches mod==NM
        let s = make_score(GameMods::new());
        assert!(score_matches_filters(&s, &["mod==NM".to_string()]));

        // Has any mod → does NOT match mod==NM
        let s = make_score(mods_with(&["HD"]));
        assert!(!score_matches_filters(&s, &["mod==NM".to_string()]));
    }

    #[test]
    fn mod_eq_subset_still_works() {
        // mod=DT (单 =) 是子集匹配
        let s = make_score(mods_with(&["HD", "DT"]));
        assert!(score_matches_filters(&s, &["mod=DT".to_string()]));
    }

    #[test]
    fn mod_noteq_negation_of_subset() {
        // 纯 HD 不包含 DT → 匹配 mod!=DT
        let s = make_score(mods_with(&["HD"]));
        assert!(score_matches_filters(&s, &["mod!=DT".to_string()]));

        // 纯 DT 包含 DT → 不匹配 mod!=DT
        let s = make_score(mods_with(&["DT"]));
        assert!(!score_matches_filters(&s, &["mod!=DT".to_string()]));

        // HDDT 包含 DT → 不匹配 mod!=DT
        let s = make_score(mods_with(&["HD", "DT"]));
        assert!(!score_matches_filters(&s, &["mod!=DT".to_string()]));
    }

    #[test]
    fn mod_comparison_ops_silently_pass() {
        // mod>DT 等在 mod 键上无意义 → 静默忽略
        let s = make_score(mods_with(&["HD"]));
        for op_filter in &["mod>DT", "mod<DT", "mod>=DT", "mod<=DT"] {
            assert!(
                score_matches_filters(&s, &[op_filter.to_string()]),
                "{op_filter} should silently pass"
            );
        }
    }

    #[test]
    fn cmp_i64_eq() {
        assert!(cmp_i64(5, 5, FilterOp::Eq));
        assert!(!cmp_i64(5, 6, FilterOp::Eq));
    }

    #[test]
    fn cmp_i64_noteq() {
        assert!(cmp_i64(5, 6, FilterOp::NotEq));
        assert!(!cmp_i64(5, 5, FilterOp::NotEq));
    }

    #[test]
    fn cmp_i64_ordering() {
        assert!(cmp_i64(6, 5, FilterOp::Gt));
        assert!(!cmp_i64(5, 5, FilterOp::Gt));
        assert!(cmp_i64(5, 5, FilterOp::GtEq));
        assert!(cmp_i64(5, 6, FilterOp::Lt));
        assert!(cmp_i64(5, 5, FilterOp::LtEq));
        assert!(!cmp_i64(5, 5, FilterOp::Lt));
    }

    #[test]
    fn cmp_i64_negative() {
        assert!(cmp_i64(-3, -5, FilterOp::Gt));
        assert!(cmp_i64(-5, -5, FilterOp::Eq));
    }

    #[test]
    fn cmp_f64_eq_uses_tolerance() {
        assert!(cmp_f64(500.4, 500.0, FilterOp::Eq, 0.5));
        assert!(!cmp_f64(500.6, 500.0, FilterOp::Eq, 0.5));
        assert!(cmp_f64(500.0, 500.0, FilterOp::Eq, 0.5));
    }

    #[test]
    fn cmp_f64_noteq_uses_tolerance() {
        assert!(cmp_f64(500.6, 500.0, FilterOp::NotEq, 0.5));
        assert!(!cmp_f64(500.4, 500.0, FilterOp::NotEq, 0.5));
    }

    #[test]
    fn cmp_f64_ordering_strict() {
        // > and >= use strict comparison (no tolerance)
        assert!(cmp_f64(500.6, 500.0, FilterOp::Gt, 0.5));
        assert!(cmp_f64(500.4, 500.0, FilterOp::Gt, 0.5));
        assert!(cmp_f64(500.0, 500.0, FilterOp::GtEq, 0.5));
        assert!(cmp_f64(499.4, 500.0, FilterOp::Lt, 0.5));
        assert!(cmp_f64(499.6, 500.0, FilterOp::Lt, 0.5));
        assert!(cmp_f64(500.0, 500.0, FilterOp::LtEq, 0.5));
    }
}
