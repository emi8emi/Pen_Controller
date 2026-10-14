//! pen-recog: recognise pen drawings by comparing them with examples you saved.
//!
//! It is a point-cloud matcher (the $P family): every drawing, however many strokes it has, becomes a
//! fixed number of points, scaled to a unit box and centred; two drawings are close when their clouds are.
//! Stroke order, stroke count and direction do not matter, which suits handwriting shorthand and symbols
//! drawn by a person (you rarely draw the same star with the same strokes twice). It does **not** read
//! free handwriting: that stays the job of a real handwriting recognizer. Use this for the things such a
//! recognizer cannot know: your own symbols, abbreviations and phrases.
//!
//! ```
//! use pen_recog::{Library, Point, Stroke};
//!
//! let line = |pts: &[(f32, f32)]| Stroke { points: pts.iter().map(|&(x, y)| Point { x, y }).collect() };
//! let mut lib = Library::new();
//! lib.add("plus", vec![line(&[(0.0, 5.0), (10.0, 5.0)]), line(&[(5.0, 0.0), (5.0, 10.0)])]);
//! lib.add("minus", vec![line(&[(0.0, 5.0), (10.0, 5.0)])]);
//! let found = lib.recognize(&[line(&[(1.0, 6.0), (21.0, 6.0)]), line(&[(11.0, 0.0), (11.0, 20.0)])]);
//! assert_eq!(found[0].label, "plus");
//! ```
//!
//! Samples from the pen controller become strokes with [`strokes_from_samples`].

mod cloud;
mod library;
mod samples;

pub use cloud::{Cloud, CLOUD_POINTS};
pub use library::{Library, Match, ParseError};
pub use samples::strokes_from_samples;

/// One point, in whatever units the caller draws in (the matcher rescales).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// Pen down to pen up.
#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub points: Vec<Point>,
}
