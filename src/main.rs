use std::io::{self, IsTerminal};
use std::process::ExitCode;

use std::path::PathBuf;

use tess::app::{self, RebuildSpec};
use tess::batch::{self, BatchDestination, BatchSpec};
use tess::cli::Args;
use tess::error::{Error, Result};
use tess::filter::{CompiledFilter, FilterSpec};
use tess::grep::GrepPredicate;
use tess::format;
use tess::line_index::LineIndex;
use tess::prettify::{self, PrettifyMode, ResolvedType};
use tess::source::{find_tail_offset, FileSource, LiveFileSource, MemorySource, MockSource, Source, StdinSource, TransformingSource};
use tess::terminal::{install_panic_hook, install_signal_flag, TerminalGuard};
use tess::viewport::Viewport;
use clap::Parser;

const MANUAL_TEXT: &str = include_str!("../MANUAL.md");

const EXAMPLES_TEXT: &str = "\
tess — usage examples
=====================

Plain viewing
-------------
  tess Cargo.toml                       # open a file
  tess -N -S src/main.rs                # line numbers, no wrap
  tess --tab-width 4 Makefile           # custom tab stops

Piped input
-----------
  git log | tess                        # page through git log
  cargo build 2>&1 | tess               # keep build output on screen
  ls --color=always | tess              # ANSI passes through

Big files: --head / --tail
--------------------------
  tess --head 50 schema.sql             # first 50 lines
  tess --tail 1000 huge.log             # last 1000 — opens instantly
  tess -f --tail 1000 huge.log          # tail-follow last 1000

Pretty-printing structured files
--------------------------------
  tess --prettify config.json           # detect from extension
  tess --prettify schema.yaml
  tess --prettify Cargo.toml
  tess --prettify page.html             # lenient HTML mode
  tess --prettify rows.csv              # column-aligned table
  tess --content-type=json data.bin     # force when extension lies
  # Inside the pager: Shift-P toggles, -Pj/y/t/x/h/c/a/r switches type.

Following live output
---------------------
  tess -f /var/log/syslog               # watch a log file (appends)
  tail -F /var/log/access.log | tess -f # streaming pipe with -f
  tess --live src/main.rs               # watch a file rewritten in place
  tess --live notes.md                  # follow saves from your editor / agent

Plain-text grep (no format needed)
----------------------------------
  tess --grep error access.log                        # plain regex filter, no format needed
  tess --grep error --grep '^\\[' access.log          # AND multiple --grep patterns

Apache log analysis (built-in formats)
--------------------------------------
  tess --format apache-combined --filter status~^5 access.log
  tess --format apache-combined --filter status~^5 --filter url~^/api/ access.log
  tess --format apache-combined --filter 'status!=200' access.log
  tess --format apache-combined --filter 'status>=500' access.log
  tess --format apache-combined --filter status~^5 --dim access.log
  tess -f --tail 100 --format apache-combined --filter status~^5 access.log
  tess --format apache-combined --filter status=500 --grep timeout access.log

Note: single-quote filters that use `!` or `<`/`>` — bash's history
expansion eats `!`, and `<`/`>` are I/O redirection without quotes.

Batch (non-interactive) output
------------------------------
  tess --filter status~^5 --format apache-combined -o errors.log access.log
  tess --head 1000 --stdout huge.log | grep -c something
  tess --prettify --stdout config.json > pretty.json
  tess -f --format app --filter level=ERROR -o /tmp/live-errors.log app.log

Display templates (reformat each line)
--------------------------------------
  tess --format apache-combined --display '[<status>] <method> <url>' access.log
  tess --format app --display '[<ts>] <level> <msg>' --filter 'level>=WARN' app.log
  tess --format apache-combined --display '<status>: <url>' \
       --filter 'status>=500' -o errors.log access.log

Or set the default per format in ~/.config/tess/formats.toml:
  # [format.app]
  # regex   = '...'
  # display = '[<ts>] <level> <msg>'

