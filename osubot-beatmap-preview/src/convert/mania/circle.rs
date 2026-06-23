use super::*;

// ── circle generator ──

pub(super) fn circle_resolve_convert_type(
    s: &ConversionState,
    ho: &StandardHitObject,
    time_gap: i64,
    pos_gap: f64,
) -> u32 {
    let mut ct: u32 = 0;
    let t_cols = s.total_columns;
    let beat_len = circle_beat_length_at(s.timing_points, ho.start_time);
    let density = s.density();

    if time_gap <= 80 {
        ct |= P_FORCE_NOT_STACK | P_KEEP_SINGLE;
    } else if time_gap <= 95 {
        ct |= P_FORCE_NOT_STACK | P_KEEP_SINGLE | s.stair_type;
    } else if time_gap <= 105 {
        ct |= P_FORCE_NOT_STACK | P_LOW_PROBABILITY;
    } else if time_gap <= 125 {
        ct |= P_FORCE_NOT_STACK;
    } else if time_gap <= 135 && pos_gap < 20.0 {
        ct |= P_CYCLE | P_KEEP_SINGLE;
    } else if time_gap <= 150 && pos_gap < 20.0 {
        ct |= P_FORCE_STACK | P_LOW_PROBABILITY;
    } else if pos_gap < 20.0 && density >= beat_len / 2.5 {
        ct |= P_REVERSE | P_LOW_PROBABILITY;
    } else if density < beat_len / 2.5 || kiai_at(ho.start_time, s.timing_points) {
        // high density, no special flag
    } else {
        ct |= P_LOW_PROBABILITY;
    }

    if ct & P_KEEP_SINGLE == 0 {
        if (ho.hitsound & HIT_FINISH != 0) && t_cols != 8 {
            ct |= P_MIRROR;
        } else if ho.hitsound & HIT_CLAP != 0 {
            ct |= P_GATHERED;
        }
    }

    ct
}

fn circle_beat_length_at(timing_points: &[TimingPoint], time: i64) -> f64 {
    let mut base = 500.0;
    for tp in timing_points {
        if tp.uninherited && tp.time <= time as f64 {
            base = tp.beat_length;
        }
    }
    base
}

