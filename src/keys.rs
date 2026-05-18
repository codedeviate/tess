//! Custom keybindings loaded from `~/.config/tess/keys.toml`.
//!
//! Schema:
//! ```toml
//! [bindings]
//! "j" = "scroll-down"
//! "f1" = "!git status"
//! ```

use std::collections::HashMap;
use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;

use crate::input::Command;

#[derive(Debug, Clone)]
pub enum BindingTarget {
    Command(Command),
    Shell(String),
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    map: HashMap<KeyEvent, BindingTarget>,
}

impl KeyMap {
    pub fn empty() -> Self {
        Self { map: HashMap::new() }
    }

    /// Load from the default path `~/.config/tess/keys.toml`. Missing file
    /// is OK and returns an empty map. Returns an error if the file exists
    /// but can't be parsed.
    pub fn load_from_default_path() -> Result<Self, String> {
        let Some(path) = user_keys_path() else {
            return Ok(Self::empty());
        };
        if !path.exists() {
            return Ok(Self::empty());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("keys.toml: reading {}: {e}", path.display()))?;
        Self::load_from_str(&text)
            .map_err(|e| format!("keys.toml: {e}"))
    }

    pub fn load_from_str(toml_text: &str) -> Result<Self, String> {
        let cfg: KeysConfig = toml::from_str(toml_text)
            .map_err(|e| format!("parsing: {e}"))?;
        let mut map = HashMap::with_capacity(cfg.bindings.len());
        for (key_spec, action) in cfg.bindings {
            let key = parse_key_spec(&key_spec)
                .map_err(|e| format!("'{key_spec}': {e}"))?;
            reject_forbidden_key(&key, &key_spec)?;
            let target = parse_action(&action)
                .map_err(|e| format!("'{key_spec}': {e}"))?;
            map.insert(key, target);
        }
        Ok(Self { map })
    }

    pub fn lookup(&self, key: &KeyEvent) -> Option<&BindingTarget> {
        self.map.get(key)
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[derive(Debug, Deserialize, Default)]
struct KeysConfig {
    #[serde(default)]
    bindings: HashMap<String, String>,
}

fn user_keys_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        let mut p = PathBuf::from(h);
        p.push(".config");
        p.push("tess");
        p.push("keys.toml");
        p
    })
}

/// Parse a key spec string into a `KeyEvent`.
fn parse_key_spec(spec: &str) -> Result<KeyEvent, String> {
    let lower = spec.to_lowercase();
    let mut parts: Vec<&str> = lower.split('-').collect();
    if parts.is_empty() {
        return Err("empty key spec".to_string());
    }
    let key_part = parts.pop().unwrap();
    let mut modifiers = KeyModifiers::NONE;
    for m in &parts {
        if m.is_empty() {
            // Handle "ctrl--" (ctrl + dash). The trailing "-" became "".
            continue;
        }
        match *m {
            "ctrl" => modifiers |= KeyModifiers::CONTROL,
            "alt" => modifiers |= KeyModifiers::ALT,
            "shift" => modifiers |= KeyModifiers::SHIFT,
            other => return Err(format!("unknown modifier '{other}'")),
        }
    }
    let code = match key_part {
        "esc" => KeyCode::Esc,
        "enter" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "space" => KeyCode::Char(' '),
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "" => {
            // The trailing piece was empty, meaning the key is literal "-"
            // and the modifiers consumed everything else (e.g. "ctrl--").
            KeyCode::Char('-')
        }
        s if s.starts_with('f') && s.len() > 1 => {
            let n: u8 = s[1..].parse()
                .map_err(|_| format!("unknown key '{s}'"))?;
            KeyCode::F(n)
        }
        s if s.chars().count() == 1 => {
            // For a BARE letter with no modifiers, an uppercase letter
            // promotes to shift-prefix semantics ("J" == "shift-j").
            // When modifiers are already present (e.g. "ctrl-J"), the
            // user's intent is the literal letter — don't add SHIFT.
            let original_char = spec.chars().last().unwrap();
            if original_char.is_ascii_uppercase() && modifiers == KeyModifiers::NONE {
                modifiers |= KeyModifiers::SHIFT;
                KeyCode::Char(original_char.to_ascii_lowercase())
            } else {
                // Either lowercase letter, or uppercase with an explicit
                // modifier — lowercase the char either way for consistency.
                KeyCode::Char(original_char.to_ascii_lowercase())
            }
        }
        other => return Err(format!("unknown key '{other}'")),
    };
    Ok(KeyEvent::new(code, modifiers))
}

