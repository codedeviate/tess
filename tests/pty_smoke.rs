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
        WaitStatus::Signaled(_, _, _) => {}
        other => panic!("unexpected wait status: {other:?}"),
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
