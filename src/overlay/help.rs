//! Help overlay: lists every binding from KEY_REGISTRY, grouped by category,
//! with type-to-filter. User remaps from keys.toml replace the displayed
//! keys per command_name.

use std::borrow::Cow;
use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keymap::{Category, KeyEntry, KEY_REGISTRY};
use crate::overlay::{Overlay, OverlayFrame, OverlayOutcome};

pub struct HelpOverlay {
    filter: String,
    cursor: usize,          // row index across the flat rendered list
    user_remaps: HashMap<String, Vec<String>>,
}

impl HelpOverlay {
    pub fn new(user_remaps: HashMap<String, Vec<String>>) -> Self {
        Self { filter: String::new(), cursor: 0, user_remaps }
    }

    fn visible_entries(&self) -> Vec<&'static KeyEntry> {
        let needle = self.filter.to_lowercase();
        KEY_REGISTRY.iter()
            .filter(|e| {
                if needle.is_empty() { return true; }
                let keys_joined = e.keys.join(" ").to_lowercase();
                e.description.to_lowercase().contains(&needle)
                    || keys_joined.contains(&needle)
            })
            .collect()
    }

    fn display_keys<'a>(&'a self, entry: &'static KeyEntry) -> Vec<&'a str> {
        if let Some(user) = self.user_remaps.get(entry.command_name) {
            user.iter().map(String::as_str).collect()
        } else {
            entry.keys.iter().copied().collect()
        }
    }
}

impl Overlay for HelpOverlay {
    fn handle_key(&mut self, key: KeyEvent) -> OverlayOutcome {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => {
                if self.filter.is_empty() {
                    OverlayOutcome::Close
                } else {
                    self.filter.clear();
                    self.cursor = 0;
                    OverlayOutcome::Stay
                }
            }
            (KeyCode::Up, _) => {
                self.cursor = self.cursor.saturating_sub(1);
                OverlayOutcome::Stay
            }
            (KeyCode::Char('k'), m) if m == KeyModifiers::NONE => {
                self.cursor = self.cursor.saturating_sub(1);
                OverlayOutcome::Stay
            }
            (KeyCode::Down, _) => {
                self.cursor = self.cursor.saturating_add(1);
                OverlayOutcome::Stay
            }
            (KeyCode::Char('j'), m) if m == KeyModifiers::NONE => {
                self.cursor = self.cursor.saturating_add(1);
                OverlayOutcome::Stay
            }
            (KeyCode::PageUp, _) => { self.cursor = self.cursor.saturating_sub(10); OverlayOutcome::Stay }
            (KeyCode::PageDown, _) => { self.cursor = self.cursor.saturating_add(10); OverlayOutcome::Stay }
            (KeyCode::Home, _) => { self.cursor = 0; OverlayOutcome::Stay }
            (KeyCode::Backspace, _) => {
                self.filter.pop();
                self.cursor = 0;
                OverlayOutcome::Stay
            }
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) => {
                self.filter.push(c);
                self.cursor = 0;
                OverlayOutcome::Stay
            }
            _ => OverlayOutcome::Stay,
        }
    }

    fn render(&self, _width: u16, _height: u16) -> OverlayFrame {
        let entries = self.visible_entries();
        let total = entries.len();
        let mut body = Vec::new();
        let title = if self.filter.is_empty() {
            "Help".to_string()
        } else {
            format!("Help ({} matches for \"{}\")", total, self.filter)
        };
        body.push(title);
        body.push(String::new());

        // Compute the key column width once.
        let key_col = entries.iter()
            .map(|e| self.display_keys(e).join(" / ").chars().count())
            .max()
            .unwrap_or(0)
            .min(30);

        // Walk Category::ORDER, emit a header line then entries in that
        // category that are in `entries`.
        for cat in Category::ORDER {
            let cat_entries: Vec<&KeyEntry> = entries.iter()
                .copied()
                .filter(|e| e.category == *cat)
                .collect();
            if cat_entries.is_empty() { continue; }
            body.push(String::new());
            body.push(cat.label().to_string());
            for e in &cat_entries {
                let keys_str = self.display_keys(e).join(" / ");
                body.push(format!("  {keys_str:<key_col$}  {desc}", desc = e.description));
            }
        }

        let status = "[filter]  \u{2191}\u{2193} Esc".to_string();
        OverlayFrame { body, status }
    }

    fn title(&self) -> Cow<'_, str> { Cow::Borrowed("Help") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn help() -> HelpOverlay { HelpOverlay::new(HashMap::new()) }

    #[test]
    fn esc_closes_when_filter_empty() {
        let mut h = help();
        let out = h.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(out, OverlayOutcome::Close));
    }

    #[test]
    fn esc_clears_filter_first() {
        let mut h = help();
        h.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        let out = h.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(out, OverlayOutcome::Stay));
        assert_eq!(h.filter, "");
    }

    #[test]
    fn filter_matches_description_substring() {
        let mut h = help();
        h.handle_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE));
        h.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        h.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        let entries = h.visible_entries();
        assert!(entries.iter().any(|e| e.command_name == "mark-set"));
        assert!(entries.iter().any(|e| e.command_name == "mark-jump"));
        assert!(!entries.iter().any(|e| e.command_name == "scroll-down"));
    }

    #[test]
    fn user_remap_replaces_default_keys() {
        let mut remaps = HashMap::new();
        remaps.insert("scroll-down".to_string(), vec!["F3".to_string(), "Space".to_string()]);
        let h = HelpOverlay::new(remaps);
        let entry = KEY_REGISTRY.iter().find(|e| e.command_name == "scroll-down").unwrap();
        let displayed = h.display_keys(entry);
        assert_eq!(displayed, vec!["F3", "Space"]);
    }

    #[test]
    fn render_includes_category_headers_in_fixed_order() {
        let h = help();
        let frame = h.render(80, 30);
        // Find the row indices of each category label.
        let positions: Vec<usize> = Category::ORDER.iter()
            .map(|c| frame.body.iter().position(|l| l == c.label()).unwrap_or(usize::MAX))
            .collect();
        // Strictly ascending (with usize::MAX guard for any category missing).
        for w in positions.windows(2) {
            assert!(w[0] < w[1], "categories out of order: {:?}", positions);
        }
    }

    #[test]
    fn render_filter_title_shows_matches() {
        let mut h = help();
        h.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        let frame = h.render(80, 30);
        assert!(frame.body[0].starts_with("Help ("), "title: {:?}", frame.body[0]);
        assert!(frame.body[0].contains("\"q\""));
    }
}
