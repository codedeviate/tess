use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering}};
use std::time::SystemTime;

use crate::prettify::{self, PrettifyMode};

pub trait Source: Send + Sync {
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]>;
    fn is_complete(&self) -> bool;
    /// Read any new bytes that have become available since the last call.
    /// Default no-op for static sources. Streaming sources override.
    fn pump(&self) {}
    /// Monotonic counter that bumps whenever the source's *content* has been
    /// replaced wholesale (as opposed to merely appended to). Append-style
    /// sources keep this at 0; `LiveFileSource` increments it on each detected
    /// rewrite so the event loop knows to rebuild the line index instead of
    /// just folding in new bytes.
    fn revision(&self) -> u64 { 0 }

    /// Current prettify mode if this source has a `TransformingSource` wrapper.
    /// `None` means the source is not capable of prettification (e.g. a plain
    /// `FileSource`). `Some(Off)` means wrapped but currently raw.
    fn prettify_mode(&self) -> Option<PrettifyMode> { None }

    /// Status-line label for the active prettify state, e.g. `"json"` or
    /// `"json:err"`. `None` when there's no wrapper or the mode is `Off`
    /// without an error.
    fn prettify_label(&self) -> Option<String> { None }

    /// Switch to a specific prettify mode. No-op for sources without a wrapper.
    /// Bumps `revision()` so callers know to rebuild the line index.
    fn set_prettify_mode(&self, _mode: PrettifyMode) {}

    /// Flip between the current mode and the last active (non-Off) mode.
    /// No-op if the source is not wrapped, or if it has never had an active
    /// mode (i.e. only ever been raw).
    fn toggle_prettify(&self) {}

    /// Re-run byte-based content detection and apply the result. Used by the
    /// interactive `-Pa` ("auto") sub-command. No-op without a wrapper.
    fn redetect_prettify(&self) {}
}

/// Find the byte offset such that `bytes[offset..]` is exactly the last `n`
/// logical lines of `src` (lines delimited by `\n`; a trailing `\n` at EOF is
/// not its own line). Returns 0 when the source has fewer than `n` lines, or
/// `src.len()` when `n == 0`.
///
/// Reverse-scans in 64 KiB chunks so a 10 GB file stays cheap — only the
/// trailing pages need to be touched.
pub fn find_tail_offset(src: &dyn Source, n: usize) -> usize {
    let total = src.len();
    if n == 0 || total == 0 {
        return total;
    }
    // Don't count a trailing newline as a "line break to step over": it
    // terminates the last line, it's not a separator before another line.
    let mut end = total;
    if end > 0 && src.bytes((end - 1)..end)[0] == b'\n' {
        end -= 1;
    }

    let chunk_size: usize = 64 * 1024;
    let mut count = 0usize;
    let mut pos = end;
    while pos > 0 {
        let chunk_start = pos.saturating_sub(chunk_size);
        let bytes = src.bytes(chunk_start..pos);
        for i in (0..bytes.len()).rev() {
            if bytes[i] == b'\n' {
                count += 1;
                if count == n {
                    return chunk_start + i + 1;
                }
            }
        }
        pos = chunk_start;
    }
    0
}

pub struct FileSource {
    mmap: Option<memmap2::Mmap>,
    fallback_buf: Option<Vec<u8>>,
    initial_size: usize,
    appended_len: AtomicUsize,
    streaming: Mutex<StreamingState>,
}

struct StreamingState {
    file: File,
    appended: Vec<u8>,
}

impl std::fmt::Debug for FileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSource").finish()
    }
}

