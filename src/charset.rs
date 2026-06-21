//! Charset decoding for non-UTF-8 input, wrapping `encoding_rs` behind a small
//! stable surface. `decode_line` is the single decode used by both rendering
//! (non-UTF-8 path) and all matching.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding(&'static encoding_rs::Encoding);

impl Encoding {
    pub fn utf8() -> Encoding { Encoding(encoding_rs::UTF_8) }
    pub fn is_utf8(&self) -> bool { self.0 == encoding_rs::UTF_8 }
    pub fn label(&self) -> &'static str { self.0.name() }
}

impl Default for Encoding {
    fn default() -> Self { Encoding::utf8() }
}

/// Parse a WHATWG label (`utf-8`, `iso-8859-1`, `latin1`, `windows-1252`,
/// `shift_jis`, …). Case-insensitive; `None` for an unknown label.
pub fn parse_label(label: &str) -> Option<Encoding> {
    encoding_rs::Encoding::for_label(label.as_bytes()).map(Encoding)
}

/// Decode one line's bytes to text using `enc`. UTF-8 is a lossy decode
/// (invalid → U+FFFD); single-byte charsets never fail.
pub fn decode_line(bytes: &[u8], enc: Encoding) -> std::borrow::Cow<'_, str> {
    let (cow, _, _) = enc.0.decode(bytes);
    cow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_known_and_alias_labels() {
        assert!(parse_label("utf-8").unwrap().is_utf8());
        assert!(parse_label("latin1").is_some());
        assert!(parse_label("iso-8859-1").is_some());
        assert!(parse_label("windows-1252").is_some());
        assert!(parse_label("shift_jis").is_some());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse_label("not-a-charset-xyz").is_none());
    }

    #[test]
    fn decode_latin1_high_byte_is_e_acute() {
        let enc = parse_label("iso-8859-1").unwrap();
        assert_eq!(decode_line(&[0x63, 0x61, 0x66, 0xE9], enc), "café");
    }

    #[test]
    fn decode_utf8_passthrough_and_ascii_identical() {
        let enc = Encoding::utf8();
        assert_eq!(decode_line("café".as_bytes(), enc), "café");
        let l1 = parse_label("iso-8859-1").unwrap();
        assert_eq!(decode_line(b"plain ascii", l1), "plain ascii");
    }

    #[test]
    fn decode_utf8_invalid_is_lossy_replacement() {
        let enc = Encoding::utf8();
        let s = decode_line(&[b'a', 0xC3, b'b'], enc);
        assert!(s.contains('\u{FFFD}'));
        assert!(s.starts_with('a') && s.ends_with('b'));
    }
}
