use osubot_render::score_style::{wrap_score_html, ScoreCardData};
use osubot_types::{GameMode, PpBreakdown, PpIfAcc, Score, ScoreStatistics, ScoreUser};
use rosu_mods::GameMod;
use std::path::Path;

fn make_max_mods() -> rosu_mods::GameMods {
    let mut mods = rosu_mods::GameMods::new();
    mods.insert(GameMod::HiddenOsu(Default::default()));
    mods.insert(GameMod::DoubleTimeOsu(Default::default()));
    mods.insert(GameMod::HardRockOsu(Default::default()));
    mods.insert(GameMod::FlashlightOsu(Default::default()));
    mods.insert(GameMod::EasyOsu(Default::default()));
    mods
}

fn make_max_score(mode: GameMode) -> Score {
    let is_mania = mode == GameMode::Mania;
    Score {
        score_id: 999999,
        beatmap_id: 999999,
        beatmapset_id: 999999,
        artist: "Maximum".to_string(),
        title: "Max Test Title for Overflow".to_string(),
        version: "MaxExtra".to_string(),
        creator: "MaxMapper".to_string(),
        star_rating: 12.0,
        bpm: 300.0,
        ar: 7.0,
        od: 7.0,
        cs: 7.0,
        hp: 7.0,
        length_seconds: 7200,
        score_value: 9999999,
        accuracy: 0.9999,
        max_combo: 9999,
        beatmap_max_combo: 9999,
        pp: Some(9999.9),
        pp_breakdown: Some(PpBreakdown {
            aim: Some(9999.9),
            speed: Some(9999.9),
            accuracy: 9999.9,
            flashlight: Some(9999.9),
            difficulty: Some(9999.9),
            total_pp: 9999.9,
            star_rating: None,
        }),
        pp_if_acc: Some(PpIfAcc {
            acc_95: 9999.9,
            acc_97: 9999.9,
            acc_98: 9999.9,
            acc_99: 9999.9,
            acc_100: 9999.9,
            if_fc: 9999.9,
        }),
        rank: "X".to_string(),
        passed: true,
        mods: make_max_mods(),
        is_perfect: true,
        created_at: "2025-12-31T15:59:59Z".to_string(),
        is_lazer: true,
        has_replay: true,
        legacy_score_id: None,
        statistics: {
            let mut s = ScoreStatistics {
                count_geki: 9999,
                count_300: 9999,
                count_katu: 9999,
                count_100: 9999,
                count_50: 9999,
                count_miss: 9999,
            };
            if !is_mania {
                s.count_geki = 0;
                s.count_katu = 0;
            }
            s
        },
        cover_url: "https://assets.ppy.sh/beatmaps/999999/covers/cover@2x.jpg".to_string(),
        user: ScoreUser {
            avatar_url: "https://a.ppy.sh/1".to_string(),
            country_code: "JP".to_string(),
            global_rank: Some(999999),
            country_rank: Some(999999),
            pp: 99999.9,
        },
        fav_count: Some(99999),
        play_count: Some(999999),
        status: "ranked".to_string(),
    }
}

fn make_max_score_card_data(mode: GameMode) -> ScoreCardData {
    ScoreCardData {
        score: make_max_score(mode),
        username: "MaxPlayerName".to_string(),
        mode,
        user_pp: 99999.9,
        user_global_rank: Some(999999),
        user_country_rank: Some(999999),
        country_code: "JP".to_string(),
        avatar_data_uri: String::new(),
        bg_data_uri: String::new(),
        thumb_data_uri: String::new(),
        play_time: "2025/12/31 23:59:59".to_string(),
        hue: 200,
        sat: 60,
        fav_count: Some(99999),
        play_count: Some(999999),
        pp_change: Some(999.0),
        global_rank_change: Some(-99999),
        country_rank_change: Some(-99999),
        ranked_status: "Ranked".to_string(),
        ur_value: Some(999.9),
        ar_eff: Some(12.5),
        od_eff: Some(12.5),
        cs_eff: Some(4.0),
        hp_eff: Some(5.5),
    }
}

fn do_dump(mode: GameMode) {
    let data = make_max_score_card_data(mode);
    let html = wrap_score_html(&data);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dump");
    std::fs::create_dir_all(&dir).unwrap();
    let file_name = format!("score-{}.html", mode.api_value());
    std::fs::write(dir.join(file_name), &html).unwrap();
    assert!(!html.is_empty());
}

#[test]
#[ignore = "run with --ignored to dump HTML for visual inspection"]
fn dump_html_osu() {
    do_dump(GameMode::Osu);
}

#[test]
#[ignore = "run with --ignored to dump HTML for visual inspection"]
fn dump_html_taiko() {
    do_dump(GameMode::Taiko);
}

#[test]
#[ignore = "run with --ignored to dump HTML for visual inspection"]
fn dump_html_catch() {
    do_dump(GameMode::Catch);
}

#[test]
#[ignore = "run with --ignored to dump HTML for visual inspection"]
fn dump_html_mania() {
    do_dump(GameMode::Mania);
}
