#[test]
fn examples_cover_new_categories() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_tess"))
        .arg("--examples").output().expect("run");
    assert!(out.status.success());
    let s = String::from_utf8_lossy(&out.stdout);
    for marker in ["OR-group", "layout", "diff", "clipboard"] {
        assert!(s.contains(marker), "examples missing category marker: {marker}");
    }
}
