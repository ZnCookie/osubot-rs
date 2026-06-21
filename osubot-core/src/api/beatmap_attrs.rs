//! 谱面属性（AR/OD/CS/HP）按 mod 调整的纯函数。
//! 与 score_convert 无关：转换 API score 到内部 Score 是 IO 边界，
//! 属性调整是纯计算。

use crate::types::GameMode;

/// Apply mod adjustments to base AR/OD/CS/HP values.
/// Returns the effective in-game values after mods (DT, HT, HR, EZ, DA, etc).
/// No beatmap download needed — uses BeatmapAttributesBuilder with base stats.
pub fn apply_mod_adjustment_to_stats(
    mode: GameMode,
    ar: f64,
    od: f64,
    cs: f64,
    hp: f64,
    mods: &rosu_mods::GameMods,
) -> (f64, f64, f64, f64) {
    if mods.is_empty() {
        return (ar, od, cs, hp);
    }

    use rosu_mods::GameMod;

    let (mut ar, mut od, mut cs, mut hp) = (ar, od, cs, hp);
    let mut effective_mods = mods.clone();
    let mut has_da = false;

    for m in mods.iter() {
        match m {
            GameMod::DifficultyAdjustOsu(da) => {
                has_da = true;
                if let Some(v) = da.approach_rate {
                    ar = v;
                }
                if let Some(v) = da.circle_size {
                    cs = v;
                }
                if let Some(v) = da.drain_rate {
                    hp = v;
                }
                if let Some(v) = da.overall_difficulty {
                    od = v;
                }
                effective_mods.remove(m);
            }
            GameMod::DifficultyAdjustTaiko(da) => {
                has_da = true;
                if let Some(v) = da.drain_rate {
                    hp = v;
                }
                if let Some(v) = da.overall_difficulty {
                    od = v;
                }
                effective_mods.remove(m);
            }
            GameMod::DifficultyAdjustCatch(da) => {
                has_da = true;
                if let Some(v) = da.approach_rate {
                    ar = v;
                }
                if let Some(v) = da.circle_size {
                    cs = v;
                }
                if let Some(v) = da.drain_rate {
                    hp = v;
                }
                if let Some(v) = da.overall_difficulty {
                    od = v;
                }
                effective_mods.remove(m);
            }
            GameMod::DifficultyAdjustMania(da) => {
                has_da = true;
                if let Some(v) = da.drain_rate {
                    hp = v;
                }
                if let Some(v) = da.overall_difficulty {
                    od = v;
                }
                effective_mods.remove(m);
            }
            _ => {}
        }
    }

    let mods_for_builder = if has_da { &effective_mods } else { mods };

    use rosu_pp::model::beatmap::BeatmapAttributesBuilder;
    use rosu_pp::model::mode::GameMode as RosuMode;
    let rosu_mode = match mode {
        GameMode::Osu => RosuMode::Osu,
        GameMode::Taiko => RosuMode::Taiko,
        GameMode::Catch => RosuMode::Catch,
        GameMode::Mania => RosuMode::Mania,
    };
    let adjusted = BeatmapAttributesBuilder::new()
        .mode(rosu_mode, false)
        .ar(ar as f32, false)
        .od(od as f32, false)
        .cs(cs as f32, false)
        .hp(hp as f32, false)
        .mods(mods_for_builder.clone())
        .build()
        .apply_clock_rate();
    (
        adjusted.ar,
        adjusted.od,
        adjusted.cs as f64,
        adjusted.hp as f64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_adjust_osu_hr_scales_cs_and_ar() {
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::HardRockOsu(Default::default()));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!((ar - 10.0).abs() < 0.01, "ar={ar}");
        assert!((od - 10.0).abs() < 0.01, "od={od}");
        assert!((cs - 5.2).abs() < 0.01, "cs={cs}");
        assert!((hp - 7.0).abs() < 0.01, "hp={hp}");
    }

    #[test]
    fn mod_adjust_mania_hr_leaves_cs_and_ar_unchanged() {
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::HardRockMania(Default::default()));
        let (ar, _od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Mania, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!(
            (ar - 9.0).abs() < 0.01,
            "mania ar should be unchanged, got {ar}"
        );
        assert!(
            (cs - 4.0).abs() < 0.01,
            "mania cs should be unchanged, got {cs}"
        );
        assert!((hp - 7.0).abs() < 0.01, "hp={hp}");
    }

    #[test]
    fn mod_adjust_no_mods_returns_base() {
        let mods = rosu_mods::GameMods::new();
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert_eq!((ar, od, cs, hp), (9.0, 8.0, 4.0, 5.0));
    }

    #[test]
    fn mod_adjust_da_osu_overrides_all() {
        use rosu_mods::generated_mods::DifficultyAdjustOsu;
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustOsu(
            DifficultyAdjustOsu {
                approach_rate: Some(3.0),
                circle_size: Some(7.0),
                drain_rate: Some(2.0),
                overall_difficulty: Some(6.0),
                ..Default::default()
            },
        ));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!((ar - 3.0).abs() < 0.01, "ar={ar}, expected 3.0");
        assert!((od - 6.0).abs() < 0.01, "od={od}, expected 6.0");
        assert!((cs - 7.0).abs() < 0.01, "cs={cs}, expected 7.0");
        assert!((hp - 2.0).abs() < 0.01, "hp={hp}, expected 2.0");
    }

    #[test]
    fn mod_adjust_da_partial_override() {
        use rosu_mods::generated_mods::DifficultyAdjustOsu;
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustOsu(
            DifficultyAdjustOsu {
                approach_rate: Some(7.5),
                ..Default::default()
            },
        ));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 5.0, 6.0, 4.0, 5.0, &mods);
        assert!((ar - 7.5).abs() < 0.01, "ar={ar}, expected 7.5");
        assert!((od - 6.0).abs() < 0.01, "od={od}, expected 6.0 (unchanged)");
        assert!((cs - 4.0).abs() < 0.01, "cs={cs}, expected 4.0 (unchanged)");
        assert!((hp - 5.0).abs() < 0.01, "hp={hp}, expected 5.0 (unchanged)");
    }

    #[test]
    fn mod_adjust_da_plus_dt_applies_clock_rate() {
        use rosu_mods::generated_mods::{DifficultyAdjustOsu, DoubleTimeOsu};
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustOsu(
            DifficultyAdjustOsu {
                approach_rate: Some(5.0),
                overall_difficulty: Some(5.0),
                ..Default::default()
            },
        ));
        mods.insert(rosu_mods::GameMod::DoubleTimeOsu(DoubleTimeOsu::default()));
        let (ar, od, _cs, _hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!((ar - 7.666).abs() < 0.1, "ar={ar}, expected ~7.67");
        assert!((od - 7.777).abs() < 0.1, "od={od}, expected ~7.78");
    }

    #[test]
    fn mod_adjust_da_plus_hr_applies_hr_after_da() {
        use rosu_mods::generated_mods::{DifficultyAdjustOsu, HardRockOsu};
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustOsu(
            DifficultyAdjustOsu {
                approach_rate: Some(5.0),
                overall_difficulty: Some(5.0),
                circle_size: Some(4.0),
                drain_rate: Some(5.0),
                ..Default::default()
            },
        ));
        mods.insert(rosu_mods::GameMod::HardRockOsu(HardRockOsu::default()));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Osu, 9.0, 8.0, 4.0, 5.0, &mods);
        assert!((ar - 7.0).abs() < 0.01, "ar={ar}, expected 7.0");
        assert!((od - 7.0).abs() < 0.01, "od={od}, expected 7.0");
        assert!((cs - 5.2).abs() < 0.01, "cs={cs}, expected 5.2");
        assert!((hp - 7.0).abs() < 0.01, "hp={hp}, expected 7.0");
    }

    #[test]
    fn mod_adjust_da_taiko() {
        use rosu_mods::generated_mods::DifficultyAdjustTaiko;
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustTaiko(
            DifficultyAdjustTaiko {
                drain_rate: Some(3.0),
                overall_difficulty: Some(7.0),
                ..Default::default()
            },
        ));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Taiko, 5.0, 5.0, 5.0, 5.0, &mods);
        assert!((ar - 5.0).abs() < 0.01, "taiko ar unchanged, ar={ar}");
        assert!((od - 7.0).abs() < 0.01, "taiko od={od}, expected 7.0");
        assert!((cs - 5.0).abs() < 0.01, "taiko cs unchanged, cs={cs}");
        assert!((hp - 3.0).abs() < 0.01, "taiko hp={hp}, expected 3.0");
    }

    #[test]
    fn mod_adjust_da_mania() {
        use rosu_mods::generated_mods::DifficultyAdjustMania;
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustMania(
            DifficultyAdjustMania {
                drain_rate: Some(4.0),
                overall_difficulty: Some(8.0),
                ..Default::default()
            },
        ));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Mania, 5.0, 5.0, 5.0, 5.0, &mods);
        assert!((ar - 5.0).abs() < 0.01, "mania ar unchanged, ar={ar}");
        assert!((od - 8.0).abs() < 0.01, "mania od={od}, expected 8.0");
        assert!((cs - 5.0).abs() < 0.01, "mania cs unchanged, cs={cs}");
        assert!((hp - 4.0).abs() < 0.01, "mania hp={hp}, expected 4.0");
    }

    #[test]
    fn mod_adjust_da_catch() {
        use rosu_mods::generated_mods::DifficultyAdjustCatch;
        let mut mods = rosu_mods::GameMods::new();
        mods.insert(rosu_mods::GameMod::DifficultyAdjustCatch(
            DifficultyAdjustCatch {
                approach_rate: Some(6.0),
                circle_size: Some(3.0),
                drain_rate: Some(4.0),
                overall_difficulty: Some(7.0),
                ..Default::default()
            },
        ));
        let (ar, od, cs, hp) =
            apply_mod_adjustment_to_stats(GameMode::Catch, 5.0, 5.0, 5.0, 5.0, &mods);
        assert!((ar - 6.0).abs() < 0.01, "catch ar={ar}, expected 6.0");
        assert!((od - 7.0).abs() < 0.01, "catch od={od}, expected 7.0");
        assert!((cs - 3.0).abs() < 0.01, "catch cs={cs}, expected 3.0");
        assert!((hp - 4.0).abs() < 0.01, "catch hp={hp}, expected 4.0");
    }
}
