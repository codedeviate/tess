use crate::source::Source;
use regex::bytes::Regex;
use std::ops::Range;

pub struct LineIndex {
    starts: Vec<usize>,
    record_starts: Vec<usize>,
    record_start_regex: Option<Regex>,
    scanned_through: usize,
    start_byte: usize,
    pending_line_start: bool,
    head_cap: Option<usize>,
    /// True once we've committed the first record (either a real match
    /// or the synthetic record-0 absorbing orphan-head lines). Always
    /// true when no record_start_regex is set.
    record_zero_committed: bool,
}

impl Default for LineIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl LineIndex {
    pub fn new() -> Self {
        Self::new_starting_at(0)
    }

    /// Construct a line index that begins at the given byte offset. Bytes
    /// before `start_byte` are never scanned and never appear in `line_range`.
    /// Used by `--tail` to skip past the head of the source.
    pub fn new_starting_at(start_byte: usize) -> Self {
        Self {
            starts: vec![start_byte],
            record_starts: vec![start_byte],
            record_start_regex: None,
            scanned_through: start_byte,
            start_byte,
            pending_line_start: false,
            head_cap: None,
            record_zero_committed: true,
        }
    }

    /// Limit the index to the first N logical lines from the start point.
    /// `line_count` clamps to this and `extend_to_byte` stops scanning
    /// past it. Used by `--head N`.
    pub fn set_head_cap(&mut self, cap: usize) {
        self.head_cap = Some(cap);
    }

    /// Enable records mode using the supplied regex. Must be called before
    /// any scanning has begun. Re-calling panics in debug builds.
    pub fn set_record_start(&mut self, re: Regex) {
        debug_assert!(
            self.scanned_through == self.start_byte && self.starts.len() == 1,
            "set_record_start must be called before scanning"
        );
        self.record_start_regex = Some(re);
        self.record_zero_committed = false;
    }

    /// Clear records mode and reset the index so the source will be rescanned
    /// without a record-start regex. Preserves `start_byte` and `head_cap`.
    /// Safe to call at any point (unlike `set_record_start`, which must be
    /// called before scanning).
    pub fn clear_record_start(&mut self) {
        self.reset_record_start_opt(None);
    }

    /// Reset the index (like `clear_record_start`) and optionally install a new
    /// record-start regex. The index will rescan from `start_byte` on the next
    /// access. Safe to call at any point; `head_cap` is preserved.
    pub fn reset_record_start_opt(&mut self, re: Option<Regex>) {
        self.record_start_regex = re;
        self.starts = vec![self.start_byte];
        self.record_starts = vec![self.start_byte];
        self.scanned_through = self.start_byte;
        self.pending_line_start = false;
        self.record_zero_committed = self.record_start_regex.is_none();
    }

    /// True iff records mode is active (a regex was set).
    pub fn records_mode(&self) -> bool {
        self.record_start_regex.is_some()
    }

    pub fn line_count(&self) -> usize {
        let raw = if self.scanned_through == self.start_byte && self.starts.len() == 1 {
            0
        } else {
            self.starts.len()
        };
        match self.head_cap {
            Some(cap) => raw.min(cap),
            None => raw,
        }
    }

    /// True once we've scanned one entry past the cap. We always keep a
    /// "sentinel" entry beyond the cap so that `line_range(cap-1)` knows
    /// where the last visible line ends.
    fn at_scan_cap(&self) -> bool {
        matches!(self.head_cap, Some(cap) if self.starts.len() > cap)
    }

    /// Scan `src` from `scanned_through` to at least byte position `target_byte`.
    fn extend_to_byte(&mut self, src: &dyn Source, target_byte: usize) {
        if self.at_scan_cap() {
            return;
        }
        if matches!(self.head_cap, Some(0)) {
            return;
        }
        let total = src.len();
        let stop = target_byte.min(total);
        if self.scanned_through >= stop {
            return;
        }

        if self.pending_line_start {
            let line_start = self.scanned_through;
            self.starts.push(line_start);
            self.maybe_push_record_start(line_start, src);
            self.pending_line_start = false;
            if self.at_scan_cap() {
                return;
            }
        }

        let chunk = src.bytes(self.scanned_through..total);
        let mut pos = self.scanned_through;
        for &b in chunk.iter() {
            pos += 1;
            if b == b'\n' {
                if pos < total {
                    let new_line_start = pos;
                    self.starts.push(new_line_start);
                    self.maybe_push_record_start(new_line_start, src);
                    if self.at_scan_cap() {
                        self.scanned_through = pos;
                        return;
                    }
                } else {
                    self.pending_line_start = true;
                }
            }
            if pos >= stop && b == b'\n' {
                self.scanned_through = pos;
                return;
            }
        }
        self.scanned_through = total;
    }

