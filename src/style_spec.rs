//! Parser for the `--status-style` / `--prompt-style` CLI flags and the
//! per-format `prompt_style` config key. Maps a comma-separated token list
//! onto an `ansi::Style`.
//!
//! Grammar:
//! ```text
//! spec   := token ("," token)*
//! token  := attr | "fg=" color | "bg=" color
//! attr   := bold | dim | italic | underline | reverse
//! color  := name | "#RRGGBB" | "0".."255"
//! name   := ("bright-")? (black|red|green|yellow|blue|magenta|cyan|white)
//! ```
//!
//! Empty input is treated as "no styling" and returns `Ok(Style::default())`.

use crate::ansi::{Color, Style};

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    UnknownAttr(String),
    UnknownColor(String),
    BadHex(String),
    BadToken(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::UnknownAttr(t) => write!(f, "unknown attribute `{t}`"),
            ParseError::UnknownColor(t) => write!(f, "unknown color `{t}`"),
            ParseError::BadHex(t) => write!(f, "bad hex color `{t}` (expected #RRGGBB)"),
            ParseError::BadToken(t) => write!(f, "bad token `{t}`"),
        }
    }
}

/// Parse a style spec into an `ansi::Style`. Empty / whitespace-only input
/// returns `Style::default()` so callers can treat empty CLI flags as "off".
pub fn parse(spec: &str) -> Result<Style, ParseError> {
    let mut style = Style::default();
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return Ok(style);
    }
    for raw in trimmed.split(',') {
        let tok = raw.trim();
        if tok.is_empty() {
            continue;
        }
        if let Some(c) = tok.strip_prefix("fg=") {
            style.fg = Some(parse_color(c)?);
        } else if let Some(c) = tok.strip_prefix("bg=") {
            style.bg = Some(parse_color(c)?);
        } else {
            match tok {
                "bold" => style.bold = true,
                "dim" => style.dim = true,
                "italic" => style.italic = true,
                "underline" => style.underline = true,
                "reverse" => style.reverse = true,
                other if other.contains('=') => {
                    return Err(ParseError::BadToken(other.to_string()))
                }
                other => return Err(ParseError::UnknownAttr(other.to_string())),
            }
        }
    }
    Ok(style)
}

fn parse_color(s: &str) -> Result<Color, ParseError> {
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return Err(ParseError::BadHex(s.to_string()));
        }
        let r = u8::from_str_radix(&hex[0..2], 16)
            .map_err(|_| ParseError::BadHex(s.to_string()))?;
        let g = u8::from_str_radix(&hex[2..4], 16)
            .map_err(|_| ParseError::BadHex(s.to_string()))?;
        let b = u8::from_str_radix(&hex[4..6], 16)
            .map_err(|_| ParseError::BadHex(s.to_string()))?;
        return Ok(Color::Rgb(r, g, b));
    }
    if let Ok(n) = s.parse::<u8>() {
        return Ok(if n < 16 { Color::Ansi(n) } else { Color::Indexed(n) });
    }
    let (bright, name) = match s.strip_prefix("bright-") {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let base: u8 = match name {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "white" => 7,
        _ => return Err(ParseError::UnknownColor(s.to_string())),
    };
    Ok(Color::Ansi(if bright { base + 8 } else { base }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_default_style() {
        assert_eq!(parse("").unwrap(), Style::default());
        assert_eq!(parse("   ").unwrap(), Style::default());
    }

    #[test]
    fn bold_only() {
        let s = parse("bold").unwrap();
        assert!(s.bold);
        assert!(s.fg.is_none());
    }

    #[test]
    fn fg_named() {
        let s = parse("fg=cyan").unwrap();
        assert_eq!(s.fg, Some(Color::Ansi(6)));
    }

    #[test]
    fn fg_bright_named() {
        let s = parse("fg=bright-red").unwrap();
        assert_eq!(s.fg, Some(Color::Ansi(9)));
    }

    #[test]
    fn bg_hex() {
        let s = parse("bg=#ff0080").unwrap();
        assert_eq!(s.bg, Some(Color::Rgb(0xff, 0x00, 0x80)));
    }

    #[test]
    fn fg_indexed_under_16_is_ansi() {
        let s = parse("fg=4").unwrap();
        assert_eq!(s.fg, Some(Color::Ansi(4)));
    }

    #[test]
    fn fg_indexed_over_16_is_palette() {
        let s = parse("fg=200").unwrap();
        assert_eq!(s.fg, Some(Color::Indexed(200)));
    }

    #[test]
    fn combined_attrs_and_colors() {
        let s = parse("bold,fg=cyan,bg=black").unwrap();
        assert!(s.bold);
        assert_eq!(s.fg, Some(Color::Ansi(6)));
        assert_eq!(s.bg, Some(Color::Ansi(0)));
    }

    #[test]
    fn reverse_attr() {
        let s = parse("reverse").unwrap();
        assert!(s.reverse);
    }

    #[test]
    fn bad_hex_errors() {
        assert!(matches!(parse("fg=#12"), Err(ParseError::BadHex(_))));
        assert!(matches!(parse("fg=#xxxxxx"), Err(ParseError::BadHex(_))));
    }

    #[test]
    fn unknown_attr_errors() {
        assert!(matches!(parse("blink"), Err(ParseError::UnknownAttr(_))));
    }

    #[test]
    fn unknown_color_name_errors() {
        assert!(matches!(parse("fg=puce"), Err(ParseError::UnknownColor(_))));
    }

    #[test]
    fn trailing_comma_is_tolerated() {
        let s = parse("bold,").unwrap();
        assert!(s.bold);
    }
}
