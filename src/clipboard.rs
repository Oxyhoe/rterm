//! Clipboard access through the platform's own helper tools.
//!
//! rterm stays free of X11/Wayland bindings: `wl-copy`, `xclip`, `xsel` or
//! `pbcopy` are spawned only when the clipboard is actually touched, and the
//! first backend that works wins.

use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Clipboard,
    /// The X11 PRIMARY selection (middle-click paste).
    Selection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    Wayland,
    Xclip,
    Xsel,
    Mac,
}

/// Backends to try, best match for the current session first.
fn tools() -> Vec<Tool> {
    let mut out = Vec::new();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    if wayland {
        out.push(Tool::Wayland);
    }
    if x11 {
        out.push(Tool::Xclip);
        out.push(Tool::Xsel);
    }
    if !wayland && !x11 {
        // Unknown session type: try everything, failures are cheap.
        out.extend([Tool::Wayland, Tool::Xclip, Tool::Xsel]);
    }
    if cfg!(target_os = "macos") {
        out.push(Tool::Mac);
    }
    out
}

impl Tool {
    fn set_command(self, target: Target) -> Option<Command> {
        let mut cmd = match self {
            Tool::Wayland => {
                let mut c = Command::new("wl-copy");
                if target == Target::Selection {
                    c.arg("--primary");
                }
                c
            }
            Tool::Xclip => {
                let mut c = Command::new("xclip");
                c.args(["-in", "-selection", selection_arg(target)]);
                c
            }
            Tool::Xsel => {
                let mut c = Command::new("xsel");
                if target == Target::Selection {
                    c.args(["--primary", "--input"]);
                } else {
                    c.args(["--clipboard", "--input"]);
                }
                c
            }
            Tool::Mac if target == Target::Clipboard => Command::new("pbcopy"),
            Tool::Mac => return None,
        };
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Some(cmd)
    }

    fn get_command(self, target: Target) -> Option<Command> {
        let mut cmd = match self {
            Tool::Wayland => {
                let mut c = Command::new("wl-paste");
                if target == Target::Selection {
                    c.arg("--primary");
                }
                c
            }
            Tool::Xclip => {
                let mut c = Command::new("xclip");
                c.args(["-out", "-selection", selection_arg(target)]);
                c
            }
            Tool::Xsel => {
                let mut c = Command::new("xsel");
                if target == Target::Selection {
                    c.args(["--primary", "--output"]);
                } else {
                    c.args(["--clipboard", "--output"]);
                }
                c
            }
            Tool::Mac if target == Target::Clipboard => Command::new("pbpaste"),
            Tool::Mac => return None,
        };
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        Some(cmd)
    }
}

fn selection_arg(target: Target) -> &'static str {
    match target {
        Target::Clipboard => "clipboard",
        Target::Selection => "primary",
    }
}

/// Store text in the clipboard. Returns true when some backend accepted it.
pub fn store(target: Target, text: &str) -> bool {
    for tool in tools() {
        let Some(mut cmd) = tool.set_command(target) else {
            continue;
        };
        let Ok(mut child) = cmd.spawn() else { continue };
        let fed = child
            .stdin
            .take()
            .and_then(|mut stdin| stdin.write_all(text.as_bytes()).ok())
            .is_some();
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if fed && ok {
            return true;
        }
    }
    false
}

/// Read the clipboard. Returns `None` when no backend has data (or none exist).
pub fn load(target: Target) -> Option<String> {
    for tool in tools() {
        let Some(mut cmd) = tool.get_command(target) else {
            continue;
        };
        let Ok(out) = cmd.output() else { continue };
        if out.status.success() {
            return Some(String::from_utf8_lossy(&out.stdout).into_owned());
        }
    }
    None
}
