use osubot_render::match_style::{wrap_match_result_html, MatchPlayerRowData, MatchResultCardData};
use osubot_render::score_list_style::{
    wrap_score_list_html, ScoreListCardData, ScoreListHtmlParams,
};
use osubot_render::score_style::{wrap_score_html, ScoreCardData};
use osubot_render::{render_match_result_card, MatchResultParams, MatchResultPlayerParams};
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
            perfect_pp: 9999.9,
        }),
        perfect_pp: None,
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
                osu_large_tick_hits: 0,
                osu_small_tick_hits: 0,
                osu_slider_tail_hits: 0,
                osu_large_tick_misses: 0,
                osu_small_tick_misses: 0,
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
            user_id: None,
            username: None,
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

fn make_varied_score(idx: usize) -> Score {
    let ranks = [
        "X", "S", "A", "B", "S", "X", "A", "S", "B", "A", "S", "X", "A", "S", "B", "X", "A", "S",
        "A", "B",
    ];
    let titles = [
        "Camellia - Exit This Earth's Atomosphere",
        "xi - Blue Zenith",
        "UNDEAD CORPORATION - Everything will freeze",
        "Halozy - Genryuu Kaiko",
        "t+pazolite - CENSORED!!",
        "Foreground Eclipse - Truths, Ironies, The Secret Lyrics",
        "Nanahira - Bassline Yatteru w",
        "DragonForce - Through the Fire and Flames",
        "IOSYS - Cirno's Perfect Math Class",
        "Demetori - Emotional Skyscraper",
        "sasakure.UK - 終焉の栞",
        "kors k - Insane Techniques",
        "Getty vs. DJ DiA - Drop",
        "Rohi - Kakuzetsu Andante",
        "Function Phantom - Euclidean",
        "OISHII - ONIGIRI FREEWAY",
        "Umeboshi Chazuke - ICHIBANBOSHI*ROCKET",
        "USAO - BroGamer",
        "lapix - Carry Me Away",
        "aran - COZMIC DRIVE",
    ];
    let artists = [
        "Camellia",
        "xi",
        "UNDEAD CORPORATION",
        "Halozy",
        "t+pazolite",
        "Foreground Eclipse",
        "Nanahira",
        "DragonForce",
        "IOSYS",
        "Demetori",
        "sasakure.UK",
        "kors k",
        "Getty vs. DJ DiA",
        "Rohi",
        "Function Phantom",
        "OISHII",
        "Umeboshi Chazuke",
        "USAO",
        "lapix",
        "aran",
    ];
    let versions = [
        "Cosmic",
        "FOUR DIMENSIONS",
        "Extra",
        "Lunatic",
        "Crazy",
        "Extra",
        "w",
        "Expert",
        "Lunatic",
        "Lunatic",
        "Andante",
        "Speed",
        "Expert",
        "Isolation",
        "Extra",
        "SMILE",
        "ROCKET",
        "GAMER",
        "Lapix",
        "DRIVE",
    ];

    let pp_values = [
        456.0, 389.0, 312.0, 287.0, 245.0, 198.0, 185.0, 170.0, 155.0, 140.0, 130.0, 120.0, 110.0,
        100.0, 90.0, 80.0, 72.0, 65.0, 58.0, 50.0,
    ];
    let star_ratings = [
        8.52, 7.85, 7.20, 6.88, 6.50, 6.12, 5.80, 5.45, 5.10, 4.80, 4.50, 4.20, 3.90, 3.60, 3.30,
        3.00, 2.70, 2.40, 2.10, 1.80,
    ];
    let acc_values = [
        0.985, 0.992, 0.971, 0.988, 0.953, 0.980, 0.975, 0.990, 0.965, 0.982, 0.978, 0.995, 0.960,
        0.987, 0.970, 0.983, 0.955, 0.991, 0.968, 0.977,
    ];
    let score_values = [
        1234567, 987654, 876543, 765432, 654321, 543210, 432109, 321098, 210987, 100000, 999999,
        888888, 777777, 666666, 555555, 444444, 333333, 222222, 111111, 99999,
    ];

    let mut mods = rosu_mods::GameMods::new();
    match idx % 5 {
        0 => {
            mods.insert(GameMod::HiddenOsu(Default::default()));
        }
        1 => {
            mods.insert(GameMod::HiddenOsu(Default::default()));
            mods.insert(GameMod::DoubleTimeOsu(Default::default()));
        }
        2 => {
            mods.insert(GameMod::HardRockOsu(Default::default()));
        }
        3 => {
            mods.insert(GameMod::DoubleTimeOsu(Default::default()));
        }
        _ => {}
    }

    Score {
        score_id: idx as i64 + 1,
        beatmap_id: idx as i64 + 900000,
        beatmapset_id: idx as i64 + 100,
        artist: artists[idx].to_string(),
        title: titles[idx].to_string(),
        version: versions[idx].to_string(),
        creator: format!("Mapper{}", idx),
        star_rating: star_ratings[idx],
        bpm: 180.0 + idx as f64 * 5.0,
        ar: 9.3,
        od: 8.5,
        cs: 4.0,
        hp: 6.0,
        length_seconds: 120 + idx as i64 * 15,
        score_value: score_values[idx],
        accuracy: acc_values[idx],
        max_combo: 500 + idx as i64 * 30,
        beatmap_max_combo: 600 + idx as i64 * 30,
        pp: Some(pp_values[idx]),
        pp_breakdown: None,
        pp_if_acc: None,
        perfect_pp: None,
        rank: ranks[idx].to_string(),
        passed: !matches!(idx, 3 | 8 | 14),
        mods,
        is_perfect: idx.is_multiple_of(4),
        created_at: format!(
            "2025-06-{:02}T{:02}:{:02}:00Z",
            (idx % 28) + 1,
            idx % 24,
            idx * 3 % 60
        ),
        is_lazer: idx.is_multiple_of(3),
        has_replay: true,
        legacy_score_id: None,
        statistics: ScoreStatistics {
            count_geki: 0,
            count_300: 800 + idx as i64 * 5,
            count_katu: 0,
            count_100: 50 - idx as i64,
            count_50: 5,
            count_miss: idx as i64 % 7,
            osu_large_tick_hits: 0,
            osu_small_tick_hits: 0,
            osu_slider_tail_hits: 0,
            osu_large_tick_misses: 0,
            osu_small_tick_misses: 0,
        },
        cover_url: format!(
            "https://assets.ppy.sh/beatmaps/{}/covers/cover@2x.jpg",
            idx + 1000
        ),
        user: ScoreUser {
            avatar_url: "https://a.ppy.sh/1".to_string(),
            country_code: "CN".to_string(),
            user_id: None,
            username: None,
            global_rank: Some(12345),
            country_rank: Some(1000),
            pp: 9876.5,
        },
        fav_count: Some(1000 + idx as i64 * 100),
        play_count: Some(50000 + idx as i64 * 1000),
        status: "ranked".to_string(),
    }
}

