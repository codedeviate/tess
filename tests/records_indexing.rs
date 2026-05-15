//! End-to-end record indexing against the real FileSource (mmap).
//! Complements the MockSource-based unit tests in src/line_index.rs.

use regex::bytes::Regex;
use tess::line_index::LineIndex;
use tess::source::FileSource;

const FIXTURE: &str = "tests/fixtures/multiline-records.log";

fn open_indexed() -> (FileSource, LineIndex) {
    let src = FileSource::open(std::path::Path::new(FIXTURE)).unwrap();
    let mut idx = LineIndex::new();
    idx.set_record_start(Regex::new(r"^\[").unwrap());
    idx.extend_to_end(&src);
    (src, idx)
}

#[test]
fn fixture_has_expected_record_and_line_counts() {
    let (_src, idx) = open_indexed();
    assert_eq!(idx.line_count(), 13);
    assert_eq!(idx.record_count(), 6);
}

#[test]
fn record_zero_covers_banner_only() {
    let (_src, idx) = open_indexed();
    assert_eq!(idx.record_line_range(0), 0..2);
}

#[test]
fn record_two_contains_blank_continuation() {
    let (src, idx) = open_indexed();
    let bytes_cow = idx.record_bytes(2, &src);
    let bytes: &[u8] = &bytes_cow;
    let s = std::str::from_utf8(bytes).unwrap();
    assert!(s.contains("Failed to render template"));
    assert!(s.contains("#2 {main}"));
    assert!(s.contains("\n\n"), "embedded blank line preserved");
}

#[test]
fn line_to_record_round_trips_against_fixture() {
    let (_src, idx) = open_indexed();
    assert_eq!(idx.line_to_record(0), 0);  // banner A
    assert_eq!(idx.line_to_record(1), 0);  // banner B
    assert_eq!(idx.line_to_record(2), 1);  // INFO startup
    assert_eq!(idx.line_to_record(3), 2);  // ERROR header
    for line in 4..=7 {
        assert_eq!(idx.line_to_record(line), 2, "line {line} should be in record 2");
    }
    assert_eq!(idx.line_to_record(8), 3);  // INFO request
}
