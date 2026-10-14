//! A drawing as a normalised point cloud, and the distance between two of them ($P, Vatavu et al. 2012).

use crate::Stroke;

/// Points per cloud. 32 is the $P default: enough to tell shapes apart, cheap to match.
pub const CLOUD_POINTS: usize = 32;

/// A normalised drawing: `CLOUD_POINTS` points in a unit box, centred on their centroid.
#[derive(Clone, Debug, PartialEq)]
pub struct Cloud {
    pts: Vec<(f32, f32)>,
}

impl Cloud {
    /// `None` when there is nothing to match (no points at all).
    pub fn new(strokes: &[Stroke]) -> Option<Cloud> {
        let raw = resample(strokes, CLOUD_POINTS)?;
        Some(Cloud { pts: normalise(raw) })
    }

    pub fn points(&self) -> &[(f32, f32)] {
        &self.pts
    }

    /// 0 = identical. About the average distance between matched points, in unit-box units (so 0.05 is a
    /// close copy and anything over about 0.15 is a different drawing). Greedy cloud matching, both ways round.
    pub fn distance(&self, other: &Cloud) -> f32 {
        let n = self.pts.len().min(other.pts.len());
        let weights = (n * (n + 1) / 2) as f32;
        greedy(&self.pts, &other.pts).min(greedy(&other.pts, &self.pts)) / weights
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// n points evenly spaced along the drawn path. The jump between one stroke's end and the next one's start
/// does not count as path (the pen was up). A drawing that is only dots gets its points spread over the dots.
fn resample(strokes: &[Stroke], n: usize) -> Option<Vec<(f32, f32)>> {
    let strokes: Vec<&Stroke> = strokes.iter().filter(|s| !s.points.is_empty()).collect();
    if strokes.is_empty() {
        return None;
    }
    let mut total = 0.0;
    for s in &strokes {
        for w in s.points.windows(2) {
            total += dist((w[0].x, w[0].y), (w[1].x, w[1].y));
        }
    }
    if total <= f32::EPSILON {
        // dots only: repeat the dots round-robin so the cloud still has n points
        let all: Vec<(f32, f32)> = strokes.iter().flat_map(|s| s.points.iter().map(|p| (p.x, p.y))).collect();
        return Some((0..n).map(|i| all[i * all.len() / n]).collect());
    }
    let step = total / (n as f32 - 1.0);
    let mut out: Vec<(f32, f32)> = Vec::with_capacity(n);
    let mut carried = 0.0; // path walked since the last emitted point
    for s in &strokes {
        let pts: Vec<(f32, f32)> = s.points.iter().map(|p| (p.x, p.y)).collect();
        if out.is_empty() {
            out.push(pts[0]);
        }
        let mut prev = pts[0];
        for &p in &pts[1..] {
            let mut seg = dist(prev, p);
            let mut from = prev;
            while seg > 0.0 && carried + seg >= step {
                let t = (step - carried) / seg;
                let q = (from.0 + t * (p.0 - from.0), from.1 + t * (p.1 - from.1));
                out.push(q);
                from = q;
                seg = dist(from, p);
                carried = 0.0;
            }
            carried += seg;
            prev = p;
        }
        // the pen lifts: the distance owed carries into the next stroke, not the gap
    }
    while out.len() < n {
        // rounding left us one short: the last drawn point closes the cloud
        let last = strokes.last().and_then(|s| s.points.last()).map(|p| (p.x, p.y)).unwrap_or(out[0]);
        out.push(last);
    }
    out.truncate(n);
    Some(out)
}

/// Scale so the longer side is 1 (the aspect ratio is kept: a long word is not a tall symbol), move the
/// centroid to the origin.
fn normalise(mut pts: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in &pts {
        x0 = x0.min(p.0);
        y0 = y0.min(p.1);
        x1 = x1.max(p.0);
        y1 = y1.max(p.1);
    }
    let size = (x1 - x0).max(y1 - y0);
    let scale = if size > f32::EPSILON { 1.0 / size } else { 1.0 };
    for p in &mut pts {
        *p = ((p.0 - x0) * scale, (p.1 - y0) * scale);
    }
    let n = pts.len() as f32;
    let (cx, cy) = pts.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
    let (cx, cy) = (cx / n, cy / n);
    pts.iter().map(|p| (p.0 - cx, p.1 - cy)).collect()
}

/// Match each point of `a` to its nearest unused point of `b`, earlier points weighing more; try several
/// starting points and keep the best.
fn greedy(a: &[(f32, f32)], b: &[(f32, f32)]) -> f32 {
    let n = a.len();
    let step = ((n as f32).powf(0.5)).floor().max(1.0) as usize; // $P: epsilon = 0.5 gives n^0.5 starts
    let mut best = f32::MAX;
    let mut start = 0;
    while start < n {
        best = best.min(cloud_pass(a, b, start));
        best = best.min(cloud_pass(b, a, start));
        start += step;
    }
    best
}

fn cloud_pass(a: &[(f32, f32)], b: &[(f32, f32)], start: usize) -> f32 {
    let n = a.len();
    let mut matched = vec![false; n];
    let mut sum = 0.0;
    let mut i = start;
    let mut weight = n as f32;
    loop {
        let mut best = f32::MAX;
        let mut at = 0;
        for (j, &q) in b.iter().enumerate() {
            if !matched[j] {
                let d = dist(a[i], q);
                if d < best {
                    best = d;
                    at = j;
                }
            }
        }
        matched[at] = true;
        sum += weight * best;
        weight -= 1.0;
        i = (i + 1) % n;
        if i == start {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Point;

    fn st(pts: &[(f32, f32)]) -> Stroke {
        Stroke { points: pts.iter().map(|&(x, y)| Point { x, y }).collect() }
    }

    #[test]
    fn nothing_to_match_gives_none() {
        assert!(Cloud::new(&[]).is_none());
        assert!(Cloud::new(&[Stroke { points: vec![] }]).is_none());
    }

    #[test]
    fn a_cloud_has_the_fixed_size_and_is_centred_in_a_unit_box() {
        let c = Cloud::new(&[st(&[(10.0, 10.0), (110.0, 10.0), (110.0, 60.0)])]).unwrap();
        assert_eq!(c.points().len(), CLOUD_POINTS);
        let (sx, sy) = c.points().iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        assert!((sx / 32.0).abs() < 1e-4 && (sy / 32.0).abs() < 1e-4);
        let w = c.points().iter().map(|p| p.0).fold(f32::MIN, f32::max) - c.points().iter().map(|p| p.0).fold(f32::MAX, f32::min);
        assert!(w <= 1.0001 && w > 0.9);
    }

    #[test]
    fn a_dot_and_a_pile_of_dots_do_not_break_it() {
        assert!(Cloud::new(&[st(&[(5.0, 5.0)])]).is_some());
        assert!(Cloud::new(&[st(&[(5.0, 5.0), (5.0, 5.0), (5.0, 5.0)])]).is_some());
        assert!(Cloud::new(&[st(&[(1.0, 1.0)]), st(&[(9.0, 9.0)])]).is_some());
    }

    #[test]
    fn same_drawing_at_another_size_and_place_is_the_same_cloud() {
        let a = Cloud::new(&[st(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)])]).unwrap();
        let b = Cloud::new(&[st(&[(100.0, 50.0), (300.0, 50.0), (300.0, 250.0)])]).unwrap();
        assert!(a.distance(&b) < 0.05, "{}", a.distance(&b));
    }

    #[test]
    fn different_drawings_are_further_apart_than_the_same_one() {
        let l = Cloud::new(&[st(&[(0.0, 0.0), (0.0, 10.0), (10.0, 10.0)])]).unwrap();
        let l2 = Cloud::new(&[st(&[(0.0, 1.0), (0.5, 10.5), (10.0, 10.0)])]).unwrap();
        let line = Cloud::new(&[st(&[(0.0, 0.0), (10.0, 0.0)])]).unwrap();
        assert!(l.distance(&l2) < l.distance(&line));
    }

    #[test]
    fn stroke_count_and_order_do_not_matter() {
        let two = Cloud::new(&[st(&[(0.0, 5.0), (10.0, 5.0)]), st(&[(5.0, 0.0), (5.0, 10.0)])]).unwrap();
        let swapped = Cloud::new(&[st(&[(5.0, 0.0), (5.0, 10.0)]), st(&[(0.0, 5.0), (10.0, 5.0)])]).unwrap();
        let reversed = Cloud::new(&[st(&[(10.0, 5.0), (0.0, 5.0)]), st(&[(5.0, 10.0), (5.0, 0.0)])]).unwrap();
        assert!(two.distance(&swapped) < 0.05);
        assert!(two.distance(&reversed) < 0.05);
    }
}
