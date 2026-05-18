//! xxd-style hex dump rendering. One row = 16 bytes, with offset prefix
//! and ASCII gutter.

/// Format one row of a hex dump.
///
/// Layout: `<8-hex-digit offset>: <16 bytes as 8 x 2-byte words> <16-char ASCII gutter>`.
/// When `bytes.len() < 16`, the hex portion is right-padded with spaces so
/// the ASCII gutter remains column-aligned with full rows.
///
/// Offsets larger than 0xFFFFFFFF still render with at least 8 hex digits
/// (the format width is a minimum, not a max).
///
/// Non-printable bytes (outside 0x20..=0x7E) render as `.` in the ASCII gutter.
pub fn format_hex_row(offset: usize, bytes: &[u8]) -> String {
    debug_assert!(bytes.len() <= 16, "hex row must be <= 16 bytes");
    let mut out = String::with_capacity(80);
    out.push_str(&format!("{:08x}: ", offset));
    for i in 0..16 {
        if i > 0 && i % 2 == 0 {
            out.push(' ');
        }
        if i < bytes.len() {
            out.push_str(&format!("{:02x}", bytes[i]));
        } else {
            out.push_str("  ");
        }
    }
    out.push_str("  ");
    for b in bytes {
        if (0x20..=0x7E).contains(b) {
            out.push(*b as char);
        } else {
            out.push('.');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_input_16_bytes_renders_full_row() {
        let bytes = b"Hello world. tes";
        let row = format_hex_row(0, bytes);
        assert_eq!(
            row,
            "00000000: 4865 6c6c 6f20 776f 726c 642e 2074 6573  Hello world. tes"
        );
    }

    #[test]
    fn short_tail_pads_ascii_gutter_columns() {
        let bytes = b"t.";
        let row = format_hex_row(0x10, bytes);
        assert!(row.starts_with("00000010: 742e "));
        assert!(row.ends_with("  t."));
        let ascii_start = row.find("  t.").unwrap();
        let full_row = format_hex_row(0, b"0123456789abcdef");
        let full_ascii_start = full_row.rfind("  ").unwrap();
        assert_eq!(ascii_start, full_ascii_start,
                   "short-row ASCII column should align with full-row ASCII column");
    }

    #[test]
    fn all_printable_bytes_show_in_gutter() {
        let bytes = b"abcdefghijklmnop";
        let row = format_hex_row(0, bytes);
        assert!(row.ends_with("  abcdefghijklmnop"));
    }

    #[test]
    fn all_non_printable_bytes_show_as_dots() {
        let bytes = &[0x00, 0x01, 0x02, 0x1f, 0x7f, 0x80, 0xff];
        let row = format_hex_row(0, bytes);
        assert!(row.ends_with("  ......."));
    }

    #[test]
    fn utf8_multibyte_renders_as_dots_in_gutter() {
        let bytes = "ä".as_bytes();
        let row = format_hex_row(0, bytes);
        assert!(row.contains("c3a4"));
        assert!(row.ends_with("  .."));
    }

    #[test]
    fn offset_grows_past_0x10000() {
        let bytes = b"X";
        let row = format_hex_row(0x123456, bytes);
        assert!(row.starts_with("00123456: "));
    }

    #[test]
    fn offset_grows_past_8_digits() {
        let bytes = b"X";
        let row = format_hex_row(0x1_2345_6789, bytes);
        assert!(row.starts_with("123456789: "));
    }
}
