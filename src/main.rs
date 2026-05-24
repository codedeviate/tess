use std::fmt::Write as FmtWrite;
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
use tess::source::{find_tail_offset, MockSource, Source, StdinSource, TransformingSource};
use tess::terminal::{install_panic_hook, install_signal_flag, TerminalGuard};
use tess::viewport::Viewport;
use clap::Parser;

const MANUAL_TEXT: &str = include_str!("../MANUAL.md");

use colored::Colorize;

fn examples_section(buf: &mut String, title: &str) {
    let _ = writeln!(buf, "  {}", title.yellow().bold());
    let _ = writeln!(buf);
}

fn examples_example(buf: &mut String, desc: &str, commands: &[&str]) {
    let _ = writeln!(buf, "    {}", desc.bold());
    for cmd in commands {
        let _ = writeln!(buf, "      {}", cmd.cyan());
    }
    let _ = writeln!(buf);
}

fn examples_note(buf: &mut String, text: &str) {
    let _ = writeln!(buf, "    {} {}", "note:".dimmed().bold(), text.dimmed());
    let _ = writeln!(buf);
}

fn build_examples_text() -> String {
    let mut buf = String::new();
    let _ = writeln!(buf);
    let _ = writeln!(buf, "{}", "tess — usage examples".bold());
    let _ = writeln!(buf);

    examples_section(&mut buf, "Plain viewing");
    examples_example(&mut buf, "Open a file", &[
        "tess Cargo.toml",
        "tess -N -S src/main.rs",
        "tess --tab-width 4 Makefile",
    ]);

    examples_section(&mut buf, "Piped input");
    examples_example(&mut buf, "Pipe output through tess", &[
        "git log | tess",
        "cargo build 2>&1 | tess",
        "ls --color=always | tess",
    ]);

    examples_section(&mut buf, "Big files: --head / --tail");
    examples_example(&mut buf, "Cheap views of large files", &[
        "tess --head 50 schema.sql",
        "tess --tail 1000 huge.log",
        "tess -f --tail 1000 huge.log",
    ]);

    examples_section(&mut buf, "Pretty-printing structured files");
    examples_example(&mut buf, "Auto-detect from extension or force a content type", &[
        "tess --prettify config.json",
        "tess --prettify schema.yaml",
        "tess --prettify Cargo.toml",
        "tess --prettify page.html",
        "tess --prettify rows.csv",
        "tess --content-type=json data.bin",
    ]);
    examples_note(&mut buf, "Inside the pager: Shift-P toggles, -Pj/y/t/x/h/c/a/r switches type.");

    examples_section(&mut buf, "Following live output");
    examples_example(&mut buf, "Watch a log file or a file rewritten in place", &[
        "tess -f /var/log/syslog",
        "tail -F /var/log/access.log | tess -f",
        "tess --live src/main.rs",
        "tess --live notes.md",
    ]);

    examples_section(&mut buf, "Plain-text grep (no format needed)");
    examples_example(&mut buf, "Filter lines by regex — no format required", &[
        r"tess --grep error access.log",
        r"tess --grep error --grep '^\[' access.log",
    ]);

    examples_section(&mut buf, "Apache log analysis (built-in formats)");
    examples_example(&mut buf, "Filter by status code, URL, or combine grep and filter", &[
        "tess --format apache-combined --filter status~^5 access.log",
        "tess --format apache-combined --filter status~^5 --filter url~^/api/ access.log",
        "tess --format apache-combined --filter 'status!=200' access.log",
        "tess --format apache-combined --filter 'status>=500' access.log",
        "tess --format apache-combined --filter status~^5 --dim access.log",
        "tess -f --tail 100 --format apache-combined --filter status~^5 access.log",
        "tess --format apache-combined --filter status=500 --grep timeout access.log",
    ]);
    examples_note(&mut buf, "Single-quote filters that use `!` or `<`/`>` — bash's history expansion eats `!`, and `<`/`>` are I/O redirection without quotes.");

    examples_section(&mut buf, "Batch (non-interactive) output");
    examples_example(&mut buf, "Write filtered output to a file or stdout", &[
        "tess --filter status~^5 --format apache-combined -o errors.log access.log",
        "tess --head 1000 --stdout huge.log | grep -c something",
        "tess --prettify --stdout config.json > pretty.json",
        "tess -f --format app --filter level=ERROR -o /tmp/live-errors.log app.log",
    ]);

    examples_section(&mut buf, "Display templates (reformat each line)");
    examples_example(&mut buf, "Reformat matched lines with a custom template", &[
        "tess --format apache-combined --display '[<status>] <method> <url>' access.log",
        "tess --format app --display '[<ts>] <level> <msg>' --filter 'level>=WARN' app.log",
        r"tess --format apache-combined --display '<status>: <url>' \",
        r"     --filter 'status>=500' -o errors.log access.log",
    ]);
    examples_note(&mut buf, "Or set the default per format: add `display = '[<ts>] <level> <msg>'` under `[format.app]` in ~/.config/tess/formats.toml.");

    examples_section(&mut buf, "Custom format (declare in ~/.config/tess/formats.toml)");
    examples_example(&mut buf, "Define a named format with a regex and optional display template", &[
        r"# [format.app]",
        r"# regex = '^(?P<ts>\S+ \S+) (?P<level>\w+) \[(?P<reqid>[0-9a-f]+)\] (?P<msg>.*)$'",
        "",
        "tess --list-formats",
        "tess --format app --filter level=ERROR app.log",
        "tess --format app --filter 'level~^(ERROR|WARN)$' app.log",
        "tess -f --tail 200 --format app --filter level=ERROR app.log",
    ]);

    examples_section(&mut buf, "Groups (shortcut bundles, also in formats.toml)");
    examples_example(&mut buf, "Expand --<groupname> into a fixed flag bundle", &[
        "# [group.errorlog]",
        r#"# format = "app""#,
        r#"# file   = "/var/log/myapp/app.log""#,
        "# follow = true",
        "# tail   = 1000",
        r#"# filter = ["level=ERROR"]"#,
        "",
        "tess --errorlog",
        "tess --errorlog 'msg~timeout'",
        "tess --errorlog --tail 50",
    ]);

    examples_section(&mut buf, "Interactive keys (inside tess)");
    examples_example(&mut buf, "Search, scroll, and toggle display options", &[
        "/ pat <Enter>     forward regex search       n / N    repeat search",
        "? pat <Enter>     backward regex search      g / G    top / bottom",
        "Space / b         page down / up             Shift-F  toggle follow",
        "-N / -S / -F      toggle line numbers / chop / follow",
        "R                 force-reload from disk (with --live)",
        "q                 quit",
    ]);

    let _ = writeln!(buf, "  {}", "See `tess --manual` for the full reference, or `tess --help` for a flag list.".dimmed());
    let _ = writeln!(buf);

    buf
}


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

