//! Static registry of every default keybinding, with category and human-
//! readable description. Used by the help overlay; not the runtime dispatcher
//! (that's `input::translate`). The `registry_matches_translate` test in this
//! module enforces that the two don't drift.
//!
//! User remaps from `~/.config/tess/keys.toml` are layered on top in the help
//! overlay via `KeyMap::user_keys_by_command_name`.

#[cfg(test)]
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::input::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Movement,
    Search,
    Files,
    Marks,
    Tags,
    Misc,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Movement => "Movement",
            Category::Search => "Search",
            Category::Files => "Files",
            Category::Marks => "Marks",
            Category::Tags => "Tags",
            Category::Misc => "Misc",
        }
    }

    /// Render order in the help overlay.
    pub const ORDER: &'static [Category] = &[
        Category::Movement,
        Category::Search,
        Category::Files,
        Category::Marks,
        Category::Tags,
        Category::Misc,
    ];
}

#[derive(Debug)]
pub struct KeyEntry {
    /// Human-readable display strings, e.g. `&["j", "↓", "Enter"]`. The first
    /// entry is the canonical one shown when help has no extra room.
    pub keys: &'static [&'static str],
    pub category: Category,
    pub description: &'static str,
    pub command: Command,
    /// Stable name used for `keys.toml` remapping (kebab-case).
    pub command_name: &'static str,
}