pub(super) fn circle_generate(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    time_gap: i64,
    pos_gap: f64,
) -> Result<Vec<ManiaHitObject>> {
    let ct = circle_resolve_convert_type(s, ho, time_gap, pos_gap);
    let t_cols = s.total_columns;
    let t = ho.start_time;
    let mut pattern = Pattern::new();
    let prev = s.prev_pattern.clone();

    if t_cols <= 1 {
        pattern.add(0, t, t);
        s.prev_pattern = pattern;
        return Ok(s.prev_pattern.objects.clone());
    }

    // Reverse
    if ct & P_REVERSE != 0 && prev.column_count() > 0 {
        for c in s.random_start..t_cols {
            if prev.has_column(c) {
                pattern.add(s.random_start + t_cols - c - 1, t, t);
            }
        }
        s.prev_pattern = pattern;
        return Ok(s.prev_pattern.objects.clone());
    }

    // Cycle
    if ct & P_CYCLE != 0
        && prev.column_count() == 1
        && !(t_cols == 8 && prev.any_column() == 0)
        && !(t_cols % 2 == 1 && prev.any_column() == t_cols / 2)
    {
        let col = s.random_start + t_cols - prev.any_column() - 1;
        pattern.add(col, t, t);
        s.prev_pattern = pattern;
        return Ok(s.prev_pattern.objects.clone());
    }

    // ForceStack
    if ct & P_FORCE_STACK != 0 && prev.column_count() > 0 {
        for c in s.random_start..t_cols {
            if prev.has_column(c) {
                pattern.add(c, t, t);
            }
        }
        s.prev_pattern = pattern;
        return Ok(s.prev_pattern.objects.clone());
    }

    // Stair (from previous)
    if prev.column_count() == 1 {
        let last_col = prev.any_column();
        if ct & P_STAIR != 0 {
            let mut col = last_col + 1;
            if col >= t_cols {
                col = s.random_start;
            }
            pattern.add(col, t, t);
            if col == t_cols - 1 {
                s.stair_type = P_REVERSE_STAIR;
            }
            s.prev_pattern = pattern;
            return Ok(s.prev_pattern.objects.clone());
        }
        if ct & P_REVERSE_STAIR != 0 {
            let mut col = last_col - 1;
            if col < s.random_start {
                col = t_cols - 1;
            }
            pattern.add(col, t, t);
            if col == s.random_start {
                s.stair_type = P_STAIR;
            }
            s.prev_pattern = pattern;
            return Ok(s.prev_pattern.objects.clone());
        }
    }

    // KeepSingle
    if ct & P_KEEP_SINGLE != 0 {
        circle_gen_random_notes(s, &mut pattern, 1, ct, ho, &prev)?;
        s.prev_pattern = pattern;
        return Ok(s.prev_pattern.objects.clone());
    }

    // Mirror
    if ct & P_MIRROR != 0 {
        circle_gen_mirrored(s, &mut pattern, ct, ho, &prev)?;
        s.prev_pattern = pattern;
        return Ok(s.prev_pattern.objects.clone());
    }

    // Random with conversion difficulty
    let cd = s.conv_diff;
    let lp = ct & P_LOW_PROBABILITY != 0;
    let (p2, p3): (f64, f64) = if cd > 6.5 {
        if lp {
            (0.78, 0.42)
        } else {
            (1.0, 0.62)
        }
    } else if cd > 4.0 {
        if lp {
            (0.35, 0.08)
        } else {
            (0.52, 0.15)
        }
    } else if cd > 2.0 {
        if lp {
            (0.18, 0.0)
        } else {
            (0.45, 0.0)
        }
    } else {
        (0.0, 0.0)
    };

    let note_count = circle_get_random_note_count(s, ho, p2, p3, 0.0, 0.0);
    circle_gen_random_notes(s, &mut pattern, note_count, ct, ho, &prev)?;
    circle_add_special_column(s, &mut pattern, ho);
    s.prev_pattern = pattern;
    Ok(s.prev_pattern.objects.clone())
}

fn circle_gen_random_notes(
    s: &mut ConversionState,
    pattern: &mut Pattern,
    count: i32,
    ct: u32,
    ho: &StandardHitObject,
    prev: &Pattern,
) -> Result<()> {
    let allow_stack = ct & P_FORCE_NOT_STACK == 0;
    let mut count = count;

    if !allow_stack {
        let occupied = prev.column_count();
        count = i32::min(count, s.total_columns - s.random_start - occupied);
    }

    let mut col = s.get_column(ho.x, true);
    let gathered = ct & P_GATHERED != 0;

    for _ in 0..count {
        col = if allow_stack {
            s.find_available_column(col, &[&*pattern], None, None, gathered, None)?
        } else {
            s.find_available_column(col, &[&*pattern, prev], None, None, gathered, None)?
        };
        pattern.add(col, ho.start_time, ho.start_time);
    }
    Ok(())
}

