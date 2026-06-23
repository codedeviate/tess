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

/// Resolved predicates for a single pane: compiled grep, filter, display
/// renderer, record-start regex, and format label. Used to seed pane B's
/// `--right-*` flags via `resolve_pane_predicates`.
struct ResolvedPredicates {
    grep: Option<GrepPredicate>,
    filter: Option<CompiledFilter>,
    display: Option<format::DisplayRenderer>,
    record_start: Option<regex::bytes::Regex>,
    format_label: Option<String>,
}

impl ResolvedPredicates {
    /// No-op predicates: a pane that carries the shared display config but no
    /// grep/filter/format/display of its own (e.g. `--split` panes C, D, …).
    fn empty() -> Self {
        ResolvedPredicates {
            grep: None,
            filter: None,
            display: None,
            record_start: None,
            format_label: None,
        }
    }
}

/// Resolve a pane's predicates from raw flag values (used for pane B's
/// `--right-*`). `filter` and `display` require a `format_name`.
fn resolve_pane_predicates(
    grep: &[String],
    filter: &[String],
    format_name: Option<&str>,
    display: Option<&str>,
    case_mode: tess::viewport::CaseMode,
) -> Result<ResolvedPredicates> {
    let grep_pred = if grep.is_empty() {
        None
    } else {
        Some(GrepPredicate::compile(grep, case_mode).map_err(Error::Runtime)?)
    };
    let (filter_pred, display_rend, record_start, label) = match format_name {
        None => (None, None, None, None),
        Some(name) => {
            let formats = format::load_all().map_err(Error::Runtime)?;
            let fmt = formats.get(name).ok_or_else(|| {
                Error::Runtime(format!(
                    "unknown format `{name}` (run --list-formats to see available)"
                ))
            })?;
            let filter_pred = if filter.is_empty() {
                None
            } else {
                let specs: Vec<FilterSpec> = filter
                    .iter()
                    .map(|s| FilterSpec::parse(s).map_err(Error::Runtime))
                    .collect::<Result<_>>()?;
                Some(CompiledFilter::compile(fmt, specs, case_mode).map_err(Error::Runtime)?)
            };
            let display_rend = match display {
                Some(t) => Some(format::DisplayRenderer::new(
                    format::DisplayTemplate::compile(t, &fmt.field_names)
                        .map_err(|e| Error::Runtime(format!("--right-display: {e}")))?,
                    fmt.regex.clone(),
                )),
                None => fmt
                    .display
                    .clone()
                    .map(|t| format::DisplayRenderer::new(t, fmt.regex.clone())),
            };
            let rec = fmt
                .record_start
                .as_ref()
                .and_then(|re| regex::bytes::Regex::new(re.as_str()).ok());
            (filter_pred, display_rend, rec, Some(name.to_string()))
        }
    };
    Ok(ResolvedPredicates {
        grep: grep_pred,
        filter: filter_pred,
        display: display_rend,
        record_start,
        format_label: label,
    })
}

