use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// How the renderer treats escape sequences in input bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnsiMode {
    /// Pre-0.18 default. ESC renders as `^[` caret form; CSI bytes show as
    /// `^[` + literal text. Used when `--no-color` is set.
    #[default]
    Strict,
    /// Default at app level. SGR sequences update cell styles (zero columns
    /// consumed); non-SGR CSI is parsed and discarded silently; OSC 8 wraps
    /// hyperlinks.
    Interpret,
    /// `-r` / `--raw-control-chars`. Identical to Strict in the render
    /// kernel — the writer handles raw passthrough.
    Raw,
}

/// Per-source rendering state that persists across line renders. Carries the
/// SGR style register and the current OSC 8 hyperlink so that an unclosed
/// `\x1b[31m` on line N keeps line N+1 red until reset.
#[derive(Debug, Default, Clone)]
pub struct RenderState {
    pub style: crate::ansi::Style,
    pub hyperlink: Option<String>,
    pub parse: crate::ansi::ParseState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Char {
        ch: char,
        width: u8,
        style: crate::ansi::Style,
        hyperlink: Option<Arc<str>>,
    },
    Continuation,
    Empty,
}

#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub tab_width: u8,
    pub wrap: bool,
    pub cols: u16,
    pub mode: AnsiMode,
    /// In chop mode, when a line overflows the right edge, replace the
    /// last cell with this character to signal "more content right".
    /// `None` disables the marker. Matches less's `--rscroll=c`.
    pub rscroll_char: Option<char>,
    /// In wrap mode, break lines on whitespace boundaries instead of
    /// mid-character when possible. Falls back to mid-character break
    /// when no whitespace fits in the row. Matches less's `--wordwrap`.
    pub word_wrap: bool,
    /// Horizontal scroll offset in display columns. Only honored in chop mode
    /// (`wrap == false`); the first `left_col` columns of each line are skipped
    /// before emitting up to `cols` cells. Ignored in wrap mode. Default 0.
    pub left_col: usize,
    /// Chop-mode frozen left columns (`--header ,C`). When `> 0`, chop mode, and
    /// `left_col > 0`, the first `frozen_cols` display columns are pinned and the
    /// rest of the line scrolls past a dim `│` divider. 0 disables. See render_line.
    pub frozen_cols: usize,
    /// Explicit tab-stop columns (sorted, ascending, from `--tabs`). When
    /// `Some`, overrides the uniform `tab_width`: tabs advance to the next
    /// listed column; past the final stop the last interval repeats. `None`
    /// uses uniform `tab_width` spacing.
    pub tab_stops: Option<Vec<usize>>,
    /// Input charset. UTF-8 (default) keeps the grapheme-cluster + `<HH>`
    /// path; any other encoding decodes the line first (see render_line).
    pub encoding: crate::charset::Encoding,
}

impl Default for RenderOpts {
    fn default() -> Self {
        Self {
            tab_width: 8, wrap: true, cols: 80,
            mode: AnsiMode::Strict, rscroll_char: None, word_wrap: false,
            left_col: 0,
            frozen_cols: 0,
            tab_stops: None,
            encoding: crate::charset::Encoding::utf8(),
        }
    }
}

/// Next tab stop strictly greater than `col`, honoring explicit `tab_stops`
/// (with last-interval repetition past the final stop) or uniform `width`.
pub fn next_tab_stop(col: usize, width: usize, tab_stops: &Option<Vec<usize>>) -> usize {
    let w = width.max(1);
    match tab_stops {
        None => ((col / w) + 1) * w,
        Some(stops) if stops.is_empty() => ((col / w) + 1) * w,
        Some(stops) => {
            if let Some(&s) = stops.iter().find(|&&s| s > col) {
                return s;
            }
            let last = *stops.last().unwrap();
            let interval = if stops.len() >= 2 { last - stops[stops.len() - 2] } else { last.max(1) };
            last + (((col - last) / interval) + 1) * interval
        }
    }
}

/// Whether the writer should pass 24-bit RGB colors through to the terminal
/// or downsample to the 256-color cube first. Resolved once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrueColor {
    Always,
    Never,
    /// Inspect `$COLORTERM` to decide.
    #[default]
    Auto,
}

impl TrueColor {
    /// Resolve this mode to a concrete pass-through flag. `Auto` looks at
    /// the `COLORTERM` env var and treats values `truecolor` / `24bit` as
    /// supporting truecolor.
    pub fn resolve(self) -> bool {
        match self {
            TrueColor::Always => true,
            TrueColor::Never => false,
            TrueColor::Auto => matches!(
                std::env::var("COLORTERM").ok().as_deref(),
                Some("truecolor") | Some("24bit"),
            ),
        }
    }
}

/// Downsample 24-bit RGB to the xterm 256-color palette. Uses the standard
/// 6×6×6 cube plus the 24-step grayscale ramp.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    if r == g && g == b {
        if r < 8 { return 16; }
        if r > 248 { return 231; }
        return 232 + ((r as u16 - 8) * 24 / 240) as u8;
    }
    let q = |c: u8| -> u8 {
        if c < 48 { 0 }
        else if c < 115 { 1 }
        else { ((c as u16 - 35) / 40) as u8 }
    };
    16 + 36 * q(r) + 6 * q(g) + q(b)
}

/// Inverse of the relevant range of `rgb_to_256`: the RGB triple for an xterm
/// 256-color index in the 6×6×6 cube (16..=231) or the grayscale ramp
/// (232..=255). Indices 0..=15 (terminal-defined) are not produced by
/// `rgb_to_256`; we map them to black defensively.
pub fn color_256_to_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        16..=231 => {
            let i = idx as u32 - 16;
            let levels = [0u8, 95, 135, 175, 215, 255];
            let r = levels[(i / 36) as usize];
            let g = levels[((i / 6) % 6) as usize];
            let b = levels[(i % 6) as usize];
            (r, g, b)
        }
        232..=255 => {
            let v = 8 + (idx as u32 - 232) * 10;
            (v as u8, v as u8, v as u8)
        }
        _ => (0, 0, 0),
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