fn resolve_ansi_mode(args: &Args) -> tess::render::AnsiMode {
    use tess::render::AnsiMode;
    if args.raw_control_chars {
        return AnsiMode::Raw;
    }
    if args.no_color {
        return AnsiMode::Strict;
    }
    if let Ok(v) = std::env::var("NO_COLOR") {
        if !v.is_empty() {
            return AnsiMode::Strict;
        }
    }
    if std::env::var("CLICOLOR").as_deref() == Ok("0") {
        return AnsiMode::Strict;
    }
    AnsiMode::Interpret
}

fn resolve_truecolor(args: &Args) -> std::result::Result<bool, String> {
    use tess::render::TrueColor;
    match args.truecolor.as_str() {
        "always" => Ok(true),
        "never" => Ok(false),
        "auto" => Ok(TrueColor::Auto.resolve()),
        other => Err(format!(
            "--truecolor: unknown mode `{other}` (expected auto, always, never)"
        )),
    }
}

fn page_bytes(label: &str, content: &[u8], ansi_mode: tess::render::AnsiMode) -> Result<()> {
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
    // page_bytes is the internal helper used for --manual / --examples;
    // it always enters the alt-screen so the help-style content paints
    // over the user's shell rather than appending to it.
    let _guard = TerminalGuard::enter(false, true)
        .map_err(|e| Error::Runtime(format!("terminal init: {}", e)))?;
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut viewport = Viewport::new(cols, rows, label.to_string());
    viewport.set_ansi_mode(ansi_mode);
    let idx = LineIndex::new();
    let keymap = tess::keys::KeyMap::load_from_default_path()
        .unwrap_or_else(|_| tess::keys::KeyMap::empty());
    let file_set = tess::file_set::FileSet::new(vec![std::path::PathBuf::from(label)]);
    let stub_args = Args::parse_from(["tess"]);
    app::run(Box::new(src), viewport, idx, sigterm, RebuildSpec::default(), keymap, file_set, None, stub_args, None, None)?;
    Ok(())
}

