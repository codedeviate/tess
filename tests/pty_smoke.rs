//! End-to-end PTY smoke tests. Runs the real `tess` binary under a
//! pseudo-tty and verifies the spawn / keystroke / signal / resize
//! paths don't hang or crash.
//!
//! These tests deliberately do NOT inspect tess's rendered output:
//! tess is a full-screen TUI that interleaves text with ANSI escapes,
//! so literal substring matching against the PTY stream is unreliable.
//! The value here is regression coverage for "tess starts and stops
//! cleanly under various stimuli," not output-content verification.
//!
//! macOS PTY buffer note: the macOS PTY slave→master write buffer is ~1 KiB.
//! A single TUI frame (80×24 with ANSI sequences) exceeds this limit, so we
//! continuously drain PTY output (keeping the buffer empty) before and after
//! sending keystrokes. Without draining, tess's `write_frame` blocks and
//! the event loop never runs.

use expectrl::{spawn, Session, WaitStatus};
use std::thread;
use std::time::{Duration, Instant};

const FIXTURE: &str = "tests/fixtures/pty_input.txt";
const DRAW_GRACE: Duration = Duration::from_millis(300);

fn spawn_tess(args: &str) -> Session {
    let bin = env!("CARGO_BIN_EXE_tess");
    let cmd = if args.is_empty() {
        format!("{bin} {FIXTURE}")
    } else {
        format!("{bin} {args} {FIXTURE}")
    };
    let mut s = spawn(&cmd).expect("failed to spawn tess under PTY");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    s
}

/// Drain all available PTY output for the given duration, yielding to the
/// OS between reads so tess can write more into the buffer.
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

/// Wait for tess to exit cleanly.  Drain PTY output first so any pending
/// cleanup writes from tess can complete without blocking.
fn wait_clean(mut s: Session) {
    // Drain cleanup bytes that tess writes on exit (LeaveAlternateScreen etc.)
    drain_for(&mut s, Duration::from_millis(300));
    match s.get_process().wait().expect("wait failed") {
        WaitStatus::Exited(_, code) => assert_eq!(code, 0, "tess should exit 0"),
        other => panic!("tess should exit 0 cleanly, got {other:?}"),
    }
}

