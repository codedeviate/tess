use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossterm::cursor::{Hide, Show};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};

/// RAII guard that enables raw mode + (optionally) alt screen on construction
/// and restores the terminal on drop (including during panic unwind).
///
/// `with_alt_screen = false` is the `less -X` / `--no-init` mode: stay on
/// the primary screen so content remains in scrollback after exit. Raw
/// mode is still enabled because we need keystroke capture either way.
pub struct TerminalGuard {
    mouse: bool,
    alt_screen: bool,
}

impl TerminalGuard {
    pub fn enter(mouse: bool, with_alt_screen: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        if with_alt_screen {
            crossterm::execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        } else {
            crossterm::execute!(io::stdout(), Hide)?;
        }
        if mouse {
            crossterm::execute!(io::stdout(), crossterm::event::EnableMouseCapture)?;
        }
        Ok(TerminalGuard { mouse, alt_screen: with_alt_screen })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.mouse {
            let _ = crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture);
        }
        if self.alt_screen {
            let _ = crossterm::execute!(io::stdout(), Show, LeaveAlternateScreen);
        } else {
            let _ = crossterm::execute!(io::stdout(), Show);
        }
        let _ = disable_raw_mode();
    }
}

/// Restore terminal manually (for panic hook). Idempotent and best-effort.
pub fn restore_terminal_best_effort() {
    let _ = crossterm::execute!(io::stdout(), Show, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal_best_effort();
        prev(info);
    }));
}

/// Returns a flag that becomes `true` on SIGTERM or SIGHUP.
pub fn install_signal_flag() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag));
    let _ = signal_hook::flag::register(signal_hook::consts::SIGHUP, Arc::clone(&flag));
    flag
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn signal_flag_starts_false() {
        let f = install_signal_flag();
        assert!(!f.load(Ordering::SeqCst));
    }

    #[test]
    fn restore_is_idempotent() {
        // Should not panic when raw mode was never enabled.
        restore_terminal_best_effort();
        restore_terminal_best_effort();
    }
}
