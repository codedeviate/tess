// Hide mode (--filter / --grep without --dim) must honor `top_row` so the
// viewport can scroll *into* a matching line's wrap rows — same as non-hide
// mode. Before this, hide mode scrolled by whole visible lines with
// top_row==0, so a matching line taller than the body could never show its
// tail, and goto_bottom anchored on the line's start.

use tess::line_index::LineIndex;
use tess::render::Cell;
use tess::source::{MockSource, Source};
use tess::viewport::{CaseMode, Viewport};

fn grep_source() -> MockSource {
    let s = MockSource::new();
    s.append(b"DROP a\n");
    s.append(b"KEEP one\n");
    s.append(b"DROP b\n");
    // A matching line 59 chars long -> 6 wrap rows at cols=10; last row ends "END".
    let mut long = b"KEEP".to_vec();
    long.extend_from_slice(&[b'x'; 52]);
    long.extend_from_slice(b"END");
    long.push(b'\n');
    s.append(&long);
    s.finish();
    s
}

fn hide_viewport(cols: u16, rows: u16) -> Viewport {
    let mut v = Viewport::new(cols, rows, "mock".to_string());
    v.set_grep(Some(
        tess::grep::GrepPredicate::compile(&["^KEEP".to_string()], CaseMode::Sensitive).unwrap(),
    ));
    v
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
fn hide_goto_bottom_shows_tail_of_tall_matching_line() {
    let src = grep_source();
    let mut idx = LineIndex::new();
    let mut v = hide_viewport(10, 4); // body = 3
    idx.notice_new_bytes(&src);
    v.extend_visible_lines(&idx, &src);
    v.goto_bottom(&src, &mut idx);

    let body = body_text(&mut v, &src);
    // The 59-char match wraps to 6 rows; the bottom 3 must end with the tail row.
    assert_eq!(
        body.last().unwrap(),
        "xxxxxxEND",
        "goto_bottom must anchor the matching line's END at the bottom, got {body:?}"
    );
    assert!(v.is_at_bottom(&src, &idx));
}

#[test]
fn hide_scroll_walks_wrap_rows_then_clamps() {
    let src = grep_source();
    let mut idx = LineIndex::new();
    let mut v = hide_viewport(10, 4); // body = 3
    idx.notice_new_bytes(&src);
    v.extend_visible_lines(&idx, &src);
    // Start at top: visible lines are "KEEP one" (1 row) and the 6-row match.
    // Scroll down row by row and watch top advance through wrap rows.
    v.scroll_lines(1, &src, &mut idx); // into the long line, row 0
    let b1 = body_text(&mut v, &src);
    assert_eq!(b1[0], "KEEPxxxxxx", "first row should be the long match's row 0");
    // Scroll to the very bottom; clamp must leave the END on the last row.
    v.scroll_lines(100, &src, &mut idx);
    let b2 = body_text(&mut v, &src);
    assert_eq!(b2.last().unwrap(), "xxxxxxEND");
    assert!(v.is_at_bottom(&src, &idx));
    // Already at the bottom: scrolling further is a no-op.
    let before = b2.clone();
    v.scroll_lines(5, &src, &mut idx);
    assert_eq!(body_text(&mut v, &src), before, "must not scroll past the bottom");
}

#[test]
fn hide_scroll_up_snaps_within_wraps() {
    let src = grep_source();
    let mut idx = LineIndex::new();
    let mut v = hide_viewport(10, 4); // body = 3
    idx.notice_new_bytes(&src);
    v.extend_visible_lines(&idx, &src);
    v.goto_bottom(&src, &mut idx); // deep inside the long match's wraps
    let bottom = body_text(&mut v, &src);
    // Scroll up one row: the view should move up by exactly one wrap row.
    v.scroll_lines(-1, &src, &mut idx);
    let up = body_text(&mut v, &src);
    assert_ne!(up, bottom, "scrolling up one row must change the view");
    assert!(!v.is_at_bottom(&src, &idx), "after scrolling up we are no longer at the bottom");
}
