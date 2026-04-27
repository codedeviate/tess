use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}};

pub trait Source: Send + Sync {
    fn len(&self) -> usize;
    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]>;
    fn is_complete(&self) -> bool;
    /// Read any new bytes that have become available since the last call.
    /// Default no-op for static sources. Streaming sources override.
    fn pump(&self) {}
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