fn make_score_list_card_data() -> Vec<ScoreListCardData> {
    (0..20)
        .map(|i| {
            let score = make_varied_score(i);
            ScoreListCardData::from_score(&score, String::new())
        })
        .collect()
}

#[test]
#[ignore = "run with --ignored to dump HTML for visual inspection"]
fn dump_score_list_html() {
    let cards = make_score_list_card_data();
    let indices: Vec<usize> = (0..20).collect();
    let params = ScoreListHtmlParams {
        cards: &cards,
        username: "ZnCookie",
        mode: GameMode::Osu,
        label: "最近游玩",
        count_text: "{} 条记录",
        avatar_data_uri: "",
        hero_bg_data_uri: "",
        user_pp: 9876.5,
        user_global_rank: Some(12345),
        user_country_rank: Some(1000),
        country_code: "CN",
        pp_change: Some(12.5),
        global_rank_change: Some(-99),
        country_rank_change: Some(50),
        original_indices: &indices,
    };
    let html = wrap_score_list_html(&params);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dump");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("score-list.html"), &html).unwrap();
    assert!(!html.is_empty());
}

#[test]
fn test_wrap_score_list_html_basic() {
    let cards = make_score_list_card_data();
    let indices: Vec<usize> = (0..20).collect();
    let params = ScoreListHtmlParams {
        cards: &cards,
        username: "ZnCookie",
        mode: GameMode::Osu,
        label: "最近游玩",
        count_text: "{} 条记录",
        avatar_data_uri: "",
        hero_bg_data_uri: "",
        user_pp: 9876.5,
        user_global_rank: Some(12345),
        user_country_rank: Some(1000),
        country_code: "CN",
        pp_change: Some(12.5),
        global_rank_change: Some(-99),
        country_rank_change: Some(50),
        original_indices: &indices,
    };
    let html = wrap_score_list_html(&params);

    // 验证新布局元素
    assert!(html.contains(r#"class="mini-card""#));
    assert!(html.contains(r#"class="cover-strip""#));
    assert!(html.contains(r#"class="star-in-cover""#));
    assert!(html.contains(r#"class="time-in-cover""#));
    assert!(html.contains(r#"class="row2""#));

    // 验证 acc/pp 行内容
    assert!(html.contains(r#"class="acc""#));
    assert!(html.contains(r#"class="pp""#));

    // 验证移除的元素
    assert!(!html.contains("mini-score"));
}

fn make_match_card_data() -> MatchResultCardData {
    let cover_data_uri = {
        let cover = make_match_cover_image();
        let mut cursor = std::io::Cursor::new(Vec::new());
        cover
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .unwrap();
        format!(
            "data:image/jpeg;base64,{}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                cursor.into_inner()
            )
        )
    };

    MatchResultCardData {
        match_id: 12_345_678,
        match_name: "Dump Match".to_string(),
        event_label: "Game finished".to_string(),
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
        cover_data_uri,
        is_started: false,
        selected_mods: vec!["HD".to_string(), "HR".to_string()],
        team_type: Some("team-vs".to_string()),
        scoring_type: Some("score".to_string()),
        team_results: Vec::new(),
        players: vec![
            MatchPlayerRowData {
                placement: 1,
                username: "Alice".to_string(),
                avatar_data_uri: String::new(),
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

fn make_match_cover_image() -> image::DynamicImage {
    let mut img = image::RgbImage::new(640, 360);
    for (x, y, pixel) in img.enumerate_pixels_mut() {
        let r = ((x * 255) / 639) as u8;
        let g = ((y * 180) / 359) as u8;
        let b = (120 + ((x + y) % 120)) as u8;
        *pixel = image::Rgb([r, g, b]);
    }
    image::DynamicImage::ImageRgb8(img)
}

fn make_match_params() -> MatchResultParams {
    MatchResultParams {
        match_id: 12_345_678,
        match_name: "Dump Match".to_string(),
        event_label: "Game finished".to_string(),
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
        cover_image: Some(make_match_cover_image()),
        is_started: false,
        selected_mods: vec!["HD".to_string(), "HR".to_string()],
        team_type: Some("team-vs".to_string()),
        scoring_type: Some("score".to_string()),
        team_results: Vec::new(),
        players: vec![
            MatchResultPlayerParams {
                placement: 1,
                username: "Alice".to_string(),
                avatar_url: None,
                avatar_image: None,
                team: Some("Red".to_string()),
                score: 1_234_567,
                accuracy: 0.9876,
                max_combo: 1234,
                mods: vec!["HD".to_string(), "HR".to_string()],
                rank: "A".to_string(),
                passed: true,
            },
            MatchResultPlayerParams {
                placement: 2,
                username: "Bob".to_string(),
                avatar_url: None,
                avatar_image: None,
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
#[ignore = "run with --ignored to dump match HTML/JPEG for visual inspection"]
fn dump_match_html() {
    let data = make_match_card_data();
    let html = wrap_match_result_html(&data);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dump");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("match-result.html"), &html).unwrap();
    assert!(!html.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run with --ignored to dump match HTML/JPEG for visual inspection"]
async fn dump_match_jpeg() {
    let params = make_match_params();
    let jpeg = render_match_result_card(params).await.expect("render ok");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dump");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("match-result.jpeg"), &jpeg).unwrap();
    assert!(!jpeg.is_empty());
}
