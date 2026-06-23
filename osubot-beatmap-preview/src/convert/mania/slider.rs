use super::*;

// ── slider generator ──

pub(super) struct SliderCtx {
    pub(super) start_time: i64,
    pub(super) spans: i32,
    pub(super) seg_dur: i64,
    pub(super) end_time: i64,
    pub(super) duration: i64,
    pub(super) convert_type: u32,
}

pub(super) fn slider_generate(
    s: &mut ConversionState,
    ho: &StandardHitObject,
) -> Result<Vec<ManiaHitObject>> {
    let spans = i32::max(1, ho.slider_repeats);
    let seg_dur = s.slider_segment_duration(ho);
    let end_time = ho.start_time + seg_dur * spans as i64;
    let mut ctx = SliderCtx {
        start_time: ho.start_time,
        spans,
        seg_dur,
        end_time,
        duration: end_time - ho.start_time,
        convert_type: if kiai_at(ho.start_time, s.timing_points) {
            0
        } else {
            P_LOW_PROBABILITY
        },
    };

    let pattern = if ctx.spans > 1 {
        slider_gen_multi_span(s, ho, &mut ctx)?
    } else {
        slider_gen_single_span(s, ho, &mut ctx)?
    };

    Ok(slider_split_patterns(s, &ctx, pattern))
}

fn slider_split_patterns(
    s: &mut ConversionState,
    ctx: &SliderCtx,
    pattern: Pattern,
) -> Vec<ManiaHitObject> {
    if pattern.objects.len() == 1 {
        s.prev_pattern = pattern;
        return s.prev_pattern.objects.clone();
    }

    let mut intermediate = Pattern::new();
    let mut end_pattern = Pattern::new();
    for obj in &pattern.objects {
        if ctx.end_time != obj.end_time {
            intermediate.add(obj.lane, obj.start_time, obj.end_time);
        } else {
            end_pattern.add(obj.lane, obj.start_time, obj.end_time);
        }
    }

    let mut out = intermediate.objects;
    out.extend(end_pattern.objects.iter().copied());
    s.prev_pattern = end_pattern;
    out
}

fn slider_gen_multi_span(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &mut SliderCtx,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let seg = ctx.seg_dur;

    if seg <= 90 {
        return slider_gen_holds(s, ctx, 1);
    }
    if seg <= 120 {
        ctx.convert_type |= P_FORCE_NOT_STACK;
        return slider_gen_notes_no_stack(s, ho, ctx, ctx.spans + 1);
    }
    if seg <= 160 {
        return slider_gen_stair(s, ho, ctx);
    }
    if seg <= 200 && s.conv_diff > 3.0 {
        return slider_gen_random_multiple(s, ho, ctx);
    }

    if ctx.duration >= 4000 {
        return slider_gen_n_random_notes(s, ho, ctx, 0.23, 0.0, 0.0);
    }

    if seg > 400 && ctx.spans < t_cols - 1 - s.random_start {
        return slider_gen_tiled_holds(s, ho, ctx);
    }

    slider_gen_hold_and_normal(s, ho, ctx)
}

fn slider_gen_single_span(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &mut SliderCtx,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let cd = s.conv_diff;
    let lp = ctx.convert_type & P_LOW_PROBABILITY != 0;
    let seg = ctx.seg_dur;

    if seg <= 110 {
        if s.prev_pattern.column_count() < t_cols {
            ctx.convert_type |= P_FORCE_NOT_STACK;
        } else {
            ctx.convert_type &= !P_FORCE_NOT_STACK;
        }
        return slider_gen_notes_no_stack(s, ho, ctx, if seg < 80 { 1 } else { 2 });
    }

    // graded by ConversionDifficulty
    let (p2, p3): (f64, f64) = if cd > 6.5 {
        if lp {
            (0.78, 0.3)
        } else {
            (0.85, 0.36)
        }
    } else if cd > 4.0 {
        if lp {
            (0.43, 0.08)
        } else {
            (0.56, 0.18)
        }
    } else if cd > 2.5 {
        if lp {
            (0.3, 0.0)
        } else {
            (0.37, 0.08)
        }
    } else if lp {
        (0.17, 0.0)
    } else {
        (0.27, 0.0)
    };

    let (p2, p3, _p4) = slider_cap_hold_counts(t_cols, p2, p3, 0.0);
    slider_gen_n_random_notes(s, ho, ctx, p2, p3, 0.0)
}

