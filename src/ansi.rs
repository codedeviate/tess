//! ANSI SGR (Select Graphic Rendition) + OSC 8 hyperlink parser.
//!
//! The parser is byte-driven (not char-driven) because ANSI sequences are
//! ASCII-only and we want to operate on raw input from arbitrary sources.
//! Public API: `Style`, `Color`, `ParseState`, `step`, `strip_sgr`.

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
    pub strike: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// 16 named colors: 0..=7 standard, 8..=15 bright.
    Ansi(u8),
    /// xterm-256.
    Indexed(u8),
    /// 24-bit truecolor.
    Rgb(u8, u8, u8),
    /// Explicit reset to terminal default (`\x1b[39m` for fg, `\x1b[49m` for bg).
    Default,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ParseState {
    #[default]
    Normal,
    EscSeen,
    CsiBuilding(Vec<u8>),
    OscBuilding(Vec<u8>),
}

/// Single-step transition driven by one input byte.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseStep {
    /// Byte is regular content; caller should emit it as a printable cell.
    Printable(u8),
    /// SGR sequence completed; `style` has been updated in place.
    StyleChanged,
    /// Non-SGR CSI completed; bytes consumed, no visible effect.
    OtherCsiSkipped,
    /// OSC 8 hyperlink open/close; `hyperlink` has been updated in place.
    HyperlinkChanged,
    /// Mid-sequence — caller should consume the byte and yield nothing yet.
    Consuming,
}

/// Apply one input byte to the state machine. `style` and `hyperlink` are
/// updated in place when relevant sequences complete.
pub fn step(
    state: &mut ParseState,
    style: &mut Style,
    hyperlink: &mut Option<String>,
    byte: u8,
) -> ParseStep {
    match state {
        ParseState::Normal => {
            if byte == 0x1b {
                *state = ParseState::EscSeen;
                ParseStep::Consuming
            } else {
                ParseStep::Printable(byte)
            }
        }
        ParseState::EscSeen => match byte {
            b'[' => {
                *state = ParseState::CsiBuilding(Vec::with_capacity(16));
                ParseStep::Consuming
            }
            b']' => {
                *state = ParseState::OscBuilding(Vec::with_capacity(32));
                ParseStep::Consuming
            }
            _ => {
                *state = ParseState::Normal;
                ParseStep::OtherCsiSkipped
            }
        },
        ParseState::CsiBuilding(buf) => {
            if (0x40..=0x7e).contains(&byte) {
                let params = std::mem::take(buf);
                let final_byte = byte;
                *state = ParseState::Normal;
                if final_byte == b'm' {
                    apply_sgr(&params, style);
                    ParseStep::StyleChanged
                } else {
                    ParseStep::OtherCsiSkipped
                }
            } else {
                buf.push(byte);
                ParseStep::Consuming
            }
        }
        ParseState::OscBuilding(buf) => {
            if byte == 0x07 {
                let body = std::mem::take(buf);
                *state = ParseState::Normal;
                apply_osc(&body, hyperlink);
                ParseStep::HyperlinkChanged
            } else if byte == b'\\' && buf.last() == Some(&0x1b) {
                buf.pop();
                let body = std::mem::take(buf);
                *state = ParseState::Normal;
                apply_osc(&body, hyperlink);
                ParseStep::HyperlinkChanged
            } else {
                buf.push(byte);
                ParseStep::Consuming
            }
        }
    }
}