/// Build the second (background) pane for `--split`. Opens `path` through the
/// same source pipeline as a file-switch (`open::open_source_for_path` — no
/// prettify/format-specific config) and configures a viewport with the
/// display-relevant, non-format-specific options the focused pane uses. The
/// pane's grep/filter/format/display predicates are resolved by the CALLER and
/// passed in as `predicates` (from `--right-*` for `--split` pane B, or from the
/// section's own flags for the `--` per-pane form). The encoding is resolved
/// per-pane here from `args.encoding` against this source's own head bytes.
/// `other_pane_init` corrects the size, so the `cols`/`rows` passed here are
/// provisional.
#[allow(clippy::too_many_arguments)]
fn build_second_pane(
    path: &std::path::Path,
    args: &Args,
    ansi_mode: tess::render::AnsiMode,
    cols: u16,
    rows: u16,
    preprocessor: Option<&tess::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    case_mode: tess::viewport::CaseMode,
    predicates: ResolvedPredicates,
) -> Result<tess::pane::Pane> {
    let (src, label, preprocess_failure) =
        tess::open::open_source_for_path(path, args, preprocessor)?;

    let mut idx = match args.tail {
        Some(n) => {
            let off = find_tail_offset(src.as_ref(), n);
            LineIndex::new_starting_at(off)
        }
        None => LineIndex::new(),
    };
    if let Some(n) = args.head {
        idx.set_head_cap(n);
    }
    if let Some(re) = record_start_regex {
        idx.set_record_start(re.clone());
    }

    let mut viewport = Viewport::new(cols, rows, label);
    // Shared subset (line numbers, chop, tab_width, follow, live, hex, squeeze,
    // status_column, rscroll, word_wrap, page_size, file_index) — the single
    // source of truth shared with the runtime `:vsplit` pane.
    tess::app::apply_pane_display_config(&mut viewport, args);
    // Startup-only extras the runtime `:vsplit` path intentionally omits: the
    // `--tabs`/`--header` spec parsers, ANSI mode, and preprocess failure.
    // (`--hex-group` is already validated when the first pane was built, so the
    // helper's silent-ignore of a bad value is unreachable here.)
    if let Some(spec) = args.tabs.as_deref() {
        let stops = parse_tab_stops(spec).map_err(Error::Runtime)?;
        if stops.len() == 1 {
            viewport.opts.tab_width = stops[0].clamp(1, u8::MAX as usize) as u8;
        } else {
            viewport.opts.tab_stops = Some(stops);
        }
    }
    let enc = {
        let head_len = src.len().min(4);
        let head = src.bytes(0..head_len);
        tess::open::resolve_encoding(&args.encoding, &head).map_err(Error::Runtime)?
    };
    viewport.set_encoding(enc);
    viewport.set_ansi_mode(ansi_mode);
    if let Some(spec) = args.header.as_deref() {
        let (lines, hcols) = parse_header_spec(spec).map_err(Error::Runtime)?;
        viewport.set_header(lines, hcols);
    }
    viewport.set_preprocess_failure(preprocess_failure);

    // Status/prompt theming parity with the first pane: resolve `--status-style`
    // as the base, with `--prompt-style` (or the per-format `prompt_style`)
    // taking over when a prompt is active. Mirrors the resolution in real_main
    // so the focused-pane status bar is themed identically once focus can switch.
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
        let prompt_active = match args.prompt.as_deref() {
            Some(_) => true,
            None => fmt_prompt.is_some(),
        };
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

    // Image auto-detection: the split compositor is cell-based, so render any
    // image as ASCII (force_cell_mode in app::run pins the protocol to ASCII).
    // Mirror the first pane: prefer animated playback so the per-pane tick has
    // an AnimationState to advance; fall back to a static first frame otherwise.
    #[cfg(feature = "image")]
    if !args.hex && !args.no_image {
        let head_len = src.len().min(64);
        let head = src.bytes(0..head_len);
        if let Some(fmt) = tess::image_render::sniff_image_format(&head) {
            let all = src.bytes(0..src.len());
            let style = if args.blocks {
                tess::image_render::AsciiStyle::Blocks
            } else {
                tess::image_render::AsciiStyle::Ramp
            };
            use tess::image_render::AnimationDecode;
            let decoded = if args.no_animate {
                AnimationDecode::Static
            } else {
                tess::image_render::decode_animation(&all)
            };
            let loaded = if let AnimationDecode::Animated(anim) = decoded {
                viewport.set_animation(anim, fmt, style, args.image_width);
                true
            } else {
                // Unsupported animation → fall back to the static first frame;
                // the hint flash is the focused pane's concern, so skip it here.
                match tess::image_render::decode_image(&all) {
                    Ok(rgba) => {
                        viewport.set_image(rgba, fmt, style, args.image_width);
                        true
                    }
                    Err(_) => false,
                }
            };
            if loaded {
                viewport.set_image_no_color(args.no_color);
            }
        }
    }

    // Pane B's search uses the (independent) right-case mode passed in, matching
    // the case used to compile its --right-* predicates below. For additional
    // panes (C, D, …) the right-case is not applied; they use Sensitive by
    // convention (the caller passes the base case_mode instead).
    viewport.set_case_mode(case_mode);

    // Apply this pane's predicates (resolved by the caller — from `--right-*`
    // for `--split` pane B, or from the section's own flags for the `--` form).
    // The index is pre-scan here, so set_record_start (the pre-scan setter) is
    // safe. A `--right-format`/section `--format` record_start wins; otherwise we
    // keep the caller's record_start_regex fallback (applied above if present).
    if let Some(re) = predicates.record_start {
        idx.set_record_start(re);
    }
    viewport.set_format_label(predicates.format_label);
    if let Some(g) = predicates.grep {
        viewport.set_grep(Some(g));
    }
    if let Some(f) = predicates.filter {
        viewport.set_filter(Some(f));
    }
    if let Some(d) = predicates.display {
        viewport.set_display(Some(d));
    }
    // In hide mode the visible-line cache must be built up front (the setters
    // cleared it) — otherwise a grep/filter pane renders blank on a static
    // source. Mirrors the focused-pane startup scan.
    if (viewport.filter_active() || viewport.grep_active()) && !viewport.dim_mode() {
        idx.extend_to_end(src.as_ref());
        viewport.extend_visible_lines(&idx, src.as_ref());
    }

    Ok(tess::pane::Pane {
        last_revision: src.revision(),
        #[cfg(feature = "image")]
        last_tick: std::time::Instant::now(),
        src,
        idx,
        viewport,
    })
}

/// Build the extra-panes Vec for `--split`/`--diff` startup.
///
/// - `--diff`: requires exactly two files; returns one extra pane from `files[1]`
///   (with `--right-*` applied).
/// - `--split` with ≥2 files: returns one pane per `files[1..]`; `--right-*`
///   applies to the first extra pane only (pane B); panes C, D, … get shared
///   display config only.
/// - `--split` with 1 file: returns one extra pane from `files[0]` (two views
///   of the same file), with `--right-*` applied.
/// - Neither flag active, or no files: returns an empty Vec.
#[allow(clippy::too_many_arguments)]
fn build_extra_panes(
    files: &[std::path::PathBuf],
    args: &Args,
    ansi_mode: tess::render::AnsiMode,
    cols: u16,
    rows: u16,
    preprocessor: Option<&tess::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    case_mode: tess::viewport::CaseMode,
) -> Result<Vec<tess::pane::Pane>> {
    if args.diff {
        if files.len() != 2 {
            return Err(Error::Runtime("--diff takes exactly two files".into()));
        }
        let rp = resolve_pane_predicates(
            &args.right_grep, &args.right_filter,
            args.right_format.as_deref(), args.right_display.as_deref(),
            case_mode,
        )?;
        let pane = build_second_pane(
            &files[1], args, ansi_mode, cols, rows,
            preprocessor, record_start_regex, case_mode, rp,
        )?;
        return Ok(vec![pane]);
    }

    if args.split {
        if files.len() >= 2 {
            let mut panes = Vec::with_capacity(files.len() - 1);
            for (i, path) in files[1..].iter().enumerate() {
                let (rp, pane_case) = if i == 0 {
                    let rp = resolve_pane_predicates(
                        &args.right_grep, &args.right_filter,
                        args.right_format.as_deref(), args.right_display.as_deref(),
                        case_mode,
                    )?;
                    (rp, case_mode)
                } else {
                    (ResolvedPredicates::empty(), tess::viewport::CaseMode::Sensitive)
                };
                let pane = build_second_pane(
                    path, args, ansi_mode, cols, rows,
                    preprocessor, record_start_regex, pane_case, rp,
                )?;
                panes.push(pane);
            }
            return Ok(panes);
        } else if let Some(path) = files.first() {
            let rp = resolve_pane_predicates(
                &args.right_grep, &args.right_filter,
                args.right_format.as_deref(), args.right_display.as_deref(),
                case_mode,
            )?;
            let pane = build_second_pane(
                path, args, ansi_mode, cols, rows,
                preprocessor, record_start_regex, case_mode, rp,
            )?;
            return Ok(vec![pane]);
        }
    }

    Ok(vec![])
}

