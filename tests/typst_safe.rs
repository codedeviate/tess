use std::io::Write;
use std::process::{Command, Stdio};

/// Run MANUAL.md preprocessor on `input`, return its stdout.
fn run(input: &str) -> String {
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/typst-safe.awk");
    let mut child = Command::new("awk")
        .arg("-f")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn awk");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("await awk");
    assert!(out.status.success(), "awk failed: {:?}", out.status);
    String::from_utf8(out.stdout).expect("utf8")
}

const ZWSP: char = '\u{200b}';

#[test]
fn inserts_zwsp_between_closing_code_span_and_period() {
    // The recon-typst field-access trigger: `code`. — a U+200B must be
    // inserted between the closing backtick and the period.
    let out = run("see `--grep`. Next\n");
    assert!(
        out.contains(&format!("`{}.", ZWSP)),
        "expected ZWSP between backtick and period, got: {:?}",
        out
    );
}

#[test]
fn escapes_angle_brackets_outside_code() {
    assert_eq!(run("a <b> c\n").trim_end(), "a \\<b\\> c");
}

#[test]
fn leaves_angle_brackets_inside_code_untouched() {
    assert_eq!(run("x `a <b> c` y\n").trim_end(), "x `a <b> c` y");
}

#[test]
fn passes_fenced_blocks_through_verbatim() {
    let input = "```\nlet x: Vec<u8> = `raw`.field;\n```\n";
    let out = run(input);
    assert!(
        out.contains("let x: Vec<u8> = `raw`.field;"),
        "fenced content must be verbatim, got: {:?}",
        out
    );
}

#[test]
fn strips_single_line_html_comments() {
    assert_eq!(run("before <!-- hi --> after\n").trim_end(), "before  after");
}
