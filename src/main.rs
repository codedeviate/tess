use std::io::{self, IsTerminal};
use std::process::ExitCode;

use rustless::app;
use rustless::cli::Args;
use rustless::error::{Error, Result};
use rustless::source::{FileSource, Source, StdinSource};
use rustless::terminal::{install_panic_hook, install_signal_flag, TerminalGuard};
use rustless::viewport::Viewport;
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

fn real_main() -> Result<()> {
    let args = Args::parse();

    // Resolve source.
    let (src, label): (Box<dyn Source>, String) = if let Some(path) = args.files.first() {
        if args.files.len() > 1 {
            eprintln!(
                "rustless: ignoring {} additional file(s) (multi-file navigation not yet supported)",
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
        (Box::new(StdinSource::spawn()), "(stdin)".to_string())
    } else {
        return Err(Error::NoInput);
    };

    let sigterm = install_signal_flag();
    let _guard = TerminalGuard::enter()
        .map_err(|e| Error::Runtime(format!("terminal init: {}", e)))?;

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut viewport = Viewport::new(cols, rows, label);
    if args.line_numbers { viewport.toggle_line_numbers(); }
    if args.chop { viewport.toggle_chop(); }
    viewport.opts.tab_width = args.tab_width;

    app::run(src, viewport, sigterm)?;
    Ok(())
}
