use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::{Print, ResetColor, SetAttribute, Attribute};
use crossterm::terminal::{Clear, ClearType, size};
use crossterm::QueueableCommand;

use crate::error::Result;
use crate::input::{translate, Command};
use crate::marks::{mark_set, mark_jump, jump_previous, update_prev_position, is_valid_mark_name};
use crate::line_index::LineIndex;
use crate::prettify::PrettifyMode;
use crate::render::Cell;
use crate::source::{find_tail_offset, Source};
use crate::viewport::{Frame, RowStyle, SearchDirection, Viewport};

/// Constraints to re-apply when the source content has been replaced wholesale
/// (`--live`). The line index is rebuilt from scratch each time, so caps that
/// were originally honored at startup need to be reasserted.
#[derive(Default, Clone, Copy)]
pub struct RebuildSpec {
    pub head: Option<usize>,
    pub tail: Option<usize>,
}

/// Per-keystroke modes the app event loop can be in.
#[derive(Debug, Clone)]
enum InputMode {
    Normal,
    /// User pressed `-`; the next keystroke selects an option to toggle.
    OptionPrefix,
    /// User pressed `-P`; the next keystroke chooses a prettify mode
    /// (`j`/`y`/`t`/`x`/`h`/`c`/`a`/`r`).
    PrettifyPrefix,
    /// User pressed `/` or `?`; subsequent characters accumulate into a
    /// search pattern until Enter (commit) or Esc (cancel).
    SearchPrompt {
        direction: SearchDirection,
        buffer: String,
        /// If a search compile error occurred, show this in place of the
        /// buffer until the next keystroke.
        error: Option<String>,
    },
    /// User pressed `!`. The next keystrokes build a shell command in
    /// `buffer`; Enter executes via shell::run_shell_command, Esc cancels.
    ShellPrompt { buffer: String, error: Option<String> },
    /// Set-mark prefix: the next keystroke names the mark to set.
    MarkSetPending,
    /// Jump-to-mark prefix: the next keystroke names the mark to jump to.
    MarkJumpPending,
    /// First half of the Ctrl-X Ctrl-X chord.
    CtrlXPending,
}

