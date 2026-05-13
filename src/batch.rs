//! Non-interactive write mode (`--output FILE` / `--stdout`).
//!
//! Walks the source from beginning to end applying any --filter / --head /
//! --tail / --prettify already baked into the `Source` + `LineIndex`, writes
//! the surviving logical lines as raw bytes to the destination, then exits.
//!
//! With `--follow`, doesn't exit — keeps polling for appended bytes and
//! appends matching new lines to the destination, exactly mirroring the
//! viewer's auto-scroll behavior. SIGTERM/SIGHUP and Ctrl-C cleanly close
//! the file.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::filter::{CompiledFilter, FilterMatch};
use crate::format::DisplayRenderer;
use crate::grep::GrepPredicate;
use crate::line_index::LineIndex;
use crate::source::Source;

/// Where the batch run writes its output.
pub enum BatchDestination {
    Stdout,
    File(PathBuf),
}

pub struct BatchSpec {
    pub destination: BatchDestination,
    /// When true, after the initial pass keep polling the source for new
    /// bytes and append matching lines until SIGTERM/SIGHUP arrives.
    pub follow: bool,
    /// Poll cadence for follow mode. 250 ms matches the interactive loop.
    pub poll_interval: Duration,
}

impl Default for BatchSpec {
    fn default() -> Self {
        Self {
            destination: BatchDestination::Stdout,
            follow: false,
            poll_interval: Duration::from_millis(250),
        }
    }
}

pub fn run(
    src: Box<dyn Source>,
    mut idx: LineIndex,
    filter: Option<CompiledFilter>,
    grep: Option<GrepPredicate>,
    display: Option<DisplayRenderer>,
    spec: BatchSpec,
    sigterm: Arc<AtomicBool>,
) -> Result<()> {
    let mut out: Box<dyn Write> = match &spec.destination {
        BatchDestination::Stdout => Box::new(io::stdout().lock()),
        BatchDestination::File(path) => {
            let f = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(path)
                .map_err(|e| Error::Runtime(format!("open {}: {e}", path.display())))?;
            Box::new(f)
        }
    };

    // First pass: extend index to the current end of the source and write all
    // matching lines. Static file sources resolve their full length here;
    // streaming stdin sources whose initial bytes are present do too.
    idx.extend_to_end(src.as_ref());
    let mut next_line = 0usize;
    next_line = emit_pending(src.as_ref(), &mut idx, filter.as_ref(), grep.as_ref(), display.as_ref(), &mut *out, next_line)?;
    out.flush().map_err(|e| Error::Runtime(format!("flush: {e}")))?;

    if !spec.follow {
        return Ok(());
    }

    // Follow mode: poll for new bytes, emit any new matching lines, repeat
    // until interrupted. We don't fight stdin/tty here because we never
    // entered raw mode — Ctrl-C is delivered to the process directly.
    while !sigterm.load(Ordering::SeqCst) {
        std::thread::sleep(spec.poll_interval);
        if !src.is_complete() {
            src.pump();
        }
        let lines_before = idx.line_count();
        idx.notice_new_bytes(src.as_ref());
        if idx.line_count() != lines_before {
            next_line = emit_pending(src.as_ref(), &mut idx, filter.as_ref(), grep.as_ref(), display.as_ref(), &mut *out, next_line)?;
            out.flush().map_err(|e| Error::Runtime(format!("flush: {e}")))?;
        }
        // For static sources, nothing more will ever arrive — break out so
        // `--follow --output FILE somefile` doesn't sit forever.
        if src.is_complete() && idx.line_count() == lines_before {
            // No growth in this tick; if the source is complete we're done.
            // (For incomplete streaming sources we keep waiting.)
            // Safety: `lines_before` was sampled before pump() above, but
            // `is_complete()` is sticky-true once set, so this is a safe early
            // exit only when the source has truly finished. Keep polling
            // otherwise — the next tick may bring more.
            break;
        }
    }
    Ok(())
}

