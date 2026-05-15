use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Char { ch: char, width: u8 },
    Continuation,
    Empty,
}

#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub tab_width: u8,
    pub wrap: bool,
    pub cols: u16,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self { tab_width: 8, wrap: true, cols: 80 }
    }
}

/// Try to decode one grapheme cluster starting at `bytes[i]`.
/// Returns the cluster as &str and number of bytes consumed.
/// Returns None if `bytes[i..]` does not begin with a valid UTF-8 sequence.
fn decode_cluster(bytes: &[u8], i: usize) -> Option<(&str, usize)> {
    // Find the longest valid UTF-8 prefix starting at i (capped at 4 bytes
    // for the first codepoint, then continue while next codepoint is a
    // zero-width continuation of the same cluster).
    // Strategy: try to validate up to 4 bytes for the leading codepoint,
    // then extend as long as additional codepoints belong to the same cluster.

    // First, validate one codepoint.
    let max = (i + 4).min(bytes.len());
    let mut end = i;
    for try_end in (i + 1)..=max {
        if std::str::from_utf8(&bytes[i..try_end]).is_ok() {
            end = try_end;
            break;
        }
    }
    if end == i {
        return None;
    }

    // Now extend by additional valid codepoints that the segmenter groups
    // into the first cluster. Use unicode-segmentation for cluster boundaries.
    // We keep adding bytes (validated as UTF-8) until the cluster boundary
    // changes or we run out of bytes.
    let mut probe_end = end;
    loop {
        // Try extending by up to 4 more bytes.
        let probe_max = (probe_end + 4).min(bytes.len());
        let mut next_end = probe_end;
        for try_end in (probe_end + 1)..=probe_max {
            if std::str::from_utf8(&bytes[i..try_end]).is_ok() {
                next_end = try_end;
                break;
            }
        }
        if next_end == probe_end {
            break;
        }
        let candidate = std::str::from_utf8(&bytes[i..next_end]).unwrap();
        let cluster_count = candidate.graphemes(true).count();
        if cluster_count > 1 {
            // Adding broke into a new cluster; stop at probe_end.
            break;
        }
        probe_end = next_end;
    }

    Some((std::str::from_utf8(&bytes[i..probe_end]).unwrap(), probe_end - i))
}

pub fn render_line(bytes: &[u8], opts: &RenderOpts) -> Vec<Vec<Cell>> {
    let cols = opts.cols as usize;
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut current: Vec<Cell> = Vec::with_capacity(cols);

    fn push(current: &mut Vec<Cell>, rows: &mut Vec<Vec<Cell>>, cell: Cell, opts: &RenderOpts) {
        if current.len() >= opts.cols as usize {
            if opts.wrap {
                let mut full = std::mem::replace(current, Vec::with_capacity(opts.cols as usize));
                while full.len() < opts.cols as usize { full.push(Cell::Empty); }
                rows.push(full);
            } else {
                return;
            }
        }
        current.push(cell);
    }

    fn push_str(current: &mut Vec<Cell>, rows: &mut Vec<Vec<Cell>>, s: &str, opts: &RenderOpts) {
        for c in s.chars() {
            push(current, rows, Cell::Char { ch: c, width: 1 }, opts);
        }
    }

    fn push_wide(
        current: &mut Vec<Cell>,
        rows: &mut Vec<Vec<Cell>>,
        ch: char,
        width: u8,
        opts: &RenderOpts,
    ) {
        let cols = opts.cols as usize;
        // If the wide char wouldn't fit in the remainder of this row, wrap first.
        if current.len() + width as usize > cols {
            if opts.wrap {
                let mut full = std::mem::replace(current, Vec::with_capacity(cols));
                while full.len() < cols { full.push(Cell::Empty); }
                rows.push(full);
            } else {
                return; // chop
            }
        }
        current.push(Cell::Char { ch, width });
        for _ in 1..width {
            current.push(Cell::Continuation);
        }
    }

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\t' {
            let stop = opts.tab_width.max(1) as usize;
            let cur_col = current.len();
            let next_stop = ((cur_col / stop) + 1) * stop;
            for _ in cur_col..next_stop {
                push(&mut current, &mut rows, Cell::Char { ch: ' ', width: 1 }, opts);
            }
            i += 1;
        } else if b == b'\n' {
            i += 1;
        } else if b < 0x20 || b == 0x7F {
            let printable = if b == 0x7F { '?' } else { (b ^ 0x40) as char };
            push(&mut current, &mut rows, Cell::Char { ch: '^', width: 1 }, opts);
            push(&mut current, &mut rows, Cell::Char { ch: printable, width: 1 }, opts);
            i += 1;
        } else {
            // Try to decode a UTF-8 grapheme cluster starting at i.
            match decode_cluster(bytes, i) {
                Some((cluster, consumed)) => {
                    let w = UnicodeWidthStr::width(cluster) as u8;
                    let base_char = cluster.chars().next().unwrap_or('\u{FFFD}');
                    if w == 0 {
                        // Lone combining mark with no base — emit replacement.
                        push(&mut current, &mut rows, Cell::Char { ch: '\u{FFFD}', width: 1 }, opts);
                    } else {
                        push_wide(&mut current, &mut rows, base_char, w, opts);
                    }
                    i += consumed;
                }
                None => {
                    // Invalid byte: emit <HH>, advance one byte.
                    let s = format!("<{:02X}>", b);
                    push_str(&mut current, &mut rows, &s, opts);
                    i += 1;
                }
            }
        }
    }

    while current.len() < cols {
        current.push(Cell::Empty);
    }
    rows.push(current);
    rows
}

