// Copyright (c) 2026 xuan_yuan (from osu-beatmap-preview, MIT licensed)
// Copyright (c) 2026 ZnCookie
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Slider path approximation shared by standard and catch.
//! Standard applies RDP simplification + truncate/extend fit;
//! catch keeps full point sets + lazer calculateLength-style fit.

pub const BEZIER_TOLERANCE: f64 = 0.25;
pub const CATMULL_DETAIL: usize = 50;
pub const CATMULL_MIN_DISTANCE: f64 = 6.0;

pub type P = (f64, f64);

#[derive(Debug, Clone, Default)]
pub struct SliderPath {
    pub points: Vec<P>,
    pub cumulative_lengths: Vec<f64>,
    pub total_length: f64,
}

fn dist(a: P, b: P) -> f64 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    (dx * dx + dy * dy).sqrt()
}

pub fn build_path(points: &[P]) -> SliderPath {
    let deduped = dedupe_points(points);
    if deduped.is_empty() {
        return SliderPath::default();
    }
    let mut cumulative = Vec::with_capacity(deduped.len());
    cumulative.push(0.0);
    let mut travelled = 0.0;
    for i in 1..deduped.len() {
        travelled += dist(deduped[i - 1], deduped[i]);
        cumulative.push(travelled);
    }
    SliderPath {
        points: deduped,
        cumulative_lengths: cumulative,
        total_length: travelled,
    }
}

pub fn path_position_at(path: &SliderPath, progress: f64) -> P {
    if path.points.is_empty() {
        return (0.0, 0.0);
    }
    if path.points.len() < 2 || path.total_length <= 0.0 {
        return path.points[0];
    }
    let target = path.total_length * progress.clamp(0.0, 1.0);
    path_position_at_distance(path, target)
}

pub fn path_position_at_distance(path: &SliderPath, target: f64) -> P {
    if target <= 0.0 {
        return path.points[0];
    }
    if target >= path.total_length {
        return *path
            .points
            .last()
            .expect("SliderPath must have at least 1 point");
    }
    // bisect_right equivalent
    let index = path.cumulative_lengths.partition_point(|&v| v <= target);
    let previous_index = index.saturating_sub(1);
    let next_index = index.min(path.points.len() - 1);
    let previous = path.points[previous_index];
    let current = path.points[next_index];
    let segment_length =
        path.cumulative_lengths[next_index] - path.cumulative_lengths[previous_index];
    if segment_length <= 0.0 {
        return current;
    }
    let ratio = (target - path.cumulative_lengths[previous_index]) / segment_length;
    (
        previous.0 + (current.0 - previous.0) * ratio,
        previous.1 + (current.1 - previous.1) * ratio,
    )
}

pub fn slice_path(path: &SliderPath, start_progress: f64, end_progress: f64) -> Vec<P> {
    if path.points.len() < 2 || path.total_length <= 0.0 {
        return path.points.clone();
    }
    let (mut s, mut e) = (start_progress, end_progress);
    if s > e {
        std::mem::swap(&mut s, &mut e);
    }
    let s = s.clamp(0.0, 1.0);
    let e = e.clamp(0.0, 1.0);
    let start_distance = path.total_length * s;
    let end_distance = path.total_length * e;
    let mut sliced = vec![path_position_at_distance(path, start_distance)];
    for index in 1..path.cumulative_lengths.len() - 1 {
        let distance = path.cumulative_lengths[index];
        if start_distance < distance && distance < end_distance {
            sliced.push(path.points[index]);
        }
    }
    sliced.push(path_position_at_distance(path, end_distance));
    dedupe_points(&sliced)
}

/// Build the raw curve (before length fitting) for the given slider type.
fn approximate_curve(slider_type: &str, points: &[P], perfect_lazer_semantics: bool) -> Vec<P> {
    match slider_type {
        "L" => points.to_vec(),
        "P" => approximate_perfect_curve(points, perfect_lazer_semantics),
        "C" => approximate_catmull(points),
        _ => approximate_bezier_segments(points),
    }
}

