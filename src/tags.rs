//! Tag-file parsing and lookup. Supports ctags (traditional + exuberant
//! suffix) and etags formats. Public API: `TagFile::load`, `TagFile::lookup`,
//! `TagFile::find_walking_up`, `TagFile::reload_if_changed`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagAddress {
    /// 1-based line number, as stored in the tags file.
    Line(usize),
    /// ctags `/pattern/` or `?pattern?` with the delimiters stripped.
    Pattern(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    pub file: PathBuf,
    pub address: TagAddress,
}

#[derive(Debug, Clone)]
pub struct TagFile {
    base_dir: PathBuf,
    /// On-disk path of the tag file, retained so `reload_if_changed` can
    /// re-stat and re-parse without a separate handle.
    path: PathBuf,
    /// Mtime captured at last load. `UNIX_EPOCH` if the filesystem couldn't
    /// report one (treated as "unknown" — any next mtime triggers reload).
    mtime: SystemTime,
    by_name: HashMap<String, Vec<TagEntry>>,
}

impl TagFile {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let bytes = fs::read(path).map_err(|_| Error::TagFileNotFound)?;
        let base_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        let mtime = fs::metadata(path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let by_name = if bytes.first().copied() == Some(b'\x0c') {
            parse_etags(&bytes, &base_dir, path)?
        } else {
            let text = std::str::from_utf8(&bytes).map_err(|_| {
                Error::TagFileParse("not UTF-8".into(), path.to_path_buf(), 0)
            })?;
            parse_ctags(text, &base_dir)
        };

        Ok(TagFile { base_dir, path: path.to_path_buf(), mtime, by_name })
    }

    /// Re-stat the on-disk tag file and, if its mtime changed, re-parse it
    /// in place. Returns `Ok(true)` when a reload happened, `Ok(false)`
    /// otherwise. Stat or parse errors that occur during reload are
    /// surfaced as `Err`; callers may choose to surface them as a status
    /// hint and keep using the previously-loaded state.
    pub fn reload_if_changed(&mut self) -> Result<bool, Error> {
        let new_mtime = match fs::metadata(&self.path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return Ok(false),
        };
        if new_mtime == self.mtime {
            return Ok(false);
        }
        let fresh = Self::load(&self.path)?;
        self.mtime = fresh.mtime;
        self.by_name = fresh.by_name;
        Ok(true)
    }

    pub fn lookup(&self, name: &str) -> &[TagEntry] {
        self.by_name
            .get(name)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }

    /// Walk up from `start` looking for a `tags` file. If `start` is a
    /// regular file, begin at its parent directory. Returns the first
    /// `tags` found, or `None` at the filesystem root.
    pub fn find_walking_up(start: &Path) -> Option<PathBuf> {
        let mut cur = if start.is_file() {
            start.parent()?.to_path_buf()
        } else {
            start.to_path_buf()
        };
        loop {
            let candidate = cur.join("tags");
            if candidate.is_file() {
                return Some(candidate);
            }
            if !cur.pop() {
                return None;
            }
        }
    }
}

fn parse_ctags(text: &str, base_dir: &Path) -> HashMap<String, Vec<TagEntry>> {
    let mut by_name: HashMap<String, Vec<TagEntry>> = HashMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with("!_TAG_") {
            continue;
        }
        let mut parts = line.splitn(3, '\t');
        let (Some(name), Some(file_field), Some(rest)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Some(address) = parse_ctags_address(rest) else {
            continue;
        };
        let file = base_dir.join(file_field);
        by_name
            .entry(name.to_string())
            .or_default()
            .push(TagEntry { file, address });
    }
    by_name
}

/// Address column has shape:
///   "42"                            → Line(42)
///   "42;\""                         → Line(42) (exuberant suffix stripped)
///   "/^pattern$/"  or  "/pat/;\""   → Pattern("^pattern$") / Pattern("pat")
///   "?pattern?"                     → Pattern("pattern")
///   anything else                   → None (line skipped silently)
fn parse_ctags_address(s: &str) -> Option<TagAddress> {
    let body = match s.find(";\"") {
        Some(idx) => &s[..idx],
        None => s,
    };
    let body = body.trim();
    if body.is_empty() {
        return None;
    }
    if let Ok(n) = body.parse::<usize>() {
        return Some(TagAddress::Line(n));
    }
    let bytes = body.as_bytes();
    let first = *bytes.first()?;
    let last = *bytes.last()?;
    if (first == b'/' || first == b'?') && first == last && bytes.len() >= 2 {
        let inner = &body[1..body.len() - 1];
        return Some(TagAddress::Pattern(inner.to_string()));
    }
    None
}

