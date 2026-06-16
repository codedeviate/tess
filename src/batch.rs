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
use crate::or::OrGroups;
use crate::source::Source;

/// Where the batch run writes its output.
pub enum BatchDestination {
    Stdout,
    File(PathBuf),
    /// Buffer the whole result in memory and flush it to the system clipboard
    /// once at the end. One-shot: `--follow` is rejected upstream.
    Clipboard,
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

#[allow(clippy::too_many_arguments)]
pub fn run(
    src: Box<dyn Source>,
    mut idx: LineIndex,
    filter: Option<CompiledFilter>,
    grep: Option<GrepPredicate>,
    or_groups: OrGroups,
    display: Option<DisplayRenderer>,
    spec: BatchSpec,
    sigterm: Arc<AtomicBool>,
) -> Result<()> {
    // Clipboard is a one-shot sink (follow is rejected upstream): buffer the
    // whole filtered result in memory via the same first-pass line-walk that
    // `emit_pending` performs for every destination, then flush it to the OS
    // clipboard in a single call. Handling it as a separate early path keeps
    // `out` a `Box<dyn Write>` for the file/stdout (and follow) paths without
    // the borrow trap of boxing `&mut buf`.
    if matches!(spec.destination, BatchDestination::Clipboard) {
        idx.extend_to_end(src.as_ref());
        let mut buf: Vec<u8> = Vec::new();
        emit_pending(src.as_ref(), &mut idx, filter.as_ref(), grep.as_ref(), &or_groups, display.as_ref(), &mut buf, 0)?;
        crate::clipboard::write(&buf).map_err(Error::Runtime)?;
        return Ok(());
    }

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
        BatchDestination::Clipboard => unreachable!("clipboard handled above"),
    };

    // First pass: extend index to the current end of the source and write all
    // matching lines. Static file sources resolve their full length here;
    // streaming stdin sources whose initial bytes are present do too.
    idx.extend_to_end(src.as_ref());
    let mut next_line = 0usize;
    next_line = emit_pending(src.as_ref(), &mut idx, filter.as_ref(), grep.as_ref(), &or_groups, display.as_ref(), &mut *out, next_line)?;
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
            next_line = emit_pending(src.as_ref(), &mut idx, filter.as_ref(), grep.as_ref(), &or_groups, display.as_ref(), &mut *out, next_line)?;
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

/// Emit pending output that passes the filter (or all of it if no filter
/// is bound). In line mode the cursor is a logical-line index; in records
/// mode the predicates evaluate per record (filter on the header line,
/// grep on the full multi-line record bytes) and all physical lines of a
/// matching record are emitted. When a `DisplayRenderer` is supplied,
/// parsed lines are written through the template; lines that don't parse
/// fall back to the raw bytes so no data is silently lost.
///
/// `next_line` is always advanced to one-past the last emitted physical
/// line so the follow-mode caller can pick up cleanly when new bytes
/// arrive.
#[allow(clippy::too_many_arguments)]
fn emit_pending(
    src: &dyn Source,
    idx: &mut LineIndex,
    filter: Option<&CompiledFilter>,
    grep: Option<&GrepPredicate>,
    or_groups: &OrGroups,
    display: Option<&DisplayRenderer>,
    out: &mut dyn Write,
    mut next_line: usize,
) -> Result<usize> {
    let total = idx.line_count();
    if idx.records_mode() {
        // Walk records that overlap `[next_line, total)`. Skip records whose
        // entire line range lies before `next_line` (already emitted). For
        // each remaining record, evaluate the predicates once and, if the
        // record passes, emit *all* of its physical lines.
        let total_records = idx.record_count();
        let start_record = idx.line_to_record(next_line);
        for r in start_record..total_records {
            let range = idx.record_line_range(r);
            if range.end <= next_line {
                continue;
            }
            let passes = record_passes_batch(idx, src, r, filter, grep, or_groups);
            if passes {
                for line_n in range.clone() {
                    if line_n < next_line {
                        continue;
                    }
                    emit_line(src, idx, line_n, display, out)?;
                }
            }
            next_line = range.end;
        }
        Ok(next_line)
    } else {
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
            let or_ok = or_groups.matches_line(&bytes);
            if filter_ok && grep_ok && or_ok {
                emit_line(src, idx, next_line, display, out)?;
            }
            next_line += 1;
        }
        Ok(next_line)
    }
}

