use tess::filter::{CompiledFilter, FilterSpec};
use tess::format;
use tess::line_index::LineIndex;
use tess::render::Cell;
use tess::source::MockSource;
use tess::viewport::{RowStyle, Viewport};

const APACHE_LOG: &[u8] = br#"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] "GET /index.html HTTP/1.1" 200 2326 "-" "Mozilla/5.0"
10.1.2.3 - bob [10/Oct/2023:13:55:37 +0000] "POST /api/login HTTP/1.1" 401 512 "-" "curl/7.0"
192.168.1.1 - carol [10/Oct/2023:13:55:38 +0000] "GET /api/data HTTP/1.1" 500 128 "-" "test/1.0"
127.0.0.1 - alice [10/Oct/2023:13:55:39 +0000] "GET /favicon.ico HTTP/1.1" 200 89 "-" "Mozilla/5.0"
10.1.2.3 - bob [10/Oct/2023:13:55:40 +0000] "GET /api/users HTTP/1.1" 503 256 "-" "curl/7.0"
"#;

fn setup() -> (MockSource, LineIndex, Viewport) {
    let m = MockSource::new();
    m.append(APACHE_LOG);
    m.finish();
    let idx = LineIndex::new();
    let v = Viewport::new(120, 7, "access.log".into()); // body = 6, fits all 5 lines
    (m, idx, v)
}

fn body_starts_with(body: &[Vec<Cell>], idx: usize, prefix: &str) -> bool {
    let chars: String = body[idx]
        .iter()
        .filter_map(|c| match c {
            Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        })
        .collect();
    chars.starts_with(prefix)
}

#[test]
fn filter_hide_mode_shows_only_matching_lines() {
    let (m, mut idx, mut v) = setup();
    let formats = format::load_all().unwrap();
    let fmt = &formats["apache-combined"];
    let specs = vec![FilterSpec::parse("status~^5").unwrap()];
    let f = CompiledFilter::compile(fmt, specs, tess::viewport::CaseMode::Sensitive).unwrap();
    v.set_filter(Some(f));
    idx.extend_to_end(&m);
    v.extend_visible_lines(&idx, &m);
    let frame = v.frame(&m, &mut idx);
    // Only lines 3 (status 500) and 5 (status 503) should appear.
    assert!(body_starts_with(&frame.body, 0, "192.168.1.1"));
    assert!(body_starts_with(&frame.body, 1, "10.1.2.3 - bob"));
    // Row 2..end should be padding (empty cells).
    assert!(frame.body[2].iter().all(|c| matches!(c, Cell::Empty)));
    // All visible rows are Normal style in hide mode.
    assert_eq!(frame.row_styles[0], RowStyle::Normal);
    assert_eq!(frame.row_styles[1], RowStyle::Normal);
    // Status line shows the format and a (matched/total) count.
    assert!(frame.status.contains("[apache-combined]"), "status: {}", frame.status);
    assert!(frame.status.contains("2/5"), "status: {}", frame.status);
}

#[test]
fn filter_dim_mode_shows_all_lines_with_non_matches_dimmed() {
    let (m, mut idx, mut v) = setup();
    let formats = format::load_all().unwrap();
    let fmt = &formats["apache-combined"];
    let specs = vec![FilterSpec::parse("status~^5").unwrap()];
    let f = CompiledFilter::compile(fmt, specs, tess::viewport::CaseMode::Sensitive).unwrap();
    v.set_filter(Some(f));
    v.set_dim_mode(true);
    idx.extend_to_end(&m);
    let frame = v.frame(&m, &mut idx);
    // All 5 source lines visible plus padding.
    assert!(body_starts_with(&frame.body, 0, "127.0.0.1 - alice"));
    assert!(body_starts_with(&frame.body, 2, "192.168.1.1"));
    assert!(body_starts_with(&frame.body, 4, "10.1.2.3 - bob"));
    // Lines that don't match the filter should be Dim, matching ones Normal.
    // Row 0: alice 200 → Dim. Row 1: bob 401 → Dim. Row 2: carol 500 → Normal.
    // Row 3: alice 200 → Dim. Row 4: bob 503 → Normal.
    assert_eq!(frame.row_styles[0], RowStyle::Dim);
    assert_eq!(frame.row_styles[1], RowStyle::Dim);
    assert_eq!(frame.row_styles[2], RowStyle::Normal);
    assert_eq!(frame.row_styles[3], RowStyle::Dim);
    assert_eq!(frame.row_styles[4], RowStyle::Normal);
    assert!(frame.status.contains("[dim]"), "status: {}", frame.status);
}

#[test]
fn multi_filter_and_semantics() {
    let (m, mut idx, mut v) = setup();
    let formats = format::load_all().unwrap();
    let fmt = &formats["apache-combined"];
    let specs = vec![
        FilterSpec::parse("status~^5").unwrap(),
        FilterSpec::parse(r"url~/api/").unwrap(),
    ];
    let f = CompiledFilter::compile(fmt, specs, tess::viewport::CaseMode::Sensitive).unwrap();
    v.set_filter(Some(f));
    idx.extend_to_end(&m);
    v.extend_visible_lines(&idx, &m);
    let frame = v.frame(&m, &mut idx);
    // Both /api/data 500 (line 3) and /api/users 503 (line 5) match.
    assert!(body_starts_with(&frame.body, 0, "192.168.1.1"));
    assert!(body_starts_with(&frame.body, 1, "10.1.2.3 - bob"));
    assert!(frame.status.contains("2/5"));
}