/// Build one pane per `--`-delimited section (sections 1..N of the per-pane argv
/// form). Each `Args` here is a fully-parsed section: its file is the first
/// positional, and its per-view flags (`--grep`/`--filter`/`--format`/
/// `--display`/`-i`/`-I` + the shared display config) drive the pane. The global
/// preprocessor (from section 0) is shared. Globals on a section's `Args` are
/// ignored. Each section resolves its own encoding (inside `build_second_pane`)
/// and its own record_start (from its `--format`).
fn build_panes_from_sections(
    sections: &[Args],
    ansi_mode: tess::render::AnsiMode,
    cols: u16,
    rows: u16,
    preprocessor: Option<&tess::preprocess::Preprocessor>,
) -> Result<Vec<tess::pane::Pane>> {
    let mut panes = Vec::with_capacity(sections.len());
    for (i, sa) in sections.iter().enumerate() {
        let pane_no = i + 2; // section 0 = focused pane (pane 1); these start at 2
        let path = sa.files.first().ok_or_else(|| {
            Error::Runtime(format!("pane {pane_no}: no file given"))
        })?;
        if !sa.filter.is_empty() && sa.format.is_none() {
            return Err(Error::Runtime(format!(
                "pane {pane_no}: --filter requires --format"
            )));
        }
        if sa.display.is_some() && sa.format.is_none() {
            return Err(Error::Runtime(format!(
                "pane {pane_no}: --display requires --format"
            )));
        }
        // OR-groups are section-0-only; a later section that sets them would have
        // them silently dropped, so reject rather than surprise the user.
        if !sa.or_filter.is_empty() || !sa.or_grep.is_empty() || !sa.or_group.is_empty() {
            return Err(Error::Runtime(format!(
                "pane {pane_no}: --or-filter/--or-grep/--or-group are only allowed before the first `--`"
            )));
        }
        let case_mode = if sa.IGNORE_CASE {
            tess::viewport::CaseMode::Insensitive
        } else if sa.ignore_case {
            tess::viewport::CaseMode::Smart
        } else {
            tess::viewport::CaseMode::Sensitive
        };
        let rp = resolve_pane_predicates(
            &sa.grep, &sa.filter,
            sa.format.as_deref(), sa.display.as_deref(),
            case_mode,
        )?;
        let pane = build_second_pane(
            path, sa, ansi_mode, cols, rows,
            preprocessor, None, case_mode, rp,
        )?;
        panes.push(pane);
    }
    Ok(panes)
}

