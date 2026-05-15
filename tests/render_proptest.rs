//! Property tests for the `render` kernel. The kernel is pure and
//! total — these properties should hold for any input bytes and any
//! reasonable RenderOpts.

use proptest::prelude::*;
use tess::render::{count_rows, render_line, RenderOpts};

fn opts_strategy() -> impl Strategy<Value = RenderOpts> {
    (1u16..200, any::<bool>(), 1u8..16).prop_map(|(cols, wrap, tab_width)| {
        RenderOpts { cols, wrap, tab_width }
    })
}

proptest! {
    /// Design-spec invariant: counting rows must match rendering rows.
    #[test]
    fn count_rows_matches_render_line_len(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        opts in opts_strategy(),
    ) {
        let counted = count_rows(&bytes, &opts);
        let rendered = render_line(&bytes, &opts).len();
        prop_assert_eq!(counted, rendered);
    }

    /// With wrap disabled, every input collapses to exactly one row.
    #[test]
    fn no_wrap_always_one_row(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
        cols in 1u16..200,
        tab_width in 1u8..16,
    ) {
        let opts = RenderOpts { cols, wrap: false, tab_width };
        prop_assert_eq!(count_rows(&bytes, &opts), 1);
        prop_assert_eq!(render_line(&bytes, &opts).len(), 1);
    }

    /// Totality: `render_line` must never panic, including on invalid
    /// UTF-8 sequences, control bytes, and wide-char boundaries.
    #[test]
    fn render_line_never_panics(
        bytes in proptest::collection::vec(any::<u8>(), 0..1024),
        opts in opts_strategy(),
    ) {
        let _ = render_line(&bytes, &opts);
    }
}
