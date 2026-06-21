use super::{GameMode, OsuApiScoreStatistics};

pub(super) fn get_stable_rank(
    stats: &OsuApiScoreStatistics,
    mode: GameMode,
    passed: bool,
    has_hidden: bool,
) -> String {
    if !passed {
        return "F".to_string();
    }

    let great = stats.count_300;
    let count_100 = if stats.count_100 != 0 {
        stats.count_100
    } else {
        stats.ok
    };
    let meh = stats.count_50;
    let miss = stats.count_miss;

    let total = match mode {
        GameMode::Taiko => great + count_100 + miss,
        GameMode::Catch => {
            great
                + stats.osu_large_tick_hits
                + stats.osu_small_tick_hits
                + stats.osu_large_tick_misses
                + stats.osu_small_tick_misses
                + miss
        }
        GameMode::Mania => stats.count_geki + great + stats.count_katu + count_100 + meh + miss,
        _ => great + count_100 + meh + miss,
    };

    let rank = match mode {
        GameMode::Taiko => {
            if great == total {
                "X"
            } else if great * 10 > total * 9 {
                if miss > 0 {
                    "A"
                } else {
                    "S"
                }
            } else if great * 10 > total * 8 {
                if miss > 0 {
                    "B"
                } else {
                    "A"
                }
            } else if great * 10 > total * 7 {
                if miss > 0 {
                    "C"
                } else {
                    "B"
                }
            } else if great * 10 > total * 6 {
                "C"
            } else {
                "D"
            }
        }
        GameMode::Catch => {
            let hit = great + stats.osu_large_tick_hits + stats.osu_small_tick_hits;
            if hit == total {
                "X"
            } else if hit * 100 > total * 98 {
                "S"
            } else if hit * 100 > total * 94 {
                "A"
            } else if hit * 100 > total * 90 {
                "B"
            } else if hit * 100 > total * 85 {
                "C"
            } else {
                "D"
            }
        }
        GameMode::Mania => {
            let perfect = stats.count_geki;
            let good = stats.count_katu;
            let judgement = perfect * 300 + great * 300 + good * 200 + count_100 * 100 + meh * 50;
            if judgement == total * 300 {
                "X"
            } else if judgement * 100 > total * 300 * 95 {
                "S"
            } else if judgement * 100 > total * 300 * 90 {
                "A"
            } else if judgement * 100 > total * 300 * 80 {
                "B"
            } else if judgement * 100 > total * 300 * 70 {
                "C"
            } else {
                "D"
            }
        }
        _ => {
            let is50_over_1p = meh * 100 > total;
            if great == total {
                "X"
            } else if great * 10 > total * 9 {
                if miss > 0 || is50_over_1p {
                    "A"
                } else {
                    "S"
                }
            } else if great * 10 > total * 8 {
                if miss > 0 {
                    "B"
                } else {
                    "A"
                }
            } else if great * 10 > total * 7 {
                if miss > 0 {
                    "C"
                } else {
                    "B"
                }
            } else if great * 10 > total * 6 {
                "C"
            } else {
                "D"
            }
        }
    };

    if has_hidden && (rank == "S" || rank == "X") {
        format!("{}H", rank)
    } else {
        rank.to_string()
    }
}

