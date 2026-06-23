//! Acquire a file's committed `HEAD` version from git, for `--gitdiff`. The
//! decision logic (`classify`) is pure and unit-tested; the `Command` calls are
//! thin wrappers that capture raw bytes (a blob may be binary / non-UTF-8).

use std::path::{Path, PathBuf};
use std::process::Command;

/// What `git show HEAD:<path>` produced.
#[derive(Debug)]
pub enum BlobOutcome {
    /// The committed bytes (may be empty for an empty file).
    Bytes(Vec<u8>),
    /// Path is tracked-on-disk but not present in HEAD (new/untracked file).
    NotInHead,
    /// HEAD does not resolve (repository has no commits yet).
    NoCommits,
}

/// Classification of a FAILED `git show` invocation, derived purely from stderr.
#[derive(Debug, PartialEq, Eq)]
pub enum FailKind {
    NotInHead,
    NoCommits,
    Other,
}

/// Pure: map `git show` failure stderr -> a `FailKind`.
pub fn classify(stderr: &str) -> FailKind {
    let s = stderr.to_lowercase();
    if s.contains("does not exist in") || s.contains("exists on disk, but not in") {
        FailKind::NotInHead
    } else if s.contains("unknown revision")
        || s.contains("bad revision")
        || s.contains("ambiguous argument 'head'")
    {
        FailKind::NoCommits
    } else {
        FailKind::Other
    }
}

/// A file located within a git repository.
pub struct GitFile {
    pub repo_root: PathBuf,
    /// Repo-relative, forward-slash path (the form `git show HEAD:<x>` wants).
    pub rel_path: String,
}

/// Resolve `path`'s repo root and repo-relative path. Works whether or not the
/// file currently exists on disk (deletion case). Err if not in a git repo.
pub fn resolve(path: &Path) -> Result<GitFile, String> {
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let out = Command::new("git")
        .arg("-C").arg(&dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("git not found: {e}"))?;
    if !out.status.success() {
        return Err(format!("not a git repository: {}", path.display()));
    }
    let root = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| {
        match (path.parent().and_then(|p| std::fs::canonicalize(p).ok()), path.file_name()) {
            (Some(p), Some(n)) => p.join(n),
            _ => path.to_path_buf(),
        }
    });
    let root_real = std::fs::canonicalize(&root).unwrap_or(root.clone());
    let rel = abs.strip_prefix(&root_real)
        .map_err(|_| format!("{} is outside the git repository", path.display()))?;
    Ok(GitFile { repo_root: root_real, rel_path: rel.to_string_lossy().replace('\\', "/") })
}

/// `git -C <root> show HEAD:<rel_path>`, capturing stdout bytes.
pub fn head_blob(file: &GitFile) -> Result<BlobOutcome, String> {
    let spec = format!("HEAD:{}", file.rel_path);
    let out = Command::new("git")
        .arg("-C").arg(&file.repo_root)
        .arg("show").arg(&spec)
        .output()
        .map_err(|e| format!("git show failed: {e}"))?;
    if out.status.success() {
        return Ok(BlobOutcome::Bytes(out.stdout));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match classify(&stderr) {
        FailKind::NotInHead => Ok(BlobOutcome::NotInHead),
        FailKind::NoCommits => Ok(BlobOutcome::NoCommits),
        FailKind::Other => Err(format!("git show {spec}: {}", stderr.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_not_in_head() {
        assert!(matches!(classify("fatal: path 'foo.rs' does not exist in 'HEAD'"), FailKind::NotInHead));
        assert!(matches!(classify("fatal: path 'x' exists on disk, but not in 'HEAD'"), FailKind::NotInHead));
    }

    #[test]
    fn classify_no_commits() {
        assert!(matches!(classify("fatal: bad revision 'HEAD'"), FailKind::NoCommits));
        assert!(matches!(classify("fatal: ambiguous argument 'HEAD': unknown revision"), FailKind::NoCommits));
    }

    #[test]
    fn classify_other() {
        assert!(matches!(classify("fatal: something unexpected"), FailKind::Other));
    }

    #[test]
    fn head_blob_roundtrips_committed_then_sees_change() {
        let dir = std::env::temp_dir().join(format!("tess_gitdiff_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().expect("git");
        };
        run(&["init", "-q"]);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"v1\n").unwrap();
        run(&["add", "a.txt"]);
        std::process::Command::new("git").arg("-C").arg(&dir)
            .args(["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "c1"])
            .output().expect("commit");
        std::fs::write(&f, b"v2\n").unwrap();

        let gf = resolve(&f).expect("resolve repo");
        assert_eq!(gf.rel_path, "a.txt");
        match head_blob(&gf).expect("head_blob") {
            BlobOutcome::Bytes(b) => assert_eq!(b, b"v1\n", "HEAD blob is the committed version"),
            other => panic!("expected Bytes, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn head_blob_untracked_is_not_in_head() {
        let dir = std::env::temp_dir().join(format!("tess_gitdiff_u_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir).args(["init","-q"]).output().unwrap();
        std::fs::write(dir.join("seed"), b"x\n").unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir).args(["add","seed"]).output().unwrap();
        std::process::Command::new("git").arg("-C").arg(&dir)
            .args(["-c","user.email=t@t","-c","user.name=t","commit","-q","-m","c"]).output().unwrap();
        let nf = dir.join("new.txt");
        std::fs::write(&nf, b"brand new\n").unwrap();
        let gf = resolve(&nf).unwrap();
        assert!(matches!(head_blob(&gf).unwrap(), BlobOutcome::NotInHead));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