    /// Decide whether a newly-discovered line at `line_start` should also
    /// start a new record. In line-per-record mode, every line is a record.
    /// In records mode, we test the line's bytes against the regex.
    fn maybe_push_record_start(&mut self, line_start: usize, src: &dyn Source) {
        match &self.record_start_regex {
            None => {
                // Line-per-record: every line-start push is also a record-start.
                self.record_starts.push(line_start);
            }
            Some(re) => {
                let line_end = self.find_line_end(line_start, src);
                let line_bytes = src.bytes(line_start..line_end);
                let is_match = re.is_match(&line_bytes);
                if is_match {
                    if !self.record_zero_committed {
                        // First time we've seen a real record-start match.
                        if line_start == self.start_byte {
                            // The bootstrap entry at start_byte was a placeholder;
                            // this line is record 0. No second push needed —
                            // record_starts[0] already equals start_byte.
                        } else {
                            // Lines before this one form synthetic record 0; the
                            // bootstrap entry stays as record 0's start. Push the
                            // new line's offset as record 1's start.
                            self.record_starts.push(line_start);
                        }
                        self.record_zero_committed = true;
                    } else {
                        self.record_starts.push(line_start);
                    }
                } else if !self.record_zero_committed && line_start == self.start_byte {
                    // Very first line is a non-match: synthetic record 0 will
                    // absorb it and subsequent continuations. The bootstrap
                    // entry stays; mark the synthetic record committed.
                    self.record_zero_committed = true;
                }
                // Otherwise: continuation line. No push.
            }
        }
    }

