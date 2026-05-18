//! "Drop TUI -> run shell command -> restore TUI" helper. Used by `!cmd` at
//! runtime AND by lesskey's `!shell command` bindings (Task 3).

use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

use crate::terminal::restore_terminal_best_effort;

/// Run a shell command with the user's `$SHELL` (falling back to `/bin/sh`).
/// Tears down and rebuilds the terminal around the command.
///
/// On success returns `Ok(())` — the terminal is restored and the caller
/// should redraw. On failure (shell not found, etc.) returns `Err(...)`;
/// the terminal is still restored. The error message is safe to surface in
/// the status line.
pub fn run_shell_command(cmd_text: &str) -> io::Result<()> {
    // Tear down the TUI. `restore_terminal_best_effort` is idempotent and
    // does the same work the active TerminalGuard's Drop would: disable
    // raw mode + LeaveAlternateScreen + Show cursor.
    restore_terminal_best_effort();

    // Print a separator so the user sees where command output begins.
    let _ = writeln!(io::stderr(), "---");

    // Resolve the shell.
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());

    // Spawn the child with inherited stdin/stdout/stderr.
    let status_result = Command::new(&shell)
        .arg("-c")
        .arg(cmd_text)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status();

    let _ = writeln!(io::stderr(), "[Press any key to continue]");
    let _ = io::stderr().flush();

    // Re-enable raw mode BEFORE reading the keystroke. In canonical
    // (cooked) mode, read() would block until a newline; raw mode
    // delivers single bytes immediately so any keypress unblocks.
    // Both calls are best-effort: in test environments there is no real
    // TTY and enable_raw_mode() returns ENXIO — that's fine, the caller's
    // TerminalGuard will clean up anyway.
    use crossterm::cursor::Hide;
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
    let _ = enable_raw_mode();

    // Read one byte from stdin. If stdin is closed or fails, proceed
    // anyway — the user will see the terminal restore happen.
    let mut buf = [0u8; 1];
    let _ = io::stdin().read(&mut buf);

    // NOW enter the alt-screen so the next frame draw paints over the
    // shell-command output.
    let _ = crossterm::execute!(io::stdout(), EnterAlternateScreen, Hide);

    status_result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests exec real subprocesses and toggle terminal modes.
    // They must run serial (the project uses --test-threads=1 across the
    // board, so this is enforced at the cargo-test command level).

    #[test]
    fn run_shell_command_happy_path() {
        let result = run_shell_command("true");
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    #[test]
    fn run_shell_command_propagates_nonzero_exit_as_ok() {
        // The helper returns Ok even when the child exits non-zero —
        // it's the spawn result that matters, not the exit code.
        let result = run_shell_command("false");
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    #[test]
    fn run_shell_command_missing_executable_returns_err() {
        let prev = std::env::var("SHELL").ok();
        std::env::set_var("SHELL", "/this/path/does/not/exist/x9z");
        let result = run_shell_command("true");
        // Restore SHELL first so a failed assertion doesn't pollute later tests.
        if let Some(p) = prev {
            std::env::set_var("SHELL", p);
        } else {
            std::env::remove_var("SHELL");
        }
        assert!(result.is_err(), "expected Err for missing shell, got {:?}", result);
    }
}
