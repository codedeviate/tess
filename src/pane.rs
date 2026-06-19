//! Split-view layout. The FOCUSED pane lives in `app::run`'s loose locals; this
//! module bundles the OTHER pane (`Pane`) and provides the pure compositor that
//! stitches two half-width frames into one full-width frame. No terminal I/O.

use crate::line_index::LineIndex;
use crate::render::Cell;
use crate::source::Source;
use crate::viewport::{Frame, RowStyle, Viewport};

/// Width of the inter-pane divider, in columns.
pub const DIVIDER: usize = 1;

/// One side of a split: its own source, index, viewport, and per-pane
/// follow/animation bookkeeping. The focused pane lives in app::run's loose
/// locals; this is the stashed partner swapped in on focus change.
pub struct Pane {
    pub src: Box<dyn Source>,
    pub idx: LineIndex,
    pub viewport: Viewport,
    pub last_revision: u64,
    #[cfg(feature = "image")]
    pub last_tick: std::time::Instant,
}

/// Left/right content widths for a split at `cols` columns (1-col divider).
/// Right gets the extra column on odd widths. Returns `(cols, 0)` when there's
/// no room to split — caller renders the focused pane full-width.
pub fn split_widths(cols: u16) -> (u16, u16) {
    const MIN: usize = 8; // each pane needs a usable minimum
    let c = cols as usize;
    if c < 2 * MIN + DIVIDER {
        return (cols, 0);
    }
    let usable = c - DIVIDER;
    let left = usable / 2;
    (left as u16, (usable - left) as u16)
}

fn divider_cell() -> Cell {
    Cell::Char {
        ch: '\u{2502}', // │
        width: 1,
        style: crate::ansi::Style { dim: true, ..Default::default() },
        hyperlink: None,
    }
}

/// `--dim` is a per-row style, but a merged row spans two panes that may differ.
/// Flatten a pane's row-level dim into its cells so the merged row can carry it.
fn flatten_dim(cells: &mut [Cell]) {
    for c in cells.iter_mut() {
        if let Cell::Char { style, .. } = c {
            style.dim = true;
        }
    }
}

/// Fit a pane's status to `w` display columns, prefixing the focused pane's with
/// a `*` marker. Width-aware (so `×`/`»` glyphs count as 1).
fn fit_pane_status(s: &str, w: usize, focused: bool) -> String {
    use unicode_width::UnicodeWidthChar;
    let marked = if focused { format!("*{s}") } else { s.to_string() };
    let mut out = String::with_capacity(w);
    let mut width = 0usize;
    for ch in marked.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + cw > w {
            break;
        }
        out.push(ch);
        width += cw;
    }
    for _ in width..w {
        out.push(' ');
    }
    out
}

