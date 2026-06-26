//! Custom keybindings loaded from `~/.config/tess/keys.toml`.
//!
//! Schema:
//! ```toml
//! [bindings]
//! "j" = "scroll-down"
//! "f1" = "!git status"
//! ```

use std::collections::HashMap;

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

    /// Load keys from the global config dir (`/etc/tess/` or
    /// `$TESS_GLOBAL_CONFIG_DIR`) and from `~/.config/tess/keys.toml`,
    /// merging per individual binding key with local winning.
    ///
    /// Global parse errors warn on stderr and the global layer is treated as
    /// empty; local parse errors fail startup.
    pub fn load_layered() -> Result<Self, String> {
        let mut bindings: HashMap<String, String> = HashMap::new();

        // Global layer.
        if let Some(dir) = crate::config_path::global_config_dir() {
            let path = dir.join("keys.toml");
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        match toml::from_str::<KeysConfig>(&text) {
                            Ok(cfg) => {
                                for (k, v) in cfg.bindings {
                                    bindings.insert(k, v);
                                }
                            }
                            Err(e) => eprintln!(
                                "tess: warning: keys.toml: {}: {e}; ignoring global config",
                                path.display()
                            ),
                        }
                    }
                    Err(e) => eprintln!(
                        "tess: warning: keys.toml: {}: {e}; ignoring global config",
                        path.display()
                    ),
                }
            }
        }

        // Local layer.
        if let Some(dir) = crate::config_path::user_config_dir() {
            let path = dir.join("keys.toml");
            if path.exists() {
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| format!("keys.toml: reading {}: {e}", path.display()))?;
                let cfg: KeysConfig = toml::from_str(&text)
                    .map_err(|e| format!("keys.toml: parsing {}: {e}", path.display()))?;
                for (k, v) in cfg.bindings {
                    bindings.insert(k, v);
                }
            }
        }

        // Build the final KeyMap from the merged bindings table.
        let mut map = HashMap::with_capacity(bindings.len());
        for (key_spec, action) in bindings {
            let key = parse_key_spec(&key_spec)
                .map_err(|e| format!("keys.toml: '{key_spec}': {e}"))?;
            reject_forbidden_key(&key, &key_spec)
                .map_err(|e| format!("keys.toml: {e}"))?;
            let target = parse_action(&action)
                .map_err(|e| format!("keys.toml: '{key_spec}': {e}"))?;
            map.insert(key, target);
        }
        Ok(Self { map })
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

    /// Group user bindings by the kebab-case command name they target.
    /// Shell-target bindings are excluded. Used by the help overlay to
    /// replace default key displays with the user's chosen keys.
    pub fn user_keys_by_command_name(&self) -> std::collections::HashMap<String, Vec<String>> {
        let mut out: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for (key, target) in &self.map {
            let BindingTarget::Command(cmd) = target else { continue };
            let Some(name) = command_to_kebab(cmd) else { continue };
            out.entry(name.to_string())
                .or_default()
                .push(format_key_event(*key));
        }
        out
    }
}

#[derive(Debug, Deserialize, Default)]
struct KeysConfig {
    #[serde(default)]
    bindings: HashMap<String, String>,
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
        "hscroll-left" => Some(Command::HScrollLeft),
        "hscroll-right" => Some(Command::HScrollRight),
        "hscroll-left-step" => Some(Command::HScrollLeftStep),
        "hscroll-right-step" => Some(Command::HScrollRightStep),
        "clipboard-yank-line" => Some(Command::YankLine),
        "anim-pause" => Some(Command::AnimPause),
        "anim-step-forward" => Some(Command::AnimStepForward),
        "anim-step-back" => Some(Command::AnimStepBack),
        "anim-restart" => Some(Command::AnimRestart),
        "focus-other-pane" => Some(Command::FocusOtherPane),
        "focus-prev-pane" => Some(Command::FocusPrevPane),
        "scroll-lock-toggle" => Some(Command::ToggleScrollLock),
        "zoom-pane" => Some(Command::ZoomPane),
        "mouse-toggle" => Some(Command::ToggleMouse),
        "diff-next-change" => Some(Command::DiffNextChange),
        "diff-prev-change" => Some(Command::DiffPrevChange),
        _ => None,
    }
}

