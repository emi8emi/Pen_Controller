//! The controller's Windows glue around pen-core's reusable pieces: it finds the HWND of the winit
//! window, gives it the overlay styles, and turns pen samples into entries of the main loop's queue.
//!
//! The actual work lives in `pen_core::pointer` (WM_POINTER samples) and `pen_core::overlay` (window styles).

use crate::{send, Incoming, UserEvent, QUEUE};
use pen_core::{overlay, pointer};
use std::ffi::c_void;
use std::time::Instant;

fn hwnd_of(window: &winit::window::Window) -> Option<*mut c_void> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get() as *mut c_void),
        _ => None,
    }
}

/// Subclass the window for WM_POINTER, and give it the overlay styles: showing it must never steal focus
/// from the program you are drawing over (same as the pen-summoned Tauri overlay).
pub fn install(window: &winit::window::Window) {
    let Some(hwnd) = hwnd_of(window) else { return };
    overlay::apply_overlay_styles(hwnd);
    let result = pointer::install(hwnd, |samples| {
        let recv = Instant::now();
        QUEUE.lock().unwrap().extend(samples.iter().map(|s| Incoming { sample: *s, recv }));
        send(UserEvent::Samples);
    });
    if let Err(e) = result {
        eprintln!("pen input is not available: {e}");
    }
}

/// Hide for real (see `pen_core::overlay::hide`).
pub fn hide(window: &winit::window::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        overlay::hide(hwnd);
    }
}

/// Show without activating (see `pen_core::overlay::show_noactivate`).
pub fn show_noactivate(window: &winit::window::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        overlay::show_noactivate(hwnd);
    }
}
