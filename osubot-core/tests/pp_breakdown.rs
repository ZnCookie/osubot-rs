use osubot_core::api::{calculate_pp_breakdown, calculate_pp_if_acc, PpCalcParams};
use osubot_types::{GameMode, ScoreStatistics};
use rosu_mods::{GameMod, GameMods};
use std::path::PathBuf;

fn resource(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/resources")
        .join(name)
}

// === Non-convert (native beatmaps) ===

#[test]
fn std_breakdown_populates_aim_speed_acc() {
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("should return breakdown");
    assert!(pp.aim.unwrap() > 0.0, "aim should be > 0");
    assert!(pp.speed.unwrap() > 0.0, "speed should be > 0");
    assert!(pp.accuracy > 0.0, "accuracy pp should be > 0");
    assert_eq!(pp.difficulty, None, "std has no difficulty field");
    assert!(pp.total_pp > 0.0);
}

#[test]
fn taiko_breakdown_has_difficulty_not_aim() {
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("1028484.osu"),
        mode: GameMode::Taiko,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("should return breakdown");
    assert_eq!(pp.aim, None, "taiko has no aim");
    assert_eq!(pp.speed, None, "taiko has no speed");
    assert!(pp.difficulty.unwrap() > 0.0, "difficulty should be > 0");
    assert!(pp.accuracy > 0.0, "taiko has pp_acc");
    assert!(pp.total_pp > 0.0);
}

#[test]
fn mania_breakdown_difficulty_only_and_no_aim() {
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 100,
        count_katu: 0,
        count_100: 5,
        count_50: 0,
        count_miss: 2,
    };
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("1638954.osu"),
        mode: GameMode::Mania,
        mods: GameMods::new(),
        accuracy: 0.95,
        max_combo: 200,
        miss_count: 2,
        is_lazer: true,
        statistics: Some(&stats),
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("should return breakdown");
    assert_eq!(pp.aim, None, "mania has no aim");
    assert_eq!(pp.speed, None, "mania has no speed");
    assert_eq!(pp.accuracy, 0.0, "mania has no pp_acc (rosu-pp limitation)");
    assert!(pp.difficulty.unwrap() > 0.0, "mania has pp_difficulty");
    assert!(pp.total_pp > 0.0);
}

#[test]
fn catch_breakdown_returns_minimal() {
    let result = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2118524.osu"),
        mode: GameMode::Catch,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    });
    let pp = result.expect("catch should return breakdown");
    assert!(pp.star_rating.is_some(), "catch should have star_rating");
}

// === Bug A: mode conversion ===

#[test]
fn std_to_mania_convert_produces_mania_breakdown() {
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 100,
        count_katu: 0,
        count_100: 5,
        count_50: 0,
        count_miss: 2,
    };
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Mania,
        mods: GameMods::new(),
        accuracy: 0.95,
        max_combo: 200,
        miss_count: 2,
        is_lazer: true,
        statistics: Some(&stats),
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("should return breakdown for convert");
    assert!(
        pp.difficulty.is_some(),
        "mania convert should have difficulty"
    );
    assert_eq!(pp.aim, None, "mania convert should have no aim");
    assert_eq!(pp.speed, None, "mania convert should have no speed");
    assert_eq!(pp.accuracy, 0.0, "mania has no pp_acc");
    assert!(pp.total_pp > 0.0);
}

#[test]
fn std_to_taiko_convert_produces_taiko_breakdown() {
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Taiko,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("should return breakdown for convert");
    assert!(
        pp.difficulty.is_some(),
        "taiko convert should have difficulty"
    );
    assert!(pp.accuracy > 0.0, "taiko convert has pp_acc");
    assert_eq!(pp.aim, None);
    assert_eq!(pp.speed, None);
}

#[test]
fn std_to_catch_convert_returns_minimal() {
    let result = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Catch,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    });
    let pp = result.expect("catch convert should return breakdown");
    assert!(
        pp.star_rating.is_some(),
        "catch convert should have star_rating"
    );
}

// === Bug B: Mania acc_95~100 ===

