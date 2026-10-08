//! pen-core: what every pen overlay needs, with no UI toolkit attached.
//!
//! Windows windows are passed in as raw `HWND` pointers (`*mut c_void`), so this crate does not depend on
//! winit or on a particular version of the `windows` crate in its public API.

pub mod lifecycle;
#[cfg(windows)]
pub mod overlay;
#[cfg(windows)]
pub mod pen_win;
#[cfg(windows)]
pub mod pointer;

pub use lifecycle::{Action, Lifecycle};