fn parse_etags(
    bytes: &[u8],
    base_dir: &Path,
    path: &Path,
) -> Result<HashMap<String, Vec<TagEntry>>, Error> {
    let mut by_name: HashMap<String, Vec<TagEntry>> = HashMap::new();
    let text = std::str::from_utf8(bytes).map_err(|_| {
        Error::TagFileParse("not UTF-8".into(), path.to_path_buf(), 0)
    })?;
    for section in text.split("\x0c\n").skip(1) {
        let mut lines = section.lines();
        let Some(header) = lines.next() else { continue };
        let Some((file_field, _size)) = header.rsplit_once(',') else {
            continue;
        };
        let file = base_dir.join(file_field);
        for line in lines {
            let Some((_src, after_del)) = line.split_once('\x7f') else {
                continue;
            };
            let Some((tag, after_soh)) = after_del.split_once('\x01') else {
                continue;
            };
            let Some((line_str, _offset)) = after_soh.split_once(',') else {
                continue;
            };
            let Ok(line_num) = line_str.parse::<usize>() else {
                continue;
            };
            by_name.entry(tag.to_string()).or_default().push(TagEntry {
                file: file.clone(),
                address: TagAddress::Line(line_num),
            });
        }
    }
    Ok(by_name)
}

/// Convert a ctags pattern body to a regex pattern. Vi-style `^` / `$`
/// anchors at the boundaries are preserved as regex anchors; the inner
/// text is regex-escaped so literal metacharacters in source don't
/// mis-match.
pub fn pattern_to_regex(pattern: &str) -> String {
    let (anchor_start, body) = if let Some(rest) = pattern.strip_prefix('^') {
        ("^", rest)
    } else {
        ("", pattern)
    };
    let (body, anchor_end) = if let Some(stripped) = body.strip_suffix('$') {
        (stripped, "$")
    } else {
        (body, "")
    };
    format!("{anchor_start}{}{anchor_end}", regex::escape(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf_from_ctags(text: &str) -> TagFile {
        let by_name = parse_ctags(text, Path::new("/proj"));
        TagFile {
            base_dir: PathBuf::from("/proj"),
            path: PathBuf::from("/proj/tags"),
            mtime: std::time::SystemTime::UNIX_EPOCH,
            by_name,
        }
    }

    #[test]
    fn ctags_three_column_line_parses() {
        let t = tf_from_ctags("foo\tsrc/lib.rs\t42\n");
        let entries = t.lookup("foo");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file, PathBuf::from("/proj/src/lib.rs"));
        assert_eq!(entries[0].address, TagAddress::Line(42));
    }

    #[test]
    fn ctags_exuberant_suffix_is_stripped() {
        let t = tf_from_ctags("foo\tsrc/lib.rs\t42;\"\tf\tfile:\n");
        assert_eq!(t.lookup("foo")[0].address, TagAddress::Line(42));
    }

    #[test]
    fn ctags_metadata_line_is_skipped() {
        let t = tf_from_ctags("!_TAG_FILE_FORMAT\t2\t/extended format/\nfoo\tsrc/lib.rs\t1\n");
        assert!(t.lookup("!_TAG_FILE_FORMAT").is_empty());
        assert_eq!(t.lookup("foo").len(), 1);
    }

    #[test]
    fn ctags_forward_slash_pattern_parses() {
        let t = tf_from_ctags("foo\tsrc/lib.rs\t/^fn foo()$/\n");
        assert_eq!(
            t.lookup("foo")[0].address,
            TagAddress::Pattern("^fn foo()$".into())
        );
    }

    #[test]
    fn ctags_question_mark_pattern_parses() {
        let t = tf_from_ctags("foo\tsrc/lib.rs\t?pattern?\n");
        assert_eq!(
            t.lookup("foo")[0].address,
            TagAddress::Pattern("pattern".into())
        );
    }

    #[test]
    fn ctags_pattern_with_suffix_strips_suffix() {
        let t = tf_from_ctags("foo\tsrc/lib.rs\t/^pat$/;\"\tf\n");
        assert_eq!(
            t.lookup("foo")[0].address,
            TagAddress::Pattern("^pat$".into())
        );
    }

    #[test]
    fn multiple_entries_for_same_name_accumulate() {
        let t = tf_from_ctags("foo\ta.rs\t1\nfoo\tb.rs\t2\n");
        assert_eq!(t.lookup("foo").len(), 2);
    }

    #[test]
    fn malformed_ctags_line_is_skipped() {
        let t = tf_from_ctags("oneword\nfoo\tsrc/lib.rs\t1\n");
        assert_eq!(t.lookup("foo").len(), 1);
        assert!(t.lookup("oneword").is_empty());
    }

    #[test]
    fn empty_address_is_skipped() {
        let t = tf_from_ctags("foo\tsrc/lib.rs\t\n");
        assert!(t.lookup("foo").is_empty());
    }

    #[test]
    fn etags_single_section_parses() {
        let bytes = b"\x0c\nsrc/lib.rs,42\n\x7ffoo\x01100,0\n";
        let by_name = parse_etags(bytes, Path::new("/proj"), Path::new("/proj/TAGS")).unwrap();
        let entries = by_name.get("foo").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].file, PathBuf::from("/proj/src/lib.rs"));
        assert_eq!(entries[0].address, TagAddress::Line(100));
    }

    #[test]
    fn etags_multiple_sections_accumulate() {
        let bytes =
            b"\x0c\na.rs,10\n\x7ffoo\x011,0\n\x0c\nb.rs,10\n\x7fbar\x012,0\n";
        let by_name = parse_etags(bytes, Path::new("/proj"), Path::new("/proj/TAGS")).unwrap();
        assert_eq!(by_name.len(), 2);
        assert!(by_name.contains_key("foo"));
        assert!(by_name.contains_key("bar"));
    }

    #[test]
    fn etags_malformed_line_is_skipped() {
        let bytes = b"\x0c\nsrc/lib.rs,42\nno-delimiters\n\x7ffoo\x011,0\n";
        let by_name = parse_etags(bytes, Path::new("/proj"), Path::new("/proj/TAGS")).unwrap();
        assert_eq!(by_name.get("foo").unwrap().len(), 1);
    }

    #[test]
    fn pattern_to_regex_preserves_anchors() {
        assert_eq!(pattern_to_regex("^fn foo()$"), "^fn foo\\(\\)$");
        assert_eq!(pattern_to_regex("foo"), "foo");
        assert_eq!(pattern_to_regex("^foo"), "^foo");
        assert_eq!(pattern_to_regex("foo$"), "foo$");
    }

    #[test]
    fn pattern_to_regex_escapes_metacharacters() {
        assert_eq!(pattern_to_regex("a.b"), "a\\.b");
        assert_eq!(pattern_to_regex("^a[b]c$"), "^a\\[b\\]c$");
    }

    #[test]
    fn find_walking_up_finds_in_same_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tags"), b"").unwrap();
        let found = TagFile::find_walking_up(dir.path());
        assert_eq!(found, Some(dir.path().join("tags")));
    }

    #[test]
    fn find_walking_up_finds_two_directories_up() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("tags"), b"").unwrap();
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let found = TagFile::find_walking_up(&nested);
        assert_eq!(found, Some(root.path().join("tags")));
    }

    #[test]
    fn find_walking_up_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(TagFile::find_walking_up(dir.path()), None);
    }

    #[test]
    fn reload_if_changed_picks_up_new_entries() {
        use std::{thread, time::Duration};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tags");
        std::fs::write(&path, "foo\tsrc/a.rs\t1\n").unwrap();
        let mut tf = TagFile::load(&path).unwrap();
        assert_eq!(tf.lookup("bar").len(), 0);

        // Sleep past filesystem mtime granularity (HFS+ / APFS = 1s).
        thread::sleep(Duration::from_millis(1100));
        std::fs::write(&path, "foo\tsrc/a.rs\t1\nbar\tsrc/b.rs\t2\n").unwrap();

        assert!(tf.reload_if_changed().unwrap());
        assert_eq!(tf.lookup("bar").len(), 1);
        // A second call without further changes returns false.
        assert!(!tf.reload_if_changed().unwrap());
    }
}
