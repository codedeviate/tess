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
            // Newlines never reach render_line in practice (LineIndex splits on them).
            // Defensive: ignore.
            i += 1;
        } else if b < 0x20 || b == 0x7F {
            // Control byte → ^X form
            let printable = if b == 0x7F { '?' } else { (b ^ 0x40) as char };
            push(&mut current, &mut rows, Cell::Char { ch: '^', width: 1 }, opts);
            push(&mut current, &mut rows, Cell::Char { ch: printable, width: 1 }, opts);
            i += 1;
        } else if b < 0x80 {
            // Plain printable ASCII
            push(&mut current, &mut rows, Cell::Char { ch: b as char, width: 1 }, opts);
            i += 1;
        } else {
            // High-bit byte: in this task, always render as <HH>. Task 6 promotes
            // valid UTF-8 sequences to a single grapheme cell.
            let s = format!("<{:02X}>", b);
            push_str(&mut current, &mut rows, &s, opts);
            i += 1;
        }
    }

    while current.len() < cols {
        current.push(Cell::Empty);
    }
    rows.push(current);
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
        for i in 0..8 {
            assert_eq!(rows[0][i], ch(' '), "col {} should be space", i);
        }
        assert_eq!(rows[0][8], ch('x'));
    }

    #[test]
    fn tab_at_col_three_advances_to_next_stop() {
        // "abc\tx" → cols 0,1,2 = a,b,c; tab fills to col 8 with spaces; col 8 = x
        let rows = render_line(b"abc\tx", &opts(20, true));
        assert_eq!(rows[0][0], ch('a'));
        assert_eq!(rows[0][2], ch('c'));
        for i in 3..8 {
            assert_eq!(rows[0][i], ch(' '));
        }
        assert_eq!(rows[0][8], ch('x'));
    }

    #[test]
    fn tab_at_col_eight_advances_to_sixteen() {
        let mut input = vec![b'a'; 8];
        input.push(b'\t');
        input.push(b'x');
        let rows = render_line(&input, &opts(20, true));
        for i in 8..16 {
            assert_eq!(rows[0][i], ch(' '));
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
}