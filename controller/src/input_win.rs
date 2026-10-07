//! Windows pen input and window styling, for the controller's overlay window.
//!
//! * WM_POINTER with pointer history, by subclassing winit's window. winit's own touch events lose tilt,
//!   hover and the coalesced history, so we read them ourselves.
//! * A non-activating tool window that stays out of Alt+Tab, with show/hide that really work.

use crate::{send, Incoming, UserEvent, QUEUE};
use pen_proto::{Phase, Sample};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::time::Instant;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::Pointer::{GetPointerPenInfoHistory, POINTER_PEN_INFO};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_HIDE, SW_SHOWNOACTIVATE, WM_POINTERDOWN, WM_POINTERUP,
    WM_POINTERUPDATE, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

// Plain u32 values: pointerFlags is a POINTER_FLAGS newtype (use .0), penMask is a bare u32.
const POINTER_FLAG_INCONTACT: u32 = 0x0000_0004;
const PEN_MASK_PRESSURE: u32 = 0x0000_0001;
const PEN_MASK_TILT_X: u32 = 0x0000_0004;
const PEN_MASK_TILT_Y: u32 = 0x0000_0008;
// penFlags is assumed to be a bare u32 like penMask; if the compiler says it is a newtype, use `.0`.
const PEN_FLAG_BARREL: u32 = 0x0000_0001;
const PEN_FLAG_ERASER: u32 = 0x0000_0004;

static IN_CONTACT: AtomicBool = AtomicBool::new(false);

fn hwnd_of(window: &winit::window::Window) -> Option<HWND> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(w) => Some(HWND(w.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Non-activating tool window that stays out of Alt+Tab and the taskbar. WS_EX_APPWINDOW has to be
/// cleared as well: while it is set, the window is listed even with WS_EX_TOOLWINDOW. Changes to these
/// styles are only picked up reliably while the window is hidden, so this runs right before each show.
unsafe fn apply_styles(hwnd: HWND) {
    let ex = (GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32 | WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0)
        & !WS_EX_APPWINDOW.0;
    let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex as isize);
}

/// Subclass the window for WM_POINTER, and give it the styles above: showing it must never steal focus
/// from the program you are drawing over (same as the pen-summoned Tauri overlay).
pub fn install(window: &winit::window::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(subclass_proc), 1, 0);
            apply_styles(hwnd);
        }
    }
}

/// Hide for real. winit's set_visible(false) is a no-op here: winit never learned that we showed the
/// window with a raw ShowWindow, so as far as it knows the window is already hidden.
pub fn hide(window: &winit::window::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// Show without activating (winit's set_visible(true) would activate the window).
pub fn show_noactivate(window: &winit::window::Window) {
    if let Some(hwnd) = hwnd_of(window) {
        unsafe {
            apply_styles(hwnd); // winit may have re-applied its own styles since the last show
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) && handle(hwnd, wparam) {
        // Pen message consumed: not passing it on also stops Windows from synthesising
        // legacy mouse messages from it, which is what we want here.
        return LRESULT(0);
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Returns true if this was a pen pointer and we turned it into samples.
unsafe fn handle(hwnd: HWND, wparam: WPARAM) -> bool {
    let recv = Instant::now();
    let id = (wparam.0 & 0xFFFF) as u32; // GET_POINTERID_WPARAM

    let mut count = 0u32;
    if GetPointerPenInfoHistory(id, &mut count, None).is_err() || count == 0 {
        return false; // not a pen (mouse, touch): let winit handle it
    }
    let mut buf: Vec<POINTER_PEN_INFO> = vec![std::mem::zeroed(); count as usize];
    if GetPointerPenInfoHistory(id, &mut count, Some(buf.as_mut_ptr())).is_err() {
        return false;
    }
    buf.truncate(count as usize);

    let mut out = Vec::with_capacity(buf.len());
    // history is newest-first; walk it oldest-first
    for info in buf.iter().rev() {
        let pi = &info.pointerInfo;
        let contact = pi.pointerFlags.0 & POINTER_FLAG_INCONTACT != 0;
        // Capabilities the pen does not report stay None (never a fake zero).
        let pressure = if info.penMask & PEN_MASK_PRESSURE != 0 {
            Some((info.pressure as f32 / 1024.0).clamp(0.0, 1.0))
        } else {
            None
        };
        let tilt = if info.penMask & PEN_MASK_TILT_X != 0 && info.penMask & PEN_MASK_TILT_Y != 0 {
            Some((info.tiltX as f32, info.tiltY as f32))
        } else {
            None
        };
        let eraser = info.penFlags & PEN_FLAG_ERASER != 0;
        let buttons = (info.penFlags & PEN_FLAG_BARREL != 0) as u8;
        // NOTE: integer pixels, so strokes can look slightly stepped. Next step: use
        // ptHimetricLocationRaw + GetPointerDeviceRects for sub-pixel positions.
        let mut p = POINT { x: pi.ptPixelLocation.x, y: pi.ptPixelLocation.y };
        let _ = ScreenToClient(hwnd, &mut p);

        let prev = IN_CONTACT.swap(contact, Relaxed);
        let phase = match (prev, contact) {
            (false, true) => Phase::Down,
            (true, true) => Phase::Move,
            (true, false) => Phase::Up,
            (false, false) => Phase::Hover,
        };
        out.push(Incoming {
            sample: Sample {
                pen_id: 0,
                phase,
                x: p.x as f32,
                y: p.y as f32,
                tablet: None,
                pressure,
                tilt,
                buttons,
                eraser,
                // the input stack's receive time in microseconds: not the device's own clock
                t_us: pi.PerformanceCount,
                t_hardware: false,
            },
            recv,
        });
    }

    QUEUE.lock().unwrap().extend(out);
    send(UserEvent::Samples);
    true
}
