// Reproduces the user's reported scenario: 10 logical lines, each 5000 chars,
// in a 100×40 viewport. Each j press should advance one screen-row, walking
// through wrap rows so the entire content of every line is reachable.

use tess::line_index::LineIndex;
use tess::render::Cell;
use tess::source::{FileSource, MockSource, Source};
use tess::viewport::Viewport;

fn first_body_row(viewport: &Viewport, src: &dyn Source) -> String {
    let mut idx = LineIndex::new();
    let frame = viewport.frame(src, &mut idx);
    let row = &frame.body[0];
    let mut s = String::new();
    for c in row.iter().take(20) {
        if let Cell::Char { ch, .. } = c { s.push(*ch); }
    }
    s
}

#[test]
fn user_scenario_scroll_walks_wraps() {
    let m = MockSource::new();
    let mut content = Vec::new();
    for i in 0..10 {
        let marker = format!("L{:02}_", i);
        let mut row = String::new();
        while row.len() < 5000 {
            row.push_str(&marker);
        }
        row.truncate(5000);
        content.extend_from_slice(row.as_bytes());
        content.push(b'\n');
    }
    m.append(&content);
    m.finish();
    let mut idx = LineIndex::new();

    // 100 cols × 40 rows → body_rows = 39. Each line wraps to 50 wrap rows.
    let mut v = Viewport::new(100, 40, "test".into());

    let initial = first_body_row(&v, &m);
    assert!(initial.starts_with("L00_"), "initial row 0 should start with L00_, got {:?}", initial);

    // One j: still inside line 0 (it has 50 wraps), top body row should
    // still be L00_ content (just one wrap row down).
    v.scroll_lines(1, &m, &mut idx);
    let after_one = first_body_row(&v, &m);
    assert!(after_one.starts_with("L00_"), "after 1 j we should still be in line 0; got {:?}", after_one);

    // 12 j presses: with 39 body rows + 50 wrap rows in line 0,
    // top_row=12 makes wrap rows 12..50 visible (the END of line 0).
    for _ in 0..11 { v.scroll_lines(1, &m, &mut idx); }
    let after_twelve = first_body_row(&v, &m);
    assert!(after_twelve.starts_with("L00_"), "after 12 j still in line 0; got {:?}", after_twelve);

    // 50 j presses total: line 0 has 50 wrap rows, so we land on line 1 wrap 0.
    for _ in 0..38 { v.scroll_lines(1, &m, &mut idx); }
    let after_fifty = first_body_row(&v, &m);
    assert!(after_fifty.starts_with("L01_"), "after 50 j we should be at line 1; got {:?}", after_fifty);
}

#[test]
fn user_scenario_with_real_filesource() {
    // Same scenario but via FileSource (matches the real binary's path).
    let path = std::env::temp_dir().join("tess_user_scenario.log");
    let mut content = Vec::new();
    for i in 0..10 {
        let marker = format!("L{:02}_", i);
        let mut row = String::new();
        while row.len() < 5000 { row.push_str(&marker); }
        row.truncate(5000);
        content.extend_from_slice(row.as_bytes());
        content.push(b'\n');
    }
    std::fs::write(&path, &content).unwrap();
    let src = FileSource::open(&path).unwrap();
    let mut idx = LineIndex::new();
    let mut v = Viewport::new(100, 40, "test".into());

    // Scroll 11 times. body_rows = 39, line 0 has 50 wraps. At top=(0,11),
    // the body should show wraps 11..49 (= 39 rows) of line 0. The bottom
    // body row must be the LAST wrap of line 0 — i.e., the end is reachable.
    for _ in 0..11 { v.scroll_lines(1, &src, &mut idx); }
    let frame = v.frame(&src, &mut idx);
    let body_last = &frame.body[frame.body.len() - 1];
    let mut s = String::new();
    for c in body_last.iter() {
        if let Cell::Char { ch, .. } = c { s.push(*ch); }
    }
    assert!(s.starts_with("L00_"), "bottom body row should still be wrap 49 of line 0 (the END); got {:?}", s);
}
