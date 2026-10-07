//! pen-core: what every pen overlay needs, with no UI toolkit attached.

pub mod lifecycle;
#[cfg(windows)]
pub mod pen_win;

pub use lifecycle::{Action, Lifecycle};
