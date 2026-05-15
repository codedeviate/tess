//! Integration tests for records-aware search and grep on the
//! multiline-records fixture.

use regex::bytes::Regex;
use tess::grep::GrepPredicate;
use tess::line_index::LineIndex;
use tess::source::FileSource;
use tess::viewport::{SearchDirection, Viewport};

const FIXTURE: &str = "tests/fixtures/multiline-records.log";

fn setup() -> (FileSource, LineIndex) {
    let src = FileSource::open(std::path::Path::new(FIXTURE)).unwrap();
    let mut idx = LineIndex::new();
    idx.set_record_start(Regex::new(r"^\[").unwrap());
    idx.extend_to_end(&src);
    (src, idx)
}

#[test]
fn search_finds_cross_line_pattern_in_records() {
    let (src, mut idx) = setup();
    let mut v = Viewport::new(80, 24, "fixture".into());
    // Start at top (record 0 / banner). Search forward for a pattern that spans
    // the ERROR header and a continuation line.
    v.set_search(r"(?s)ERROR.*Renderer\.php".into(), SearchDirection::Forward).unwrap();
    let hit = v.search_repeat(&src, &mut idx, false);
    assert!(hit, "should find the ERROR record");
    // ERROR record (record 2) starts at line 3.
    assert_eq!(v.top_line(), 3);
}

#[test]
fn grep_hides_nonmatching_records_entirely() {
    let (src, mut idx) = setup();
    let mut v = Viewport::new(80, 24, "fixture".into());
    v.set_grep(Some(GrepPredicate::compile(&["ERROR".to_string()]).unwrap()));
    v.extend_visible_lines(&idx, &src);
    // Only record 2 (ERROR) visible → lines 3..8.
    assert_eq!(v.visible_lines(), &[3usize, 4, 5, 6, 7][..]);
}

#[test]
fn grep_with_cross_line_pattern_matches_full_record() {
    let (src, mut idx) = setup();
    let mut v = Viewport::new(80, 24, "fixture".into());
    v.set_grep(Some(
        GrepPredicate::compile(&[r"(?s)WARN.*duration_ms".to_string()]).unwrap(),
    ));
    v.extend_visible_lines(&idx, &src);
    // Only record 4 (WARN) matches: lines 9..12.
    assert_eq!(v.visible_lines(), &[9usize, 10, 11][..]);
}
