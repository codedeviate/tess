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

#[test]
fn startup_follow_anchors_end_when_last_line_wraps_many_rows() {
    // A handful of short lines, then a final line far longer than the body.
    // Mimics the --follow/--live startup snap: extend then goto_bottom.
    let s = MockSource::new();
    s.append(b"one\ntwo\nthree\n");
    s.append(&[b'z'; 95]); // 95 chars at cols=10 -> 10 wrap rows
    s.append(b"\n");
    s.finish();

    let mut idx = LineIndex::new();
    let mut v = Viewport::new(10, 6, "mock".to_string()); // body_rows = 5
    v.set_follow_mode(true);
    // Replicate app.rs startup snap order:
    idx.notice_new_bytes(&s);
    v.extend_visible_lines(&idx, &s);
    v.goto_bottom(&s, &mut idx);

    let body = body_text(&mut v, &s);
    eprintln!("body = {body:?}");
    // The final body row must be the TAIL of the last line (its last 5 chars),
    // not its start. 95 z's -> rows of 10 except the last row of 5.
    assert_eq!(body.last().unwrap(), "zzzzz", "should show the END of the wrapped last line, got {body:?}");
}

#[test]
fn startup_follow_anchors_end_when_only_last_line_wraps() {
    // Two short lines + one very long line, total just over the body.
    let s = MockSource::new();
    s.append(b"alpha\n");
    s.append(&[b'q'; 35]); // 35 chars at cols=10 -> 4 wrap rows
    s.append(b"\n");
    s.finish();

    let mut idx = LineIndex::new();
    let mut v = Viewport::new(10, 4, "mock".to_string()); // body_rows = 3
    v.set_follow_mode(true);
    idx.notice_new_bytes(&s);
    v.extend_visible_lines(&idx, &s);
    v.goto_bottom(&s, &mut idx);

    let body = body_text(&mut v, &s);
    eprintln!("body = {body:?}");
    assert_eq!(body.last().unwrap(), "qqqqq", "should show the END of the wrapped last line, got {body:?}");
}

#[test]
fn real_file_sources_anchor_wrapping_last_line() {
    use tess::source::{FileSource, LiveFileSource};
    let dir = std::env::temp_dir().join("tess_wrap_repro");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("f.txt");
    // 3 short lines + a final 95-char line that wraps to 10 rows at cols=10.
    let mut content = b"one\ntwo\nthree\n".to_vec();
    content.extend_from_slice(&[b'z'; 95]);
    content.push(b'\n');
    std::fs::write(&path, &content).unwrap();

    for which in ["file", "live"] {
        let src: Box<dyn Source> = match which {
            "file" => Box::new(FileSource::open(&path).unwrap()),
            _ => Box::new(LiveFileSource::open(&path).unwrap()),
        };
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 6, "f".to_string()); // body_rows = 5
        v.set_follow_mode(true);
        // app.rs startup snap order:
        src.pump();
        idx.notice_new_bytes(src.as_ref());
        v.extend_visible_lines(&idx, src.as_ref());
        v.goto_bottom(src.as_ref(), &mut idx);

        let body = body_text(&mut v, src.as_ref());
        eprintln!("[{which}] body = {body:?}");
        assert_eq!(
            body.last().unwrap(),
            "zzzzz",
            "[{which}] should show the END of the wrapped last line, got {body:?}"
        );
    }
}

#[test]
fn last_line_no_trailing_newline_anchors_end() {
    let s = MockSource::new();
    s.append(b"one\ntwo\nthree\n");
    s.append(&[b'z'; 95]); // NO trailing newline
    s.finish();
    let mut idx = LineIndex::new();
    let mut v = Viewport::new(10, 6, "mock".to_string()); // body 5
    v.set_follow_mode(true);
    idx.notice_new_bytes(&s);
    v.extend_visible_lines(&idx, &s);
    v.goto_bottom(&s, &mut idx);
    let body = body_text(&mut v, &s);
    eprintln!("no-newline body = {body:?}");
    assert_eq!(body.last().unwrap(), "zzzzz", "got {body:?}");
}