impl FileSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let initial_size = metadata.len() as usize;
        let (mmap, fallback_buf) = if initial_size == 0 {
            (None, Some(Vec::new()))
        } else {
            // SAFETY: file remains open as long as Mmap exists; we never modify the file.
            match unsafe { memmap2::Mmap::map(&file) } {
                Ok(m) => (Some(m), None),
                Err(_) => {
                    let mut buf = Vec::new();
                    let mut f = File::open(path)?;
                    f.read_to_end(&mut buf)?;
                    (None, Some(buf))
                }
            }
        };
        // Separate handle for streaming reads. Seeked past the initial content
        // so subsequent reads only return bytes appended after open.
        let mut stream_file = File::open(path)?;
        stream_file.seek(SeekFrom::Start(initial_size as u64))?;
        Ok(Self {
            mmap,
            fallback_buf,
            initial_size,
            appended_len: AtomicUsize::new(0),
            streaming: Mutex::new(StreamingState {
                file: stream_file,
                appended: Vec::new(),
            }),
        })
    }

    fn static_bytes(&self) -> &[u8] {
        if let Some(m) = &self.mmap {
            &m[..]
        } else if let Some(b) = &self.fallback_buf {
            &b[..]
        } else {
            &[]
        }
    }
}

impl Source for FileSource {
    fn len(&self) -> usize {
        self.initial_size + self.appended_len.load(Ordering::Acquire)
    }

    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        let static_bytes = self.static_bytes();
        if range.end <= self.initial_size {
            return Cow::Borrowed(&static_bytes[range]);
        }
        let stream = self.streaming.lock().unwrap();
        let total = self.initial_size + stream.appended.len();
        let start = range.start.min(total);
        let end = range.end.min(total);
        if start >= self.initial_size {
            let off = start - self.initial_size;
            let off_end = end - self.initial_size;
            Cow::Owned(stream.appended[off..off_end].to_vec())
        } else {
            let mut v = Vec::with_capacity(end - start);
            v.extend_from_slice(&static_bytes[start..self.initial_size]);
            v.extend_from_slice(&stream.appended[..end - self.initial_size]);
            Cow::Owned(v)
        }
    }

    fn is_complete(&self) -> bool { true }

    fn pump(&self) {
        let mut stream = self.streaming.lock().unwrap();
        let mut tmp = [0u8; 8192];
        loop {
            match stream.file.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => stream.appended.extend_from_slice(&tmp[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let new_len = stream.appended.len();
        self.appended_len.store(new_len, Ordering::Release);
    }
}

/// A test/utility source whose contents can be appended at runtime.
pub struct MockSource {
    buf: Arc<Mutex<Vec<u8>>>,
    complete: Arc<AtomicBool>,
}

impl Default for MockSource {
    fn default() -> Self {
        Self::new()
    }
}

impl MockSource {
    pub fn new() -> Self {
        Self {
            buf: Arc::new(Mutex::new(Vec::new())),
            complete: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn append(&self, more: &[u8]) {
        self.buf.lock().unwrap().extend_from_slice(more);
    }

    pub fn finish(&self) {
        self.complete.store(true, Ordering::SeqCst);
    }
}

impl Source for MockSource {
    fn len(&self) -> usize { self.buf.lock().unwrap().len() }
    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        Cow::Owned(self.buf.lock().unwrap()[range].to_vec())
    }
    fn is_complete(&self) -> bool { self.complete.load(Ordering::SeqCst) }
}

/// A file source that watches for *whole-file* rewrites. Unlike `FileSource`
/// (which only picks up appended bytes via a streaming handle), `LiveFileSource`
/// re-reads the entire file when its `(mtime, size, ino)` signature changes,
/// swaps the buffer in place, and bumps a revision counter so callers know to
/// rebuild any per-line state. Intended for source-file-sized inputs being
/// rewritten by an editor or AI agent.
pub struct LiveFileSource {
    path: PathBuf,
    state: Mutex<LiveState>,
    revision: AtomicU64,
}

struct LiveState {
    bytes: Vec<u8>,
    signature: FileSignature,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FileSignature {
    mtime: Option<SystemTime>,
    size: u64,
    ino: u64,
}

impl FileSignature {
    fn read(path: &Path) -> std::io::Result<Self> {
        let md = std::fs::metadata(path)?;
        Ok(Self {
            mtime: md.modified().ok(),
            size: md.len(),
            #[cfg(unix)]
            ino: {
                use std::os::unix::fs::MetadataExt;
                md.ino()
            },
            #[cfg(not(unix))]
            ino: 0,
        })
    }
}

impl std::fmt::Debug for LiveFileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveFileSource").field("path", &self.path).finish()
    }
}

impl LiveFileSource {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let md = std::fs::metadata(path)?;
        if !md.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        let bytes = std::fs::read(path)?;
        // Re-stat after read so the signature reflects the bytes we actually
        // captured (concurrent writers won't tear our buffer, but the mtime/size
        // we record should match what we read).
        let signature = FileSignature::read(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            state: Mutex::new(LiveState { bytes, signature }),
            revision: AtomicU64::new(0),
        })
    }
}

