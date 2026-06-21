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
        let enc = crate::charset::parse_label(flag)
            .ok_or_else(|| format!("unknown --encoding label: {flag}"))?;
        // The line index splits on a lone 0x0A, and UTF-16 code units embed
        // 0x00/0x0A, so every line after the first would byte-misalign. Reject
        // UTF-16 rather than render mojibake (a future enhancement could
        // transcode the whole buffer to UTF-8 at the source layer).
        if matches!(enc.label(), "UTF-16LE" | "UTF-16BE") {
            return Err(format!("encoding {} is not supported (line-oriented decode)", enc.label()));
        }
        return Ok(enc);
    }
    // Default path. A UTF-16 BOM cannot be honored for the same reason; error
    // rather than mojibake. (A UTF-8 BOM just means UTF-8, the default.)
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return Err("UTF-16 BOM detected; UTF-16 is not supported (line-oriented decode)".to_string());
    }
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
    fn resolve_encoding_plain_default_is_utf8() {
        assert!(resolve_encoding("utf-8", b"hello").unwrap().is_utf8());
        // A UTF-8 BOM still resolves to UTF-8 (the default).
        assert!(resolve_encoding("utf-8", &[0xEF,0xBB,0xBF,b'h']).unwrap().is_utf8());
    }
    #[test]
    fn resolve_encoding_utf16_is_rejected() {
        // UTF-16 BOM under the default → error (line-oriented decode can't handle it).
        assert!(resolve_encoding("utf-8", &[0xFF,0xFE,b'h',0]).is_err());
        assert!(resolve_encoding("utf-8", &[0xFE,0xFF,0,b'h']).is_err());
        // Explicit UTF-16 label → error too.
        assert!(resolve_encoding("utf-16le", b"").is_err());
        assert!(resolve_encoding("utf-16be", b"").is_err());
    }
    #[test]
    fn resolve_encoding_unknown_errs() {
        assert!(resolve_encoding("bogus-xyz", b"").is_err());
    }
}
