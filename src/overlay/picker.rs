//! File picker overlay. Lists every file in the working set, supports
//! type-to-filter, Enter to open, Ctrl-D to drop.

use std::borrow::Cow;
use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::file_set::FileSet;
use crate::input::Command;
use crate::overlay::{Overlay, OverlayContext, OverlayFrame, OverlayOutcome};

pub struct FilePicker {
    filter: String,
    cursor: usize,              // index into `visible`
    visible: Vec<usize>,        // indices into FileSet
    rows_offset: Cell<usize>,   // first visible row when list overflows screen (interior-mutable so render stays &self)
    /// Snapshot of each file's last-known top line, indexed parallel to FileSet.
    /// (Captured at open time and passed to the picker by the caller.)
    saved_lines: Vec<usize>,
    /// Path display strings, parallel to FileSet indices, captured at open.
    paths: Vec<String>,
    /// Index of the current file in the FileSet at open time (for the
    /// "← current" annotation).
    current_index: usize,
}

impl FilePicker {
    pub fn new(file_set: &FileSet, saved_lines: Vec<usize>) -> Self {
        let paths: Vec<String> = (0..file_set.len())
            .map(|i| file_set.nth(i).map(|p| p.display().to_string()).unwrap_or_default())
            .collect();
        let visible: Vec<usize> = (0..file_set.len()).collect();
        let cursor = file_set.current_index().min(visible.len().saturating_sub(1));
        Self {
            filter: String::new(),
            cursor,
            visible,
            rows_offset: Cell::new(0),
            saved_lines,
            paths,
            current_index: file_set.current_index(),
        }
    }

    fn recompute_visible(&mut self) {
        let needle = self.filter.to_lowercase();
        if needle.is_empty() {
            self.visible = (0..self.paths.len()).collect();
        } else {
            self.visible = (0..self.paths.len())
                .filter(|&i| self.paths[i].to_lowercase().contains(&needle))
                .collect();
        }
        if self.cursor >= self.visible.len() {
            self.cursor = self.visible.len().saturating_sub(1);
        }
        self.rows_offset.set(0);
    }
}

