//! pen-ink: the ink half of the pen controller, as a crate of its own.
//!
//!   brush     pen samples -> dabs. No GPU, no window; unit-tested anywhere.
//!   renderer  `InkRenderer` trait + the wgpu implementation (dabs -> persistent ink texture -> surface).
//!
//! Used by the controller (Canvas mode) and by the sketch studio.

pub mod brush;
pub mod renderer;

pub use brush::Stroker;
pub use renderer::{Border, Dab, InkRenderer, RendererOptions, WgpuRenderer};
