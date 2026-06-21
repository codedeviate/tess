//! Pure line/char diff for the split view's diff mode. Wraps the `similar`
//! crate behind stable `DiffPair` / `char_spans` types so the rest of the
//! codebase doesn't depend on `similar` directly.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffClass {
    Equal,
    Changed,
    Added,   // present only on the right (left is filler)
    Removed, // present only on the left  (right is filler)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffPair {
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub class: DiffClass,
}

/// Align two sequences of line keys into an ordered list of `DiffPair`s.
/// Uses `similar`'s Myers diff. Generic over the key type so tests can use
/// integers; production passes `&[u64]` line hashes.
///
/// A Replace run pairs `min(del, ins)` lines as `Changed` and emits the surplus
/// as `Removed` (left-only) or `Added` (right-only).
pub fn align<T: std::hash::Hash + Ord + Eq>(left: &[T], right: &[T]) -> Vec<DiffPair> {
    use similar::{capture_diff_slices, Algorithm, DiffOp};
    let ops = capture_diff_slices(Algorithm::Myers, left, right);
    let mut pairs = Vec::new();
    for op in ops {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                for k in 0..len {
                    pairs.push(DiffPair { left: Some(old_index + k), right: Some(new_index + k), class: DiffClass::Equal });
                }
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                for k in 0..old_len {
                    pairs.push(DiffPair { left: Some(old_index + k), right: None, class: DiffClass::Removed });
                }
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                for k in 0..new_len {
                    pairs.push(DiffPair { left: None, right: Some(new_index + k), class: DiffClass::Added });
                }
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                let paired = old_len.min(new_len);
                for k in 0..paired {
                    pairs.push(DiffPair { left: Some(old_index + k), right: Some(new_index + k), class: DiffClass::Changed });
                }
                for k in paired..old_len {
                    pairs.push(DiffPair { left: Some(old_index + k), right: None, class: DiffClass::Removed });
                }
                for k in paired..new_len {
                    pairs.push(DiffPair { left: None, right: Some(new_index + k), class: DiffClass::Added });
                }
            }
        }
    }
    pairs
}

/// Hash a line into a comparison key. With `ignore_ws`, leading/trailing
/// whitespace is trimmed and internal whitespace runs collapse to one space
/// *for the hash only* — the displayed text is untouched. Whitespace bytes are
/// ASCII (space/tab/CR/FF/VT), so this is encoding-invariant.
///
/// Hashing means a vanishingly rare collision could mis-classify two lines as
/// equal; acceptable for a pager diff (same trade-off git makes).
pub fn line_key(bytes: &[u8], ignore_ws: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    if ignore_ws {
        let mut started = false;
        let mut prev_ws = false;
        let mut pending_space = false;
        for &b in bytes {
            let is_ws = b == b' ' || b == b'\t' || b == b'\r' || b == 0x0c || b == 0x0b;
            if is_ws { prev_ws = true; continue; }
            if started && prev_ws { pending_space = true; }
            if pending_space { b' '.hash(&mut h); pending_space = false; }
            b.hash(&mut h);
            started = true;
            prev_ws = false;
        }
    } else {
        bytes.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classes(p: &[DiffPair]) -> Vec<DiffClass> { p.iter().map(|x| x.class).collect() }

    #[test]
    fn identical_files_are_all_equal() {
        let a = [1u64, 2, 3];
        let pairs = align(&a, &a);
        assert_eq!(pairs.len(), 3);
        assert!(pairs.iter().all(|p| p.class == DiffClass::Equal));
        assert_eq!(pairs[0], DiffPair { left: Some(0), right: Some(0), class: DiffClass::Equal });
        assert_eq!(pairs[2], DiffPair { left: Some(2), right: Some(2), class: DiffClass::Equal });
    }

    #[test]
    fn pure_insertion_on_right() {
        let pairs = align(&[1u64, 3], &[1u64, 2, 3]);
        assert_eq!(classes(&pairs), vec![DiffClass::Equal, DiffClass::Added, DiffClass::Equal]);
        let added = pairs.iter().find(|p| p.class == DiffClass::Added).unwrap();
        assert_eq!(added.left, None);
        assert_eq!(added.right, Some(1));
    }

    #[test]
    fn pure_deletion_on_left() {
        let pairs = align(&[1u64, 2, 3], &[1u64, 3]);
        assert_eq!(classes(&pairs), vec![DiffClass::Equal, DiffClass::Removed, DiffClass::Equal]);
        let removed = pairs.iter().find(|p| p.class == DiffClass::Removed).unwrap();
        assert_eq!(removed.left, Some(1));
        assert_eq!(removed.right, None);
    }

    #[test]
    fn one_for_one_change_is_changed() {
        let pairs = align(&[1u64, 8, 3], &[1u64, 9, 3]);
        assert_eq!(classes(&pairs), vec![DiffClass::Equal, DiffClass::Changed, DiffClass::Equal]);
        let ch = pairs.iter().find(|p| p.class == DiffClass::Changed).unwrap();
        assert_eq!((ch.left, ch.right), (Some(1), Some(1)));
    }

    #[test]
    fn replace_with_surplus_splits_into_changed_then_added() {
        let pairs = align(&[8u64], &[9u64, 10]);
        assert_eq!(classes(&pairs), vec![DiffClass::Changed, DiffClass::Added]);
        assert_eq!((pairs[0].left, pairs[0].right), (Some(0), Some(0)));
        assert_eq!((pairs[1].left, pairs[1].right), (None, Some(1)));
    }

    #[test]
    fn replace_with_surplus_splits_into_changed_then_removed() {
        let pairs = align(&[8u64, 11], &[9u64]);
        assert_eq!(classes(&pairs), vec![DiffClass::Changed, DiffClass::Removed]);
        assert_eq!((pairs[1].left, pairs[1].right), (Some(1), None));
    }

    #[test]
    fn empty_vs_nonempty_is_all_added() {
        let pairs = align(&[], &[1u64, 2]);
        assert_eq!(classes(&pairs), vec![DiffClass::Added, DiffClass::Added]);
    }

    #[test]
    fn both_empty_is_empty() {
        assert!(align::<u64>(&[], &[]).is_empty());
    }

    #[test]
    fn line_key_distinguishes_different_lines() {
        assert_ne!(line_key(b"hello", false), line_key(b"world", false));
    }
    #[test]
    fn line_key_same_line_is_stable() {
        assert_eq!(line_key(b"hello", false), line_key(b"hello", false));
    }
    #[test]
    fn ignore_ws_collapses_whitespace_differences() {
        assert_eq!(line_key(b"  foo  bar", true), line_key(b"foo bar", true));
        assert_eq!(line_key(b"\tfoo\tbar", true), line_key(b"foo bar", true));
        assert_ne!(line_key(b"  foo  bar", false), line_key(b"foo bar", false));
    }
    #[test]
    fn ignore_ws_still_distinguishes_real_content() {
        assert_ne!(line_key(b"foo bar", true), line_key(b"foo baz", true));
    }
}
