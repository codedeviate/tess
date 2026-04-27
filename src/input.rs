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
        (Char('-'), false) => Command::Noop, // -N / -S need a follow-up letter; handled in app loop later
        (Char('N'), false) => Command::ToggleLineNumbers,
        (Char('S'), false) => Command::ToggleChop,
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
    fn capital_n_toggles_line_numbers() {
        assert_eq!(translate(key(KeyCode::Char('N'), KeyModifiers::SHIFT)), Command::ToggleLineNumbers);
    }

    #[test]
    fn resize_event() {
        assert_eq!(translate(Event::Resize(80, 24)), Command::Resize(80, 24));
    }
}
