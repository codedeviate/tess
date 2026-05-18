use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    OpenFile { path: PathBuf, source: std::io::Error },
    NotAFile { path: PathBuf },
    NoInput,
    Runtime(String),
    /// Tags file specified by `-T` or required by `-t` could not be opened.
    TagFileNotFound,
    /// Tags file is malformed: (reason, path, line number).
    TagFileParse(String, std::path::PathBuf, usize),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::OpenFile { .. }
            | Error::NotAFile { .. }
            | Error::NoInput
            | Error::TagFileNotFound
            | Error::TagFileParse(..) => 1,
            Error::Runtime(_) => 2,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OpenFile { path, source } => {
                write!(f, "tess: {}: {}", path.display(), source)
            }
            Error::NotAFile { path } => {
                write!(f, "tess: {}: not a regular file", path.display())
            }
            Error::NoInput => write!(f, "tess: no input (specify a file or pipe stdin)"),
            Error::Runtime(msg) => write!(f, "tess: {}", msg),
            Error::TagFileNotFound => {
                write!(f, "tags file not found")
            }
            Error::TagFileParse(reason, path, line) => {
                write!(f, "tags file parse error: {reason} at {}:{line}", path.display())
            }
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_wraps_io_error_with_context() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e = Error::OpenFile { path: "/nope".into(), source: io };
        assert_eq!(format!("{}", e), "tess: /nope: no such file");
    }

    #[test]
    fn exit_code_for_startup_is_one() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "x");
        let e = Error::OpenFile { path: "/nope".into(), source: io };
        assert_eq!(e.exit_code(), 1);
    }

    #[test]
    fn exit_code_for_runtime_is_two() {
        let e = Error::Runtime("stdout closed".into());
        assert_eq!(e.exit_code(), 2);
    }
}