/// Every default binding. The order within a category is the order shown
/// in the help overlay.
pub static KEY_REGISTRY: &[KeyEntry] = &[
    // ── Movement ───────────────────────────────────────────────
    KeyEntry {
        keys: &["j", "↓", "e", "Enter"],
        category: Category::Movement,
        description: "scroll down one line",
        command: Command::ScrollLines(1),
        command_name: "scroll-down",
    },
    KeyEntry {
        keys: &["k", "↑", "y"],
        category: Category::Movement,
        description: "scroll up one line",
        command: Command::ScrollLines(-1),
        command_name: "scroll-up",
    },
    KeyEntry {
        keys: &["J"],
        category: Category::Movement,
        description: "next logical line (skip wrap rows)",
        command: Command::ScrollLogicalLines(1),
        command_name: "scroll-logical-down",
    },
    KeyEntry {
        keys: &["K"],
        category: Category::Movement,
        description: "previous logical line",
        command: Command::ScrollLogicalLines(-1),
        command_name: "scroll-logical-up",
    },
    KeyEntry {
        keys: &["Space", "f", "Ctrl-F", "PgDn"],
        category: Category::Movement,
        description: "page down",
        command: Command::PageDown,
        command_name: "page-down",
    },
    KeyEntry {
        keys: &["b", "Ctrl-B", "PgUp"],
        category: Category::Movement,
        description: "page up",
        command: Command::PageUp,
        command_name: "page-up",
    },
    KeyEntry {
        keys: &["d", "Ctrl-D"],
        category: Category::Movement,
        description: "half page down",
        command: Command::HalfPageDown,
        command_name: "half-page-down",
    },
    KeyEntry {
        keys: &["u", "Ctrl-U"],
        category: Category::Movement,
        description: "half page up",
        command: Command::HalfPageUp,
        command_name: "half-page-up",
    },
    KeyEntry {
        keys: &["g", "<", "Home"],
        category: Category::Movement,
        description: "jump to first line (or line N with prefix)",
        command: Command::GotoLine,
        command_name: "goto-line",
    },
    KeyEntry {
        keys: &["G", ">", "End"],
        category: Category::Movement,
        description: "jump to last line (or record N with prefix)",
        command: Command::GotoRecord,
        command_name: "goto-record",
    },
    KeyEntry {
        keys: &["%"],
        category: Category::Movement,
        description: "jump to N% through file",
        command: Command::GotoPercent,
        command_name: "goto-percent",
    },

    // ── Search ────────────────────────────────────────────────
    KeyEntry {
        keys: &["/"],
        category: Category::Search,
        description: "search forward",
        command: Command::SearchForward,
        command_name: "search-forward",
    },
    KeyEntry {
        keys: &["?"],
        category: Category::Search,
        description: "search backward",
        command: Command::SearchBackward,
        command_name: "search-backward",
    },
    KeyEntry {
        keys: &["n"],
        category: Category::Search,
        description: "next match",
        command: Command::NextMatch,
        command_name: "next-match",
    },
    KeyEntry {
        keys: &["N"],
        category: Category::Search,
        description: "previous match",
        command: Command::PreviousMatch,
        command_name: "previous-match",
    },

    // ── Files ─────────────────────────────────────────────────
    KeyEntry {
        keys: &[":n"],
        category: Category::Files,
        description: "next file",
        command: Command::ColonPrompt,  // surfaced through colon parser
        command_name: "next-file",
    },
    KeyEntry {
        keys: &[":p"],
        category: Category::Files,
        description: "previous file",
        command: Command::ColonPrompt,
        command_name: "prev-file",
    },
    KeyEntry {
        keys: &[":b", ":buffers"],
        category: Category::Files,
        description: "open file picker",
        command: Command::OpenPicker,
        command_name: "open-picker",
    },
    KeyEntry {
        keys: &[":e PATH"],
        category: Category::Files,
        description: "open a new file (add to set)",
        command: Command::ColonPrompt,
        command_name: "edit-file",
    },
    KeyEntry {
        keys: &[":d"],
        category: Category::Files,
        description: "drop current file from set",
        command: Command::ColonPrompt,
        command_name: "drop-file",
    },
    KeyEntry {
        keys: &[":x"],
        category: Category::Files,
        description: "jump to first file",
        command: Command::ColonPrompt,
        command_name: "first-file",
    },
    KeyEntry {
        keys: &[":t"],
        category: Category::Files,
        description: "jump to last file",
        command: Command::ColonPrompt,
        command_name: "last-file",
    },

    // ── Marks ─────────────────────────────────────────────────
    KeyEntry {
        keys: &["m<a-z>"],
        category: Category::Marks,
        description: "set mark to current position",
        command: Command::MarkSet,
        command_name: "mark-set",
    },
    KeyEntry {
        keys: &["'<a-z>"],
        category: Category::Marks,
        description: "jump to mark",
        command: Command::MarkJump,
        command_name: "mark-jump",
    },
    // Two-key chord: first Ctrl-X emits CtrlXPrefix; the second Ctrl-X dispatches JumpPrevious.
    // The registry records the final intent (JumpPrevious) as that's what users see in help.
    KeyEntry {
        keys: &["Ctrl-X Ctrl-X"],
        category: Category::Marks,
        description: "jump to previous position",
        command: Command::JumpPrevious,
        command_name: "jump-previous",
    },

    // ── Tags ──────────────────────────────────────────────────
    KeyEntry {
        keys: &["Ctrl-]"],
        category: Category::Tags,
        description: "jump to tag (prompts for name)",
        command: Command::TagPrompt,
        command_name: "tag-prompt",
    },
    KeyEntry {
        keys: &["Ctrl-T"],
        category: Category::Tags,
        description: "pop tag stack",
        command: Command::TagPop,
        command_name: "tag-pop",
    },

    // ── Misc ──────────────────────────────────────────────────
    KeyEntry {
        keys: &["q", "Q", "Ctrl-C"],
        category: Category::Misc,
        description: "quit",
        command: Command::Quit,
        command_name: "quit",
    },
    KeyEntry {
        keys: &["r", "Ctrl-L"],
        category: Category::Misc,
        description: "refresh screen",
        command: Command::Refresh,
        command_name: "refresh",
    },
    KeyEntry {
        keys: &["R"],
        category: Category::Misc,
        description: "reload source from disk",
        command: Command::Reload,
        command_name: "reload",
    },
    KeyEntry {
        keys: &["F"],
        category: Category::Misc,
        description: "toggle follow mode",
        command: Command::ToggleFollow,
        command_name: "toggle-follow",
    },
    KeyEntry {
        keys: &["P"],
        category: Category::Misc,
        description: "toggle prettify",
        command: Command::TogglePrettify,
        command_name: "toggle-prettify",
    },
    KeyEntry {
        keys: &["-"],
        category: Category::Misc,
        description: "option-toggle prefix (N=lines, S=chop, F=follow)",
        command: Command::OptionPrefix,
        command_name: "option-prefix",
    },
    KeyEntry {
        keys: &["!"],
        category: Category::Misc,
        description: "shell escape (run external command)",
        command: Command::ShellEscape,
        command_name: "shell-escape",
    },
    KeyEntry {
        keys: &[":"],
        category: Category::Misc,
        description: "colon command prompt",
        command: Command::ColonPrompt,
        command_name: "colon-prompt",
    },
    KeyEntry {
        keys: &["0", "1-9"],
        category: Category::Misc,
        description: "numeric prefix (e.g. 5G jumps to record 5)",
        command: Command::Digit(0),
        command_name: "digit-prefix",
    },
    KeyEntry {
        keys: &["Esc"],
        category: Category::Misc,
        description: "cancel pending numeric prefix or command",
        command: Command::Cancel,
        command_name: "cancel",
    },
    KeyEntry {
        keys: &[":help", ":h", "F1"],
        category: Category::Misc,
        description: "open this help overlay",
        command: Command::OpenHelp,
        command_name: "open-help",
    },
];

