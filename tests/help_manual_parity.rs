use std::collections::BTreeSet;
use std::process::Command;

/// Every long flag shown in `tess --help` must be documented in MANUAL.md
/// (the complete reference). Meta/info flags that are not features are
/// allowlisted. README is intentionally a curated subset and is NOT checked;
/// the man page is generated from --help so needs no parity check.
#[test]
fn every_help_flag_is_documented_in_the_manual() {
    let out = Command::new(env!("CARGO_BIN_EXE_tess"))
        .arg("--help")
        .output()
        .expect("run tess --help");
    assert!(out.status.success(), "tess --help failed: {:?}", out.status);
    let help = String::from_utf8_lossy(&out.stdout);

    // Long flags can be lowercase or uppercase (e.g. --RAW-CONTROL-CHARS,
    // --IGNORE-CASE, --right-IGNORE-CASE).
    let re = regex::Regex::new(r"--[A-Za-z][A-Za-z0-9-]+").unwrap();
    let mut flags: BTreeSet<String> =
        re.find_iter(&help).map(|m| m.as_str().to_string()).collect();

    // Meta/info flags that document tess itself, not a feature — no manual
    // entry required. Keep this list MINIMAL; add only with justification.
    for meta in ["--help", "--version", "--manual", "--examples", "--list-formats"] {
        flags.remove(meta);
    }

    let manual = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/MANUAL.md"))
        .expect("read MANUAL.md");

    let missing: Vec<&String> = flags.iter().filter(|f| !manual.contains(f.as_str())).collect();
    assert!(
        missing.is_empty(),
        "flags present in `tess --help` but absent from MANUAL.md (document them, \
         or allowlist with justification): {:?}",
        missing
    );
}