fn circle_gen_mirrored(
    s: &mut ConversionState,
    pattern: &mut Pattern,
    ct: u32,
    ho: &StandardHitObject,
    prev: &Pattern,
) -> Result<()> {
    let t_cols = s.total_columns;
    let cd = s.conv_diff;

    if ct & P_FORCE_NOT_STACK != 0 {
        let (p2, p3, p4, p5): (f64, f64, f64, f64) = if cd > 6.5 {
            (0.5 + 0.38 / 2.0, 0.38, (0.38 + 0.12) / 2.0, 0.12)
        } else if cd > 4.0 {
            (0.5 + 0.17 / 2.0, 0.17, 0.17 / 2.0, 0.0)
        } else {
            (0.5, 0.0, 0.0, 0.0)
        };
        let nc = circle_get_random_note_count(s, ho, p2, p3, p4, p5);
        circle_gen_random_notes(s, pattern, nc, ct, ho, prev)?;
        circle_add_special_column(s, pattern, ho);
        return Ok(());
    }

    let mut centre_p: f64 = 0.12;
    let (mut p2, mut p3): (f64, f64) = if cd > 6.5 {
        (0.38, 0.12)
    } else if cd > 4.0 {
        (0.17, 0.0)
    } else {
        (0.0, 0.0)
    };

    // cap mirrored probabilities per key count
    if t_cols == 2 {
        centre_p = 0.0;
        p2 = 0.0;
        p3 = 0.0;
    } else if t_cols == 3 {
        centre_p = f64::min(centre_p, 0.03);
        p2 = 0.0;
        p3 = 0.0;
    } else if t_cols == 4 {
        centre_p = 0.0;
        p2 = 1.0 - f64::max((1.0 - p2) * 2.0, 0.8);
    } else if t_cols == 5 {
        centre_p = f64::min(centre_p, 0.03);
        p3 = 0.0;
    } else if t_cols == 6 {
        centre_p = 0.0;
        p2 = 1.0 - f64::max((1.0 - p2) * 2.0, 0.5);
        p3 = 1.0 - f64::max((1.0 - p3) * 2.0, 0.85);
    }

    p2 = p2.clamp(0.0, 1.0);
    p3 = p3.clamp(0.0, 1.0);
    let centre_val = s.rng.next_double();
    let note_count = s.get_random_note_count(p2, p3, 0.0, 0.0);
    let add_centre = t_cols % 2 == 1 && note_count != 3 && centre_val > 1.0 - centre_p;

    let half = (if t_cols % 2 == 0 { t_cols } else { t_cols - 1 }) / 2;
    let mut col = s.get_random_column(None, Some(s.random_start + half));
    for _ in 0..note_count {
        col = s.find_available_column(
            col,
            &[&*pattern],
            None,
            Some(s.random_start + half),
            false,
            None,
        )?;
        pattern.add(col, ho.start_time, ho.start_time);
        pattern.add(
            s.random_start + t_cols - col - 1,
            ho.start_time,
            ho.start_time,
        );
    }

    if add_centre {
        pattern.add(t_cols / 2, ho.start_time, ho.start_time);
    }

    circle_add_special_column(s, pattern, ho);
    Ok(())
}

fn circle_add_special_column(s: &ConversionState, pattern: &mut Pattern, ho: &StandardHitObject) {
    if s.random_start > 0
        && (ho.hitsound & HIT_CLAP != 0)
        && (ho.hitsound & HIT_FINISH != 0)
        && !pattern.has_column(0)
    {
        pattern.add(0, ho.start_time, ho.start_time);
    }
}

fn circle_cap_note_counts(t_cols: i32, p2: f64, p3: f64, p4: f64, p5: f64) -> (f64, f64, f64, f64) {
    if t_cols == 2 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    if t_cols == 3 {
        return (f64::min(p2, 0.1), 0.0, 0.0, 0.0);
    }
    if t_cols == 4 {
        return (f64::min(p2, 0.23), f64::min(p3, 0.04), 0.0, 0.0);
    }
    if t_cols == 5 {
        return (p2, f64::min(p3, 0.15), f64::min(p4, 0.03), 0.0);
    }
    (p2, p3, p4, p5)
}

fn circle_get_random_note_count(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    p2: f64,
    p3: f64,
    p4: f64,
    p5: f64,
) -> i32 {
    let (mut p2, p3, p4, p5) = circle_cap_note_counts(s.total_columns, p2, p3, p4, p5);
    if ho.hitsound & HIT_CLAP != 0 {
        p2 = 1.0;
    }
    s.get_random_note_count(p2, p3, p4, p5)
}