/// Inverse of `command_from_kebab` — covers the same command set.
fn command_to_kebab(cmd: &Command) -> Option<&'static str> {
    match cmd {
        Command::ScrollLines(1) => Some("scroll-down"),
        Command::ScrollLines(-1) => Some("scroll-up"),
        Command::ScrollLogicalLines(1) => Some("scroll-logical-down"),
        Command::ScrollLogicalLines(-1) => Some("scroll-logical-up"),
        Command::PageDown => Some("page-down"),
        Command::PageUp => Some("page-up"),
        Command::HalfPageDown => Some("half-page-down"),
        Command::HalfPageUp => Some("half-page-up"),
        Command::Quit => Some("quit"),
        Command::Refresh => Some("refresh"),
        Command::Reload => Some("reload"),
        Command::ToggleLineNumbers => Some("toggle-line-numbers"),
        Command::ToggleChop => Some("toggle-chop"),
        Command::ToggleFollow => Some("toggle-follow"),
        Command::TogglePrettify => Some("toggle-prettify"),
        Command::SearchForward => Some("search-forward"),
        Command::SearchBackward => Some("search-backward"),
        Command::NextMatch => Some("next-match"),
        Command::PreviousMatch => Some("previous-match"),
        Command::OptionPrefix => Some("option-prefix"),
        Command::GotoLine => Some("goto-line"),
        Command::GotoRecord => Some("goto-record"),
        Command::GotoPercent => Some("goto-percent"),
        Command::MarkSet => Some("mark-set"),
        Command::MarkJump => Some("mark-jump"),
        Command::CtrlXPrefix => Some("ctrl-x-prefix"),
        Command::JumpPrevious => Some("jump-previous"),
        Command::ShellEscape => Some("shell-escape"),
        Command::Cancel => Some("cancel"),
        Command::HScrollLeft => Some("hscroll-left"),
        Command::HScrollRight => Some("hscroll-right"),
        Command::HScrollLeftStep => Some("hscroll-left-step"),
        Command::HScrollRightStep => Some("hscroll-right-step"),
        Command::YankLine => Some("clipboard-yank-line"),
        Command::AnimPause => Some("anim-pause"),
        Command::AnimStepForward => Some("anim-step-forward"),
        Command::AnimStepBack => Some("anim-step-back"),
        Command::AnimRestart => Some("anim-restart"),
        Command::FocusOtherPane => Some("focus-other-pane"),
        Command::FocusPrevPane => Some("focus-prev-pane"),
        Command::ToggleScrollLock => Some("scroll-lock-toggle"),
        Command::ZoomPane => Some("zoom-pane"),
        Command::ToggleMouse => Some("mouse-toggle"),
        Command::DiffNextChange => Some("diff-next-change"),
        Command::DiffPrevChange => Some("diff-prev-change"),
        _ => None,
    }
}