impl Overlay for FilePicker {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayOutcome {
        // Ctrl-D: remove highlighted file
        if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
            // Guard on global set count, not the filtered view — :d's semantics.
            if self.paths.len() <= 1 {
                return OverlayOutcome::Refuse("can't remove last file");
            }
            let target = match self.visible.get(self.cursor) {
                Some(&t) => t,
                None => return OverlayOutcome::Stay,
            };
            return OverlayOutcome::Apply(Command::DropFileAt(target));
        }
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                if self.filter.is_empty() {
                    OverlayOutcome::Close
                } else {
                    self.filter.clear();
                    self.recompute_visible();
                    OverlayOutcome::Stay
                }
            }
            (KeyCode::Up, _) => {
                self.cursor = self.cursor.saturating_sub(1);
                OverlayOutcome::Stay
            }
            // j/k vim keys require NO modifiers — Shift+k must fall through to
            // the filter so users can type uppercase letters into the search.
            (KeyCode::Char('k'), m) if m == KeyModifiers::NONE => {
                self.cursor = self.cursor.saturating_sub(1);
                OverlayOutcome::Stay
            }
            (KeyCode::Down, _) => {
                if self.cursor + 1 < self.visible.len() {
                    self.cursor += 1;
                }
                OverlayOutcome::Stay
            }
            (KeyCode::Char('j'), m) if m == KeyModifiers::NONE => {
                if self.cursor + 1 < self.visible.len() {
                    self.cursor += 1;
                }
                OverlayOutcome::Stay
            }
            (KeyCode::PageUp, _) => {
                self.cursor = self.cursor.saturating_sub(10);
                OverlayOutcome::Stay
            }
            (KeyCode::PageDown, _) => {
                self.cursor = (self.cursor + 10).min(self.visible.len().saturating_sub(1));
                OverlayOutcome::Stay
            }
            (KeyCode::Home, _) => { self.cursor = 0; OverlayOutcome::Stay }
            (KeyCode::End, _)  => {
                self.cursor = self.visible.len().saturating_sub(1);
                OverlayOutcome::Stay
            }
            (KeyCode::Enter, _) => {
                match self.visible.get(self.cursor) {
                    Some(&i) => OverlayOutcome::CloseAnd(Command::SelectFile(i)),
                    None => OverlayOutcome::Stay,
                }
            }
            (KeyCode::Backspace, _) => {
                self.filter.pop();
                self.recompute_visible();
                OverlayOutcome::Stay
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) => {
                self.filter.push(c);
                self.recompute_visible();
                OverlayOutcome::Stay
            }
            _ => OverlayOutcome::Stay,
        }
    }

    fn handle_mouse(&mut self, ev: crossterm::event::MouseEvent, _body_rows: u16) -> OverlayOutcome {
        use crossterm::event::{MouseButton, MouseEventKind};
        match ev.kind {
            MouseEventKind::ScrollDown => {
                if self.cursor + 1 < self.visible.len() {
                    self.cursor += 1;
                }
                OverlayOutcome::Stay
            }
            MouseEventKind::ScrollUp => {
                self.cursor = self.cursor.saturating_sub(1);
                OverlayOutcome::Stay
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // Body layout: row 0 title, row 1 blank, row 2+ entries.
                // rows_offset accounts for how far the list has been scrolled.
                let row = ev.row as usize;
                if row < 2 { return OverlayOutcome::Stay; }
                let visible_idx = row - 2 + self.rows_offset.get();
                if visible_idx >= self.visible.len() { return OverlayOutcome::Stay; }
                self.cursor = visible_idx;
                OverlayOutcome::CloseAnd(Command::SelectFile(self.visible[self.cursor]))
            }
            _ => OverlayOutcome::Stay,
        }
    }

    fn render(&self, width: u16, height: u16) -> OverlayFrame {
        let mut body = Vec::with_capacity(height as usize);
        // Row 0: title
        let title = if self.filter.is_empty() {
            format!("Files ({})", self.visible.len())
        } else {
            format!(
                "Files ({} of {} matching \"{}\")",
                self.visible.len(), self.paths.len(), self.filter,
            )
        };
        body.push(title);
        body.push(String::new());

        // Compute the column width for filenames.
        let name_col = self.visible.iter()
            .map(|&i| self.paths[i].chars().count())
            .max()
            .unwrap_or(0)
            .min(width.saturating_sub(20) as usize);

        // Adjust rows_offset to keep cursor visible (stable: only move when
        // cursor goes off-screen, not pinned to bottom on every render).
        let visible_rows = (height as usize).saturating_sub(3); // title + blank + status
        let mut offset = self.rows_offset.get();
        if visible_rows > 0 {
            if self.cursor < offset {
                // Cursor went off the top: scroll up to put it at the top.
                offset = self.cursor;
            } else if self.cursor >= offset + visible_rows {
                // Cursor went off the bottom: scroll just enough to put it at the bottom.
                offset = self.cursor + 1 - visible_rows;
            }
            // Otherwise: cursor is already visible; leave offset alone.
        }
        self.rows_offset.set(offset);

        for (row, &i) in self.visible.iter().enumerate().skip(offset).take(visible_rows) {
            let is_cursor = row == self.cursor;
            let is_current = i == self.current_index;
            let gutter = if is_cursor { ">" } else { " " };
            let line_n = self.saved_lines.get(i).copied().unwrap_or(0).max(1);
            let trailer = if is_current { "  \u{2190} current" } else { "" };
            let path = &self.paths[i];
            // Truncate paths that overflow the column so the L<line> field
            // stays on-screen.
            let path_display: String = if path.chars().count() > name_col && name_col > 0 {
                let mut s: String = path.chars().take(name_col.saturating_sub(1)).collect();
                s.push('\u{2026}'); // ellipsis …
                s
            } else {
                path.clone()
            };
            body.push(format!(
                "{gutter} {path_display:<name_col$}  L{line_n}{trailer}",
            ));
        }

        let status = "[filter]  \u{2191}\u{2193} Enter  Ctrl-D remove  Esc".to_string();
        OverlayFrame { body, status }
    }

    fn title(&self) -> Cow<'_, str> { Cow::Borrowed("Files") }

    fn refresh(&mut self, ctx: OverlayContext) {
        // Re-snapshot paths from the (possibly mutated) FileSet.
        self.paths = (0..ctx.file_set.len())
            .map(|i| ctx.file_set.nth(i).map(|p| p.display().to_string()).unwrap_or_default())
            .collect();
        // saved_lines is now stale for removed entries; trim if longer.
        self.saved_lines.truncate(self.paths.len());
        while self.saved_lines.len() < self.paths.len() {
            self.saved_lines.push(0);
        }
        self.current_index = ctx.file_set.current_index();
        self.recompute_visible();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent as KE, MouseButton, MouseEvent, MouseEventKind};
    use std::path::PathBuf;

    fn fs(names: &[&str]) -> FileSet {
        FileSet::new(names.iter().map(PathBuf::from).collect())
    }

    fn picker(names: &[&str]) -> FilePicker {
        FilePicker::new(&fs(names), vec![0; names.len()])
    }

    fn key(code: KeyCode, mods: KeyModifiers) -> KE {
        KE::new(code, mods)
    }

    #[test]
    fn starts_with_cursor_on_current_file() {
        let mut f = fs(&["a", "b", "c"]);
        f.set_current_index(1);
        let p = FilePicker::new(&f, vec![0, 0, 0]);
        assert_eq!(p.cursor, 1);
        assert_eq!(p.visible, vec![0, 1, 2]);
    }

    #[test]
    fn down_arrow_moves_cursor() {
        let mut p = picker(&["a", "b", "c"]);
        assert!(matches!(p.handle_key(key(KeyCode::Down, KeyModifiers::NONE)), OverlayOutcome::Stay));
        assert_eq!(p.cursor, 1);
    }

    #[test]
    fn up_arrow_at_top_is_clamped() {
        let mut p = picker(&["a", "b"]);
        p.handle_key(key(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn typing_filters_visible_list() {
        let mut p = picker(&["alpha", "beta", "alpine"]);
        p.handle_key(key(KeyCode::Char('a'), KeyModifiers::NONE));
        p.handle_key(key(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(p.filter, "al");
        assert_eq!(p.visible, vec![0, 2]);
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut p = picker(&["Alpha", "beta", "ALPINE"]);
        p.handle_key(key(KeyCode::Char('a'), KeyModifiers::NONE));
        p.handle_key(key(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(p.visible, vec![0, 2]);
    }

    #[test]
    fn backspace_trims_filter_and_restores_visibility() {
        let mut p = picker(&["alpha", "uno"]);
        p.handle_key(key(KeyCode::Char('a'), KeyModifiers::NONE));
        assert_eq!(p.visible.len(), 1);
        p.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(p.filter, "");
        assert_eq!(p.visible, vec![0, 1]);
    }

    #[test]
    fn esc_clears_filter_first_then_closes() {
        let mut p = picker(&["a", "b"]);
        p.handle_key(key(KeyCode::Char('a'), KeyModifiers::NONE));
        let first = p.handle_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(first, OverlayOutcome::Stay));
        assert_eq!(p.filter, "");
        let second = p.handle_key(key(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(second, OverlayOutcome::Close));
    }

    #[test]
    fn enter_emits_select_file_with_visible_index() {
        let mut p = picker(&["a", "b", "c"]);
        p.handle_key(key(KeyCode::Down, KeyModifiers::NONE)); // cursor=1
        let out = p.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
        match out {
            OverlayOutcome::CloseAnd(Command::SelectFile(i)) => assert_eq!(i, 1),
            other => panic!("expected SelectFile(1), got {other:?}"),
        }
    }

    #[test]
    fn ctrl_d_with_n_equals_1_refuses() {
        let mut p = picker(&["only"]);
        let out = p.handle_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(matches!(out, OverlayOutcome::Refuse(_)));
    }

    #[test]
    fn ctrl_d_with_n_gt_1_applies_drop() {
        let mut p = picker(&["a", "b"]);
        let out = p.handle_key(key(KeyCode::Char('d'), KeyModifiers::CONTROL));
        match out {
            OverlayOutcome::Apply(Command::DropFileAt(i)) => assert_eq!(i, 0),
            other => panic!("expected Apply(DropFileAt(0)), got {other:?}"),
        }
    }

    #[test]
    fn cursor_clamped_when_filter_shrinks_visible() {
        let mut p = picker(&["alpha", "beta", "gamma"]);
        p.handle_key(key(KeyCode::End, KeyModifiers::NONE));  // cursor=2
        p.handle_key(key(KeyCode::Char('b'), KeyModifiers::NONE)); // visible=[1]
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn filter_uses_substring_not_prefix() {
        // 'log' should match files where 'log' appears anywhere in the path,
        // not just at the start.
        let mut p = picker(&["app.rs", "build.log", "src/logger.rs"]);
        p.handle_key(key(KeyCode::Char('l'), KeyModifiers::NONE));
        p.handle_key(key(KeyCode::Char('o'), KeyModifiers::NONE));
        p.handle_key(key(KeyCode::Char('g'), KeyModifiers::NONE));
        // build.log → contains "log" (substring) ✓
        // src/logger.rs → contains "log" (substring) ✓
        // app.rs → no match
        assert_eq!(p.visible, vec![1, 2], "substring filter should match 'log' anywhere in path");
    }

    #[test]
    fn enter_on_empty_visible_is_noop() {
        // Spec testing-strategy explicitly calls out this case.
        let mut p = picker(&["alpha", "beta"]);
        // Filter to nothing.
        p.handle_key(key(KeyCode::Char('z'), KeyModifiers::NONE));
        assert!(p.visible.is_empty());
        let out = p.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(out, OverlayOutcome::Stay));
    }

    #[test]
    fn refresh_after_drop_rebuilds_visible() {
        let mut fs = fs(&["a", "b", "c"]);
        let mut p = FilePicker::new(&fs, vec![0, 0, 0]);
        p.handle_key(key(KeyCode::Down, KeyModifiers::NONE)); // cursor=1
        // Simulate the app dispatching DropFileAt(0): FileSet shrinks.
        fs.delete_current().unwrap();
        p.refresh(OverlayContext { file_set: &fs });
        assert_eq!(p.paths.len(), 2);
        assert!(p.cursor < p.paths.len());
    }

    #[test]
    fn render_lists_all_files_with_position() {
        let p = FilePicker::new(&fs(&["a.log", "b.log"]), vec![1, 42]);
        let frame = p.render(80, 10);
        assert_eq!(frame.body[0], "Files (2)");
        // Row 0 = title; row 1 = blank; row 2 onward = entries.
        assert!(frame.body.iter().any(|l| l.contains("a.log") && l.contains("L1")));
        assert!(frame.body.iter().any(|l| l.contains("b.log") && l.contains("L42")));
    }

    #[test]
    fn render_marks_current_with_arrow() {
        let mut f = fs(&["a", "b"]);
        f.set_current_index(1);
        let p = FilePicker::new(&f, vec![0, 0]);
        let frame = p.render(80, 10);
        let current_line = frame.body.iter().find(|l| l.contains("b")).expect("b line");
        assert!(current_line.contains("\u{2190} current"), "current marker missing: {current_line:?}");
    }

    #[test]
    fn render_title_updates_when_filtering() {
        let mut p = picker(&["alpha", "beta", "alpine"]);
        p.handle_key(KE::new(KeyCode::Char('a'), KeyModifiers::NONE));
        p.handle_key(KE::new(KeyCode::Char('l'), KeyModifiers::NONE));
        let frame = p.render(80, 10);
        assert_eq!(frame.body[0], "Files (2 of 3 matching \"al\")");
    }

    #[test]
    fn render_status_shows_keybindings() {
        let p = picker(&["a"]);
        let frame = p.render(80, 10);
        assert!(frame.status.contains("Enter"), "status missing Enter hint");
        assert!(frame.status.contains("Ctrl-D"), "status missing Ctrl-D hint");
        assert!(frame.status.contains("Esc"), "status missing Esc hint");
    }

    #[test]
    fn scroll_offset_keeps_cursor_in_band_stably() {
        // 20 files, terminal height 8 (visible_rows = 5).
        let names: Vec<String> = (0..20).map(|n| format!("file_{n:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut p = picker(&refs);
        // Cursor at 10 — well into the list.
        for _ in 0..10 {
            p.handle_key(KE::new(KeyCode::Down, KeyModifiers::NONE));
        }
        let _ = p.render(80, 8);   // visible_rows = 5
        // After scrolling down: offset should put cursor on the bottom row.
        // cursor = 10, visible_rows = 5 → offset = 6 (cursor at bottom of window).
        assert_eq!(p.rows_offset.get(), 6);

        // Now scroll up by 2 — cursor = 8. The window should NOT change because
        // 8 is still inside [6, 6+5).
        p.handle_key(KE::new(KeyCode::Up, KeyModifiers::NONE));
        p.handle_key(KE::new(KeyCode::Up, KeyModifiers::NONE));
        let _ = p.render(80, 8);
        assert_eq!(p.rows_offset.get(), 6, "window should be stable while cursor is in band");

        // Scroll up enough to go off-screen — cursor below offset.
        for _ in 0..5 {
            p.handle_key(KE::new(KeyCode::Up, KeyModifiers::NONE));
        }
        // cursor = 3, offset was 6 → new offset = 3.
        let _ = p.render(80, 8);
        assert_eq!(p.rows_offset.get(), 3);
    }

    #[test]
    fn long_paths_are_truncated_with_ellipsis() {
        // Make one path that exceeds the column.
        let p = FilePicker::new(
            &fs(&["short.rs", "very/long/nested/path/to/some_module.rs"]),
            vec![0, 0],
        );
        // Narrow terminal: 40 cols → name_col = 20 max.
        let frame = p.render(40, 10);
        let long_row = frame.body.iter().find(|l| l.contains('\u{2026}')).expect("ellipsis row");
        // Should contain ellipsis, and the L<line> column should still be visible.
        assert!(long_row.contains("L1"), "L<line> column should still be visible: {long_row:?}");
    }

    fn mouse(kind: MouseEventKind, row: u16) -> MouseEvent {
        MouseEvent { kind, column: 0, row, modifiers: KeyModifiers::NONE }
    }

    #[test]
    fn left_click_sets_cursor_and_selects() {
        // body layout: row 0 = title, row 1 = blank, row 2 = first entry,
        // row 3 = second entry, ...
        let mut p = picker(&["a", "b", "c"]);
        let out = p.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3), 10);
        match out {
            OverlayOutcome::CloseAnd(Command::SelectFile(i)) => assert_eq!(i, 1),
            other => panic!("expected SelectFile(1), got {other:?}"),
        }
    }

    #[test]
    fn scrollwheel_moves_cursor() {
        let mut p = picker(&["a", "b", "c"]);
        p.handle_mouse(mouse(MouseEventKind::ScrollDown, 0), 10);
        assert_eq!(p.cursor, 1);
        p.handle_mouse(mouse(MouseEventKind::ScrollUp, 0), 10);
        assert_eq!(p.cursor, 0);
    }
}