/// Stitch two half-width pane frames into one full-width frame:
/// `left cells | divider | right cells` per body row, per-pane statuses joined,
/// right pane's highlight ranges shifted past the divider, row-level dim
/// flattened into cells. Pure.
pub fn compose_split(left: &Frame, right: &Frame, left_w: u16, cols: u16, focused_left: bool) -> Frame {
    let lw = left_w as usize;
    let rw = (cols as usize).saturating_sub(lw + DIVIDER);
    let body_rows = left.body.len().max(right.body.len());
    let mut body = Vec::with_capacity(body_rows);
    let mut highlights = Vec::with_capacity(body_rows);
    let empty_row: Vec<Cell> = Vec::new();
    let no_hl: Vec<std::ops::Range<usize>> = Vec::new();
    for r in 0..body_rows {
        let mut lcells = left.body.get(r).cloned().unwrap_or_else(|| empty_row.clone());
        lcells.resize(lw, Cell::Empty);
        if left.row_styles.get(r) == Some(&RowStyle::Dim) {
            flatten_dim(&mut lcells);
        }
        let mut rcells = right.body.get(r).cloned().unwrap_or_else(|| empty_row.clone());
        rcells.resize(rw, Cell::Empty);
        if right.row_styles.get(r) == Some(&RowStyle::Dim) {
            flatten_dim(&mut rcells);
        }
        let mut row = Vec::with_capacity(cols as usize);
        row.extend(lcells);
        row.push(divider_cell());
        row.extend(rcells);
        body.push(row);

        let off = lw + DIVIDER;
        let mut hl = left.highlights.get(r).cloned().unwrap_or_else(|| no_hl.clone());
        if let Some(rh) = right.highlights.get(r) {
            hl.extend(rh.iter().map(|x| (x.start + off)..(x.end + off)));
        }
        highlights.push(hl);
    }
    let lstat = fit_pane_status(&left.status, lw, focused_left);
    let rstat = fit_pane_status(&right.status, rw, !focused_left);
    let status = format!("{lstat}\u{2502}{rstat}");
    Frame {
        body,
        row_styles: vec![RowStyle::Normal; body_rows],
        highlights,
        status,
        status_style: left.status_style,
        raw_rows: vec![None; body_rows],
        image_blob: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ansi::Style;

    fn cell(ch: char) -> Cell {
        Cell::Char { ch, width: 1, style: Style::default(), hyperlink: None }
    }
    fn frame(rows: Vec<Vec<Cell>>, status: &str) -> Frame {
        let n = rows.len();
        Frame {
            body: rows,
            row_styles: vec![RowStyle::Normal; n],
            highlights: vec![Vec::new(); n],
            status: status.to_string(),
            status_style: Style::default(),
            raw_rows: vec![None; n],
            image_blob: None,
        }
    }

    #[test]
    fn split_widths_even_odd_and_too_small() {
        assert_eq!(split_widths(33), (16, 16));
        assert_eq!(split_widths(34), (16, 17));
        assert_eq!(split_widths(10), (10, 0));
    }

    #[test]
    fn compose_stitches_rows_with_divider() {
        let l = frame(vec![vec![cell('a'), cell('b')]], "L");
        let r = frame(vec![vec![cell('x'), cell('y')]], "R");
        let m = compose_split(&l, &r, 2, 5, true);
        assert_eq!(m.body.len(), 1);
        let row = &m.body[0];
        assert_eq!(row.len(), 5);
        assert!(matches!(row[0], Cell::Char { ch: 'a', .. }));
        assert!(matches!(row[1], Cell::Char { ch: 'b', .. }));
        assert!(matches!(row[2], Cell::Char { ch: '\u{2502}', .. }), "divider at col 2");
        assert!(matches!(row[3], Cell::Char { ch: 'x', .. }));
        assert!(matches!(row[4], Cell::Char { ch: 'y', .. }));
        assert!(m.status.starts_with("*L"), "focused-left status marked: {:?}", m.status);
        assert!(m.status.contains('\u{2502}'));
    }

    #[test]
    fn right_pane_highlights_shifted_past_divider() {
        let l = frame(vec![vec![cell('a'), cell('b')]], "L");
        let mut r = frame(vec![vec![cell('x'), cell('y')]], "R");
        r.highlights[0] = vec![0..1];
        let m = compose_split(&l, &r, 2, 5, true);
        assert_eq!(m.highlights[0], vec![3..4]);
    }

    #[test]
    fn dim_row_flattened_into_cells() {
        let mut l = frame(vec![vec![cell('a')]], "L");
        l.row_styles[0] = RowStyle::Dim;
        let r = frame(vec![vec![cell('x')]], "R");
        let m = compose_split(&l, &r, 1, 3, true);
        match &m.body[0][0] {
            Cell::Char { style, .. } => assert!(style.dim, "left dim flattened into cell"),
            _ => panic!(),
        }
        assert_eq!(m.row_styles[0], RowStyle::Normal, "merged row style is Normal");
    }

    #[test]
    fn focused_right_marks_right_status() {
        // cols=5 so the right pane has width 2 — room for the `*R` marker.
        // (At cols=3 the 1-col right pane can only hold `*`, truncating the `R`.)
        let l = frame(vec![vec![cell('a'), cell('b')]], "L");
        let r = frame(vec![vec![cell('x'), cell('y')]], "R");
        let m = compose_split(&l, &r, 2, 5, false);
        assert!(m.status.contains("\u{2502}*R"), "focused-right status marked: {:?}", m.status);
    }

    #[test]
    fn uneven_body_rows_pad_with_empty() {
        // Left has 2 rows, right has 1: merged row 1's right half must be all Empty,
        // and every merged row is exactly left_w + divider + right_w cells.
        let l = frame(vec![vec![cell('a')], vec![cell('b')]], "L"); // 2 rows
        let r = frame(vec![vec![cell('x')]], "R");                  // 1 row
        let m = compose_split(&l, &r, 1, 3, true); // lw=1, rw = 3-(1+1)=1
        assert_eq!(m.body.len(), 2, "merged uses the taller pane's row count");
        for row in &m.body {
            assert_eq!(row.len(), 3, "each merged row is lw + divider + rw");
            assert!(matches!(row[1], Cell::Char { ch: '\u{2502}', .. }), "divider at col 1");
        }
        // Row 1: left 'b', divider, right padded Empty.
        assert!(matches!(m.body[1][0], Cell::Char { ch: 'b', .. }));
        assert!(matches!(m.body[1][2], Cell::Empty), "missing right row → Empty pad");
    }

    #[test]
    fn pane_status_truncates_to_width() {
        // A status longer than the pane width is fit-truncated by display columns.
        // lw=4: focused-left status "*LongStatus" truncates to 4 cols → "*Lon".
        let l = frame(vec![vec![cell('a')]], "LongStatus");
        let r = frame(vec![vec![cell('x')]], "R");
        let m = compose_split(&l, &r, 4, 9, true); // lw=4, rw = 9-(4+1)=4
        // Left status segment is exactly the first 4 cols of the row's status.
        assert!(m.status.starts_with("*Lon"), "focused-left status truncated to width 4: {:?}", m.status);
        // The divider separates the two pane statuses; left segment is 4 wide.
        let div_pos = m.status.find('\u{2502}').expect("divider in status");
        // 4 display columns before the divider (all ASCII here → 4 bytes).
        assert_eq!(div_pos, 4, "left status occupies exactly left_w columns before divider");
    }
}
