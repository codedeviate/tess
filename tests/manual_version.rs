use std::process::Command;

fn manual_output() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_tess"))
        .arg("--manual")
        .output()
        .expect("run tess --manual");
    assert!(out.status.success(), "tess --manual failed: {:?}", out.status);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn manual_resolves_version_token() {
    let text = manual_output();
    let ver = env!("CARGO_PKG_VERSION");
    assert!(
        text.contains(ver),
        "rendered manual must contain the crate version {ver}"
    );
    assert!(
        !text.contains("{{VERSION}}"),
        "rendered manual must not contain the literal token"
    );
}

#[test]
fn manual_source_uses_token_not_hardcoded_version() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/MANUAL.md"))
        .expect("read MANUAL.md");
    assert!(src.contains("{{VERSION}}"), "MANUAL.md must use the {{VERSION}} token");
    // No hardcoded `tess 0.NN.N` version stamp should remain in the source.
    let re = regex::Regex::new(r"tess `?0\.\d+\.\d+").unwrap();
    assert!(
        !re.is_match(&src),
        "MANUAL.md must not hardcode a tess version stamp"
    );
}
