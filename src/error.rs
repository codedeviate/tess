use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum Error {
    OpenFile { path: PathBuf, source: std::io::Error },
    NotAFile { path: PathBuf },
    NoInput,
    Runtime(String),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::OpenFile { .. } | Error::NotAFile { .. } | Error::NoInput => 1,
            Error::Runtime(_) => 2,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OpenFile { path, source } => {
                write!(f, "rustless: {}: {}", path.display(), source)
            }
            Error::NotAFile { path } => {
                write!(f, "rustless: {}: not a regular file", path.display())
            }
            Error::NoInput => write!(f, "rustless: no input (specify a file or pipe stdin)"),
            Error::Runtime(msg) => write!(f, "rustless: {}", msg),
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
        assert_eq!(format!("{}", e), "rustless: /nope: no such file");
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