#[test]
fn mania_if_acc_fc_has_reasonable_values() {
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 900,
        count_katu: 0,
        count_100: 80,
        count_50: 10,
        count_miss: 10,
    };
    let if_acc = calculate_pp_if_acc(
        PpCalcParams {
            osu_path: &resource("1638954.osu"),
            mode: GameMode::Mania,
            mods: GameMods::new(),
            accuracy: 0.95,
            max_combo: 900,
            miss_count: 10,
            is_lazer: true,
            statistics: Some(&stats),
            beatmap_star_rating: None,
            passed: true,
        },
        1000,
    )
    .expect("should return if_acc");
    assert!(if_acc.if_fc > 0.0, "if_fc should be > 0");
    assert!(if_acc.acc_95 <= if_acc.acc_97);
    assert!(if_acc.acc_97 <= if_acc.acc_98);
    assert!(if_acc.acc_98 <= if_acc.acc_99);
    assert!(if_acc.acc_99 <= if_acc.acc_100);
}

#[test]
fn mania_acc_95_differs_from_acc_100() {
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 900,
        count_katu: 0,
        count_100: 80,
        count_50: 10,
        count_miss: 10,
    };
    let if_acc = calculate_pp_if_acc(
        PpCalcParams {
            osu_path: &resource("1638954.osu"),
            mode: GameMode::Mania,
            mods: GameMods::new(),
            accuracy: 0.95,
            max_combo: 900,
            miss_count: 10,
            is_lazer: true,
            statistics: Some(&stats),
            beatmap_star_rating: None,
            passed: true,
        },
        1000,
    )
    .expect("should return if_acc");
    assert_ne!(
        if_acc.acc_95, if_acc.acc_100,
        "acc_95 and acc_100 should differ for Mania"
    );
}

#[test]
fn mania_acc_95_on_convert_uses_accuracy_path() {
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 100,
        count_katu: 0,
        count_100: 5,
        count_50: 0,
        count_miss: 2,
    };
    let if_acc = calculate_pp_if_acc(
        PpCalcParams {
            osu_path: &resource("2785319.osu"),
            mode: GameMode::Mania,
            mods: GameMods::new(),
            accuracy: 0.95,
            max_combo: 200,
            miss_count: 2,
            is_lazer: true,
            statistics: Some(&stats),
            beatmap_star_rating: None,
            passed: true,
        },
        500,
    )
    .expect("should return if_acc for convert");
    assert_ne!(
        if_acc.acc_95, if_acc.acc_100,
        "acc_95 and acc_100 should differ for Mania convert"
    );
}

// === Empty mods with star rating (production path via enrich_score_with_pp) ===

#[test]
fn std_no_mods_with_star_rating_calculates_breakdown() {
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: Some(5.5),
        passed: true,
    })
    .expect("should return breakdown");
    assert!(
        pp.aim.unwrap() > 0.0,
        "aim should be calculated even without mods"
    );
    assert!(pp.speed.unwrap() > 0.0, "speed should be calculated");
    assert!(pp.total_pp > 0.0, "total_pp should be calculated");
}

// === Star rating ===

#[test]
fn std_with_hd_populates_star_rating() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::HiddenOsu(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: Some(5.5),
        passed: true,
    })
    .expect("should return breakdown");
    assert!(pp.star_rating.is_some(), "star_rating should be populated");
    assert!(pp.star_rating.unwrap() > 0.0, "star_rating should be > 0");
}

#[test]
fn std_nf_only_returns_beatmap_star_rating() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::NoFailOsu(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: Some(5.5),
        passed: true,
    })
    .expect("should return breakdown");
    assert_eq!(
        pp.star_rating,
        Some(5.5),
        "NF-only should return beatmap star_rating"
    );
    assert_eq!(
        pp.total_pp, 0.0,
        "NF-only fast path should have total_pp = 0.0"
    );
}

#[test]
fn std_cl_only_returns_beatmap_star_rating() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::ClassicOsu(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: Some(5.5),
        passed: true,
    })
    .expect("should return breakdown");
    assert_eq!(
        pp.star_rating,
        Some(5.5),
        "CL-only should return beatmap star_rating"
    );
    assert_eq!(
        pp.total_pp, 0.0,
        "CL-only fast path should have total_pp = 0.0"
    );
}

