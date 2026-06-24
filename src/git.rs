//! Acquire a file's version from git at an arbitrary revision or the index, for
//! `--gitdiff`. The decision logic (`classify`) is pure and unit-tested; the
//! `Command` calls are thin wrappers that capture raw bytes (a blob may be
//! binary / non-UTF-8).

use std::path::{Path, PathBuf};
use std::process::Command;

/// What `git show <spec>` produced.
#[derive(Debug)]
pub enum BlobOutcome {
    /// The bytes (may be empty for an empty file).
    Bytes(Vec<u8>),
    /// Path is not present in the queried rev / index → empty side.
    Absent,
}

/// Classification of a FAILED `git show` invocation, derived purely from stderr.
#[derive(Debug, PartialEq, Eq)]
pub enum FailKind { Absent, NoCommits, BadRev, Other }

/// Pure: map `git show` failure stderr -> a `FailKind`.
pub fn classify(stderr: &str) -> FailKind {
    let s = stderr.to_lowercase();
    if s.contains("does not exist in") || s.contains("exists on disk, but not in") {
        FailKind::Absent
    } else if s.contains("invalid object name 'head'")
        || s.contains("ambiguous argument 'head'")
        || s.contains("bad revision 'head'")
    {
        // Unborn HEAD (`git init`, no commit yet) — head-qualified messages.
        FailKind::NoCommits
    } else if s.contains("unknown revision")
        || s.contains("bad revision")
        || s.contains("invalid object name")
    {
        FailKind::BadRev
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
        // The file may not exist (deletion). Anchor on the canonicalized parent
        // dir (`dir` is `path.parent()` or "."), so `abs` is absolute and strips
        // against the absolute repo root — a bare relative filename here would
        // otherwise leave `abs` relative and fail the strip_prefix below.
        let parent_real = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        match path.file_name() {
            Some(n) => parent_real.join(n),
            None => path.to_path_buf(),
        }
    });
    let root_real = std::fs::canonicalize(&root).unwrap_or(root.clone());
    let rel = abs.strip_prefix(&root_real)
        .map_err(|_| format!("{} is outside the git repository", path.display()))?;
    Ok(GitFile { repo_root: root_real, rel_path: rel.to_string_lossy().replace('\\', "/") })
}

/// `git -C <root> show <rev>:<rel_path>`, capturing stdout bytes.
pub fn rev_blob(file: &GitFile, rev: &str) -> Result<BlobOutcome, String> {
    show_blob(file, &format!("{rev}:{}", file.rel_path), Some(rev))
}

/// `git -C <root> show :<rel_path>` — the staged (index) version.
pub fn index_blob(file: &GitFile) -> Result<BlobOutcome, String> {
    show_blob(file, &format!(":{}", file.rel_path), None)
}