impl Source for LiveFileSource {
    fn len(&self) -> usize { self.state.lock().unwrap().bytes.len() }

    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        let s = self.state.lock().unwrap();
        let end = range.end.min(s.bytes.len());
        let start = range.start.min(end);
        Cow::Owned(s.bytes[start..end].to_vec())
    }

    /// Live sources are never "complete" — the file may be rewritten at any
    /// time. The status line picks this up as the `+` suffix on totals.
    fn is_complete(&self) -> bool { false }

    fn pump(&self) {
        // Cheap stat to detect a change. If the file vanished, leave the
        // current buffer in place and try again next tick.
        let new_sig = match FileSignature::read(&self.path) {
            Ok(sig) => sig,
            Err(_) => return,
        };
        let mut s = self.state.lock().unwrap();
        if new_sig == s.signature {
            return;
        }
        let new_bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(_) => return,
        };
        // Re-read signature after the read so we reflect what was actually loaded.
        let post_sig = FileSignature::read(&self.path).unwrap_or(new_sig);
        s.bytes = new_bytes;
        s.signature = post_sig;
        drop(s);
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    fn revision(&self) -> u64 { self.revision.load(Ordering::Acquire) }
}

/// A source that wraps another source and applies a `PrettifyMode` transform
/// to its bytes. Toggling the mode bumps `revision()` so the event loop
/// rebuilds the line index. Falls back to passing through the inner bytes if
/// the transform fails to parse, surfacing the error via `last_error()`.
pub struct TransformingSource {
    inner: Box<dyn Source>,
    state: Mutex<TransformState>,
    revision: AtomicU64,
}

struct TransformState {
    mode: PrettifyMode,
    /// Most recent non-Off mode. Used by `toggle_prettify` to flip back to
    /// what the user was looking at most recently. Stays at the original
    /// detected/explicit mode at startup, even if the user toggles to Off.
    last_active: PrettifyMode,
    /// Whatever the consumer should see — either the raw inner bytes or the
    /// successfully transformed bytes. Always populated.
    cached: Vec<u8>,
    /// Set when the last attempted transform failed to parse. Cleared on a
    /// successful transform or on switching to `Off`.
    last_error: Option<String>,
}

impl std::fmt::Debug for TransformingSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransformingSource").finish()
    }
}

impl TransformingSource {
    /// Build a wrapping source. The inner is read fully via its current bytes
    /// view; the transform runs once. If the transform fails, the cache holds
    /// the raw inner bytes and `last_error()` returns the parse error.
    pub fn wrap(inner: Box<dyn Source>, mode: PrettifyMode) -> Self {
        let raw = inner.bytes(0..inner.len()).to_vec();
        let (cached, last_error) = run_transform(mode, &raw);
        let last_active = if mode.is_active() { mode } else { PrettifyMode::Off };
        Self {
            inner,
            state: Mutex::new(TransformState { mode, last_active, cached, last_error }),
            revision: AtomicU64::new(0),
        }
    }

    pub fn mode(&self) -> PrettifyMode {
        self.state.lock().unwrap().mode
    }

    /// Currently surfaced parse error, if any. Cleared by a successful
    /// `set_mode` or by switching to `Off`.
    pub fn last_error(&self) -> Option<String> {
        self.state.lock().unwrap().last_error.clone()
    }

