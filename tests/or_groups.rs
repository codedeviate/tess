// Drives the built binary in batch mode with a temp HOME so config + CLI
// OR-flags exercise the predicate end-to-end.
use std::process::Command;

fn tess() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tess"))
}

#[test]
fn cli_or_grep_default_pool_narrows_within_required() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    std::fs::create_dir_all(home.join(".config/tess")).unwrap();
    std::fs::write(
        home.join(".config/tess/formats.toml"),
        "[format.app]\nregex = '^(?P<lvl>\\w+) (?P<msg>.+)$'\n",
    )
    .unwrap();
    let log = home.join("app.log");
    std::fs::write(&log, "ERROR boom\nINFO ok\nERROR panic\n").unwrap();

    let out = tess()
        .env("HOME", home)
        .args([
            "--format", "app",
            "--filter", "lvl=ERROR",
            "--or-grep", "panic",
            "--or-grep", "boom",
            "-o", "-",
        ])
        .arg(&log)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout, "ERROR boom\nERROR panic\n", "stderr: {}", String::from_utf8_lossy(&out.stderr));
}