/// Standard-mode slider path: simplify (RDP) + truncate/extend fit.
pub fn build_standard_slider_path(
    x: i32,
    y: i32,
    slider_points: &[(i32, i32)],
    slider_type: &str,
    pixel_length: f64,
) -> SliderPath {
    let mut points: Vec<P> = Vec::with_capacity(slider_points.len() + 1);
    points.push((x as f64, y as f64));
    points.extend(slider_points.iter().map(|&(px, py)| (px as f64, py as f64)));
    let path = approximate_curve(slider_type, &points, false);
    let fitted = fit_path_truncate_extend(&path, pixel_length);
    build_path(&simplify_path(&fitted, 1.0))
}

/// Catch-mode slider path: NO simplification, lazer calculateLength fit.
pub fn build_catch_slider_path(
    x: i32,
    y: i32,
    slider_points: &[(i32, i32)],
    slider_type: &str,
    pixel_length: f64,
) -> SliderPath {
    let mut points: Vec<P> = Vec::with_capacity(slider_points.len() + 1);
    points.push((x as f64, y as f64));
    points.extend(slider_points.iter().map(|&(px, py)| (px as f64, py as f64)));
    let path = approximate_curve(slider_type, &points, true);
    build_path(&fit_path_lazer(&path, pixel_length))
}

pub fn simplify_path(points: &[P], tolerance: f64) -> Vec<P> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let tolerance_sq = tolerance * tolerance;
    let mut result: Vec<usize> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    stack.push((0, points.len() - 1));

    while let Some((start, end)) = stack.pop() {
        if end - start < 2 {
            if result.is_empty()
                || *result
                    .last()
                    .expect("result non-empty after is_empty guard")
                    != start
            {
                result.push(start);
            }
            result.push(end);
            continue;
        }

        let (sx, sy) = points[start];
        let (ex, ey) = points[end];
        let dx = ex - sx;
        let dy = ey - sy;
        let line_len_sq = dx * dx + dy * dy;
        let mut max_dist_sq = 0.0;
        let mut max_idx = start;

        for (i, p) in points.iter().enumerate().take(end).skip(start + 1) {
            let dist_sq = if line_len_sq < 0.0001 {
                (p.0 - sx).powi(2) + (p.1 - sy).powi(2)
            } else {
                let t = (((p.0 - sx) * dx + (p.1 - sy) * dy) / line_len_sq).clamp(0.0, 1.0);
                let px = sx + t * dx;
                let py = sy + t * dy;
                (p.0 - px).powi(2) + (p.1 - py).powi(2)
            };
            if dist_sq > max_dist_sq {
                max_dist_sq = dist_sq;
                max_idx = i;
            }
        }

        if max_dist_sq <= tolerance_sq {
            if result.is_empty()
                || *result
                    .last()
                    .expect("result non-empty after is_empty guard")
                    != start
            {
                result.push(start);
            }
            result.push(end);
        } else {
            stack.push((max_idx, end));
            stack.push((start, max_idx));
        }
    }

    result.sort_unstable();
    result.dedup();
    result.into_iter().map(|i| points[i]).collect()
}

// ——— Bezier ———

fn approximate_bezier_segments(points: &[P]) -> Vec<P> {
    let mut path: Vec<P> = Vec::new();
    let mut segment = vec![points[0]];
    for &point in &points[1..] {
        segment.push(point);
        if segment.len() > 2 && point == segment[segment.len() - 2] {
            segment.pop();
            path.extend(approximate_bezier(&segment));
            segment = vec![point];
        }
    }
    if segment.len() > 1 {
        path.extend(approximate_bezier(&segment));
    }
    dedupe_points(&path)
}

fn approximate_bezier(points: &[P]) -> Vec<P> {
    if points.len() < 2 {
        return points.to_vec();
    }
    if points.len() == 2 {
        return vec![points[0], points[1]];
    }
    let mut result = vec![points[0]];
    let mut stack: Vec<Vec<P>> = vec![points.to_vec()];
    while let Some(parent) = stack.pop() {
        if bezier_is_flat_enough(&parent) {
            result.extend(bezier_approximate(&parent));
        } else {
            let (left, right) = bezier_subdivide(&parent);
            stack.push(right);
            stack.push(left);
        }
    }
    result.push(*points.last().expect("RDP points non-empty"));
    result
}

