//! From the controller's samples to strokes.

use pen_proto::{Phase, Sample};

use crate::{Point, Stroke};

/// Split a run of samples into strokes: a stroke starts at `Down`, follows the `Move`s and ends at `Up`.
/// Hover samples are ignored. A stream that ends before the `Up` still yields its last stroke, and a `Move`
/// with no `Down` before it (a stream joined in the middle) starts one.
pub fn strokes_from_samples(samples: &[Sample]) -> Vec<Stroke> {
    let mut out = Vec::new();
    let mut cur: Vec<Point> = Vec::new();
    for s in samples {
        match s.phase {
            Phase::Hover => {}
            Phase::Down => {
                if !cur.is_empty() {
                    out.push(Stroke {
                        points: std::mem::take(&mut cur),
                    });
                }
                cur.push(Point { x: s.x, y: s.y });
            }
            Phase::Move => cur.push(Point { x: s.x, y: s.y }),
            Phase::Up => {
                cur.push(Point { x: s.x, y: s.y });
                out.push(Stroke {
                    points: std::mem::take(&mut cur),
                });
            }
        }
    }
    if !cur.is_empty() {
        out.push(Stroke { points: cur });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(phase: Phase, x: f32, y: f32) -> Sample {
        Sample {
            pen_id: 0,
            phase,
            x,
            y,
            tablet: None,
            pressure: None,
            tilt: None,
            buttons: 0,
            eraser: false,
            t_us: 0,
            t_hardware: false,
        }
    }

    #[test]
    fn splits_on_down_and_up_and_skips_hover() {
        let v = [
            s(Phase::Hover, 9.0, 9.0),
            s(Phase::Down, 0.0, 0.0),
            s(Phase::Move, 1.0, 0.0),
            s(Phase::Up, 2.0, 0.0),
            s(Phase::Hover, 9.0, 9.0),
            s(Phase::Down, 5.0, 5.0),
            s(Phase::Up, 5.0, 6.0),
        ];
        let st = strokes_from_samples(&v);
        assert_eq!(st.len(), 2);
        assert_eq!(st[0].points.len(), 3);
        assert_eq!(
            st[1].points,
            vec![Point { x: 5.0, y: 5.0 }, Point { x: 5.0, y: 6.0 }]
        );
    }

    #[test]
    fn a_stream_cut_short_or_joined_late_still_gives_strokes() {
        let cut = [s(Phase::Down, 0.0, 0.0), s(Phase::Move, 1.0, 1.0)];
        assert_eq!(strokes_from_samples(&cut).len(), 1);
        let late = [s(Phase::Move, 1.0, 1.0), s(Phase::Up, 2.0, 2.0)];
        assert_eq!(strokes_from_samples(&late).len(), 1);
        assert!(strokes_from_samples(&[]).is_empty());
        assert!(strokes_from_samples(&[s(Phase::Hover, 0.0, 0.0)]).is_empty());
    }

    #[test]
    fn down_without_up_before_the_next_down_closes_the_stroke() {
        let v = [
            s(Phase::Down, 0.0, 0.0),
            s(Phase::Move, 1.0, 0.0),
            s(Phase::Down, 5.0, 5.0),
            s(Phase::Up, 6.0, 5.0),
        ];
        assert_eq!(strokes_from_samples(&v).len(), 2);
    }
}