pub(super) fn get_stable_accuracy(
    stats: &OsuApiScoreStatistics,
    mode: GameMode,
    passed: bool,
) -> f64 {
    let great = stats.count_300 as f64;
    let count_100 = if stats.count_100 != 0 {
        stats.count_100 as f64
    } else {
        stats.ok as f64
    };
    let meh = stats.count_50 as f64;
    let miss = stats.count_miss as f64;

    let total = if passed {
        match mode {
            GameMode::Taiko => great + count_100 + miss,
            GameMode::Catch => {
                great
                    + stats.osu_large_tick_hits as f64
                    + stats.osu_small_tick_hits as f64
                    + stats.osu_large_tick_misses as f64
                    + stats.osu_small_tick_misses as f64
                    + miss
            }
            GameMode::Mania => {
                stats.count_geki as f64 + great + stats.count_katu as f64 + count_100 + meh + miss
            }
            _ => great + count_100 + meh + miss,
        }
    } else {
        return 0.0;
    };

    if total == 0.0 {
        return 0.0;
    }

    let hit = match mode {
        GameMode::Taiko => great + 1.0 / 2.0 * count_100,
        GameMode::Catch => {
            (great + stats.osu_large_tick_hits as f64 + stats.osu_small_tick_hits as f64) * 1.0
        }
        GameMode::Mania => {
            let perfect = stats.count_geki as f64;
            let good = stats.count_katu as f64;
            perfect + great + 2.0 / 3.0 * good + 1.0 / 3.0 * count_100 + 1.0 / 6.0 * meh
        }
        _ => great + 1.0 / 3.0 * count_100 + 1.0 / 6.0 * meh,
    };

    (hit / total).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osu_stats(
        count_300: i64,
        count_100: i64,
        count_50: i64,
        count_miss: i64,
    ) -> OsuApiScoreStatistics {
        OsuApiScoreStatistics {
            count_300,
            count_100,
            count_50,
            count_miss,
            ..Default::default()
        }
    }

    fn taiko_stats(count_300: i64, count_100: i64, count_miss: i64) -> OsuApiScoreStatistics {
        OsuApiScoreStatistics {
            count_300,
            count_100,
            count_miss,
            ..Default::default()
        }
    }

    fn catch_stats(
        fruits: i64,
        large_hit: i64,
        small_hit: i64,
        miss: i64,
    ) -> OsuApiScoreStatistics {
        OsuApiScoreStatistics {
            count_300: fruits,
            osu_large_tick_hits: large_hit,
            osu_small_tick_hits: small_hit,
            count_miss: miss,
            ..Default::default()
        }
    }

    fn mania_stats(
        count_geki: i64,
        count_300: i64,
        count_katu: i64,
        count_100: i64,
        count_50: i64,
        count_miss: i64,
    ) -> OsuApiScoreStatistics {
        OsuApiScoreStatistics {
            count_geki,
            count_300,
            count_katu,
            count_100,
            count_50,
            count_miss,
            ..Default::default()
        }
    }

    #[test]
    fn rank_x_100_percent_all_modes() {
        assert_eq!(
            get_stable_rank(&osu_stats(100, 0, 0, 0), GameMode::Osu, true, false),
            "X"
        );
        assert_eq!(
            get_stable_rank(&taiko_stats(100, 0, 0), GameMode::Taiko, true, false),
            "X"
        );
        assert_eq!(
            get_stable_rank(&catch_stats(100, 0, 0, 0), GameMode::Catch, true, false),
            "X"
        );
        assert_eq!(
            get_stable_rank(
                &mania_stats(100, 0, 0, 0, 0, 0),
                GameMode::Mania,
                true,
                false
            ),
            "X"
        );
    }

    #[test]
    fn rank_s_below_x_osu() {
        let rank = get_stable_rank(&osu_stats(95, 5, 0, 0), GameMode::Osu, true, false);
        assert_eq!(rank, "S");
    }

    #[test]
    fn rank_d_low_accuracy_osu() {
        let rank = get_stable_rank(&osu_stats(5, 0, 5, 0), GameMode::Osu, true, false);
        assert!(matches!(rank.as_str(), "D" | "C"), "got {rank}");
    }

    #[test]
    fn accuracy_unpassed_is_zero() {
        let acc = get_stable_accuracy(&osu_stats(0, 0, 0, 0), GameMode::Osu, false);
        assert_eq!(acc, 0.0);
    }

    #[test]
    fn accuracy_perfect_osu_is_one() {
        let acc = get_stable_accuracy(&osu_stats(300, 0, 0, 0), GameMode::Osu, true);
        assert!((acc - 1.0).abs() < 1e-6);
    }
}