    /// Internal apply-mode helper. Re-reads inner bytes, runs the transform,
    /// updates the cache, bumps revision. Tracks `last_active` so toggle
    /// can flip back to a non-Off mode.
    fn apply_mode(&self, mode: PrettifyMode) {
        let raw = self.inner.bytes(0..self.inner.len()).to_vec();
        let (cached, last_error) = run_transform(mode, &raw);
        let mut s = self.state.lock().unwrap();
        s.mode = mode;
        if mode.is_active() && last_error.is_none() {
            s.last_active = mode;
        }
        s.cached = cached;
        s.last_error = last_error;
        drop(s);
        self.revision.fetch_add(1, Ordering::AcqRel);
    }
}

fn run_transform(mode: PrettifyMode, raw: &[u8]) -> (Vec<u8>, Option<String>) {
    match prettify::prettify(mode, raw) {
        Ok(out) => (out, None),
        Err(e) => (raw.to_vec(), Some(e)),
    }
}

impl Source for TransformingSource {
    fn len(&self) -> usize { self.state.lock().unwrap().cached.len() }

    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        let s = self.state.lock().unwrap();
        let end = range.end.min(s.cached.len());
        let start = range.start.min(end);
        Cow::Owned(s.cached[start..end].to_vec())
    }

    fn is_complete(&self) -> bool { self.inner.is_complete() }

    fn pump(&self) { self.inner.pump(); }

    fn revision(&self) -> u64 { self.revision.load(Ordering::Acquire) }

    fn prettify_mode(&self) -> Option<PrettifyMode> {
        Some(self.state.lock().unwrap().mode)
    }

    fn prettify_label(&self) -> Option<String> {
        let s = self.state.lock().unwrap();
        if s.last_error.is_some() {
            // Whatever mode we tried last; show "<mode>:err".
            let lbl = s.mode.label();
            let lbl = if lbl.is_empty() { s.last_active.label() } else { lbl };
            Some(format!("{lbl}:err"))
        } else if s.mode.is_active() {
            Some(s.mode.label().to_string())
        } else {
            None
        }
    }

    fn set_prettify_mode(&self, mode: PrettifyMode) {
        self.apply_mode(mode);
    }

    fn toggle_prettify(&self) {
        let target = {
            let s = self.state.lock().unwrap();
            if s.mode.is_active() {
                PrettifyMode::Off
            } else if s.last_active.is_active() {
                s.last_active
            } else {
                // Never had an active mode (started raw and never flipped on).
                // Try a one-shot detection from the inner bytes; if nothing
                // matches, leave state alone.
                drop(s);
                self.redetect_prettify();
                return;
            }
        };
        self.apply_mode(target);
    }

    fn redetect_prettify(&self) {
        let raw = self.inner.bytes(0..self.inner.len()).to_vec();
        let detected = crate::prettify::detect_from_bytes(&raw);
        if let Some(mode) = detected {
            self.apply_mode(mode);
        }
        // Undetected: leave state alone (caller will see same label).
    }
}

pub struct StdinSource {
    inner: StdinInner,
}

enum StdinInner {
    Static(Vec<u8>),
    Streaming {
        buf: Arc<Mutex<Vec<u8>>>,
        len_cache: Arc<AtomicUsize>,
        complete: Arc<AtomicBool>,
    },
}

impl StdinSource {
    /// Read all of stdin into a buffer synchronously. After this returns,
    /// stdin (fd 0) is at EOF; the caller is responsible for redirecting fd 0
    /// to /dev/tty before entering raw mode if interactive input is needed.
    pub fn read_all() -> std::io::Result<Self> {
        let mut bytes = Vec::new();
        std::io::stdin().lock().read_to_end(&mut bytes)?;
        Ok(Self { inner: StdinInner::Static(bytes) })
    }