fn reject_forbidden_key(key: &KeyEvent, original_spec: &str) -> Result<(), String> {
    let forbidden = match (&key.code, key.modifiers) {
        (KeyCode::Char('m'), KeyModifiers::NONE) => true,
        (KeyCode::Char('\''), KeyModifiers::NONE) => true,
        (KeyCode::Char('-'), KeyModifiers::NONE) => true,
        (KeyCode::Char('x'), KeyModifiers::CONTROL) => true,
        (KeyCode::Char(c), KeyModifiers::NONE) if c.is_ascii_digit() => true,
        _ => false,
    };
    if forbidden {
        return Err(format!(
            "'{original_spec}' is part of a multi-key sequence and cannot be rebound"
        ));
    }
    Ok(())
}

fn parse_action(action: &str) -> Result<BindingTarget, String> {
    if let Some(shell_cmd) = action.strip_prefix('!') {
        if shell_cmd.is_empty() {
            return Err("shell binding requires a command after '!'".to_string());
        }
        return Ok(BindingTarget::Shell(shell_cmd.to_string()));
    }
    let cmd = command_from_kebab(action)
        .ok_or_else(|| format!("unknown command '{action}'"))?;
    Ok(BindingTarget::Command(cmd))
}

/// Map a kebab-case command name to the corresponding `Command` enum variant.
fn command_from_kebab(name: &str) -> Option<Command> {
    match name {
        "scroll-down" => Some(Command::ScrollLines(1)),
        "scroll-up" => Some(Command::ScrollLines(-1)),
        "scroll-logical-down" => Some(Command::ScrollLogicalLines(1)),
        "scroll-logical-up" => Some(Command::ScrollLogicalLines(-1)),
        "page-down" => Some(Command::PageDown),
        "page-up" => Some(Command::PageUp),
        "half-page-down" => Some(Command::HalfPageDown),
        "half-page-up" => Some(Command::HalfPageUp),
        "quit" => Some(Command::Quit),
        "refresh" => Some(Command::Refresh),
        "reload" => Some(Command::Reload),
        "toggle-line-numbers" => Some(Command::ToggleLineNumbers),
        "toggle-chop" => Some(Command::ToggleChop),
        "toggle-follow" => Some(Command::ToggleFollow),
        "toggle-prettify" => Some(Command::TogglePrettify),
        "search-forward" => Some(Command::SearchForward),
        "search-backward" => Some(Command::SearchBackward),
        "next-match" => Some(Command::NextMatch),
        "previous-match" => Some(Command::PreviousMatch),
        "option-prefix" => Some(Command::OptionPrefix),
        "goto-line" => Some(Command::GotoLine),
        "goto-record" => Some(Command::GotoRecord),
        "goto-percent" => Some(Command::GotoPercent),
        "mark-set" => Some(Command::MarkSet),
        "mark-jump" => Some(Command::MarkJump),
        "ctrl-x-prefix" => Some(Command::CtrlXPrefix),
        "jump-previous" => Some(Command::JumpPrevious),
        "shell-escape" => Some(Command::ShellEscape),
        "cancel" => Some(Command::Cancel),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_file_returns_empty_map() {
        let m = KeyMap::load_from_str("").unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn parse_single_binding() {
        let toml = r#"
[bindings]
"j" = "scroll-down"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(m.lookup(&key), Some(BindingTarget::Command(Command::ScrollLines(1)))));
    }

    #[test]
    fn parse_named_special_key() {
        let toml = r#"
[bindings]
"f1" = "toggle-line-numbers"
"esc" = "cancel"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let f1 = KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(matches!(m.lookup(&f1), Some(BindingTarget::Command(Command::ToggleLineNumbers))));
        assert!(matches!(m.lookup(&esc), Some(BindingTarget::Command(Command::Cancel))));
    }

    #[test]
    fn parse_modifier_combinations() {
        let toml = r#"
[bindings]
"ctrl-r" = "reload"
"shift-tab" = "scroll-logical-up"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        let shift_tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT);
        assert!(matches!(m.lookup(&ctrl_r), Some(BindingTarget::Command(Command::Reload))));
        assert!(matches!(m.lookup(&shift_tab), Some(BindingTarget::Command(Command::ScrollLogicalLines(-1)))));
    }

    #[test]
    fn case_letter_resolves_to_shift_prefix() {
        let toml = r#"
[bindings]
"J" = "scroll-logical-down"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let shift_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::SHIFT);
        assert!(matches!(m.lookup(&shift_j), Some(BindingTarget::Command(Command::ScrollLogicalLines(1)))));
    }

    #[test]
    fn forbidden_keys_error_at_parse() {
        for key in &["m", "'", "-", "ctrl-x", "0", "5", "9"] {
            let toml = format!(r#"
[bindings]
"{key}" = "quit"
"#);
            let err = KeyMap::load_from_str(&toml).unwrap_err();
            assert!(err.contains("multi-key sequence"),
                    "key '{key}' should be forbidden: {err}");
        }
    }

    #[test]
    fn unknown_command_name_errors() {
        let toml = r#"
[bindings]
"j" = "definitely-not-a-real-command"
"#;
        let err = KeyMap::load_from_str(toml).unwrap_err();
        assert!(err.contains("unknown command"));
    }

    #[test]
    fn empty_shell_binding_errors() {
        let toml = r#"
[bindings]
"f1" = "!"
"#;
        let err = KeyMap::load_from_str(toml).unwrap_err();
        assert!(err.contains("requires a command"));
    }

    #[test]
    fn parse_inline_shell_binding() {
        let toml = r#"
[bindings]
"f2" = "!git status"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let f2 = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        match m.lookup(&f2) {
            Some(BindingTarget::Shell(cmd)) => assert_eq!(cmd, "git status"),
            other => panic!("expected Shell, got {:?}", other),
        }
    }

    #[test]
    fn lookup_returns_none_for_unbound_key() {
        let toml = r#"
[bindings]
"j" = "scroll-down"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let other = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(m.lookup(&other).is_none());
    }

    #[test]
    fn ctrl_uppercase_letter_does_not_add_shift() {
        // "ctrl-J" should be Ctrl + 'j', NOT Ctrl + Shift + 'j'.
        let toml = r#"
[bindings]
"ctrl-J" = "reload"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let ctrl_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert!(matches!(m.lookup(&ctrl_j), Some(BindingTarget::Command(Command::Reload))),
                "ctrl-J should resolve to Ctrl+j without Shift");
        let ctrl_shift_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert!(m.lookup(&ctrl_shift_j).is_none(),
                "ctrl-J should NOT also match Ctrl+Shift+j");
    }

    #[test]
    fn dash_with_modifier_is_a_real_key() {
        // "ctrl--" should resolve to Ctrl + '-' (a valid bind).
        // (Bare "-" is forbidden by reject_forbidden_key, but Ctrl-- isn't.)
        let toml = r#"
[bindings]
"ctrl--" = "refresh"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let ctrl_dash = KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL);
        assert!(matches!(m.lookup(&ctrl_dash), Some(BindingTarget::Command(Command::Refresh))));
    }
}
