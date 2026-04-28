use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent};
use crossterm::style::{Print, ResetColor, SetAttribute, Attribute};
use crossterm::terminal::{Clear, ClearType, size};
use crossterm::QueueableCommand;

use crate::error::Result;
use crate::input::{translate, Command};
use crate::line_index::LineIndex;
use crate::render::Cell;
use crate::source::Source;
use crate::viewport::{Frame, RowStyle, SearchDirection, Viewport};

/// Per-keystroke modes the app event loop can be in.
#[derive(Debug, Clone)]
enum InputMode {
    Normal,
    /// User pressed `-`; the next keystroke selects an option to toggle.
    OptionPrefix,
    /// User pressed `/` or `?`; subsequent characters accumulate into a
    /// search pattern until Enter (commit) or Esc (cancel).
    SearchPrompt {
        direction: SearchDirection,
        buffer: String,
        /// If a search compile error occurred, show this in place of the
        /// buffer until the next keystroke.
        error: Option<String>,
    },
}

pub fn run(
    src: Box<dyn Source>,
    mut viewport: Viewport,
    mut idx: LineIndex,
    sigterm: Arc<AtomicBool>,
) -> Result<()> {
    let (mut cols, mut rows) = size().unwrap_or((80, 24));
    viewport.resize(cols, rows);

    let mut stdout = io::stdout();
    let timeout = Duration::from_millis(250);

    // If a filter is active in hide mode, we need to scan the whole source
    // up front to find matching lines. Without a filter this is intentionally
    // skipped — lazy indexing keeps `tess` fast on huge files.
    if viewport.filter_active() && !viewport.dim_mode() {
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

    loop {
        if sigterm.load(Ordering::SeqCst) {
            break;
        }

        if needs_redraw {
            let mut frame = viewport.frame(src.as_ref(), &mut idx);
            // Override the status row when we're in an interactive prompt.
            if let InputMode::SearchPrompt { direction, buffer, error } = &mode {
                let prefix = if matches!(direction, SearchDirection::Forward) { "/" } else { "?" };
                frame.status = match error {
                    Some(e) => format!("{prefix}{buffer}  [error: {e}]"),
                    None => format!("{prefix}{buffer}"),
                };
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
                                        mode = InputMode::Normal;
                                    } else {
                                        match viewport.set_search(buffer.clone(), *direction) {
                                            Ok(()) => {
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
                                _ => {}
                            }
                        }
                        mode = InputMode::Normal;
                        needs_redraw = true;
                        continue;
                    }
                    InputMode::Normal => {}
                }
                let cmd = translate(event);
                match cmd {
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
                    Command::GoTop => {
                        viewport.goto_top();
                        needs_redraw = true;
                    }
                    Command::GoBottom => {
                        viewport.goto_bottom(src.as_ref(), &mut idx);
                        needs_redraw = true;
                    }
                    Command::Refresh => {
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
                    Command::NextMatch => {
                        if viewport.search_repeat(src.as_ref(), &mut idx, false) {
                            needs_redraw = true;
                        }
                    }
                    Command::PreviousMatch => {
                        if viewport.search_repeat(src.as_ref(), &mut idx, true) {
                            needs_redraw = true;
                        }
                    }
                    Command::OptionPrefix => {
                        mode = InputMode::OptionPrefix;
                    }
                    Command::Noop => {}
                }
            }
            Ok(false) => {
                // Timeout — check whether the source has grown.
                if viewport.follow_mode() {
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