    /// Duplicate fd 0 onto a private fd, then spawn a thread that reads from
    /// it into a shared buffer. Caller can safely `dup2(/dev/tty, 0)` afterwards
    /// without disturbing the thread — it reads from the duplicated fd, not
    /// from `STDIN_FILENO`.
    #[cfg(unix)]
    pub fn spawn_streaming() -> std::io::Result<Self> {
        use std::os::unix::io::FromRawFd;
        let cloned_fd = unsafe { libc::dup(libc::STDIN_FILENO) };
        if cloned_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: cloned_fd is now owned by the File; closed on Drop.
        let mut file = unsafe { File::from_raw_fd(cloned_fd) };

        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
        let len_cache = Arc::new(AtomicUsize::new(0));
        let complete = Arc::new(AtomicBool::new(false));
        let buf_w = Arc::clone(&buf);
        let len_w = Arc::clone(&len_cache);
        let complete_w = Arc::clone(&complete);
        std::thread::spawn(move || {
            let mut tmp = [0u8; 8192];
            loop {
                match file.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        let mut b = buf_w.lock().unwrap();
                        b.extend_from_slice(&tmp[..n]);
                        len_w.store(b.len(), Ordering::Release);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            complete_w.store(true, Ordering::SeqCst);
        });
        Ok(Self { inner: StdinInner::Streaming { buf, len_cache, complete } })
    }
}

impl Source for StdinSource {
    fn len(&self) -> usize {
        match &self.inner {
            StdinInner::Static(v) => v.len(),
            StdinInner::Streaming { len_cache, .. } => len_cache.load(Ordering::Acquire),
        }
    }
    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        match &self.inner {
            StdinInner::Static(v) => Cow::Borrowed(&v[range]),
            StdinInner::Streaming { buf, .. } => Cow::Owned(buf.lock().unwrap()[range].to_vec()),
        }
    }
    fn is_complete(&self) -> bool {
        match &self.inner {
            StdinInner::Static(_) => true,
            StdinInner::Streaming { complete, .. } => complete.load(Ordering::Acquire),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn file_source_reads_temp_file() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"hello world").unwrap();
        let src = FileSource::open(tmp.path()).unwrap();
        assert_eq!(src.len(), 11);
        assert_eq!(&*src.bytes(0..5), b"hello");
        assert_eq!(&*src.bytes(6..11), b"world");
        assert!(src.is_complete());
    }