Custom format (declare in ~/.config/tess/formats.toml)
------------------------------------------------------
  # ~/.config/tess/formats.toml
  # [format.app]
  # regex = '^(?P<ts>\\S+ \\S+) (?P<level>\\w+) \\[(?P<reqid>[0-9a-f]+)\\] (?P<msg>.*)$'

  tess --list-formats                                  # confirm it loaded
  tess --format app --filter level=ERROR app.log
  tess --format app --filter 'level~^(ERROR|WARN)$' app.log
  tess -f --tail 200 --format app --filter level=ERROR app.log

Groups (shortcut bundles, also in formats.toml)
-----------------------------------------------
  # [group.errorlog]
  # format = \"app\"
  # file   = \"/var/log/myapp/app.log\"
  # follow = true
  # tail   = 1000
  # filter = [\"level=ERROR\"]

  tess --errorlog                       # expands to the full command above
  tess --errorlog 'msg~timeout'         # bare positional becomes --filter
  tess --errorlog --tail 50             # CLI flag overrides group's tail

Interactive keys (inside tess)
------------------------------
  / pat <Enter>     forward regex search       n / N    repeat search
  ? pat <Enter>     backward regex search      g / G    top / bottom
  Space / b         page down / up             Shift-F  toggle follow
  -N / -S / -F      toggle line numbers / chop / follow
  R                 force-reload from disk (with --live)
  q                 quit

See `tess --manual` for the full reference, or `tess --help` for a flag list.
";

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

/// Page an in-memory blob through tess itself. Used for `--manual` and
/// `--examples` when stdout is a TTY: the user gets scroll/search instead of
/// content scrolling off the top of their terminal.
/// Open a single source file using the same pipeline as startup:
/// preprocessor (if configured), live-mode wrapper (if --live).
///
/// Returns the boxed Source, the user-facing label, and any preprocess
/// stderr that should be surfaced in the status line.
///
/// Content-type detection and the prettify wrapper are handled by the
/// caller after this returns, since they can also apply to stdin.
fn open_source_for_path(
    path: &std::path::Path,
    args: &tess::cli::Args,
    preprocessor: Option<&tess::preprocess::Preprocessor>,
) -> Result<(Box<dyn Source>, String, Option<String>)> {
    let label = path.display().to_string();
    if args.live {
        let lfs = LiveFileSource::open(path).map_err(|source| {
            if let std::io::ErrorKind::InvalidInput = source.kind() {
                Error::NotAFile { path: path.to_path_buf() }
            } else {
                Error::OpenFile { path: path.to_path_buf(), source }
            }
        })?;
        return Ok((Box::new(lfs), label, None));
    }
    if let Some(p) = preprocessor {
        // Run the preprocessor. On success use MemorySource; on failure fall
        // back to the raw file and stash the error for the status line.
        match tess::preprocess::run(p, path) {
            tess::preprocess::PreprocessResult::Bytes(bytes) => {
                return Ok((Box::new(MemorySource::new(bytes)), label, None));
            }
            tess::preprocess::PreprocessResult::Failed { stderr } => {
                let fs = FileSource::open(path).map_err(|source| {
                    if let std::io::ErrorKind::InvalidInput = source.kind() {
                        Error::NotAFile { path: path.to_path_buf() }
                    } else {
                        Error::OpenFile { path: path.to_path_buf(), source }
                    }
                })?;
                return Ok((Box::new(fs), label, Some(stderr)));
            }
        }
    }
    let fs = FileSource::open(path).map_err(|source| {
        if let std::io::ErrorKind::InvalidInput = source.kind() {
            Error::NotAFile { path: path.to_path_buf() }
        } else {
            Error::OpenFile { path: path.to_path_buf(), source }
        }
    })?;
    Ok((Box::new(fs), label, None))
}