/// less-style `+CMD` startup command parsed off argv before clap sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PlusCmd {
    /// `+G` → jump to bottom on startup.
    GotoBottom,
    /// `+NUM` → jump to 1-indexed line NUM on startup.
    Goto(usize),
    /// `+/pattern` → forward search; jump to first match.
    SearchForward(String),
    /// `+?pattern` → backward search; jump to first prior match.
    SearchBackward(String),
}

fn parse_plus_cmd(s: &str) -> std::result::Result<PlusCmd, String> {
    debug_assert!(s.starts_with('+') && s.len() > 1);
    let rest = &s[1..];
    if rest == "G" {
        return Ok(PlusCmd::GotoBottom);
    }
    if let Some(p) = rest.strip_prefix('/') {
        return Ok(PlusCmd::SearchForward(p.to_string()));
    }
    if let Some(p) = rest.strip_prefix('?') {
        return Ok(PlusCmd::SearchBackward(p.to_string()));
    }
    if let Ok(n) = rest.parse::<usize>() {
        return Ok(PlusCmd::Goto(n));
    }
    Err(format!(
        "unrecognized startup command `{s}` (expected +G, +N, +/pat, or +?pat)"
    ))
}

fn real_main() -> Result<()> {
    // Extract `+CMD` startup tokens before clap sees the argv — clap doesn't
    // recognize the `+` prefix natively. Order is preserved so multiple
    // `+CMD`s apply in argv order against the viewport just before the
    // event loop starts.
    let raw_argv: Vec<String> = std::env::args().collect();
    let plus_cmds: Vec<String> = raw_argv
        .iter()
        .skip(1)
        .filter(|a| a.starts_with('+') && a.len() > 1)
        .cloned()
        .collect();
    let cleaned_argv: Vec<String> = raw_argv
        .into_iter()
        .enumerate()
        .filter(|(i, a)| *i == 0 || !(a.starts_with('+') && a.len() > 1))
        .map(|(_, a)| a)
        .collect();

    // Expand any user-defined groups (`[group.X]` in formats.toml) before clap
    // parses. A `--<groupname>` token becomes the group's flags inline, and
    // remaining bare positionals become `--filter <arg>` pairs.
    let groups = format::load_groups().map_err(Error::Runtime)?;
    let argv = format::expand_argv(cleaned_argv, &groups);
    let args = Args::parse_from(argv);

    // Parse +CMD tokens up front so a typo fails before raw-mode entry.
    let parsed_plus_cmds: Vec<PlusCmd> = plus_cmds
        .iter()
        .map(|s| parse_plus_cmd(s).map_err(Error::Runtime))
        .collect::<Result<Vec<_>>>()?;

    // Info-only flags. When stdout is a TTY, page through tess itself so the
    // content doesn't fly past — the user gets scroll/search/quit. When stdout
    // is redirected (`tess --manual | grep …`, `> out.txt`), print plain text.
    let ansi_mode = resolve_ansi_mode(&args);
    // Validate --truecolor early; the resolved bool is consumed inside app::run.
    resolve_truecolor(&args).map_err(Error::Runtime)?;
    if args.manual {
        if io::stdout().is_terminal() {
            return page_bytes("(manual)", MANUAL_TEXT.as_bytes(), ansi_mode);
        }
        print!("{}", MANUAL_TEXT);
        return Ok(());
    }
    if args.examples {
        let is_tty = io::stdout().is_terminal();
        colored::control::set_override(is_tty);
        let text = build_examples_text();
        if is_tty {
            return page_bytes("(examples)", text.as_bytes(), ansi_mode);
        }
        print!("{}", text);
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
    let file_set = tess::file_set::FileSet::new(args.files.clone());
    let mut consumed_stdin = false;
    let mut source_supports_tail = true;
    let mut preprocess_failure: Option<String> = None;
    let (src, label): (Box<dyn Source>, String) = match args.files.first() {
        Some(path) => {
            let (s, l, pf) = tess::open::open_source_for_path(path, &args, preprocessor.as_ref())?;
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
    let record_start_regex: Option<regex::bytes::Regex> = {
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
            idx.set_record_start(re.clone());
            Some(re)
        } else {
            None
        }
    };

    // Only redirect fd 0 to /dev/tty if we actually drained stdin from a pipe.
    // For file inputs, stdin is already the user's terminal — replacing it with
    // a read-only /dev/tty fd would break crossterm's event source.
    #[cfg(unix)]
    if consumed_stdin {
        let _ = redirect_stdin_to_tty();
    }

    // Compile --grep patterns up front (no --format required). A failing
    // pattern errors cleanly to stderr without entering raw mode.
    //
    // Resolve case policy from -I / -i. `-I` wins over `-i` (clap also
    // enforces mutual exclusion); both unset → sensitive.
    let case_mode = if args.IGNORE_CASE {
        tess::viewport::CaseMode::Insensitive
    } else if args.ignore_case {
        tess::viewport::CaseMode::Smart
    } else {
        tess::viewport::CaseMode::Sensitive
    };
    let compiled_grep = if !args.grep.is_empty() {
        Some(
            GrepPredicate::compile(&args.grep, case_mode)
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
            Some(CompiledFilter::compile(fmt, specs, case_mode).map_err(Error::Runtime)?)
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

    // `-F` / `--quit-if-one-screen`: if the entire source fits in one
    // screen, write it to stdout and exit — no pager. Skipped when the
    // source can still grow (follow on piped stdin) since "one screen"
    // is meaningless for an open producer.
    if args.quit_if_one_screen && !args.follow && src.is_complete() {
        let (_cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
        let body_rows = rows.saturating_sub(1) as usize;
        let total_len = src.len();
        let bytes = src.bytes(0..total_len);
        let trailing_nl = bytes.last().map_or(true, |&b| b == b'\n');
        let line_count = bytes.iter().filter(|&&b| b == b'\n').count()
            + if trailing_nl { 0 } else { 1 };
        if line_count <= body_rows {
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(&bytes).map_err(|e| Error::Runtime(format!("stdout: {e}")))?;
            if !trailing_nl {
                let _ = stdout.write_all(b"\n");
            }
            return Ok(());
        }
    }

    let _guard = TerminalGuard::enter(args.mouse, !args.no_init)
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
        let bpg = tess::hex::hex_chars_to_bytes_per_group(args.hex_group)
            .ok_or_else(|| Error::Runtime(format!(
                "--hex-group must be one of 2, 4, 8, 16, 32 (got {})",
                args.hex_group
            )))?;
        viewport.set_hex_group_size(bpg);
    }
    viewport.set_ansi_mode(ansi_mode);
    viewport.set_case_mode(case_mode);
    viewport.set_hilite_search(!args.no_hilite_search);
    let qae = if args.QUIT_AT_EOF {
        tess::viewport::QuitAtEof::First
    } else if args.quit_at_eof {
        tess::viewport::QuitAtEof::Second
    } else {
        tess::viewport::QuitAtEof::Off
    };
    viewport.set_quit_at_eof(qae);
    viewport.set_preprocess_failure(preprocess_failure);

    // Resolve --prompt: CLI flag takes priority; fall back to the active
    // format's prompt (if a --format was given and defines one).
    {
        let (fmt_prompt, fmt_prompt_style): (Option<tess::prompt::ParsedPrompt>, Option<tess::ansi::Style>) =
            if let Some(name) = args.format.as_deref() {
                let formats = format::load_all().map_err(Error::Runtime)?;
                let entry = formats.get(name);
                (
                    entry.and_then(|f| f.prompt.clone()),
                    entry.and_then(|f| f.prompt_style),
                )
            } else {
                (None, None)
            };
        let prompt_template: Option<tess::prompt::ParsedPrompt> = match args.prompt.as_deref() {
            Some(s) => Some(tess::prompt::ParsedPrompt::parse(s)
                .map_err(|e| Error::Runtime(format!("--prompt: {e}")))?),
            None => fmt_prompt,
        };
        let prompt_active = prompt_template.is_some();
        viewport.set_prompt(prompt_template);

        // Resolve status / prompt theming once at startup.
        // - `--status-style` is the base (default `reverse`).
        // - When a custom prompt is active, `--prompt-style` (if non-empty)
        //   wins, otherwise per-format `prompt_style` wins, otherwise
        //   `--status-style` is used.
        let status_style_base = tess::style_spec::parse(&args.status_style)
            .map_err(|e| Error::Runtime(format!("--status-style: {e}")))?;
        let cli_prompt_style = if args.prompt_style.trim().is_empty() {
            None
        } else {
            Some(tess::style_spec::parse(&args.prompt_style)
                .map_err(|e| Error::Runtime(format!("--prompt-style: {e}")))?)
        };
        let resolved_status_style = if prompt_active {
            cli_prompt_style
                .or(fmt_prompt_style)
                .unwrap_or(status_style_base)
        } else {
            status_style_base
        };
        viewport.set_status_style(resolved_status_style);
    }
    viewport.set_format_label(args.format.clone());
    viewport.set_file_index(0, file_set.len());

    let rebuild_spec = RebuildSpec {
        head: args.head,
        tail: if source_supports_tail { args.tail } else { None },
    };
    let keymap = tess::keys::KeyMap::load_from_default_path()
        .map_err(Error::Runtime)?;

    let tag_file: Option<tess::tags::TagFile> = if let Some(path) = &args.tag_file {
        match tess::tags::TagFile::load(path) {
            Ok(tf) => Some(tf),
            Err(e) => {
                eprintln!("tess: {e}");
                std::process::exit(1);
            }
        }
    } else {
        let start = args
            .files
            .first()
            .map(|p| p.as_path())
            .unwrap_or_else(|| std::path::Path::new("."));
        if let Some(found) = tess::tags::TagFile::find_walking_up(start) {
            tess::tags::TagFile::load(&found).ok()
        } else {
            None
        }
    };

    if args.tag.is_some() && tag_file.is_none() {
        eprintln!("tess: tags file not found (required by -t)");
        std::process::exit(1);
    }

    // Apply `+CMD` startup commands against the live viewport before the
    // event loop spins up. Search compiles use the resolved case mode so
    // `+/foo` honors `-i` / `-I`.
    for cmd in &parsed_plus_cmds {
        match cmd {
            PlusCmd::Goto(n) if *n > 0 => viewport.goto_line(n - 1, src.as_ref(), &mut idx),
            PlusCmd::Goto(_) => viewport.goto_top(),
            PlusCmd::GotoBottom => viewport.goto_bottom(src.as_ref(), &mut idx),
            PlusCmd::SearchForward(p) => {
                viewport
                    .set_search(p.clone(), tess::viewport::SearchDirection::Forward)
                    .map_err(|e| Error::Runtime(format!("+/{p}: {e}")))?;
                viewport.search_repeat(src.as_ref(), &mut idx, false);
            }
            PlusCmd::SearchBackward(p) => {
                viewport
                    .set_search(p.clone(), tess::viewport::SearchDirection::Backward)
                    .map_err(|e| Error::Runtime(format!("+?{p}: {e}")))?;
                viewport.search_repeat(src.as_ref(), &mut idx, false);
            }
        }
    }

    app::run(src, viewport, idx, sigterm, rebuild_spec, keymap, file_set, record_start_regex, args, preprocessor, tag_file)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plus_g_is_goto_bottom() {
        assert_eq!(parse_plus_cmd("+G"), Ok(PlusCmd::GotoBottom));
    }

    #[test]
    fn parse_plus_num_is_goto() {
        assert_eq!(parse_plus_cmd("+42"), Ok(PlusCmd::Goto(42)));
        assert_eq!(parse_plus_cmd("+1"), Ok(PlusCmd::Goto(1)));
    }

    #[test]
    fn parse_plus_slash_is_search_forward() {
        assert_eq!(
            parse_plus_cmd("+/error"),
            Ok(PlusCmd::SearchForward("error".into()))
        );
    }

    #[test]
    fn parse_plus_question_is_search_backward() {
        assert_eq!(
            parse_plus_cmd("+?warning"),
            Ok(PlusCmd::SearchBackward("warning".into()))
        );
    }

    #[test]
    fn parse_plus_unknown_errors() {
        assert!(parse_plus_cmd("+xyzzy").is_err());
        assert!(parse_plus_cmd("+abc").is_err());
    }

    #[test]
    fn parse_plus_empty_pattern_still_parses() {
        // `+/` with empty pattern is structurally a search forward;
        // the regex compile would catch the empty pattern downstream.
        assert_eq!(
            parse_plus_cmd("+/"),
            Ok(PlusCmd::SearchForward("".into()))
        );
    }
}
