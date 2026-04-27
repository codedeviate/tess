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

pub fn run(src: Box<dyn Source>, mut viewport: Viewport, sigterm: Arc<AtomicBool>) -> Result<()> {
    let mut idx = LineIndex::new();
    let (mut cols, mut rows) = size().unwrap_or((80, 24));
    viewport.resize(cols, rows);

    let mut stdout = io::stdout();

    loop {
        if sigterm.load(Ordering::SeqCst) {
            break;
        }

        let frame = viewport.frame(src.as_ref(), &mut idx);
        write_frame(&mut stdout, &frame, cols, rows)
            .map_err(|e| crate::error::Error::Runtime(format!("stdout: {}", e)))?;

        // Poll with timeout so stdin sources can be re-checked.
        let has_event = poll(Duration::from_millis(50)).unwrap_or(false);
        if has_event {
            let event = read().map_err(|e| crate::error::Error::Runtime(format!("input: {}", e)))?;
            let cmd = translate(event);
            match cmd {
                Command::Quit => break,
                Command::Resize(c, r) => {
                    cols = c; rows = r;
                    viewport.resize(c, r);
                }
                Command::ScrollLines(n) => viewport.scroll_lines(n, src.as_ref(), &mut idx),
                Command::PageDown => viewport.page_down(src.as_ref(), &mut idx),
                Command::PageUp => viewport.page_up(src.as_ref(), &mut idx),
                Command::HalfPageDown => viewport.half_page_down(src.as_ref(), &mut idx),
                Command::HalfPageUp => viewport.half_page_up(src.as_ref(), &mut idx),
                Command::GoTop => viewport.goto_top(),
                Command::GoBottom => viewport.goto_bottom(src.as_ref(), &mut idx),
                Command::Refresh => { /* re-render on next loop */ }
                Command::ToggleLineNumbers => viewport.toggle_line_numbers(),
                Command::ToggleChop => viewport.toggle_chop(),
                Command::Noop => {}
            }
        } else if !src.is_complete() {
            idx.notice_new_bytes(src.as_ref());
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
