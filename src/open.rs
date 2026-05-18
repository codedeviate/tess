//! Helper to open a file source, applying the live-mode wrapper and/or
//! preprocessor as configured. Factored out of `main.rs` so that `app.rs`
//! can also call it when switching files via colon-prompt commands.

use crate::cli::Args;
use crate::error::{Error, Result};
use crate::source::{FileSource, LiveFileSource, MemorySource, Source};

/// Open a single source file using the same pipeline as startup:
/// preprocessor (if configured), live-mode wrapper (if --live).
///
/// Returns the boxed Source, the user-facing label, and any preprocess
/// stderr that should be surfaced in the status line.
///
/// Content-type detection and the prettify wrapper are NOT applied here —
/// they are handled at startup (or not at all on file-switch).
pub fn open_source_for_path(
    path: &std::path::Path,
    args: &Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
) -> Result<(Box<dyn Source>, String, Option<String>)> {
    let label = path.display().to_string();
    if args.live {
        let lfs = LiveFileSource::open(path).map_err(|source| {
            if let std::io::ErrorKind::InvalidInput = source.kind() {
                Error::NotAFile { path: path.to_path_buf() }
            } else {
                Error::OpenFile { path: path.to_path_buf(), source }
            }
        })?;
        return Ok((Box::new(lfs), label, None));
    }
    if let Some(p) = preprocessor {
        match crate::preprocess::run(p, path) {
            crate::preprocess::PreprocessResult::Bytes(bytes) => {
                return Ok((Box::new(MemorySource::new(bytes)), label, None));
            }
            crate::preprocess::PreprocessResult::Failed { stderr } => {
                let fs = FileSource::open(path).map_err(|source| {
                    if let std::io::ErrorKind::InvalidInput = source.kind() {
                        Error::NotAFile { path: path.to_path_buf() }
                    } else {
                        Error::OpenFile { path: path.to_path_buf(), source }
                    }
                })?;
                return Ok((Box::new(fs), label, Some(stderr)));
            }
        }
    }
    let fs = FileSource::open(path).map_err(|source| {
        if let std::io::ErrorKind::InvalidInput = source.kind() {
            Error::NotAFile { path: path.to_path_buf() }
        } else {
            Error::OpenFile { path: path.to_path_buf(), source }
        }
    })?;
    Ok((Box::new(fs), label, None))
}
