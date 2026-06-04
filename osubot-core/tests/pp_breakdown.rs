use osubot_core::api::{calculate_pp_breakdown, calculate_pp_if_acc, PpCalcParams};
use osubot_types::{GameMode, ScoreStatistics};
use rosu_mods::GameMods;
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
    })
    .expect("should return breakdown");
    assert_eq!(pp.aim, None, "mania has no aim");
    assert_eq!(pp.speed, None, "mania has no speed");
    assert_eq!(pp.accuracy, 0.0, "mania has no pp_acc (rosu-pp limitation)");
    assert!(pp.difficulty.unwrap() > 0.0, "mania has pp_difficulty");
    assert!(pp.total_pp > 0.0);
}

#[test]
fn catch_breakdown_is_none() {
    let result = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2118524.osu"),
        mode: GameMode::Catch,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
    });
    assert!(result.is_none(), "catch has no breakdown");
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
fn std_to_catch_convert_produces_none() {
    let result = calculate_pp_breakdown(PpCalcParams {
        osu_path: &resource("2785319.osu"),
        mode: GameMode::Catch,
        mods: GameMods::new(),
        accuracy: 0.98,
        max_combo: 500,
        miss_count: 1,
        is_lazer: false,
        statistics: None,
    });
    assert!(result.is_none(), "catch convert has no breakdown");
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
        },
        500,
    )
    .expect("should return if_acc for convert");
    assert_ne!(
        if_acc.acc_95, if_acc.acc_100,
        "acc_95 and acc_100 should differ for Mania convert"
    );
}