#[test]
fn hide_mode_wrapping_last_line_shows_end() {
    // Filtered (grep) follow where the last matching line wraps to a few rows
    // — the common case. goto_bottom must anchor the kept line's END at the
    // bottom, not its start.
    let s = MockSource::new();
    s.append(b"DROP me\n");
    s.append(b"KEEP short\n");
    s.append(b"DROP again\n");
    let mut keep = b"KEEP".to_vec();
    keep.extend_from_slice(&[b'z'; 36]); // "KEEP"+36 z = 40 chars @ cols10 -> 4 rows
    keep.push(b'\n');
    s.append(&keep);
    s.finish();

    let mut idx = LineIndex::new();
    let mut v = Viewport::new(10, 6, "mock".to_string()); // body 5
    v.set_follow_mode(true);
    v.set_grep(Some(
        tess::grep::GrepPredicate::compile(
            &["^KEEP".to_string()],
            tess::viewport::CaseMode::Sensitive,
        )
        .unwrap(),
    ));
    idx.notice_new_bytes(&s);
    v.extend_visible_lines(&idx, &s);
    v.goto_bottom(&s, &mut idx);
    let body = body_text(&mut v, &s);
    eprintln!("hide-mode body = {body:?}");
    // 40 chars / 10 cols = 4 rows; the final row is the last 10 z's.
    assert_eq!(body.last().unwrap(), "zzzzzzzzzz", "filtered wrapping last line: END must be at the bottom, got {body:?}");
    assert!(v.is_at_bottom(&s, &idx));
}

#[test]
fn count_rows_matches_render_and_no_phantom_line() {
    use tess::render::{count_rows, render_line, RenderOpts};
    let opts = RenderOpts { cols: 10, ..RenderOpts::default() };
    for len in [9usize, 10, 11, 19, 20, 21, 30] {
        let line: Vec<u8> = std::iter::repeat(b'x').take(len).collect();
        let cr = count_rows(&line, &opts, None);
        let rl = render_line(&line, &opts, None).len();
        eprintln!("len={len}: count_rows={cr} render_line={rl} {}", if cr==rl {"OK"} else {"MISMATCH"});
        assert_eq!(cr, rl, "count_rows != render_line rows for len {len}");
    }
    // Phantom trailing-line check on a real-ish MockSource ending in \n.
    let s = MockSource::new();
    s.append(b"a\nb\nc\n");
    s.finish();
    let mut idx = LineIndex::new();
    idx.notice_new_bytes(&s);
    eprintln!("line_count for \"a\\nb\\nc\\n\" = {}", idx.line_count());
    assert_eq!(idx.line_count(), 3, "trailing newline must not add a phantom empty line");
}

#[test]
fn resize_smaller_then_repin_keeps_end_in_view() {
    // The resize bug: a viewport pinned at the bottom of a wrapping tail must
    // re-pin when the body shrinks (terminals emit a resize on startup). This
    // exercises the mechanism the app's resize handler relies on: after a
    // resize, goto_bottom re-anchors the END at the bottom.
    let s = MockSource::new();
    s.append(b"one\ntwo\nthree\n");
    s.append(&[b'z'; 35]); // 35 chars @ cols10 -> 4 rows
    s.append(b"\n");
    s.finish();
    let mut idx = LineIndex::new();
    // Tall viewport: everything (3 + 4 = 7 rows) fits in body 11.
    let mut v = Viewport::new(10, 12, "mock".to_string());
    v.set_follow_mode(true);
    idx.notice_new_bytes(&s);
    v.extend_visible_lines(&idx, &s);
    v.goto_bottom(&s, &mut idx);
    assert!(v.is_at_bottom(&s, &idx));

    // Shrink the body; the old top no longer shows the end -> not at bottom.
    v.resize(10, 5); // body 4
    assert!(!v.is_at_bottom(&s, &idx), "after shrink the tail is off-screen");

    // The app re-pins: goto_bottom must put the END back at the bottom row.
    v.goto_bottom(&s, &mut idx);
    let body = body_text(&mut v, &s);
    assert_eq!(body.last().unwrap(), "zzzzz", "END must be at the bottom after re-pin, got {body:?}");
    assert!(v.is_at_bottom(&s, &idx));
}
