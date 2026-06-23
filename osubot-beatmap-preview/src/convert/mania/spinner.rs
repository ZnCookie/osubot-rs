use super::*;

// ── spinner generator ──

pub(super) fn spinner_generate(
    s: &mut ConversionState,
    ho: &StandardHitObject,
) -> Result<Vec<ManiaHitObject>> {
    let t_cols = s.total_columns;
    let dur = ho.end_time - ho.start_time;
    let is_hold = dur >= 100;
    let prev = s.prev_pattern.clone();
    let force_not = prev.column_count() < t_cols;

    let col = if force_not {
        let start = s.get_random_column(None, None);
        s.find_available_column(start, &[&prev], None, None, false, None)?
    } else {
        s.get_random_column(Some(0), None)
    };

    let end = if is_hold { ho.end_time } else { ho.start_time };
    let mut pattern = Pattern::new();
    pattern.add(col, ho.start_time, end);
    Ok(pattern.objects)
}
