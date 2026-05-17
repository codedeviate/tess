//! Pure helpers for marks (`m<x>` / `'<x>`) and previous-position swap (`^X^X`).
//!
//! Marks are session-local: a HashMap<char, usize> owned by `app::run`.
//! Previous-position is a single `Option<usize>` slot also owned by `app::run`.

use std::collections::HashMap;

/// True iff `c` is a valid mark name (ASCII lowercase letter or ASCII digit).
pub fn is_valid_mark_name(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit()
}

/// Set mark `name` to `top_line`. Silently no-ops on invalid mark names.
pub fn mark_set(marks: &mut HashMap<char, usize>, name: char, top_line: usize) {
    if is_valid_mark_name(name) {
        marks.insert(name, top_line);
    }
}

/// Jump to mark `name`. Returns the line number to jump to (clamped to
/// `[0, line_count-1]`), or `None` if the mark is unknown / name is invalid /
/// the source is empty. On a successful jump, records current top into
/// `previous_position`.
pub fn mark_jump(
    marks: &HashMap<char, usize>,
    name: char,
    line_count: usize,
    previous_position: &mut Option<usize>,
    current_top: usize,
) -> Option<usize> {
    if !is_valid_mark_name(name) {
        return None;
    }
    let raw = *marks.get(&name)?;
    if line_count == 0 {
        return None;
    }
    *previous_position = Some(current_top);
    Some(raw.min(line_count - 1))
}

/// Swap current top_line with the previous-position slot. Returns the new
/// top_line, or `None` if no previous position has been recorded.
pub fn jump_previous(
    previous_position: &mut Option<usize>,
    current_top: usize,
) -> Option<usize> {
    let prev = previous_position.take()?;
    *previous_position = Some(current_top);
    Some(prev)
}

/// Helper for big-jump dispatch sites: record the current top_line as the
/// previous position before performing a discontinuous move.
pub fn update_prev_position(previous_position: &mut Option<usize>, current_top: usize) {
    *previous_position = Some(current_top);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_valid_mark_name_accepts_lowercase_letters() {
        for c in 'a'..='z' {
            assert!(is_valid_mark_name(c), "{c} should be valid");
        }
    }

    #[test]
    fn is_valid_mark_name_accepts_digits() {
        for c in '0'..='9' {
            assert!(is_valid_mark_name(c), "{c} should be valid");
        }
    }

    #[test]
    fn is_valid_mark_name_rejects_uppercase_and_punctuation() {
        assert!(!is_valid_mark_name('A'));
        assert!(!is_valid_mark_name('Z'));
        assert!(!is_valid_mark_name('!'));
        assert!(!is_valid_mark_name(' '));
        assert!(!is_valid_mark_name('\''));
    }

    #[test]
    fn mark_set_records_top_line() {
        let mut marks = HashMap::new();
        mark_set(&mut marks, 'a', 42);
        assert_eq!(marks.get(&'a'), Some(&42));
    }

    #[test]
    fn mark_set_invalid_name_is_noop() {
        let mut marks = HashMap::new();
        mark_set(&mut marks, '!', 42);
        mark_set(&mut marks, 'A', 42);
        assert!(marks.is_empty());
    }

    #[test]
    fn mark_set_overwrites_silently() {
        let mut marks = HashMap::new();
        mark_set(&mut marks, 'a', 10);
        mark_set(&mut marks, 'a', 20);
        assert_eq!(marks.get(&'a'), Some(&20));
    }

    #[test]
    fn mark_jump_known_mark_returns_value_and_updates_prev() {
        let mut marks = HashMap::new();
        marks.insert('a', 50);
        let mut prev = None;
        let result = mark_jump(&marks, 'a', 1000, &mut prev, 100);
        assert_eq!(result, Some(50));
        assert_eq!(prev, Some(100));
    }

    #[test]
    fn mark_jump_unknown_mark_returns_none_no_prev_update() {
        let marks = HashMap::new();
        let mut prev = None;
        let result = mark_jump(&marks, 'q', 1000, &mut prev, 100);
        assert_eq!(result, None);
        assert_eq!(prev, None);
    }

    #[test]
    fn mark_jump_invalid_name_returns_none() {
        let mut marks = HashMap::new();
        marks.insert('!', 50);
        let mut prev = None;
        let result = mark_jump(&marks, '!', 1000, &mut prev, 100);
        assert_eq!(result, None);
    }

    #[test]
    fn mark_jump_clamps_to_last_line_when_source_shrank() {
        let mut marks = HashMap::new();
        marks.insert('a', 500);
        let mut prev = None;
        let result = mark_jump(&marks, 'a', 10, &mut prev, 0);
        assert_eq!(result, Some(9), "should clamp to line_count - 1");
    }

    #[test]
    fn mark_jump_empty_source_returns_none() {
        let mut marks = HashMap::new();
        marks.insert('a', 0);
        let mut prev = None;
        let result = mark_jump(&marks, 'a', 0, &mut prev, 0);
        assert_eq!(result, None);
        assert_eq!(prev, None);
    }

    #[test]
    fn jump_previous_first_call_returns_none() {
        let mut prev = None;
        let result = jump_previous(&mut prev, 50);
        assert_eq!(result, None);
        assert_eq!(prev, None);
    }

    #[test]
    fn jump_previous_swaps_and_keeps_history() {
        let mut prev = Some(10);
        let result = jump_previous(&mut prev, 50);
        assert_eq!(result, Some(10));
        assert_eq!(prev, Some(50));
    }

    #[test]
    fn jump_previous_repeated_oscillates() {
        let mut prev = Some(10);
        let r1 = jump_previous(&mut prev, 50);
        assert_eq!(r1, Some(10));
        let r2 = jump_previous(&mut prev, 10);
        assert_eq!(r2, Some(50));
    }

    #[test]
    fn update_prev_position_overwrites_slot() {
        let mut prev = Some(7);
        update_prev_position(&mut prev, 42);
        assert_eq!(prev, Some(42));
    }
}