fn slider_gen_holds(s: &mut ConversionState, ctx: &SliderCtx, count: i32) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let prev = s.prev_pattern.clone();
    let usable = t_cols - s.random_start - prev.column_count();
    let count = i32::min(count, t_cols - s.random_start);
    let mut pattern = Pattern::new();
    let mut col = s.get_random_column(None, None);
    let n1 = i32::min(usable, count);
    for _ in 0..n1 {
        col = s.find_available_column(col, &[&pattern, &prev], None, None, false, None)?;
        pattern.add(col, ctx.start_time, ctx.end_time);
    }
    for _ in 0..(count - n1) {
        col = s.find_available_column(col, &[&pattern], None, None, false, None)?;
        pattern.add(col, ctx.start_time, ctx.end_time);
    }
    Ok(pattern)
}

fn slider_gen_notes_no_stack(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &SliderCtx,
    count: i32,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let prev = s.prev_pattern.clone();
    let mut pattern = Pattern::new();
    let mut col = s.get_column(ho.x, true);
    if ctx.convert_type & P_FORCE_NOT_STACK != 0 && prev.column_count() < t_cols {
        col = s.find_available_column(col, &[&prev], None, None, false, None)?;
    }

    let mut last_col = col;
    for i in 0..count {
        let t = ctx.start_time + i as i64 * ctx.seg_dur;
        pattern.add(col, t, t);
        col = s.find_available_column(col, &[], None, None, false, Some(last_col))?;
        last_col = col;
    }
    Ok(pattern)
}

fn slider_gen_stair(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &SliderCtx,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let mut col = s.get_column(ho.x, true);
    let mut increasing = s.rng.next_double() > 0.5;
    let mut pattern = Pattern::new();
    for i in 0..(ctx.spans + 1) {
        let t = ctx.start_time + i as i64 * ctx.seg_dur;
        pattern.add(col, t, t);
        if increasing {
            if col >= t_cols - 1 {
                increasing = false;
                col -= 1;
            } else {
                col += 1;
            }
        } else if col <= s.random_start {
            increasing = true;
            col += 1;
        } else {
            col -= 1;
        }
    }
    Ok(pattern)
}

fn slider_gen_random_multiple(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &SliderCtx,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let legacy = (4..=8).contains(&t_cols);
    let interval = s.rng.next_range(1, t_cols - if legacy { 1 } else { 0 });
    let mut col = s.get_column(ho.x, true);
    let mut pattern = Pattern::new();
    for i in 0..(ctx.spans + 1) {
        let t = ctx.start_time + i as i64 * ctx.seg_dur;
        pattern.add(col, t, t);
        let mut col2 = col + interval;
        if col2 >= t_cols - s.random_start {
            col2 = col2 - t_cols + if legacy { 1 } else { 0 };
        }
        col2 += s.random_start;
        if t_cols > 2 {
            pattern.add(col2, t, t);
        }
        col = s.get_random_column(None, None);
    }
    Ok(pattern)
}

fn slider_gen_tiled_holds(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &SliderCtx,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let col_repeat = i32::min(ctx.spans, t_cols);
    let prev = s.prev_pattern.clone();
    let mut col = s.get_column(ho.x, true);
    if ctx.convert_type & P_FORCE_NOT_STACK != 0 && prev.column_count() < t_cols {
        col = s.find_available_column(col, &[&prev], None, None, false, None)?;
    }

    let mut pattern = Pattern::new();
    for i in 0..col_repeat {
        let t = ctx.start_time + i as i64 * ctx.seg_dur;
        col = s.find_available_column(col, &[&pattern], None, None, false, None)?;
        pattern.add(col, t, ctx.end_time);
    }
    Ok(pattern)
}

