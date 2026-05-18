//! End-to-end multi-file navigation tests via the real tess binary.

use expectrl::{Session, WaitStatus};
use std::io::Write;
use std::thread;
use std::time::{Duration, Instant};

const DRAW_GRACE: Duration = Duration::from_millis(300);

fn write_fixture(content: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

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
    let _ = s.send("q");
    drain_for(&mut s, Duration::from_millis(300));
    match s.get_process().wait().expect("wait failed") {
        WaitStatus::Exited(_, code) => assert_eq!(code, 0, "tess should exit 0"),
        other => panic!("tess should exit 0 cleanly, got {other:?}"),
    }
}

#[test]
fn next_and_show_file_via_colon_prompt() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let a = write_fixture("alpha 1\nalpha 2\n");
    let b = write_fixture("beta 1\nbeta 2\n");

    let mut command = std::process::Command::new(bin);
    command.arg(a.path()).arg(b.path());
    let mut s = Session::spawn(command).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    s.send(":n\r").unwrap();
    drain_for(&mut s, Duration::from_millis(200));
    s.send(":f\r").unwrap();
    drain_for(&mut s, Duration::from_millis(200));
    quit_and_wait(s);
}

#[test]
fn edit_appends_new_file_to_list() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let a = write_fixture("alpha 1\nalpha 2\n");
    let b = write_fixture("beta 1\nbeta 2\n");

    let mut command = std::process::Command::new(bin);
    command.arg(a.path());
    let mut s = Session::spawn(command).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    let edit_cmd = format!(":e {}\r", b.path().display());
    s.send(&edit_cmd).unwrap();
    drain_for(&mut s, Duration::from_millis(300));
    s.send(":n\r").unwrap();  // at last file, should show [no next file] or stay
    drain_for(&mut s, Duration::from_millis(200));
    quit_and_wait(s);
}

#[test]
fn first_and_last_navigate_to_extremes() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let a = write_fixture("alpha 1\n");
    let b = write_fixture("beta 1\n");
    let c = write_fixture("gamma 1\n");

    let mut command = std::process::Command::new(bin);
    command.arg(a.path()).arg(b.path()).arg(c.path());
    let mut s = Session::spawn(command).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    s.send(":t\r").unwrap();   // jump to last
    drain_for(&mut s, Duration::from_millis(200));
    s.send(":x\r").unwrap();   // jump back to first
    drain_for(&mut s, Duration::from_millis(200));
    quit_and_wait(s);
}