/// In `AnsiMode::Interpret`, pre-filter the raw byte stream through the ANSI
/// parser and return a list of `(byte, style_at_byte, hyperlink_at_byte)` for
/// printable bytes only. ESC sequences consume bytes but produce no entries.
///
/// In `AnsiMode::Strict` / `AnsiMode::Raw`, every byte is printable (no
/// pre-filtering). Style is default and hyperlink is None for all entries.
fn prefilter(
    bytes: &[u8],
    mode: AnsiMode,
    state: Option<&mut RenderState>,
) -> Vec<(u8, crate::ansi::Style, Option<Arc<str>>)> {
    match mode {
        AnsiMode::Strict | AnsiMode::Raw => {
            // Bypass: every byte is printable with default style. Raw passthrough
            // is handled by the writer layer, not the render kernel.
            bytes
                .iter()
                .map(|&b| (b, crate::ansi::Style::default(), None))
                .collect()
        }
        AnsiMode::Interpret => {
            use crate::ansi::ParseStep;
            // Use a temporary local state when the caller passes None.
            let mut tmp;
            let st: &mut RenderState = match state {
                Some(s) => s,
                None => {
                    tmp = RenderState::default();
                    &mut tmp
                }
            };
            let mut out = Vec::with_capacity(bytes.len());
            for &b in bytes {
                let step =
                    crate::ansi::step(&mut st.parse, &mut st.style, &mut st.hyperlink, b);
                if let ParseStep::Printable(pb) = step {
                    let hl = st.hyperlink.as_deref().map(Arc::from);
                    out.push((pb, st.style, hl));
                }
            }
            out
        }
    }
}