fn bezier_is_flat_enough(points: &[P]) -> bool {
    let threshold = BEZIER_TOLERANCE * BEZIER_TOLERANCE * 4.0;
    for i in 1..points.len() - 1 {
        let dx = points[i - 1].0 - 2.0 * points[i].0 + points[i + 1].0;
        let dy = points[i - 1].1 - 2.0 * points[i].1 + points[i + 1].1;
        if dx * dx + dy * dy > threshold {
            return false;
        }
    }
    true
}

fn bezier_subdivide(points: &[P]) -> (Vec<P>, Vec<P>) {
    let count = points.len();
    let mut midpoints = points.to_vec();
    let mut left = vec![points[0]; count];
    let mut right = vec![*points.last().expect("points non-empty for extend"); count];
    for i in 0..count {
        left[i] = midpoints[0];
        right[count - i - 1] = midpoints[count - i - 1];
        for j in 0..count - i - 1 {
            midpoints[j] = (
                (midpoints[j].0 + midpoints[j + 1].0) / 2.0,
                (midpoints[j].1 + midpoints[j + 1].1) / 2.0,
            );
        }
    }
    (left, right)
}

fn bezier_approximate(points: &[P]) -> Vec<P> {
    let count = points.len();
    let (left, _) = bezier_subdivide(points);
    let mut output = Vec::with_capacity(count.saturating_sub(2));
    for i in 1..count - 1 {
        let p0 = left[i - 1];
        let p1 = left[i];
        let p2 = left[i + 1];
        output.push((
            0.25 * (p0.0 + 2.0 * p1.0 + p2.0),
            0.25 * (p0.1 + 2.0 * p1.1 + p2.1),
        ));
    }
    output
}

// ——— Perfect circle ———

fn approximate_perfect_curve(points: &[P], lazer_semantics: bool) -> Vec<P> {
    if points.len() != 3 || are_collinear(points[0], points[1], points[2]) {
        return approximate_bezier_segments(points);
    }
    let centre = circle_centre(points[0], points[1], points[2]);
    let radius = dist(centre, points[0]);
    let start_angle = (points[0].1 - centre.1).atan2(points[0].0 - centre.0);
    let middle_angle = (points[1].1 - centre.1).atan2(points[1].0 - centre.0);
    let end_angle = (points[2].1 - centre.1).atan2(points[2].0 - centre.0);
    let end_angle = normalise_arc_end(start_angle, middle_angle, end_angle);
    let theta_range = end_angle - start_angle;

    let tau = std::f64::consts::TAU;
    let step_angle = if radius > 0.1 {
        2.0 * (1.0 - 0.1 / radius).clamp(-1.0, 1.0).acos()
    } else {
        tau
    };
    let n = ((theta_range.abs() / step_angle).ceil() as i64).max(2);
    if n >= 1000 {
        return approximate_bezier_segments(points);
    }

    if lazer_semantics {
        // n = point count; divide by (n - 1)
        let point_count = n;
        (0..point_count)
            .map(|index| {
                let angle = start_angle + theta_range * index as f64 / (point_count - 1) as f64;
                (
                    centre.0 + angle.cos() * radius,
                    centre.1 + angle.sin() * radius,
                )
            })
            .collect()
    } else {
        // n = segment count; produce n+1 points dividing by n
        let steps = n;
        (0..=steps)
            .map(|index| {
                let angle = start_angle + theta_range * index as f64 / steps as f64;
                (
                    centre.0 + angle.cos() * radius,
                    centre.1 + angle.sin() * radius,
                )
            })
            .collect()
    }
}

fn circle_centre(first: P, second: P, third: P) -> P {
    let (ax, ay) = first;
    let (bx, by) = second;
    let (cx, cy) = third;
    let d = 2.0 * (ax * (by - cy) + bx * (cy - ay) + cx * (ay - by));
    let ux = ((ax * ax + ay * ay) * (by - cy)
        + (bx * bx + by * by) * (cy - ay)
        + (cx * cx + cy * cy) * (ay - by))
        / d;
    let uy = ((ax * ax + ay * ay) * (cx - bx)
        + (bx * bx + by * by) * (ax - cx)
        + (cx * cx + cy * cy) * (bx - ax))
        / d;
    (ux, uy)
}

