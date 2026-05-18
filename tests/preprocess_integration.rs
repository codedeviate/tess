//! End-to-end test: spawn the real tess binary with --preprocess against
//! a fixture, verify clean exit.

use expectrl::{Session, WaitStatus};
use std::io::Write;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const DRAW_GRACE: Duration = Duration::from_millis(300);

/// Drain all available PTY output for the given duration so tess's write
/// buffer never fills and blocks the event loop.
fn drain_for(s: &mut Session, duration: Duration) {
    let mut buf = [0u8; 4096];
    let deadline = Instant::now() + duration;
    while Instant::now() < deadline {
        match s.try_read(&mut buf) {
            Ok(0) | Err(_) => thread::sleep(Duration::from_millis(5)),
            Ok(_) => {}
        }
    }
}

fn quit_and_wait(mut s: Session) {
    drain_for(&mut s, DRAW_GRACE);
    // Ignore send errors: tess may have already exited (e.g. empty file or
    // fast-path exit). The wait below will catch any non-zero exit code.
    let _ = s.send("q");
    drain_for(&mut s, Duration::from_millis(300));
    match s.get_process().wait().expect("wait failed") {
        WaitStatus::Exited(_, code) => assert_eq!(code, 0, "tess should exit 0"),
        other => panic!("tess should exit 0 cleanly, got {other:?}"),
    }
}

#[test]
fn preprocess_cat_passes_file_through() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(b"hello preprocessor\n").unwrap();
    // Use Command::new so arg quoting is handled by the OS, not tokenize_command.
    let mut cmd = Command::new(bin);
    cmd.arg("--preprocess").arg("|cat %s").arg(tmp.path());
    let s = Session::spawn(cmd).expect("failed to spawn tess");
    quit_and_wait(s);
}

#[test]
fn preprocess_failed_command_falls_back_to_file() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    tmp.write_all(b"real file content\n").unwrap();
    let mut cmd = Command::new(bin);
    cmd.arg("--preprocess").arg("|false").arg(tmp.path());
    let s = Session::spawn(cmd).expect("failed to spawn tess");
    quit_and_wait(s);
}