pub fn render_line(
    bytes: &[u8],
    opts: &RenderOpts,
    state: Option<&mut RenderState>,
) -> Vec<Vec<Cell>> {
    // Frozen left columns (`--header ,C`): when engaged (chop mode, positive
    // frozen_cols, scrolled, and wide enough to hold the frozen region + a
    // divider), render the frozen prefix and the scrolled remainder via the
    // existing machinery (recursively, with frozen_cols=0) and stitch them with
    // a dim `│` divider. One row (chop mode is always one row).
    if !opts.wrap
        && opts.frozen_cols > 0
        && opts.left_col > 0
        && (opts.cols as usize) > opts.frozen_cols + 1
    {
        // Frozen prefix: render ONE extra column (frozen_cols + 1), no scroll, no
        // rscroll marker. The extra column lets us detect a width-2 char that
        // straddles the frozen boundary — its trailing `Continuation` lands at
        // index `frozen_cols`. Without the extra column, chop mode would drop the
        // straddling wide char and slide the next narrow char into the gutter.
        let frozen_opts = RenderOpts {
            left_col: 0,
            cols: opts.frozen_cols as u16 + 1,
            frozen_cols: 0,
            rscroll_char: None,
            ..opts.clone()
        };
        // Scrolled remainder: skip the frozen region AND the scroll offset,
        // into the width left after the frozen region + divider.
        let scrolled_opts = RenderOpts {
            left_col: opts.frozen_cols + opts.left_col,
            cols: opts.cols - opts.frozen_cols as u16 - 1,
            frozen_cols: 0,
            ..opts.clone()
        };
        // The frozen pass must not advance the caller's SGR state twice; clone
        // for it and let the scrolled pass (same bytes) leave `state` correct.
        let mut frozen_state = state.as_deref().cloned();
        let frozen_rows = render_line(bytes, &frozen_opts, frozen_state.as_mut());
        let scrolled_rows = render_line(bytes, &scrolled_opts, state);

        let mut row = frozen_rows.into_iter().next().unwrap_or_default();
        // A width-2 char straddling the boundary leaves a `Continuation` at index
        // frozen_cols; its `Char` sits at the last frozen column. Rendering that
        // half alone would misalign (width-2 glyph in a 1-col slot), so blank it —
        // the straddling glyph is dropped at the edge (matching the left-edge
        // straddle behavior), not slid in.
        let straddles = matches!(row.get(opts.frozen_cols), Some(Cell::Continuation));
        row.truncate(opts.frozen_cols);
        if straddles {
            if let Some(last) = row.last_mut() {
                *last = Cell::Empty;
            }
        }
        while row.len() < opts.frozen_cols {
            row.push(Cell::Empty);
        }
        row.push(Cell::Char {
            ch: '\u{2502}',
            width: 1,
            style: crate::ansi::Style { dim: true, ..Default::default() },
            hyperlink: None,
        });
        if let Some(scrolled) = scrolled_rows.into_iter().next() {
            row.extend(scrolled);
        }
        while row.len() < opts.cols as usize {
            row.push(Cell::Empty);
        }
        return vec![row];
    }

    // Charset decode: for non-UTF-8 encodings, transcode the line to UTF-8 up
    // front so the existing grapheme/width/tab/control pipeline renders the
    // decoded glyphs. UTF-8 keeps the original bytes (and its <HH> behavior).
    // (C1 controls 0x80–0x9F become their codepoints' UTF-8 here; for v1 they
    // render as glyphs if the control branch only checks single low bytes.)
    let decoded: std::borrow::Cow<[u8]> = if opts.encoding.is_utf8() {
        std::borrow::Cow::Borrowed(bytes)
    } else {
        std::borrow::Cow::Owned(
            crate::charset::decode_line(bytes, opts.encoding).into_owned().into_bytes()
        )
    };
    let bytes = decoded.as_ref();

    let cols = opts.cols as usize;
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut current: Vec<Cell> = Vec::with_capacity(cols);

    // Pre-filter: resolve styles and strip escape sequences for Interpret mode.
    let filtered = prefilter(bytes, opts.mode, state);

    // Chop-mode horizontal scroll: skip this many leading display columns.
    let mut to_skip = if opts.wrap { 0 } else { opts.left_col };

    /// Returns true if the cell was dropped due to chop-mode overflow.
    /// The caller uses this to decide whether to paint the `rscroll` marker.
    fn push(current: &mut Vec<Cell>, rows: &mut Vec<Vec<Cell>>, cell: Cell, opts: &RenderOpts, to_skip: &mut usize) -> bool {
        if *to_skip > 0 {
            *to_skip -= 1;   // this column scrolled off the left edge
            return false;
        }
        if current.len() >= opts.cols as usize {
            if opts.wrap {
                let mut full = std::mem::replace(current, Vec::with_capacity(opts.cols as usize));
                // `--wordwrap`: prefer a break on the last whitespace cell.
                // Anything past the break carries over to the next row as
                // its leading content. Falls back to mid-character break
                // when no whitespace is found.
                if opts.word_wrap {
                    if let Some(ws_idx) = (0..full.len()).rev().find(|&i| matches!(
                        full[i],
                        Cell::Char { ch, .. } if ch == ' ' || ch == '\t'
                    )) {
                        // Carry everything after the whitespace into the new
                        // current row (so the next word starts at column 0).
                        let carry: Vec<Cell> = full.drain((ws_idx + 1)..).collect();
                        *current = carry;
                    }
                }
                while full.len() < opts.cols as usize { full.push(Cell::Empty); }
                rows.push(full);
            } else {
                return true;
            }
        }
        current.push(cell);
        false
    }

    fn push_str(
        current: &mut Vec<Cell>,
        rows: &mut Vec<Vec<Cell>>,
        s: &str,
        style: crate::ansi::Style,
        hyperlink: Option<Arc<str>>,
        opts: &RenderOpts,
        to_skip: &mut usize,
    ) -> bool {
        let mut overflowed = false;
        for c in s.chars() {
            overflowed |= push(
                current,
                rows,
                Cell::Char { ch: c, width: 1, style, hyperlink: hyperlink.clone() },
                opts,
                to_skip,
            );
        }
        overflowed
    }

    #[allow(clippy::too_many_arguments)]
    fn push_wide(
        current: &mut Vec<Cell>,
        rows: &mut Vec<Vec<Cell>>,
        ch: char,
        width: u8,
        style: crate::ansi::Style,
        hyperlink: Option<Arc<str>>,
        opts: &RenderOpts,
        to_skip: &mut usize,
    ) -> bool {
        let cols = opts.cols as usize;
        let w = width as usize;
        if *to_skip >= w {
            *to_skip -= w;   // wholly off the left edge
            return false;
        }
        if *to_skip > 0 {
            // straddles the left edge: emit a blank for each visible half-column
            let visible = w - *to_skip;
            *to_skip = 0;
            let mut of = false;
            for _ in 0..visible {
                of |= push(current, rows, Cell::Char { ch: ' ', width: 1, style, hyperlink: hyperlink.clone() }, opts, to_skip);
            }
            return of;
        }
        // If the wide char wouldn't fit in the remainder of this row, wrap first.
        if current.len() + w > cols {
            if opts.wrap {
                let mut full = std::mem::replace(current, Vec::with_capacity(cols));
                // `--wordwrap`: prefer a break on the last whitespace. Same
                // logic as in `push`; kept duplicated rather than factored
                // out because the two helpers track `current.len()` slightly
                // differently and the inline form is easier to follow.
                if opts.word_wrap {
                    if let Some(ws_idx) = (0..full.len()).rev().find(|&i| matches!(
                        full[i],
                        Cell::Char { ch, .. } if ch == ' ' || ch == '\t'
                    )) {
                        let carry: Vec<Cell> = full.drain((ws_idx + 1)..).collect();
                        *current = carry;
                    }
                }
                while full.len() < cols { full.push(Cell::Empty); }
                rows.push(full);
            } else {
                return true; // chop overflow
            }
        }
        current.push(Cell::Char { ch, width, style, hyperlink });
        for _ in 1..width {
            current.push(Cell::Continuation);
        }
        false
    }

    // Walk filtered bytes (raw bytes for Strict, printable-only for Interpret).
    // Track chop-mode overflow so we can paint the rscroll marker afterward.
    let mut overflowed = false;
    let mut i = 0;
    while i < filtered.len() {
        let (b, style, hyperlink) = filtered[i].clone();
        if b == b'\t' {
            // Tab stop calculation must account for already-skipped columns.
            // `current.len()` only tracks emitted cells, not skipped ones, so
            // we add `opts.left_col - to_skip` (columns already consumed/skipped)
            // to get the true logical column position for tab-stop math.
            let skipped_so_far = if opts.wrap { 0 } else { opts.left_col - to_skip };
            let cur_col = current.len() + skipped_so_far;
            let next_stop = next_tab_stop(cur_col, opts.tab_width as usize, &opts.tab_stops);
            // Emit spaces from logical cur_col up to next_stop.
            for _ in cur_col..next_stop {
                overflowed |= push(
                    &mut current,
                    &mut rows,
                    Cell::Char { ch: ' ', width: 1, style, hyperlink: hyperlink.clone() },
                    opts,
                    &mut to_skip,
                );
            }
            i += 1;
        } else if b == b'\n' {
            i += 1;
        } else if b < 0x20 || b == 0x7F {
            let printable = if b == 0x7F { '?' } else { (b ^ 0x40) as char };
            overflowed |= push(
                &mut current,
                &mut rows,
                Cell::Char { ch: '^', width: 1, style, hyperlink: hyperlink.clone() },
                opts,
                &mut to_skip,
            );
            overflowed |= push(
                &mut current,
                &mut rows,
                Cell::Char { ch: printable, width: 1, style, hyperlink },
                opts,
                &mut to_skip,
            );
            i += 1;
        } else {
            // Try to decode a UTF-8 grapheme cluster. We reconstruct raw bytes
            // from the filtered stream for cluster decoding.
            let raw_bytes: Vec<u8> = filtered[i..].iter().map(|(b, _, _)| *b).collect();
            match decode_cluster(&raw_bytes, 0) {
                Some((cluster, consumed)) => {
                    let w = UnicodeWidthStr::width(cluster) as u8;
                    let base_char = cluster.chars().next().unwrap_or('\u{FFFD}');
                    if w == 0 {
                        // Lone combining mark with no base — emit replacement.
                        overflowed |= push(
                            &mut current,
                            &mut rows,
                            Cell::Char {
                                ch: '\u{FFFD}',
                                width: 1,
                                style,
                                hyperlink,
                            },
                            opts,
                            &mut to_skip,
                        );
                    } else {
                        overflowed |= push_wide(&mut current, &mut rows, base_char, w, style, hyperlink, opts, &mut to_skip);
                    }
                    i += consumed;
                }
                None => {
                    // Invalid byte: emit <HH>, advance one byte.
                    let s = format!("<{:02X}>", b);
                    overflowed |= push_str(&mut current, &mut rows, &s, style, hyperlink, opts, &mut to_skip);
                    i += 1;
                }
            }
        }
    }

    while current.len() < cols {
        current.push(Cell::Empty);
    }

    // `--rscroll`: in chop mode, when the line overflowed the right edge,
    // replace the last cell with the marker char (styled dim) so the user
    // can see that content was truncated.
    if !opts.wrap && overflowed && cols > 0 {
        if let Some(marker) = opts.rscroll_char {
            current[cols - 1] = Cell::Char {
                ch: marker,
                width: 1,
                style: crate::ansi::Style { dim: true, ..Default::default() },
                hyperlink: None,
            };
        }
    }

    rows.push(current);
    rows
}

