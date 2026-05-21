//! File picker overlay. Lists every file in the working set, supports
//! type-to-filter, Enter to open, Ctrl-D to drop.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::file_set::FileSet;
use crate::input::Command;
use crate::overlay::{Overlay, OverlayContext, OverlayFrame, OverlayOutcome};

pub struct FilePicker {
    filter: String,
    cursor: usize,           // index into `visible`
    visible: Vec<usize>,     // indices into FileSet
    rows_offset: usize,      // first visible row when list overflows screen
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
            rows_offset: 0,
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
        self.rows_offset = 0;
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

    fn render(&self, _width: u16, _height: u16) -> OverlayFrame {
        // Implemented in Task 9.
        OverlayFrame { body: vec![], status: String::new() }
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
    use crossterm::event::KeyEvent as KE;
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
}