fn page_bytes(label: &str, content: &[u8]) -> Result<()> {
    let src = MockSource::new();
    src.append(content);
    src.finish();

    // We need keyboard input on fd 0. If the user piped something into us
    // (e.g. `cat x | tess --manual`), redirect fd 0 to /dev/tty first.
    #[cfg(unix)]
    if !io::stdin().is_terminal() {
        let _ = redirect_stdin_to_tty();
    }

    let sigterm = install_signal_flag();
    let _guard = TerminalGuard::enter()
        .map_err(|e| Error::Runtime(format!("terminal init: {}", e)))?;
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let viewport = Viewport::new(cols, rows, label.to_string());
    let idx = LineIndex::new();
    let keymap = tess::keys::KeyMap::load_from_default_path()
        .unwrap_or_else(|_| tess::keys::KeyMap::empty());
    app::run(Box::new(src), viewport, idx, sigterm, RebuildSpec::default(), keymap)?;
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

    // Info-only flags. When stdout is a TTY, page through tess itself so the
    // content doesn't fly past — the user gets scroll/search/quit. When stdout
    // is redirected (`tess --manual | grep …`, `> out.txt`), print plain text.
    if args.manual {
        if io::stdout().is_terminal() {
            return page_bytes("(manual)", MANUAL_TEXT.as_bytes());
        }
        print!("{}", MANUAL_TEXT);
        return Ok(());
    }
    if args.examples {
        if io::stdout().is_terminal() {
            return page_bytes("(examples)", EXAMPLES_TEXT.as_bytes());
        }
        print!("{}", EXAMPLES_TEXT);
        return Ok(());
    }
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
    if args.display.is_some() && args.format.is_none() {
        return Err(Error::Runtime(
            "--display requires --format".to_string(),
        ));
    }
    if args.dim && args.filter.is_empty() && args.grep.is_empty() {
        return Err(Error::Runtime(
            "--dim has no effect without --filter or --grep".to_string(),
        ));
    }
    if args.live && args.files.is_empty() {
        return Err(Error::Runtime(
            "--live requires a file path (stdin can't be re-stat'd)".to_string(),
        ));
    }

    // Batch (`--output` / `--stdout`) is incompatible with `--live`: live mode
    // is "watch a file rewrite, render the new view" — there's no view to
    // render in batch.
    let batch_destination: Option<BatchDestination> = if args.stdout {
        Some(BatchDestination::Stdout)
    } else if let Some(path) = args.output.as_deref() {
        if path == "-" { Some(BatchDestination::Stdout) }
        else { Some(BatchDestination::File(PathBuf::from(path))) }
    } else {
        None
    };
    if batch_destination.is_some() && args.live {
        return Err(Error::Runtime(
            "--output / --stdout is not compatible with --live".to_string(),
        ));
    }

    // Resolve --content-type now (parse + validation) so errors land cleanly.
    let explicit_content_type: Option<PrettifyMode> = match args.content_type.as_deref() {
        Some(name) => prettify::parse_content_type(name).map_err(Error::Runtime)?,
        None => None,
    };
    // Setting --content-type to a concrete (non-raw, non-auto) type implies
    // --prettify is on. `raw` explicitly disables prettify even if --prettify
    // is also passed.
    let want_prettify = match explicit_content_type {
        Some(PrettifyMode::Off) => false,
        Some(_) => true,
        None => args.prettify,
    };
    if want_prettify {
        if args.follow {
            return Err(Error::Runtime(
                "--prettify is not supported with --follow (streaming partial \
documents can't be parsed)".to_string(),
            ));
        }
        if args.live {
            return Err(Error::Runtime(
                "--prettify is not supported with --live".to_string(),
            ));
        }
        if !args.filter.is_empty() {
            return Err(Error::Runtime(
                "--prettify is not supported with --filter".to_string(),
            ));
        }
        if !args.grep.is_empty() {
            return Err(Error::Runtime(
                "--prettify is not supported with --grep".to_string(),
            ));
        }
        if args.display.is_some() {
            return Err(Error::Runtime(
                "--prettify is not supported with --display".to_string(),
            ));
        }
    }

    // Resolve the preprocessor from --preprocess or $LESSOPEN (pipe-mode only).
    // --no-preprocess suppresses both. Stdin sources skip preprocessing entirely.
    let preprocessor: Option<tess::preprocess::Preprocessor> = if args.no_preprocess {
        None
    } else {
        let raw = args.preprocess.clone()
            .or_else(|| std::env::var("LESSOPEN").ok());
        match raw {
            Some(r) => Some(tess::preprocess::Preprocessor::parse(&r)
                .map_err(Error::Runtime)?),
            None => None,
        }
    };

    // Resolve source. Track whether we actually consumed stdin — only then
    // do we need to redirect fd 0 to /dev/tty for keyboard input. Also track
    // whether `--tail` is meaningful for this source (streaming stdin can't
    // do random-access tail).
    let mut consumed_stdin = false;
    let mut source_supports_tail = true;
    let mut preprocess_failure: Option<String> = None;
    let (src, label): (Box<dyn Source>, String) = match args.files.first() {
        Some(path) => {
            if args.files.len() > 1 {
                eprintln!(
                    "tess: ignoring {} additional file(s) (multi-file navigation not yet supported)",
                    args.files.len() - 1
                );
            }
            let (s, l, pf) = open_source_for_path(path, &args, preprocessor.as_ref())?;
            preprocess_failure = pf;
            (s, l)
        }
        None if !io::stdin().is_terminal() => {
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
        }
        None => {
            return Err(Error::NoInput);
        }
    };

    // If the user wants prettification, resolve the mode against the inner
    // source's first bytes + the path (if any) and wrap the source. Failure
    // to detect under `--prettify` (no --content-type given) is a soft fall:
    // print a stderr note and proceed with the raw view.
    let (src, prettify_label): (Box<dyn Source>, Option<String>) = if want_prettify {
        let head = src.bytes(0..src.len().min(512)).to_vec();
        let path_for_detect = args.files.first().map(|p| p.as_path());
        let resolved = prettify::resolve(explicit_content_type, path_for_detect, &head);
        match resolved {
            ResolvedType::Mode(PrettifyMode::Off) => (src, None),
            ResolvedType::Mode(mode) => {
                let label = mode.label().to_string();
                let wrapped = TransformingSource::wrap(src, mode);
                if let Some(err) = wrapped.last_error() {
                    eprintln!("tess: prettify ({label}) failed: {err} \u{2014} showing raw");
                    (Box::new(wrapped), Some(format!("{label}:err")))
                } else {
                    (Box::new(wrapped), Some(label))
                }
            }
            ResolvedType::Undetected => {
                eprintln!(
                    "tess: --prettify requested but content type could not be detected; \
showing raw (use --content-type=NAME to override)"
                );
                (src, None)
            }
        }
    } else {
        (src, None)
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

    // Resolve --record-start: CLI flag takes priority; fall back to the active
    // format's record_start (if a --format was given and defines one).
    // Must be set BEFORE any idx.extend_* call.
    {
        let fmt_record_start: Option<String> = if let Some(name) = args.format.as_deref() {
            let formats = format::load_all().map_err(Error::Runtime)?;
            formats.get(name).and_then(|f| {
                f.record_start.as_ref().map(|r| r.as_str().to_string())
            })
        } else {
            None
        };
        let record_start_pattern: Option<String> = args.record_start
            .clone()
            .or(fmt_record_start);
        if let Some(pat) = record_start_pattern {
            let re = regex::bytes::Regex::new(&pat)
                .map_err(|e| Error::Runtime(format!("--record-start: {e}")))?;
            idx.set_record_start(re);
        }
    }

    // Only redirect fd 0 to /dev/tty if we actually drained stdin from a pipe.
    // For file inputs, stdin is already the user's terminal — replacing it with
    // a read-only /dev/tty fd would break crossterm's event source.
    #[cfg(unix)]
    if consumed_stdin {
        let _ = redirect_stdin_to_tty();
    }

    // Compile --grep patterns up front (no --format required). A failing
    // pattern errors cleanly to stderr without entering raw mode.
    let compiled_grep = if !args.grep.is_empty() {
        Some(
            GrepPredicate::compile(&args.grep)
                .map_err(Error::Runtime)?,
        )
    } else {
        None
    };

    // Compile filter specs and resolve the display template against the chosen
    // format BEFORE entering raw mode so errors print cleanly. The
    // `DisplayRenderer` bundles the (CLI-overridable) template with the
    // format's regex so rendering is a single call later.
    let (compiled_filter, display_renderer) = if let Some(name) = args.format.as_deref() {
        let formats = format::load_all().map_err(Error::Runtime)?;
        let fmt = formats.get(name).ok_or_else(|| {
            Error::Runtime(format!(
                "unknown format `{name}` (run --list-formats to see available)"
            ))
        })?;
        let filter = if !args.filter.is_empty() {
            let specs: Vec<FilterSpec> = args.filter.iter()
                .map(|s| FilterSpec::parse(s).map_err(Error::Runtime))
                .collect::<Result<_>>()?;
            Some(CompiledFilter::compile(fmt, specs).map_err(Error::Runtime)?)
        } else {
            None
        };
        // CLI --display overrides the format's default; otherwise use the
        // format's default (if any).
        let template = if let Some(cli_tmpl) = args.display.as_deref() {
            Some(
                format::DisplayTemplate::compile(cli_tmpl, &fmt.field_names)
                    .map_err(|e| Error::Runtime(format!("--display: {e}")))?,
            )
        } else {
            fmt.display.clone()
        };
        let renderer = template.map(|t| format::DisplayRenderer::new(t, fmt.regex.clone()));
        (filter, renderer)
    } else {
        (None, None)
    };

    let sigterm = install_signal_flag();

    // Batch mode: skip the terminal guard entirely and route through
    // `batch::run` instead of the interactive event loop.
    if let Some(destination) = batch_destination {
        let _ = prettify_label; // batch keeps the prettified bytes; label unused
        let spec = BatchSpec {
            destination,
            follow: args.follow,
            poll_interval: std::time::Duration::from_millis(250),
        };
        return batch::run(src, idx, compiled_filter, compiled_grep, display_renderer, spec, sigterm);
    }

    let _guard = TerminalGuard::enter()
        .map_err(|e| Error::Runtime(format!("terminal init: {}", e)))?;

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut viewport = Viewport::new(cols, rows, label);
    if args.line_numbers { viewport.toggle_line_numbers(); }
    if args.chop { viewport.toggle_chop(); }
    viewport.opts.tab_width = args.tab_width;
    viewport.set_follow_mode(args.follow);
    viewport.set_live_mode(args.live);
    viewport.set_prettify_label(prettify_label);
    if let Some(f) = compiled_filter {
        viewport.set_filter(Some(f));
    }
    if let Some(g) = compiled_grep {
        viewport.set_grep(Some(g));
    }
    if args.dim {
        viewport.set_dim_mode(true);
    }
    if let Some(d) = display_renderer {
        viewport.set_display(Some(d));
    }
    if args.hex {
        viewport.set_hex_mode(true);
    }
    viewport.set_preprocess_failure(preprocess_failure);

    // Resolve --prompt: CLI flag takes priority; fall back to the active
    // format's prompt (if a --format was given and defines one).
    {
        let fmt_prompt: Option<tess::prompt::ParsedPrompt> = if let Some(name) = args.format.as_deref() {
            let formats = format::load_all().map_err(Error::Runtime)?;
            formats.get(name).and_then(|f| f.prompt.clone())
        } else {
            None
        };
        let prompt_template: Option<tess::prompt::ParsedPrompt> = match args.prompt.as_deref() {
            Some(s) => Some(tess::prompt::ParsedPrompt::parse(s)
                .map_err(|e| Error::Runtime(format!("--prompt: {e}")))?),
            None => fmt_prompt,
        };
        viewport.set_prompt(prompt_template);
    }
    viewport.set_format_label(args.format.clone());

    let rebuild_spec = RebuildSpec {
        head: args.head,
        tail: if source_supports_tail { args.tail } else { None },
    };
    let keymap = tess::keys::KeyMap::load_from_default_path()
        .map_err(Error::Runtime)?;
    app::run(src, viewport, idx, sigterm, rebuild_spec, keymap)?;
    Ok(())
}