#[test]
fn std_to_taiko_convert_populates_star_rating() {
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Taiko,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("should return breakdown for convert");
    assert!(
        pp.star_rating.is_some(),
        "convert should populate star_rating"
    );
    assert!(
        pp.star_rating.unwrap() > 0.0,
        "convert star_rating should be > 0"
    );
}

#[test]
fn std_to_catch_convert_returns_star_rating() {
    let result = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Catch,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    });
    assert!(
        result.is_some(),
        "catch convert should return Some (minimal PpBreakdown)"
    );
    let pp = result.unwrap();
    assert!(
        pp.star_rating.is_some(),
        "catch convert should have star_rating"
    );
    assert_eq!(pp.aim, None, "catch has no aim");
    assert_eq!(pp.difficulty, None, "catch has no difficulty");
    assert!(pp.total_pp > 0.0, "catch should have total_pp");
}

// === Complex mod combinations ===

#[test]
fn std_with_hdhr_populates_breakdown() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::HiddenOsu(Default::default()));
    mods.insert(rosu_mods::GameMod::HardRockOsu(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("HDHR should return breakdown");
    assert!(pp.aim.unwrap() > 0.0, "HDHR aim should be > 0");
    assert!(pp.speed.unwrap() > 0.0, "HDHR speed should be > 0");
    assert!(
        pp.star_rating.unwrap() > 0.0,
        "HDHR star_rating should be > 0"
    );
    assert!(pp.total_pp > 0.0, "HDHR total_pp should be > 0");
}

#[test]
fn std_with_dt_populates_breakdown() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::DoubleTimeOsu(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("DT should return breakdown");
    assert!(pp.aim.unwrap() > 0.0, "DT aim should be > 0");
    assert!(pp.speed.unwrap() > 0.0, "DT speed should be > 0");
    assert!(
        pp.star_rating.unwrap() > 0.0,
        "DT star_rating should be > 0"
    );
    assert!(pp.total_pp > 0.0, "DT total_pp should be > 0");
}

#[test]
fn std_to_taiko_with_hd_populates_breakdown() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::HiddenTaiko(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Taiko,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("convert with HD should return breakdown");
    assert!(
        pp.difficulty.is_some(),
        "taiko convert should have difficulty"
    );
    assert!(
        pp.star_rating.unwrap() > 0.0,
        "convert with HD should have star_rating"
    );
    assert!(pp.total_pp > 0.0, "convert with HD should have total_pp");
}

#[test]
fn std_to_mania_with_hd_populates_breakdown() {
    let mut mods = GameMods::new();
    mods.insert(rosu_mods::GameMod::HiddenMania(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Mania,
        mods,
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("mania convert with HD should return breakdown");
    assert!(
        pp.star_rating.unwrap() > 0.0,
        "mania convert with HD should have star_rating"
    );
    assert!(
        pp.total_pp > 0.0,
        "mania convert with HD should have total_pp"
    );
}

// === passed_objects behavior on failed scores ===

#[test]
fn failed_score_with_statistics_uses_passed_objects() {
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 200,
        count_katu: 0,
        count_100: 10,
        count_50: 0,
        count_miss: 5,
    };
    let pp_passed = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods: GameMods::new(),
        accuracy: 0.97,
        max_combo: 210,
        miss_count: 5,
        is_lazer: false,
        statistics: Some(&stats),
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("passed score should return breakdown");
    let pp_failed = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods: GameMods::new(),
        accuracy: 0.97,
        max_combo: 210,
        miss_count: 5,
        is_lazer: false,
        statistics: Some(&stats),
        beatmap_star_rating: None,
        passed: false,
    })
    .expect("failed score should return breakdown");
    // passed_objects truncates strain/difficulty to actual hits,
    // so the failed score must have strictly lower total_pp
    // (assuming non-zero hits were registered).
    assert!(
        pp_failed.total_pp < pp_passed.total_pp,
        "failed PP ({}) should be < passed PP ({})",
        pp_failed.total_pp,
        pp_passed.total_pp
    );
    assert!(
        pp_failed.total_pp > 0.0,
        "failed score with statistics should still produce non-zero PP (got {})",
        pp_failed.total_pp
    );
}

