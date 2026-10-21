//! A set of labelled examples, matching, and a plain-text file format.

use std::fmt;

use crate::{Cloud, Point, Stroke};

/// How far apart (see [`Cloud::distance`]) two drawings can be and still count as a match at all.
const MAX_DISTANCE: f32 = 0.12;

/// A ranked answer. `score` is 1 for an identical drawing and falls to 0 at the matching limit; it is a
/// rank within this library, not a probability.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub label: String,
    pub score: f32,
    pub distance: f32,
}

#[derive(Debug)]
struct Entry {
    label: String,
    strokes: Vec<Stroke>,
    cloud: Cloud,
}

#[derive(Debug, Default)]
pub struct Library {
    entries: Vec<Entry>,
}

#[derive(Debug, PartialEq)]
pub struct ParseError {
    pub line: usize,
    pub what: &'static str,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.what)
    }
}

impl Library {
    pub fn new() -> Self {
        Library::default()
    }

    /// Number of saved examples (a label can have several).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Labels, in the order they were first added.
    pub fn labels(&self) -> Vec<&str> {
        let mut seen: Vec<&str> = Vec::new();
        for e in &self.entries {
            if !seen.contains(&e.label.as_str()) {
                seen.push(&e.label);
            }
        }
        seen
    }

    /// How many examples a label has.
    pub fn examples(&self, label: &str) -> usize {
        self.entries.iter().filter(|e| e.label == label).count()
    }

    /// Save an example. Returns false (and saves nothing) for an empty label or a drawing with no points.
    pub fn add(&mut self, label: &str, strokes: Vec<Stroke>) -> bool {
        if label.is_empty() {
            return false;
        }
        let Some(cloud) = Cloud::new(&strokes) else {
            return false;
        };
        self.entries.push(Entry {
            label: label.to_string(),
            strokes,
            cloud,
        });
        true
    }

    /// Like [`add`](Self::add), but keeps at most `max` examples per label: past that the oldest goes.
    pub fn add_capped(&mut self, label: &str, strokes: Vec<Stroke>, max: usize) -> bool {
        if !self.add(label, strokes) {
            return false;
        }
        while self.examples(label) > max.max(1) {
            if let Some(i) = self.entries.iter().position(|e| e.label == label) {
                self.entries.remove(i);
            }
        }
        true
    }

