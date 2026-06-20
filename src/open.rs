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

/// Resolve the effective input encoding: an explicit non-default `--encoding`
/// wins; otherwise a leading BOM selects it; otherwise UTF-8. `Err` on an
/// unknown explicit label.
pub fn resolve_encoding(flag: &str, head: &[u8]) -> std::result::Result<crate::charset::Encoding, String> {
    if flag != "utf-8" {
        return crate::charset::parse_label(flag)
            .ok_or_else(|| format!("unknown --encoding label: {flag}"));
    }
    if head.starts_with(&[0xEF, 0xBB, 0xBF]) { return Ok(crate::charset::Encoding::utf8()); }
    if head.starts_with(&[0xFF, 0xFE]) { return Ok(crate::charset::parse_label("utf-16le").unwrap()); }
    if head.starts_with(&[0xFE, 0xFF]) { return Ok(crate::charset::parse_label("utf-16be").unwrap()); }
    Ok(crate::charset::Encoding::utf8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_encoding_explicit_wins_over_bom() {
        let enc = resolve_encoding("iso-8859-1", &[0xEF,0xBB,0xBF]).unwrap();
        assert_eq!(enc.label(), crate::charset::parse_label("iso-8859-1").unwrap().label());
    }
    #[test]
    fn resolve_encoding_default_honors_bom() {
        assert!(resolve_encoding("utf-8", &[0xEF,0xBB,0xBF,b'h']).unwrap().is_utf8());
        let e16 = resolve_encoding("utf-8", &[0xFF,0xFE,b'h',0]).unwrap();
        assert_eq!(e16.label(), crate::charset::parse_label("utf-16le").unwrap().label());
    }
    #[test]
    fn resolve_encoding_unknown_errs() {
        assert!(resolve_encoding("bogus-xyz", b"").is_err());
    }
}
