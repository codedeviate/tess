use crate::source::Source;
use std::ops::Range;

pub struct LineIndex {
    starts: Vec<usize>,       // byte offset of line N
    scanned_through: usize,
    /// Byte offset where indexing begins. Non-zero when `--tail` has skipped
    /// over the head of the source.
    start_byte: usize,
    /// True when the last byte scanned was a '\n'. Used to defer adding the
    /// next line-start until we know whether more bytes will arrive (i.e. the
    /// '\n' is not actually trailing).
    pending_line_start: bool,
    /// Optional cap on the number of lines exposed via `line_count` and
    /// scanned via `extend_to_byte`. Used by `--head N`.
    head_cap: Option<usize>,
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
            scanned_through: start_byte,
            start_byte,
            pending_line_start: false,
            head_cap: None,
        }
    }

    /// Limit the index to the first N logical lines from the start point.
    /// `line_count` clamps to this and `extend_to_byte` stops scanning
    /// past it. Used by `--head N`.
    pub fn set_head_cap(&mut self, cap: usize) {
        self.head_cap = Some(cap);
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
        // head_cap == 0 means no lines visible, so don't scan at all.
        if matches!(self.head_cap, Some(0)) {
            return;
        }
        let total = src.len();
        let stop = target_byte.min(total);
        if self.scanned_through >= stop {
            return;
        }

        // If the previous scan ended on a '\n' and new bytes have arrived,
        // that '\n' is no longer trailing — commit the deferred line start now.
        if self.pending_line_start {
            self.starts.push(self.scanned_through);
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
                    // Not at EOF: this newline definitely starts a new line.
                    self.starts.push(pos);
                    if self.at_scan_cap() {
                        self.scanned_through = pos;
                        return;
                    }
                } else {
                    // At EOF: may be trailing. Defer until we know more bytes arrive.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MockSource;

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
}
