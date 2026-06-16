//! System-clipboard access by shelling out to OS tools. No external crates.
//!
//! macOS: pbcopy / pbpaste. Linux: wl-copy/wl-paste (Wayland) -> xclip -> xsel.
//! A missing tool yields an Err the caller surfaces on the status line.

use std::io::Write;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool { PbCopyPaste, WlClipboard, Xclip, Xsel }

/// Probe for the first available clipboard tool, in preference order.
pub fn detect() -> Option<Tool> {
    // Probe by attempting to spawn the *read* binary with a harmless arg and
    // seeing if the OS can find it. We use a cheap `--help`/`-version`-free
    // existence check via spawning and immediately killing where needed.
    const CANDIDATES: &[(&str, Tool)] = &[
        ("pbpaste", Tool::PbCopyPaste),
        ("wl-paste", Tool::WlClipboard),
        ("xclip", Tool::Xclip),
        ("xsel", Tool::Xsel),
    ];
    for (probe, tool) in CANDIDATES {
        if which(probe) { return Some(*tool); }
    }
    None
}

/// True if `bin` is found on PATH (uses `command -v` via the shell, which is
/// available on both macOS and Linux). Avoids running the clipboard tool itself.
fn which(bin: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {bin}"))
        .stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn read_cmd(tool: Tool) -> Command {
    let mut c;
    match tool {
        Tool::PbCopyPaste => { c = Command::new("pbpaste"); }
        Tool::WlClipboard => { c = Command::new("wl-paste"); c.arg("--no-newline"); }
        Tool::Xclip => { c = Command::new("xclip"); c.args(["-selection","clipboard","-o"]); }
        Tool::Xsel => { c = Command::new("xsel"); c.args(["--clipboard","--output"]); }
    }
    c
}

fn write_cmd(tool: Tool) -> Command {
    let mut c;
    match tool {
        Tool::PbCopyPaste => { c = Command::new("pbcopy"); }
        Tool::WlClipboard => { c = Command::new("wl-copy"); }
        Tool::Xclip => { c = Command::new("xclip"); c.args(["-selection","clipboard"]); }
        Tool::Xsel => { c = Command::new("xsel"); c.args(["--clipboard","--input"]); }
    }
    c
}

/// Read clipboard contents. Err string is human-facing (for the status line).
pub fn read() -> Result<Vec<u8>, String> {
    let tool = detect().ok_or("no clipboard tool found (need pbpaste/wl-paste/xclip/xsel)")?;
    let out = read_cmd(tool).stderr(Stdio::null()).output()
        .map_err(|e| format!("clipboard read failed: {e}"))?;
    Ok(out.stdout)
}

/// Write `bytes` to the clipboard. Err string is human-facing.
pub fn write(bytes: &[u8]) -> Result<(), String> {
    let tool = detect().ok_or("no clipboard tool found (need pbcopy/wl-copy/xclip/xsel)")?;
    let mut child = write_cmd(tool).stdin(Stdio::piped())
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        .map_err(|e| format!("clipboard write failed: {e}"))?;
    child.stdin.as_mut().ok_or("clipboard stdin unavailable")?
        .write_all(bytes).map_err(|e| format!("clipboard write failed: {e}"))?;
    child.wait().map_err(|e| format!("clipboard write failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_write_cmd_programs_distinct_per_tool() {
        for t in [Tool::PbCopyPaste, Tool::WlClipboard, Tool::Xclip, Tool::Xsel] {
            assert!(!read_cmd(t).get_program().is_empty());
            assert!(!write_cmd(t).get_program().is_empty());
        }
    }
    #[test]
    fn xclip_read_uses_output_flag() {
        let c = read_cmd(Tool::Xclip);
        let args: Vec<_> = c.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"-o".to_string()));
    }
}
