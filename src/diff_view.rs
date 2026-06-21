//! Renders the diff-mode aligned view: walks a `diff::DiffPair` alignment from a
//! `(pair_index, sub_row)` scroll position and stitches one full-width `Frame`
//! with filler rows, gutter signs, and per-class coloring. Bypasses the
//! per-pane viewports and `pane::compose_split`. Pure (reads line bytes via the
//! passed sources/indices).

use crate::diff::{DiffClass, DiffPair};
use crate::line_index::LineIndex;
use crate::render::{Cell, RenderOpts};
use crate::source::Source;
use crate::viewport::{Frame, RowStyle};

const DIVIDER: usize = 1;

fn gutter_sign(class: DiffClass) -> char {
    match class {
        DiffClass::Equal   => ' ',
        DiffClass::Changed => '~',
        DiffClass::Added   => '+',
        DiffClass::Removed => '-',
    }
}

fn class_style(class: DiffClass) -> crate::ansi::Style {
    use crate::ansi::{Color, Style};
    let mut s = Style::default();
    match class {
        DiffClass::Equal   => {}
        // Yellow = ANSI index 3
        DiffClass::Changed => s.fg = Some(Color::Ansi(3)),
        // Green = ANSI index 2
        DiffClass::Added   => s.fg = Some(Color::Ansi(2)),
        // Red = ANSI index 1
        DiffClass::Removed => s.fg = Some(Color::Ansi(1)),
    }
    s
}

/// Wrap one side's line (or empty for a filler side) into rows at opts.cols,
/// overlaying the class fg where the source didn't already set one.
fn side_rows(
    line: Option<usize>,
    class: DiffClass,
    src: &dyn Source,
    idx: &mut LineIndex,
    opts: &RenderOpts,
) -> Vec<Vec<Cell>> {
    match line {
        None => Vec::new(),
        Some(n) => {
            // Extend to n+1 so `scanned_through` passes line n's newline,
            // making `line_range(n)` return the full content bytes (not 0..0).
            idx.extend_to_line(n + 1, src);
            let range = idx.line_range(n, src);
            let bytes = src.bytes(range);
            let mut rows = crate::render::render_line(&bytes, opts, None);
            let style = class_style(class);
            if style != crate::ansi::Style::default() {
                for row in &mut rows {
                    for c in row.iter_mut() {
                        if let Cell::Char { style: cs, .. } = c {
                            if cs.fg.is_none() {
                                cs.fg = style.fg;
                            }
                        }
                    }
                }
            }
            if rows.is_empty() {
                rows.push(Vec::new());
            }
            rows
        }
    }
}

pub fn pair_height(
    pair: &DiffPair,
    lsrc: &dyn Source,
    lidx: &mut LineIndex,
    rsrc: &dyn Source,
    ridx: &mut LineIndex,
    lopts: &RenderOpts,
    ropts: &RenderOpts,
) -> usize {
    let l = side_rows(pair.left, pair.class, lsrc, lidx, lopts);
    let r = side_rows(pair.right, pair.class, rsrc, ridx, ropts);
    l.len().max(r.len()).max(1)
}