fn apply_sgr(params: &[u8], style: &mut Style) {
    if params.is_empty() {
        *style = Style::default();
        return;
    }

    let text = match std::str::from_utf8(params) {
        Ok(s) => s,
        Err(_) => return,
    };

    let parts: Vec<&str> = text.split(';').collect();
    let mut i = 0;
    while i < parts.len() {
        let n: u16 = match parts[i].parse() {
            Ok(n) => n,
            Err(_) => {
                if parts[i].is_empty() {
                    *style = Style::default();
                    i += 1;
                    continue;
                }
                i += 1;
                continue;
            }
        };
        match n {
            0 => *style = Style::default(),
            1 => style.bold = true,
            2 => style.dim = true,
            3 => style.italic = true,
            4 => style.underline = true,
            7 => style.reverse = true,
            9 => style.strike = true,
            22 => {
                style.bold = false;
                style.dim = false;
            }
            23 => style.italic = false,
            24 => style.underline = false,
            27 => style.reverse = false,
            29 => style.strike = false,
            30..=37 => style.fg = Some(Color::Ansi((n - 30) as u8)),
            90..=97 => style.fg = Some(Color::Ansi((n - 90 + 8) as u8)),
            40..=47 => style.bg = Some(Color::Ansi((n - 40) as u8)),
            100..=107 => style.bg = Some(Color::Ansi((n - 100 + 8) as u8)),
            39 => style.fg = Some(Color::Default),
            49 => style.bg = Some(Color::Default),
            38 | 48 => {
                let dest = n;
                let mode: u16 = match parts.get(i + 1).and_then(|s| s.parse().ok()) {
                    Some(m) => m,
                    None => {
                        i += 1;
                        continue;
                    }
                };
                match mode {
                    5 => {
                        let idx: u16 = match parts.get(i + 2).and_then(|s| s.parse().ok()) {
                            Some(x) => x,
                            None => {
                                i += 2;
                                continue;
                            }
                        };
                        let color = Color::Indexed(idx as u8);
                        if dest == 38 {
                            style.fg = Some(color);
                        } else {
                            style.bg = Some(color);
                        }
                        i += 3;
                        continue;
                    }
                    2 => {
                        let r: u16 = parts.get(i + 2).and_then(|s| s.parse().ok()).unwrap_or(0);
                        let g: u16 = parts.get(i + 3).and_then(|s| s.parse().ok()).unwrap_or(0);
                        let b: u16 = parts.get(i + 4).and_then(|s| s.parse().ok()).unwrap_or(0);
                        let color = Color::Rgb(r as u8, g as u8, b as u8);
                        if dest == 38 {
                            style.fg = Some(color);
                        } else {
                            style.bg = Some(color);
                        }
                        i += 5;
                        continue;
                    }
                    _ => {
                        i += 2;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn apply_osc(body: &[u8], hyperlink: &mut Option<String>) {
    let text = match std::str::from_utf8(body) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut parts = text.splitn(3, ';');
    let cmd = parts.next().unwrap_or("");
    if cmd != "8" {
        return;
    }
    let _params = parts.next().unwrap_or("");
    let uri = parts.next().unwrap_or("");
    if uri.is_empty() {
        *hyperlink = None;
    } else {
        *hyperlink = Some(uri.to_string());
    }
}

/// Strip SGR / CSI / OSC sequences from a byte slice. Returns a `Cow<[u8]>`
/// that borrows the input when no sequences are present (common case) and
/// owns a new buffer otherwise.
pub fn strip_sgr(bytes: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    if !bytes.contains(&0x1b) {
        return std::borrow::Cow::Borrowed(bytes);
    }
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut state = ParseState::Normal;
    let mut style = Style::default();
    let mut hyperlink: Option<String> = None;
    for &b in bytes {
        if let ParseStep::Printable(byte) = step(&mut state, &mut style, &mut hyperlink, b) {
            out.push(byte);
        }
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(bytes: &[u8]) -> (Vec<u8>, Style, Option<String>) {
        let mut state = ParseState::Normal;
        let mut style = Style::default();
        let mut link = None;
        let mut printable = Vec::new();
        for &b in bytes {
            if let ParseStep::Printable(byte) = step(&mut state, &mut style, &mut link, b) {
                printable.push(byte);
            }
        }
        (printable, style, link)
    }

    #[test]
    fn plain_bytes_pass_through() {
        let (out, style, _) = run(b"hello");
        assert_eq!(out, b"hello");
        assert_eq!(style, Style::default());
    }

    #[test]
    fn sgr_red_then_text() {
        let (out, style, _) = run(b"\x1b[31mhi");
        assert_eq!(out, b"hi");
        assert_eq!(style.fg, Some(Color::Ansi(1)));
    }

    #[test]
    fn sgr_reset_clears_style() {
        let (out, style, _) = run(b"\x1b[1;31mbold\x1b[0mreset");
        assert_eq!(out, b"boldreset");
        assert_eq!(style, Style::default());
    }

    #[test]
    fn sgr_named_colors_0_to_15() {
        let (_, style, _) = run(b"\x1b[37m");
        assert_eq!(style.fg, Some(Color::Ansi(7)));
        let (_, style2, _) = run(b"\x1b[90m");
        assert_eq!(style2.fg, Some(Color::Ansi(8)));
        let (_, style3, _) = run(b"\x1b[97m");
        assert_eq!(style3.fg, Some(Color::Ansi(15)));
    }

    #[test]
    fn sgr_256_indexed_fg() {
        let (_, style, _) = run(b"\x1b[38;5;208m");
        assert_eq!(style.fg, Some(Color::Indexed(208)));
    }

    #[test]
    fn sgr_truecolor_fg() {
        let (_, style, _) = run(b"\x1b[38;2;255;128;0m");
        assert_eq!(style.fg, Some(Color::Rgb(255, 128, 0)));
    }

    #[test]
    fn sgr_256_indexed_bg() {
        let (_, style, _) = run(b"\x1b[48;5;15m");
        assert_eq!(style.bg, Some(Color::Indexed(15)));
    }

    #[test]
    fn sgr_truecolor_bg() {
        let (_, style, _) = run(b"\x1b[48;2;10;20;30m");
        assert_eq!(style.bg, Some(Color::Rgb(10, 20, 30)));
    }

    #[test]
    fn sgr_attributes_all() {
        let (_, style, _) = run(b"\x1b[1;2;3;4;7;9m");
        assert!(style.bold);
        assert!(style.dim);
        assert!(style.italic);
        assert!(style.underline);
        assert!(style.reverse);
        assert!(style.strike);
    }

    #[test]
    fn sgr_attribute_cancels() {
        let (_, style, _) = run(b"\x1b[1;2;3;4;7;9m\x1b[22;23;24;27;29m");
        assert!(!style.bold);
        assert!(!style.dim);
        assert!(!style.italic);
        assert!(!style.underline);
        assert!(!style.reverse);
        assert!(!style.strike);
    }

    #[test]
    fn sgr_default_fg_bg_reset_colors_only() {
        let (_, style, _) = run(b"\x1b[1;31;42m\x1b[39;49m");
        assert!(style.bold);
        assert_eq!(style.fg, Some(Color::Default));
        assert_eq!(style.bg, Some(Color::Default));
    }

    #[test]
    fn sgr_empty_treated_as_reset() {
        let (_, style, _) = run(b"\x1b[31m\x1b[m");
        assert_eq!(style, Style::default());
    }

    #[test]
    fn unknown_sgr_code_ignored() {
        let (_, style, _) = run(b"\x1b[31;999;1m");
        assert_eq!(style.fg, Some(Color::Ansi(1)));
        assert!(style.bold);
    }

    #[test]
    fn non_sgr_csi_skipped() {
        let (out, style, _) = run(b"\x1b[2Jcleared");
        assert_eq!(out, b"cleared");
        assert_eq!(style, Style::default());
    }

    #[test]
    fn incomplete_csi_at_eof_recovers() {
        let (out, _, _) = run(b"\x1b[31");
        assert_eq!(out, b"");
        let mut state = ParseState::Normal;
        let mut style = Style::default();
        let mut link = None;
        for &b in b"\x1b[31" {
            let _ = step(&mut state, &mut style, &mut link, b);
        }
        for &b in b"m" {
            let _ = step(&mut state, &mut style, &mut link, b);
        }
        assert_eq!(style.fg, Some(Color::Ansi(1)));
    }

    #[test]
    fn osc8_hyperlink_open_with_bel() {
        let (_, _, link) = run(b"\x1b]8;;https://example.com\x07");
        assert_eq!(link, Some("https://example.com".to_string()));
    }

    #[test]
    fn osc8_hyperlink_open_with_st() {
        let (_, _, link) = run(b"\x1b]8;;https://example.com\x1b\\");
        assert_eq!(link, Some("https://example.com".to_string()));
    }

    #[test]
    fn osc8_hyperlink_close() {
        let mut state = ParseState::Normal;
        let mut style = Style::default();
        let mut link = Some("https://example.com".to_string());
        for &b in b"\x1b]8;;\x07" {
            let _ = step(&mut state, &mut style, &mut link, b);
        }
        assert_eq!(link, None);
    }

    #[test]
    fn strip_sgr_borrows_when_no_escapes() {
        let s = strip_sgr(b"plain");
        assert!(matches!(s, std::borrow::Cow::Borrowed(_)));
        assert_eq!(s.as_ref(), b"plain");
    }

    #[test]
    fn strip_sgr_owns_and_removes_sgr() {
        let s = strip_sgr(b"\x1b[31merror\x1b[0m");
        assert_eq!(s.as_ref(), b"error");
    }

    #[test]
    fn strip_sgr_preserves_utf8() {
        let s = strip_sgr("\x1b[31m日本\x1b[0m".as_bytes());
        assert_eq!(s.as_ref(), "日本".as_bytes());
    }

    #[test]
    fn strip_sgr_handles_real_git_diff_line() {
        let input = b"\x1b[1mdiff --git a/foo b/foo\x1b[m\n\x1b[31m-old line\x1b[m\n\x1b[32m+new line\x1b[m\n";
        let stripped = strip_sgr(input);
        assert_eq!(
            stripped.as_ref(),
            b"diff --git a/foo b/foo\n-old line\n+new line\n"
        );
    }
}
