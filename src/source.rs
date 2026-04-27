use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;
use std::path::Path;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};

pub trait Source: Send + Sync {
    fn len(&self) -> usize;
    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]>;
    fn is_complete(&self) -> bool;
}

pub struct FileSource {
    inner: Inner,
}

enum Inner {
    Mmap(memmap2::Mmap),
    Buf(Vec<u8>),
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
        if metadata.len() == 0 {
            return Ok(Self { inner: Inner::Buf(Vec::new()) });
        }
        // SAFETY: file remains open as long as Mmap exists; we never modify the file.
        match unsafe { memmap2::Mmap::map(&file) } {
            Ok(m) => Ok(Self { inner: Inner::Mmap(m) }),
            Err(_) => {
                use std::io::Read;
                let mut buf = Vec::new();
                let mut f = File::open(path)?;
                f.read_to_end(&mut buf)?;
                Ok(Self { inner: Inner::Buf(buf) })
            }
        }
    }
}

impl Source for FileSource {
    fn len(&self) -> usize {
        match &self.inner {
            Inner::Mmap(m) => m.len(),
            Inner::Buf(b) => b.len(),
        }
    }

    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> {
        let bytes: &[u8] = match &self.inner {
            Inner::Mmap(m) => &m[..],
            Inner::Buf(b) => &b[..],
        };
        Cow::Borrowed(&bytes[range])
    }

    fn is_complete(&self) -> bool { true }
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
    bytes: Vec<u8>,
}

impl StdinSource {
    /// Read all of stdin into a buffer synchronously. After this returns,
    /// stdin (fd 0) is at EOF; the caller is responsible for redirecting fd 0
    /// to /dev/tty before entering raw mode if interactive input is needed.
    pub fn read_all() -> std::io::Result<Self> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::io::stdin().lock().read_to_end(&mut bytes)?;
        Ok(Self { bytes })
    }
}

impl Source for StdinSource {
    fn len(&self) -> usize { self.bytes.len() }
    fn bytes(&self, range: Range<usize>) -> Cow<'_, [u8]> { Cow::Borrowed(&self.bytes[range]) }
    fn is_complete(&self) -> bool { true }
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