    #[test]
    fn file_source_empty_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let src = FileSource::open(tmp.path()).unwrap();
        assert_eq!(src.len(), 0);
    }

    #[test]
    fn file_source_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = FileSource::open(dir.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn file_source_pump_picks_up_appended_bytes() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"first").unwrap();
        tmp.flush().unwrap();
        let src = FileSource::open(tmp.path()).unwrap();
        assert_eq!(src.len(), 5);
        // Append more bytes to the underlying file.
        tmp.write_all(b" second").unwrap();
        tmp.flush().unwrap();
        // Before pump, len() reflects only what we knew at open.
        assert_eq!(src.len(), 5);
        src.pump();
        assert_eq!(src.len(), 12);
        // Borrowed range entirely in the original mmap.
        assert_eq!(&*src.bytes(0..5), b"first");
        // Range entirely in the appended region.
        assert_eq!(&*src.bytes(5..12), b" second");
        // Range straddling the boundary (3..10 = 7 bytes of "first second").
        assert_eq!(&*src.bytes(3..10), b"st seco");
    }

    #[test]
    fn find_tail_offset_zero_lines_returns_total() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");
        assert_eq!(find_tail_offset(&m, 0), 6);
    }

    #[test]
    fn find_tail_offset_empty_source() {
        let m = MockSource::new();
        assert_eq!(find_tail_offset(&m, 5), 0);
    }

    #[test]
    fn find_tail_offset_fewer_lines_than_n_returns_zero() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");  // 3 lines
        assert_eq!(find_tail_offset(&m, 10), 0);
    }

    #[test]
    fn find_tail_offset_last_one_with_trailing_newline() {
        let m = MockSource::new();
        m.append(b"alpha\nbeta\ngamma\n");  // 3 lines
        // gamma starts at byte 11.
        assert_eq!(find_tail_offset(&m, 1), 11);
    }

    #[test]
    fn find_tail_offset_last_two_with_trailing_newline() {
        let m = MockSource::new();
        m.append(b"alpha\nbeta\ngamma\n");
        // beta starts at byte 6.
        assert_eq!(find_tail_offset(&m, 2), 6);
    }

    #[test]
    fn find_tail_offset_last_one_no_trailing_newline() {
        let m = MockSource::new();
        m.append(b"alpha\nbeta\ngamma");  // last line not terminated
        // gamma starts at byte 11.
        assert_eq!(find_tail_offset(&m, 1), 11);
    }

    #[test]
    fn find_tail_offset_exactly_n_lines_returns_zero() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");  // 3 lines exactly
        assert_eq!(find_tail_offset(&m, 3), 0);
    }

    #[test]
    fn live_source_reads_initial_content() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"alpha\nbeta\n").unwrap();
        tmp.flush().unwrap();
        let src = LiveFileSource::open(tmp.path()).unwrap();
        assert_eq!(src.len(), 11);
        assert_eq!(&*src.bytes(0..11), b"alpha\nbeta\n");
        assert_eq!(src.revision(), 0);
        assert!(!src.is_complete()); // live sources are always "growing"
    }

    #[test]
    fn live_source_pump_picks_up_rewritten_content() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"first\n").unwrap();
        let src = LiveFileSource::open(tmp.path()).unwrap();
        assert_eq!(src.len(), 6);
        assert_eq!(src.revision(), 0);

        // Rewrite the file with different (longer) content. mtime should
        // bump; signatures differ; pump() picks it up.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(tmp.path(), b"second longer line\n").unwrap();
        src.pump();
        assert_eq!(src.len(), 19);
        assert_eq!(&*src.bytes(0..19), b"second longer line\n");
        assert_eq!(src.revision(), 1);
    }

    #[test]
    fn live_source_pump_no_change_does_not_bump_revision() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"stable\n").unwrap();
        let src = LiveFileSource::open(tmp.path()).unwrap();
        let r0 = src.revision();
        src.pump();
        src.pump();
        src.pump();
        assert_eq!(src.revision(), r0);
    }

    #[test]
    fn live_source_handles_file_shrink() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"longer initial content\n").unwrap();
        let src = LiveFileSource::open(tmp.path()).unwrap();
        assert!(src.len() > 5);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(tmp.path(), b"x\n").unwrap();
        src.pump();
        assert_eq!(src.len(), 2);
        assert_eq!(&*src.bytes(0..2), b"x\n");
        assert_eq!(src.revision(), 1);
    }

    #[test]
    fn live_source_handles_atomic_rename() {
        // Most editors write atomically: write tmp file, rename over original.
        // The inode changes; mtime/size likely change. pump() must follow.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("file.txt");
        std::fs::write(&target, b"original\n").unwrap();
        let src = LiveFileSource::open(&target).unwrap();
        assert_eq!(&*src.bytes(0..9), b"original\n");

        std::thread::sleep(std::time::Duration::from_millis(20));
        let staging = dir.path().join("file.txt.tmp");
        std::fs::write(&staging, b"renamed in\n").unwrap();
        std::fs::rename(&staging, &target).unwrap();
        src.pump();
        assert_eq!(src.len(), 11);
        assert_eq!(&*src.bytes(0..11), b"renamed in\n");
        assert_eq!(src.revision(), 1);
    }

    #[test]
    fn live_source_rebuild_flow_against_line_index() {
        use crate::line_index::LineIndex;

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), b"a\nb\nc\n").unwrap();
        let src = LiveFileSource::open(tmp.path()).unwrap();

        let mut idx = LineIndex::new();
        idx.notice_new_bytes(&src);
        assert_eq!(idx.line_count(), 3);
        let r0 = src.revision();

        // Rewrite to a 5-line file.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(tmp.path(), b"one\ntwo\nthree\nfour\nfive\n").unwrap();
        src.pump();
        assert_ne!(src.revision(), r0, "revision must bump on rewrite");

        // Caller's job to rebuild the index — mirror what app::rebuild_after_replace does.
        idx = LineIndex::new();
        idx.notice_new_bytes(&src);
        assert_eq!(idx.line_count(), 5);
        assert_eq!(&*src.bytes(idx.line_range(2, &src)), b"three");
    }

    #[test]
    fn transforming_source_passes_through_when_off() {
        let inner = MockSource::new();
        inner.append(b"hello\nworld\n");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Off);
        assert_eq!(&*t.bytes(0..t.len()), b"hello\nworld\n");
        assert!(t.last_error().is_none());
        assert_eq!(t.revision(), 0);
    }

    #[test]
    fn transforming_source_emits_pretty_bytes_when_on() {
        let inner = MockSource::new();
        inner.append(b"{\"a\":1,\"b\":2}");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Json);
        let out = t.bytes(0..t.len()).to_vec();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"a\": 1"));
        assert!(s.contains("\"b\": 2"));
        assert!(t.last_error().is_none());
    }

    #[test]
    fn transforming_source_revision_bumps_on_mode_change() {
        let inner = MockSource::new();
        inner.append(b"{\"x\":1}");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Off);
        let r0 = t.revision();
        t.set_prettify_mode(PrettifyMode::Json);
        assert!(t.revision() > r0);
        let r1 = t.revision();
        t.set_prettify_mode(PrettifyMode::Off);
        assert!(t.revision() > r1);
    }

    #[test]
    fn transforming_source_falls_back_to_raw_on_parse_error() {
        let inner = MockSource::new();
        inner.append(b"not actually json");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Json);
        // Cache holds raw bytes since the transform failed.
        assert_eq!(&*t.bytes(0..t.len()), b"not actually json");
        let err = t.last_error().expect("expected parse error to be surfaced");
        assert!(err.contains("json"), "expected json in error, got: {err}");
        // Status label reflects the error.
        let label = t.prettify_label().expect("error → label should be set");
        assert!(label.ends_with(":err"), "expected :err label, got: {label}");
    }

    #[test]
    fn transforming_source_set_mode_recovers_from_error() {
        let inner = MockSource::new();
        inner.append(b"plain content");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Json);
        assert!(t.last_error().is_some());
        t.set_prettify_mode(PrettifyMode::Off);
        assert!(t.last_error().is_none());
        assert_eq!(&*t.bytes(0..t.len()), b"plain content");
    }

    #[test]
    fn transforming_source_toggle_flips_between_active_and_off() {
        let inner = MockSource::new();
        inner.append(b"{\"x\":1}");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Json);
        assert_eq!(t.mode(), PrettifyMode::Json);
        t.toggle_prettify();
        assert_eq!(t.mode(), PrettifyMode::Off);
        t.toggle_prettify();
        // Flipped back to last active.
        assert_eq!(t.mode(), PrettifyMode::Json);
    }

    #[test]
    fn transforming_source_redetect_picks_up_format() {
        let inner = MockSource::new();
        inner.append(b"<?xml version=\"1.0\"?><root/>");
        let t = TransformingSource::wrap(Box::new(inner), PrettifyMode::Off);
        // Initially raw; after redetect, should be XML.
        t.redetect_prettify();
        assert_eq!(t.mode(), PrettifyMode::Xml);
    }

    #[test]
    fn live_source_directory_errors() {
        let dir = tempfile::tempdir().unwrap();
        let err = LiveFileSource::open(dir.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn mock_source_grows_and_finishes() {
        let m = MockSource::new();
        assert_eq!(m.len(), 0);
        assert!(!m.is_complete());
        m.append(b"abc");
        assert_eq!(m.len(), 3);
        assert_eq!(&*m.bytes(0..3), b"abc");
        m.append(b"def");
        assert_eq!(&*m.bytes(0..6), b"abcdef");
        m.finish();
        assert!(m.is_complete());
    }
}