    /// Forget every example of a label. Returns how many went.
    pub fn remove(&mut self, label: &str) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| e.label != label);
        before - self.entries.len()
    }

    /// The labels whose best example is within the matching limit, closest first. Empty when the library is
    /// empty, the drawing has no points, or nothing is close.
    pub fn recognize(&self, strokes: &[Stroke]) -> Vec<Match> {
        let Some(cloud) = Cloud::new(strokes) else {
            return Vec::new();
        };
        let mut best: Vec<Match> = Vec::new();
        for e in &self.entries {
            let d = cloud.distance(&e.cloud);
            if d > MAX_DISTANCE {
                continue;
            }
            match best.iter_mut().find(|m| m.label == e.label) {
                Some(m) if d < m.distance => {
                    m.distance = d;
                    m.score = score(d);
                }
                Some(_) => {}
                None => best.push(Match {
                    label: e.label.clone(),
                    score: score(d),
                    distance: d,
                }),
            }
        }
        best.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        best
    }

    // ---- file format ----
    //
    //   pen-recog 1
    //   T<TAB>label            (\\ \n \t escaped)
    //   S<TAB>x,y x,y x,y      one line per stroke, belongs to the T above it
    //
    // Text on purpose: easy to read, diff and fix by hand.

    pub fn to_text(&self) -> String {
        let mut out = String::from("pen-recog 1\n");
        for e in &self.entries {
            out.push_str("T\t");
            out.push_str(&escape(&e.label));
            out.push('\n');
            for s in &e.strokes {
                out.push_str("S\t");
                let pts: Vec<String> = s
                    .points
                    .iter()
                    .map(|p| format!("{},{}", p.x, p.y))
                    .collect();
                out.push_str(&pts.join(" "));
                out.push('\n');
            }
        }
        out
    }

    pub fn from_text(text: &str) -> Result<Library, ParseError> {
        let mut lines = text.lines().enumerate();
        match lines.next() {
            Some((_, "pen-recog 1")) => {}
            _ => {
                return Err(ParseError {
                    line: 1,
                    what: "not a pen-recog 1 file",
                })
            }
        }
        let mut lib = Library::new();
        let mut cur: Option<(String, Vec<Stroke>)> = None;
        let flush = |lib: &mut Library, cur: &mut Option<(String, Vec<Stroke>)>| {
            if let Some((label, strokes)) = cur.take() {
                lib.add(&label, strokes);
            }
        };
        for (i, line) in lines {
            let n = i + 1;
            if line.is_empty() {
                continue;
            }
            if let Some(label) = line.strip_prefix("T\t") {
                flush(&mut lib, &mut cur);
                cur = Some((unescape(label), Vec::new()));
            } else if let Some(pts) = line.strip_prefix("S\t") {
                let Some((_, strokes)) = cur.as_mut() else {
                    return Err(ParseError {
                        line: n,
                        what: "a stroke before any label",
                    });
                };
                let mut points = Vec::new();
                for pair in pts.split_whitespace() {
                    let (x, y) = pair.split_once(',').ok_or(ParseError {
                        line: n,
                        what: "a point is not x,y",
                    })?;
                    let x: f32 = x.parse().map_err(|_| ParseError {
                        line: n,
                        what: "bad number",
                    })?;
                    let y: f32 = y.parse().map_err(|_| ParseError {
                        line: n,
                        what: "bad number",
                    })?;
                    if !x.is_finite() || !y.is_finite() {
                        return Err(ParseError {
                            line: n,
                            what: "bad number",
                        });
                    }
                    points.push(Point { x, y });
                }
                strokes.push(Stroke { points });
            } else {
                return Err(ParseError {
                    line: n,
                    what: "unknown line",
                });
            }
        }
        flush(&mut lib, &mut cur);
        Ok(lib)
    }
}

