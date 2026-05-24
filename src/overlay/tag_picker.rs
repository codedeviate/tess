//! Tag picker overlay (`:tselect`). Lists every match for a tag name
//! and lets the user pick one with the keyboard. Enter dispatches a
//! `SelectTagMatch(idx)` command back to the app loop.

use std::borrow::Cow;
use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::input::Command;
use crate::overlay::{Overlay, OverlayFrame, OverlayOutcome};
use crate::tags::{TagAddress, TagEntry};

pub struct TagPicker {
    /// The tag name being selected for (shown in the title).
    name: String,
    /// All matches, exactly as they appear in TagStack::active. The index
    /// emitted via `SelectTagMatch(idx)` is into this vec.
    entries: Vec<TagEntry>,
    cursor: usize,
    rows_offset: Cell<usize>,
}

impl TagPicker {
    pub fn new(name: String, entries: Vec<TagEntry>, initial_cursor: usize) -> Self {
        let cursor = initial_cursor.min(entries.len().saturating_sub(1));
        Self {
            name,
            entries,
            cursor,
            rows_offset: Cell::new(0),
        }
    }

    fn format_row(&self, idx: usize) -> String {
        let e = &self.entries[idx];
        let file = e.file.display().to_string();
        let addr = match &e.address {
            TagAddress::Line(n) => format!(":{n}"),
            TagAddress::Pattern(p) => format!("  /{p}/"),
            TagAddress::Chained(parts) => format!("  ({} steps)", parts.len()),
            TagAddress::Unsupported(_) => "  (unsupported)".to_string(),
        };
        format!("{:>3}. {file}{addr}", idx + 1)
    }
}

impl Overlay for TagPicker {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayOutcome {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => OverlayOutcome::Close,
            (KeyCode::Enter, _) => OverlayOutcome::CloseAnd(Command::SelectTagMatch(self.cursor)),
            (KeyCode::Up, _) | (KeyCode::Char('k'), KeyModifiers::NONE) => {
                self.cursor = self.cursor.saturating_sub(1);
                OverlayOutcome::Stay
            }
            (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                }
                OverlayOutcome::Stay
            }
            (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.cursor = 0;
                OverlayOutcome::Stay
            }
            (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                self.cursor = self.entries.len().saturating_sub(1);
                OverlayOutcome::Stay
            }
            // Number shortcuts: 1-9 jump to that match (when in range).
            (KeyCode::Char(c), KeyModifiers::NONE) if c.is_ascii_digit() && c != '0' => {
                let n = (c as u8 - b'0') as usize;
                if n <= self.entries.len() {
                    self.cursor = n - 1;
                    OverlayOutcome::CloseAnd(Command::SelectTagMatch(self.cursor))
                } else {
                    OverlayOutcome::Stay
                }
            }
            _ => OverlayOutcome::Stay,
        }
    }

    fn render(&self, _width: u16, height: u16) -> OverlayFrame {
        let body_rows = (height as usize).saturating_sub(1).max(1);

        // Adjust rows_offset so the cursor stays visible.
        let mut off = self.rows_offset.get();
        if self.cursor < off {
            off = self.cursor;
        } else if self.cursor >= off + body_rows {
            off = self.cursor + 1 - body_rows;
        }
        off = off.min(self.entries.len().saturating_sub(body_rows));
        self.rows_offset.set(off);

        let mut body: Vec<String> = Vec::with_capacity(body_rows);
        for slot in 0..body_rows {
            let row_idx = off + slot;
            if row_idx >= self.entries.len() {
                body.push(String::new());
                continue;
            }
            let marker = if row_idx == self.cursor { "> " } else { "  " };
            body.push(format!("{marker}{}", self.format_row(row_idx)));
        }

        let status = format!(
            "tselect: {}  [{}/{}]  Enter=jump  Esc=cancel  1-9=quick",
            self.name,
            self.cursor + 1,
            self.entries.len(),
        );
        OverlayFrame { body, status }
    }

    fn title(&self) -> Cow<'_, str> {
        Cow::Owned(format!("tselect: {}", self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entries(n: usize) -> Vec<TagEntry> {
        (0..n)
            .map(|i| TagEntry {
                file: PathBuf::from(format!("src/f{i}.rs")),
                address: TagAddress::Line(i + 1),
            })
            .collect()
    }

    #[test]
    fn enter_emits_select_with_cursor_index() {
        let mut p = TagPicker::new("foo".into(), entries(3), 0);
        match p.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)) {
            OverlayOutcome::Stay => {}
            other => panic!("expected Stay, got {other:?}"),
        }
        match p.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)) {
            OverlayOutcome::CloseAnd(Command::SelectTagMatch(1)) => {}
            other => panic!("expected SelectTagMatch(1), got {other:?}"),
        }
    }

    #[test]
    fn number_shortcut_picks_directly() {
        let mut p = TagPicker::new("foo".into(), entries(5), 0);
        match p.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE)) {
            OverlayOutcome::CloseAnd(Command::SelectTagMatch(2)) => {}
            other => panic!("expected SelectTagMatch(2), got {other:?}"),
        }
    }

    #[test]
    fn number_shortcut_out_of_range_stays() {
        let mut p = TagPicker::new("foo".into(), entries(2), 0);
        match p.handle_key(KeyEvent::new(KeyCode::Char('9'), KeyModifiers::NONE)) {
            OverlayOutcome::Stay => {}
            other => panic!("expected Stay, got {other:?}"),
        }
    }

    #[test]
    fn esc_closes() {
        let mut p = TagPicker::new("foo".into(), entries(3), 0);
        match p.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)) {
            OverlayOutcome::Close => {}
            other => panic!("expected Close, got {other:?}"),
        }
    }

    #[test]
    fn render_marks_cursor_row() {
        let p = TagPicker::new("foo".into(), entries(3), 1);
        let f = p.render(80, 10);
        assert!(f.body.iter().any(|l| l.starts_with("> ")));
        // Title-ish status with current/total.
        assert!(f.status.contains("[2/3]"));
    }
}