/// Full expanded display width of a line in columns (tabs expanded to tab
/// stops, cluster widths summed). Used by the viewport to clamp horizontal
/// scroll. Independent of `cols`/`left_col`.
pub fn display_width(bytes: &[u8], opts: &RenderOpts) -> usize {
    let filtered = prefilter(bytes, opts.mode, None);
    let mut col = 0usize;
    let mut i = 0;
    while i < filtered.len() {
        let (b, _, _) = &filtered[i];
        if *b == b'\t' {
            col = next_tab_stop(col, opts.tab_width as usize, &opts.tab_stops);
            i += 1;
            continue;
        }
        if *b == b'\n' {
            i += 1;
            continue;
        }
        if *b < 0x20 || *b == 0x7F {
            // Control byte renders as ^X (2 columns)
            col += 2;
            i += 1;
            continue;
        }
        let raw_bytes: Vec<u8> = filtered[i..].iter().map(|(b, _, _)| *b).collect();
        match decode_cluster(&raw_bytes, 0) {
            Some((cluster, consumed)) => {
                let w = UnicodeWidthStr::width(cluster);
                col += if w == 0 { 1 } else { w }; // zero-width → replacement char = 1
                i += consumed;
            }
            None => {
                // Invalid byte: <HH> = 4 columns
                col += 4;
                i += 1;
            }
        }
    }
    col
}

