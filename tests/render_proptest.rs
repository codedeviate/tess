//! Property tests for the `render` kernel. The kernel is pure and
//! total — these properties should hold for any input bytes and any
//! reasonable RenderOpts.

use proptest::prelude::*;
use regex::bytes::Regex as BytesRegex;
use tess::line_index::LineIndex;
use tess::render::{count_rows, render_line, RenderOpts};
use tess::source::MockSource;

fn opts_strategy() -> impl Strategy<Value = RenderOpts> {
    (1u16..200, any::<bool>(), 1u8..16).prop_map(|(cols, wrap, tab_width)| {
        RenderOpts { cols, wrap, tab_width, ..RenderOpts::default() }
    })
}

proptest! {
    /// Design-spec invariant: counting rows must match rendering rows.
    #[test]
    fn count_rows_matches_render_line_len(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        opts in opts_strategy(),
    ) {
        let counted = count_rows(&bytes, &opts, None);
        let rendered = render_line(&bytes, &opts, None).len();
        prop_assert_eq!(counted, rendered);
    }

    /// With wrap disabled, every input collapses to exactly one row.
    #[test]
    fn no_wrap_always_one_row(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        cols in 1u16..200,
        tab_width in 1u8..16,
    ) {
        let opts = RenderOpts { cols, wrap: false, tab_width, ..RenderOpts::default() };
        prop_assert_eq!(count_rows(&bytes, &opts, None), 1);
        prop_assert_eq!(render_line(&bytes, &opts, None).len(), 1);
    }

    /// Totality: `render_line` must never panic, including on invalid
    /// UTF-8 sequences, control bytes, and wide-char boundaries.
    #[test]
    fn render_line_never_panics(
        bytes in proptest::collection::vec(any::<u8>(), 0..1024),
        opts in opts_strategy(),
    ) {
        let _ = render_line(&bytes, &opts, None);
    }

    /// Every record contains at least one physical line, but a record can
    /// span multiple lines. So `record_count <= line_count` always (except
    /// for empty sources where both are zero).
    #[test]
    fn record_count_le_line_count(
        bytes in proptest::collection::vec(any::<u8>(), 0..1024),
    ) {
        let src = MockSource::new();
        src.append(&bytes);
        let mut idx = LineIndex::new();
        idx.set_record_start(BytesRegex::new(r"^\[").unwrap());
        idx.extend_to_end(&src);
        prop_assert!(idx.record_count() <= idx.line_count(),
                     "records={} lines={}",
                     idx.record_count(), idx.line_count());
    }

    /// line_to_record is non-decreasing as line index grows.
    #[test]
    fn line_to_record_is_monotonic(
        bytes in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        let src = MockSource::new();
        src.append(&bytes);
        let mut idx = LineIndex::new();
        idx.set_record_start(BytesRegex::new(r"^\[").unwrap());
        idx.extend_to_end(&src);
        let mut prev = 0usize;
        for line_n in 0..idx.line_count() {
            let r = idx.line_to_record(line_n);
            prop_assert!(r >= prev, "non-monotonic: line {} → record {} after {}",
                         line_n, r, prev);
            prev = r;
        }
    }

    /// record_count never decreases as bytes are appended.
    #[test]
    fn record_count_monotonic_under_appends(
        chunks in proptest::collection::vec(
            proptest::collection::vec(any::<u8>(), 0..128),
            0..16,
        ),
    ) {
        let src = MockSource::new();
        let mut idx = LineIndex::new();
        idx.set_record_start(BytesRegex::new(r"^\[").unwrap());
        let mut prev = 0usize;
        for chunk in chunks {
            src.append(&chunk);
            idx.notice_new_bytes(&src);
            let now = idx.record_count();
            prop_assert!(now >= prev,
                         "record_count decreased: {} -> {}", prev, now);
            prev = now;
        }
    }
}