fn normalise_arc_end(start: f64, middle: f64, end: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let mut end = end;
    while end < start {
        end += tau;
    }
    let mut middle_forward = middle;
    while middle_forward < start {
        middle_forward += tau;
    }
    if middle_forward <= end {
        return end;
    }
    while end > start {
        end -= tau;
    }
    end
}

fn are_collinear(first: P, second: P, third: P) -> bool {
    ((second.1 - first.1) * (third.0 - first.0) - (second.0 - first.0) * (third.1 - first.1)).abs()
        < 0.001
}

// ——— Catmull ———

fn approximate_catmull(points: &[P]) -> Vec<P> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let mut path: Vec<P> = Vec::new();
    let mut extended = Vec::with_capacity(points.len() + 2);
    extended.push(points[0]);
    extended.extend_from_slice(points);
    extended.push(*points.last().expect("catmull points non-empty"));
    for index in 1..extended.len() - 2 {
        let p0 = extended[index - 1];
        let p1 = extended[index];
        let p2 = extended[index + 1];
        let p3 = extended[index + 2];
        for step in 0..CATMULL_DETAIL {
            path.push(catmull_at(
                p0,
                p1,
                p2,
                p3,
                step as f64 / CATMULL_DETAIL as f64,
            ));
        }
    }
    path.push(
        *points
            .last()
            .expect("catmull points = extended[1..=n] non-empty"),
    );
    catmull_optimise(&path, points)
}

fn catmull_at(p0: P, p1: P, p2: P, p3: P, t: f64) -> P {
    let t2 = t * t;
    let t3 = t2 * t;
    let x = 0.5
        * ((2.0 * p1.0)
            + (-p0.0 + p2.0) * t
            + (2.0 * p0.0 - 5.0 * p1.0 + 4.0 * p2.0 - p3.0) * t2
            + (-p0.0 + 3.0 * p1.0 - 3.0 * p2.0 + p3.0) * t3);
    let y = 0.5
        * ((2.0 * p1.1)
            + (-p0.1 + p2.1) * t
            + (2.0 * p0.1 - 5.0 * p1.1 + 4.0 * p2.1 - p3.1) * t2
            + (-p0.1 + 3.0 * p1.1 - 3.0 * p2.1 + p3.1) * t3);
    (x, y)
}

fn catmull_optimise(path: &[P], knots: &[P]) -> Vec<P> {
    let is_knot = |p: P| knots.contains(&p);
    let mut result = vec![path[0]];
    for i in 1..path.len() {
        let prev = *result.last().expect("result initialized with path[0]");
        let curr = path[i];
        if dist(prev, curr) >= CATMULL_MIN_DISTANCE || is_knot(curr) || i == path.len() - 1 {
            result.push(curr);
        }
    }
    result
}

// ——— Length fitting ———

/// Standard variant: truncate at expected length or extend final point outward.
fn fit_path_truncate_extend(path: &[P], expected_length: f64) -> Vec<P> {
    if path.len() < 2 || expected_length <= 0.0 {
        return path.to_vec();
    }
    let mut cumulative = Vec::with_capacity(path.len());
    cumulative.push(0.0);
    let mut travelled = 0.0;
    for i in 1..path.len() {
        travelled += dist(path[i - 1], path[i]);
        cumulative.push(travelled);
    }
    if travelled <= 0.0 {
        return path.to_vec();
    }

    if travelled > expected_length {
        let mut fitted = vec![path[0]];
        let mut previous_distance = 0.0;
        for i in 1..path.len() {
            let current_distance = cumulative[i];
            let previous = path[i - 1];
            let current = path[i];
            if current_distance >= expected_length {
                let segment_length = current_distance - previous_distance;
                if segment_length <= 0.0 {
                    fitted.push(current);
                } else {
                    let ratio = (expected_length - previous_distance) / segment_length;
                    fitted.push((
                        previous.0 + (current.0 - previous.0) * ratio,
                        previous.1 + (current.1 - previous.1) * ratio,
                    ));
                }
                return fitted;
            }
            fitted.push(current);
            previous_distance = current_distance;
        }
        return fitted;
    }

    let mut fitted = path.to_vec();
    let n = fitted.len();
    if fitted[n - 1] == fitted[n - 2] {
        return fitted;
    }
    let remaining = expected_length - travelled;
    let direction = (
        fitted[n - 1].0 - fitted[n - 2].0,
        fitted[n - 1].1 - fitted[n - 2].1,
    );
    let direction_length = (direction.0 * direction.0 + direction.1 * direction.1).sqrt();
    if direction_length > 0.0 {
        fitted[n - 1] = (
            fitted[n - 1].0 + direction.0 / direction_length * remaining,
            fitted[n - 1].1 + direction.1 / direction_length * remaining,
        );
    }
    fitted
}

