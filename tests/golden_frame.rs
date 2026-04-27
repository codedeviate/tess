use std::path::Path;

use tess::line_index::LineIndex;
use tess::render::Cell;
use tess::source::FileSource;
use tess::viewport::Viewport;

fn render_to_strings(viewport: &Viewport, src: &dyn tess::source::Source) -> (Vec<String>, String) {
    let mut idx = LineIndex::new();
    let frame = viewport.frame(src, &mut idx);
    let body: Vec<String> = frame.body.iter().map(|row| {
        let mut s = String::new();
        for c in row {
            match c {
                Cell::Char { ch, .. } => s.push(*ch),
                Cell::Continuation => {}
                Cell::Empty => s.push(' '),
            }
        }
        s.trim_end().to_string()
    }).collect();
    (body, frame.status)
}

#[test]
fn first_frame_no_options() {
    let path = Path::new("tests/fixtures/small.txt");
    let src = FileSource::open(path).unwrap();
    let v = Viewport::new(20, 6, path.display().to_string());  // body = 5
    let (body, status) = render_to_strings(&v, &src);
    assert_eq!(body, vec![
        "alpha",
        "beta",
        "gamma",
        "delta",
        "epsilon",
    ]);
    assert!(status.starts_with(&format!("{}  1-5/10", path.display())));
}

#[test]
fn first_frame_with_line_numbers() {
    let path = Path::new("tests/fixtures/small.txt");
    let src = FileSource::open(path).unwrap();
    let mut v = Viewport::new(20, 6, path.display().to_string());
    v.toggle_line_numbers();
    let (body, _) = render_to_strings(&v, &src);
    assert_eq!(body, vec![
        " 1 alpha",
        " 2 beta",
        " 3 gamma",
        " 4 delta",
        " 5 epsilon",
    ]);
}
