//! Terminal graphics capability + cell-pixel-size detection. The response
//! PARSERS here are pure and unit-tested; the I/O round-trip in `detect` is
//! verified manually (no PTY tests, per project policy).

use std::io::{Read, Write};
use std::time::Duration;

/// What the terminal supports, plus its cell pixel size when reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TermGraphics {
    pub kitty: bool,
    pub sixel: bool,
    pub cell_px: Option<(u16, u16)>, // (width, height) in pixels
}

/// Parse a batch of terminal query responses (raw bytes) into capabilities.
/// Looks for: Kitty `ESC _ G ... ; OK ESC \`, a DA1 `ESC [ ? <attrs> c` whose
/// attribute list contains `4` (Sixel), and a cell-size report
/// `ESC [ 6 ; <h> ; <w> t`.
pub fn parse_responses(buf: &[u8]) -> TermGraphics {
    let s = String::from_utf8_lossy(buf);
    let mut g = TermGraphics::default();

    if s.contains("\x1b_G") && s.contains(";OK") {
        g.kitty = true;
    }

    if let Some(start) = s.find("\x1b[?") {
        if let Some(end_rel) = s[start..].find('c') {
            let attrs = &s[start + 3..start + end_rel];
            if attrs.split(';').any(|a| a == "4") {
                g.sixel = true;
            }
        }
    }

    if let Some(start) = s.find("\x1b[6;") {
        if let Some(end_rel) = s[start..].find('t') {
            let body = &s[start + 4..start + end_rel];
            let mut it = body.split(';');
            if let (Some(h), Some(w)) = (it.next(), it.next()) {
                if let (Ok(h), Ok(w)) = (h.parse::<u16>(), w.parse::<u16>()) {
                    g.cell_px = Some((w, h));
                }
            }
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kitty_ok() {
        let g = parse_responses(b"\x1b_Gi=31;OK\x1b\\");
        assert!(g.kitty);
        assert!(!g.sixel);
    }

    #[test]
    fn parses_da1_with_sixel() {
        let g = parse_responses(b"\x1b[?62;4;9c");
        assert!(g.sixel);
    }

    #[test]
    fn da1_without_sixel_is_not_sixel() {
        let g = parse_responses(b"\x1b[?62;9c");
        assert!(!g.sixel);
    }

    #[test]
    fn parses_cell_size() {
        let g = parse_responses(b"\x1b[6;16;8t");
        assert_eq!(g.cell_px, Some((8, 16)));
    }

    #[test]
    fn garbage_yields_nothing() {
        let g = parse_responses(b"random noise no escapes");
        assert_eq!(g, TermGraphics::default());
    }

    #[test]
    fn combined_response_parses_all() {
        let g = parse_responses(b"\x1b_Gi=1;OK\x1b\\\x1b[6;16;8t\x1b[?62;4c");
        assert!(g.kitty && g.sixel);
        assert_eq!(g.cell_px, Some((8, 16)));
    }

    #[test]
    fn truncated_da1_without_c_is_safe() {
        // `\x1b[?` with no terminating `c`: must not panic, no sixel detected.
        let g = parse_responses(b"\x1b[?62;4");
        assert!(!g.sixel);
    }

    #[test]
    fn non_numeric_cell_size_is_ignored() {
        // Malformed cell-size body: parse fails gracefully, cell_px stays None.
        let g = parse_responses(b"\x1b[6;xx;yyt");
        assert_eq!(g.cell_px, None);
    }
}

// wired up in the --image-protocol auto path (later task)
/// Probe the terminal for graphics support. Writes the query sequences to
/// `/dev/tty`, reads the response with a short deadline, and parses it. Falls
/// back to environment heuristics when the round-trip yields nothing. Pure
/// ASCII (no support) is represented by an all-false `TermGraphics`.
#[allow(dead_code)]
pub fn detect() -> TermGraphics {
    if let Some(g) = query_tty(Duration::from_millis(120)) {
        if g.kitty || g.sixel {
            return merge_env(g);
        }
    }
    env_fallback()
}

// wired up in the --image-protocol auto path (later task)
#[allow(dead_code)]
fn query_tty(timeout: Duration) -> Option<TermGraphics> {
    use std::fs::OpenOptions;
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
    // Kitty graphics query (1x1, action=query), cell-size (CSI 16 t), then DA1
    // (CSI c). DA1 always answers, so it bounds the read.
    let q = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[c";
    tty.write_all(q).ok()?;
    tty.flush().ok()?;

    let (tx, rx) = std::sync::mpsc::channel();
    // Detached reader: we read on a thread and abandon it on timeout. A
    // conformant terminal answers DA1 (`c`) within ms so the thread exits
    // promptly; a terminal that answers only partially leaves the thread
    // blocked on read until process exit. Acceptable for a one-shot startup
    // probe.
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match tty.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => {
                    buf.push(byte[0]);
                    // DA1 terminates with 'c' after an ESC was seen; stop there.
                    if byte[0] == b'c' && buf.contains(&0x1b) { break; }
                    if buf.len() > 4096 { break; }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(buf);
    });
    let buf = rx.recv_timeout(timeout).ok()?;
    Some(parse_responses(&buf))
}

// wired up in the --image-protocol auto path (later task)
#[allow(dead_code)]
fn merge_env(mut g: TermGraphics) -> TermGraphics {
    let env = env_fallback();
    g.kitty |= env.kitty;
    g.sixel |= env.sixel;
    g
}

/// Best-effort capability guess from environment variables, used when the
/// active query times out (e.g. inside some terminal multiplexers).
pub fn env_fallback() -> TermGraphics {
    let term = std::env::var("TERM").unwrap_or_default().to_lowercase();
    let prog = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let kitty = std::env::var("KITTY_WINDOW_ID").is_ok()
        || term.contains("kitty")
        || term.contains("wezterm")
        || prog.contains("wezterm")
        || prog.contains("iterm")
        || prog.contains("ghostty");
    let sixel = term.contains("foot")
        || term.contains("mlterm")
        || term.contains("vt340")
        || term.contains("wezterm");
    TermGraphics { kitty, sixel, cell_px: None }
}
