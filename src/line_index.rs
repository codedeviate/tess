use crate::source::Source;
use std::ops::Range;

pub struct LineIndex {
    starts: Vec<usize>,       // byte offset of line N
    scanned_through: usize,
    /// True when the last byte scanned was a '\n'. Used to defer adding the
    /// next line-start until we know whether more bytes will arrive (i.e. the
    /// '\n' is not actually trailing).
    pending_line_start: bool,
}

impl LineIndex {
    pub fn new() -> Self {
        Self { starts: vec![0], scanned_through: 0, pending_line_start: false }
    }

    pub fn line_count(&self) -> usize {
        // If file is empty, we report 0; if non-empty, starts.len() lines have been seen.
        if self.scanned_through == 0 && self.starts.len() == 1 {
            0
        } else {
            self.starts.len()
        }
    }

    /// Scan `src` from `scanned_through` to at least byte position `target_byte`.
    fn extend_to_byte(&mut self, src: &dyn Source, target_byte: usize) {
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
        }

        let chunk = src.bytes(self.scanned_through..total);
        let mut pos = self.scanned_through;
        for &b in chunk.iter() {
            pos += 1;
            if b == b'\n' {
                if pos < total {
                    // Not at EOF: this newline definitely starts a new line.
                    self.starts.push(pos);
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
        let end = if n + 1 < self.starts.len() {
            // Drop the trailing newline preceding the next line start.
            self.starts[n + 1] - 1
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