pub fn count_rows(
    bytes: &[u8],
    opts: &RenderOpts,
    state: Option<&mut RenderState>,
) -> usize {
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

    // Pre-filter: only printable bytes contribute to column count.
    let filtered = prefilter(bytes, opts.mode, state);

    let mut i = 0;
    while i < filtered.len() {
        let (b, _, _) = filtered[i];
        if b == b'\t' {
            let next_stop = next_tab_stop(col, opts.tab_width as usize, &opts.tab_stops);
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
            let raw_bytes: Vec<u8> = filtered[i..].iter().map(|(b, _, _)| *b).collect();
            match decode_cluster(&raw_bytes, 0) {
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

    fn cell_char(c: &Cell) -> char {
        match c {
            Cell::Char { ch, .. } => *ch,
            _ => ' ',
        }
    }

    #[test]
    fn explicit_tab_stops_list() {
        let o = RenderOpts { wrap: false, cols: 40, tab_width: 8,
            tab_stops: Some(vec![4, 8]), ..Default::default() };
        let rows = render_line(b"a\tb\tc", &o, None);
        let text: String = rows[0].iter().take(9).map(cell_char).collect();
        assert_eq!(text, "a   b   c");
    }

    #[test]
    fn tab_stops_repeat_last_interval_past_final_stop() {
        let o = RenderOpts { wrap: false, cols: 40, tab_width: 8,
            tab_stops: Some(vec![4, 8]), ..Default::default() };
        let rows = render_line(b"abcdefghi\tx", &o, None); // 'x' lands at col 12
        let text: String = rows[0].iter().take(13).map(cell_char).collect();
        assert_eq!(text, "abcdefghi   x");
    }

    #[test]
    fn single_value_tab_stops_matches_uniform() {
        let list = RenderOpts { wrap: false, cols: 40, tab_width: 8,
            tab_stops: Some(vec![4]), ..Default::default() };
        let uniform = RenderOpts { wrap: false, cols: 40, tab_width: 4, ..Default::default() };
        assert_eq!(render_line(b"a\tb", &list, None), render_line(b"a\tb", &uniform, None));
    }

    #[test]
    fn rgb_to_256_pure_corners_map_to_palette_extremes() {
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
    }

    #[test]
    fn color_256_to_rgb_inverts_cube_and_gray() {
        assert_eq!(color_256_to_rgb(16), (0, 0, 0));
        assert_eq!(color_256_to_rgb(231), (255, 255, 255));
        let (r, g, b) = color_256_to_rgb(232);
        assert_eq!((r, g, b), (8, 8, 8));
        // Round-trip: any cube color re-quantizes to the same index.
        let idx = rgb_to_256(0, 128, 255);
        let (r, g, b) = color_256_to_rgb(idx);
        assert_eq!(rgb_to_256(r, g, b), idx, "palette RGB re-quantizes to the same index");
    }

    #[test]
    fn rgb_to_256_mid_gray_lands_in_grayscale_ramp() {
        let n = rgb_to_256(128, 128, 128);
        assert!((232..=255).contains(&n), "expected grayscale slot 232..=255, got {n}");
    }

    #[test]
    fn rgb_to_256_pure_rgb_lands_in_cube_extremes() {
        assert_eq!(rgb_to_256(255, 0, 0), 196);
        assert_eq!(rgb_to_256(0, 255, 0), 46);
        assert_eq!(rgb_to_256(0, 0, 255), 21);
    }

    #[test]
    fn rgb_to_256_low_channel_quantizes_to_zero() {
        // 256-cube index = 16 + 36*r6 + 6*g6 + b6, here r6=0 g6=4 b6=0 -> 40.
        assert_eq!(rgb_to_256(40, 200, 0), 40);
    }

    #[test]
    fn rgb_to_256_near_black_gray_is_palette_black() {
        assert_eq!(rgb_to_256(5, 5, 5), 16);
    }

    #[test]
    fn rgb_to_256_near_white_gray_is_palette_white() {
        assert_eq!(rgb_to_256(250, 250, 250), 231);
    }

    #[test]
    fn truecolor_always_resolves_true_regardless_of_env() {
        assert!(TrueColor::Always.resolve());
    }

    #[test]
    fn truecolor_never_resolves_false_regardless_of_env() {
        assert!(!TrueColor::Never.resolve());
    }

    #[test]
    fn rscroll_marker_appears_on_chopped_row() {
        let mut o = opts(5, false); // 5 cols, chop mode
        o.rscroll_char = Some('>');
        let rows = render_line(b"abcdefgh", &o, None);
        assert_eq!(rows.len(), 1);
        match &rows[0][4] {
            Cell::Char { ch, .. } => assert_eq!(*ch, '>'),
            other => panic!("expected `>` marker, got {other:?}"),
        }
    }

    #[test]
    fn rscroll_marker_absent_on_fitting_row() {
        let mut o = opts(10, false);
        o.rscroll_char = Some('>');
        let rows = render_line(b"abc", &o, None);
        match &rows[0][2] {
            Cell::Char { ch, .. } => assert_eq!(*ch, 'c'),
            other => panic!("expected content `c`, got {other:?}"),
        }
    }

    #[test]
    fn rscroll_marker_disabled_emits_normal_chop() {
        let mut o = opts(5, false);
        o.rscroll_char = None;
        let rows = render_line(b"abcdefgh", &o, None);
        match &rows[0][4] {
            Cell::Char { ch, .. } => assert_eq!(*ch, 'e'),
            other => panic!("expected last fitting char, got {other:?}"),
        }
    }

    #[test]
    fn word_wrap_breaks_on_whitespace() {
        let mut o = opts(8, true);
        o.word_wrap = true;
        let rows = render_line(b"the quick brown fox", &o, None);
        // First row should break at the last whitespace before col 8.
        let r0: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        }).collect();
        assert_eq!(r0.trim_end(), "the");
    }

    #[test]
    fn word_wrap_falls_back_when_no_whitespace_fits() {
        let mut o = opts(5, true);
        o.word_wrap = true;
        let rows = render_line(b"antidisestablishment", &o, None);
        let r0: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        }).collect();
        // No whitespace anywhere → mid-character break preserved.
        assert_eq!(r0.trim_end(), "antid");
    }

    #[test]
    fn word_wrap_off_breaks_mid_word() {
        let mut o = opts(8, true);
        o.word_wrap = false;
        let rows = render_line(b"the quick brown fox", &o, None);
        let r0: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        }).collect();
        // First 8 chars verbatim: "the quic"
        assert_eq!(r0.trim_end(), "the quic");
    }

    #[test]
    fn rscroll_marker_absent_in_wrap_mode() {
        let mut o = opts(5, true);
        o.rscroll_char = Some('>');
        let rows = render_line(b"abcdefgh", &o, None);
        // Wrap mode produces multiple rows; rscroll only fires in chop.
        assert!(rows.len() > 1);
        for row in &rows {
            for cell in row {
                if let Cell::Char { ch, .. } = cell {
                    assert_ne!(*ch, '>', "rscroll marker leaked into wrap mode");
                }
            }
        }
    }

    fn opts(cols: u16, wrap: bool) -> RenderOpts {
        RenderOpts { tab_width: 8, wrap, cols, mode: AnsiMode::Strict, rscroll_char: None, word_wrap: false, left_col: 0, frozen_cols: 0, tab_stops: None, encoding: crate::charset::Encoding::utf8() }
    }

    fn ch(c: char) -> Cell {
        Cell::Char { ch: c, width: 1, style: crate::ansi::Style::default(), hyperlink: None }
    }

    #[test]
    fn ascii_short_line_pads_to_cols() {
        let rows = render_line(b"hi", &opts(5, true), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![ch('h'), ch('i'), Cell::Empty, Cell::Empty, Cell::Empty]);
    }

    #[test]
    fn ascii_exact_width() {
        let rows = render_line(b"hello", &opts(5, true), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![ch('h'), ch('e'), ch('l'), ch('l'), ch('o')]);
    }

    #[test]
    fn empty_input_yields_one_empty_row() {
        let rows = render_line(b"", &opts(3, true), None);
        assert_eq!(rows, vec![vec![Cell::Empty, Cell::Empty, Cell::Empty]]);
    }

    #[test]
    fn tab_at_col_zero_expands_to_eight() {
        let rows = render_line(b"\tx", &opts(20, true), None);
        // Eight spaces, then 'x', then padding.
        for (i, cell) in rows[0].iter().take(8).enumerate() {
            assert_eq!(*cell, ch(' '), "col {i} should be space");
        }
        assert_eq!(rows[0][8], ch('x'));
    }

    #[test]
    fn tab_at_col_three_advances_to_next_stop() {
        // "abc\tx" → cols 0,1,2 = a,b,c; tab fills to col 8 with spaces; col 8 = x
        let rows = render_line(b"abc\tx", &opts(20, true), None);
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
        let rows = render_line(&input, &opts(20, true), None);
        for cell in rows[0].iter().skip(8).take(8) {
            assert_eq!(*cell, ch(' '));
        }
        assert_eq!(rows[0][16], ch('x'));
    }

    #[test]
    fn null_renders_as_caret_at() {
        let rows = render_line(b"\0", &opts(5, true), None);
        assert_eq!(rows[0][0], ch('^'));
        assert_eq!(rows[0][1], ch('@'));
    }

    #[test]
    fn esc_renders_as_caret_lbracket() {
        let rows = render_line(b"\x1b", &opts(5, true), None);
        assert_eq!(rows[0][0], ch('^'));
        assert_eq!(rows[0][1], ch('['));
    }

    #[test]
    fn del_renders_as_caret_question() {
        let rows = render_line(b"\x7f", &opts(5, true), None);
        assert_eq!(rows[0][0], ch('^'));
        assert_eq!(rows[0][1], ch('?'));
    }

    #[test]
    fn invalid_utf8_byte_renders_as_angle_hex() {
        let rows = render_line(&[0xFF], &opts(8, true), None);
        assert_eq!(rows[0][0], ch('<'));
        assert_eq!(rows[0][1], ch('F'));
        assert_eq!(rows[0][2], ch('F'));
        assert_eq!(rows[0][3], ch('>'));
    }

    #[test]
    fn latin1_high_byte_renders_as_glyph() {
        let mut opts = RenderOpts::default();
        opts.encoding = crate::charset::parse_label("iso-8859-1").unwrap();
        opts.cols = 10;
        let rows = render_line(&[0x63, 0x61, 0x66, 0xE9], &opts, None); // "caf" + 0xE9
        let text: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        }).collect();
        assert!(text.starts_with("café"), "got: {text:?}");
        assert!(!text.contains('<'), "should not show <E9>");
    }

    #[test]
    fn utf8_default_path_unchanged_for_invalid_byte() {
        let opts = RenderOpts::default(); // encoding defaults to utf-8
        let rows = render_line(&[0xC3], &opts, None); // lone invalid byte
        let text: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        }).collect();
        assert!(text.contains("<C3>"), "utf-8 path must keep <HH>; got {text:?}");
    }

    #[test]
    fn partial_multibyte_each_byte_renders_separately() {
        // 0xC3 starts a 2-byte sequence; alone it's invalid → <C3>
        let rows = render_line(&[0xC3], &opts(8, true), None);
        assert_eq!(rows[0][0], ch('<'));
        assert_eq!(rows[0][1], ch('C'));
        assert_eq!(rows[0][2], ch('3'));
        assert_eq!(rows[0][3], ch('>'));
    }

    #[test]
    fn single_byte_utf8_e_acute() {
        let rows = render_line("é".as_bytes(), &opts(5, true), None);
        assert_eq!(
            rows[0][0],
            Cell::Char { ch: 'é', width: 1, style: crate::ansi::Style::default(), hyperlink: None }
        );
    }

    #[test]
    fn cjk_char_takes_two_columns() {
        // 日 is width 2.
        let rows = render_line("日".as_bytes(), &opts(5, true), None);
        assert_eq!(
            rows[0][0],
            Cell::Char { ch: '日', width: 2, style: crate::ansi::Style::default(), hyperlink: None }
        );
        assert_eq!(rows[0][1], Cell::Continuation);
        assert_eq!(rows[0][2], Cell::Empty);
    }

    #[test]
    fn emoji_takes_two_columns() {
        let rows = render_line("🦀".as_bytes(), &opts(5, true), None);
        // Width depends on unicode-width; crab emoji is width 2.
        assert!(matches!(rows[0][0], Cell::Char { width: 2, .. }));
        assert_eq!(rows[0][1], Cell::Continuation);
    }

    #[test]
    fn combining_mark_folds_into_prior_cell() {
        // "e\u{0301}" is one grapheme cluster (e with combining acute).
        let rows = render_line("e\u{0301}".as_bytes(), &opts(5, true), None);
        // Cluster renders as a single cell carrying base char.
        assert!(matches!(rows[0][0], Cell::Char { width: 1, .. }));
        assert_eq!(rows[0][1], Cell::Empty);
    }

    #[test]
    fn wrap_long_line_into_multiple_rows() {
        let rows = render_line(b"abcdefghij", &opts(4, true), None);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], vec![ch('a'), ch('b'), ch('c'), ch('d')]);
        assert_eq!(rows[1], vec![ch('e'), ch('f'), ch('g'), ch('h')]);
        assert_eq!(rows[2], vec![ch('i'), ch('j'), Cell::Empty, Cell::Empty]);
    }

    #[test]
    fn chop_long_line_truncates() {
        let rows = render_line(b"abcdefghij", &opts(4, false), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0], vec![ch('a'), ch('b'), ch('c'), ch('d')]);
    }

    #[test]
    fn wide_char_at_boundary_pushed_to_next_row() {
        // cols=3, content "ab日" — 日 is width 2, doesn't fit at col 2,
        // so row 0 = a, b, Empty; row 1 = 日(continuation), Empty.
        let rows = render_line("ab日".as_bytes(), &opts(3, true), None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec![ch('a'), ch('b'), Cell::Empty]);
        assert_eq!(
            rows[1][0],
            Cell::Char { ch: '日', width: 2, style: crate::ansi::Style::default(), hyperlink: None }
        );
        assert_eq!(rows[1][1], Cell::Continuation);
        assert_eq!(rows[1][2], Cell::Empty);
    }

    #[test]
    fn count_rows_matches_render_line_for_short() {
        let o = opts(80, true);
        let bytes = b"hello world";
        assert_eq!(count_rows(bytes, &o, None), render_line(bytes, &o, None).len());
    }

    #[test]
    fn count_rows_matches_render_line_for_long_wrap() {
        let o = opts(4, true);
        let bytes = b"abcdefghij";
        assert_eq!(count_rows(bytes, &o, None), render_line(bytes, &o, None).len());
    }

    #[test]
    fn count_rows_chop_is_one() {
        let o = opts(4, false);
        let bytes = b"abcdefghij";
        assert_eq!(count_rows(bytes, &o, None), 1);
    }

    #[test]
    fn count_rows_handles_wide_char() {
        let o = opts(3, true);
        let bytes = "ab日".as_bytes();
        assert_eq!(count_rows(bytes, &o, None), render_line(bytes, &o, None).len());
    }

    // ---- Interpret-mode tests ----

    fn interpret_opts() -> RenderOpts {
        RenderOpts { mode: AnsiMode::Interpret, ..Default::default() }
    }

    #[test]
    fn interpret_red_text() {
        let mut state = RenderState::default();
        let rows = render_line(b"\x1b[31mhi", &interpret_opts(), Some(&mut state));
        let cells: Vec<&Cell> =
            rows.iter().flatten().filter(|c| matches!(c, Cell::Char { .. })).collect();
        assert_eq!(cells.len(), 2);
        for c in cells {
            if let Cell::Char { style, .. } = c {
                assert_eq!(style.fg, Some(crate::ansi::Color::Ansi(1)));
            }
        }
    }

    #[test]
    fn interpret_truecolor() {
        let mut state = RenderState::default();
        let rows =
            render_line(b"\x1b[38;2;255;0;0mfoo", &interpret_opts(), Some(&mut state));
        let cells: Vec<&Cell> =
            rows.iter().flatten().filter(|c| matches!(c, Cell::Char { .. })).collect();
        for c in cells {
            if let Cell::Char { style, .. } = c {
                assert_eq!(style.fg, Some(crate::ansi::Color::Rgb(255, 0, 0)));
            }
        }
    }

    #[test]
    fn interpret_wide_char_carries_color() {
        let mut state = RenderState::default();
        let rows =
            render_line("\x1b[31m日".as_bytes(), &interpret_opts(), Some(&mut state));
        let jp_cell = rows.iter().flatten().find_map(|c| match c {
            Cell::Char { ch: '日', style, width, .. } => Some((style, *width)),
            _ => None,
        });
        let (style, width) = jp_cell.expect("expected 日 cell");
        assert_eq!(style.fg, Some(crate::ansi::Color::Ansi(1)));
        assert_eq!(width, 2);
    }

    #[test]
    fn interpret_state_persists_across_calls() {
        let mut state = RenderState::default();
        let _ = render_line(b"\x1b[31mline1", &interpret_opts(), Some(&mut state));
        let rows = render_line(b"line2", &interpret_opts(), Some(&mut state));
        let l_cell = rows.iter().flatten().find_map(|c| match c {
            Cell::Char { ch: 'l', style, .. } => Some(style),
            _ => None,
        });
        assert_eq!(
            l_cell.expect("expected l cell").fg,
            Some(crate::ansi::Color::Ansi(1))
        );
    }

    #[test]
    fn interpret_reset_clears_state() {
        let mut state = RenderState::default();
        let _ =
            render_line(b"\x1b[31mline1\x1b[0m", &interpret_opts(), Some(&mut state));
        let rows = render_line(b"line2", &interpret_opts(), Some(&mut state));
        let l_cell = rows.iter().flatten().find_map(|c| match c {
            Cell::Char { ch: 'l', style, .. } => Some(style),
            _ => None,
        });
        assert_eq!(l_cell.expect("expected l cell"), &crate::ansi::Style::default());
    }

    #[test]
    fn interpret_non_sgr_csi_is_zero_width() {
        let mut state = RenderState::default();
        let rows = render_line(b"\x1b[2Jdata", &interpret_opts(), Some(&mut state));
        let chars: String = rows
            .iter()
            .flatten()
            .filter_map(|c| match c {
                Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            })
            .collect();
        assert_eq!(chars, "data");
    }

    #[test]
    fn strict_mode_esc_still_renders_as_caret_lbracket() {
        // LOCKDOWN: pre-0.18 behavior must survive.
        let rows = render_line(b"\x1b[31mhi", &RenderOpts::default(), None);
        let chars: String = rows
            .iter()
            .flatten()
            .filter_map(|c| match c {
                Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            })
            .collect();
        assert!(chars.starts_with("^["), "got: {chars:?}");
    }

    #[test]
    fn osc8_hyperlink_attached_to_cells() {
        let mut state = RenderState::default();
        let rows = render_line(
            b"\x1b]8;;https://example.com\x07click\x1b]8;;\x07",
            &interpret_opts(),
            Some(&mut state),
        );
        let click_cell = rows.iter().flatten().find_map(|c| match c {
            Cell::Char { ch: 'c', hyperlink, .. } => Some(hyperlink.clone()),
            _ => None,
        });
        let link = click_cell.expect("expected c cell").expect("expected hyperlink");
        assert_eq!(link.as_ref(), "https://example.com");
    }

    #[test]
    fn left_col_skips_leading_columns_in_chop() {
        let opts = RenderOpts { wrap: false, cols: 4, left_col: 3, ..Default::default() };
        let rows = render_line(b"abcdefgh", &opts, None);
        assert_eq!(rows.len(), 1);
        let s: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch), _ => None }).collect();
        assert_eq!(s, "defg");
    }

    #[test]
    fn left_col_zero_is_unchanged() {
        let opts = RenderOpts { wrap: false, cols: 4, left_col: 0, ..Default::default() };
        let rows = render_line(b"abcdefgh", &opts, None);
        let s: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch), _ => None }).collect();
        assert_eq!(s, "abcd");
    }

    #[test]
    fn left_col_ignored_in_wrap_mode() {
        let opts = RenderOpts { wrap: true, cols: 4, left_col: 3, ..Default::default() };
        let rows = render_line(b"abcdefgh", &opts, None);
        let first: String = rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch), _ => None }).collect();
        assert_eq!(first, "abcd");
    }

    #[test]
    fn left_col_past_end_is_blank() {
        let opts = RenderOpts { wrap: false, cols: 4, left_col: 20, ..Default::default() };
        let rows = render_line(b"abc", &opts, None);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].iter().all(|c| matches!(c, Cell::Empty)));
    }

    #[test]
    fn left_col_tab_expansion_across_boundary() {
        let opts = RenderOpts { wrap: false, cols: 4, left_col: 2, tab_width: 4, ..Default::default() };
        let rows = render_line(b"\tX", &opts, None);
        let cells = &rows[0];
        assert!(matches!(cells[0], Cell::Char { ch: ' ', .. }));
        assert!(matches!(cells[1], Cell::Char { ch: ' ', .. }));
        assert!(matches!(cells[2], Cell::Char { ch: 'X', .. }));
    }

    #[test]
    fn left_col_does_not_change_count_rows() {
        let opts = RenderOpts { wrap: false, cols: 4, left_col: 3, ..Default::default() };
        assert_eq!(count_rows(b"abcdefgh", &opts, None), 1);
    }

    fn row_chars(rows: &[Vec<Cell>]) -> String {
        rows[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch), _ => None }).collect()
    }

    #[test]
    fn frozen_cols_zero_is_unchanged() {
        let opts = RenderOpts { wrap: false, cols: 10, left_col: 3, frozen_cols: 0, ..Default::default() };
        assert_eq!(row_chars(&render_line(b"abcdefghij", &opts, None)), "defghij");
    }

    #[test]
    fn frozen_cols_no_effect_when_not_scrolled() {
        // left_col == 0 → freeze not engaged; identical to a normal render.
        let opts = RenderOpts { wrap: false, cols: 10, left_col: 0, frozen_cols: 4, ..Default::default() };
        assert_eq!(row_chars(&render_line(b"abcdefghij", &opts, None)), "abcdefghij");
    }

    #[test]
    fn frozen_cols_engaged_pins_prefix_and_divides() {
        // C=4, left_col=3, cols=10: first 4 cols frozen, then '│', then the
        // remainder skipped by frozen(4)+left_col(3)=7 → starts at 'h'.
        let opts = RenderOpts { wrap: false, cols: 10, left_col: 3, frozen_cols: 4, ..Default::default() };
        assert_eq!(row_chars(&render_line(b"abcdefghij", &opts, None)), "abcd\u{2502}hij");
    }

    #[test]
    fn frozen_cols_wide_char_straddling_boundary_is_blanked_not_slid() {
        // "a世bcdef": display cols a=0, 世=1-2 (width 2), b=3, c=4…
        // C=2 → frozen region is cols 0-1; 世 straddles (occupies 1 and 2), so it
        // must be dropped at the edge and col 1 blanked — NOT have 'b' slide in.
        // Engaged (left_col=2): scrolled skips frozen(2)+left_col(2)=4 cols → 'c'.
        // Result: 'a' + blank + '│' + "cdef" → row_chars = "a\u{2502}cdef".
        let opts = RenderOpts { wrap: false, cols: 10, left_col: 2, frozen_cols: 2, ..Default::default() };
        let rows = render_line("a世bcdef".as_bytes(), &opts, None);
        // The dropped wide char and its blank don't appear in row_chars (Empty is
        // skipped); the key assertion is 'b' did NOT slide into the frozen gutter.
        assert_eq!(row_chars(&rows), "a\u{2502}cdef");
        // Row width invariant: exactly `cols` cells.
        assert_eq!(rows[0].len(), 10);
        // Frozen col 1 is blanked (Empty), not a 'b'.
        assert!(matches!(rows[0][1], Cell::Empty),
            "straddling wide char's frozen column must be blanked, not back-filled");
    }

    #[test]
    fn frozen_cols_wide_char_fully_inside_frozen_is_kept() {
        // "世abcdef": 世 occupies cols 0-1, fully inside a C=2 frozen region.
        let opts = RenderOpts { wrap: false, cols: 10, left_col: 2, frozen_cols: 2, ..Default::default() };
        let rows = render_line("世abcdef".as_bytes(), &opts, None);
        // 世 kept in frozen; scrolled skips 2+2=4 display cols (世=2, a,b=2) → 'c'.
        assert_eq!(row_chars(&rows), "世\u{2502}cdef");
        assert_eq!(rows[0].len(), 10);
    }

    #[test]
    fn frozen_cols_too_narrow_falls_back_to_plain_hscroll() {
        // cols (5) not greater than frozen_cols+1 (5) → no freeze; plain hscroll.
        let opts = RenderOpts { wrap: false, cols: 5, left_col: 3, frozen_cols: 4, ..Default::default() };
        assert_eq!(row_chars(&render_line(b"abcdefghij", &opts, None)), "defgh");
    }

    #[test]
    fn frozen_cols_noop_in_wrap_mode() {
        // wrap mode ignores frozen_cols (hscroll is already a no-op there).
        let opts = RenderOpts { wrap: true, cols: 4, left_col: 3, frozen_cols: 2, ..Default::default() };
        assert_eq!(render_line(b"abcdefgh", &opts, None)[0].iter().filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch), _ => None }).collect::<String>(), "abcd");
    }

    #[test]
    fn display_width_counts_tabs_and_ascii() {
        let opts = RenderOpts { tab_width: 4, ..Default::default() };
        assert_eq!(display_width(b"ab", &opts), 2);
        assert_eq!(display_width(b"\tab", &opts), 6);
    }

    #[test]
    fn display_width_agrees_with_rendered_columns() {
        // A mixed ASCII + wide-char + tab line: display_width must equal the
        // number of display columns render_line lays out for it in a very wide
        // chop window (so nothing is dropped).
        let line = "a\tÅ中b".as_bytes();
        let opts = RenderOpts { wrap: false, cols: 1000, tab_width: 4, ..Default::default() };
        let rows = render_line(line, &opts, None);
        let cols_used = rows[0].iter().take_while(|c| !matches!(c, Cell::Empty)).count();
        assert_eq!(display_width(line, &opts), cols_used);
    }
}
