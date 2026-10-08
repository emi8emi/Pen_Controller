//! Pen samples from `WM_POINTER`, with pointer history (Windows only).
//!
//! Toolkits such as winit lose tilt, hover and the coalesced history of pen input, so this reads them
//! straight from the window's messages by subclassing the window. Every pen sample is delivered to a
//! callback as a `pen_proto::Sample`.
//!
//! State is per window: two windows (even in one process) never share pen-contact state, and the state is
//! freed when its window is destroyed.
//!
//! Rules for the callback (the "sink"):
//!   * it runs on the window's own thread, inside the window procedure, so keep it quick;
//!   * it must not pump messages or do anything that re-enters this window's procedure. If it does, the
//!     re-entrant message is not turned into samples (Windows then synthesises mouse messages from it).
//!
//! Pen messages are consumed here: not passing them on also stops Windows from synthesising legacy mouse
//! messages from them. Mouse and touch are left to the toolkit.

use std::cell::RefCell;
use std::ffi::c_void;
use std::io;

use pen_proto::{Phase, Sample};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::Pointer::{GetPointerPenInfoHistory, POINTER_PEN_INFO};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{WM_NCDESTROY, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE};

// Plain u32 values: pointerFlags is a POINTER_FLAGS newtype (use .0), penMask and penFlags are bare u32.
const POINTER_FLAG_INCONTACT: u32 = 0x0000_0004;
const PEN_MASK_PRESSURE: u32 = 0x0000_0001;
const PEN_MASK_TILT_X: u32 = 0x0000_0004;
const PEN_MASK_TILT_Y: u32 = 0x0000_0008;
const PEN_FLAG_BARREL: u32 = 0x0000_0001;
const PEN_FLAG_ERASER: u32 = 0x0000_0004;

const SUBCLASS_ID: usize = 1;

struct State {
    sink: Box<dyn FnMut(&[Sample])>,
    in_contact: bool,
}

/// Start delivering this window's pen input to `sink`. Samples are in window client pixels.
///
/// Call it once per window, from the thread that owns the window. There is nothing to undo: the subclass
/// and its state are removed automatically when the window is destroyed.
pub fn install(hwnd: *mut c_void, sink: impl FnMut(&[Sample]) + 'static) -> io::Result<()> {
    if hwnd.is_null() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "null window handle"));
    }
    let state = Box::into_raw(Box::new(RefCell::new(State { sink: Box::new(sink), in_contact: false })));
    let ok = unsafe { SetWindowSubclass(HWND(hwnd), Some(subclass_proc), SUBCLASS_ID, state as usize) }.as_bool();
    if ok {
        Ok(())
    } else {
        // Windows never took the pointer, so nobody else will free it
        unsafe { drop(Box::from_raw(state)) };
        Err(io::Error::last_os_error())
    }
}

/// Contact transitions: down on the first sample in contact, up on the first one out of it.
fn phase_for(prev_contact: bool, contact: bool) -> Phase {
    match (prev_contact, contact) {
        (false, true) => Phase::Down,
        (true, true) => Phase::Move,
        (true, false) => Phase::Up,
        (false, false) => Phase::Hover,
    }
}

/// Windows reports pressure as 0..1024.
fn pressure_01(raw: u32) -> f32 {
    (raw as f32 / 1024.0).clamp(0.0, 1.0)
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_NCDESTROY {
        // last message this window gets: unhook, then free the state we leaked in `install`
        let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), id);
        drop(Box::from_raw(data as *mut RefCell<State>));
        return DefSubclassProc(hwnd, msg, wparam, lparam);
    }
    if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
        let cell = &*(data as *const RefCell<State>);
        if let Ok(mut state) = cell.try_borrow_mut() {
            if handle(hwnd, &mut state, wparam) {
                return LRESULT(0);
            }
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Returns true if this was a pen pointer and we turned it into samples.
unsafe fn handle(hwnd: HWND, state: &mut State, wparam: WPARAM) -> bool {
    let id = (wparam.0 & 0xFFFF) as u32; // GET_POINTERID_WPARAM

    let mut count = 0u32;
    if GetPointerPenInfoHistory(id, &mut count, None).is_err() || count == 0 {
        return false; // not a pen (mouse, touch): let the toolkit handle it
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
        let pressure = if info.penMask & PEN_MASK_PRESSURE != 0 { Some(pressure_01(info.pressure)) } else { None };
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

        let prev = std::mem::replace(&mut state.in_contact, contact);
        out.push(Sample {
            pen_id: 0,
            phase: phase_for(prev, contact),
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
        });
    }

    (state.sink)(&out);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_transitions_map_to_phases() {
        assert_eq!(phase_for(false, false), Phase::Hover);
        assert_eq!(phase_for(false, true), Phase::Down);
        assert_eq!(phase_for(true, true), Phase::Move);
        assert_eq!(phase_for(true, false), Phase::Up);
    }

    #[test]
    fn pressure_is_scaled_and_clamped() {
        assert_eq!(pressure_01(0), 0.0);
        assert_eq!(pressure_01(512), 0.5);
        assert_eq!(pressure_01(1024), 1.0);
        assert_eq!(pressure_01(5000), 1.0);
    }
}
