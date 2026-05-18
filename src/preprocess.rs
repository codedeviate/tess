//! Input preprocessor: pipe the source file through a user-defined command
//! before tess reads it. Supports `--preprocess CMD` (CLI) and `$LESSOPEN`
//! (env var). Pipe-mode only — the command must start with `|`, and `%s`
//! is substituted with the source file path (shell-quoted).

use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct Preprocessor {
    /// The command string with the `|` prefix stripped. Still contains `%s`
    /// as a placeholder for the file path.
    pub command: String,
}

impl Preprocessor {
    /// Parse a raw value (from CLI flag or env var). The value must start
    /// with `|`; that prefix is stripped from the stored `command`.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let Some(rest) = raw.strip_prefix('|') else {
            return Err(format!(
                "preprocess: '{}' must start with '|' (tempfile mode is not supported)",
                raw
            ));
        };
        if rest.trim().is_empty() {
            return Err("preprocess: command after '|' is empty".to_string());
        }
        Ok(Self { command: rest.to_string() })
    }
}

#[derive(Debug)]
pub enum PreprocessResult {
    /// Command succeeded; use these bytes as the source.
    Bytes(Vec<u8>),
    /// Command failed (non-zero exit, empty output, or spawn error). The
    /// caller should fall back to the raw file and surface the stderr.
    Failed { stderr: String },
}

/// Run the preprocessor against `file_path`. Substitutes `%s` with the
/// path (shell-quoted via single quotes with internal quote escaping).
/// Spawns via `sh -c`.
pub fn run(p: &Preprocessor, file_path: &Path) -> PreprocessResult {
    let path_str = file_path.to_string_lossy();
    let quoted = shell_quote(&path_str);
    let cmd_line = p.command.replace("%s", &quoted);

    let output_result = Command::new("/bin/sh")
        .arg("-c")
        .arg(&cmd_line)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    match output_result {
        Ok(output) => {
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let msg = if stderr.is_empty() {
                    format!("exited with status {}", output.status)
                } else {
                    stderr
                };
                return PreprocessResult::Failed { stderr: msg };
            }
            if output.stdout.is_empty() {
                return PreprocessResult::Failed {
                    stderr: "preprocessor produced no output".to_string(),
                };
            }
            PreprocessResult::Bytes(output.stdout)
        }
        Err(e) => PreprocessResult::Failed {
            stderr: format!("spawn failed: {e}"),
        },
    }
}

/// Single-quote a string for safe inclusion in a sh -c command. Any
/// internal single quote is closed, escaped, and reopened: `it's` becomes
/// `'it'\''s'`.
fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_strips_pipe_prefix() {
        let p = Preprocessor::parse("|cat %s").unwrap();
        assert_eq!(p.command, "cat %s");
    }

    #[test]
    fn parse_rejects_value_without_pipe() {
        let err = Preprocessor::parse("cat %s").unwrap_err();
        assert!(err.contains("must start with '|'"));
    }

    #[test]
    fn parse_rejects_pipe_with_empty_command() {
        let err = Preprocessor::parse("|").unwrap_err();
        assert!(err.contains("empty"));
        let err = Preprocessor::parse("|   ").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn run_cat_on_fixture_returns_bytes() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"hello world\n").unwrap();
        let p = Preprocessor::parse("|cat %s").unwrap();
        match run(&p, tmp.path()) {
            PreprocessResult::Bytes(b) => assert_eq!(b, b"hello world\n"),
            PreprocessResult::Failed { stderr } => panic!("expected Bytes, got Failed: {stderr}"),
        }
    }

    #[test]
    fn run_false_returns_failed() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"x").unwrap();
        let p = Preprocessor::parse("|false").unwrap();
        match run(&p, tmp.path()) {
            PreprocessResult::Failed { .. } => {}
            PreprocessResult::Bytes(_) => panic!("expected Failed"),
        }
    }

    #[test]
    fn run_missing_command_returns_failed() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"x").unwrap();
        let p = Preprocessor::parse("|definitely-not-a-real-command-x9z %s").unwrap();
        match run(&p, tmp.path()) {
            PreprocessResult::Failed { stderr } => {
                assert!(!stderr.is_empty(), "stderr should describe the error");
            }
            PreprocessResult::Bytes(_) => panic!("expected Failed"),
        }
    }

    #[test]
    fn run_empty_stdout_returns_failed() {
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"x").unwrap();
        let p = Preprocessor::parse("|true").unwrap();
        match run(&p, tmp.path()) {
            PreprocessResult::Failed { stderr } => {
                assert!(stderr.contains("no output"));
            }
            PreprocessResult::Bytes(_) => panic!("expected Failed"),
        }
    }

    #[test]
    fn run_substitutes_path_with_spaces_safely() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("name with spaces.txt");
        std::fs::write(&path, b"content\n").unwrap();
        let p = Preprocessor::parse("|cat %s").unwrap();
        match run(&p, &path) {
            PreprocessResult::Bytes(b) => assert_eq!(b, b"content\n"),
            PreprocessResult::Failed { stderr } => panic!("expected Bytes, got: {stderr}"),
        }
    }

    #[test]
    fn shell_quote_handles_single_quote() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
