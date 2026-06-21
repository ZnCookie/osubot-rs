use crate::parser::round_half_even;

pub fn gif_frame_count(duration_ms: f64, fps: f64) -> usize {
    round_half_even(duration_ms * fps / 1000.0).max(1) as usize
}

pub fn gif_frame_duration_ms(fps: f64) -> u32 {
    round_half_even(1000.0 / fps).max(1) as u32
}

pub fn gif_snapshot_times(
    start_time: i64,
    frame_count: usize,
    speed_multiplier: f64,
    fps: f64,
) -> Vec<i64> {
    (0..frame_count)
        .map(|fi| start_time + round_half_even(fi as f64 * 1000.0 * speed_multiplier / fps))
        .collect()
}
