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


/// Re-derive a pane's top line under scroll-lock. `focused_top` is the focused
/// pane's current top; `focused_offset`/`pane_offset` are the two panes' offsets
/// relative to the leftmost pane (captured at lock enable). Clamped to
/// `0..=pane_max`. Recomputed from the fixed offsets each call (no drift;
/// restores after a clamp). Tab-invariant.
pub fn locked_pane_top(focused_top: usize, focused_offset: isize, pane_offset: isize, pane_max: usize) -> usize {
    let raw = focused_top as isize + (pane_offset - focused_offset);
    raw.clamp(0, pane_max as isize) as usize
}

/// Column widths for `n` vertical panes at `cols` columns (n-1 dividers).
/// Remainder columns go to the rightmost panes (matching the 2-pane legacy
/// convention where the right pane receives the extra column). Returns a single
/// `[cols]` entry (the too-narrow fallback — caller renders only the focused
/// pane full-width) when each pane would fall below the usable minimum.
pub fn split_widths_n(cols: u16, n: usize) -> Vec<u16> {
    const MIN: usize = 8;
    if n <= 1 {
        return vec![cols];
    }
    let c = cols as usize;
    let dividers = n - 1;
    if c < n * MIN + dividers {
        return vec![cols];
    }
    let usable = c - dividers;
    let base = usable / n;
    let rem = usable % n;
    // Remainder goes to the rightmost panes, matching split_widths's convention
    // of giving the right pane the extra column on odd widths.
    (0..n).map(|i| (base + if i >= n - rem { 1 } else { 0 }) as u16).collect()
}

