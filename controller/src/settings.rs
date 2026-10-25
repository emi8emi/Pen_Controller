//! The one setting that is saved between runs: `clear_on_dismiss`. Auto-show and hands-off are
//! deliberately never saved (they start on / off), and the overlay position is not configurable yet.
//!
//! File: `$XDG_CONFIG_HOME/pen-controller/config` (else `~/.config/...`) on Linux, `%APPDATA%\pen-controller\config`
//! on Windows. Plain `key = value` lines, `#` starts a comment. Unknown keys are ignored, so a newer
//! version's file does not break an older one.

use std::path::PathBuf;

use pen_proto::CanvasOptions;

fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join("pen-controller").join("config"))
}

fn parse(text: &str) -> CanvasOptions {
    let mut o = CanvasOptions::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if k.trim() == "clear_on_dismiss" {
            o.clear_on_dismiss = matches!(v.trim(), "true" | "on" | "1" | "yes");
        }
    }
    o
}

/// Saved options, or the defaults if there is no file (or it cannot be read).
/// `PEN_CLEAR_ON_DISMISS` set in the environment turns clear-on-dismiss on for this run, whatever the file says.
pub fn load() -> CanvasOptions {
    let mut o = path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse(&t))
        .unwrap_or_default();
    if std::env::var("PEN_CLEAR_ON_DISMISS").is_ok() {
        o.clear_on_dismiss = true;
    }
    o
}

pub fn save(o: CanvasOptions) {
    let Some(p) = path() else { return };
    let text = format!(
        "# Pen controller. Written when you change a setting; safe to edit by hand.\nclear_on_dismiss = {}\n",
        o.clear_on_dismiss
    );
    let result = p
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&p, text));
    if let Err(e) = result {
        eprintln!("could not save {}: {e}", p.display());
    }
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn reads_the_flag() {
        assert!(parse("clear_on_dismiss = true\n").clear_on_dismiss);
        assert!(parse("# c\n  clear_on_dismiss=on # x\n").clear_on_dismiss);
        assert!(!parse("clear_on_dismiss = false").clear_on_dismiss);
        assert!(!parse("nonsense\nother = 1").clear_on_dismiss);
    }
}