fn format_key_event(ke: KeyEvent) -> String {
    let ctrl  = ke.modifiers.contains(KeyModifiers::CONTROL);
    let alt   = ke.modifiers.contains(KeyModifiers::ALT);
    let shift = ke.modifiers.contains(KeyModifiers::SHIFT);

    // Convention: SHIFT-alone on an ASCII letter displays as the uppercase
    // letter with no "Shift-" prefix (matches KEY_REGISTRY's `"J"` form).
    if shift && !ctrl && !alt {
        if let KeyCode::Char(c) = ke.code {
            if c.is_ascii_alphabetic() {
                return c.to_ascii_uppercase().to_string();
            }
        }
    }

    let mut parts: Vec<&'static str> = Vec::new();
    if ctrl  { parts.push("Ctrl"); }
    if alt   { parts.push("Alt"); }
    if shift { parts.push("Shift"); }

    let key = match ke.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c)   => c.to_string(),
        KeyCode::F(n)      => format!("F{n}"),
        KeyCode::Esc       => "Esc".into(),
        KeyCode::Enter     => "Enter".into(),
        KeyCode::Tab       => "Tab".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Up        => "\u{2191}".into(),
        KeyCode::Down      => "\u{2193}".into(),
        KeyCode::Left      => "\u{2190}".into(),
        KeyCode::Right     => "\u{2192}".into(),
        KeyCode::Home      => "Home".into(),
        KeyCode::End       => "End".into(),
        KeyCode::PageUp    => "PgUp".into(),
        KeyCode::PageDown  => "PgDn".into(),
        other              => format!("{other:?}"),
    };
    if parts.is_empty() { key } else { format!("{}-{}", parts.join("-"), key) }
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
    fn user_remaps_by_command_name_groups_keys() {
        let toml = r#"
[bindings]
"f3" = "scroll-down"
"f4" = "scroll-down"
"f5" = "quit"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let groups = m.user_keys_by_command_name();
        let mut down = groups.get("scroll-down").cloned().unwrap_or_default();
        down.sort();
        assert_eq!(down, vec!["F3".to_string(), "F4".to_string()]);
        assert_eq!(groups.get("quit").cloned().unwrap_or_default(), vec!["F5".to_string()]);
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

    #[test]
    fn format_key_event_renders_modifier_combos() {
        // Ctrl alone
        assert_eq!(
            format_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)),
            "Ctrl-r",
        );
        // Shift alone on a letter → uppercase, no prefix
        assert_eq!(
            format_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::SHIFT)),
            "J",
        );
        // Shift alone on a non-letter (e.g. Tab) → "Shift-Tab"
        assert_eq!(
            format_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)),
            "Shift-Tab",
        );
        // Plain lowercase letter → "j" (no uppercasing)
        assert_eq!(
            format_key_event(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
            "j",
        );
        // Function key
        assert_eq!(
            format_key_event(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
            "F3",
        );
        // Ctrl+Shift+letter — composed prefix, lowercase stored char
        assert_eq!(
            format_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL | KeyModifiers::SHIFT)),
            "Ctrl-Shift-x",
        );
    }

    #[test]
    fn hscroll_names_resolve_to_commands() {
        let toml = r#"
[bindings]
"f6" = "hscroll-right"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let f6 = KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE);
        assert!(matches!(m.lookup(&f6), Some(BindingTarget::Command(Command::HScrollRight))));

        assert!(matches!(command_from_kebab("hscroll-left"), Some(Command::HScrollLeft)));
        assert!(matches!(command_from_kebab("hscroll-left-step"), Some(Command::HScrollLeftStep)));
        assert!(matches!(command_from_kebab("hscroll-right-step"), Some(Command::HScrollRightStep)));
    }

    #[test]
    fn command_kebab_round_trip() {
        // Every name in command_from_kebab must round-trip through command_to_kebab.
        let names = [
            "scroll-down", "scroll-up", "scroll-logical-down", "scroll-logical-up",
            "page-down", "page-up", "half-page-down", "half-page-up",
            "quit", "refresh", "reload",
            "toggle-line-numbers", "toggle-chop", "toggle-follow", "toggle-prettify",
            "search-forward", "search-backward", "next-match", "previous-match",
            "option-prefix", "goto-line", "goto-record", "goto-percent",
            "mark-set", "mark-jump", "ctrl-x-prefix", "jump-previous",
            "shell-escape", "cancel",
            "hscroll-left", "hscroll-right", "hscroll-left-step", "hscroll-right-step",
            "clipboard-yank-line",
            "anim-pause", "anim-step-forward", "anim-step-back", "anim-restart",
            "focus-other-pane", "focus-prev-pane", "scroll-lock-toggle", "zoom-pane", "mouse-toggle",
            "diff-next-change", "diff-prev-change",
        ];
        for name in &names {
            let cmd = command_from_kebab(name).expect(&format!("from_kebab failed for {name}"));
            let back = command_to_kebab(&cmd).expect(&format!("to_kebab failed for {name}"));
            assert_eq!(back, *name, "round-trip mismatch for {name}");
        }
    }

    #[test]
    fn binds_scroll_lock_toggle_from_config() {
        let m = KeyMap::load_from_str("[bindings]\n\"=\" = \"scroll-lock-toggle\"\n").unwrap();
        let ev = KeyEvent::new(KeyCode::Char('='), KeyModifiers::NONE);
        assert!(matches!(
            m.lookup(&ev),
            Some(BindingTarget::Command(Command::ToggleScrollLock))
        ));
    }

    #[test]
    fn shell_bindings_are_excluded_from_user_keys() {
        let toml = r#"
[bindings]
"f2" = "!git status"
"f3" = "scroll-down"
"#;
        let m = KeyMap::load_from_str(toml).unwrap();
        let groups = m.user_keys_by_command_name();
        assert!(!groups.values().any(|v| v.contains(&"F2".to_string())),
                "shell-bound F2 should not appear: {groups:?}");
        assert_eq!(groups.get("scroll-down").cloned().unwrap_or_default(), vec!["F3".to_string()]);
    }

    #[test]
    fn layered_keys_local_overrides_global_per_binding() {
        let _guard = crate::test_env::lock();
        let prev_home = std::env::var_os("HOME");
        let prev_global = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");

        let home = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();

        std::env::set_var("HOME", home.path());
        std::env::set_var("TESS_GLOBAL_CONFIG_DIR", global.path());

        std::fs::write(
            global.path().join("keys.toml"),
            r#"
[bindings]
"j" = "scroll-down"
"k" = "scroll-up"
"#,
        )
        .unwrap();

        let cfg_dir = home.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("keys.toml"),
            r#"
[bindings]
"j" = "page-down"
"#,
        )
        .unwrap();

        let km = KeyMap::load_layered().unwrap();

        // j: local wins (page-down)
        let j = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::NONE,
        );
        match km.lookup(&j) {
            Some(BindingTarget::Command(cmd)) => {
                let dbg = format!("{cmd:?}");
                assert!(dbg.to_lowercase().contains("page"), "got: {dbg}");
            }
            other => panic!("expected Command(PageDown), got {other:?}"),
        }

        // k: global survives (scroll-up)
        let k = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('k'),
            crossterm::event::KeyModifiers::NONE,
        );
        match km.lookup(&k) {
            Some(BindingTarget::Command(cmd)) => {
                assert!(matches!(cmd, Command::ScrollLines(n) if *n < 0), "got: {cmd:?}");
            }
            other => panic!("expected Command(ScrollLines(-1)), got {other:?}"),
        }

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_global {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }

    #[test]
    fn layered_keys_warns_on_bad_global() {
        let _guard = crate::test_env::lock();
        let prev_home = std::env::var_os("HOME");
        let prev_global = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");

        let home = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();

        std::env::set_var("HOME", home.path());
        std::env::set_var("TESS_GLOBAL_CONFIG_DIR", global.path());

        std::fs::write(
            global.path().join("keys.toml"),
            "= = not valid",
        )
        .unwrap();

        // Local missing — should still succeed.
        let km = KeyMap::load_layered().unwrap();
        assert!(km.is_empty());

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_global {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }
}
