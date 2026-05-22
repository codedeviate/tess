//! xxd-style hex dump rendering. One row = 16 bytes, with offset prefix
//! and ASCII gutter.

/// Format one row of a hex dump.
///
/// Layout: `<8-hex-digit offset>: <hex bytes grouped by `bytes_per_group`> <16-char ASCII gutter>`.
/// `bytes_per_group` must be one of 1, 2, 4, 8, 16 — corresponding to 2, 4, 8,
/// 16, or 32 hex characters per group (16 = the whole row as a single group,
/// no spacing between hex pairs).
///
/// When `bytes.len() < 16`, the hex portion is right-padded with spaces so
/// the ASCII gutter remains column-aligned with full rows.
///
/// Offsets larger than 0xFFFFFFFF still render with at least 8 hex digits
/// (the format width is a minimum, not a max).
///
/// Non-printable bytes (outside 0x20..=0x7E) render as `.` in the ASCII gutter.
pub fn format_hex_row(offset: usize, bytes: &[u8], bytes_per_group: usize) -> String {
    debug_assert!(bytes.len() <= 16, "hex row must be <= 16 bytes");
    debug_assert!(
        matches!(bytes_per_group, 1 | 2 | 4 | 8 | 16),
        "bytes_per_group must be 1, 2, 4, 8, or 16"
    );
    let mut out = String::with_capacity(80);
    out.push_str(&format!("{:08x}: ", offset));
    for i in 0..16 {
        if i > 0 && i % bytes_per_group == 0 {
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

/// Translate a user-facing "hex chars per group" value (2/4/8/16/32) to the
/// internal bytes-per-group unit (1/2/4/8/16). Returns `None` for any other
/// input.
pub fn hex_chars_to_bytes_per_group(hex_chars: usize) -> Option<usize> {
    match hex_chars {
        2 => Some(1),
        4 => Some(2),
        8 => Some(4),
        16 => Some(8),
        32 => Some(16),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_input_16_bytes_renders_full_row() {
        let bytes = b"Hello world. tes";
        let row = format_hex_row(0, bytes, 2);
        assert_eq!(
            row,
            "00000000: 4865 6c6c 6f20 776f 726c 642e 2074 6573  Hello world. tes"
        );
    }

    #[test]
    fn short_tail_pads_ascii_gutter_columns() {
        let bytes = b"t.";
        let row = format_hex_row(0x10, bytes, 2);
        assert!(row.starts_with("00000010: 742e "));
        assert!(row.ends_with("  t."));
        let ascii_start = row.find("  t.").unwrap();
        let full_row = format_hex_row(0, b"0123456789abcdef", 2);
        let full_ascii_start = full_row.rfind("  ").unwrap();
        assert_eq!(ascii_start, full_ascii_start,
                   "short-row ASCII column should align with full-row ASCII column");
    }

    #[test]
    fn all_printable_bytes_show_in_gutter() {
        let bytes = b"abcdefghijklmnop";
        let row = format_hex_row(0, bytes, 2);
        assert!(row.ends_with("  abcdefghijklmnop"));
    }

    #[test]
    fn all_non_printable_bytes_show_as_dots() {
        let bytes = &[0x00, 0x01, 0x02, 0x1f, 0x7f, 0x80, 0xff];
        let row = format_hex_row(0, bytes, 2);
        assert!(row.ends_with("  ......."));
    }

    #[test]
    fn utf8_multibyte_renders_as_dots_in_gutter() {
        let bytes = "ä".as_bytes();
        let row = format_hex_row(0, bytes, 2);
        assert!(row.contains("c3a4"));
        assert!(row.ends_with("  .."));
    }

    #[test]
    fn offset_grows_past_0x10000() {
        let bytes = b"X";
        let row = format_hex_row(0x123456, bytes, 2);
        assert!(row.starts_with("00123456: "));
    }

    #[test]
    fn offset_grows_past_8_digits() {
        let bytes = b"X";
        let row = format_hex_row(0x1_2345_6789, bytes, 2);
        assert!(row.starts_with("123456789: "));
    }

    #[test]
    fn group_size_1_byte_renders_2_hex_per_group() {
        let bytes = b"Hello world. tes";
        let row = format_hex_row(0, bytes, 1);
        assert_eq!(
            row,
            "00000000: 48 65 6c 6c 6f 20 77 6f 72 6c 64 2e 20 74 65 73  Hello world. tes"
        );
    }

    #[test]
    fn group_size_4_bytes_renders_8_hex_per_group() {
        let bytes = b"Hello world. tes";
        let row = format_hex_row(0, bytes, 4);
        assert_eq!(
            row,
            "00000000: 48656c6c 6f20776f 726c642e 20746573  Hello world. tes"
        );
    }

    #[test]
    fn group_size_8_bytes_renders_16_hex_per_group() {
        let bytes = b"Hello world. tes";
        let row = format_hex_row(0, bytes, 8);
        assert_eq!(
            row,
            "00000000: 48656c6c6f20776f 726c642e20746573  Hello world. tes"
        );
    }

    #[test]
    fn group_size_16_bytes_renders_whole_row_unspaced() {
        let bytes = b"Hello world. tes";
        let row = format_hex_row(0, bytes, 16);
        assert_eq!(
            row,
            "00000000: 48656c6c6f20776f726c642e20746573  Hello world. tes"
        );
    }

    #[test]
    fn short_tail_aligns_across_all_group_sizes() {
        let short = b"t.";
        let full = b"0123456789abcdef";
        for &bpg in &[1usize, 2, 4, 8, 16] {
            let short_row = format_hex_row(0x10, short, bpg);
            let full_row = format_hex_row(0, full, bpg);
            let short_ascii = short_row.find("  t.").unwrap();
            let full_ascii = full_row.rfind("  ").unwrap();
            assert_eq!(short_ascii, full_ascii,
                       "ascii column misaligned for bytes_per_group={bpg}");
        }
    }

    #[test]
    fn hex_chars_to_bytes_per_group_maps_valid_values() {
        assert_eq!(hex_chars_to_bytes_per_group(2), Some(1));
        assert_eq!(hex_chars_to_bytes_per_group(4), Some(2));
        assert_eq!(hex_chars_to_bytes_per_group(8), Some(4));
        assert_eq!(hex_chars_to_bytes_per_group(16), Some(8));
        assert_eq!(hex_chars_to_bytes_per_group(32), Some(16));
    }

    #[test]
    fn hex_chars_to_bytes_per_group_rejects_invalid() {
        assert_eq!(hex_chars_to_bytes_per_group(0), None);
        assert_eq!(hex_chars_to_bytes_per_group(1), None);
        assert_eq!(hex_chars_to_bytes_per_group(3), None);
        assert_eq!(hex_chars_to_bytes_per_group(64), None);
    }
}