fn show_blob(file: &GitFile, spec: &str, rev: Option<&str>) -> Result<BlobOutcome, String> {
    let out = Command::new("git")
        .arg("-C").arg(&file.repo_root)
        .arg("show").arg(spec)
        .output()
        .map_err(|e| format!("git show failed: {e}"))?;
    if out.status.success() {
        return Ok(BlobOutcome::Bytes(out.stdout));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    match classify(&stderr) {
        FailKind::Absent => Ok(BlobOutcome::Absent),
        FailKind::NoCommits => Err("no commits in HEAD yet".to_string()),
        FailKind::BadRev => Err(format!("bad revision '{}': {}", rev.unwrap_or("<index>"), stderr.trim())),
        FailKind::Other => Err(format!("git show {spec}: {}", stderr.trim())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_absent() {
        assert!(matches!(classify("fatal: path 'foo.rs' does not exist in 'HEAD'"), FailKind::Absent));
        assert!(matches!(classify("fatal: path 'x' exists on disk, but not in 'HEAD'"), FailKind::Absent));
    }

    #[test]
    fn classify_no_commits_only_for_head() {
        assert!(matches!(classify("fatal: invalid object name 'HEAD'."), FailKind::NoCommits));
        assert!(matches!(classify("fatal: ambiguous argument 'HEAD': unknown revision"), FailKind::NoCommits));
        assert!(matches!(classify("fatal: bad revision 'HEAD'"), FailKind::NoCommits));
    }

    #[test]
    fn classify_bad_rev_for_non_head() {
        assert!(matches!(classify("fatal: invalid object name 'deadbeef'"), FailKind::BadRev));
        assert!(matches!(classify("fatal: bad revision 'v9.9.9'"), FailKind::BadRev));
        assert!(matches!(classify("fatal: unknown revision or path not in the working tree"), FailKind::BadRev));
    }

    #[test]
    fn classify_other() {
        assert!(matches!(classify("fatal: something unexpected"), FailKind::Other));
    }

    fn run(dir: &std::path::Path, args: &[&str]) {
        std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
    }
    fn commit(dir: &std::path::Path, msg: &str) {
        std::process::Command::new("git").arg("-C").arg(dir)
            .args(["-c","user.email=t@t","-c","user.name=t","commit","-q","-m",msg])
            .output().expect("commit");
    }

    #[test]
    fn rev_blob_and_index_blob_resolve_each_side() {
        let dir = std::env::temp_dir().join(format!("tess_gdr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"v1\n").unwrap(); run(&dir, &["add","a.txt"]); commit(&dir, "c1");
        std::fs::write(&f, b"v2\n").unwrap(); run(&dir, &["add","a.txt"]); commit(&dir, "c2");
        std::fs::write(&f, b"v3\n").unwrap(); run(&dir, &["add","a.txt"]);   // staged, uncommitted
        std::fs::write(&f, b"v4\n").unwrap();                                 // working tree only

        let gf = resolve(&f).unwrap();
        assert!(matches!(rev_blob(&gf, "HEAD~1").unwrap(), BlobOutcome::Bytes(ref b) if b == b"v1\n"));
        assert!(matches!(rev_blob(&gf, "HEAD").unwrap(),   BlobOutcome::Bytes(ref b) if b == b"v2\n"));
        assert!(matches!(index_blob(&gf).unwrap(),         BlobOutcome::Bytes(ref b) if b == b"v3\n"));
        assert_eq!(std::fs::read(&f).unwrap(), b"v4\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rev_blob_absent_when_path_not_in_rev() {
        let dir = std::env::temp_dir().join(format!("tess_gda_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init","-q"]);
        std::fs::write(dir.join("seed"), b"x\n").unwrap(); run(&dir, &["add","seed"]); commit(&dir, "c1");
        let nf = dir.join("new.txt");
        std::fs::write(&nf, b"new\n").unwrap();
        let gf = resolve(&nf).unwrap();
        assert!(matches!(rev_blob(&gf, "HEAD").unwrap(), BlobOutcome::Absent));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rev_blob_bad_rev_errs_and_unborn_head_says_no_commits() {
        let dir = std::env::temp_dir().join(format!("tess_gdb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init","-q"]);
        let f = dir.join("a.txt");
        std::fs::write(&f, b"x\n").unwrap(); run(&dir, &["add","a.txt"]); commit(&dir, "c1");
        let gf = resolve(&f).unwrap();
        let e = rev_blob(&gf, "nope-not-a-rev").unwrap_err().to_lowercase();
        assert!(e.contains("bad revision") || e.contains("nope-not-a-rev"), "got: {e}");

        let dir2 = std::env::temp_dir().join(format!("tess_gdu_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir2);
        std::fs::create_dir_all(&dir2).unwrap();
        run(&dir2, &["init","-q"]);
        let f2 = dir2.join("a.txt"); std::fs::write(&f2, b"x\n").unwrap();
        let gf2 = resolve(&f2).unwrap();
        assert!(rev_blob(&gf2, "HEAD").unwrap_err().to_lowercase().contains("no commits"));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn resolve_handles_working_tree_deleted_file() {
        let dir = std::env::temp_dir().join(format!("tess_gdel_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        let f = dir.join("gone.txt");
        std::fs::write(&f, b"committed\n").unwrap();
        run(&dir, &["add", "gone.txt"]);
        commit(&dir, "c1");
        std::fs::remove_file(&f).unwrap(); // deleted from the working tree

        let gf = resolve(&f).expect("deleted file should still resolve");
        assert_eq!(gf.rel_path, "gone.txt");
        assert!(matches!(rev_blob(&gf, "HEAD").unwrap(), BlobOutcome::Bytes(ref b) if b == b"committed\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_handles_deleted_file_in_subdir() {
        let dir = std::env::temp_dir().join(format!("tess_gdels_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        run(&dir, &["init", "-q"]);
        let f = dir.join("sub").join("gone.txt");
        std::fs::write(&f, b"x\n").unwrap();
        run(&dir, &["add", "sub/gone.txt"]);
        commit(&dir, "c1");
        std::fs::remove_file(&f).unwrap();

        let gf = resolve(&f).expect("deleted subdir file should resolve");
        assert_eq!(gf.rel_path, "sub/gone.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_handles_deleted_file_given_as_bare_relative_name() {
        // The actual regression: a committed-but-deleted file passed as a BARE
        // relative name (empty parent) with the CWD inside the repo. Pre-fix the
        // canonicalize fallback produced a relative path that couldn't strip the
        // absolute repo root → "outside the git repository". Mutating CWD is
        // process-global, so serialize on the shared test lock and restore CWD
        // before asserting (so a failed assert can't leak a wrong CWD).
        let _guard = crate::test_env::lock();
        let dir = std::env::temp_dir().join(format!("tess_gdbare_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        std::fs::write(dir.join("gone.txt"), b"committed\n").unwrap();
        run(&dir, &["add", "gone.txt"]);
        commit(&dir, "c1");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir).unwrap();
        let result = resolve(std::path::Path::new("gone.txt"));
        let head = result.as_ref().ok().map(|gf| rev_blob(gf, "HEAD"));
        std::env::set_current_dir(&orig).unwrap(); // restore before any assert

        let gf = result.expect("bare-relative deleted file should resolve");
        assert_eq!(gf.rel_path, "gone.txt");
        assert!(matches!(head, Some(Ok(BlobOutcome::Bytes(ref b))) if b == b"committed\n"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
