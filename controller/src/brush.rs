//! The brush engine: pen samples in, dabs out. Circle brush only for now (the Tauri overlay's brush
//! engine has airbrush and rectangle brushes to port later).
//!
//! No GPU and no window in here, so it is unit-tested on any machine.

use crate::renderer::Dab;
use pen_proto::{Phase, Sample};

fn radius_for(p: f32) -> f32 {
    1.0 + 7.0 * p
}
fn dab(x: f32, y: f32, p: f32) -> Dab {
    Dab { x, y, radius: radius_for(p), alpha: 1.0 }
}

#[derive(Default)]
pub struct Stroker {
    last: Option<(f32, f32, f32)>,
    acc: f32,
}

impl Stroker {
    /// Feed one sample; the dabs it produces are appended to `out`.
    pub fn feed(&mut self, s: Sample, out: &mut Vec<Dab>) {
        let pressure = s.pressure.unwrap_or(1.0); // a pen without pressure draws at full size
        match s.phase {
            Phase::Hover | Phase::Up => self.last = None,
            Phase::Down | Phase::Move => match self.last {
                None => {
                    out.push(dab(s.x, s.y, pressure));
                    self.acc = 0.0;
                    self.last = Some((s.x, s.y, pressure));
                }
                Some((lx, ly, lp)) => {
                    let (dx, dy) = (s.x - lx, s.y - ly);
                    let l = dx.hypot(dy);
                    if l < 0.5 {
                        return;
                    }
                    let sp = (radius_for(pressure) * 0.15).max(0.7);
                    let mut need = sp - self.acc;
                    while need <= l {
                        let t = need / l;
                        out.push(dab(lx + dx * t, ly + dy * t, lp + (pressure - lp) * t));
                        need += sp;
                    }
                    self.acc = l - (need - sp);
                    self.last = Some((s.x, s.y, pressure));
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(phase: Phase, x: f32, y: f32, pressure: Option<f32>) -> Sample {
        Sample {
            pen_id: 0,
            phase,
            x,
            y,
            tablet: None,
            pressure,
            tilt: None,
            buttons: 0,
            eraser: false,
            t_us: 0,
            t_hardware: false,
        }
    }

    fn run(samples: &[Sample]) -> Vec<Dab> {
        let mut s = Stroker::default();
        let mut out = Vec::new();
        for x in samples {
            s.feed(*x, &mut out);
        }
        out
    }

    #[test]
    fn a_tap_leaves_exactly_one_dab() {
        let dabs = run(&[sample(Phase::Down, 10.0, 20.0, Some(1.0)), sample(Phase::Up, 10.0, 20.0, Some(0.0))]);
        assert_eq!(dabs.len(), 1);
        assert_eq!((dabs[0].x, dabs[0].y), (10.0, 20.0));
        assert_eq!(dabs[0].radius, radius_for(1.0));
    }

    #[test]
    fn hovering_draws_nothing() {
        let dabs = run(&[sample(Phase::Hover, 1.0, 1.0, Some(0.0)), sample(Phase::Hover, 50.0, 50.0, Some(0.0))]);
        assert!(dabs.is_empty());
    }

    #[test]
    fn a_straight_stroke_is_evenly_spaced() {
        let dabs = run(&[sample(Phase::Down, 0.0, 0.0, Some(1.0)), sample(Phase::Move, 100.0, 0.0, Some(1.0))]);
        let spacing = (radius_for(1.0) * 0.15).max(0.7);
        assert!((80..=90).contains(&dabs.len()), "got {} dabs", dabs.len());
        for pair in dabs.windows(2) {
            assert!((pair[1].x - pair[0].x - spacing).abs() < 1e-3);
            assert_eq!(pair[0].y, 0.0);
        }
        assert!(dabs.last().unwrap().x <= 100.0);
    }

    #[test]
    fn spacing_carries_over_between_samples() {
        // the same stroke delivered as many small moves must produce the same dabs as one long move
        let one = run(&[sample(Phase::Down, 0.0, 0.0, Some(1.0)), sample(Phase::Move, 60.0, 0.0, Some(1.0))]);
        let mut many = vec![sample(Phase::Down, 0.0, 0.0, Some(1.0))];
        for i in 1..=60 {
            many.push(sample(Phase::Move, i as f32, 0.0, Some(1.0)));
        }
        let many = run(&many);
        assert_eq!(one.len(), many.len());
        for (a, b) in one.iter().zip(&many) {
            assert!((a.x - b.x).abs() < 1e-3);
        }
    }

    #[test]
    fn pressure_changes_the_dab_size_along_the_stroke() {
        let dabs = run(&[sample(Phase::Down, 0.0, 0.0, Some(0.1)), sample(Phase::Move, 80.0, 0.0, Some(1.0))]);
        assert!(dabs.first().unwrap().radius < dabs.last().unwrap().radius);
    }

    #[test]
    fn a_missing_pressure_draws_at_full_size() {
        let dabs = run(&[sample(Phase::Down, 5.0, 5.0, None)]);
        assert_eq!(dabs[0].radius, radius_for(1.0));
    }

    #[test]
    fn lifting_the_pen_starts_a_new_stroke_instead_of_joining_the_two() {
        let dabs = run(&[
            sample(Phase::Down, 0.0, 0.0, Some(1.0)),
            sample(Phase::Up, 0.0, 0.0, Some(0.0)),
            sample(Phase::Down, 500.0, 0.0, Some(1.0)),
            sample(Phase::Up, 500.0, 0.0, Some(0.0)),
        ]);
        assert_eq!(dabs.len(), 2, "no dabs between the two taps");
    }
}
