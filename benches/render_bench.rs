//! Baseline performance benchmarks. Five scenarios:
//!   1. big_file_open: build a LineIndex over a synthetic 10 MB input.
//!   2. scroll_page: compose one full viewport frame over a large index.
//!   3. regex_search: run a regex over every line of a large input.
//!   4. render_line / count_rows: per-line kernel cost.
//!   5. big_file_open_with_records: same as (1) but with record-start indexing enabled.
//!
//! Run with: cargo bench
//! HTML reports are written to target/criterion/.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::io::Write;
use tempfile::NamedTempFile;

use tess::line_index::LineIndex;
use tess::render::{count_rows, render_line, RenderOpts};
use tess::source::FileSource;
use tess::viewport::Viewport;

const LINE: &[u8] = b"the quick brown fox jumps over the lazy dog 0123456789\n";

fn make_big_input(target_bytes: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(target_bytes + LINE.len());
    while v.len() < target_bytes {
        v.extend_from_slice(LINE);
    }
    v
}

fn bench_big_file_open(c: &mut Criterion) {
    let bytes = make_big_input(10 * 1024 * 1024);
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&bytes).unwrap();

    c.bench_function("big_file_open_10mb", |b| {
        b.iter(|| {
            let src = FileSource::open(tmp.path()).unwrap();
            let mut idx = LineIndex::new();
            idx.extend_to_end(&src);
            black_box(idx.line_count());
        });
    });
}

fn bench_scroll_page(c: &mut Criterion) {
    let bytes = make_big_input(1024 * 1024);
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&bytes).unwrap();
    let src = FileSource::open(tmp.path()).unwrap();
    let mut idx = LineIndex::new();
    idx.extend_to_end(&src);

    let viewport = Viewport::new(80, 24, String::from("bench"));
    c.bench_function("scroll_page_compose_frame", |b| {
        b.iter(|| {
            let frame = viewport.frame(&src, &mut idx);
            black_box(frame);
        });
    });
}

fn bench_regex_search(c: &mut Criterion) {
    let bytes = make_big_input(2 * 1024 * 1024);
    let re = regex::Regex::new(r"lazy\s+dog").unwrap();
    c.bench_function("regex_search_2mb", |b| {
        b.iter(|| {
            let mut hits = 0usize;
            for line in bytes.split(|&b| b == b'\n') {
                if let Ok(s) = std::str::from_utf8(line) {
                    if re.is_match(s) {
                        hits += 1;
                    }
                }
            }
            black_box(hits);
        });
    });
}

fn bench_render_line(c: &mut Criterion) {
    let opts = RenderOpts { tab_width: 8, wrap: true, cols: 80, mode: tess::render::AnsiMode::Strict };
    let line: &[u8] = b"a fairly long line with \t tabs and \x1b control bytes and \xff invalid \xfe utf8 to stress decoding";
    c.bench_function("render_line_80cols", |b| {
        b.iter(|| {
            let rows = render_line(black_box(line), black_box(&opts), None);
            black_box(rows.len());
        });
    });
    c.bench_function("count_rows_80cols", |b| {
        b.iter(|| {
            let n = count_rows(black_box(line), black_box(&opts), None);
            black_box(n);
        });
    });
}

fn bench_big_file_open_with_records(c: &mut Criterion) {
    let bytes = make_big_input(10 * 1024 * 1024);
    let mut tmp = NamedTempFile::new().unwrap();
    tmp.write_all(&bytes).unwrap();

    c.bench_function("big_file_open_10mb_with_records", |b| {
        b.iter(|| {
            let src = FileSource::open(tmp.path()).unwrap();
            let mut idx = LineIndex::new();
            idx.set_record_start(regex::bytes::Regex::new(r"^the").unwrap());
            idx.extend_to_end(&src);
            black_box(idx.record_count());
        });
    });
}

criterion_group!(benches,
    bench_big_file_open,
    bench_scroll_page,
    bench_regex_search,
    bench_render_line,
    bench_big_file_open_with_records,
);
criterion_main!(benches);