/// Emit lines `[next_line, idx.line_count())` that pass the filter (or all of
/// them if no filter is bound). Returns the new `next_line` cursor. When a
/// `DisplayRenderer` is supplied, parsed lines are written through the
/// template; lines that don't parse fall back to the raw bytes so no data is
/// silently lost.
fn emit_pending(
    src: &dyn Source,
    idx: &mut LineIndex,
    filter: Option<&CompiledFilter>,
    grep: Option<&GrepPredicate>,
    display: Option<&DisplayRenderer>,
    out: &mut dyn Write,
    mut next_line: usize,
) -> Result<usize> {
    let total = idx.line_count();
    while next_line < total {
        let range = idx.line_range(next_line, src);
        let bytes = src.bytes(range);
        let filter_ok = match filter {
            None => true,
            Some(f) => matches!(f.evaluate(&bytes), FilterMatch::Matched),
        };
        let grep_ok = match grep {
            None => true,
            Some(g) => g.matches(&bytes),
        };
        if filter_ok && grep_ok {
            match display.and_then(|r| r.render_line(&bytes)) {
                Some(rendered) => {
                    out.write_all(rendered.as_bytes()).map_err(|e| Error::Runtime(format!("write: {e}")))?;
                }
                None => {
                    out.write_all(&bytes).map_err(|e| Error::Runtime(format!("write: {e}")))?;
                }
            }
            out.write_all(b"\n").map_err(|e| Error::Runtime(format!("write: {e}")))?;
        }
        next_line += 1;
    }
    Ok(next_line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::LogFormat;
    use crate::filter::FilterSpec;
    use crate::source::MockSource;
    use std::io::Read;

    fn run_to_vec(
        src: Box<dyn Source>,
        idx: LineIndex,
        filter: Option<CompiledFilter>,
        grep: Option<crate::grep::GrepPredicate>,
        display: Option<crate::format::DisplayRenderer>,
    ) -> Vec<u8> {
        // Use a tempfile destination since BatchDestination::Stdout would
        // capture the test runner's stdout. Easier and more honest.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        run(
            src,
            idx,
            filter,
            grep,
            display,
            BatchSpec {
                destination: BatchDestination::File(path.clone()),
                follow: false,
                poll_interval: Duration::from_millis(50),
            },
            Arc::new(AtomicBool::new(false)),
        ).unwrap();
        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        buf
    }

    #[test]
    fn writes_all_lines_unfiltered() {
        let m = MockSource::new();
        m.append(b"alpha\nbeta\ngamma\n");
        m.finish();
        let out = run_to_vec(Box::new(m), LineIndex::new(), None, None, None);
        assert_eq!(out, b"alpha\nbeta\ngamma\n");
    }

    #[test]
    fn display_template_rewrites_lines() {
        let m = MockSource::new();
        m.append(b"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] \"GET /a HTTP/1.1\" 200 1024 \"-\" \"-\"\n");
        m.append(b"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] \"GET /b HTTP/1.1\" 500 64 \"-\" \"-\"\n");
        m.finish();

        let fmt = LogFormat::compile(
            "apache-combined",
            r#"^(?P<ip>\S+) \S+ (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+) "(?P<referer>[^"]*)" "(?P<agent>[^"]*)"$"#,
        ).unwrap();
        let template = crate::format::DisplayTemplate::compile("<status> <method> <url>", &fmt.field_names).unwrap();
        let renderer = crate::format::DisplayRenderer::new(template, fmt.regex.clone());

        let out = run_to_vec(Box::new(m), LineIndex::new(), None, None, Some(renderer));
        let s = std::str::from_utf8(&out).unwrap();
        assert_eq!(s, "200 GET /a\n500 GET /b\n");
    }

    #[test]
    fn display_falls_back_to_raw_when_line_doesnt_parse() {
        let m = MockSource::new();
        // A single word with no space — won't match `\w+ .+`.
        m.append(b"singleword\n");
        m.finish();

        let fmt = LogFormat::compile(
            "simple",
            r"^(?P<level>\w+) (?P<msg>.+)$",
        ).unwrap();
        let template = crate::format::DisplayTemplate::compile("<level>: <msg>", &fmt.field_names).unwrap();
        let renderer = crate::format::DisplayRenderer::new(template, fmt.regex.clone());

        let out = run_to_vec(Box::new(m), LineIndex::new(), None, None, Some(renderer));
        // Falls back to the raw line so data isn't lost.
        assert_eq!(out, b"singleword\n");
    }

    #[test]
    fn filter_drops_non_matches() {
        let m = MockSource::new();
        m.append(b"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] \"GET / HTTP/1.1\" 200 1024 \"-\" \"-\"\n");
        m.append(b"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] \"GET /api HTTP/1.1\" 500 64 \"-\" \"-\"\n");
        m.append(b"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] \"GET /b HTTP/1.1\" 404 12 \"-\" \"-\"\n");
        m.finish();

        let fmt = LogFormat::compile(
            "apache-combined",
            r#"^(?P<ip>\S+) \S+ (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+) "(?P<referer>[^"]*)" "(?P<agent>[^"]*)"$"#,
        ).unwrap();
        let f = CompiledFilter::compile(
            &fmt,
            vec![FilterSpec::parse("status>=500").unwrap()],
        ).unwrap();

        let out = run_to_vec(Box::new(m), LineIndex::new(), Some(f), None, None);
        let s = std::str::from_utf8(&out).unwrap();
        assert_eq!(s.lines().count(), 1);
        assert!(s.contains("/api"), "expected the 500 line, got {:?}", s);
    }

    #[test]
    fn head_cap_limits_output() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n5\n");
        m.finish();
        let mut idx = LineIndex::new();
        idx.set_head_cap(3);
        let out = run_to_vec(Box::new(m), idx, None, None, None);
        assert_eq!(out, b"1\n2\n3\n");
    }

    #[test]
    fn grep_filters_in_batch_mode() {
        use crate::grep::GrepPredicate;
        let m = MockSource::new();
        m.append(b"keep error one\n");
        m.append(b"drop me\n");
        m.append(b"keep error two\n");
        m.finish();
        let g = GrepPredicate::compile(&["error".to_string()]).unwrap();
        let out = run_to_vec(Box::new(m), LineIndex::new(), None, Some(g), None);
        assert_eq!(out, b"keep error one\nkeep error two\n");
    }

    #[test]
    fn filter_and_grep_combine_in_batch_mode() {
        use crate::grep::GrepPredicate;
        let m = MockSource::new();
        m.append(b"ERROR timeout one\n");
        m.append(b"ERROR not this\n");
        m.append(b"WARN timeout other\n");
        m.finish();
        let fmt = LogFormat::compile("simple", r"^(?P<level>\w+) (?P<msg>.+)$").unwrap();
        let f = CompiledFilter::compile(
            &fmt,
            vec![FilterSpec::parse("level=ERROR").unwrap()],
        ).unwrap();
        let g = GrepPredicate::compile(&["timeout".to_string()]).unwrap();
        let out = run_to_vec(Box::new(m), LineIndex::new(), Some(f), Some(g), None);
        assert_eq!(out, b"ERROR timeout one\n");
    }
}
