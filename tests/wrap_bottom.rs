// Regression tests: the "bottom" of a wrapped view must be computed in
// display-row units, not logical-line units. With wrap on (the default), the
// last few logical lines occupy more rows than there are body rows, so
// anchoring on `line_count - body` leaves the true last line off-screen below.
//
// Symptoms this guards against (reported against --live / --follow / End key):
//   * End / goto-bottom lands ~1 page above the actual end.
//   * Pressing End while already at the end jumps *up* ~1 page.
//   * Follow mode stops tracking the end because is_at_bottom() reads true
//     while the real tail is still below the viewport.

use tess::line_index::LineIndex;
use tess::render::Cell;
use tess::source::{MockSource, Source};
use tess::viewport::Viewport;

/// 5 logical lines, each 15 chars at cols=10 -> 2 display rows per line
/// (10 + 5). Distinct leading char per line so we can tell which line a
/// rendered row came from.
fn wrapping_source() -> MockSource {
    let s = MockSource::new();
    for ch in ['a', 'b', 'c', 'd', 'e'] {
        let mut line: String = std::iter::repeat(ch).take(15).collect();
        line.push('\n');
        s.append(line.as_bytes());
    }
    s.finish();
    s
}

fn body_text(viewport: &mut Viewport, src: &dyn Source) -> Vec<String> {
    let mut idx = LineIndex::new();
    let frame = viewport.frame(src, &mut idx);
    frame
        .body
        .iter()
        .map(|row| {
            let mut s = String::new();
            for c in row {
                match c {
                    Cell::Char { ch, .. } => s.push(*ch),
                    Cell::Continuation => {}
                    Cell::Empty => s.push(' '),
                }
            }
            s.trim_end().to_string()
        })
        .collect()
}

#[test]
fn goto_bottom_shows_last_line_when_lines_wrap() {
    let src = wrapping_source();
    let mut idx = LineIndex::new();
    // rows=4 -> body_rows=3. Total display rows = 5 lines * 2 = 10.
    let mut v = Viewport::new(10, 4, "mock".to_string());
    v.goto_bottom(&src, &mut idx);

    let body = body_text(&mut v, &src);
    // The true bottom shows the last 3 rows: d-tail, e-head, e-tail.
    assert_eq!(body, vec!["ddddd", "eeeeeeeeee", "eeeee"]);
    assert!(
        body.last().unwrap().starts_with('e'),
        "last body row must be the final line's tail, got {body:?}"
    );
}

#[test]
fn is_at_bottom_false_when_wrapped_tail_is_offscreen() {
    let src = wrapping_source();
    let mut idx = LineIndex::new();
    idx.notice_new_bytes(&src);
    let mut v = Viewport::new(10, 4, "mock".to_string());
    // The OLD (buggy) anchor: top_line = total(5) - body(3) = 2, top_row = 0.
    // From line 2 (0-indexed) the frame shows c,c,d -- the real end (e) is
    // still below, so we are NOT at the bottom.
    v.goto_line(2, &src, &mut idx);
    assert!(
        !v.is_at_bottom(&src, &idx),
        "viewport whose tail is still off-screen must not report at-bottom"
    );
}

#[test]
fn cannot_scroll_below_the_bottom_anchor() {
    let src = wrapping_source();
    let mut idx = LineIndex::new();
    let mut v = Viewport::new(10, 4, "mock".to_string());
    // Scroll down far more than the document is tall.
    v.scroll_lines(100, &src, &mut idx);
    let body = body_text(&mut v, &src);
    assert_eq!(
        body,
        vec!["ddddd", "eeeeeeeeee", "eeeee"],
        "scrolling past the end must clamp with the last line at the bottom row"
    );
    assert!(v.is_at_bottom(&src, &idx));
}
