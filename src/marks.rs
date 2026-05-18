//! Pure helpers for marks (`m<x>` / `'<x>`) and previous-position swap (`^X^X`).
//!
//! Marks are session-local: a HashMap<char, (usize, usize)> owned by `app::run`.
//! Previous-position is a single `Option<(usize, usize)>` slot also owned by `app::run`.

use std::collections::HashMap;

/// Result of a mark jump or previous-position swap. The caller (app
/// dispatch) handles `OtherFile` by switching files before applying
/// the line jump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkTarget {
    SameFile { line: usize },
    OtherFile { file_index: usize, line: usize },
}

/// True iff `c` is a valid mark name (ASCII lowercase letter or ASCII digit).
pub fn is_valid_mark_name(c: char) -> bool {
    c.is_ascii_alphanumeric() && (c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Set mark `name` to `(file_index, top_line)`. Silently no-ops on invalid mark names.
pub fn mark_set(
    marks: &mut HashMap<char, (usize, usize)>,
    name: char,
    file_index: usize,
    top_line: usize,
) {
    if is_valid_mark_name(name) {
        marks.insert(name, (file_index, top_line));
    }
}

/// Jump to mark `name`. Returns a `MarkTarget` describing the destination,
/// or `None` if the mark is unknown / name is invalid. On a successful jump,
/// records `(current_file_index, current_top)` into `previous_position`.
///
/// Note: clamping to line_count is the caller's responsibility, since the
/// destination file's line_count isn't known until after a potential file switch.
pub fn mark_jump(
    marks: &HashMap<char, (usize, usize)>,
    name: char,
    current_file_index: usize,
    previous_position: &mut Option<(usize, usize)>,
    current_top: usize,
) -> Option<MarkTarget> {
    if !is_valid_mark_name(name) {
        return None;
    }
    let (target_file, target_line) = *marks.get(&name)?;
    *previous_position = Some((current_file_index, current_top));
    if target_file == current_file_index {
        Some(MarkTarget::SameFile { line: target_line })
    } else {
        Some(MarkTarget::OtherFile {
            file_index: target_file,
            line: target_line,
        })
    }
}

/// Swap current position with the previous-position slot. Returns the previous
/// position as a `MarkTarget`, or `None` if no previous position has been recorded.
pub fn jump_previous(
    previous_position: &mut Option<(usize, usize)>,
    current_file_index: usize,
    current_top: usize,
) -> Option<MarkTarget> {
    let (prev_file, prev_line) = previous_position.take()?;
    *previous_position = Some((current_file_index, current_top));
    if prev_file == current_file_index {
        Some(MarkTarget::SameFile { line: prev_line })
    } else {
        Some(MarkTarget::OtherFile {
            file_index: prev_file,
            line: prev_line,
        })
    }
}

/// Helper for big-jump dispatch sites: record the current position as the
/// previous position before performing a discontinuous move.
pub fn update_prev_position(
    previous_position: &mut Option<(usize, usize)>,
    current_file_index: usize,
    current_top: usize,
) {
    *previous_position = Some((current_file_index, current_top));
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
        mark_set(&mut marks, 'a', 0, 42);
        assert_eq!(marks.get(&'a'), Some(&(0, 42)));
    }

    #[test]
    fn mark_set_invalid_name_is_noop() {
        let mut marks = HashMap::new();
        mark_set(&mut marks, '!', 0, 42);
        mark_set(&mut marks, 'A', 0, 42);
        assert!(marks.is_empty());
    }

    #[test]
    fn mark_set_overwrites_silently() {
        let mut marks = HashMap::new();
        mark_set(&mut marks, 'a', 0, 10);
        mark_set(&mut marks, 'a', 0, 20);
        assert_eq!(marks.get(&'a'), Some(&(0, 20)));
    }

    #[test]
    fn mark_jump_known_mark_returns_value_and_updates_prev() {
        let mut marks = HashMap::new();
        marks.insert('a', (0, 50));
        let mut prev = None;
        let result = mark_jump(&marks, 'a', 0, &mut prev, 100);
        assert_eq!(result, Some(MarkTarget::SameFile { line: 50 }));
        assert_eq!(prev, Some((0, 100)));
    }

    #[test]
    fn mark_jump_unknown_mark_returns_none_no_prev_update() {
        let marks = HashMap::new();
        let mut prev = None;
        let result = mark_jump(&marks, 'q', 0, &mut prev, 100);
        assert_eq!(result, None);
        assert_eq!(prev, None);
    }

    #[test]
    fn mark_jump_invalid_name_returns_none() {
        let mut marks = HashMap::new();
        marks.insert('!', (0, 50));
        let mut prev = None;
        let result = mark_jump(&marks, '!', 0, &mut prev, 100);
        assert_eq!(result, None);
    }

    #[test]
    fn jump_previous_first_call_returns_none() {
        let mut prev = None;
        let result = jump_previous(&mut prev, 0, 50);
        assert_eq!(result, None);
        assert_eq!(prev, None);
    }

    #[test]
    fn jump_previous_swaps_and_keeps_history() {
        let mut prev = Some((0, 10));
        let result = jump_previous(&mut prev, 0, 50);
        assert_eq!(result, Some(MarkTarget::SameFile { line: 10 }));
        assert_eq!(prev, Some((0, 50)));
    }

    #[test]
    fn jump_previous_repeated_oscillates() {
        let mut prev = Some((0, 10));
        let r1 = jump_previous(&mut prev, 0, 50);
        assert_eq!(r1, Some(MarkTarget::SameFile { line: 10 }));
        let r2 = jump_previous(&mut prev, 0, 10);
        assert_eq!(r2, Some(MarkTarget::SameFile { line: 50 }));
    }

    #[test]
    fn update_prev_position_overwrites_slot() {
        let mut prev = Some((0, 7));
        update_prev_position(&mut prev, 0, 42);
        assert_eq!(prev, Some((0, 42)));
    }

    #[test]
    fn mark_jump_returns_other_file_target_when_set_in_different_file() {
        let mut marks = HashMap::new();
        marks.insert('a', (1, 200));
        let mut prev = None;
        let result = mark_jump(&marks, 'a', 0, &mut prev, 50);
        assert_eq!(result, Some(MarkTarget::OtherFile { file_index: 1, line: 200 }));
        assert_eq!(prev, Some((0, 50)));
    }

    #[test]
    fn jump_previous_returns_other_file_when_previous_was_in_another_file() {
        let mut prev = Some((1, 75));
        let result = jump_previous(&mut prev, 0, 25);
        assert_eq!(result, Some(MarkTarget::OtherFile { file_index: 1, line: 75 }));
        assert_eq!(prev, Some((0, 25)));
    }
}