/// Map a byte range (into the decoded string) to a [start_col, end_col) cell-
/// column range on a single rendered row. Returns None if the line wraps to
/// more than one row (v1: intra-line spans only for single-row changed lines).
fn cols_for_byte_range(
    bytes: &[u8],
    opts: &RenderOpts,
    span: &std::ops::Range<usize>,
) -> Option<std::ops::Range<usize>> {
    let rows = crate::render::render_line(bytes, opts, None);
    if rows.len() != 1 {
        return None;
    }
    let decoded = crate::charset::decode_line(bytes, opts.encoding);
    let mut col = 0usize;
    let mut byte = 0usize;
    let mut start_col = None;
    for ch in decoded.chars() {
        if byte == span.start {
            start_col = Some(col);
        }
        if byte == span.end {
            return start_col.map(|s| s..col);
        }
        byte += ch.len_utf8();
        col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    if byte == span.end {
        return start_col.map(|s| s..col);
    }
    start_col.map(|s| s..col)
}

fn divider_cell() -> Cell {
    Cell::Char {
        ch: '\u{2502}',
        width: 1,
        style: crate::ansi::Style { dim: true, ..Default::default() },
        hyperlink: None,
    }
}

fn sign_cell(sign: char, class: DiffClass) -> Cell {
    Cell::Char {
        ch: sign,
        width: 1,
        style: class_style(class),
        hyperlink: None,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn compose_diff(
    pairs: &[DiffPair],
    lsrc: &dyn Source,
    lidx: &mut LineIndex,
    rsrc: &dyn Source,
    ridx: &mut LineIndex,
    pos: (usize, usize),
    cols: u16,
    left_w: u16,
    body_rows: usize,
    pane_opts: &RenderOpts,
    _focused_left: bool,
) -> Frame {
    let lw = left_w as usize;
    let rw = (cols as usize).saturating_sub(lw + DIVIDER);
    let lopts = RenderOpts { cols: lw as u16, ..pane_opts.clone() };
    let ropts = RenderOpts { cols: rw as u16, ..pane_opts.clone() };
    let mut body: Vec<Vec<Cell>> = Vec::with_capacity(body_rows);
    let mut row_styles: Vec<RowStyle> = Vec::with_capacity(body_rows);
    // Parallel to body: column ranges to render with reverse-video highlight.
    let mut highlights: Vec<Vec<std::ops::Range<usize>>> = Vec::with_capacity(body_rows);

    let (mut pi, mut sub) = pos;
    while body.len() < body_rows && pi < pairs.len() {
        let pair = &pairs[pi];
        let l = side_rows(pair.left, pair.class, lsrc, lidx, &lopts);
        let r = side_rows(pair.right, pair.class, rsrc, ridx, &ropts);
        let height = l.len().max(r.len()).max(1);

        // For Changed pairs where both sides render to a single row, pre-compute
        // the intra-line char highlight ranges. V1 limitation: multi-row lines
        // are skipped (line-level color still applies).
        let pair_hl: Vec<std::ops::Range<usize>> = if pair.class == DiffClass::Changed
            && l.len() == 1
            && r.len() == 1
        {
            // Re-fetch raw bytes (extend_to_line is idempotent so this is cheap).
            let lbytes: Vec<u8> = pair.left.map(|n| {
                let range = lidx.line_range(n, lsrc);
                lsrc.bytes(range).to_vec()
            }).unwrap_or_default();
            let rbytes: Vec<u8> = pair.right.map(|n| {
                let range = ridx.line_range(n, rsrc);
                rsrc.bytes(range).to_vec()
            }).unwrap_or_default();

            let (lspans, rspans) = crate::diff::char_spans(&lbytes, &rbytes, lopts.encoding);
            let mut hl = Vec::new();
            for span in &lspans {
                if let Some(col_range) = cols_for_byte_range(&lbytes, &lopts, span) {
                    hl.push(col_range);
                }
            }
            for span in &rspans {
                if let Some(col_range) = cols_for_byte_range(&rbytes, &ropts, span) {
                    // Offset right-side columns past the left pane + divider.
                    hl.push((lw + DIVIDER + col_range.start)..(lw + DIVIDER + col_range.end));
                }
            }
            hl
        } else {
            Vec::new()
        };

        for row_in_pair in sub..height {
            if body.len() >= body_rows {
                break;
            }
            let mut lcells = l.get(row_in_pair).cloned().unwrap_or_default();
            lcells.resize(lw, Cell::Empty);
            let mut rcells = r.get(row_in_pair).cloned().unwrap_or_default();
            rcells.resize(rw, Cell::Empty);
            if row_in_pair == 0 {
                let sign = gutter_sign(pair.class);
                if sign != ' ' {
                    if pair.left.is_some() {
                        if let Some(c) = lcells.get_mut(0) {
                            *c = sign_cell(sign, pair.class);
                        }
                    }
                    if pair.right.is_some() {
                        if let Some(c) = rcells.get_mut(0) {
                            *c = sign_cell(sign, pair.class);
                        }
                    }
                }
            }
            let mut row = Vec::with_capacity(cols as usize);
            row.extend(lcells);
            row.push(divider_cell());
            row.extend(rcells);
            body.push(row);
            row_styles.push(RowStyle::Normal);
            // Highlights only on the first row of the pair (intra-line spans
            // apply to single-row changed lines only — see pair_hl computation).
            if row_in_pair == 0 {
                highlights.push(pair_hl.clone());
            } else {
                highlights.push(Vec::new());
            }
        }
        pi += 1;
        sub = 0;
    }
    // Pad remaining rows with empty filler
    while body.len() < body_rows {
        let mut row = vec![Cell::Empty; lw];
        row.push(divider_cell());
        row.extend(vec![Cell::Empty; rw]);
        body.push(row);
        row_styles.push(RowStyle::Normal);
        highlights.push(Vec::new());
    }
    Frame {
        body,
        row_styles,
        highlights,
        status: String::new(),
        status_style: crate::ansi::Style::default(),
        raw_rows: vec![None; body_rows],
        image_blob: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MockSource;

    fn opts(cols: u16) -> RenderOpts {
        // encoding defaults to UTF-8 via Default; wrap on for pair-padding.
        RenderOpts { cols, wrap: true, ..RenderOpts::default() }
    }

    fn mock(bytes: &[u8]) -> MockSource {
        let s = MockSource::new();
        s.append(bytes);
        s
    }

    #[test]
    fn pair_height_is_max_of_two_sides() {
        let lsrc = mock(b"aaaaaa\n"); // 2 rows at width 4
        let rsrc = mock(b"bb\n");     // 1 row
        let mut lidx = LineIndex::new();
        let mut ridx = LineIndex::new();
        let pair = DiffPair { left: Some(0), right: Some(0), class: DiffClass::Changed };
        let h = pair_height(&pair, &lsrc, &mut lidx, &rsrc, &mut ridx, &opts(4), &opts(4));
        assert_eq!(h, 2);
    }

    #[test]
    fn compose_has_divider_and_fills_body_rows() {
        let lsrc = mock(b"aaaaaa\n");
        let rsrc = mock(b"bb\n");
        let mut lidx = LineIndex::new();
        let mut ridx = LineIndex::new();
        let pairs = vec![DiffPair { left: Some(0), right: Some(0), class: DiffClass::Changed }];
        let frame = compose_diff(&pairs, &lsrc, &mut lidx, &rsrc, &mut ridx, (0, 0), 9, 4, 3, &opts(4), true);
        assert_eq!(frame.body.len(), 3); // requested body rows
        for row in &frame.body {
            assert!(matches!(row.get(4), Some(Cell::Char { ch: '\u{2502}', .. })));
        }
    }

    #[test]
    fn added_pair_left_is_filler() {
        let lsrc = mock(b"\n");
        let rsrc = mock(b"new line\n");
        let mut lidx = LineIndex::new();
        let mut ridx = LineIndex::new();
        let pairs = vec![DiffPair { left: None, right: Some(0), class: DiffClass::Added }];
        let frame = compose_diff(&pairs, &lsrc, &mut lidx, &rsrc, &mut ridx, (0, 0), 21, 10, 1, &opts(10), true);
        let left_half = &frame.body[0][0..10];
        assert!(left_half.iter().all(|c| matches!(c, Cell::Empty) || matches!(c, Cell::Char { ch: ' ', .. })));
    }

    #[test]
    fn changed_pair_highlights_differing_chars() {
        // "workers 4" vs "workers 8" — single row each at a wide pane; the
        // differing last char highlighted on both sides (right offset past divider).
        let lsrc = mock(b"workers 4\n");
        let rsrc = mock(b"workers 8\n");
        let mut lidx = LineIndex::new();
        let mut ridx = LineIndex::new();
        let pairs = vec![DiffPair { left: Some(0), right: Some(0), class: DiffClass::Changed }];
        let left_w: u16 = 12;
        let cols = left_w * 2 + 1;
        let frame = compose_diff(&pairs, &lsrc, &mut lidx, &rsrc, &mut ridx, (0,0), cols, left_w, 1, &opts(left_w), true);
        assert!(frame.highlights[0].contains(&(8..9)), "left hl: {:?}", frame.highlights[0]);
        let off = left_w as usize + 1;
        assert!(frame.highlights[0].iter().any(|r| *r == ((off+8)..(off+9))), "right hl: {:?}", frame.highlights[0]);
    }
}