pub fn count_rows(bytes: &[u8], opts: &RenderOpts) -> usize {
    if !opts.wrap {
        return 1;
    }
    let cols = opts.cols.max(1) as usize;
    let mut col = 0usize;
    let mut rows = 1usize;

    let bump = |w: usize, col: &mut usize, rows: &mut usize| {
        if *col + w > cols {
            *rows += 1;
            *col = 0;
        }
        *col += w;
    };

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\t' {
            let stop = opts.tab_width.max(1) as usize;
            let next_stop = ((col / stop) + 1) * stop;
            let advance = next_stop - col;
            // Tabs may overflow into multiple wraps if cols < tab_width.
            for _ in 0..advance {
                bump(1, &mut col, &mut rows);
            }
            i += 1;
        } else if b == b'\n' {
            i += 1;
        } else if b < 0x20 || b == 0x7F {
            bump(1, &mut col, &mut rows); // ^
            bump(1, &mut col, &mut rows); // X
            i += 1;
        } else {
            match decode_cluster(bytes, i) {
                Some((cluster, consumed)) => {
                    let w = UnicodeWidthStr::width(cluster);
                    let w = if w == 0 { 1 } else { w };
                    bump(w, &mut col, &mut rows);
                    i += consumed;
                }
                None => {
                    // <HH> = 4 cells
                    for _ in 0..4 { bump(1, &mut col, &mut rows); }
                    i += 1;
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(cols: u16, wrap: bool) -> RenderOpts {
        RenderOpts { tab_width: 8, wrap, cols }
    }

    fn ch(c: char) -> Cell { Cell::Char { ch: c, width: 1 } }

    #[test]
    fn ascii_short_line_pads_to_cols() {
        let rows = render_line(b"hi", &opts(5, true));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![ch('h'), ch('i'), Cell::Empty, Cell::Empty, Cell::Empty]);
    }

    #[test]
    fn ascii_exact_width() {
        let rows = render_line(b"hello", &opts(5, true));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![ch('h'), ch('e'), ch('l'), ch('l'), ch('o')]);
    }

    #[test]
    fn empty_input_yields_one_empty_row() {
        let rows = render_line(b"", &opts(3, true));
        assert_eq!(rows, vec![vec![Cell::Empty, Cell::Empty, Cell::Empty]]);
    }

    #[test]
    fn tab_at_col_zero_expands_to_eight() {
        let rows = render_line(b"\tx", &opts(20, true));
        // Eight spaces, then 'x', then padding.
        for (i, cell) in rows[0].iter().take(8).enumerate() {
            assert_eq!(*cell, ch(' '), "col {i} should be space");
        }
        assert_eq!(rows[0][8], ch('x'));
    }

    #[test]
    fn tab_at_col_three_advances_to_next_stop() {
        // "abc\tx" → cols 0,1,2 = a,b,c; tab fills to col 8 with spaces; col 8 = x
        let rows = render_line(b"abc\tx", &opts(20, true));
        assert_eq!(rows[0][0], ch('a'));
        assert_eq!(rows[0][2], ch('c'));
        for cell in rows[0].iter().skip(3).take(5) {
            assert_eq!(*cell, ch(' '));
        }
        assert_eq!(rows[0][8], ch('x'));
    }

    #[test]
    fn tab_at_col_eight_advances_to_sixteen() {
        let mut input = vec![b'a'; 8];
        input.push(b'\t');
        input.push(b'x');
        let rows = render_line(&input, &opts(20, true));
        for cell in rows[0].iter().skip(8).take(8) {
            assert_eq!(*cell, ch(' '));
        }
        assert_eq!(rows[0][16], ch('x'));
    }

    #[test]
    fn null_renders_as_caret_at() {
        let rows = render_line(b"\0", &opts(5, true));
        assert_eq!(rows[0][0], ch('^'));
        assert_eq!(rows[0][1], ch('@'));
    }

    #[test]
    fn esc_renders_as_caret_lbracket() {
        let rows = render_line(b"\x1b", &opts(5, true));
        assert_eq!(rows[0][0], ch('^'));
        assert_eq!(rows[0][1], ch('['));
    }

    #[test]
    fn del_renders_as_caret_question() {
        let rows = render_line(b"\x7f", &opts(5, true));
        assert_eq!(rows[0][0], ch('^'));
        assert_eq!(rows[0][1], ch('?'));
    }

    #[test]
    fn invalid_utf8_byte_renders_as_angle_hex() {
        let rows = render_line(&[0xFF], &opts(8, true));
        assert_eq!(rows[0][0], ch('<'));
        assert_eq!(rows[0][1], ch('F'));
        assert_eq!(rows[0][2], ch('F'));
        assert_eq!(rows[0][3], ch('>'));
    }

    #[test]
    fn partial_multibyte_each_byte_renders_separately() {
        // 0xC3 starts a 2-byte sequence; alone it's invalid → <C3>
        let rows = render_line(&[0xC3], &opts(8, true));
        assert_eq!(rows[0][0], ch('<'));
        assert_eq!(rows[0][1], ch('C'));
        assert_eq!(rows[0][2], ch('3'));
        assert_eq!(rows[0][3], ch('>'));
    }

    #[test]
    fn single_byte_utf8_e_acute() {
        let rows = render_line("é".as_bytes(), &opts(5, true));
        assert_eq!(rows[0][0], Cell::Char { ch: 'é', width: 1 });
    }

    #[test]
    fn cjk_char_takes_two_columns() {
        // 日 is width 2.
        let rows = render_line("日".as_bytes(), &opts(5, true));
        assert_eq!(rows[0][0], Cell::Char { ch: '日', width: 2 });
        assert_eq!(rows[0][1], Cell::Continuation);
        assert_eq!(rows[0][2], Cell::Empty);
    }

    #[test]
    fn emoji_takes_two_columns() {
        let rows = render_line("🦀".as_bytes(), &opts(5, true));
        // Width depends on unicode-width; crab emoji is width 2.
        assert!(matches!(rows[0][0], Cell::Char { width: 2, .. }));
        assert_eq!(rows[0][1], Cell::Continuation);
    }

    #[test]
    fn combining_mark_folds_into_prior_cell() {
        // "e\u{0301}" is one grapheme cluster (e with combining acute).
        let rows = render_line("e\u{0301}".as_bytes(), &opts(5, true));
        // Cluster renders as a single cell carrying base char.
        assert!(matches!(rows[0][0], Cell::Char { width: 1, .. }));
        assert_eq!(rows[0][1], Cell::Empty);
    }

    #[test]
    fn wrap_long_line_into_multiple_rows() {
        let rows = render_line(b"abcdefghij", &opts(4, true));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec![ch('a'), ch('b'), ch('c'), ch('d')]);
        assert_eq!(rows[1], vec![ch('e'), ch('f'), ch('g'), ch('h')]);
        assert_eq!(rows[2], vec![ch('i'), ch('j'), Cell::Empty, Cell::Empty]);
    }

    #[test]
    fn chop_long_line_truncates() {
        let rows = render_line(b"abcdefghij", &opts(4, false));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![ch('a'), ch('b'), ch('c'), ch('d')]);
    }

    #[test]
    fn wide_char_at_boundary_pushed_to_next_row() {
        // cols=3, content "ab日" — 日 is width 2, doesn't fit at col 2,
        // so row 0 = a, b, Empty; row 1 = 日(continuation), Empty.
        let rows = render_line("ab日".as_bytes(), &opts(3, true));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec![ch('a'), ch('b'), Cell::Empty]);
        assert_eq!(rows[1][0], Cell::Char { ch: '日', width: 2 });
        assert_eq!(rows[1][1], Cell::Continuation);
        assert_eq!(rows[1][2], Cell::Empty);
    }

    #[test]
    fn count_rows_matches_render_line_for_short() {
        let o = opts(80, true);
        let bytes = b"hello world";
        assert_eq!(count_rows(bytes, &o), render_line(bytes, &o).len());
    }

    #[test]
    fn count_rows_matches_render_line_for_long_wrap() {
        let o = opts(4, true);
        let bytes = b"abcdefghij";
        assert_eq!(count_rows(bytes, &o), render_line(bytes, &o).len());
    }

    #[test]
    fn count_rows_chop_is_one() {
        let o = opts(4, false);
        let bytes = b"abcdefghij";
        assert_eq!(count_rows(bytes, &o), 1);
    }

    #[test]
    fn count_rows_handles_wide_char() {
        let o = opts(3, true);
        let bytes = "ab日".as_bytes();
        assert_eq!(count_rows(bytes, &o), render_line(bytes, &o).len());
    }
}