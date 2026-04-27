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

    let push_char = |current: &mut Vec<Cell>, rows: &mut Vec<Vec<Cell>>, cell: Cell, opts: &RenderOpts| {
        if current.len() >= opts.cols as usize {
            if opts.wrap {
                rows.push(std::mem::take(current));
            } else {
                return;
            }
        }
        current.push(cell);
    };

    // Treat input as ASCII for now; bytes >= 0x80 handled in later tasks.
    for &b in bytes {
        if b.is_ascii() && !b.is_ascii_control() {
            push_char(&mut current, &mut rows, Cell::Char { ch: b as char, width: 1 }, opts);
        }
        // Other bytes: ignored for now (filled in by later tasks).
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
}