/// Records-mode predicate for batch: both filter and grep evaluate against
/// the full multi-line record bytes. Filter uses the format regex with
/// dotall + multi-line semantics so greedy captures span the whole record
/// body. Mirrors `Viewport::record_passes` so the interactive and batch
/// paths agree.
fn record_passes_batch(
    idx: &LineIndex,
    src: &dyn Source,
    r: usize,
    filter: Option<&CompiledFilter>,
    grep: Option<&GrepPredicate>,
    or_groups: &OrGroups,
) -> bool {
    if filter.is_none() && grep.is_none() && !or_groups.is_active() {
        return true;
    }
    let bytes = idx.record_bytes_stripped(r, src);
    let filter_ok = match filter {
        Some(f) => matches!(f.evaluate_record(&bytes), FilterMatch::Matched),
        None => true,
    };
    let grep_ok = match grep {
        Some(g) => g.matches(&bytes),
        None => true,
    };
    let or_ok = if or_groups.is_active() {
        or_groups.matches_record(&bytes)
    } else {
        true
    };
    filter_ok && grep_ok && or_ok
}

fn emit_line(
    src: &dyn Source,
    idx: &LineIndex,
    line_n: usize,
    display: Option<&DisplayRenderer>,
    out: &mut dyn Write,
) -> Result<()> {
    let range = idx.line_range(line_n, src);
    let bytes = src.bytes(range);
    match display.and_then(|r| r.render_line(&bytes)) {
        Some(rendered) => {
            out.write_all(rendered.as_bytes()).map_err(|e| Error::Runtime(format!("write: {e}")))?;
        }
        None => {
            out.write_all(&bytes).map_err(|e| Error::Runtime(format!("write: {e}")))?;
        }
    }
    out.write_all(b"\n").map_err(|e| Error::Runtime(format!("write: {e}")))?;
    Ok(())
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
            OrGroups::default(),
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
            crate::viewport::CaseMode::Sensitive,
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
        let g = GrepPredicate::compile(&["error".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        let out = run_to_vec(Box::new(m), LineIndex::new(), None, Some(g), None);
        assert_eq!(out, b"keep error one\nkeep error two\n");
    }

    #[test]
    fn filter_in_records_mode_emits_all_lines_of_matching_record() {
        // The format regex ends with `$`; applied to a multi-line record blob
        // it would never match. Batch must evaluate the filter against the
        // first line of each record, then emit *all* of the record's lines
        // when it matches.
        let m = MockSource::new();
        m.append(
            b"[1] kind=category\n  body a\n  body a2\n\
              [2] kind=rule\n  body b\n\
              [3] kind=category\n  body c\n",
        );
        m.finish();
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());

        let fmt = LogFormat::compile(
            "rec",
            r"^\[(?P<id>\d+)\] kind=(?P<kind>.+)$",
        )
        .unwrap();
        let f = CompiledFilter::compile(
            &fmt,
            vec![FilterSpec::parse("kind~category").unwrap()],
            crate::viewport::CaseMode::Sensitive,
        )
        .unwrap();

        let out = run_to_vec(Box::new(m), idx, Some(f), None, None);
        assert_eq!(
            out,
            b"[1] kind=category\n  body a\n  body a2\n\
              [3] kind=category\n  body c\n",
        );
    }

    #[test]
    fn filter_in_records_mode_matches_pattern_in_body() {
        // The user's real case: format captures `message` as the tail after
        // the timestamp; the record body holds the searched-for token on a
        // continuation line, not the header. Records-mode evaluation runs
        // the format regex with dotall+multiline so `(?P<message>.*)$`
        // captures the whole body across newlines.
        let m = MockSource::new();
        m.append(
            b"[23-Jul-2025 10:41:20 Europe/Stockholm] SourceFactory::getSource - sourceId: category, {\n    \"config\": \"[]\",\n    \"count\": \"0\"\n[23-Jul-2025 10:41:20 Europe/Stockholm] SourceFactory::getSource - sourceId: rule, {\n    \"rule_id\": \"1\",\n    \"count\": \"0\"\n",
        );
        m.finish();
        let mut idx = LineIndex::new();
        idx.set_record_start(
            regex::bytes::Regex::new(r"^\[\d{2}-[A-Za-z]{3}-\d{4} \d{2}:\d{2}:\d{2} [^\]]+\]").unwrap(),
        );

        let fmt = LogFormat::compile(
            "swerror",
            r"^\[(?P<timestamp>(?P<day>\d{1,2})-(?P<month>[A-Za-z]+)-(?P<year>\d{4})\s(?P<hour>\d{2}):(?P<minute>\d{2}):(?P<second>\d{2})\s(?P<timezone>[^\]]+))\]\s(?P<message>.*)$",
        )
        .unwrap();
        let f = CompiledFilter::compile(
            &fmt,
            vec![FilterSpec::parse("message~config").unwrap()],
            crate::viewport::CaseMode::Sensitive,
        )
        .unwrap();

        let out = run_to_vec(Box::new(m), idx, Some(f), None, None);
        let s = std::str::from_utf8(&out).unwrap();
        // Only the first record contains "config" — but it's in the body,
        // not the header. The whole record (including the rule_id record that
        // doesn't contain "config") should emit exactly the first record.
        assert!(s.contains("sourceId: category"), "expected category record, got: {s}");
        assert!(s.contains("\"config\":"), "expected body line with \"config\", got: {s}");
        assert!(!s.contains("sourceId: rule"), "rule record should be filtered out, got: {s}");
    }

    #[test]
    fn grep_in_records_mode_emits_all_lines_of_matching_record() {
        use crate::grep::GrepPredicate;
        let m = MockSource::new();
        m.append(
            b"[1] head\n  Renderer.php\n  more body\n\
              [2] other\n  unrelated\n",
        );
        m.finish();
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());

        // Pattern matches a continuation line, not the header. Records-aware
        // grep should pull in the whole record.
        let g = GrepPredicate::compile(&["Renderer".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        let out = run_to_vec(Box::new(m), idx, None, Some(g), None);
        assert_eq!(out, b"[1] head\n  Renderer.php\n  more body\n");
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
            crate::viewport::CaseMode::Sensitive,
        ).unwrap();
        let g = GrepPredicate::compile(&["timeout".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        let out = run_to_vec(Box::new(m), LineIndex::new(), Some(f), Some(g), None);
        assert_eq!(out, b"ERROR timeout one\n");
    }

    #[test]
    fn or_groups_filter_in_batch_line_mode() {
        // Source: "login failed\nlogin ok\naccess denied\n"
        // default OR-group: "failed" OR "denied" → emit lines 1 and 3.
        let mut raw = crate::or::OrSpecRaw::new();
        raw.add_grep(crate::or::DEFAULT_GROUP, "failed".into());
        raw.add_grep(crate::or::DEFAULT_GROUP, "denied".into());
        let og = crate::or::OrGroups::compile(&raw, None, crate::viewport::CaseMode::Sensitive).unwrap();

        let m = MockSource::new();
        m.append(b"login failed\n");
        m.append(b"login ok\n");
        m.append(b"access denied\n");
        m.finish();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        run(
            Box::new(m),
            LineIndex::new(),
            None,
            None,
            og,
            None,
            BatchSpec {
                destination: BatchDestination::File(path.clone()),
                follow: false,
                poll_interval: Duration::from_millis(50),
            },
            Arc::new(AtomicBool::new(false)),
        ).unwrap();
        let mut buf = Vec::new();
        std::fs::File::open(&path).unwrap().read_to_end(&mut buf).unwrap();
        assert_eq!(buf, b"login failed\naccess denied\n");
    }
}
