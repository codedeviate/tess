use std::io::{self, IsTerminal};
use std::process::ExitCode;

use tess::app;
use tess::cli::Args;
use tess::error::{Error, Result};
use tess::source::{FileSource, Source, StdinSource};
use tess::terminal::{install_panic_hook, install_signal_flag, TerminalGuard};
use tess::viewport::Viewport;
use clap::Parser;

fn main() -> ExitCode {
    install_panic_hook();
    match real_main() {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

/// Redirect fd 0 to /dev/tty so crossterm can read keyboard events after
/// stdin has been fully consumed from a pipe. Opened read+write because
/// crossterm needs both directions on the tty fd.
#[cfg(unix)]
fn redirect_stdin_to_tty() -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")?;
    unsafe {
        if libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

fn real_main() -> Result<()> {
    let args = Args::parse();

    // Resolve source. Track whether we actually consumed stdin — only then
    // do we need to redirect fd 0 to /dev/tty for keyboard input.
    let mut consumed_stdin = false;
    let (src, label): (Box<dyn Source>, String) = if let Some(path) = args.files.first() {
        if args.files.len() > 1 {
            eprintln!(
                "tess: ignoring {} additional file(s) (multi-file navigation not yet supported)",
                args.files.len() - 1
            );
        }
        let fs = FileSource::open(path).map_err(|source| {
            if let std::io::ErrorKind::InvalidInput = source.kind() {
                Error::NotAFile { path: path.clone() }
            } else {
                Error::OpenFile { path: path.clone(), source }
            }
        })?;
        (Box::new(fs), path.display().to_string())
    } else if !io::stdin().is_terminal() {
        let ss = if args.follow {
            StdinSource::spawn_streaming()
                .map_err(|e| Error::Runtime(format!("stdin: {}", e)))?
        } else {
            StdinSource::read_all()
                .map_err(|e| Error::Runtime(format!("stdin: {}", e)))?
        };
        consumed_stdin = true;
        (Box::new(ss), "(stdin)".to_string())
    } else {
        return Err(Error::NoInput);
    };

    // Only redirect fd 0 to /dev/tty if we actually drained stdin from a pipe.
    // For file inputs, stdin is already the user's terminal — replacing it with
    // a read-only /dev/tty fd would break crossterm's event source.
    #[cfg(unix)]
    if consumed_stdin {
        let _ = redirect_stdin_to_tty();
    }

    let sigterm = install_signal_flag();
    let _guard = TerminalGuard::enter()
        .map_err(|e| Error::Runtime(format!("terminal init: {}", e)))?;

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut viewport = Viewport::new(cols, rows, label);
    if args.line_numbers { viewport.toggle_line_numbers(); }
    if args.chop { viewport.toggle_chop(); }
    viewport.opts.tab_width = args.tab_width;
    viewport.set_follow_mode(args.follow);

    app::run(src, viewport, sigterm)?;
    Ok(())
}
