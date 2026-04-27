use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;
use std::path::Path;

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
}