    /// Find the byte position one past the end of the line starting at
    /// `line_start`. Reads forward until the next `\n` or EOF. Used by
    /// `maybe_push_record_start` to extract the line bytes for regex testing.
    fn find_line_end(&self, line_start: usize, src: &dyn Source) -> usize {
        let total = src.len();
        let chunk = src.bytes(line_start..total);
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'\n' {
                return line_start + i;
            }
        }
        total
    }

    pub fn extend_to_line(&mut self, n: usize, src: &dyn Source) {
        while self.starts.len() <= n && self.scanned_through < src.len() {
            if self.at_scan_cap() {
                // head_cap is set and we've already scanned the sentinel past
                // the cap; no further progress is possible.
                return;
            }
            self.extend_to_byte(src, src.len());
        }
    }

    pub fn extend_to_end(&mut self, src: &dyn Source) {
        self.extend_to_byte(src, src.len());
    }

    pub fn notice_new_bytes(&mut self, src: &dyn Source) {
        self.extend_to_byte(src, src.len());
    }

    /// Byte position up to which the index has scanned.
    pub fn scanned_through(&self) -> usize {
        self.scanned_through
    }

    /// Extend the index until `byte` is covered (or EOF). Public wrapper
    /// around the private `extend_to_byte`, for callers that need to query
    /// a specific byte position.
    pub fn extend_to_byte_for_query(&mut self, src: &dyn Source, byte: usize) {
        self.extend_to_byte(src, byte);
    }

    /// Find the physical-line index containing the byte at position `byte`.
    /// Returns `None` if `byte` is before `start_byte` or at/after
    /// `scanned_through`.
    pub fn line_at_byte(&self, byte: usize) -> Option<usize> {
        if byte < self.start_byte || byte >= self.scanned_through {
            return None;
        }
        match self.starts.binary_search(&byte) {
            Ok(idx) => Some(idx),
            Err(0) => None,
            Err(idx) => Some(idx - 1),
        }
    }

    /// Byte range of line `n` (excluding the trailing newline).
    /// Caller must ensure n < line_count() and the index has scanned through the line.
    pub fn line_range(&self, n: usize, src: &dyn Source) -> Range<usize> {
        let start = self.starts[n];
        // When head_cap is set, line `cap-1` is the last visible line. Its end
        // is the byte just before line `cap`'s start (a real line break exists
        // there, otherwise the cap wouldn't have been reached). The "last line
        // unbounded" branch below should not be entered for a capped line.
        let next_known = self.starts.get(n + 1).copied();
        let end = if let Some(next_start) = next_known {
            // Drop the trailing newline preceding the next line start.
            next_start - 1
        } else {
            // Last line: from start to current scanned end (minus trailing \n if present).
            let total_scanned = src.len().min(self.scanned_through.max(start));
            if total_scanned > start && src.bytes(total_scanned - 1..total_scanned)[0] == b'\n' {
                total_scanned - 1
            } else {
                total_scanned
            }
        };
        start..end
    }

    /// Number of records exposed by this index. Equals `line_count()` when
    /// records mode is inactive. May be less than `line_count()` when
    /// records mode is active and records span multiple physical lines.
    pub fn record_count(&self) -> usize {
        let raw = if self.scanned_through == self.start_byte && self.record_starts.len() == 1
            && self.record_start_regex.is_none()
        {
            // Empty source, no records mode: zero records.
            0
        } else if self.scanned_through == self.start_byte && self.record_starts.len() == 1
            && self.record_start_regex.is_some() && !self.record_zero_committed
        {
            // Empty source, records mode set but nothing scanned yet: zero records.
            0
        } else {
            self.record_starts.len()
        };
        match self.head_cap {
            Some(0) => 0,
            Some(cap) => {
                let visible_lines = raw.min(self.starts.len()).min(cap);
                self.line_to_record_inner(visible_lines.saturating_sub(1))
                    .map(|r| r + 1)
                    .unwrap_or(0)
            }
            None => raw,
        }
    }

    /// Byte range of record `n` including embedded `\n`s. Excludes the
    /// trailing `\n` after the record's last physical line (if any).
    pub fn record_range(&self, n: usize, src: &dyn Source) -> Range<usize> {
        let start = self.record_starts[n];
        let end = if n + 1 < self.record_starts.len() {
            // End is just before the next record-start; that byte is a `\n`.
            self.record_starts[n + 1] - 1
        } else {
            // Last record: extend to scanned_through, trimming trailing `\n`.
            let total_scanned = src.len().min(self.scanned_through.max(start));
            if total_scanned > start && src.bytes(total_scanned - 1..total_scanned)[0] == b'\n' {
                total_scanned - 1
            } else {
                total_scanned
            }
        };
        start..end
    }

    /// Range of physical line indices `[first..last)` covered by record `n`.
    pub fn record_line_range(&self, n: usize) -> Range<usize> {
        let first_line = self.starts.binary_search(&self.record_starts[n])
            .expect("record start is always a line start");
        let last_line = if n + 1 < self.record_starts.len() {
            self.starts.binary_search(&self.record_starts[n + 1])
                .expect("record start is always a line start")
        } else {
            self.starts.len()
        };
        first_line..last_line
    }

    /// Record index that contains physical line `line_n`. O(log records).
    pub fn line_to_record(&self, line_n: usize) -> usize {
        self.line_to_record_inner(line_n).unwrap_or(0)
    }

    fn line_to_record_inner(&self, line_n: usize) -> Option<usize> {
        if self.starts.len() <= line_n {
            return None;
        }
        let line_start = self.starts[line_n];
        match self.record_starts.binary_search(&line_start) {
            Ok(idx) => Some(idx),
            Err(0) => Some(0),
            Err(idx) => Some(idx - 1),
        }
    }

    /// Contiguous byte slice covering record `n`. Embedded `\n`s present.
    pub fn record_bytes<'a>(&self, n: usize, src: &'a dyn Source) -> std::borrow::Cow<'a, [u8]> {
        let r = self.record_range(n, src);
        src.bytes(r)
    }

    /// Return the bytes of line `n` with SGR/CSI/OSC sequences stripped.
    /// Borrows the source's bytes when no escape sequences are present
    /// (common case); owns a new buffer otherwise.
    pub fn line_bytes_stripped<'a>(
        &self,
        n: usize,
        src: &'a dyn Source,
    ) -> std::borrow::Cow<'a, [u8]> {
        let range = self.line_range(n, src);
        let raw = src.bytes(range);
        crate::ansi::strip_sgr(&raw).into_owned().into()
    }

    /// Like `line_bytes_stripped` but for records (multi-line mode).
    pub fn record_bytes_stripped<'a>(
        &self,
        n: usize,
        src: &'a dyn Source,
    ) -> std::borrow::Cow<'a, [u8]> {
        let range = self.record_range(n, src);
        let raw = src.bytes(range);
        crate::ansi::strip_sgr(&raw).into_owned().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MockSource;
    use regex::bytes::Regex;

    #[test]
    fn empty_source_zero_lines() {
        let m = MockSource::new();
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 0);
    }

    #[test]
    fn single_line_no_newline() {
        let m = MockSource::new();
        m.append(b"hello");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.line_range(0, &m), 0..5);
    }

    #[test]
    fn single_line_trailing_newline() {
        let m = MockSource::new();
        m.append(b"hello\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 1);
        assert_eq!(idx.line_range(0, &m), 0..5);
    }

    #[test]
    fn multiple_lines() {
        let m = MockSource::new();
        m.append(b"a\nbb\nccc\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.line_range(0, &m), 0..1);
        assert_eq!(idx.line_range(1, &m), 2..4);
        assert_eq!(idx.line_range(2, &m), 5..8);
    }

    #[test]
    fn head_cap_truncates_line_count() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n5\n6\n7\n8\n");  // 8 lines
        let mut idx = LineIndex::new();
        idx.set_head_cap(3);
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3, "should be capped to 3 lines");
        // Lines 0..2 inclusive must have correct ranges.
        assert_eq!(idx.line_range(0, &m), 0..1);
        assert_eq!(idx.line_range(1, &m), 2..3);
        assert_eq!(idx.line_range(2, &m), 4..5);
    }

    #[test]
    fn head_cap_extend_to_line_terminates() {
        // Regression: extend_to_line(n) used to spin forever when head_cap
        // had already been hit, because extend_to_byte returned without
        // advancing scanned_through.
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let mut idx = LineIndex::new();
        idx.set_head_cap(3);
        idx.extend_to_line(20, &m);  // far past the cap
        assert_eq!(idx.line_count(), 3);
    }

    #[test]
    fn head_cap_zero_yields_empty() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n");
        let mut idx = LineIndex::new();
        idx.set_head_cap(0);
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 0);
    }

    #[test]
    fn start_byte_skips_head_of_source() {
        let m = MockSource::new();
        // 5 lines: alpha\nbeta\ngamma\ndelta\nepsilon\n
        m.append(b"alpha\nbeta\ngamma\ndelta\nepsilon\n");
        // gamma starts at byte 11.
        let mut idx = LineIndex::new_starting_at(11);
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3, "from byte 11 there are 3 lines: gamma, delta, epsilon");
        assert_eq!(idx.line_range(0, &m), 11..16); // gamma
        assert_eq!(idx.line_range(1, &m), 17..22); // delta
        assert_eq!(idx.line_range(2, &m), 23..30); // epsilon
    }

    #[test]
    fn start_byte_with_empty_remainder() {
        let m = MockSource::new();
        m.append(b"alpha\n");
        let mut idx = LineIndex::new_starting_at(6);
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 0);
    }

    #[test]
    fn incremental_growth_via_notice_new_bytes() {
        let m = MockSource::new();
        let mut idx = LineIndex::new();

        m.append(b"alpha\n");
        idx.notice_new_bytes(&m);
        assert_eq!(idx.line_count(), 1);

        m.append(b"beta\ngamm");
        idx.notice_new_bytes(&m);
        assert_eq!(idx.line_count(), 3); // alpha, beta, gamm (partial, but counted)

        m.append(b"a\n");
        idx.notice_new_bytes(&m);
        assert_eq!(idx.line_count(), 3);
        // "alpha\n" = bytes 0-5, "beta\n" = bytes 6-10, "gamma" = bytes 11-15
        assert_eq!(idx.line_range(2, &m), 11..16); // "gamma"
    }

    fn re(pat: &str) -> Regex {
        Regex::new(pat).unwrap()
    }

    #[test]
    fn records_mirror_lines_when_no_regex() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.record_count(), 3);
        for i in 0..3 {
            assert_eq!(idx.record_range(i, &m), idx.line_range(i, &m));
        }
    }

    #[test]
    fn record_count_zero_for_empty_source_records_mode() {
        let m = MockSource::new();
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.record_count(), 0);
    }

    #[test]
    fn records_group_continuations() {
        let m = MockSource::new();
        m.append(b"[1] head\n  more\n  more\n[2] head\n  more\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 5);
        assert_eq!(idx.record_count(), 2);
        let r0 = idx.record_range(0, &m);
        assert_eq!(&m.bytes(r0)[..], b"[1] head\n  more\n  more");
        let r1 = idx.record_range(1, &m);
        assert_eq!(&m.bytes(r1)[..3], b"[2]");
    }

    #[test]
    fn synthetic_record_zero_absorbs_orphan_head() {
        let m = MockSource::new();
        m.append(b"banner line 1\nbanner line 2\n[1] first real record\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.record_count(), 2);
        let r0 = idx.record_range(0, &m);
        assert_eq!(&m.bytes(r0)[..], b"banner line 1\nbanner line 2");
        assert_eq!(idx.record_line_range(0), 0..2);
        assert_eq!(idx.record_line_range(1), 2..3);
    }

    #[test]
    fn line_to_record_round_trips() {
        let m = MockSource::new();
        m.append(b"[1] a\n  cont\n[2] b\n  cont\n  cont\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.line_to_record(0), 0);  // "[1] a"
        assert_eq!(idx.line_to_record(1), 0);  // "  cont"
        assert_eq!(idx.line_to_record(2), 1);  // "[2] b"
        assert_eq!(idx.line_to_record(3), 1);
        assert_eq!(idx.line_to_record(4), 1);
    }

    #[test]
    fn record_bytes_contains_embedded_newlines() {
        let m = MockSource::new();
        m.append(b"[1] head\n  more\n[2] next\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        let bytes = idx.record_bytes(0, &m);
        assert_eq!(&*bytes, b"[1] head\n  more");
    }

    #[test]
    fn no_match_at_all_is_one_synthetic_record() {
        let m = MockSource::new();
        m.append(b"plain text\nmore plain\nno brackets here\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(idx.record_count(), 1);
        assert_eq!(idx.record_line_range(0), 0..3);
    }

    #[test]
    fn pending_record_start_handles_growing_input() {
        let m = MockSource::new();
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        m.append(b"[1] head\n  more\n");
        idx.notice_new_bytes(&m);
        assert_eq!(idx.record_count(), 1);
        m.append(b"[2] head\n");
        idx.notice_new_bytes(&m);
        assert_eq!(idx.record_count(), 2);
    }

    #[test]
    fn empty_continuation_lines_are_continuations() {
        let m = MockSource::new();
        m.append(b"[1] head\n\n  after blank\n[2] next\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 4);
        assert_eq!(idx.record_count(), 2);
        assert_eq!(idx.record_line_range(0), 0..3);
    }

    #[test]
    fn line_bytes_stripped_returns_visible_text() {
        let m = MockSource::new();
        m.append(b"\x1b[31merror\x1b[0m\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let stripped = idx.line_bytes_stripped(0, &m);
        assert_eq!(stripped.as_ref(), b"error");
    }

    #[test]
    fn line_bytes_stripped_plain_input() {
        let m = MockSource::new();
        m.append(b"plain\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let stripped = idx.line_bytes_stripped(0, &m);
        assert_eq!(stripped.as_ref(), b"plain");
    }

    #[test]
    fn records_mode_reports_true_only_when_regex_set() {
        let mut idx = LineIndex::new();
        assert!(!idx.records_mode());
        idx.set_record_start(re(r"^\["));
        assert!(idx.records_mode());
    }

    #[test]
    fn record_range_handles_unterminated_last_record() {
        let m = MockSource::new();
        m.append(b"[1] head\n[2] last line no newline");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.extend_to_end(&m);
        assert_eq!(idx.record_count(), 2);
        let r1 = idx.record_range(1, &m);
        assert_eq!(&m.bytes(r1)[..], b"[2] last line no newline");
    }

    #[test]
    fn record_count_with_head_cap_zero_returns_zero_in_records_mode() {
        let m = MockSource::new();
        m.append(b"[1] head\n[2] next\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(re(r"^\["));
        idx.set_head_cap(0);
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 0);
        assert_eq!(idx.record_count(), 0);
    }
}
