//! Window styles for a pen overlay (Windows only): a non-activating tool window that stays out of Alt+Tab
//! and the taskbar, with show/hide that really work. Showing it must never steal focus from the program
//! you are drawing over.
//!
//! All functions take the window as a raw `HWND` (`*mut c_void`) and must be called from the thread that
//! owns the window.

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

/// Non-activating tool window that stays out of Alt+Tab and the taskbar. WS_EX_APPWINDOW has to be
/// cleared as well: while it is set, the window is listed even with WS_EX_TOOLWINDOW. Changes to these
/// styles are only picked up reliably while the window is hidden, so `show_noactivate` runs this right
/// before each show.
pub fn apply_overlay_styles(hwnd: *mut c_void) {
    unsafe {
        let hwnd = HWND(hwnd);
        let ex = (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0)
            & !WS_EX_APPWINDOW.0;
        let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
    }
}

/// Show without activating. (winit's `set_visible(true)` would activate the window.)
pub fn show_noactivate(hwnd: *mut c_void) {
    apply_overlay_styles(hwnd); // a toolkit may have re-applied its own styles since the last show
    unsafe {
        let _ = ShowWindow(HWND(hwnd), SW_SHOWNOACTIVATE);
    }
}

/// Hide for real. winit's `set_visible(false)` is a no-op after a raw `ShowWindow`: winit never learned
/// that the window was shown, so as far as it knows the window is already hidden.
pub fn hide(hwnd: *mut c_void) {
    unsafe {
        let _ = ShowWindow(HWND(hwnd), SW_HIDE);
    }
}
