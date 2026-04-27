use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{poll, read};
use crossterm::style::{Print, ResetColor, SetAttribute, Attribute};
use crossterm::terminal::{Clear, ClearType, size};
use crossterm::QueueableCommand;

use crate::error::Result;
use crate::input::{translate, Command};
use crate::line_index::LineIndex;
use crate::render::Cell;
use crate::source::Source;
use crate::viewport::{Frame, Viewport};

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

    // If follow mode is on at startup, snap to the bottom of the source so
    // the user sees the newest content (tail-style).
    if viewport.follow_mode() {
        src.pump();
        viewport.goto_bottom(src.as_ref(), &mut idx);
    }

    // Always draw the initial frame before entering the event loop.
    let mut needs_redraw = true;

    loop {
        if sigterm.load(Ordering::SeqCst) {
            break;
        }

        if needs_redraw {
            let frame = viewport.frame(src.as_ref(), &mut idx);
            write_frame(&mut stdout, &frame, cols, rows)
                .map_err(|e| crate::error::Error::Runtime(format!("stdout: {}", e)))?;
            needs_redraw = false;
        }

        // Poll with timeout so stdin sources can be re-checked.
        match poll(timeout) {
            Ok(true) => {
                let event = read().map_err(|e| crate::error::Error::Runtime(format!("input: {}", e)))?;
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
    out.queue(Clear(ClearType::All))?;
    for (i, row) in frame.body.iter().enumerate() {
        out.queue(MoveTo(0, i as u16))?;
        out.queue(Print(cells_to_string(row, cols)))?;
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
