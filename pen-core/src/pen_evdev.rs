//! Linux pen pressure and tip state, read from the tablet's evdev node.
//! Stopgap until a real tablet-v2 window exists. Position still comes from the toolkit.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::Arc;

use evdev::{AbsoluteAxisCode as Abs, Device, EventType, KeyCode};

#[derive(Default)]
pub struct PenState {
    pressure_bits: AtomicU32,
    contact: AtomicBool,
    in_range: AtomicBool,
    barrel: AtomicBool,
}

impl PenState {
    pub fn pressure(&self) -> f32 {
        f32::from_bits(self.pressure_bits.load(Relaxed))
    }
    pub fn contact(&self) -> bool {
        self.contact.load(Relaxed)
    }
    pub fn in_range(&self) -> bool {
        self.in_range.load(Relaxed)
    }
    pub fn barrel(&self) -> bool {
        self.barrel.load(Relaxed)
    }
}

fn find() -> Option<(std::path::PathBuf, Device)> {
    // PEN_EVDEV=/dev/input/event8 overrides the search
    if let Ok(p) = std::env::var("PEN_EVDEV") {
        return match Device::open(&p) {
            Ok(d) => Some((p.into(), d)),
            Err(e) => {
                eprintln!("pen evdev: cannot open {p}: {e}");
                None
            }
        };
    }
    evdev::enumerate().find(|(_, d)| {
        d.supported_absolute_axes()
            .map_or(false, |a| a.contains(Abs::ABS_PRESSURE))
    })
}

/// Finds the pen node and starts a reader thread. `on_change` is called (from that thread) when
/// pressure or tip/range state changed, once per report. Returns None if no device could be opened.
pub fn spawn(on_change: impl Fn() + Send + 'static) -> Option<Arc<PenState>> {
    let Some((path, mut dev)) = find() else {
        eprintln!("pen evdev: no readable pen device. Permissions? (see ls -l /dev/input/event*) Set PEN_EVDEV to pick one.");
        return None;
    };
    let max = dev
        .get_absinfo()
        .ok()?
        .find(|(c, _)| *c == Abs::ABS_PRESSURE)
        .map(|(_, i)| i.maximum() as f32)
        .filter(|m| *m > 0.0)?;
    eprintln!("pen evdev: {} (max pressure {max})", path.display());
    eprintln!("pen evdev: reading /dev/input directly. If you added yourself to the `input` group for this,");
    eprintln!("           undo it when done: sudo gpasswd -d $USER input   (then log out and in)");

    let st = Arc::new(PenState::default());
    let s2 = st.clone();
    std::thread::spawn(move || {
        let mut dirty = false;
        loop {
            let events = match dev.fetch_events() {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("pen evdev stopped: {e}");
                    return;
                }
            };
            for ev in events {
                let code = ev.code();
                match ev.event_type() {
                    EventType::ABSOLUTE if code == Abs::ABS_PRESSURE.0 => {
                        let p = (ev.value() as f32 / max).clamp(0.0, 1.0);
                        s2.pressure_bits.store(p.to_bits(), Relaxed);
                        dirty = true;
                    }
                    EventType::KEY => {
                        let down = ev.value() != 0;
                        if code == KeyCode::BTN_TOUCH.0 {
                            s2.contact.store(down, Relaxed);
                            dirty = true;
                        } else if code == KeyCode::BTN_TOOL_PEN.0 {
                            s2.in_range.store(down, Relaxed);
                            dirty = true;
                        } else if code == KeyCode::BTN_STYLUS.0 {
                            s2.barrel.store(down, Relaxed);
                            dirty = true;
                        }
                    }
                    EventType::SYNCHRONIZATION if dirty => {
                        dirty = false;
                        on_change();
                    }
                    _ => {}
                }
            }
        }
    });
    Some(st)
}
