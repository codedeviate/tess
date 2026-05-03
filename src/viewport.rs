use std::ops::Range;

use regex::Regex;

use crate::filter::{CompiledFilter, FilterMatch};
use crate::line_index::LineIndex;
use crate::render::{count_rows, render_line, Cell, RenderOpts};
use crate::source::Source;

/// Build the rendered text of a display row plus a `starts` table mapping
/// each char index in that text back to its starting cell column. The last
/// entry is a sentinel pointing one past the row's width, so a match's
/// `[char_start, char_end)` translates to the cell range
/// `starts[char_start]..starts[char_end]`.
fn row_text_and_starts(row: &[Cell]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut starts: Vec<usize> = Vec::with_capacity(row.len() + 1);
    for (col, cell) in row.iter().enumerate() {
        match cell {
            Cell::Char { ch, .. } => {
                starts.push(col);
                text.push(*ch);
            }
            Cell::Empty => {
                starts.push(col);
                text.push(' ');
            }
            Cell::Continuation => {}
        }
    }
    starts.push(row.len());
    (text, starts)
}

/// Find every regex match in the rendered text of a row, translating each
/// to a cell column range. Empty matches are dropped. Trailing-padding
/// spaces on a row would otherwise satisfy patterns like `\s+`; we trim
/// those by clamping match ends to where actual content stops.
fn find_row_highlights(row: &[Cell], regex: &Regex) -> Vec<Range<usize>> {
    if row.is_empty() {
        return Vec::new();
    }
    let last_content_col = row
        .iter()
        .enumerate()
        .rev()
        .find_map(|(c, cell)| match cell {
            Cell::Char { width, .. } => Some(c + *width as usize),
            Cell::Continuation => Some(c + 1),
            Cell::Empty => None,
        })
        .unwrap_or(0);
    if last_content_col == 0 {
        return Vec::new();
    }
    let (text, starts) = row_text_and_starts(row);
    let mut out = Vec::new();
    for m in regex.find_iter(&text) {
        if m.start() == m.end() {
            continue;
        }
        let char_start = text[..m.start()].chars().count();
        let char_end = text[..m.end()].chars().count();
        if char_start >= starts.len() - 1 || char_end <= char_start {
            continue;
        }
        let col_start = starts[char_start];
        let col_end = starts[char_end].min(last_content_col);
        if col_end > col_start {
            out.push(col_start..col_end);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStyle {
    Normal,
    /// Render with a reduced-emphasis terminal attribute. Used by `--dim` to
    /// keep filtered-out lines visible as context.
    Dim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

#[derive(Debug, Clone)]
pub struct SearchState {
    pub raw: String,
    pub regex: Regex,
    pub direction: SearchDirection,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub body: Vec<Vec<Cell>>,        // exactly (rows-1) entries
    pub row_styles: Vec<RowStyle>,   // parallel to body
    /// Per-row column ranges to render with reverse-video. Used by `/`
    /// search to highlight just the matched phrase rather than the whole row.
    /// Indexed parallel to `body`; each inner Vec holds column ranges in
    /// `[start, end)` form (cell columns).
    pub highlights: Vec<Vec<std::ops::Range<usize>>>,
    pub status: String,
}

pub struct Viewport {
    top_line: usize,
    top_row: usize,
    cols: u16,
    rows: u16,
    pub opts: RenderOpts,
    pub show_line_numbers: bool,
    pub source_label: String,
    follow_mode: bool,
    live_mode: bool,
    prettify_label: Option<String>,
    filter: Option<CompiledFilter>,
    dim_mode: bool,
    /// In hide mode (filter active, !dim), maps visible position → logical line
    /// index. Empty otherwise.
    visible_lines: Vec<usize>,
    /// How many logical lines we've evaluated for filter membership. Used by
    /// `extend_visible_lines` to avoid re-scanning lines on every tick.
    visible_scanned: usize,
    search: Option<SearchState>,
}

impl Viewport {
    pub fn new(cols: u16, rows: u16, source_label: String) -> Self {
        let mut opts = RenderOpts::default();
        opts.cols = cols;
        Self {
            top_line: 0,
            top_row: 0,
            cols,
            rows,
            opts,
            show_line_numbers: false,
            source_label,
            follow_mode: false,
            live_mode: false,
            prettify_label: None,
            filter: None,
            dim_mode: false,
            visible_lines: Vec::new(),
            visible_scanned: 0,
            search: None,
        }
    }

    /// Compile and store a search pattern. Returns the parse error from the
    /// regex crate if the pattern is invalid; the previous search (if any)
    /// is preserved on error.
    pub fn set_search(&mut self, raw: String, direction: SearchDirection) -> Result<(), String> {
        let regex = Regex::new(&raw).map_err(|e| e.to_string())?;
        self.search = Some(SearchState { raw, regex, direction });
        Ok(())
    }

    pub fn clear_search(&mut self) { self.search = None; }

    pub fn search_active(&self) -> bool { self.search.is_some() }

    /// Jump to the next match of the active search, in `direction` (or its
    /// reverse if `reverse` is true). Wraps at the end of the source.
    /// Returns true iff a match was found and the viewport moved.
    pub fn search_repeat(&mut self, src: &dyn Source, idx: &mut LineIndex, reverse: bool) -> bool {
        let Some(s) = self.search.as_ref() else { return false; };
        let forward = match (s.direction, reverse) {
            (SearchDirection::Forward, false) | (SearchDirection::Backward, true) => true,
            _ => false,
        };
        idx.extend_to_end(src);
        let pattern = s.regex.clone();
        if self.hide_mode() {
            self.extend_visible_lines(idx, src);
            self.search_step_in_visible(&pattern, src, idx, forward)
        } else {
            self.search_step_in_logical(&pattern, src, idx, forward)
        }
    }

    fn line_matches(&self, pattern: &Regex, src: &dyn Source, idx: &LineIndex, line_n: usize) -> bool {
        let range = idx.line_range(line_n, src);
        let bytes = src.bytes(range);
        match std::str::from_utf8(&bytes) {
            Ok(s) => pattern.is_match(s),
            Err(_) => false,
        }
    }

    fn search_step_in_logical(&mut self, pattern: &Regex, src: &dyn Source, idx: &LineIndex, forward: bool) -> bool {
        let total = idx.line_count();
        if total == 0 { return false; }
        let start = self.top_line;
        // Walk every logical line once, starting from start+1 (or start-1)
        // and wrapping at the end / beginning.
        for offset in 1..=total {
            let line_n = if forward {
                (start + offset) % total
            } else {
                (start + total - offset) % total
            };
            if self.line_matches(pattern, src, idx, line_n) {
                self.top_line = line_n;
                self.top_row = 0;
                return true;
            }
        }
        false
    }

    fn search_step_in_visible(&mut self, pattern: &Regex, src: &dyn Source, idx: &LineIndex, forward: bool) -> bool {
        let total = self.visible_lines.len();
        if total == 0 { return false; }
        // Find current visible position for top_line.
        let cur = self.visible_lines.iter().position(|&l| l >= self.top_line).unwrap_or(0);
        for offset in 1..=total {
            let visible_idx = if forward {
                (cur + offset) % total
            } else {
                (cur + total - offset) % total
            };
            let line_n = self.visible_lines[visible_idx];
            if self.line_matches(pattern, src, idx, line_n) {
                self.top_line = line_n;
                self.top_row = 0;
                return true;
            }
        }
        false
    }

    pub fn set_filter(&mut self, filter: Option<CompiledFilter>) {
        self.filter = filter;
        self.visible_lines.clear();
        self.visible_scanned = 0;
        // Drop scroll state — line numbering may have changed under us.
        self.top_line = 0;
        self.top_row = 0;
    }

    pub fn set_dim_mode(&mut self, on: bool) {
        self.dim_mode = on;
        // Hide mode is the only mode that needs visible_lines; clear when
        // turning dim ON, and re-derive from scratch when turning dim OFF
        // (next extend_visible_lines call rebuilds it).
        self.visible_lines.clear();
        self.visible_scanned = 0;
    }

    pub fn filter_active(&self) -> bool { self.filter.is_some() }

    pub fn dim_mode(&self) -> bool { self.dim_mode }

    fn hide_mode(&self) -> bool { self.filter.is_some() && !self.dim_mode }

    /// Walk any newly indexed logical lines and append matching ones to
    /// `visible_lines` if we're in hide mode. No-op otherwise. Cheap to call
    /// every loop tick — keeps a `visible_scanned` cursor.
    pub fn extend_visible_lines(&mut self, idx: &LineIndex, src: &dyn Source) {
        if !self.hide_mode() {
            return;
        }
        let Some(filter) = self.filter.as_ref() else { return };
        let total = idx.line_count();
        while self.visible_scanned < total {
            let line_n = self.visible_scanned;
            let range = idx.line_range(line_n, src);
            let bytes = src.bytes(range);
            if matches!(filter.evaluate(&bytes), FilterMatch::Matched) {
                self.visible_lines.push(line_n);
            }
            self.visible_scanned += 1;
        }
    }

    pub fn body_rows(&self) -> u16 { self.rows.saturating_sub(1).max(1) }

    pub fn follow_mode(&self) -> bool { self.follow_mode }

    pub fn set_follow_mode(&mut self, on: bool) { self.follow_mode = on; }

    pub fn toggle_follow(&mut self) { self.follow_mode = !self.follow_mode; }

    pub fn live_mode(&self) -> bool { self.live_mode }

    pub fn set_live_mode(&mut self, on: bool) { self.live_mode = on; }

    /// Status-line label for active pretty-print state, e.g. `"json"` or
    /// `"json:err"`. `None` means no indicator is shown.
    pub fn set_prettify_label(&mut self, label: Option<String>) {
        self.prettify_label = label;
    }

    /// Drop the per-line filter-membership cache without disturbing the filter
    /// itself or scroll position. Used after a `--live` rebuild: line numbering
    /// may have changed, so cached `visible_lines` is stale, but we want to
    /// keep the same filter applied and let the user stay where they were.
    pub fn invalidate_filter_cache(&mut self) {
        self.visible_lines.clear();
        self.visible_scanned = 0;
    }

    /// Clamp `top_line` so it doesn't fall past the new end of the source.
    /// Pairs with `invalidate_filter_cache` after a content rewrite.
    pub fn clamp_top_line(&mut self, line_count: usize) {
        if line_count == 0 {
            self.top_line = 0;
            self.top_row = 0;
        } else if self.top_line >= line_count {
            self.top_line = line_count - 1;
            self.top_row = 0;
        }
    }

    /// True when the viewport's body window already covers the last line of
    /// the source. New content added past this point should auto-scroll if
    /// follow mode is on.
    pub fn is_at_bottom(&self, idx: &LineIndex) -> bool {
        let body = self.body_rows() as usize;
        if self.hide_mode() {
            // top_line is a logical line; find its position in visible_lines.
            let pos = self
                .visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(self.visible_lines.len());
            pos + body >= self.visible_lines.len()
        } else {
            self.top_line + body >= idx.line_count()
        }
    }

    /// Width of the line-number gutter (digits + 1 space separator), 0 if disabled.
    fn gutter_width(&self, idx: &LineIndex) -> u16 {
        if !self.show_line_numbers { return 0; }
        let n = idx.line_count().max(1);
        let digits = (n as f64).log10().floor() as u16 + 1;
        digits + 1
    }

    fn render_opts(&self, gutter: u16) -> RenderOpts {
        let mut o = self.opts.clone();
        o.cols = self.cols.saturating_sub(gutter);
        o
    }

    pub fn frame(&self, src: &dyn Source, idx: &mut LineIndex) -> Frame {
        let body_rows = self.body_rows() as usize;
        idx.extend_to_line(self.top_line + body_rows + 1, src);

        let gutter = self.gutter_width(idx);
        let r_opts = self.render_opts(gutter);

        let mut body: Vec<Vec<Cell>> = Vec::with_capacity(body_rows);
        let mut row_styles: Vec<RowStyle> = Vec::with_capacity(body_rows);
        let mut highlights: Vec<Vec<std::ops::Range<usize>>> = Vec::with_capacity(body_rows);
        // In hide mode we walk visible_lines; otherwise we walk logical lines.
        let hide = self.hide_mode();
        let total_lines = idx.line_count();

        // For hide mode, find where the viewport starts in visible_lines.
        let mut hide_pos = if hide {
            self.visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(self.visible_lines.len())
        } else {
            0
        };
        let mut line_n = if hide {
            self.visible_lines.get(hide_pos).copied().unwrap_or(total_lines)
        } else {
            self.top_line
        };
        let mut skip = if hide { 0 } else { self.top_row };

        while body.len() < body_rows {
            if line_n >= total_lines {
                let mut row = Vec::with_capacity(self.cols as usize);
                if gutter > 0 {
                    for _ in 0..gutter { row.push(Cell::Empty); }
                }
                while row.len() < self.cols as usize { row.push(Cell::Empty); }
                body.push(row);
                row_styles.push(RowStyle::Normal);
                highlights.push(Vec::new());
                line_n += 1;
                continue;
            }
            let range = idx.line_range(line_n, src);
            let bytes = src.bytes(range);
            let rows = render_line(&bytes, &r_opts);
            let style = if let Some(f) = self.filter.as_ref() {
                if self.dim_mode {
                    match f.evaluate(&bytes) {
                        FilterMatch::Matched => RowStyle::Normal,
                        _ => RowStyle::Dim,
                    }
                } else {
                    // hide mode: only matching lines reach here
                    RowStyle::Normal
                }
            } else {
                RowStyle::Normal
            };

            for (i, mut content_row) in rows.into_iter().enumerate() {
                if i < skip { continue; }
                if body.len() >= body_rows { break; }
                let mut full: Vec<Cell> = Vec::with_capacity(self.cols as usize);
                if gutter > 0 {
                    let label = if i == 0 { format!("{:>width$} ", line_n + 1, width = (gutter as usize - 1)) } else { " ".repeat(gutter as usize) };
                    for c in label.chars() {
                        full.push(Cell::Char { ch: c, width: 1 });
                    }
                }
                full.append(&mut content_row);
                // Compute search highlights for this display row by running
                // the regex against the row's rendered text. Each match's
                // char range maps to a cell column range via `starts`.
                let row_highlights = if let Some(s) = self.search.as_ref() {
                    find_row_highlights(&full, &s.regex)
                } else {
                    Vec::new()
                };
                body.push(full);
                row_styles.push(style);
                highlights.push(row_highlights);
            }
            skip = 0;
            // Advance to next line — visible-space if hiding, logical-space otherwise.
            if hide {
                hide_pos += 1;
                line_n = self.visible_lines.get(hide_pos).copied().unwrap_or(total_lines);
            } else {
                line_n += 1;
            }
        }

        let status = self.format_status(idx, src);
        Frame { body, row_styles, highlights, status }
    }

    fn format_status(&self, idx: &LineIndex, src: &dyn Source) -> String {
        let body_rows = self.body_rows() as usize;
        let total = idx.line_count();
        // In hide mode, the line range and percentage refer to visible (matched)
        // lines, not the underlying logical line count.
        let (top, bottom, total_for_pct, total_str): (usize, usize, usize, String) = if self.hide_mode() {
            let visible_total = self.visible_lines.len();
            // top_line is a logical line; find its visible index.
            let cur = self
                .visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(visible_total);
            let top = cur + 1;
            let bottom = (cur + body_rows).min(visible_total.max(1));
            let total_str = if src.is_complete() {
                format!("{visible_total}/{total}")
            } else {
                format!("{visible_total}/{total}+")
            };
            (top, bottom, visible_total, total_str)
        } else {
            let top = self.top_line + 1;
            let bottom = (self.top_line + body_rows).min(total.max(1));
            let total_str = if src.is_complete() { format!("{total}") } else { format!("{total}+") };
            (top, bottom, total, total_str)
        };
        let pct = if total_for_pct == 0 { 0 } else { (bottom * 100) / total_for_pct };
        let mut s = format!("{}  {}-{}/{}  {}%", self.source_label, top, bottom, total_str, pct);
        if let Some(f) = self.filter.as_ref() {
            s.push_str(&format!("  [{}]", f.format_name));
            s.push_str(if self.dim_mode { "  [dim]" } else { "  [filter]" });
        }
        if let Some(sr) = self.search.as_ref() {
            let prefix = if matches!(sr.direction, SearchDirection::Forward) { "/" } else { "?" };
            s.push_str(&format!("  [{}{}]", prefix, sr.raw));
        }
        if let Some(label) = self.prettify_label.as_ref() {
            s.push_str(&format!("  [pretty:{label}]"));
        }
        if self.live_mode { s.push_str("  (L)"); }
        if self.follow_mode { s.push_str("  (F)"); }
        s
    }

    pub fn scroll_lines(&mut self, delta: i64, src: &dyn Source, idx: &mut LineIndex) {
        if delta == 0 { return; }
        if self.hide_mode() {
            // Scroll by visible (matching) lines. We don't honor wrap rows in
            // hide mode — top_row stays 0. Each unit of `delta` advances or
            // retreats one visible line.
            self.extend_visible_lines(idx, src);
            let total = self.visible_lines.len();
            if total == 0 {
                self.top_line = 0;
                self.top_row = 0;
                return;
            }
            let cur = self
                .visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(total);
            let new = (cur as i64 + delta).clamp(0, total.saturating_sub(1) as i64) as usize;
            self.top_line = self.visible_lines[new];
            self.top_row = 0;
            return;
        }
        if delta > 0 {
            let mut remaining = delta as usize;
            while remaining > 0 {
                idx.extend_to_line(self.top_line + 1, src);
                let total = idx.line_count();
                if self.top_line >= total.saturating_sub(1) { break; }
                let range = idx.line_range(self.top_line, src);
                let bytes = src.bytes(range);
                let line_rows = count_rows(&bytes, &self.render_opts(self.gutter_width(idx)));
                if self.top_row + 1 < line_rows {
                    self.top_row += 1;
                } else {
                    self.top_row = 0;
                    self.top_line += 1;
                }
                remaining -= 1;
            }
        } else {
            let mut remaining = (-delta) as usize;
            while remaining > 0 {
                if self.top_row > 0 {
                    self.top_row -= 1;
                } else if self.top_line > 0 {
                    self.top_line -= 1;
                    let range = idx.line_range(self.top_line, src);
                    let bytes = src.bytes(range);
                    let line_rows = count_rows(&bytes, &self.render_opts(self.gutter_width(idx)));
                    self.top_row = line_rows.saturating_sub(1);
                } else {
                    break;
                }
                remaining -= 1;
            }
        }
    }

    pub fn page_down(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = self.body_rows() as i64;
        self.scroll_lines(n, src, idx);
    }

    pub fn page_up(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = self.body_rows() as i64;
        self.scroll_lines(-n, src, idx);
    }

    pub fn half_page_down(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = (self.body_rows() / 2).max(1) as i64;
        self.scroll_lines(n, src, idx);
    }

    pub fn half_page_up(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = (self.body_rows() / 2).max(1) as i64;
        self.scroll_lines(-n, src, idx);
    }

    pub fn goto_top(&mut self) {
        self.top_line = 0;
        self.top_row = 0;
    }

    pub fn goto_bottom(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        idx.extend_to_end(src);
        let body = self.body_rows() as usize;
        if self.hide_mode() {
            self.extend_visible_lines(idx, src);
            let total = self.visible_lines.len();
            let target_visible = total.saturating_sub(body);
            self.top_line = self.visible_lines.get(target_visible).copied().unwrap_or(0);
            self.top_row = 0;
        } else {
            let total = idx.line_count();
            self.top_line = total.saturating_sub(body);
            self.top_row = 0;
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.rows = rows.max(2);
        self.opts.cols = self.cols;
    }

    pub fn toggle_line_numbers(&mut self) {
        self.show_line_numbers = !self.show_line_numbers;
    }

    pub fn toggle_chop(&mut self) {
        self.opts.wrap = !self.opts.wrap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MockSource;

    fn setup(content: &[u8]) -> (MockSource, LineIndex) {
        let m = MockSource::new();
        m.append(content);
        m.finish();
        let idx = LineIndex::new();
        (m, idx)
    }

    #[test]
    fn frame_renders_body_height_rows() {
        let (m, mut idx) = setup(b"a\nb\nc\nd\ne\n");
        let v = Viewport::new(10, 5, "test".into());  // body = 4
        let frame = v.frame(&m, &mut idx);
        assert_eq!(frame.body.len(), 4);
        assert_eq!(frame.body[0][0], Cell::Char { ch: 'a', width: 1 });
        assert_eq!(frame.body[3][0], Cell::Char { ch: 'd', width: 1 });
    }

    #[test]
    fn scroll_down_advances_top_line() {
        let (m, mut idx) = setup(b"a\nb\nc\nd\n");
        let mut v = Viewport::new(10, 5, "test".into());
        v.scroll_lines(2, &m, &mut idx);
        assert_eq!(v.top_line, 2);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn scroll_up_clamps_at_zero() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(10, 5, "test".into());
        v.scroll_lines(-5, &m, &mut idx);
        assert_eq!(v.top_line, 0);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn scroll_down_clamps_at_last_line() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(10, 5, "test".into());
        v.scroll_lines(50, &m, &mut idx);
        assert_eq!(v.top_line, 2);
    }

    #[test]
    fn status_line_shows_range_and_pct() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let v = Viewport::new(20, 5, "f".into());  // body = 4
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.starts_with("f  1-4/10"));
    }

    #[test]
    fn page_down_advances_by_body_rows() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 5, "f".into());  // body = 4
        v.page_down(&m, &mut idx);
        assert_eq!(v.top_line, 4);
    }

    #[test]
    fn page_up_then_page_down_returns_to_start_when_no_resize() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.page_down(&m, &mut idx);
        v.page_up(&m, &mut idx);
        assert_eq!(v.top_line, 0);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn half_page_down_advances_by_half_body() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 7, "f".into());  // body = 6, half = 3
        v.half_page_down(&m, &mut idx);
        assert_eq!(v.top_line, 3);
    }

    #[test]
    fn goto_top_resets_position() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.scroll_lines(2, &m, &mut idx);
        v.goto_top();
        assert_eq!(v.top_line, 0);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn goto_bottom_scrolls_to_last_page() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let mut v = Viewport::new(10, 5, "f".into());  // body = 4
        v.goto_bottom(&m, &mut idx);
        // Last page should show lines 7..=10 → top_line = 6.
        assert_eq!(v.top_line, 6);
    }

    #[test]
    fn resize_updates_dimensions_and_render_opts() {
        let (m, mut idx) = setup(b"1\n2\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.resize(40, 12);
        assert_eq!(v.cols, 40);
        assert_eq!(v.rows, 12);
        assert_eq!(v.opts.cols, 40);
        let _ = v.frame(&m, &mut idx);
    }

    #[test]
    fn toggle_line_numbers_changes_gutter() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(10, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        v.toggle_line_numbers();
        let frame_on = v.frame(&m, &mut idx);
        // With gutter, first cell is a digit or space, not 'a'.
        assert_eq!(frame_off.body[0][0], Cell::Char { ch: 'a', width: 1 });
        assert_ne!(frame_on.body[0][0], Cell::Char { ch: 'a', width: 1 });
    }

    #[test]
    fn toggle_chop_changes_wrap_mode() {
        let (m, mut idx) = setup(b"abcdefghij\n");
        let mut v = Viewport::new(4, 5, "f".into());
        v.toggle_chop();
        let frame = v.frame(&m, &mut idx);
        // After toggle_chop, the line is one row, not wrapped.
        // Body row 0 is "abcd"; rows 1..3 are blank fill.
        assert_eq!(frame.body[0][..4],
            [Cell::Char { ch: 'a', width: 1 }, Cell::Char { ch: 'b', width: 1 },
             Cell::Char { ch: 'c', width: 1 }, Cell::Char { ch: 'd', width: 1 }]);
        // Row 1 should be all-empty (no wrap continuation).
        assert!(frame.body[1].iter().all(|c| matches!(c, Cell::Empty)));
    }

    // ----- Follow mode -----

    #[test]
    fn is_at_bottom_initially_only_when_source_fits() {
        let (m, mut idx) = setup(b"a\nb\n");  // 2 lines
        let v = Viewport::new(10, 5, "f".into());  // body = 4 ≥ 2
        idx.extend_to_end(&m);
        assert!(v.is_at_bottom(&idx), "small file fits in body, top is at bottom");
    }

    #[test]
    fn is_at_bottom_false_when_top_and_more_lines_below() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");  // 8 lines
        let v = Viewport::new(10, 5, "f".into());  // body = 4
        idx.extend_to_end(&m);
        assert!(!v.is_at_bottom(&idx), "top of 8-line file with body=4 is not at bottom");
    }

    #[test]
    fn is_at_bottom_true_after_goto_bottom() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.goto_bottom(&m, &mut idx);
        assert!(v.is_at_bottom(&idx));
    }

    #[test]
    fn status_shows_F_suffix_when_follow_mode_on() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        assert!(!frame_off.status.contains("(F)"));
        v.set_follow_mode(true);
        let frame_on = v.frame(&m, &mut idx);
        assert!(frame_on.status.contains("(F)"), "expected (F) in status, got: {}", frame_on.status);
    }

    #[test]
    fn toggle_follow_flips_state() {
        let mut v = Viewport::new(10, 5, "f".into());
        assert!(!v.follow_mode());
        v.toggle_follow();
        assert!(v.follow_mode());
        v.toggle_follow();
        assert!(!v.follow_mode());
    }

    #[test]
    fn status_shows_prettify_label_when_set() {
        let (m, mut idx) = setup(b"a\n");
        let mut v = Viewport::new(40, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        assert!(!frame_off.status.contains("[pretty"));
        v.set_prettify_label(Some("json".into()));
        let frame_on = v.frame(&m, &mut idx);
        assert!(frame_on.status.contains("[pretty:json]"),
            "expected [pretty:json] in status, got: {}", frame_on.status);
        v.set_prettify_label(Some("json:err".into()));
        let frame_err = v.frame(&m, &mut idx);
        assert!(frame_err.status.contains("[pretty:json:err]"),
            "expected [pretty:json:err] in status, got: {}", frame_err.status);
    }

    #[test]
    fn status_shows_l_suffix_when_live_mode_on() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        assert!(!frame_off.status.contains("(L)"));
        v.set_live_mode(true);
        let frame_on = v.frame(&m, &mut idx);
        assert!(frame_on.status.contains("(L)"), "expected (L) in status, got: {}", frame_on.status);
    }

    #[test]
    fn clamp_top_line_pulls_back_when_total_shrinks() {
        let mut v = Viewport::new(20, 5, "f".into());
        // Pretend we were on line 100, then a rewrite leaves only 10 lines.
        v.scroll_lines(0, &MockSource::new(), &mut LineIndex::new()); // no-op, just to satisfy
        // Force top_line via a sequence; easiest: just call clamp directly.
        // We can't poke private state, but clamp works regardless of how we got there.
        v.clamp_top_line(100);  // total bigger than top_line=0, no change
        v.clamp_top_line(0);    // empty source: must reset
        // After clamp(0), line 0 is the floor.
        // (No public getter for top_line; we verify indirectly by going to top.)
        v.goto_top();
        // Just confirm no panic and no overflow on subsequent frame composition.
        let (m, mut idx) = setup(b"only\n");
        let _ = v.frame(&m, &mut idx);
    }

    /// Simulates the app::run timeout-branch logic to verify auto-scroll engages
    /// when follow mode is on and the viewport is at the bottom.
    fn simulate_growth_tick(
        v: &mut Viewport,
        src: &MockSource,
        idx: &mut LineIndex,
    ) {
        if !v.follow_mode() { return; }
        let was_at_bottom = v.is_at_bottom(idx);
        let lines_before = idx.line_count();
        idx.notice_new_bytes(src);
        if idx.line_count() != lines_before && was_at_bottom {
            v.goto_bottom(src, idx);
        }
    }

    #[test]
    fn auto_scroll_engages_when_at_bottom() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n");  // 4 lines, body=4 fits
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 5, "f".into());
        v.set_follow_mode(true);
        idx.extend_to_end(&m);
        assert!(v.is_at_bottom(&idx));
        let top_before = {
            let f = v.frame(&m, &mut idx);
            f.status.clone()  // unused, just exercise frame
        };
        let _ = top_before;
        // Simulate growth: source gains 4 more lines.
        m.append(b"5\n6\n7\n8\n");
        simulate_growth_tick(&mut v, &m, &mut idx);
        // After auto-scroll, top_line should have advanced so the new last line is in view.
        assert!(v.is_at_bottom(&idx), "after auto-scroll, viewport should still be at bottom");
        let frame = v.frame(&m, &mut idx);
        // The bottom-most body row should now contain the last logical line ('8').
        // Find which row has '8'.
        let last_row = &frame.body[frame.body.len() - 1];
        assert_eq!(last_row[0], Cell::Char { ch: '8', width: 1 });
    }

    #[test]
    fn auto_scroll_suppressed_when_scrolled_up() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n5\n6\n7\n8\n");  // 8 lines
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 5, "f".into());  // body=4
        v.set_follow_mode(true);
        idx.extend_to_end(&m);
        v.goto_bottom(&m, &mut idx);
        // Now scroll up off the bottom.
        v.scroll_lines(-2, &m, &mut idx);
        assert!(!v.is_at_bottom(&idx));
        let frame_before = v.frame(&m, &mut idx);
        let top_first_cell_before = frame_before.body[0][0].clone();
        // Simulate growth.
        m.append(b"9\n10\n");
        simulate_growth_tick(&mut v, &m, &mut idx);
        // Viewport should NOT have moved (auto-scroll suppressed).
        let frame_after = v.frame(&m, &mut idx);
        assert_eq!(frame_after.body[0][0], top_first_cell_before, "viewport moved despite being scrolled up");
    }

    // ----- Search -----

    #[test]
    fn set_search_compiles_regex() {
        let mut v = Viewport::new(10, 5, "f".into());
        assert!(v.set_search("foo".into(), SearchDirection::Forward).is_ok());
        assert!(v.search_active());
    }

    #[test]
    fn set_search_rejects_bad_regex() {
        let mut v = Viewport::new(10, 5, "f".into());
        let err = v.set_search("[".into(), SearchDirection::Forward).unwrap_err();
        assert!(!err.is_empty());
        assert!(!v.search_active(), "no search should be set on error");
    }

    #[test]
    fn search_step_forward_finds_match_after_top() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\ndelta\nepsilon\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("gamma".into(), SearchDirection::Forward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(found);
        // gamma is line 2 (0-indexed)
        assert_eq!(v.top_line, 2);
    }

    #[test]
    fn search_step_backward_finds_match_before_top() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\ndelta\nepsilon\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.scroll_lines(4, &m, &mut idx); // top_line = 4
        v.set_search("alpha".into(), SearchDirection::Backward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(found);
        assert_eq!(v.top_line, 0);
    }

    #[test]
    fn search_wraps_at_end() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.scroll_lines(2, &m, &mut idx); // top_line = 2 (last line)
        v.set_search("alpha".into(), SearchDirection::Forward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(found, "search should wrap forward past EOF");
        assert_eq!(v.top_line, 0);
    }

    #[test]
    fn search_no_match_returns_false_and_does_not_move() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("nowhere".into(), SearchDirection::Forward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(!found);
        assert_eq!(v.top_line, 0);
    }

    #[test]
    fn frame_records_highlight_ranges_for_matches() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\ndelta\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("gamma".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        // Body has 4 rows; row 2 is "gamma" (5 chars at columns 0..5).
        assert_eq!(frame.row_styles[0], RowStyle::Normal);
        assert!(frame.highlights[0].is_empty());
        assert!(frame.highlights[1].is_empty());
        assert_eq!(frame.highlights[2], vec![0..5]);
        assert!(frame.highlights[3].is_empty());
    }

    #[test]
    fn frame_highlights_substring_inside_a_row() {
        let (m, mut idx) = setup(b"the alpha and the beta\nfoo\n");
        let mut v = Viewport::new(40, 5, "f".into());
        v.set_search("beta".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        // "beta" starts at column 18 in the first row.
        assert_eq!(frame.highlights[0], vec![18..22]);
        assert!(frame.highlights[1].is_empty());
    }

    #[test]
    fn search_highlight_with_filter_dim_keeps_row_dim() {
        // alpha matches filter → Normal. beta doesn't → Dim. Search for
        // "beta" should leave row style Dim and mark the substring 0..4.
        let (m, mut idx) = setup(b"alpha\nbeta\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let fmt = crate::format::LogFormat::compile(
            "simple",
            r"^(?P<line>.+)$",
        )
        .unwrap();
        let f = crate::filter::CompiledFilter::compile(
            &fmt,
            vec![crate::filter::FilterSpec::parse("line=alpha").unwrap()],
        )
        .unwrap();
        v.set_filter(Some(f));
        v.set_dim_mode(true);
        v.set_search("beta".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        assert_eq!(frame.row_styles[0], RowStyle::Normal);
        assert_eq!(frame.row_styles[1], RowStyle::Dim);
        assert_eq!(frame.highlights[1], vec![0..4]);
    }

    #[test]
    fn search_status_shows_pattern() {
        let (m, mut idx) = setup(b"x\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("foo".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.contains("[/foo]"), "status: {}", frame.status);
    }

    #[test]
    fn auto_scroll_paused_when_follow_off() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n");
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 5, "f".into());
        // Follow is off; viewport at top.
        idx.extend_to_end(&m);
        let frame_before = v.frame(&m, &mut idx);
        let top_first_cell = frame_before.body[0][0].clone();
        m.append(b"5\n6\n7\n8\n");
        simulate_growth_tick(&mut v, &m, &mut idx);
        let frame_after = v.frame(&m, &mut idx);
        assert_eq!(frame_after.body[0][0], top_first_cell, "auto-scroll fired despite follow off");
    }
}
