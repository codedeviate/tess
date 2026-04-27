use crate::line_index::LineIndex;
use crate::render::{count_rows, render_line, Cell, RenderOpts};
use crate::source::Source;

#[derive(Debug, Clone)]
pub struct Frame {
    pub body: Vec<Vec<Cell>>,   // exactly (rows-1) entries
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
        }
    }

    pub fn body_rows(&self) -> u16 { self.rows.saturating_sub(1).max(1) }

    pub fn follow_mode(&self) -> bool { self.follow_mode }

    pub fn set_follow_mode(&mut self, on: bool) { self.follow_mode = on; }

    pub fn toggle_follow(&mut self) { self.follow_mode = !self.follow_mode; }

    /// True when the viewport's body window already covers the last line of
    /// the source. New content added past this point should auto-scroll if
    /// follow mode is on.
    pub fn is_at_bottom(&self, idx: &LineIndex) -> bool {
        let body = self.body_rows() as usize;
        self.top_line + body >= idx.line_count()
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
        let mut line_n = self.top_line;
        let mut skip = self.top_row;
        let total_lines = idx.line_count();

        while body.len() < body_rows {
            if line_n >= total_lines {
                let mut row = Vec::with_capacity(self.cols as usize);
                if gutter > 0 {
                    for _ in 0..gutter { row.push(Cell::Empty); }
                }
                while row.len() < self.cols as usize { row.push(Cell::Empty); }
                body.push(row);
                line_n += 1;
                continue;
            }
            let range = idx.line_range(line_n, src);
            let bytes = src.bytes(range);
            let rows = render_line(&bytes, &r_opts);
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
                body.push(full);
            }
            skip = 0;
            line_n += 1;
        }

        let status = self.format_status(idx, src);
        Frame { body, status }
    }

    fn format_status(&self, idx: &LineIndex, src: &dyn Source) -> String {
        let body_rows = self.body_rows() as usize;
        let total = idx.line_count();
        let top = self.top_line + 1;
        let bottom = (self.top_line + body_rows).min(total.max(1));
        let pct = if total == 0 { 0 } else { (bottom * 100) / total };
        let total_str = if src.is_complete() { format!("{}", total) } else { format!("{}+", total) };
        let follow_suffix = if self.follow_mode { "  (F)" } else { "" };
        format!("{}  {}-{}/{}  {}%{}", self.source_label, top, bottom, total_str, pct, follow_suffix)
    }

    pub fn scroll_lines(&mut self, delta: i64, src: &dyn Source, idx: &mut LineIndex) {
        if delta == 0 { return; }
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
        let total = idx.line_count();
        let body = self.body_rows() as usize;
        self.top_line = total.saturating_sub(body);
        self.top_row = 0;
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