fn score(d: f32) -> f32 {
    (1.0 - d / MAX_DISTANCE).clamp(0.0, 1.0)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(pts: &[(f32, f32)]) -> Stroke {
        Stroke {
            points: pts.iter().map(|&(x, y)| Point { x, y }).collect(),
        }
    }

    /// A circle-ish loop, a zigzag and a check mark: three things a person might use as symbols.
    fn circle(cx: f32, cy: f32, r: f32, wobble: f32) -> Stroke {
        let pts: Vec<(f32, f32)> = (0..=24)
            .map(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                let rr = r + wobble * (i as f32 * 1.7).sin();
                (cx + rr * a.cos(), cy + rr * a.sin())
            })
            .collect();
        st(&pts)
    }
    fn check(x: f32, y: f32, s: f32, jitter: f32) -> Stroke {
        st(&[
            (x, y + 0.5 * s + jitter),
            (x + 0.3 * s, y + s),
            (x + s, y + jitter),
        ])
    }
    fn zigzag(x: f32, y: f32, s: f32) -> Stroke {
        st(&[
            (x, y),
            (x + s, y + 0.2 * s),
            (x, y + 0.4 * s),
            (x + s, y + 0.6 * s),
            (x, y + 0.8 * s),
            (x + s, y + s),
        ])
    }

    fn lib() -> Library {
        let mut l = Library::new();
        assert!(l.add("yes", vec![check(0.0, 0.0, 40.0, 0.0)]));
        assert!(l.add("circle", vec![circle(0.0, 0.0, 30.0, 0.0)]));
        assert!(l.add("zig", vec![zigzag(0.0, 0.0, 40.0)]));
        l
    }

    #[test]
    fn finds_the_right_label_at_any_size_and_place_with_a_wobbly_hand() {
        let l = lib();
        let m = l.recognize(&[circle(500.0, 300.0, 90.0, 4.0)]);
        assert_eq!(m[0].label, "circle");
        let m = l.recognize(&[check(200.0, 100.0, 120.0, 5.0)]);
        assert_eq!(m[0].label, "yes");
        let m = l.recognize(&[zigzag(50.0, 50.0, 20.0)]);
        assert_eq!(m[0].label, "zig");
        assert!(m[0].score > 0.9);
    }

    #[test]
    fn something_unlike_everything_matches_nothing() {
        let l = lib();
        // a long horizontal line is none of the three
        assert!(l.recognize(&[st(&[(0.0, 0.0), (200.0, 0.0)])]).is_empty());
        assert!(Library::new()
            .recognize(&[check(0.0, 0.0, 10.0, 0.0)])
            .is_empty());
        assert!(l.recognize(&[]).is_empty());
    }

    #[test]
    fn a_label_appears_once_with_its_best_example() {
        let mut l = lib();
        l.add("yes", vec![check(0.0, 0.0, 40.0, 3.0)]);
        let m = l.recognize(&[check(0.0, 0.0, 40.0, 3.0)]);
        assert_eq!(m.iter().filter(|m| m.label == "yes").count(), 1);
        assert_eq!(m[0].label, "yes");
        assert!(m[0].distance < 0.01);
    }

    #[test]
    fn a_multi_stroke_symbol_matches_however_it_was_drawn() {
        let mut l = Library::new();
        l.add(
            "plus",
            vec![
                st(&[(0.0, 5.0), (10.0, 5.0)]),
                st(&[(5.0, 0.0), (5.0, 10.0)]),
            ],
        );
        l.add("minus", vec![st(&[(0.0, 5.0), (10.0, 5.0)])]);
        let m = l.recognize(&[
            st(&[(5.0, 10.0), (5.0, 0.0)]),
            st(&[(10.0, 5.0), (0.0, 5.0)]),
        ]);
        assert_eq!(m[0].label, "plus");
    }

    #[test]
    fn rejects_empty_labels_and_empty_drawings() {
        let mut l = Library::new();
        assert!(!l.add("", vec![check(0.0, 0.0, 1.0, 0.0)]));
        assert!(!l.add("x", vec![]));
        assert!(!l.add("x", vec![Stroke { points: vec![] }]));
        assert!(l.is_empty());
    }

    #[test]
    fn capped_adding_drops_the_oldest_example_of_that_label_only() {
        let mut l = lib();
        for k in 0..5 {
            l.add_capped("yes", vec![check(0.0, 0.0, 40.0, k as f32)], 3);
        }
        assert_eq!(l.examples("yes"), 3);
        assert_eq!(l.examples("circle"), 1);
        let mut labels = l.labels();
        labels.sort();
        assert_eq!(labels, vec!["circle", "yes", "zig"]);
    }

    #[test]
    fn remove_forgets_a_label() {
        let mut l = lib();
        assert_eq!(l.remove("circle"), 1);
        assert_eq!(l.remove("circle"), 0);
        assert_eq!(l.labels(), vec!["yes", "zig"]);
    }

    #[test]
    fn text_round_trip_keeps_labels_and_still_matches() {
        let mut l = lib();
        l.add(
            "I need\thelp\nplease \\ now",
            vec![
                st(&[(0.0, 0.0), (3.5, 4.25)]),
                st(&[(1.0, 1.0), (2.0, 9.0)]),
            ],
        );
        let text = l.to_text();
        let back = Library::from_text(&text).unwrap();
        assert_eq!(back.len(), l.len());
        assert_eq!(back.labels(), l.labels());
        assert_eq!(back.to_text(), text);
        assert_eq!(back.recognize(&[zigzag(0.0, 0.0, 40.0)])[0].label, "zig");
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        assert!(Library::from_text("").is_err());
        assert!(Library::from_text("something else\n").is_err());
        assert_eq!(
            Library::from_text("pen-recog 1\nS\t1,2\n")
                .unwrap_err()
                .what,
            "a stroke before any label"
        );
        assert!(Library::from_text("pen-recog 1\nT\ta\nS\t1;2\n").is_err());
        assert!(Library::from_text("pen-recog 1\nT\ta\nS\tx,2\n").is_err());
        assert!(Library::from_text("pen-recog 1\nT\ta\nS\tNaN,2\n").is_err());
        assert!(Library::from_text("pen-recog 1\nwhat\n").is_err());
        // a label with no strokes is simply not saved
        assert!(Library::from_text("pen-recog 1\nT\ta\n")
            .unwrap()
            .is_empty());
    }
}