/// Map a screen column to the 0-based visible pane index, given the per-pane
/// widths from `split_widths_n`. Panes are laid out left-to-right separated by a
/// 1-column `DIVIDER` (as in `compose_panes`). A column inside a pane's content
/// returns that pane; a column on the divider after pane `i` resolves to pane `i`
/// (the left pane); a column past the end clamps to the last pane. A single-entry
/// `widths` (single pane / too-narrow fallback) always returns 0.
pub fn pane_at_column(col: u16, widths: &[u16]) -> usize {
    let last = widths.len().saturating_sub(1);
    let mut x = 0usize;
    let col = col as usize;
    for (i, &w) in widths.iter().enumerate() {
        let content_end = x + w as usize; // exclusive
        if col < content_end {
            return i;
        }
        if i == last {
            return last; // past the last pane's content → clamp
        }
        if col == content_end {
            return i; // divider column belongs to the pane on its left
        }
        x = content_end + DIVIDER;
    }
    last
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

/// Stitch N pre-rendered column frames left-to-right with dividers between them.
/// `widths[i]` is column i's content width; `focused_idx` marks which pane's
/// status carries the `*`. Per-row cells concatenated (resized to width), each
/// frame's highlight ranges shifted by its running column origin, row-level dim
/// flattened into cells, statuses fit-to-width + joined by the divider char.
/// Pure; cell-mode only (raw_rows/image_blob = none).
pub fn compose_panes(frames: &[Frame], widths: &[u16], cols: u16, focused_idx: usize) -> Frame {
    let body_rows = frames.iter().map(|f| f.body.len()).max().unwrap_or(0);
    let mut body = Vec::with_capacity(body_rows);
    let mut highlights: Vec<Vec<std::ops::Range<usize>>> = Vec::with_capacity(body_rows);
    let mut row_styles = Vec::with_capacity(body_rows);
    let empty: Vec<Cell> = Vec::new();
    for r in 0..body_rows {
        let mut row = Vec::with_capacity(cols as usize);
        let mut hl: Vec<std::ops::Range<usize>> = Vec::new();
        let mut origin = 0usize;
        for (i, f) in frames.iter().enumerate() {
            let w = widths[i] as usize;
            let mut cells = f.body.get(r).cloned().unwrap_or_else(|| empty.clone());
            cells.resize(w, Cell::Empty);
            if f.row_styles.get(r) == Some(&RowStyle::Dim) {
                flatten_dim(&mut cells);
            }
            row.extend(cells);
            if let Some(ranges) = f.highlights.get(r) {
                hl.extend(ranges.iter().map(|x| (x.start + origin)..(x.end + origin)));
            }
            origin += w;
            if i + 1 < frames.len() {
                row.push(divider_cell());
                origin += DIVIDER;
            }
        }
        body.push(row);
        highlights.push(hl);
        row_styles.push(RowStyle::Normal);
    }
    let status: String = frames
        .iter()
        .enumerate()
        .map(|(i, f)| fit_pane_status(&f.status, widths[i] as usize, i == focused_idx))
        .collect::<Vec<_>>()
        .join("\u{2502}");
    Frame {
        body,
        row_styles,
        highlights,
        status,
        status_style: frames.get(focused_idx).map(|f| f.status_style).unwrap_or_default(),
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
    fn compose_panes_flattens_dim_row_into_cells() {
        let mut a = frame(vec![vec![cell('a')]], "A");
        a.row_styles[0] = RowStyle::Dim;
        let b = frame(vec![vec![cell('b')]], "B");
        let out = compose_panes(&[a, b], &[1u16, 1], 3, 0);
        match &out.body[0][0] {
            Cell::Char { style, .. } => assert!(style.dim, "dim flattened into the cell"),
            _ => panic!("expected a Char cell"),
        }
        assert_eq!(out.row_styles[0], RowStyle::Normal, "merged row style is Normal");
    }

    #[test]
    fn compose_panes_status_marks_focused_and_truncates() {
        // focused pane (index 0) gets `*` and its status fit-truncates to width 4:
        // "*LongStatus" → "*Lon"; divider at column 4.
        let a = frame(vec![vec![cell('a')]], "LongStatus");
        let b = frame(vec![vec![cell('b')]], "B");
        let out = compose_panes(&[a, b], &[4u16, 4], 9, 0);
        assert!(out.status.starts_with("*Lon"), "focused status marked + truncated: {:?}", out.status);
        assert_eq!(out.status.find('\u{2502}'), Some(4), "left status occupies exactly 4 cols");
    }

    #[test]
    fn split_widths_n_even() {
        // 3 panes, 2 dividers: usable = 32, base 10, remainder 2 → last two get +1
        // (rightmost-first distribution, matching split_widths's right-gets-extra convention).
        assert_eq!(super::split_widths_n(34, 3), vec![10, 11, 11]);
    }
    #[test]
    fn split_widths_n_two_matches_legacy() {
        let (l, r) = super::split_widths(34);
        assert_eq!(super::split_widths_n(34, 2), vec![l, r]);
    }
    #[test]
    fn split_widths_n_too_narrow_falls_back_to_one() {
        // 3 panes need >= 3*8 + 2 = 26 cols; at 20 fall back to a single full-width entry.
        assert_eq!(super::split_widths_n(20, 3), vec![20]);
    }
    #[test]
    fn split_widths_n_one_is_full_width() {
        assert_eq!(super::split_widths_n(80, 1), vec![80]);
    }

    #[test]
    fn compose_panes_stitches_n_with_dividers() {
        use crate::render::Cell;
        use crate::viewport::{Frame, RowStyle};
        let mk = |ch: char, w: usize| Frame {
            body: vec![vec![Cell::Char { ch, width: 1, style: Default::default(), hyperlink: None }; w]],
            row_styles: vec![RowStyle::Normal], highlights: vec![vec![]],
            status: format!("{ch}"), status_style: Default::default(),
            raw_rows: vec![None], image_blob: None,
        };
        let frames = vec![mk('a', 3), mk('b', 3), mk('c', 3)];
        let widths = vec![3u16, 3, 3];
        let cols = 3 + 1 + 3 + 1 + 3; // 11
        let out = super::compose_panes(&frames, &widths, cols as u16, 0);
        assert!(matches!(out.body[0][3], Cell::Char { ch: '\u{2502}', .. }));
        assert!(matches!(out.body[0][7], Cell::Char { ch: '\u{2502}', .. }));
        assert!(matches!(out.body[0][0], Cell::Char { ch: 'a', .. }));
        assert!(matches!(out.body[0][4], Cell::Char { ch: 'b', .. }));
        assert!(matches!(out.body[0][8], Cell::Char { ch: 'c', .. }));
    }

    #[test]
    fn locked_pane_top_derives_from_offsets() {
        // offsets relative to pane 0: [0, 240, 100]. Focused = pane 1 (offset 240) at top 345.
        // pane 0 target = 345 + (0 - 240) = 105; pane 2 = 345 + (100 - 240) = 205.
        assert_eq!(super::locked_pane_top(345, 240, 0, 1_000_000), 105);
        assert_eq!(super::locked_pane_top(345, 240, 100, 1_000_000), 205);
    }
    #[test]
    fn locked_pane_top_clamps_and_restores() {
        assert_eq!(super::locked_pane_top(10, 240, 0, 1_000_000), 0);     // would be negative → 0
        assert_eq!(super::locked_pane_top(300, 240, 0, 1_000_000), 60);   // restores after clamp (no drift)
        assert_eq!(super::locked_pane_top(10_000, 0, 240, 5_100), 5_100); // high clamp
    }
    #[test]
    fn locked_pane_top_tab_invariant() {
        // tops 100/340/200 → offsets 0/240/100. Deriving pane 2 from focused=pane0 vs focused=pane1 agree.
        assert_eq!(super::locked_pane_top(100, 0, 100, 1_000_000), 200);
        assert_eq!(super::locked_pane_top(340, 240, 100, 1_000_000), 200);
    }

    #[test]
    fn pane_at_column_single_pane_is_always_zero() {
        let w = vec![80u16];
        assert_eq!(pane_at_column(0, &w), 0);
        assert_eq!(pane_at_column(79, &w), 0);
        assert_eq!(pane_at_column(500, &w), 0);
    }

    #[test]
    fn pane_at_column_two_panes_with_divider() {
        // split_widths_n(80, 2) == [39, 40]: pane0 cols 0..=38, divider at 39,
        // pane1 cols 40..=79. The divider resolves to the left pane.
        let w = split_widths_n(80, 2);
        assert_eq!(w, vec![39, 40]);
        assert_eq!(pane_at_column(0, &w), 0);
        assert_eq!(pane_at_column(38, &w), 0);
        assert_eq!(pane_at_column(39, &w), 0); // divider → left pane
        assert_eq!(pane_at_column(40, &w), 1);
        assert_eq!(pane_at_column(79, &w), 1);
        assert_eq!(pane_at_column(80, &w), 1); // past end → last pane
        assert_eq!(pane_at_column(999, &w), 1);
    }

    #[test]
    fn pane_at_column_three_panes() {
        // split_widths_n(80, 3) == [26,26,26]: p0 0..=25, div 26, p1 27..=52,
        // div 53, p2 54..=79.
        let w = split_widths_n(80, 3);
        assert_eq!(w, vec![26, 26, 26]);
        assert_eq!(pane_at_column(0, &w), 0);
        assert_eq!(pane_at_column(25, &w), 0);
        assert_eq!(pane_at_column(26, &w), 0); // first divider → left pane (0)
        assert_eq!(pane_at_column(27, &w), 1);
        assert_eq!(pane_at_column(52, &w), 1);
        assert_eq!(pane_at_column(53, &w), 1); // second divider → left pane (1)
        assert_eq!(pane_at_column(54, &w), 2);
        assert_eq!(pane_at_column(79, &w), 2);
        assert_eq!(pane_at_column(200, &w), 2); // past end → last pane
    }

    #[test]
    fn compose_panes_offsets_right_highlights() {
        use crate::render::Cell;
        use crate::viewport::{Frame, RowStyle};
        let mk = |w: usize, hl: Vec<std::ops::Range<usize>>| Frame {
            body: vec![vec![Cell::Char { ch: 'x', width: 1, style: Default::default(), hyperlink: None }; w]],
            row_styles: vec![RowStyle::Normal], highlights: vec![hl],
            status: String::new(), status_style: Default::default(),
            raw_rows: vec![None], image_blob: None,
        };
        let frames = vec![mk(3, vec![0..1]), mk(3, vec![1..2])];
        let out = super::compose_panes(&frames, &vec![3u16, 3], 7, 0);
        assert!(out.highlights[0].contains(&(0..1)));
        assert!(out.highlights[0].contains(&(5..6))); // right pane offset by 3 + DIVIDER(1) = 4
    }
}