pub fn run(
    src: Box<dyn Source>,
    mut viewport: Viewport,
    mut idx: LineIndex,
    sigterm: Arc<AtomicBool>,
    rebuild_spec: RebuildSpec,
) -> Result<()> {
    let (mut cols, mut rows) = size().unwrap_or((80, 24));
    viewport.resize(cols, rows);

    let mut stdout = io::stdout();
    let timeout = Duration::from_millis(250);
    let mut last_revision = src.revision();

    // If hide-mode filtering is active (--filter or --grep without --dim),
    // we need to scan the whole source up front to find matching lines.
    // Without any predicate this is intentionally skipped — lazy indexing
    // keeps `tess` fast on huge files.
    if (viewport.filter_active() || viewport.grep_active()) && !viewport.dim_mode() {
        idx.extend_to_end(src.as_ref());
        viewport.extend_visible_lines(&idx, src.as_ref());
    }

    // If follow mode is on at startup, snap to the bottom of the (possibly
    // filtered) source so the user sees the newest content (tail-style).
    if viewport.follow_mode() {
        src.pump();
        viewport.extend_visible_lines(&idx, src.as_ref());
        viewport.goto_bottom(src.as_ref(), &mut idx);
    }

    // Always draw the initial frame before entering the event loop.
    let mut needs_redraw = true;
    let mut mode = InputMode::Normal;
    let mut numeric_prefix: Option<usize> = None;
    let mut marks: HashMap<char, usize> = HashMap::new();
    let mut previous_position: Option<usize> = None;

    loop {
        if sigterm.load(Ordering::SeqCst) {
            break;
        }

        if needs_redraw {
            let mut frame = viewport.frame(src.as_ref(), &mut idx);
            // Override the status row when we're in an interactive prompt.
            match &mode {
                InputMode::SearchPrompt { direction, buffer, error } => {
                    let prefix = if matches!(direction, SearchDirection::Forward) { "/" } else { "?" };
                    frame.status = match error {
                        Some(e) => format!("{prefix}{buffer}  [error: {e}]"),
                        None => format!("{prefix}{buffer}"),
                    };
                }
                InputMode::ShellPrompt { buffer, error } => {
                    frame.status = match error {
                        Some(e) => format!("!{buffer}  [error: {e}]"),
                        None => format!("!{buffer}"),
                    };
                }
                _ => {}
            }
            write_frame(&mut stdout, &frame, cols, rows)
                .map_err(|e| crate::error::Error::Runtime(format!("stdout: {}", e)))?;
            needs_redraw = false;
        }

        // Poll with timeout so stdin sources can be re-checked.
        match poll(timeout) {
            Ok(true) => {
                let event = read().map_err(|e| crate::error::Error::Runtime(format!("input: {}", e)))?;
                // Modal input handling: the search prompt and option prefix
                // intercept keys before they're translated to commands.
                match &mut mode {
                    InputMode::SearchPrompt { direction, buffer, error } => {
                        if let Event::Key(KeyEvent { code, .. }) = event {
                            match code {
                                KeyCode::Esc => { mode = InputMode::Normal; needs_redraw = true; }
                                KeyCode::Enter => {
                                    if buffer.is_empty() {
                                        // Empty buffer: repeat the last search in the
                                        // newly-typed direction (less compat). If no
                                        // prior search exists, just dismiss.
                                        if viewport.search_active() {
                                            let reverse = !matches!(
                                                (viewport.search_direction(), *direction),
                                                (SearchDirection::Forward, SearchDirection::Forward)
                                                | (SearchDirection::Backward, SearchDirection::Backward)
                                            );
                                            update_prev_position(&mut previous_position, viewport.top_line());
                                            viewport.search_repeat(src.as_ref(), &mut idx, reverse);
                                        }
                                        mode = InputMode::Normal;
                                    } else {
                                        match viewport.set_search(buffer.clone(), *direction) {
                                            Ok(()) => {
                                                update_prev_position(&mut previous_position, viewport.top_line());
                                                viewport.search_repeat(src.as_ref(), &mut idx, false);
                                                mode = InputMode::Normal;
                                            }
                                            Err(e) => { *error = Some(e); }
                                        }
                                    }
                                    needs_redraw = true;
                                }
                                KeyCode::Backspace => {
                                    buffer.pop();
                                    *error = None;
                                    needs_redraw = true;
                                }
                                KeyCode::Char(c) => {
                                    buffer.push(c);
                                    *error = None;
                                    needs_redraw = true;
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                    InputMode::OptionPrefix => {
                        if let Event::Key(KeyEvent { code, .. }) = event {
                            match code {
                                KeyCode::Char('N') | KeyCode::Char('n') => viewport.toggle_line_numbers(),
                                KeyCode::Char('S') | KeyCode::Char('s') => viewport.toggle_chop(),
                                KeyCode::Char('F') | KeyCode::Char('f') => viewport.toggle_follow(),
                                KeyCode::Char('P') | KeyCode::Char('p') => {
                                    // Two-key prefix: `-P` then a letter for the mode.
                                    mode = InputMode::PrettifyPrefix;
                                    needs_redraw = true;
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        mode = InputMode::Normal;
                        needs_redraw = true;
                        continue;
                    }
                    InputMode::PrettifyPrefix => {
                        if let Event::Key(KeyEvent { code, .. }) = event {
                            let target: Option<PrettifyTarget> = match code {
                                KeyCode::Char('j') | KeyCode::Char('J') => Some(PrettifyTarget::Mode(PrettifyMode::Json)),
                                KeyCode::Char('y') | KeyCode::Char('Y') => Some(PrettifyTarget::Mode(PrettifyMode::Yaml)),
                                KeyCode::Char('t') | KeyCode::Char('T') => Some(PrettifyTarget::Mode(PrettifyMode::Toml)),
                                KeyCode::Char('x') | KeyCode::Char('X') => Some(PrettifyTarget::Mode(PrettifyMode::Xml)),
                                KeyCode::Char('h') | KeyCode::Char('H') => Some(PrettifyTarget::Mode(PrettifyMode::Html)),
                                KeyCode::Char('c') | KeyCode::Char('C') => Some(PrettifyTarget::Mode(PrettifyMode::Csv)),
                                KeyCode::Char('r') | KeyCode::Char('R') => Some(PrettifyTarget::Mode(PrettifyMode::Off)),
                                KeyCode::Char('a') | KeyCode::Char('A') => Some(PrettifyTarget::Auto),
                                _ => None,
                            };
                            if let Some(t) = target {
                                apply_prettify(
                                    src.as_ref(),
                                    &mut viewport,
                                    &mut idx,
                                    rebuild_spec,
                                    t,
                                );
                                last_revision = src.revision();
                            }
                        }
                        mode = InputMode::Normal;
                        needs_redraw = true;
                        continue;
                    }
                    InputMode::MarkSetPending => {
                        if let Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) = event {
                            if is_valid_mark_name(c) {
                                mark_set(&mut marks, c, viewport.top_line());
                            }
                        }
                        mode = InputMode::Normal;
                        continue;
                    }
                    InputMode::MarkJumpPending => {
                        if let Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) = event {
                            if is_valid_mark_name(c) {
                                if let Some(line) = mark_jump(
                                    &marks, c, idx.line_count(),
                                    &mut previous_position, viewport.top_line(),
                                ) {
                                    viewport.goto_line(line, src.as_ref(), &mut idx);
                                    needs_redraw = true;
                                }
                            }
                        }
                        mode = InputMode::Normal;
                        continue;
                    }
                    InputMode::ShellPrompt { buffer, error } => {
                        if let Event::Key(KeyEvent { code, .. }) = event {
                            match code {
                                KeyCode::Esc => {
                                    mode = InputMode::Normal;
                                    needs_redraw = true;
                                }
                                KeyCode::Enter => {
                                    if buffer.is_empty() {
                                        mode = InputMode::Normal;
                                    } else {
                                        match crate::shell::run_shell_command(buffer) {
                                            Ok(()) => {
                                                mode = InputMode::Normal;
                                            }
                                            Err(e) => {
                                                *error = Some(e.to_string());
                                            }
                                        }
                                    }
                                    needs_redraw = true;
                                }
                                KeyCode::Backspace => {
                                    buffer.pop();
                                    *error = None;
                                    needs_redraw = true;
                                }
                                KeyCode::Char(c) => {
                                    buffer.push(c);
                                    *error = None;
                                    needs_redraw = true;
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                    InputMode::CtrlXPending => {
                        let is_ctrl_x = matches!(
                            event,
                            Event::Key(KeyEvent {
                                code: KeyCode::Char('x'),
                                modifiers: KeyModifiers::CONTROL,
                                ..
                            })
                        );
                        if is_ctrl_x {
                            if let Some(line) = jump_previous(
                                &mut previous_position, viewport.top_line(),
                            ) {
                                let clamped = line.min(idx.line_count().saturating_sub(1));
                                viewport.goto_line(clamped, src.as_ref(), &mut idx);
                                needs_redraw = true;
                            }
                            mode = InputMode::Normal;
                            continue;
                        }
                        // Anything else: cancel and fall through to normal dispatch.
                        mode = InputMode::Normal;
                        // Don't `continue` — let the event fall through.
                    }
                    InputMode::Normal => {}
                }
                let cmd = translate(event);
                // Consume the numeric prefix at the top of each dispatch so
                // commands that don't need it drop it implicitly.
                let prefix_at_cmd = numeric_prefix.take();
                match cmd {
                    Command::Digit(d) => {
                        let cur = prefix_at_cmd.unwrap_or(0);
                        let next = cur.saturating_mul(10).saturating_add(d as usize);
                        if next <= 99_999_999 {
                            numeric_prefix = Some(next);
                        } else {
                            // Overflow: keep previous prefix, ignore this digit.
                            numeric_prefix = prefix_at_cmd;
                        }
                        continue;
                    }
                    Command::Cancel => {
                        // prefix_at_cmd already consumed; nothing else to do.
                        continue;
                    }
                    Command::GotoLine => {
                        update_prev_position(&mut previous_position, viewport.top_line());
                        match prefix_at_cmd {
                            Some(line) if line > 0 => viewport.goto_line(line - 1, src.as_ref(), &mut idx),
                            _ => viewport.goto_top(),
                        }
                        needs_redraw = true;
                    }
                    Command::GotoRecord => {
                        update_prev_position(&mut previous_position, viewport.top_line());
                        match prefix_at_cmd {
                            Some(rec) if rec > 0 => viewport.goto_record(rec - 1, src.as_ref(), &mut idx),
                            _ => viewport.goto_bottom(src.as_ref(), &mut idx),
                        }
                        needs_redraw = true;
                    }
                    Command::GotoPercent => {
                        update_prev_position(&mut previous_position, viewport.top_line());
                        match prefix_at_cmd {
                            Some(p) if p <= 100 => viewport.goto_percent(p as u8, src.as_ref(), &mut idx),
                            _ => viewport.goto_top(),
                        }
                        needs_redraw = true;
                    }
                    Command::Quit => break,
                    Command::Resize(c, r) => {
                        cols = c; rows = r;
                        viewport.resize(c, r);
                        needs_redraw = true;
                    }
                    Command::ScrollLines(n) => {
                        viewport.scroll_lines(n, src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::ScrollLogicalLines(n) => {
                        viewport.scroll_logical_lines(n, src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::PageDown => {
                        viewport.page_down(src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::PageUp => {
                        viewport.page_up(src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::HalfPageDown => {
                        viewport.half_page_down(src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::HalfPageUp => {
                        viewport.half_page_up(src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::Refresh => {
                        needs_redraw = true;
                    }
                    Command::Reload => {
                        // Force a stat+reread now (only meaningful for live
                        // sources; static FileSource::pump() is a no-op).
                        src.pump();
                        if src.revision() != last_revision {
                            rebuild_after_replace(
                                src.as_ref(), &mut viewport, &mut idx, rebuild_spec,
                            );
                            last_revision = src.revision();
                            needs_redraw = true;
                        }
                    }
                    Command::TogglePrettify => {
                        apply_prettify(
                            src.as_ref(), &mut viewport, &mut idx, rebuild_spec,
                            PrettifyTarget::Toggle,
                        );
                        last_revision = src.revision();
                        needs_redraw = true;
                    }
                    Command::SetPrettifyMode(m) => {
                        apply_prettify(
                            src.as_ref(), &mut viewport, &mut idx, rebuild_spec,
                            PrettifyTarget::Mode(m),
                        );
                        last_revision = src.revision();
                        needs_redraw = true;
                    }
                    Command::RedetectPrettify => {
                        apply_prettify(
                            src.as_ref(), &mut viewport, &mut idx, rebuild_spec,
                            PrettifyTarget::Auto,
                        );
                        last_revision = src.revision();
                        needs_redraw = true;
                    }
                    Command::ToggleLineNumbers => {
                        viewport.toggle_line_numbers();
                        needs_redraw = true;
                    }
                    Command::ToggleChop => {
                        viewport.toggle_chop();
                        needs_redraw = true;
                    }
                    Command::ToggleFollow => {
                        viewport.toggle_follow();
                        if viewport.follow_mode() {
                            // Re-engaging: pump any pending bytes and snap to bottom.
                            src.pump();
                            idx.notice_new_bytes(src.as_ref());
                            viewport.goto_bottom(src.as_ref(), &mut idx);
                        }
                        needs_redraw = true;
                    }
                    Command::SearchForward => {
                        mode = InputMode::SearchPrompt {
                            direction: SearchDirection::Forward,
                            buffer: String::new(),
                            error: None,
                        };
                        needs_redraw = true;
                    }
                    Command::SearchBackward => {
                        mode = InputMode::SearchPrompt {
                            direction: SearchDirection::Backward,
                            buffer: String::new(),
                            error: None,
                        };
                        needs_redraw = true;
                    }
                    Command::ShellEscape => {
                        mode = InputMode::ShellPrompt {
                            buffer: String::new(),
                            error: None,
                        };
                        needs_redraw = true;
                    }
                    Command::NextMatch => {
                        update_prev_position(&mut previous_position, viewport.top_line());
                        if viewport.search_repeat(src.as_ref(), &mut idx, false) {
                            needs_redraw = true;
                        }
                    }
                    Command::PreviousMatch => {
                        update_prev_position(&mut previous_position, viewport.top_line());
                        if viewport.search_repeat(src.as_ref(), &mut idx, true) {
                            needs_redraw = true;
                        }
                    }
                    Command::OptionPrefix => {
                        mode = InputMode::OptionPrefix;
                    }
                    Command::MarkSet => {
                        mode = InputMode::MarkSetPending;
                    }
                    Command::MarkJump => {
                        mode = InputMode::MarkJumpPending;
                    }
                    Command::CtrlXPrefix => {
                        mode = InputMode::CtrlXPending;
                    }
                    Command::JumpPrevious => {
                        // Resolved inside the CtrlXPending mode intercept; this
                        // arm is defensive and should never fire.
                    }
                    Command::Noop => {}
                }
            }
            Ok(false) => {
                // Timeout — check whether the source has grown or been rewritten.
                if viewport.live_mode() {
                    let was_at_bottom = viewport.is_at_bottom(&idx);
                    src.pump();
                    if src.revision() != last_revision {
                        rebuild_after_replace(
                            src.as_ref(), &mut viewport, &mut idx, rebuild_spec,
                        );
                        if was_at_bottom {
                            viewport.goto_bottom(src.as_ref(), &mut idx);
                        }
                        last_revision = src.revision();
                        needs_redraw = true;
                    }
                } else if viewport.follow_mode() {
                    let was_at_bottom = viewport.is_at_bottom(&idx);
                    src.pump();
                    let lines_before = idx.line_count();
                    idx.notice_new_bytes(src.as_ref());
                    viewport.extend_visible_lines(&idx, src.as_ref());
                    if idx.line_count() != lines_before {
                        needs_redraw = true;
                        if was_at_bottom {
                            viewport.goto_bottom(src.as_ref(), &mut idx);
                        }
                    }
                } else if !src.is_complete() {
                    // Streaming stdin without follow mode: still keep the index
                    // up-to-date so line counts stay accurate, but don't auto-scroll.
                    let lines_before = idx.line_count();
                    idx.notice_new_bytes(src.as_ref());
                    viewport.extend_visible_lines(&idx, src.as_ref());
                    if idx.line_count() != lines_before {
                        needs_redraw = true;
                    }
                }
            }
            Err(_) => {
                // poll() error — sleep the timeout duration to avoid tight-spinning.
                std::thread::sleep(timeout);
            }
        }
    }
    Ok(())
}

/// What `apply_prettify` should do to the source's prettify state.
#[derive(Debug, Clone, Copy)]
enum PrettifyTarget {
    /// Set a specific mode (including `Off` for "raw").
    Mode(PrettifyMode),
    /// Flip between current mode and last-active mode.
    Toggle,
    /// Re-run byte-based content detection and apply the result.
    Auto,
}

/// Apply a prettify-state change to the source and propagate any visible
/// effects (line index rebuild, viewport label, scroll clamp). No-op if the
/// source isn't a `TransformingSource` (i.e. `prettify_mode()` is `None`).
fn apply_prettify(
    src: &dyn Source,
    viewport: &mut Viewport,
    idx: &mut LineIndex,
    spec: RebuildSpec,
    target: PrettifyTarget,
) {
    // Sources without a wrapper return None — nothing to do.
    if src.prettify_mode().is_none() {
        return;
    }
    match target {
        PrettifyTarget::Mode(m) => src.set_prettify_mode(m),
        PrettifyTarget::Toggle => src.toggle_prettify(),
        PrettifyTarget::Auto => src.redetect_prettify(),
    }
    rebuild_after_replace(src, viewport, idx, spec);
    viewport.set_prettify_label(src.prettify_label());
}

/// Rebuild line index and visible-line cache after the source content has
/// been replaced wholesale (e.g. an editor saved over the file). Re-applies
/// `--head`/`--tail` caps from the original CLI args; clamps `top_line` so the
/// user stays roughly where they were rather than jumping. Auto snap-to-bottom
/// (when the user *was* at the bottom) is the caller's responsibility.
fn rebuild_after_replace(
    src: &dyn Source,
    viewport: &mut Viewport,
    idx: &mut LineIndex,
    spec: RebuildSpec,
) {
    let new_off = match spec.tail {
        Some(n) => find_tail_offset(src, n),
        None => 0,
    };
    *idx = LineIndex::new_starting_at(new_off);
    if let Some(n) = spec.head {
        idx.set_head_cap(n);
    }
    viewport.invalidate_filter_cache();
    idx.notice_new_bytes(src);
    viewport.extend_visible_lines(idx, src);
    viewport.clamp_top_line(idx.line_count());
}

fn write_frame(out: &mut impl Write, frame: &Frame, cols: u16, rows: u16) -> io::Result<()> {
    // Reset attributes once before clear so the cleared cells inherit a
    // clean state (some terminals fill cleared cells with the current
    // attribute, which caused reverse-video bleed in earlier versions).
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(ResetColor)?;
    out.queue(Clear(ClearType::All))?;
    for (i, row) in frame.body.iter().enumerate() {
        out.queue(MoveTo(0, i as u16))?;
        // Defensive: every row begins with a full attribute reset, so a
        // mis-handled reset on the previous row can't bleed forward.
        out.queue(SetAttribute(Attribute::Reset))?;
        let style = frame.row_styles.get(i).copied().unwrap_or(RowStyle::Normal);
        if matches!(style, RowStyle::Dim) {
            out.queue(SetAttribute(Attribute::Dim))?;
        }
        let no_highlights = Vec::new();
        let highlights = frame.highlights.get(i).unwrap_or(&no_highlights);
        write_row_with_highlights(out, row, cols, highlights)?;
        out.queue(SetAttribute(Attribute::Reset))?;
    }
    // Status row
    out.queue(MoveTo(0, rows.saturating_sub(1)))?;
    out.queue(SetAttribute(Attribute::Reverse))?;
    let mut status = frame.status.clone();
    if status.len() > cols as usize {
        status.truncate(cols as usize);
    } else {
        let pad = cols as usize - status.len();
        status.push_str(&" ".repeat(pad));
    }
    out.queue(Print(status))?;
    out.queue(ResetColor)?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.flush()
}

fn cells_to_string(row: &[Cell], cols: u16) -> String {
    let mut s = String::with_capacity(cols as usize);
    for cell in row.iter().take(cols as usize) {
        match cell {
            Cell::Char { ch, .. } => s.push(*ch),
            Cell::Continuation => { /* width-2 char already pushed */ }
            Cell::Empty => s.push(' '),
        }
    }
    s
}

/// Emit a single row with per-substring reverse-video highlights. Highlight
/// ranges are in cell columns; any segment outside a highlight prints with
/// the row's already-applied base attribute. Reverse is toggled on/off
/// segment-by-segment with explicit `NoReverse` so a base attribute like
/// `Dim` stays in effect for un-highlighted text.
fn write_row_with_highlights(
    out: &mut impl Write,
    row: &[Cell],
    cols: u16,
    highlights: &[std::ops::Range<usize>],
) -> io::Result<()> {
    let cols_usize = cols as usize;
    if highlights.is_empty() {
        out.queue(Print(cells_to_string(row, cols)))?;
        return Ok(());
    }
    // Sort and clamp; assume non-overlapping (viewport produces them this way).
    let mut ranges: Vec<std::ops::Range<usize>> = highlights
        .iter()
        .filter_map(|r| {
            let s = r.start.min(cols_usize);
            let e = r.end.min(cols_usize);
            if e > s { Some(s..e) } else { None }
        })
        .collect();
    ranges.sort_by_key(|r| r.start);

    let mut col = 0usize;
    let mut i = 0usize;
    while col < cols_usize && i < row.len() {
        // Find which range (if any) covers this column.
        let active = ranges.iter().find(|r| r.start <= col && col < r.end);
        let (segment_end, reversed) = match active {
            Some(r) => (r.end.min(cols_usize), true),
            None => {
                // Plain segment until the next highlight or row end.
                let next = ranges.iter().find(|r| r.start > col).map(|r| r.start);
                (next.unwrap_or(cols_usize), false)
            }
        };
        if reversed { out.queue(SetAttribute(Attribute::Reverse))?; }
        // Collect cells for this segment from `col` to `segment_end`.
        let mut s = String::new();
        while col < segment_end && i < row.len() {
            match &row[i] {
                Cell::Char { ch, width } => {
                    s.push(*ch);
                    col += *width as usize;
                }
                Cell::Continuation => {
                    // Already accounted for by the preceding wide char's width.
                }
                Cell::Empty => {
                    s.push(' ');
                    col += 1;
                }
            }
            i += 1;
        }
        out.queue(Print(s))?;
        if reversed { out.queue(SetAttribute(Attribute::NoReverse))?; }
    }
    Ok(())
}