/// Catch variant: lazer SliderPath.calculateLength semantics.
fn fit_path_lazer(path: &[P], expected_length: f64) -> Vec<P> {
    if path.len() < 2 || expected_length <= 0.0 {
        return path.to_vec();
    }
    let mut cumulative = Vec::with_capacity(path.len());
    cumulative.push(0.0);
    let mut travelled = 0.0;
    for i in 1..path.len() {
        travelled += dist(path[i - 1], path[i]);
        cumulative.push(travelled);
    }
    if travelled <= 0.0 {
        return path.to_vec();
    }

    let mut fitted = path.to_vec();
    let mut fitted_lengths = cumulative;

    if travelled > expected_length {
        while !fitted_lengths.is_empty()
            && *fitted_lengths.last().expect("non-empty guard above") >= expected_length
        {
            fitted_lengths.pop();
            fitted.pop();
        }
        if fitted.is_empty() {
            return vec![path[0]];
        }
        let path_end_index = fitted.len();
        fitted.push(path[path_end_index]);
        fitted_lengths.push(expected_length);
    }

    let n = fitted.len();
    if n < 2 || fitted[n - 1] == fitted[n - 2] {
        return fitted;
    }
    let remaining = expected_length - fitted_lengths[fitted_lengths.len() - 2];
    let direction = (
        fitted[n - 1].0 - fitted[n - 2].0,
        fitted[n - 1].1 - fitted[n - 2].1,
    );
    let direction_length = (direction.0 * direction.0 + direction.1 * direction.1).sqrt();
    if direction_length > 0.0 {
        fitted[n - 1] = (
            fitted[n - 2].0 + direction.0 / direction_length * remaining,
            fitted[n - 2].1 + direction.1 / direction_length * remaining,
        );
    }
    fitted
}

pub fn dedupe_points(points: &[P]) -> Vec<P> {
    let mut deduped: Vec<P> = Vec::with_capacity(points.len());
    for &point in points {
        if deduped.last() != Some(&point) {
            deduped.push(point);
        }
    }
    deduped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perfect_curve_collinear_points() {
        let points = [(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)];
        let result = approximate_perfect_curve(&points, false);
        assert!(
            !result.is_empty(),
            "collinear points should produce a valid path"
        );
        for &p in &result {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn test_perfect_curve_duplicate_points() {
        let points = [(0.0, 0.0), (0.0, 0.0), (100.0, 0.0)];
        let result = approximate_perfect_curve(&points, false);
        assert!(
            !result.is_empty(),
            "duplicate points should produce a valid path"
        );
        for &p in &result {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn test_bezier_single_point() {
        let points = [(42.0, 73.0)];
        let result = approximate_bezier(&points);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], (42.0, 73.0));
    }

    #[test]
    fn test_catmull_endpoint_behavior() {
        let points = [(0.0, 0.0), (50.0, 100.0), (100.0, 0.0)];
        let result = approximate_catmull(&points);
        assert!(!result.is_empty(), "catmull should produce a valid path");
        assert_eq!(
            result[0], points[0],
            "first output point must match first control point"
        );
        assert_eq!(
            *result.last().unwrap(),
            *points.last().unwrap(),
            "last output point must match last control point"
        );
        for &p in &result {
            assert!(p.0.is_finite() && p.1.is_finite());
        }
    }

    #[test]
    fn test_acos_clamp_stable() {
        let points = [(0.0, 0.0), (50.0, 0.0), (25.0, 0.1)];
        let result = approximate_perfect_curve(&points, false);
        assert!(!result.is_empty(), "should produce a valid path");
        for &p in &result {
            assert!(p.0.is_finite() && p.1.is_finite(), "no NaN should appear");
        }
    }
}
