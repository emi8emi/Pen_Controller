//! The brush engine: pen samples in, dabs out. Circle brush only for now (the Tauri overlay's brush
//! engine has airbrush and rectangle brushes to port later).
//!
//! `BrushConfig::default()` reproduces the original fixed brush (radius 1 + 7 * pressure, pink), so the
//! controller keeps working unchanged. The sketch studio passes its own config.
//!
//! No GPU and no window in here, so it is unit-tested on any machine.

use crate::renderer::{Dab, DEFAULT_COLOR, DEFAULT_HARDNESS};
use pen_proto::{Phase, Sample};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushConfig {
    /// Diameter in pixels at full pressure.
    pub size: f32,
    /// With `size_from_pressure`: size at zero pressure, as a fraction of `size`.
    pub min_scale: f32,
    pub size_from_pressure: bool,
    /// Per-dab opacity, 0..1 (overlapping dabs build up).
    pub opacity: f32,
    pub opacity_from_pressure: bool,
    /// 0 = soft edge, 1 = hard edge.
    pub hardness: f32,
    /// Distance between dabs as a fraction of the dab RADIUS.
    pub spacing: f32,
    /// Straight RGB, 0..1.
    pub color: [f32; 3],
}

impl Default for BrushConfig {
    fn default() -> Self {
        // radius = 8 * (0.125 + 0.875 p) = 1 + 7 p, the original brush
        BrushConfig {
            size: 16.0,
            min_scale: 0.125,
            size_from_pressure: true,
            opacity: 1.0,
            opacity_from_pressure: false,
            hardness: DEFAULT_HARDNESS,
            spacing: 0.15,
            color: DEFAULT_COLOR,
        }
    }
}

impl BrushConfig {
    pub fn radius(&self, pressure: f32) -> f32 {
        let p = pressure.clamp(0.0, 1.0);
        let scale = if self.size_from_pressure { self.min_scale + (1.0 - self.min_scale) * p } else { 1.0 };
        (self.size * 0.5 * scale).max(0.5)
    }
}

fn dab(cfg: &BrushConfig, x: f32, y: f32, p: f32) -> Dab {
    let alpha = cfg.opacity.clamp(0.0, 1.0) * if cfg.opacity_from_pressure { p.clamp(0.0, 1.0) } else { 1.0 };
    Dab { x, y, radius: cfg.radius(p), alpha, color: cfg.color, hardness: cfg.hardness }
}

#[derive(Default)]
pub struct Stroker {
    cfg: BrushConfig,
    last: Option<(f32, f32, f32)>,
    acc: f32,
}

impl Stroker {
    pub fn new(cfg: BrushConfig) -> Self {
        Stroker { cfg, last: None, acc: 0.0 }
    }

    pub fn config(&self) -> &BrushConfig {
        &self.cfg
    }

    /// Feed one sample; the dabs it produces are appended to `out`.
    pub fn feed(&mut self, s: Sample, out: &mut Vec<Dab>) {
        let pressure = s.pressure.unwrap_or(1.0); // a pen without pressure draws at full size
        match s.phase {
            Phase::Hover | Phase::Up => self.last = None,
            Phase::Down | Phase::Move => match self.last {
                None => {
                    out.push(dab(&self.cfg, s.x, s.y, pressure));
                    self.acc = 0.0;
                    self.last = Some((s.x, s.y, pressure));
                }
                Some((lx, ly, lp)) => {
                    let (dx, dy) = (s.x - lx, s.y - ly);
                    let l = dx.hypot(dy);
                    if l < 0.5 {
                        return;
                    }
                    let sp = (self.cfg.radius(pressure) * self.cfg.spacing).max(0.7);
                    let mut need = sp - self.acc;
                    while need <= l {
                        let t = need / l;
                        out.push(dab(&self.cfg, lx + dx * t, ly + dy * t, lp + (pressure - lp) * t));
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

    fn radius_for(p: f32) -> f32 {
        BrushConfig::default().radius(p)
    }

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

    fn run_with(cfg: BrushConfig, samples: &[Sample]) -> Vec<Dab> {
        let mut s = Stroker::new(cfg);
        let mut out = Vec::new();
        for x in samples {
            s.feed(*x, &mut out);
        }
        out
    }

    fn run(samples: &[Sample]) -> Vec<Dab> {
        run_with(BrushConfig::default(), samples)
    }

    #[test]
    fn the_default_brush_is_the_original_one() {
        for p in [0.0, 0.25, 0.5, 1.0] {
            assert!((radius_for(p) - (1.0 + 7.0 * p)).abs() < 1e-5);
        }
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

    #[test]
    fn the_config_sets_colour_opacity_and_hardness_on_every_dab() {
        let cfg = BrushConfig { color: [0.1, 0.2, 0.3], opacity: 0.4, hardness: 0.9, ..BrushConfig::default() };
        let dabs = run_with(cfg, &[sample(Phase::Down, 0.0, 0.0, Some(1.0)), sample(Phase::Move, 30.0, 0.0, Some(1.0))]);
        assert!(dabs.len() > 1);
        for d in &dabs {
            assert_eq!(d.color, [0.1, 0.2, 0.3]);
            assert_eq!(d.alpha, 0.4);
            assert_eq!(d.hardness, 0.9);
        }
    }

    #[test]
    fn a_brush_without_size_from_pressure_ignores_pressure() {
        let cfg = BrushConfig { size: 10.0, size_from_pressure: false, ..BrushConfig::default() };
        let dabs = run_with(cfg, &[sample(Phase::Down, 0.0, 0.0, Some(0.1)), sample(Phase::Move, 40.0, 0.0, Some(1.0))]);
        assert!(dabs.iter().all(|d| d.radius == 5.0));
    }

    #[test]
    fn opacity_from_pressure_scales_the_alpha() {
        let cfg = BrushConfig { opacity: 0.8, opacity_from_pressure: true, ..BrushConfig::default() };
        let dabs = run_with(cfg, &[sample(Phase::Down, 0.0, 0.0, Some(0.5))]);
        assert!((dabs[0].alpha - 0.4).abs() < 1e-6);
    }
}
