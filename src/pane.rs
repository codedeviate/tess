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

/// Capture the fixed scroll-lock offset in stable physical terms:
/// `right_top - left_top`. Independent of which pane is focused, so a `Tab`
/// focus-swap never disturbs it. Returned as `isize` (the right pane may sit
/// above the left).
pub fn capture_lock_offset(focused_top: usize, partner_top: usize, focused_left: bool) -> isize {
    let (left, right) = if focused_left {
        (focused_top, partner_top)
    } else {
        (partner_top, focused_top)
    };
    right as isize - left as isize
}

/// Re-derive the non-focused pane's top line from the focused pane's current
/// top line and the fixed `offset` (`right_top - left_top`), clamped to
/// `0..=partner_max`. Always recomputed from the offset (never accumulated),
/// so an EOF/top clamp holds without drift and the alignment restores once it
/// fits again.
pub fn locked_partner_top(
    focused_top: usize,
    offset: isize,
    focused_left: bool,
    partner_max: usize,
) -> usize {
    let raw = if focused_left {
        // Focused is physical left; partner is right = left + offset.
        focused_top as isize + offset
    } else {
        // Focused is physical right; partner is left = right - offset.
        focused_top as isize - offset
    };
    raw.clamp(0, partner_max as isize) as usize
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

    #[test]
    fn capture_lock_offset_is_right_minus_left_either_focus() {
        // Physical: left_top = 100, right_top = 340 → offset 240, regardless
        // of which side is focused.
        assert_eq!(super::capture_lock_offset(100, 340, true), 240);  // focused = left
        assert_eq!(super::capture_lock_offset(340, 100, false), 240); // focused = right
    }

    #[test]
    fn locked_partner_top_applies_offset_per_focus_side() {
        let offset = 240; // right - left
        assert_eq!(super::locked_partner_top(105, offset, true, 100_000), 345);
        assert_eq!(super::locked_partner_top(345, offset, false, 100_000), 105);
    }

    #[test]
    fn locked_partner_top_clamps_low_and_high() {
        assert_eq!(super::locked_partner_top(10, 240, false, 100_000), 0);
        assert_eq!(super::locked_partner_top(5_000, 240, true, 5_100), 5_100);
    }

    #[test]
    fn locked_partner_top_restores_after_clamp() {
        let offset = 240;
        assert_eq!(super::locked_partner_top(10, offset, false, 100_000), 0);
        assert_eq!(super::locked_partner_top(300, offset, false, 100_000), 60);
    }

    #[test]
    fn locked_partner_top_is_tab_invariant() {
        let offset = 240;
        assert_eq!(super::locked_partner_top(100, offset, true, 100_000), 340);
        assert_eq!(super::locked_partner_top(340, offset, false, 100_000), 100);
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