#[test]
fn failed_score_nf_cl_only_does_not_take_fast_path() {
    let mut mods = GameMods::new();
    mods.insert(GameMod::ClassicOsu(Default::default()));
    let pp = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Osu,
        mods,
        accuracy: 0.95,
        max_combo: 100,
        miss_count: 10,
        is_lazer: false,
        statistics: None,
        beatmap_star_rating: Some(5.0),
        passed: false,
    })
    .expect("failed CL score should return breakdown");
    // Fast path returns total_pp = 0.0; the !passed guard must
    // force this case through the full calculation.
    assert!(
        pp.total_pp > 0.0,
        "failed CL-only score must not take fast path (got total_pp = {})",
        pp.total_pp
    );
}

// === Mania failed-score behavior ===

#[test]
fn failed_score_mania_uses_passed_objects() {
    let stats = ScoreStatistics {
        count_geki: 50,
        count_300: 200,
        count_katu: 10,
        count_100: 5,
        count_50: 0,
        count_miss: 3,
    };
    let pp_passed = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Mania,
        mods: GameMods::new(),
        accuracy: 0.95,
        max_combo: 268,
        miss_count: 3,
        is_lazer: true,
        statistics: Some(&stats),
        beatmap_star_rating: None,
        passed: true,
    })
    .expect("passed Mania score should return breakdown");
    let pp_failed = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Mania,
        mods: GameMods::new(),
        accuracy: 0.95,
        max_combo: 268,
        miss_count: 3,
        is_lazer: true,
        statistics: Some(&stats),
        beatmap_star_rating: None,
        passed: false,
    })
    .expect("failed Mania score should return breakdown");
    assert!(
        pp_failed.total_pp < pp_passed.total_pp,
        "failed Mania PP ({}) should be < passed Mania PP ({})",
        pp_failed.total_pp,
        pp_passed.total_pp
    );
    assert!(
        pp_failed.total_pp > 0.0,
        "failed Mania score with statistics should still produce non-zero PP (got {})",
        pp_failed.total_pp
    );
}

// === calculate_pp_if_acc passed-field behavior ===

#[test]
fn if_acc_ignores_passed_field_and_assumes_full_play() {
    // A failed score (e.g. !r on a 1k-combo map, broken at combo 500)
    // should produce the same if-acc values as a hypothetical full play,
    // because the projection is "what if I'd played through?".
    let stats = ScoreStatistics {
        count_geki: 0,
        count_300: 200,
        count_katu: 0,
        count_100: 10,
        count_50: 0,
        count_miss: 5,
    };
    let pp_if_acc_passed = calculate_pp_if_acc(
        PpCalcParams {
            osu_path: &resource("2785319.osu"),
            mode: GameMode::Osu,
            mods: GameMods::new(),
            accuracy: 0.97,
            max_combo: 500,
            miss_count: 5,
            is_lazer: false,
            statistics: Some(&stats),
            beatmap_star_rating: None,
            passed: true,
        },
        1000,
    )
    .expect("if-acc should compute for passed=true");
    let pp_if_acc_failed = calculate_pp_if_acc(
        PpCalcParams {
            osu_path: &resource("2785319.osu"),
            mode: GameMode::Osu,
            mods: GameMods::new(),
            accuracy: 0.97,
            max_combo: 500,
            miss_count: 5,
            is_lazer: false,
            statistics: Some(&stats),
            beatmap_star_rating: None,
            passed: false,
        },
        1000,
    )
    .expect("if-acc should compute for passed=false");
    // Intentional: passed is ignored in this function, so both calls
    // produce identical projections.
    assert_eq!(
        pp_if_acc_passed.acc_100, pp_if_acc_failed.acc_100,
        "if-acc 100% should not depend on the passed flag"
    );
    assert_eq!(
        pp_if_acc_passed.acc_95, pp_if_acc_failed.acc_95,
        "if-acc 95% should not depend on the passed flag"
    );
    assert_eq!(
        pp_if_acc_passed.if_fc, pp_if_acc_failed.if_fc,
        "if-FC should not depend on the passed flag"
    );
}
