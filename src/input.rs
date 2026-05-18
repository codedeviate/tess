use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::prettify::PrettifyMode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ScrollLines(i64),
    /// `J` / `K` — jump forward or backward by one whole logical line,
    /// skipping any remaining wrap rows of the current line. Useful for
    /// long lines that wrap many screen rows.
    ScrollLogicalLines(i64),
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
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
    /// `Shift-P` — toggle pretty-printing on/off (cycles back to the last
    /// active mode if currently off).
    TogglePrettify,
    /// Set a specific prettify mode (issued by the `-P<letter>` sub-prefix
    /// after the user picks j/y/t/x/h/c).
    SetPrettifyMode(PrettifyMode),
    /// Re-run byte-based content detection and apply the result (`-Pa`).
    RedetectPrettify,
    /// A digit (0-9) was pressed. The app accumulates these into a numeric
    /// prefix that the next non-digit command consumes.
    Digit(u8),
    /// Jump to physical line N (1-indexed). Without a prefix, behaves as
    /// goto-top.
    GotoLine,
    /// Jump to record N (1-indexed). Without a prefix, behaves as
    /// goto-bottom (preserves the existing bare-`G` behavior).
    GotoRecord,
    /// Jump to N percent through the file by bytes. Without a prefix,
    /// behaves as goto-top.
    GotoPercent,
    /// Cancel any pending numeric prefix without firing a command.
    Cancel,
    /// First half of a set-mark sequence (the `m` key). The next keystroke
    /// names the mark.
    MarkSet,
    /// First half of a jump-to-mark sequence (the `'` key). The next
    /// keystroke names the mark.
    MarkJump,
    /// First half of the `Ctrl-X Ctrl-X` jump-to-previous-position chord.
    /// The next keystroke must also be Ctrl-X.
    CtrlXPrefix,
    /// Jump to the previous position (Ctrl-X Ctrl-X in less). Dispatched
    /// from the CtrlXPending mode intercept in app.rs.
    JumpPrevious,
    /// Enter the !cmd shell-escape prompt.
    ShellEscape,
    /// Enter the :colon-command prompt.
    ColonPrompt,
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
        (Char('J'), false) => Command::ScrollLogicalLines(1),
        (Char('K'), false) => Command::ScrollLogicalLines(-1),
        (Char(' '), false) | (Char('f'), false) | (Char('f'), true) | (PageDown, _) => Command::PageDown,
        (Char('b'), false) | (Char('b'), true) | (PageUp, _) => Command::PageUp,
        (Char('d'), false) | (Char('d'), true) => Command::HalfPageDown,
        (Char('u'), false) | (Char('u'), true) => Command::HalfPageUp,
        (Char('0'), false) => Command::Digit(0),
        (Char('1'), false) => Command::Digit(1),
        (Char('2'), false) => Command::Digit(2),
        (Char('3'), false) => Command::Digit(3),
        (Char('4'), false) => Command::Digit(4),
        (Char('5'), false) => Command::Digit(5),
        (Char('6'), false) => Command::Digit(6),
        (Char('7'), false) => Command::Digit(7),
        (Char('8'), false) => Command::Digit(8),
        (Char('9'), false) => Command::Digit(9),
        (Char('g'), false) | (Char('<'), false) | (Home, _) => Command::GotoLine,
        (Char('G'), false) | (Char('>'), false) | (End, _) => Command::GotoRecord,
        (Char('%'), false) => Command::GotoPercent,
        (Esc, _) => Command::Cancel,
        (Char('r'), false) | (Char('l'), true) => Command::Refresh,
        (Char('R'), false) => Command::Reload,
        (Char('P'), false) => Command::TogglePrettify,
        (Char('-'), false) => Command::OptionPrefix,
        (Char('F'), false) => Command::ToggleFollow,
        (Char('/'), false) => Command::SearchForward,
        (Char('?'), false) => Command::SearchBackward,
        (Char('n'), false) => Command::NextMatch,
        (Char('N'), false) => Command::PreviousMatch,
        (Char('m'), false) => Command::MarkSet,
        (Char('\''), false) => Command::MarkJump,
        (Char('!'), false) => Command::ShellEscape,
        (Char('x'), true) => Command::CtrlXPrefix,
        (Char(':'), false) => Command::ColonPrompt,
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
    fn capital_g_goes_to_record() {
        assert_eq!(translate(key(KeyCode::Char('G'), KeyModifiers::SHIFT)), Command::GotoRecord);
    }

    #[test]
    fn lowercase_g_goes_to_line() {
        assert_eq!(translate(key(KeyCode::Char('g'), KeyModifiers::NONE)), Command::GotoLine);
    }

    #[test]
    fn percent_goes_to_percent() {
        assert_eq!(translate(key(KeyCode::Char('%'), KeyModifiers::NONE)), Command::GotoPercent);
    }

    #[test]
    fn digit_keys_produce_digit_commands() {
        for d in 0u8..=9 {
            let ch = char::from_digit(d as u32, 10).unwrap();
            assert_eq!(
                translate(key(KeyCode::Char(ch), KeyModifiers::NONE)),
                Command::Digit(d),
            );
        }
    }

    #[test]
    fn esc_produces_cancel() {
        assert_eq!(translate(key(KeyCode::Esc, KeyModifiers::NONE)), Command::Cancel);
    }

    #[test]
    fn capital_j_jumps_one_logical_line_forward() {
        assert_eq!(translate(key(KeyCode::Char('J'), KeyModifiers::SHIFT)), Command::ScrollLogicalLines(1));
    }

    #[test]
    fn capital_k_jumps_one_logical_line_backward() {
        assert_eq!(translate(key(KeyCode::Char('K'), KeyModifiers::SHIFT)), Command::ScrollLogicalLines(-1));
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
    fn capital_p_toggles_prettify() {
        assert_eq!(translate(key(KeyCode::Char('P'), KeyModifiers::SHIFT)), Command::TogglePrettify);
    }

    #[test]
    fn lowercase_p_remains_unbound() {
        assert_eq!(translate(key(KeyCode::Char('p'), KeyModifiers::NONE)), Command::Noop);
    }

    #[test]
    fn dash_is_option_prefix() {
        assert_eq!(translate(key(KeyCode::Char('-'), KeyModifiers::NONE)), Command::OptionPrefix);
    }

    #[test]
    fn resize_event() {
        assert_eq!(translate(Event::Resize(80, 24)), Command::Resize(80, 24));
    }

    #[test]
    fn m_key_produces_mark_set_command() {
        let evt = key(KeyCode::Char('m'), KeyModifiers::NONE);
        assert_eq!(translate(evt), Command::MarkSet);
    }

    #[test]
    fn single_quote_key_produces_mark_jump_command() {
        let evt = key(KeyCode::Char('\''), KeyModifiers::NONE);
        assert_eq!(translate(evt), Command::MarkJump);
    }

    #[test]
    fn ctrl_x_produces_ctrl_x_prefix_command() {
        let evt = key(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(translate(evt), Command::CtrlXPrefix);
    }

    #[test]
    fn bang_produces_shell_escape_command() {
        let evt = key(KeyCode::Char('!'), KeyModifiers::NONE);
        assert_eq!(translate(evt), Command::ShellEscape);
    }

    #[test]
    fn colon_produces_colon_prompt_command() {
        let evt = Event::Key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        assert_eq!(translate(evt), Command::ColonPrompt);
    }
}
