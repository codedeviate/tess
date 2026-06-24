//! Pane construction shared by the binary's startup paths (`--split`,
//! `--diff`, `--gitdiff`, the `--`-per-pane form) and the library event loop's
//! upcoming runtime `:layout` command. These are pure builders: given an `Args`
//! plus a resolved set of per-pane predicates, they open a source and assemble a
//! configured `Pane`. They live in the library (not `main.rs`) so `app::run` can
//! reuse them.

use crate::cli::Args;
use crate::error::{Error, Result};
use crate::filter::{CompiledFilter, FilterSpec};
use crate::format;
use crate::grep::GrepPredicate;
use crate::line_index::LineIndex;
use crate::source::find_tail_offset;
use crate::viewport::Viewport;

/// Resolved predicates for a single pane: compiled grep, filter, display
/// renderer, record-start regex, and format label. Used to seed pane B's
/// `--right-*` flags via `resolve_pane_predicates`.
pub struct ResolvedPredicates {
    pub grep: Option<GrepPredicate>,
    pub filter: Option<CompiledFilter>,
    pub display: Option<format::DisplayRenderer>,
    pub record_start: Option<regex::bytes::Regex>,
    pub format_label: Option<String>,
}

impl ResolvedPredicates {
    /// No-op predicates: a pane that carries the shared display config but no
    /// grep/filter/format/display of its own (e.g. `--split` panes C, D, …).
    pub fn empty() -> Self {
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
pub fn resolve_pane_predicates(
    grep: &[String],
    filter: &[String],
    format_name: Option<&str>,
    display: Option<&str>,
    case_mode: crate::viewport::CaseMode,
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
pub fn build_second_pane(
    path: &std::path::Path,
    args: &Args,
    ansi_mode: crate::render::AnsiMode,
    cols: u16,
    rows: u16,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    case_mode: crate::viewport::CaseMode,
    predicates: ResolvedPredicates,
) -> Result<crate::pane::Pane> {
    let (src, label, preprocess_failure) =
        crate::open::open_source_for_path(path, args, preprocessor)?;
    build_pane_from_source(
        src, label, preprocess_failure, args, ansi_mode, cols, rows,
        record_start_regex, case_mode, predicates,
    )
}

/// Build a `Pane` from an already-constructed source. The body is the tail of
/// `build_second_pane` after the source is opened, factored out so callers
/// (e.g. `--gitdiff`) can supply an in-memory source instead of a path.
#[allow(clippy::too_many_arguments)]
pub fn build_pane_from_source(
    src: Box<dyn crate::source::Source>,
    label: String,
    preprocess_failure: Option<String>,
    args: &Args,
    ansi_mode: crate::render::AnsiMode,
    cols: u16,
    rows: u16,
    record_start_regex: Option<&regex::bytes::Regex>,
    case_mode: crate::viewport::CaseMode,
    predicates: ResolvedPredicates,
) -> Result<crate::pane::Pane> {
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
    crate::app::apply_pane_display_config(&mut viewport, args);
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
        crate::open::resolve_encoding(&args.encoding, &head).map_err(Error::Runtime)?
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
        let (fmt_prompt, fmt_prompt_style): (Option<crate::prompt::ParsedPrompt>, Option<crate::ansi::Style>) =
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
        let status_style_base = crate::style_spec::parse(&args.status_style)
            .map_err(|e| Error::Runtime(format!("--status-style: {e}")))?;
        let cli_prompt_style = if args.prompt_style.trim().is_empty() {
            None
        } else {
            Some(crate::style_spec::parse(&args.prompt_style)
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
        if let Some(fmt) = crate::image_render::sniff_image_format(&head) {
            let all = src.bytes(0..src.len());
            let style = if args.blocks {
                crate::image_render::AsciiStyle::Blocks
            } else {
                crate::image_render::AsciiStyle::Ramp
            };
            use crate::image_render::AnimationDecode;
            let decoded = if args.no_animate {
                AnimationDecode::Static
            } else {
                crate::image_render::decode_animation(&all)
            };
            let loaded = if let AnimationDecode::Animated(anim) = decoded {
                viewport.set_animation(anim, fmt, style, args.image_width);
                true
            } else {
                // Unsupported animation → fall back to the static first frame;
                // the hint flash is the focused pane's concern, so skip it here.
                match crate::image_render::decode_image(&all) {
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

    Ok(crate::pane::Pane {
        last_revision: src.revision(),
        #[cfg(feature = "image")]
        last_tick: std::time::Instant::now(),
        src,
        idx,
        viewport,
    })
}

/// Build one pane per `--`-delimited section (sections 1..N of the per-pane argv
/// form). Each `Args` here is a fully-parsed section: its file is the first
/// positional, and its per-view flags (`--grep`/`--filter`/`--format`/
/// `--display`/`-i`/`-I` + the shared display config) drive the pane. The global
/// preprocessor (from section 0) is shared. Globals on a section's `Args` are
/// ignored. Each section resolves its own encoding (inside `build_second_pane`)
/// and its own record_start (from its `--format`).
pub fn build_panes_from_sections(
    sections: &[Args],
    ansi_mode: crate::render::AnsiMode,
    cols: u16,
    rows: u16,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
) -> Result<Vec<crate::pane::Pane>> {
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
            crate::viewport::CaseMode::Insensitive
        } else if sa.ignore_case {
            crate::viewport::CaseMode::Smart
        } else {
            crate::viewport::CaseMode::Sensitive
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

/// Parse `--header=L` or `--header=L,C` into `(L, C)`. Single arg
/// implies `C = 0`. Both fields default to 0 when missing.
pub fn parse_header_spec(s: &str) -> std::result::Result<(usize, usize), String> {
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

pub fn parse_tab_stops(spec: &str) -> std::result::Result<Vec<usize>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Args;
    use clap::Parser;

    #[test]
    fn resolved_predicates_empty_is_all_none() {
        let rp = ResolvedPredicates::empty();
        assert!(rp.grep.is_none());
        assert!(rp.filter.is_none());
        assert!(rp.display.is_none());
        assert!(rp.record_start.is_none());
        assert!(rp.format_label.is_none());
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
    fn resolve_pane_predicates_grep_and_filter() {
        let case = crate::viewport::CaseMode::Sensitive;
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
            crate::viewport::CaseMode::Sensitive,
        ).unwrap();
        let mut pane = build_second_pane(
            f.path(), &args, crate::render::AnsiMode::Strict, 80, 24, None, None,
            crate::viewport::CaseMode::Sensitive, rp,
        ).unwrap();
        let frame = pane.viewport.frame(pane.src.as_ref(), &mut pane.idx);
        let text: String = frame.body.iter().flat_map(|row| row.iter().filter_map(|c| match c {
            crate::render::Cell::Char { ch, .. } => Some(*ch),
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
        let right_case = if args.right_IGNORE_case { crate::viewport::CaseMode::Insensitive }
            else if args.right_ignore_case { crate::viewport::CaseMode::Smart }
            else { crate::viewport::CaseMode::Sensitive };
        let rp = resolve_pane_predicates(
            &args.right_grep, &args.right_filter,
            args.right_format.as_deref(), args.right_display.as_deref(),
            right_case,
        ).unwrap();
        let mut pane = build_second_pane(
            f.path(), &args, crate::render::AnsiMode::Strict, 80, 24, None, None,
            right_case, rp,
        ).unwrap();
        assert_eq!(pane.viewport.case_mode(), crate::viewport::CaseMode::Insensitive);
        let frame = pane.viewport.frame(pane.src.as_ref(), &mut pane.idx);
        let text: String = frame.body.iter().flat_map(|row| row.iter().filter_map(|c| match c {
            crate::render::Cell::Char { ch, .. } => Some(*ch),
            _ => None,
        })).collect();
        assert!(text.contains("error"), "case-insensitive --right-grep should match 'error'; got {text:?}");
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
            crate::render::AnsiMode::Interpret,
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
        let err = build_panes_from_sections(&[s], crate::render::AnsiMode::Interpret, 80, 24, None);
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sections_missing_file_errs() {
        let s = Args::parse_from(["tess", "--grep", "x"]);
        let err = build_panes_from_sections(&[s], crate::render::AnsiMode::Interpret, 80, 24, None);
        assert!(err.is_err());
    }

    #[test]
    fn sections_or_flags_rejected() {
        let dir = std::env::temp_dir().join(format!("tess_secor_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let pa = dir.join("a.txt");
        std::fs::write(&pa, b"x\n").unwrap();
        let s = Args::parse_from(["tess", pa.to_str().unwrap(), "--or-grep", "E"]);
        let err = build_panes_from_sections(&[s], crate::render::AnsiMode::Interpret, 80, 24, None);
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