fn slider_gen_hold_and_normal(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &SliderCtx,
) -> Result<Pattern> {
    let t_cols = s.total_columns;
    let cd = s.conv_diff;
    let prev = s.prev_pattern.clone();
    let mut col = s.get_column(ho.x, true);
    if ctx.convert_type & P_FORCE_NOT_STACK != 0 && prev.column_count() < t_cols {
        col = s.find_available_column(col, &[&prev], None, None, false, None)?;
    }

    let mut pattern = Pattern::new();
    pattern.add(col, ctx.start_time, ctx.end_time); // hold

    // accompanying note count
    let mut nc = if cd > 6.5 {
        s.get_random_note_count(0.63, 0.0, 0.0, 0.0)
    } else if cd > 4.0 {
        s.get_random_note_count(if t_cols < 6 { 0.12 } else { 0.45 }, 0.0, 0.0, 0.0)
    } else if cd > 2.5 {
        s.get_random_note_count(if t_cols < 6 { 0.0 } else { 0.24 }, 0.0, 0.0, 0.0)
    } else {
        0
    };
    nc = i32::min(t_cols - 1, nc);

    let mut next_col = s.get_random_column(None, None);
    let mut row = Pattern::new();
    let ignore_head =
        slider_hitsound_at(ho, ctx, ctx.start_time) & (HIT_WHISTLE | HIT_FINISH | HIT_CLAP) == 0;

    for i in 0..(ctx.spans + 1) {
        let t = ctx.start_time + i as i64 * ctx.seg_dur;
        if !(ignore_head && t == ctx.start_time) {
            for _ in 0..nc {
                next_col =
                    s.find_available_column(next_col, &[&row], None, None, false, Some(col))?;
                row.add(next_col, t, t);
            }
        }
        for obj in row.objects.clone() {
            pattern.add(obj.lane, obj.start_time, obj.end_time);
        }
        row = Pattern::new();
    }

    Ok(pattern)
}

fn slider_gen_n_random_notes(
    s: &mut ConversionState,
    ho: &StandardHitObject,
    ctx: &SliderCtx,
    p2: f64,
    p3: f64,
    p4: f64,
) -> Result<Pattern> {
    let mut can_generate_two_notes = ctx.convert_type & P_LOW_PROBABILITY == 0;
    can_generate_two_notes = can_generate_two_notes
        && ((ho.hitsound & (HIT_CLAP | HIT_FINISH) != 0)
            || (slider_hitsound_at(ho, ctx, ctx.start_time) & (HIT_CLAP | HIT_FINISH) != 0));
    let p2 = if can_generate_two_notes { 1.0 } else { p2 };
    let nc = s.get_random_note_count(p2, p3, p4, 0.0);
    slider_gen_holds(s, ctx, nc)
}

fn slider_hitsound_at(ho: &StandardHitObject, ctx: &SliderCtx, time: i64) -> i32 {
    if ho.slider_edge_hitsounds.is_empty() {
        return ho.hitsound;
    }
    let index: i64 = if ctx.seg_dur == 0 {
        0
    } else {
        (time - ctx.start_time).div_euclid(ctx.seg_dur)
    };
    let index = i64::max(
        0,
        i64::min(index, ho.slider_edge_hitsounds.len() as i64 - 1),
    );
    ho.slider_edge_hitsounds[index as usize]
}

fn slider_cap_hold_counts(t_cols: i32, p2: f64, p3: f64, p4: f64) -> (f64, f64, f64) {
    if t_cols == 2 {
        return (0.0, 0.0, 0.0);
    }
    if t_cols == 3 {
        return (f64::min(p2, 0.1), 0.0, 0.0);
    }
    if t_cols == 4 {
        return (f64::min(p2, 0.3), f64::min(p3, 0.04), 0.0);
    }
    if t_cols == 5 {
        return (f64::min(p2, 0.34), f64::min(p3, 0.1), f64::min(p4, 0.03));
    }
    (p2, p3, p4)
}