/// Validate the globals (`args`, from section 0) against the per-pane `--` form.
/// `section_count` is the total number of panes (sections). The `--` form is
/// mutually exclusive with `--split`/`--right-*`; `--diff` requires exactly two
/// sections; and section 0 (the first pane) must name at most one file (a stray
/// extra file there would be a confusing mix of `:n` working-set and panes).
fn validate_per_pane_argv(args: &Args, section_count: usize) -> Result<()> {
    let right_used = !args.right_grep.is_empty()
        || !args.right_filter.is_empty()
        || args.right_format.is_some()
        || args.right_display.is_some()
        || args.right_ignore_case
        || args.right_IGNORE_case;
    if args.split || right_used {
        return Err(Error::Runtime(
            "the `--` per-pane form can't be combined with --split or --right-*".to_string(),
        ));
    }
    if args.diff && section_count != 2 {
        return Err(Error::Runtime(
            "--diff with the `--` form needs exactly two panes".to_string(),
        ));
    }
    if args.files.len() > 1 {
        return Err(Error::Runtime(
            "the first pane (before `--`) must name at most one file".to_string(),
        ));
    }
    Ok(())
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
    let keymap = tess::keys::KeyMap::load_layered()
        .unwrap_or_else(|_| tess::keys::KeyMap::empty());
    let file_set = tess::file_set::FileSet::new(vec![std::path::PathBuf::from(label)]);
    let stub_args = Args::parse_from(["tess"]);
    app::run(
        Box::new(src),
        viewport,
        idx,
        sigterm,
        RebuildSpec::default(),
        keymap,
        file_set,
        None,
        stub_args,
        None,
        None,
        vec![],
        #[cfg(feature = "image")]
        (tess::viewport::ImageProtocol::Ascii, None),
    )?;
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

/// Parse `--header=L` or `--header=L,C` into `(L, C)`. Single arg
/// implies `C = 0`. Both fields default to 0 when missing.
fn parse_header_spec(s: &str) -> std::result::Result<(usize, usize), String> {
    let parts: Vec<&str> = s.split(',').collect();
    match parts.as_slice() {
        [l] => l.parse::<usize>()
            .map(|n| (n, 0))
            .map_err(|_| format!("--header: not a number `{l}`")),
        [l, c] => {
            let l = l.parse::<usize>()
                .map_err(|_| format!("--header: bad L `{l}`"))?;
            let c = c.parse::<usize>()
                .map_err(|_| format!("--header: bad C `{c}`"))?;
            Ok((l, c))
        }
        _ => Err("--header takes L or L,C".to_string()),
    }
}

fn parse_tab_stops(spec: &str) -> std::result::Result<Vec<usize>, String> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let n: usize = part.trim().parse()
            .map_err(|_| format!("--tabs: invalid number `{part}`"))?;
        if n == 0 { return Err("--tabs: stops must be >= 1".to_string()); }
        if out.last().is_some_and(|&prev| n <= prev) {
            return Err("--tabs: stops must be strictly ascending".to_string());
        }
        out.push(n);
    }
    if out.is_empty() { return Err("--tabs: empty list".to_string()); }
    Ok(out)
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

    // Split on standalone `--` into per-pane sections. Zero `--` → one section
    // (the existing single-view path, unchanged). Each section is prefixed with
    // argv[0].
    let sections = tess::cli::split_argv_sections(&raw_argv);
    let per_pane = sections.len() > 1;

    // Section 0 runs the full existing pipeline: +CMD extraction, group
    // expansion, OR-group extraction, then clap parse → the globals + pane 0.
    let s0 = &sections[0];
    let plus_cmds: Vec<String> = s0
        .iter()
        .skip(1)
        .filter(|a| a.starts_with('+') && a.len() > 1)
        .cloned()
        .collect();
    let cleaned_argv: Vec<String> = s0
        .iter()
        .enumerate()
        .filter(|(i, a)| *i == 0 || !(a.starts_with('+') && a.len() > 1))
        .map(|(_, a)| a.clone())
        .collect();

    // Expand any user-defined groups (`[group.X]` in formats.toml) before clap
    // parses. A `--<groupname>` token becomes the group's flags inline, and
    // remaining bare positionals become `--filter <arg>` pairs.
    let groups = format::load_groups().map_err(Error::Runtime)?;
    let argv = format::expand_argv(cleaned_argv, &groups);
    let or_spec = tess::or::extract_from_argv(&argv);
    let args = Args::parse_from(argv);

    // Sections 1..N: each group-expands and parses into its own per-view `Args`.
    // OR-groups and `+CMD` are section-0-only. `+CMD` is only stripped from
    // section 0, so a `+token` in a later section reaches clap as a positional
    // (a second "file") — harmless but ignored; `--or-*` in a later section is
    // rejected in `build_panes_from_sections`. `+CMD`/OR belong in section 0.
    let section_args: Vec<Args> = if per_pane {
        sections[1..]
            .iter()
            .map(|sec| {
                let expanded = format::expand_argv(sec.clone(), &groups);
                Args::parse_from(expanded)
            })
            .collect()
    } else {
        Vec::new()
    };

    if per_pane {
        validate_per_pane_argv(&args, sections.len())?;
    }

    if args.gitdiff {
        if per_pane || args.diff || args.split
            || !args.right_grep.is_empty() || !args.right_filter.is_empty()
            || args.right_format.is_some() || args.right_display.is_some()
            || args.right_ignore_case || args.right_IGNORE_case
        {
            return Err(Error::Runtime(
                "--gitdiff can't be combined with --diff / --split / --right-* / the `--` form".to_string(),
            ));
        }
        if args.from_clipboard {
            return Err(Error::Runtime("--gitdiff reads a file, not the clipboard".to_string()));
        }
        if args.files.len() != 1 {
            return Err(Error::Runtime("--gitdiff needs exactly one file".to_string()));
        }
    }

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
    // Validate --image-protocol early so a typo is rejected regardless of input
    // type (mirrors --truecolor). Pass is_tty=false so this validation never
    // queries the terminal — `auto` detection stays lazy until an image opens.
    #[cfg(feature = "image")]
    resolve_image_protocol(&args.image_protocol, false)?;

    // `--follow-name` is accepted for compatibility but already matches our
    // default behavior (rotation/truncation handled by re-opening the path).
    // When given without `-f`, surface a one-line stderr note so users don't
    // think they've also enabled follow mode.
    if args.follow_name && !args.follow {
        eprintln!("tess: --follow-name has no effect without -f / --follow");
    }
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
    if or_spec.has_filters() && args.format.is_none() {
        return Err(Error::Runtime(
            "--or-filter requires --format".to_string(),
        ));
    }
    if args.dim && args.filter.is_empty() && args.grep.is_empty() && or_spec.is_empty() {
        return Err(Error::Runtime(
            "--dim has no effect without --filter, --grep, or --or-filter/--or-grep".to_string(),
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
    let batch_destination: Option<BatchDestination> = if args.to_clipboard {
        Some(BatchDestination::Clipboard)
    } else if args.stdout {
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
    if args.to_clipboard && args.follow {
        return Err(Error::Runtime(
            "--to-clipboard is not compatible with --follow".to_string(),
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
        if !or_spec.is_empty() {
            return Err(Error::Runtime(
                "--prettify is not supported with --or-filter/--or-grep".to_string(),
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
    // `--from-clipboard` reads the clipboard into an in-memory source — no
    // file args (clap enforces `conflicts_with = "files"`) and no piped stdin.
    // Following has no producer to follow, so reject -f / --live here.
    if args.from_clipboard {
        if args.follow {
            return Err(Error::Runtime(
                "--from-clipboard is not compatible with --follow".to_string(),
            ));
        }
        if args.live {
            return Err(Error::Runtime(
                "--from-clipboard is not compatible with --live".to_string(),
            ));
        }
    }

    let file_set = tess::file_set::FileSet::new(args.files.clone());
    let mut consumed_stdin = false;
    let mut source_supports_tail = true;
    let mut preprocess_failure: Option<String> = None;
    let (src, label): (Box<dyn Source>, String) = if args.from_clipboard {
        let bytes = tess::clipboard::read().map_err(Error::Runtime)?;
        (Box::new(tess::source::MemorySource::new(bytes)), "(clipboard)".to_string())
    } else { match args.files.first() {
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
    }};

    // Resolve --encoding early: needed by the prettify transform and by batch/
    // interactive paths alike. Peek the first bytes for BOM detection.
    // Error before entering raw mode so diagnostics print cleanly.
    let resolved_enc = {
        let head_len = src.len().min(4);
        let head = src.bytes(0..head_len);
        tess::open::resolve_encoding(&args.encoding, &head)
            .map_err(Error::Runtime)?
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
                let wrapped = TransformingSource::wrap(src, mode, resolved_enc);
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
    // Pane B's case mode is INDEPENDENT of the global -i/-I (matches the
    // no-inheritance rule of the other --right-* flags); default Sensitive.
    let right_case_mode = if args.right_IGNORE_case {
        tess::viewport::CaseMode::Insensitive
    } else if args.right_ignore_case {
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

    // Compile OR-groups. Reuse the format if one is set; OR-greps work without
    // it (OR-filters without a format were already rejected above). `LogFormat`
    // is NOT Clone, so borrow `fmt` from a `formats` map kept alive inside each
    // match arm for the duration of the compile call.
    let compiled_or = if or_spec.is_empty() {
        tess::or::OrGroups::default()
    } else {
        match args.format.as_deref() {
            Some(name) => {
                let formats = format::load_all().map_err(Error::Runtime)?;
                let fmt = formats.get(name).ok_or_else(|| Error::Runtime(format!(
                    "unknown format `{name}` (run --list-formats to see available)"
                )))?;
                tess::or::OrGroups::compile(&or_spec, Some(fmt), case_mode)
                    .map_err(Error::Runtime)?
            }
            None => tess::or::OrGroups::compile(&or_spec, None, case_mode)
                .map_err(Error::Runtime)?,
        }
    };

    let sigterm = install_signal_flag();

    // Batch mode: skip the terminal guard entirely and route through
    // `batch::run` instead of the interactive event loop.
    if let Some(destination) = batch_destination {
        let _ = prettify_label; // batch keeps the prettified bytes; label unused

        #[cfg(feature = "image")]
        if !args.hex && !args.no_image {
            let head_len = src.len().min(64);
            let head = src.bytes(0..head_len);
            if tess::image_render::sniff_image_format(&head).is_some() {
                let all = src.bytes(0..src.len());
                // Animation is interactive-only; batch/export decodes a static image.
                let rgba = tess::image_render::decode_image(&all)
                    .map_err(|e| Error::Runtime(format!("image decode failed: {e}")))?;
                let style = if args.blocks {
                    tess::image_render::AsciiStyle::Blocks
                } else {
                    tess::image_render::AsciiStyle::Ramp
                };
                let width = args.image_width.map(|w| w.clamp(1, u16::MAX as usize) as u16).unwrap_or(80);
                let grid = tess::image_render::render_image(&rgba, width, style, !args.no_color);
                use std::io::Write as _;
                // Explicit terminal-graphics protocols export the encoded escape
                // blob verbatim instead of the ASCII grid. `auto`/`ascii` fall
                // through to write_grid (no terminal to detect against in batch).
                let proto_bytes: Option<Vec<u8>> = match args.image_protocol.as_str() {
                    "kitty" => Some(tess::image_protocol::encode_kitty(&rgba)),
                    "sixel" => Some(tess::image_protocol::encode_sixel(&rgba)),
                    _ => None,
                };
                if matches!(destination, BatchDestination::Clipboard) {
                    let mut buf: Vec<u8> = Vec::new();
                    if let Some(bytes) = &proto_bytes {
                        buf.extend_from_slice(bytes);
                    } else {
                        tess::image_export::write_grid(&mut buf, &grid, !args.no_color)
                            .map_err(|e| Error::Runtime(e.to_string()))?;
                    }
                    tess::clipboard::write(&buf).map_err(Error::Runtime)?;
                    return Ok(());
                }
                let mut w: Box<dyn std::io::Write> = match &destination {
                    BatchDestination::Stdout => Box::new(std::io::stdout().lock()),
                    BatchDestination::File(p) => Box::new(std::fs::File::create(p)
                        .map_err(|e| Error::Runtime(format!("{}: {e}", p.display())))?),
                    BatchDestination::Clipboard => unreachable!("clipboard handled above"),
                };
                if let Some(bytes) = &proto_bytes {
                    w.write_all(bytes).map_err(|e| Error::Runtime(e.to_string()))?;
                } else {
                    tess::image_export::write_grid(&mut w, &grid, !args.no_color)
                        .map_err(|e| Error::Runtime(e.to_string()))?;
                }
                return Ok(());
            }
        }

        let spec = BatchSpec {
            destination,
            follow: args.follow,
            poll_interval: std::time::Duration::from_millis(250),
        };
        return batch::run(src, idx, compiled_filter, compiled_grep, compiled_or, display_renderer, spec, sigterm, resolved_enc);
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
        let trailing_nl = bytes.last().is_none_or(|&b| b == b'\n');
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
    if let Some(spec) = args.tabs.as_deref() {
        let stops = parse_tab_stops(spec).map_err(Error::Runtime)?;
        if stops.len() == 1 {
            viewport.opts.tab_width = stops[0].clamp(1, u8::MAX as usize) as u8;
        } else {
            viewport.opts.tab_stops = Some(stops);
        }
    }
    viewport.set_follow_mode(args.follow);
    viewport.set_live_mode(args.live);
    viewport.set_prettify_label(prettify_label);
    if let Some(f) = compiled_filter {
        viewport.set_filter(Some(f));
    }
    if let Some(g) = compiled_grep {
        viewport.set_grep(Some(g));
    }
    if compiled_or.is_active() {
        viewport.set_or_groups(compiled_or);
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
    viewport.set_encoding(resolved_enc);
    viewport.set_ansi_mode(ansi_mode);
    viewport.set_case_mode(case_mode);
    viewport.set_hilite_search(!args.no_hilite_search);
    viewport.set_search_skip_screen(args.search_skip_screen);
    viewport.set_hilite_only_match(args.hilite_only_match);
    viewport.set_tilde_eof(args.tilde);
    viewport.set_incsearch(args.incsearch);
    let qae = if args.QUIT_AT_EOF {
        tess::viewport::QuitAtEof::First
    } else if args.quit_at_eof {
        tess::viewport::QuitAtEof::Second
    } else {
        tess::viewport::QuitAtEof::Off
    };
    viewport.set_quit_at_eof(qae);
    viewport.set_squeeze_blanks(args.squeeze_blanks);
    viewport.set_status_column(args.status_column);
    if let Some(spec) = args.header.as_deref() {
        let (lines, cols) = parse_header_spec(spec).map_err(Error::Runtime)?;
        viewport.set_header(lines, cols);
    }
    viewport.opts.rscroll_char = args.rscroll.chars().next();
    viewport.opts.word_wrap = args.word_wrap;
    viewport.set_page_size(args.window);
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
    let keymap = tess::keys::KeyMap::load_layered()
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

    // `-p PATTERN` is equivalent to `+/PATTERN`: open at the first match. Applied
    // after explicit +CMDs (last jump wins).
    if let Some(pat) = args.pattern.as_deref() {
        viewport
            .set_search(pat.to_string(), tess::viewport::SearchDirection::Forward)
            .map_err(|e| Error::Runtime(format!("-p {pat}: {e}")))?;
        viewport.search_repeat(src.as_ref(), &mut idx, false);
    }

    // Resolve the startup image protocol once: it pins the first viewport's
    // image rendering AND is handed to `app::run` so `:only` can restore it
    // after a split (split forces ASCII; `:only` undoes that).
    #[cfg(feature = "image")]
    let startup_image_protocol = {
        let is_tty = std::io::stdout().is_terminal();
        resolve_image_protocol(&args.image_protocol, is_tty)?
    };

    // Image auto-detection: sniff the source's leading bytes. If it is a
    // supported image and the user did not force raw (--no-image) or hex
    // (--hex wins), decode and switch the viewport into ASCII-art mode.
    #[cfg(feature = "image")]
    if !args.hex && !args.no_image {
        let head_len = src.len().min(64);
        let head = src.bytes(0..head_len);
        if let Some(fmt) = tess::image_render::sniff_image_format(&head) {
            let all = src.bytes(0..src.len());
            let style = if args.blocks {
                tess::image_render::AsciiStyle::Blocks
            } else {
                tess::image_render::AsciiStyle::Ramp
            };

            // Prefer animated playback when the source decodes as a
            // multi-frame image and the user did not ask for a static view.
            // Decodes all frames eagerly (a frame-count cap could be added if
            // huge animations become a memory concern).
            use tess::image_render::AnimationDecode;
            let decoded = if args.no_animate {
                AnimationDecode::Static
            } else {
                tess::image_render::decode_animation(&all)
            };
            // `Unsupported` means the source IS animated but its frames can't be
            // decoded (e.g. a 16-bit APNG) — show the static first frame and
            // hint, instead of silently dropping the animation.
            let unsupported = matches!(decoded, AnimationDecode::Unsupported(_));

            let loaded = if let AnimationDecode::Animated(anim) = decoded {
                viewport.set_animation(anim, fmt, style, args.image_width);
                true
            } else {
                match tess::image_render::decode_image(&all) {
                    Ok(rgba) => {
                        viewport.set_image(rgba, fmt, style, args.image_width);
                        if unsupported {
                            viewport.flash("couldn't decode animation; showing first frame", 20);
                        }
                        true
                    }
                    Err(e) => {
                        eprintln!("tess: image decode failed ({e}); showing raw");
                        false
                    }
                }
            };

            if loaded {
                viewport.set_image_no_color(args.no_color);
                let (proto, cell_px) = startup_image_protocol;
                viewport.set_image_protocol(proto, cell_px);
            }
        }
    }

    // Build extra panes for `--split`/`--diff` before `args`/`preprocessor`/
    // `record_start_regex` are moved into `app::run`. For `--split` with N files,
    // one pane per files[1..] is built; `--right-*` applies only to the first
    // extra pane (pane B). `other_pane_init` inside `app::run` resizes all panes,
    // so `cols`/`rows` here are provisional.
    let extra_panes: Vec<tess::pane::Pane> = if per_pane {
        build_panes_from_sections(
            &section_args,
            ansi_mode,
            cols,
            rows,
            preprocessor.as_ref(),
        )?
    } else {
        match build_extra_panes(
            &args.files,
            &args,
            ansi_mode,
            cols,
            rows,
            preprocessor.as_ref(),
            record_start_regex.as_ref(),
            right_case_mode,
        ) {
            Ok(panes) => panes,
            Err(e) => {
                eprintln!("tess: pane setup: {e}");
                vec![]
            }
        }
    };

    app::run(
        src,
        viewport,
        idx,
        sigterm,
        rebuild_spec,
        keymap,
        file_set,
        record_start_regex,
        args,
        preprocessor,
        tag_file,
        extra_panes,
        #[cfg(feature = "image")]
        startup_image_protocol,
    )?;
    Ok(())
}

#[cfg(feature = "image")]
fn resolve_image_protocol(flag: &str, is_tty: bool)
    -> std::result::Result<(tess::viewport::ImageProtocol, Option<(u16, u16)>), Error> {
    use tess::viewport::ImageProtocol;
    match flag {
        "ascii" => Ok((ImageProtocol::Ascii, None)),
        "kitty" => Ok((ImageProtocol::Kitty, None)),
        "sixel" => Ok((ImageProtocol::Sixel, None)),
        "auto" => {
            if !is_tty {
                return Ok((ImageProtocol::Ascii, None));
            }
            let g = tess::term_query::detect();
            // Preference: Kitty > Sixel > ASCII.
            let proto = if g.kitty { ImageProtocol::Kitty }
                else if g.sixel { ImageProtocol::Sixel }
                else { ImageProtocol::Ascii };
            Ok((proto, g.cell_px))
        }
        other => Err(Error::Runtime(format!(
            "--image-protocol: unknown value '{other}' (expected auto, kitty, sixel, or ascii)"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plus_g_is_goto_bottom() {
        assert_eq!(parse_plus_cmd("+G"), Ok(PlusCmd::GotoBottom));
    }

    #[test]
    fn per_pane_excludes_split_and_right() {
        let mut a = Args::parse_from(["tess", "x"]);
        a.split = true;
        assert!(validate_per_pane_argv(&a, 2).is_err());

        let mut b = Args::parse_from(["tess", "x"]);
        b.right_grep = vec!["E".into()];
        assert!(validate_per_pane_argv(&b, 2).is_err());

        let c = Args::parse_from(["tess", "x"]);
        assert!(validate_per_pane_argv(&c, 2).is_ok());
    }

    #[test]
    fn per_pane_diff_needs_exactly_two_sections() {
        let mut a = Args::parse_from(["tess", "x"]);
        a.diff = true;
        assert!(validate_per_pane_argv(&a, 2).is_ok());
        assert!(validate_per_pane_argv(&a, 1).is_err());
        assert!(validate_per_pane_argv(&a, 3).is_err());
    }

    #[test]
    fn per_pane_section0_at_most_one_file() {
        // stdin (0 files) and a single file are fine; >1 file in section 0 is an error.
        assert!(validate_per_pane_argv(&Args::parse_from(["tess"]), 2).is_ok());
        assert!(validate_per_pane_argv(&Args::parse_from(["tess", "a"]), 2).is_ok());
        assert!(validate_per_pane_argv(&Args::parse_from(["tess", "a", "b"]), 2).is_err());
    }

    #[test]
    fn resolved_predicates_empty_is_all_none() {
        let rp = ResolvedPredicates::empty();
        assert!(rp.grep.is_none());
        assert!(rp.filter.is_none());
        assert!(rp.display.is_none());
        assert!(rp.record_start.is_none());
        assert!(rp.format_label.is_none());
    }

    #[cfg(feature = "image")]
    #[test]
    fn resolve_image_protocol_explicit_and_errors() {
        use tess::viewport::ImageProtocol;
        assert_eq!(resolve_image_protocol("ascii", true).unwrap().0, ImageProtocol::Ascii);
        assert_eq!(resolve_image_protocol("kitty", true).unwrap().0, ImageProtocol::Kitty);
        assert_eq!(resolve_image_protocol("sixel", false).unwrap().0, ImageProtocol::Sixel);
        // auto with no tty → ASCII (never queries the terminal).
        assert_eq!(resolve_image_protocol("auto", false).unwrap().0, ImageProtocol::Ascii);
        // unknown value errors.
        assert!(resolve_image_protocol("bogus", true).is_err());
        // The eager-validation call uses is_tty=false; valid values must resolve
        // without error (and without touching the terminal).
        assert!(resolve_image_protocol("auto", false).is_ok());
        assert!(resolve_image_protocol("kitty", false).is_ok());
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
    fn parse_tab_stops_accepts_ascending_list() {
        assert_eq!(parse_tab_stops("4,8,16").unwrap(), vec![4, 8, 16]);
        assert_eq!(parse_tab_stops("4").unwrap(), vec![4]);
    }

    #[test]
    fn parse_tab_stops_rejects_bad_input() {
        assert!(parse_tab_stops("").is_err());            // empty
        assert!(parse_tab_stops("0").is_err());           // zero
        assert!(parse_tab_stops("4,x").is_err());         // non-numeric
        assert!(parse_tab_stops("8,4").is_err());         // not ascending
        assert!(parse_tab_stops("4,4").is_err());         // not strictly ascending
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

    #[test]
    fn resolve_pane_predicates_grep_and_filter() {
        let case = tess::viewport::CaseMode::Sensitive;
        let r = resolve_pane_predicates(&["ERROR".to_string()], &[], None, None, case).unwrap();
        assert!(r.grep.is_some() && r.filter.is_none());
        let r2 = resolve_pane_predicates(&[], &["status=404".to_string()], Some("apache-common"), None, case).unwrap();
        assert!(r2.filter.is_some());
        assert_eq!(r2.format_label.as_deref(), Some("apache-common"));
    }

    #[test]
    fn build_second_pane_hide_mode_is_not_blank() {
        // Regression: a `--right-grep` (hide-mode) pane B must build its
        // visible-line cache up front, else it renders blank on a static file.
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "alpha").unwrap();
        writeln!(f, "beta MATCH").unwrap();
        writeln!(f, "gamma").unwrap();
        f.flush().unwrap();
        let args = Args::parse_from(["tess", "--split", "x", "y", "--right-grep", "MATCH"]);
        let rp = resolve_pane_predicates(
            &args.right_grep, &args.right_filter,
            args.right_format.as_deref(), args.right_display.as_deref(),
            tess::viewport::CaseMode::Sensitive,
        ).unwrap();
        let mut pane = build_second_pane(
            f.path(), &args, tess::render::AnsiMode::Strict, 80, 24, None, None,
            tess::viewport::CaseMode::Sensitive, rp,
        ).unwrap();
        let frame = pane.viewport.frame(pane.src.as_ref(), &mut pane.idx);
        let text: String = frame.body.iter().flat_map(|row| row.iter().filter_map(|c| match c {
            tess::render::Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        })).collect();
        assert!(text.contains("MATCH"), "pane B should show the matching line; got {text:?}");
        assert!(!text.contains("alpha"), "pane B should hide non-matching lines; got {text:?}");
    }

    #[test]
    fn build_second_pane_right_ignore_case_matches_insensitively() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "alpha").unwrap();
        writeln!(f, "an error here").unwrap();   // lowercase "error"
        writeln!(f, "gamma").unwrap();
        f.flush().unwrap();
        let args = Args::parse_from([
            "tess", "--split", "x", "y", "--right-grep", "ERROR", "--right-IGNORE-CASE",
        ]);
        let right_case = if args.right_IGNORE_case { tess::viewport::CaseMode::Insensitive }
            else if args.right_ignore_case { tess::viewport::CaseMode::Smart }
            else { tess::viewport::CaseMode::Sensitive };
        let rp = resolve_pane_predicates(
            &args.right_grep, &args.right_filter,
            args.right_format.as_deref(), args.right_display.as_deref(),
            right_case,
        ).unwrap();
        let mut pane = build_second_pane(
            f.path(), &args, tess::render::AnsiMode::Strict, 80, 24, None, None,
            right_case, rp,
        ).unwrap();
        assert_eq!(pane.viewport.case_mode(), tess::viewport::CaseMode::Insensitive);
        let frame = pane.viewport.frame(pane.src.as_ref(), &mut pane.idx);
        let text: String = frame.body.iter().flat_map(|row| row.iter().filter_map(|c| match c {
            tess::render::Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        })).collect();
        assert!(text.contains("error"), "case-insensitive --right-grep should match 'error'; got {text:?}");
    }

    #[test]
    fn build_extra_panes_one_per_file_for_split() {
        use std::io::Write;
        let mk = |s: &str| { let mut f = tempfile::NamedTempFile::new().unwrap(); writeln!(f, "{s}").unwrap(); f.flush().unwrap(); f };
        let (a, b, c) = (mk("A"), mk("B"), mk("C"));
        let files = vec![a.path().to_path_buf(), b.path().to_path_buf(), c.path().to_path_buf()];
        let args = Args::parse_from(["tess", "--split", "x", "y", "z"]);
        let extras = build_extra_panes(
            &files, &args, tess::render::AnsiMode::Strict, 80, 24, None, None,
            tess::viewport::CaseMode::Sensitive,
        ).unwrap();
        assert_eq!(extras.len(), 2); // panes for files[1] and files[2]
    }

    #[test]
    fn sections_build_panes_with_independent_grep() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("tess_sec_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let pa = dir.join("a.txt");
        let pb = dir.join("b.txt");
        std::fs::File::create(&pa).unwrap().write_all(b"alpha\nbeta\n").unwrap();
        std::fs::File::create(&pb).unwrap().write_all(b"gamma\ndelta\n").unwrap();

        let s1 = Args::parse_from(["tess", pa.to_str().unwrap(), "--grep", "alpha"]);
        let s2 = Args::parse_from(["tess", pb.to_str().unwrap()]);
        let panes = build_panes_from_sections(
            &[s1, s2],
            tess::render::AnsiMode::Interpret,
            80, 24,
            None,
        ).unwrap();
        assert_eq!(panes.len(), 2);
        assert!(panes[0].viewport.grep_active());
        assert!(!panes[1].viewport.grep_active());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sections_filter_without_format_errs() {
        let dir = std::env::temp_dir().join(format!("tess_secf_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let pa = dir.join("a.txt");
        std::fs::write(&pa, b"x\n").unwrap();
        let s = Args::parse_from(["tess", pa.to_str().unwrap(), "--filter", "status=404"]);
        let err = build_panes_from_sections(&[s], tess::render::AnsiMode::Interpret, 80, 24, None);
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sections_missing_file_errs() {
        let s = Args::parse_from(["tess", "--grep", "x"]);
        let err = build_panes_from_sections(&[s], tess::render::AnsiMode::Interpret, 80, 24, None);
        assert!(err.is_err());
    }

    #[test]
    fn sections_or_flags_rejected() {
        let dir = std::env::temp_dir().join(format!("tess_secor_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let pa = dir.join("a.txt");
        std::fs::write(&pa, b"x\n").unwrap();
        let s = Args::parse_from(["tess", pa.to_str().unwrap(), "--or-grep", "E"]);
        let err = build_panes_from_sections(&[s], tess::render::AnsiMode::Interpret, 80, 24, None);
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
