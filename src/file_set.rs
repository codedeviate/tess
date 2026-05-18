//! Owns the multi-file working set: a list of paths, a current-index
//! cursor, and the navigation primitives that the colon-prompt dispatch
//! consumes (`:n`, `:p`, `:e`, `:d`, `:x`, `:t`).
//!
//! Does NOT own `Source` instances long-term — those are constructed on
//! demand by `main::open_source_for_path` and dropped on switch, so a
//! 100-file invocation doesn't mmap 100 files at once.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSetError {
    NoNextFile,
    NoPreviousFile,
    WouldEmpty,
}

impl std::fmt::Display for FileSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileSetError::NoNextFile => write!(f, "no next file"),
            FileSetError::NoPreviousFile => write!(f, "no previous file"),
            FileSetError::WouldEmpty => write!(f, "cannot remove last file"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileSet {
    paths: Vec<PathBuf>,
    current_index: usize,
}

impl FileSet {
    /// Construct with the initial path list. `paths` must be non-empty
    /// for navigation to make sense; an empty list is technically valid
    /// (stdin-source startup uses an empty FileSet) but all navigation
    /// methods then return errors or `None`.
    pub fn new(paths: Vec<PathBuf>) -> Self {
        Self { paths, current_index: 0 }
    }

    pub fn current(&self) -> Option<&Path> {
        self.paths.get(self.current_index).map(|p| p.as_path())
    }

    /// Total number of files in the set.
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn current_index(&self) -> usize {
        self.current_index
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Set the cursor directly. Out-of-range indices are clamped to the
    /// last entry (or no-op if the list is empty).
    pub fn set_current_index(&mut self, index: usize) {
        if self.paths.is_empty() {
            return;
        }
        self.current_index = index.min(self.paths.len() - 1);
    }

    /// Advance to the next file. Returns the new current path on success
    /// or `NoNextFile` if already at the last entry.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<&Path, FileSetError> {
        if self.current_index + 1 >= self.paths.len() {
            return Err(FileSetError::NoNextFile);
        }
        self.current_index += 1;
        Ok(self.paths[self.current_index].as_path())
    }

    /// Move to the previous file. Returns the new current path on success
    /// or `NoPreviousFile` if already at the first entry.
    pub fn prev(&mut self) -> Result<&Path, FileSetError> {
        if self.current_index == 0 {
            return Err(FileSetError::NoPreviousFile);
        }
        self.current_index -= 1;
        Ok(self.paths[self.current_index].as_path())
    }

    /// Jump to the first file. Returns the current path after the move,
    /// or `None` if the list is empty. Returns `Option` (not `Result`)
    /// because jumping to the boundary is always idempotent — there's
    /// no "no first file" failure mode like there is for `next`.
    pub fn first(&mut self) -> Option<&Path> {
        if self.paths.is_empty() {
            return None;
        }
        self.current_index = 0;
        Some(self.paths[0].as_path())
    }

    /// Jump to the last file. Returns the current path after the move,
    /// or `None` if the list is empty. See `first` for the rationale
    /// behind `Option` rather than `Result`.
    pub fn last(&mut self) -> Option<&Path> {
        if self.paths.is_empty() {
            return None;
        }
        self.current_index = self.paths.len() - 1;
        Some(self.paths[self.current_index].as_path())
    }

    /// Append `path` to the list and switch the cursor to it.
    pub fn append_and_switch(&mut self, path: PathBuf) -> &Path {
        self.paths.push(path);
        self.current_index = self.paths.len() - 1;
        self.paths[self.current_index].as_path()
    }

    /// Delete the current entry and move the cursor to the next file (or
    /// back to the previous if we were at the end). Returns the new
    /// current path. Errors with `WouldEmpty` when only one file remains.
    pub fn delete_current(&mut self) -> Result<&Path, FileSetError> {
        if self.paths.len() <= 1 {
            return Err(FileSetError::WouldEmpty);
        }
        self.paths.remove(self.current_index);
        if self.current_index >= self.paths.len() {
            self.current_index = self.paths.len() - 1;
        }
        Ok(self.paths[self.current_index].as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs(names: &[&str]) -> FileSet {
        FileSet::new(names.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn new_with_paths_sets_current_zero() {
        let f = fs(&["a.log", "b.log", "c.log"]);
        assert_eq!(f.current_index(), 0);
        assert_eq!(f.current(), Some(Path::new("a.log")));
    }

    #[test]
    fn len_reports_total() {
        let f = fs(&["a.log", "b.log", "c.log"]);
        assert_eq!(f.len(), 3);
    }

    #[test]
    fn next_advances_index() {
        let mut f = fs(&["a.log", "b.log", "c.log"]);
        assert_eq!(f.next().unwrap(), Path::new("b.log"));
        assert_eq!(f.current_index(), 1);
        assert_eq!(f.next().unwrap(), Path::new("c.log"));
        assert_eq!(f.current_index(), 2);
    }

    #[test]
    fn next_at_last_returns_no_next_file_error() {
        let mut f = fs(&["a.log"]);
        assert_eq!(f.next().unwrap_err(), FileSetError::NoNextFile);
        assert_eq!(f.current_index(), 0);
    }

    #[test]
    fn prev_decrements_index() {
        let mut f = fs(&["a.log", "b.log"]);
        f.next().unwrap();
        assert_eq!(f.prev().unwrap(), Path::new("a.log"));
        assert_eq!(f.current_index(), 0);
    }

    #[test]
    fn prev_at_first_returns_no_previous_file_error() {
        let mut f = fs(&["a.log", "b.log"]);
        assert_eq!(f.prev().unwrap_err(), FileSetError::NoPreviousFile);
        assert_eq!(f.current_index(), 0);
    }

    #[test]
    fn first_resets_to_zero() {
        let mut f = fs(&["a.log", "b.log", "c.log"]);
        f.next().unwrap();
        f.next().unwrap();
        assert_eq!(f.first(), Some(Path::new("a.log")));
        assert_eq!(f.current_index(), 0);
    }

    #[test]
    fn last_jumps_to_count_minus_one() {
        let mut f = fs(&["a.log", "b.log", "c.log"]);
        assert_eq!(f.last(), Some(Path::new("c.log")));
        assert_eq!(f.current_index(), 2);
    }

    #[test]
    fn append_and_switch_grows_list_and_moves_cursor() {
        let mut f = fs(&["a.log"]);
        let new_path = f.append_and_switch(PathBuf::from("b.log"));
        assert_eq!(new_path, Path::new("b.log"));
        assert_eq!(f.len(), 2);
        assert_eq!(f.current_index(), 1);
    }

    #[test]
    fn delete_current_drops_entry_and_advances() {
        let mut f = fs(&["a.log", "b.log", "c.log"]);
        f.next().unwrap();  // now at b.log
        let new_path = f.delete_current().unwrap();
        assert_eq!(new_path, Path::new("c.log"));
        assert_eq!(f.len(), 2);
        assert_eq!(f.current_index(), 1);
    }

    #[test]
    fn delete_current_at_end_moves_back() {
        let mut f = fs(&["a.log", "b.log"]);
        f.next().unwrap();  // at b.log (last)
        let new_path = f.delete_current().unwrap();
        assert_eq!(new_path, Path::new("a.log"));
        assert_eq!(f.len(), 1);
        assert_eq!(f.current_index(), 0);
    }

    #[test]
    fn delete_current_at_start_stays_at_zero() {
        let mut f = fs(&["a.log", "b.log", "c.log"]);
        // cursor is at index 0 (a.log)
        let new_path = f.delete_current().unwrap();
        assert_eq!(new_path, Path::new("b.log"));
        assert_eq!(f.len(), 2);
        assert_eq!(f.current_index(), 0);
    }

    #[test]
    fn delete_current_with_single_file_returns_would_empty_error() {
        let mut f = fs(&["a.log"]);
        assert_eq!(f.delete_current().unwrap_err(), FileSetError::WouldEmpty);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn empty_fileset_returns_none_for_current() {
        let f = FileSet::new(Vec::new());
        assert_eq!(f.current(), None);
        assert!(f.is_empty());
        assert_eq!(f.len(), 0);
    }

    #[test]
    fn set_current_index_changes_cursor() {
        let mut f = fs(&["a.log", "b.log", "c.log"]);
        f.set_current_index(2);
        assert_eq!(f.current(), Some(Path::new("c.log")));
        f.set_current_index(99);  // clamp
        assert_eq!(f.current_index(), 2);
    }

    #[test]
    fn next_on_empty_returns_no_next_file_error() {
        let mut f = FileSet::new(Vec::new());
        assert_eq!(f.next().unwrap_err(), FileSetError::NoNextFile);
    }

    #[test]
    fn prev_on_empty_returns_no_previous_file_error() {
        let mut f = FileSet::new(Vec::new());
        assert_eq!(f.prev().unwrap_err(), FileSetError::NoPreviousFile);
    }
}
