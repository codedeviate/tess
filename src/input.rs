use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ScrollLines(i64),
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    GoTop,
    GoBottom,
    Quit,
    Resize(u16, u16),
    Refresh,
    ToggleLineNumbers,
    ToggleChop,
    ToggleFollow,
    /// `/` — open the forward-search prompt.
    SearchForward,
    /// `?` — open the backward-search prompt.
    SearchBackward,
    /// `n` — repeat the last search in its original direction.
    NextMatch,
    /// `N` — repeat the last search in the opposite direction.
    PreviousMatch,
    /// `-` — option-toggle prefix: the next key chooses an option to flip
    /// (`N` → line numbers, `S` → chop, `F` → follow).
    OptionPrefix,
    /// `R` — force-reload the source from disk now (only meaningful with
    /// `--live`; no-op for static file sources and append-streaming follow).
    Reload,
    Noop,
}

pub fn translate(event: Event) -> Command {
    match event {
        Event::Resize(c, r) => Command::Resize(c, r),
        Event::Key(KeyEvent { code, modifiers, .. }) => translate_key(code, modifiers),
        _ => Command::Noop,
    }
}

fn translate_key(code: KeyCode, mods: KeyModifiers) -> Command {
    use KeyCode::*;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    match (code, ctrl) {
        (Char('q'), false) | (Char('Q'), false) => Command::Quit,
        (Char('c'), true) => Command::Quit,
        (Down, _) | (Char('j'), false) | (Char('e'), false) | (Char('e'), true) | (Enter, _) => Command::ScrollLines(1),
        (Char('y'), false) | (Char('y'), true) | (Up, _) | (Char('k'), false) => Command::ScrollLines(-1),
        (Char(' '), false) | (Char('f'), false) | (Char('f'), true) | (PageDown, _) => Command::PageDown,
        (Char('b'), false) | (Char('b'), true) | (PageUp, _) => Command::PageUp,
        (Char('d'), false) | (Char('d'), true) => Command::HalfPageDown,
        (Char('u'), false) | (Char('u'), true) => Command::HalfPageUp,
        (Char('g'), false) | (Char('<'), false) | (Home, _) => Command::GoTop,
        (Char('G'), false) | (Char('>'), false) | (End, _) => Command::GoBottom,
        (Char('r'), false) | (Char('l'), true) => Command::Refresh,
        (Char('R'), false) => Command::Reload,
        (Char('-'), false) => Command::OptionPrefix,
        (Char('F'), false) => Command::ToggleFollow,
        (Char('/'), false) => Command::SearchForward,
        (Char('?'), false) => Command::SearchBackward,
        (Char('n'), false) => Command::NextMatch,
        (Char('N'), false) => Command::PreviousMatch,
        _ => Command::Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventKind, KeyEventState};

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent {
            code, modifiers: mods,
            kind: KeyEventKind::Press, state: KeyEventState::NONE,
        })
    }

    #[test]
    fn arrow_down_scrolls_one() {
        assert_eq!(translate(key(KeyCode::Down, KeyModifiers::NONE)), Command::ScrollLines(1));
    }

    #[test]
    fn j_scrolls_one() {
        assert_eq!(translate(key(KeyCode::Char('j'), KeyModifiers::NONE)), Command::ScrollLines(1));
    }

    #[test]
    fn space_pages_down() {
        assert_eq!(translate(key(KeyCode::Char(' '), KeyModifiers::NONE)), Command::PageDown);
    }

    #[test]
    fn ctrl_c_quits() {
        assert_eq!(translate(key(KeyCode::Char('c'), KeyModifiers::CONTROL)), Command::Quit);
    }

    #[test]
    fn capital_g_goes_to_bottom() {
        assert_eq!(translate(key(KeyCode::Char('G'), KeyModifiers::SHIFT)), Command::GoBottom);
    }

    #[test]
    fn capital_f_toggles_follow() {
        assert_eq!(translate(key(KeyCode::Char('F'), KeyModifiers::SHIFT)), Command::ToggleFollow);
    }

    #[test]
    fn lowercase_f_still_pages_down() {
        assert_eq!(translate(key(KeyCode::Char('f'), KeyModifiers::NONE)), Command::PageDown);
    }

    #[test]
    fn slash_opens_forward_search() {
        assert_eq!(translate(key(KeyCode::Char('/'), KeyModifiers::NONE)), Command::SearchForward);
    }

    #[test]
    fn question_mark_opens_backward_search() {
        // `?` arrives as Char('?') with SHIFT on most layouts.
        assert_eq!(translate(key(KeyCode::Char('?'), KeyModifiers::SHIFT)), Command::SearchBackward);
    }

    #[test]
    fn n_repeats_match_forward() {
        assert_eq!(translate(key(KeyCode::Char('n'), KeyModifiers::NONE)), Command::NextMatch);
    }

    #[test]
    fn capital_n_repeats_match_backward() {
        assert_eq!(translate(key(KeyCode::Char('N'), KeyModifiers::SHIFT)), Command::PreviousMatch);
    }

    #[test]
    fn capital_r_triggers_reload() {
        assert_eq!(translate(key(KeyCode::Char('R'), KeyModifiers::SHIFT)), Command::Reload);
    }

    #[test]
    fn lowercase_r_still_refreshes() {
        assert_eq!(translate(key(KeyCode::Char('r'), KeyModifiers::NONE)), Command::Refresh);
    }

    #[test]
    fn dash_is_option_prefix() {
        assert_eq!(translate(key(KeyCode::Char('-'), KeyModifiers::NONE)), Command::OptionPrefix);
    }

    #[test]
    fn resize_event() {
        assert_eq!(translate(Event::Resize(80, 24)), Command::Resize(80, 24));
    }
}
