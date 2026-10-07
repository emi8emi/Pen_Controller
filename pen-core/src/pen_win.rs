//! Windows-only pen and mouse detection, adapted from the pen-logger prototype.
//!
//!   * Raw Input (digitizer usage page 0x0D, INPUTSINK) supplies pen reports, even while
//!     another app is focused. The report layout differs per tablet, so it comes from a
//!     [`DeviceProfile`]; [`WACOM_CTL_4100`] is the one known to work (report ID 0xD5, flags
//!     in byte 1: bit 0 = tip touching, bit 5 = pen in range).
//!   * A low-level mouse hook answers "did a real mouse move?". Pen and touch input carry
//!     the Windows Ink signature in dwExtraInfo and are ignored.
//!
//! Two events come out:
//!   Summon  - the pen just came into range or touched down
//!   Dismiss - a real mouse moved at least HANDOFF after the pen's last report

use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::core::{w, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, Copy)]
pub enum PenEvent {
    Summon,
    Dismiss,
}

// Windows Ink tags pen/touch-originated mouse messages in dwExtraInfo.
const SIG_MASK: usize = 0xFFFF_FF00;
const SIG_PEN_TOUCH: usize = 0xFF51_5700;

/// Where to find "tip touching" and "pen in range" in a tablet's raw HID report.
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfile {
    pub report_id: u8,
    /// Index of the byte (in the whole report, report ID included) that holds the flags.
    pub flags_byte: usize,
    pub tip_mask: u8,
    pub in_range_mask: u8,
}

/// Wacom Intuos S (CTL-4100), as measured on the author's tablet.
pub const WACOM_CTL_4100: DeviceProfile =
    DeviceProfile { report_id: 0xD5, flags_byte: 1, tip_mask: 0x01, in_range_mask: 0x20 };

static PROFILE: OnceLock<DeviceProfile> = OnceLock::new();

/// A mouse move this soon after the last pen report is treated as handoff jitter.
const HANDOFF: Duration = Duration::from_millis(300);

type Callback = Box<dyn Fn(PenEvent) + Send + Sync>;
static CALLBACK: OnceLock<Callback> = OnceLock::new();

struct State {
    in_range: bool,
    tip: bool,
    last_report: Option<Instant>,
}
static STATE: Mutex<State> = Mutex::new(State { in_range: false, tip: false, last_report: None });

fn emit(ev: PenEvent) {
    if let Some(cb) = CALLBACK.get() {
        cb(ev);
    }
}

/// Start the detection thread. `cb` is called from that thread, so keep it quick.
/// Call this once; a second call keeps the first profile and callback.
pub fn spawn<F>(profile: DeviceProfile, cb: F)
where
    F: Fn(PenEvent) + Send + Sync + 'static,
{
    let _ = PROFILE.set(profile);
    let _ = CALLBACK.set(Box::new(cb));
    std::thread::spawn(|| {
        if let Err(e) = unsafe { run() } {
            eprintln!("pen detection stopped: {e}");
        }
    });
}

/// File name of the program that currently has focus, e.g. "krita.exe".
pub fn foreground_exe() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid).ok()?;
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(|s| s.to_string())
    }
}

// ---------- mouse hook ----------

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && wparam.0 as u32 == WM_MOUSEMOVE {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if info.dwExtraInfo & SIG_MASK != SIG_PEN_TOUCH {
            let last = STATE.lock().unwrap().last_report;
            let quiet = match last {
                Some(t) => t.elapsed() >= HANDOFF,
                None => true,
            };
            if quiet {
                emit(PenEvent::Dismiss);
            }
        }
    }
    CallNextHookEx(HHOOK::default(), code, wparam, lparam)
}

// ---------- raw input ----------

unsafe fn on_raw_input(lparam: LPARAM) {
    let h = HRAWINPUT(lparam.0 as *mut c_void);
    let hdr_size = size_of::<RAWINPUTHEADER>() as u32;

    let mut size = 0u32;
    GetRawInputData(h, RID_INPUT, None, &mut size, hdr_size);
    if size == 0 {
        return;
    }
    // u64 buffer keeps the RAWINPUT cast aligned
    let mut buf = vec![0u64; (size as usize + 7) / 8];
    let got = GetRawInputData(h, RID_INPUT, Some(buf.as_mut_ptr() as *mut c_void), &mut size, hdr_size);
    if got == u32::MAX {
        return;
    }
    let raw = &*(buf.as_ptr() as *const RAWINPUT);
    if raw.header.dwType != RIM_TYPEHID.0 {
        return;
    }

    let hid = &raw.data.hid;
    let len = (hid.dwSizeHid * hid.dwCount) as usize;
    let bytes = std::slice::from_raw_parts(hid.bRawData.as_ptr(), len);
    let Some(profile) = PROFILE.get() else { return };
    if len <= profile.flags_byte || bytes[0] != profile.report_id {
        return;
    }

    let flags = bytes[profile.flags_byte];
    let in_range = flags & profile.in_range_mask != 0;
    let tip = flags & profile.tip_mask != 0;

    let rising = {
        let mut st = STATE.lock().unwrap();
        let rising = (in_range && !st.in_range) || (tip && !st.tip);
        st.in_range = in_range;
        st.tip = tip;
        st.last_report = Some(Instant::now());
        rising
    };
    if rising {
        emit(PenEvent::Summon);
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        on_raw_input(lparam);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn run() -> windows::core::Result<()> {
    let hinst: HINSTANCE = GetModuleHandleW(None)?.into();

    // hidden message-only window: Raw Input needs a target window
    let class = w!("PenOverlayPenWnd");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst,
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        w!("pen-overlay-input"),
        WINDOW_STYLE(0),
        0, 0, 0, 0,
        HWND_MESSAGE,
        None,
        hinst,
        None,
    )?;

    // 0x0D = Digitizers; 0x02 = Pen, 0x01 = Digitizer
    let devs = [
        RAWINPUTDEVICE { usUsagePage: 0x0D, usUsage: 0x02, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd },
        RAWINPUTDEVICE { usUsagePage: 0x0D, usUsage: 0x01, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd },
    ];
    RegisterRawInputDevices(&devs, size_of::<RAWINPUTDEVICE>() as u32)?;

    let _hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hinst, 0)?;

    // the hook and the raw input window both need this thread's message loop
    let mut msg = MSG::default();
    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
    Ok(())
}