/// Parse the first entry of `keys` into a `KeyEvent`. Used by the sync-check
/// test. Returns None for entries that don't represent a single keystroke
/// (e.g. colon commands like ":n", mark sequences like "m<a-z>").
#[cfg(test)]
fn parse_canonical_key(spec: &str) -> Option<KeyEvent> {
    if spec.starts_with(':') || spec.contains(' ') || spec.contains('<') {
        return None;
    }
    let lower = spec.to_lowercase();
    let mut parts: Vec<&str> = lower.split('-').collect();
    let key_part = parts.pop()?;
    let mut modifiers = KeyModifiers::NONE;
    for m in &parts {
        match *m {
            "ctrl" => modifiers |= KeyModifiers::CONTROL,
            "alt" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            _ => return None,
        }
    }
    let code = match key_part {
        "esc" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "space" => KeyCode::Char(' '),
        "↑" | "up" => KeyCode::Up,
        "↓" | "down" => KeyCode::Down,
        "←" | "left" => KeyCode::Left,
        "→" | "right" => KeyCode::Right,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        s if s.starts_with('f') && s.len() > 1 => {
            let n: u8 = s[1..].parse().ok()?;
            KeyCode::F(n)
        }
        s if s.chars().count() == 1 => {
            let ch = spec.chars().last()?;
            if ch.is_ascii_uppercase() && modifiers == KeyModifiers::NONE {
                modifiers |= KeyModifiers::SHIFT;
                KeyCode::Char(ch)  // keep uppercase: crossterm emits Char('J') + SHIFT, not Char('j') + SHIFT
            } else {
                KeyCode::Char(ch.to_ascii_lowercase())
            }
        }
        _ => return None,
    };
    Some(KeyEvent::new(code, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::Event;
    use std::collections::HashSet;

    #[test]
    fn command_names_are_unique() {
        let mut seen = HashSet::new();
        for entry in KEY_REGISTRY {
            assert!(
                seen.insert(entry.command_name),
                "duplicate command_name in KEY_REGISTRY: {}",
                entry.command_name,
            );
        }
    }

    #[test]
    fn registry_matches_translate_for_single_key_entries() {
        for entry in KEY_REGISTRY {
            for &key in entry.keys {
                let Some(ke) = parse_canonical_key(key) else { continue };
                let cmd = crate::input::translate(Event::Key(ke));
                // For entries whose canonical command isn't reachable through
                // translate() (e.g. open-help via F1 is reachable, open-picker
                // via :b is NOT — :b goes through the colon parser), the test
                // skips when translate returns Noop AND the key spec starts
                // with ':'. For non-colon entries we require a match.
                if key.starts_with(':') { continue; }
                assert_eq!(
                    cmd, entry.command,
                    "registry/translate drift: key={:?} entry={:?} \
                     translate returned {:?} but registry says {:?}",
                    key, entry.command_name, cmd, entry.command,
                );
            }
        }
    }

    #[test]
    fn every_category_has_at_least_one_entry() {
        for cat in Category::ORDER {
            assert!(
                KEY_REGISTRY.iter().any(|e| e.category == *cat),
                "no entries in category {:?}",
                cat,
            );
        }
    }
}
