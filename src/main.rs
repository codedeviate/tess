use std::io::{self, IsTerminal};
use std::process::ExitCode;

use tess::app;
use tess::cli::Args;
use tess::error::{Error, Result};
use tess::filter::{CompiledFilter, FilterSpec};
use tess::format;
use tess::line_index::LineIndex;
use tess::source::{find_tail_offset, FileSource, Source, StdinSource};
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
    // Expand any user-defined groups (`[group.X]` in formats.toml) before clap
    // parses. A `--<groupname>` token becomes the group's flags inline, and
    // remaining bare positionals become `--filter <arg>` pairs.
    let groups = format::load_groups().map_err(Error::Runtime)?;
    let argv: Vec<String> = std::env::args().collect();
    let argv = format::expand_argv(argv, &groups);
    let args = Args::parse_from(argv);

    // --list-formats: print and exit before doing any source/terminal work.
    if args.list_formats {
        let formats = format::load_all().map_err(Error::Runtime)?;
        format::print_format_list(&formats);
        return Ok(());
    }

    // Validate format/filter combination up front so errors land cleanly to
    // stderr without entering raw mode.
    if !args.filter.is_empty() && args.format.is_none() {
        return Err(Error::Runtime(
            "--filter requires --format".to_string(),
        ));
    }
    if args.dim && args.filter.is_empty() {
        return Err(Error::Runtime(
            "--dim has no effect without --filter".to_string(),
        ));
    }

    // Resolve source. Track whether we actually consumed stdin — only then
    // do we need to redirect fd 0 to /dev/tty for keyboard input. Also track
    // whether `--tail` is meaningful for this source (streaming stdin can't
    // do random-access tail).
    let mut consumed_stdin = false;
    let mut source_supports_tail = true;
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
            source_supports_tail = false;
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

    // Apply --tail by computing a starting byte offset for the LineIndex.
    // Streaming stdin (with -f) can't do this — bytes arrive over time.
    let mut idx = match args.tail {
        Some(n) if source_supports_tail => {
            let off = find_tail_offset(src.as_ref(), n);
            LineIndex::new_starting_at(off)
        }
        Some(_) => {
            eprintln!("tess: --tail is not supported on streaming stdin (-f); ignoring");
            LineIndex::new()
        }
        None => LineIndex::new(),
    };
    if let Some(n) = args.head {
        idx.set_head_cap(n);
    }

    // Only redirect fd 0 to /dev/tty if we actually drained stdin from a pipe.
    // For file inputs, stdin is already the user's terminal — replacing it with
    // a read-only /dev/tty fd would break crossterm's event source.
    #[cfg(unix)]
    if consumed_stdin {
        let _ = redirect_stdin_to_tty();
    }

    // Compile filter specs against the chosen format BEFORE entering raw mode
    // so errors print cleanly.
    let compiled_filter = if let Some(name) = args.format.as_deref() {
        let formats = format::load_all().map_err(Error::Runtime)?;
        let fmt = formats.get(name).ok_or_else(|| {
            Error::Runtime(format!(
                "unknown format `{name}` (run --list-formats to see available)"
            ))
        })?;
        if !args.filter.is_empty() {
            let specs: Vec<FilterSpec> = args.filter.iter()
                .map(|s| FilterSpec::parse(s).map_err(Error::Runtime))
                .collect::<Result<_>>()?;
            Some(CompiledFilter::compile(fmt, specs).map_err(Error::Runtime)?)
        } else {
            None
        }
    } else {
        None
    };

    let sigterm = install_signal_flag();
    let _guard = TerminalGuard::enter()
        .map_err(|e| Error::Runtime(format!("terminal init: {}", e)))?;

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut viewport = Viewport::new(cols, rows, label);
    if args.line_numbers { viewport.toggle_line_numbers(); }
    if args.chop { viewport.toggle_chop(); }
    viewport.opts.tab_width = args.tab_width;
    viewport.set_follow_mode(args.follow);
    if let Some(f) = compiled_filter {
        viewport.set_filter(Some(f));
        viewport.set_dim_mode(args.dim);
    }

    app::run(src, viewport, idx, sigterm)?;
    Ok(())
}
