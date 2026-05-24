//! End-to-end tests for global + local config layering. Each test sets
//! $TESS_GLOBAL_CONFIG_DIR and $HOME to temp directories, runs the tess
//! binary, and asserts on stdout/stderr.

use std::process::Command;
use tempfile::tempdir;

fn tess_bin() -> &'static str {
    env!("CARGO_BIN_EXE_tess")
}

fn run_list_formats(home: &std::path::Path, global: Option<&std::path::Path>) -> std::process::Output {
    let mut cmd = Command::new(tess_bin());
    cmd.arg("--list-formats");
    cmd.env("HOME", home);
    cmd.env_remove("TESS_GLOBAL_CONFIG_DIR");
    if let Some(g) = global {
        cmd.env("TESS_GLOBAL_CONFIG_DIR", g);
    }
    cmd.output().expect("failed to run tess")
}

fn write_local_formats(home: &std::path::Path, body: &str) {
    let dir = home.join(".config").join("tess");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("formats.toml"), body).unwrap();
}

fn write_global_formats(global: &std::path::Path, body: &str) {
    std::fs::write(global.join("formats.toml"), body).unwrap();
}

#[test]
fn global_only_format_visible() {
    let home = tempdir().unwrap();
    let global = tempdir().unwrap();
    write_global_formats(
        global.path(),
        r#"
[format.org-app]
regex = "^ORG (?P<msg>.+)$"
"#,
    );
    let out = run_list_formats(home.path(), Some(global.path()));
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("org-app"), "missing org-app in:\n{stdout}");
    assert!(stdout.contains("[global]"), "missing [global] tag in:\n{stdout}");
}

#[test]
fn local_only_format_visible() {
    let home = tempdir().unwrap();
    write_local_formats(
        home.path(),
        r#"
[format.my-app]
regex = "^MY (?P<msg>.+)$"
"#,
    );
    let out = run_list_formats(home.path(), None);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("my-app"), "missing my-app in:\n{stdout}");
    assert!(stdout.contains("[local]"), "missing [local] tag in:\n{stdout}");
}

#[test]
fn local_overrides_global() {
    let home = tempdir().unwrap();
    let global = tempdir().unwrap();
    write_global_formats(
        global.path(),
        r#"
[format.shared]
regex = "^G (?P<from>.+)$"
"#,
    );
    write_local_formats(
        home.path(),
        r#"
[format.shared]
regex = "^L (?P<from>.+)$"
"#,
    );
    let out = run_list_formats(home.path(), Some(global.path()));
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // The override annotation names the layer immediately replaced.
    assert!(
        stdout.contains("[local, overrides global]"),
        "missing override label in:\n{stdout}"
    );
}

#[test]
fn local_overrides_builtin() {
    let home = tempdir().unwrap();
    write_local_formats(
        home.path(),
        r#"
[format.apache-common]
regex = "^MINE (?P<host>.+)$"
"#,
    );
    let out = run_list_formats(home.path(), None);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[local, overrides built-in]"),
        "missing override label in:\n{stdout}"
    );
}

#[test]
fn bad_global_toml_warns_and_continues() {
    let home = tempdir().unwrap();
    let global = tempdir().unwrap();
    write_global_formats(global.path(), "= = not valid =");
    write_local_formats(
        home.path(),
        r#"
[format.my-app]
regex = "^MY (?P<msg>.+)$"
"#,
    );
    let out = run_list_formats(home.path(), Some(global.path()));
    assert!(out.status.success(), "should succeed despite bad global");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tess: warning:") && stderr.contains("formats.toml"),
        "expected warning, got stderr: {stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("my-app"), "local format should still load");
}

#[test]
fn bad_local_toml_fails_startup() {
    let home = tempdir().unwrap();
    write_local_formats(home.path(), "= = not valid =");
    let out = run_list_formats(home.path(), None);
    assert!(!out.status.success(), "should fail with bad local config");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("formats.toml"),
        "stderr should name the broken file: {stderr}"
    );
}

#[test]
fn no_global_no_local_matches_today() {
    let home = tempdir().unwrap();
    let out = run_list_formats(home.path(), None);
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Built-ins still listed.
    assert!(stdout.contains("apache-common"));
    assert!(stdout.contains("apache-combined"));
    assert!(stdout.contains("nginx-combined"));
    // All tagged built-in.
    for line in stdout.lines() {
        if line.contains("apache-common")
            || line.contains("apache-combined")
            || line.contains("nginx-combined")
        {
            assert!(line.contains("[built-in]"), "expected [built-in], got: {line}");
        }
    }
}