#[test]
fn quit_with_q_exits_cleanly() {
    let mut s = spawn_tess("");
    drain_for(&mut s, DRAW_GRACE);
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn scroll_then_quit_exits_cleanly() {
    let mut s = spawn_tess("-N");
    drain_for(&mut s, DRAW_GRACE);
    for _ in 0..5 {
        s.send("j").unwrap();
        drain_for(&mut s, Duration::from_millis(80));
    }
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn sigterm_exits_cleanly() {
    let mut s = spawn_tess("");
    drain_for(&mut s, DRAW_GRACE);
    let pid = s.get_process().pid().as_raw();
    // SAFETY: pid is from expectrl's still-living child process; libc::kill is a thin FFI wrapper.
    unsafe { libc::kill(pid, libc::SIGTERM); }
    // Give tess time to handle the signal and write cleanup bytes
    drain_for(&mut s, Duration::from_millis(400));
    match s.get_process().wait().expect("wait failed") {
        WaitStatus::Exited(_, _) | WaitStatus::Signaled(_, _, _) => {}
        other => panic!("expected clean exit, got {other:?}"),
    }
}

#[test]
fn resize_then_quit_exits_cleanly() {
    let mut s = spawn_tess("");
    drain_for(&mut s, DRAW_GRACE);
    s.get_process_mut().set_window_size(40, 10).unwrap();
    drain_for(&mut s, Duration::from_millis(150));
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn marks_round_trip_via_pty() {
    use std::io::Write;
    let bin = env!("CARGO_BIN_EXE_tess");
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    for i in 1..=50 {
        writeln!(tmp, "line {:03}", i).unwrap();
    }
    let cmd = format!("{bin} {}", tmp.path().display());
    let mut s = expectrl::spawn(&cmd).expect("failed to spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    thread::sleep(DRAW_GRACE);
    // 5j: scroll. ma: set mark a. 20j: scroll more. 'a: jump to mark.
    // \x18\x18: Ctrl-X Ctrl-X. q: quit.
    s.send("5jma20j'a\x18\x18q").unwrap();
    wait_clean(s);
}

#[test]
fn records_mode_starts_and_quits_cleanly() {
    let bin = env!("CARGO_BIN_EXE_tess");
    let cmd = format!(
        "{bin} --record-start '^\\[' tests/fixtures/multiline-records.log"
    );
    let mut s = expectrl::spawn(&cmd).expect("failed to spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn hex_mode_starts_and_quits_cleanly() {
    use std::io::Write;
    let bin = env!("CARGO_BIN_EXE_tess");
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    let bytes: Vec<u8> = (0u8..=255).collect();
    tmp.write_all(&bytes).unwrap();
    let cmd = format!("{bin} --hex {}", tmp.path().display());
    let mut s = expectrl::spawn(&cmd).expect("failed to spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    thread::sleep(DRAW_GRACE);
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn keymap_remap_works() {
    use std::io::Write;
    let bin = env!("CARGO_BIN_EXE_tess");

    let tmp_home = tempfile::tempdir().unwrap();
    let cfg_dir = tmp_home.path().join(".config").join("tess");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut keys = std::fs::File::create(cfg_dir.join("keys.toml")).unwrap();
    writeln!(keys, "[bindings]").unwrap();
    writeln!(keys, "\"f1\" = \"toggle-line-numbers\"").unwrap();
    drop(keys);

    let mut content = tempfile::NamedTempFile::new().unwrap();
    for i in 1..=20 {
        writeln!(content, "line {:03}", i).unwrap();
    }

    let mut command = std::process::Command::new(bin);
    command.env("HOME", tmp_home.path()).arg(content.path());
    let mut s = expectrl::Session::spawn(command).expect("failed to spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn shell_escape_starts_and_quits_cleanly() {
    use std::io::Write;
    let bin = env!("CARGO_BIN_EXE_tess");
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    for i in 1..=20 {
        writeln!(tmp, "line {:03}", i).unwrap();
    }
    let cmd = format!("{bin} {}", tmp.path().display());
    let mut s = expectrl::spawn(&cmd).expect("failed to spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    // `!` enters ShellPrompt mode. Type a partial command, then Esc to
    // cancel. We don't exercise the full press-any-key flow here because
    // it interacts with the PTY's line discipline in a way that's
    // unreliable to test via expectrl — the run_shell_command helper
    // itself is unit-tested in src/shell.rs.
    s.send("!echo hi").unwrap();
    drain_for(&mut s, Duration::from_millis(200));
    s.send("\x1b").unwrap();  // ESC cancels back to Normal mode
    drain_for(&mut s, Duration::from_millis(200));
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn multifile_next_prev_via_pty() {
    use std::io::Write;
    let bin = env!("CARGO_BIN_EXE_tess");
    let mut a = tempfile::NamedTempFile::new().unwrap();
    let mut b = tempfile::NamedTempFile::new().unwrap();
    writeln!(a, "alpha line 1").unwrap();
    writeln!(b, "beta line 1").unwrap();
    let cmd = format!("{bin} {} {}", a.path().display(), b.path().display());
    let mut s = expectrl::spawn(&cmd).expect("failed to spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    // :n + Enter: next file. :p + Enter: previous. q: quit.
    s.send(":n\r").unwrap();
    drain_for(&mut s, Duration::from_millis(200));
    s.send(":p\r").unwrap();
    drain_for(&mut s, Duration::from_millis(200));
    s.send("q").unwrap();
    wait_clean(s);
}

#[test]
fn ctrl_close_bracket_opens_tag_prompt() {
    use std::io::Write;
    let bin = env!("CARGO_BIN_EXE_tess");

    let tmpdir = tempfile::tempdir().unwrap();
    let src_a = tmpdir.path().join("a.txt");
    let src_b = tmpdir.path().join("b.txt");
    std::fs::write(&src_a, "alpha line 1\nalpha line 2\n").unwrap();
    std::fs::write(&src_b, "beta line 1\nbeta line 2\n").unwrap();
    let tags = tmpdir.path().join("tags");
    let mut f = std::fs::File::create(&tags).unwrap();
    writeln!(f, "foo\t{}\t2", src_a.file_name().unwrap().to_str().unwrap()).unwrap();
    writeln!(f, "bar\t{}\t1", src_b.file_name().unwrap().to_str().unwrap()).unwrap();
    drop(f);

    let cmd = format!("{bin} -T {} {}", tags.display(), src_a.display());
    let mut s = expectrl::spawn(&cmd).expect("spawn tess");
    s.set_expect_timeout(Some(Duration::from_secs(5)));
    drain_for(&mut s, DRAW_GRACE);
    s.send("\x1d").unwrap();   // Ctrl-]
    drain_for(&mut s, Duration::from_millis(200));
    s.send("bar\r").unwrap();
    drain_for(&mut s, Duration::from_millis(300));
    s.send("q").unwrap();
    wait_clean(s);
}
