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
use crate::marks::{mark_set, mark_jump, jump_previous, update_prev_position, is_valid_mark_name, MarkTarget};
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
    /// User pressed `:`. The next keystrokes build a colon command in
    /// `buffer`; Enter dispatches, Esc cancels.
    ColonPrompt { buffer: String, error: Option<String> },
    /// User pressed Ctrl-]. The next keystrokes build a tag name in
    /// `buffer`; Enter dispatches, Esc cancels.
    TagPrompt { buffer: String, error: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
enum ColonCommand {
    Next,
    Prev,
    Edit(std::path::PathBuf),
    ShowFile,
    Quit,
    Delete,
    First,
    Last,
    Tag(String),
    TagNext,
    TagPrev,
}

#[derive(Debug, Clone, PartialEq)]
enum ColonParseError {
    UnknownCommand(String),
    MissingPath,
    TagRequiresName,
}

impl std::fmt::Display for ColonParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColonParseError::UnknownCommand(t) => write!(f, "unknown command: :{t}"),
            ColonParseError::MissingPath => write!(f, ":e requires a path"),
            ColonParseError::TagRequiresName => write!(f, ":tag requires a name"),
        }
    }
}

fn parse_colon_command(buf: &str) -> std::result::Result<ColonCommand, ColonParseError> {
    let buf = buf.trim();
    if buf.is_empty() {
        return Err(ColonParseError::UnknownCommand(String::new()));
    }
    let mut parts = buf.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap();
    let rest = parts.next().unwrap_or("").trim();
    match cmd {
        "n" | "next" => Ok(ColonCommand::Next),
        "p" | "prev" => Ok(ColonCommand::Prev),
        "e" | "edit" => {
            if rest.is_empty() {
                Err(ColonParseError::MissingPath)
            } else {
                // Tilde expansion.
                let expanded = if let Some(stripped) = rest.strip_prefix("~/") {
                    if let Some(home) = std::env::var_os("HOME") {
                        let mut p = std::path::PathBuf::from(home);
                        p.push(stripped);
                        p
                    } else {
                        std::path::PathBuf::from(rest)
                    }
                } else {
                    std::path::PathBuf::from(rest)
                };
                Ok(ColonCommand::Edit(expanded))
            }
        }
        "f" => Ok(ColonCommand::ShowFile),
        "q" | "quit" => Ok(ColonCommand::Quit),
        "d" | "delete" => Ok(ColonCommand::Delete),
        "x" | "first" => Ok(ColonCommand::First),
        "t" | "last" => Ok(ColonCommand::Last),
        "tag" => {
            if rest.is_empty() {
                Err(ColonParseError::TagRequiresName)
            } else {
                Ok(ColonCommand::Tag(rest.to_string()))
            }
        }
        "tnext" => Ok(ColonCommand::TagNext),
        "tprev" => Ok(ColonCommand::TagPrev),
        other => Err(ColonParseError::UnknownCommand(other.to_string())),
    }
}

enum ColonOutcome {
    Continue(Option<String>),  // Some(msg) = transient status to show
    Quit,
}

#[derive(Debug, Default)]
struct TagStack {
    /// Where we jumped FROM, in reverse-chronological order. Tuples are
    /// (file_index, top_line) at the time of the jump.
    history: Vec<(usize, usize)>,
    /// Currently-active match list, set when a tag has at least one match
    /// and cleared on Ctrl-T or on a fresh tag jump.
    active: Option<ActiveMatches>,
}

#[derive(Debug, Clone)]
struct ActiveMatches {
    name: String,
    matches: Vec<crate::tags::TagEntry>,
    cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TagStepResult {
    /// Cursor moved; new index is `usize`.
    Moved(usize),
    /// Already at the boundary; show a transient message.
    AtBoundary,
    /// `active` was None — caller should show "no active tag".
    NoActive,
}

impl TagStack {
    fn push(&mut self, file_index: usize, top_line: usize) {
        self.history.push((file_index, top_line));
    }

    fn pop(&mut self) -> Option<(usize, usize)> {
        let popped = self.history.pop();
        if popped.is_some() {
            self.active = None;
        }
        popped
    }

    fn set_active(&mut self, name: String, matches: Vec<crate::tags::TagEntry>) {
        self.active = Some(ActiveMatches {
            name,
            matches,
            cursor: 0,
        });
    }

    fn next(&mut self) -> TagStepResult {
        let Some(a) = &mut self.active else {
            return TagStepResult::NoActive;
        };
        if a.cursor + 1 >= a.matches.len() {
            TagStepResult::AtBoundary
        } else {
            a.cursor += 1;
            TagStepResult::Moved(a.cursor)
        }
    }

    fn prev(&mut self) -> TagStepResult {
        let Some(a) = &mut self.active else {
            return TagStepResult::NoActive;
        };
        if a.cursor == 0 {
            TagStepResult::AtBoundary
        } else {
            a.cursor -= 1;
            TagStepResult::Moved(a.cursor)
        }
    }
}

/// Resolve a tag name to a list of matches, push the current position
/// onto the tag stack, set it as the active match list, and dispatch
/// the first match. Returns a transient status string when something
/// goes wrong, or `None` on success.
#[allow(clippy::too_many_arguments)]
fn dispatch_tag_jump(
    name: &str,
    tag_file: Option<&crate::tags::TagFile>,
    tag_stack: &mut TagStack,
    file_set: &mut crate::file_set::FileSet,
    current_file_index: &mut usize,
    args: &crate::cli::Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    viewport: &mut crate::viewport::Viewport,
    src: &mut Box<dyn crate::source::Source>,
    idx: &mut crate::line_index::LineIndex,
) -> Option<String> {
    let Some(tf) = tag_file else {
        return Some("[no tags file loaded]".into());
    };
    let matches = tf.lookup(name);
    if matches.is_empty() {
        return Some(format!("[tag not found: {name}]"));
    }
    let matches: Vec<crate::tags::TagEntry> = matches.to_vec();
    tag_stack.push(*current_file_index, viewport.top_line());
    tag_stack.set_active(name.to_string(), matches.clone());
    let msg = dispatch_match(
        &matches[0],
        file_set,
        current_file_index,
        args,
        preprocessor,
        record_start_regex,
        viewport,
        src,
        idx,
    );
    update_viewport_tag_indicator(tag_stack, viewport);
    msg
}

#[allow(clippy::too_many_arguments)]
fn dispatch_match(
    entry: &crate::tags::TagEntry,
    file_set: &mut crate::file_set::FileSet,
    current_file_index: &mut usize,
    args: &crate::cli::Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    viewport: &mut crate::viewport::Viewport,
    src: &mut Box<dyn crate::source::Source>,
    idx: &mut crate::line_index::LineIndex,
) -> Option<String> {
    let target_file = entry.file.as_path();
    let already_current = file_set
        .current()
        .map(|p| p == target_file)
        .unwrap_or(false);

    if !already_current {
        let existing_idx = (0..file_set.len()).find(|i| {
            file_set
                .nth(*i)
                .map(|p| p == target_file)
                .unwrap_or(false)
        });
        match existing_idx {
            Some(i) => {
                file_set.set_current_index(i);
            }
            None => {
                file_set.append_and_switch(target_file.to_path_buf());
            }
        }
        let path = file_set.current().unwrap().to_path_buf();
        if let Err(e) = switch_file(
            &path,
            file_set.current_index(),
            file_set.len(),
            args,
            preprocessor,
            viewport,
            src,
            idx,
            record_start_regex,
        ) {
            return Some(format!("[open: {e}]"));
        }
        *current_file_index = file_set.current_index();
    }

    let line = match &entry.address {
        crate::tags::TagAddress::Line(n) => n.saturating_sub(1),
        crate::tags::TagAddress::Pattern(p) => {
            let re_src = crate::tags::pattern_to_regex(p);
            let re = match regex::bytes::Regex::new(&re_src) {
                Ok(r) => r,
                Err(_) => return Some("[tag pattern not found]".into()),
            };
            match find_pattern_line(src.as_ref(), idx, &re) {
                Some(l) => l,
                None => return Some("[tag pattern not found]".into()),
            }
        }
    };

    let clamped = line.min(idx.line_count().saturating_sub(1));
    viewport.goto_line(clamped, src.as_ref(), idx);
    None
}

fn find_pattern_line(
    src: &dyn crate::source::Source,
    idx: &mut crate::line_index::LineIndex,
    re: &regex::bytes::Regex,
) -> Option<usize> {
    idx.extend_to_end(src);
    for line_no in 0..idx.line_count() {
        let range = idx.line_range(line_no, src);
        let bytes = src.bytes(range);
        if re.is_match(&bytes) {
            return Some(line_no);
        }
    }
    None
}

fn update_viewport_tag_indicator(stack: &TagStack, viewport: &mut crate::viewport::Viewport) {
    viewport.set_tag_active(stack.active.as_ref().map(|a| {
        (a.name.clone(), a.cursor + 1, a.matches.len())
    }));
}

#[allow(clippy::too_many_arguments)]
fn switch_file(
    new_path: &std::path::Path,
    new_file_index: usize,
    total_files: usize,
    args: &crate::cli::Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    viewport: &mut crate::viewport::Viewport,
    src: &mut Box<dyn crate::source::Source>,
    idx: &mut crate::line_index::LineIndex,
    record_start_regex: Option<&regex::bytes::Regex>,
) -> crate::error::Result<()> {
    let (new_src, new_label, new_failure) =
        crate::open::open_source_for_path(new_path, args, preprocessor)?;

    *src = new_src;
    let mut new_idx = crate::line_index::LineIndex::new();
    if let Some(re) = record_start_regex {
        new_idx.set_record_start(re.clone());
    }
    *idx = new_idx;

    viewport.set_source_label(new_label);
    viewport.set_file_index(new_file_index, total_files);
    viewport.set_preprocess_failure(new_failure);
    viewport.goto_top();

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn dispatch_colon_command(
    cmd: ColonCommand,
    file_set: &mut crate::file_set::FileSet,
    current_file_index: &mut usize,
    args: &crate::cli::Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    viewport: &mut crate::viewport::Viewport,
    src: &mut Box<dyn crate::source::Source>,
    idx: &mut crate::line_index::LineIndex,
    tag_stack: &mut TagStack,
    tag_file: Option<&crate::tags::TagFile>,
) -> ColonOutcome {
    match cmd {
        ColonCommand::Next => {
            match file_set.next() {
                Ok(path) => {
                    let path = path.to_path_buf();
                    let new_idx_val = file_set.current_index();
                    if let Err(e) = switch_file(&path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
                        ColonOutcome::Continue(Some(format!("[open: {e}]")))
                    } else {
                        *current_file_index = new_idx_val;
                        ColonOutcome::Continue(None)
                    }
                }
                Err(e) => ColonOutcome::Continue(Some(format!("[{e}]"))),
            }
        }
        ColonCommand::Prev => {
            match file_set.prev() {
                Ok(path) => {
                    let path = path.to_path_buf();
                    let new_idx_val = file_set.current_index();
                    if let Err(e) = switch_file(&path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
                        ColonOutcome::Continue(Some(format!("[open: {e}]")))
                    } else {
                        *current_file_index = new_idx_val;
                        ColonOutcome::Continue(None)
                    }
                }
                Err(e) => ColonOutcome::Continue(Some(format!("[{e}]"))),
            }
        }
        ColonCommand::Edit(path) => {
            // Try to open first; if successful, append + switch.
            match crate::open::open_source_for_path(&path, args, preprocessor) {
                Ok(_) => {
                    // Successful open; commit to the FileSet.
                    let final_path = file_set.append_and_switch(path.clone()).to_path_buf();
                    let new_idx_val = file_set.current_index();
                    if let Err(e) = switch_file(&final_path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
                        ColonOutcome::Continue(Some(format!("[open: {e}]")))
                    } else {
                        *current_file_index = new_idx_val;
                        ColonOutcome::Continue(None)
                    }
                }
                Err(e) => ColonOutcome::Continue(Some(format!("[open: {}: {e}]", path.display()))),
            }
        }
        ColonCommand::ShowFile => {
            let label = viewport.source_label_clone();
            let cur = file_set.current_index() + 1;
            let total = file_set.len();
            let top = viewport.top_line() + 1;
            let total_lines = idx.line_count();
            let msg = if total > 1 {
                format!("{label} (file {cur}/{total}): line {top}/{total_lines}")
            } else {
                format!("{label}: line {top}/{total_lines}")
            };
            ColonOutcome::Continue(Some(msg))
        }
        ColonCommand::Quit => ColonOutcome::Quit,
        ColonCommand::Delete => {
            match file_set.delete_current() {
                Ok(path) => {
                    let path = path.to_path_buf();
                    let new_idx_val = file_set.current_index();
                    if let Err(e) = switch_file(&path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
                        ColonOutcome::Continue(Some(format!("[open: {e}]")))
                    } else {
                        *current_file_index = new_idx_val;
                        ColonOutcome::Continue(None)
                    }
                }
                Err(e) => ColonOutcome::Continue(Some(format!("[{e}]"))),
            }
        }
        ColonCommand::First => {
            if file_set.current_index() == 0 {
                ColonOutcome::Continue(None)  // silent no-op
            } else if let Some(path) = file_set.first() {
                let path = path.to_path_buf();
                let new_idx_val = file_set.current_index();
                if let Err(e) = switch_file(&path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
                    ColonOutcome::Continue(Some(format!("[open: {e}]")))
                } else {
                    *current_file_index = new_idx_val;
                    ColonOutcome::Continue(None)
                }
            } else {
                ColonOutcome::Continue(None)
            }
        }
        ColonCommand::Last => {
            if file_set.current_index() + 1 == file_set.len() {
                ColonOutcome::Continue(None)
            } else if let Some(path) = file_set.last() {
                let path = path.to_path_buf();
                let new_idx_val = file_set.current_index();
                if let Err(e) = switch_file(&path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
                    ColonOutcome::Continue(Some(format!("[open: {e}]")))
                } else {
                    *current_file_index = new_idx_val;
                    ColonOutcome::Continue(None)
                }
            } else {
                ColonOutcome::Continue(None)
            }
        }
        ColonCommand::Tag(name) => {
            match dispatch_tag_jump(
                &name,
                tag_file,
                tag_stack,
                file_set,
                current_file_index,
                args,
                preprocessor,
                record_start_regex,
                viewport,
                src,
                idx,
            ) {
                Some(msg) => ColonOutcome::Continue(Some(msg)),
                None => ColonOutcome::Continue(None),
            }
        }
        ColonCommand::TagNext => match tag_stack.next() {
            TagStepResult::NoActive => ColonOutcome::Continue(Some("[no active tag]".into())),
            TagStepResult::AtBoundary => ColonOutcome::Continue(Some("[no more matches]".into())),
            TagStepResult::Moved(cur) => {
                let entry = tag_stack.active.as_ref().unwrap().matches[cur].clone();
                let msg = dispatch_match(
                    &entry,
                    file_set,
                    current_file_index,
                    args,
                    preprocessor,
                    record_start_regex,
                    viewport,
                    src,
                    idx,
                );
                update_viewport_tag_indicator(tag_stack, viewport);
                ColonOutcome::Continue(msg)
            }
        },
        ColonCommand::TagPrev => match tag_stack.prev() {
            TagStepResult::NoActive => ColonOutcome::Continue(Some("[no active tag]".into())),
            TagStepResult::AtBoundary => ColonOutcome::Continue(Some("[at first match]".into())),
            TagStepResult::Moved(cur) => {
                let entry = tag_stack.active.as_ref().unwrap().matches[cur].clone();
                let msg = dispatch_match(
                    &entry,
                    file_set,
                    current_file_index,
                    args,
                    preprocessor,
                    record_start_regex,
                    viewport,
                    src,
                    idx,
                );
                update_viewport_tag_indicator(tag_stack, viewport);
                ColonOutcome::Continue(msg)
            }
        },
    }
}

#[allow(clippy::too_many_arguments, clippy::collapsible_match)]
pub fn run(
    mut src: Box<dyn Source>,
    mut viewport: Viewport,
    mut idx: LineIndex,
    sigterm: Arc<AtomicBool>,
    rebuild_spec: RebuildSpec,
    keymap: crate::keys::KeyMap,
    mut file_set: crate::file_set::FileSet,
    record_start_regex: Option<regex::bytes::Regex>,
    args: crate::cli::Args,
    preprocessor: Option<crate::preprocess::Preprocessor>,
    tag_file: Option<crate::tags::TagFile>,
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
    let mut marks: HashMap<char, (usize, usize)> = HashMap::new();
    let mut previous_position: Option<(usize, usize)> = None;
    let mut current_file_index: usize = file_set.current_index();
    let mut transient_status: Option<String> = None;
    let mut tag_stack = TagStack::default();

    if let Some(tag_name) = args.tag.as_deref() {
        if let Some(msg) = dispatch_tag_jump(
            tag_name,
            tag_file.as_ref(),
            &mut tag_stack,
            &mut file_set,
            &mut current_file_index,
            &args,
            preprocessor.as_ref(),
            record_start_regex.as_ref(),
            &mut viewport,
            &mut src,
            &mut idx,
        ) {
            return Err(crate::error::Error::Runtime(format!("startup tag jump failed: {msg}")));
        }
    }

    loop {
        if sigterm.load(Ordering::SeqCst) {
            break;
        }

        if needs_redraw {
            let mut frame = viewport.frame(src.as_ref(), &mut idx);
            // Override the status row when we're in an interactive prompt OR
            // when a transient status message is pending.
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
                InputMode::ColonPrompt { buffer, error } => {
                    frame.status = match error {
                        Some(e) => format!(":{buffer}  [error: {e}]"),
                        None => format!(":{buffer}"),
                    };
                }
                InputMode::TagPrompt { buffer, error } => {
                    frame.status = match error {
                        Some(e) => format!("tag: {buffer}  [error: {e}]"),
                        None => format!("tag: {buffer}"),
                    };
                }
                _ => {
                    if let Some(msg) = transient_status.take() {
                        frame.status = msg;
                    }
                }
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
                                            update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                                            viewport.search_repeat(src.as_ref(), &mut idx, reverse);
                                        }
                                        mode = InputMode::Normal;
                                    } else {
                                        match viewport.set_search(buffer.clone(), *direction) {
                                            Ok(()) => {
                                                update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
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
                                mark_set(&mut marks, c, current_file_index, viewport.top_line());
                            }
                        }
                        mode = InputMode::Normal;
                        continue;
                    }
                    InputMode::MarkJumpPending => {
                        if let Event::Key(KeyEvent { code: KeyCode::Char(c), .. }) = event {
                            if is_valid_mark_name(c) {
                                match mark_jump(&marks, c, current_file_index, &mut previous_position, viewport.top_line()) {
                                    Some(MarkTarget::SameFile { line }) => {
                                        let clamped = line.min(idx.line_count().saturating_sub(1));
                                        viewport.goto_line(clamped, src.as_ref(), &mut idx);
                                        needs_redraw = true;
                                    }
                                    Some(MarkTarget::OtherFile { file_index, line }) => {
                                        if file_index < file_set.len() {
                                            file_set.set_current_index(file_index);
                                            let path = file_set.current().unwrap().to_path_buf();
                                            if let Err(e) = switch_file(
                                                &path, file_index, file_set.len(),
                                                &args, preprocessor.as_ref(),
                                                &mut viewport, &mut src, &mut idx,
                                                record_start_regex.as_ref(),
                                            ) {
                                                transient_status = Some(format!("[open: {e}]"));
                                            } else {
                                                let clamped = line.min(idx.line_count().saturating_sub(1));
                                                viewport.goto_line(clamped, src.as_ref(), &mut idx);
                                                current_file_index = file_index;
                                                needs_redraw = true;
                                            }
                                        }
                                    }
                                    None => {}
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
                            match jump_previous(&mut previous_position, current_file_index, viewport.top_line()) {
                                Some(MarkTarget::SameFile { line }) => {
                                    let clamped = line.min(idx.line_count().saturating_sub(1));
                                    viewport.goto_line(clamped, src.as_ref(), &mut idx);
                                    needs_redraw = true;
                                }
                                Some(MarkTarget::OtherFile { file_index, line }) => {
                                    if file_index < file_set.len() {
                                        file_set.set_current_index(file_index);
                                        let path = file_set.current().unwrap().to_path_buf();
                                        if let Err(e) = switch_file(
                                            &path, file_index, file_set.len(),
                                            &args, preprocessor.as_ref(),
                                            &mut viewport, &mut src, &mut idx,
                                            record_start_regex.as_ref(),
                                        ) {
                                            transient_status = Some(format!("[open: {e}]"));
                                        } else {
                                            let clamped = line.min(idx.line_count().saturating_sub(1));
                                            viewport.goto_line(clamped, src.as_ref(), &mut idx);
                                            current_file_index = file_index;
                                            needs_redraw = true;
                                        }
                                    }
                                }
                                None => {}
                            }
                            mode = InputMode::Normal;
                            continue;
                        }
                        // Anything else: cancel and fall through to normal dispatch.
                        mode = InputMode::Normal;
                        // Don't `continue` — let the event fall through.
                    }
                    InputMode::ColonPrompt { buffer, error } => {
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
                                        match parse_colon_command(buffer) {
                                            Ok(cmd) => {
                                                let outcome = dispatch_colon_command(
                                                    cmd,
                                                    &mut file_set,
                                                    &mut current_file_index,
                                                    &args,
                                                    preprocessor.as_ref(),
                                                    record_start_regex.as_ref(),
                                                    &mut viewport,
                                                    &mut src,
                                                    &mut idx,
                                                    &mut tag_stack,
                                                    tag_file.as_ref(),
                                                );
                                                match outcome {
                                                    ColonOutcome::Continue(msg) => {
                                                        transient_status = msg;
                                                    }
                                                    ColonOutcome::Quit => break,
                                                }
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
                    InputMode::TagPrompt { buffer, error } => {
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
                                        let name = buffer.clone();
                                        let msg = dispatch_tag_jump(
                                            &name,
                                            tag_file.as_ref(),
                                            &mut tag_stack,
                                            &mut file_set,
                                            &mut current_file_index,
                                            &args,
                                            preprocessor.as_ref(),
                                            record_start_regex.as_ref(),
                                            &mut viewport,
                                            &mut src,
                                            &mut idx,
                                        );
                                        if let Some(m) = msg {
                                            transient_status = Some(m);
                                        }
                                        mode = InputMode::Normal;
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
                    InputMode::Normal => {}
                }
                // Pre-translate keymap interception. Only consult the keymap
                // when in Normal mode (not inside a search/option/prettify/
                // shell prompt).
                let mut cmd: Option<Command> = None;
                if let InputMode::Normal = mode {
                    if let Event::Key(ke) = &event {
                        if let Some(target) = keymap.lookup(ke) {
                            match target {
                                crate::keys::BindingTarget::Shell(cmd_text) => {
                                    let cmd_text = cmd_text.clone();
                                    if let Err(e) = crate::shell::run_shell_command(&cmd_text) {
                                        let _ = writeln!(std::io::stderr(),
                                            "[shell: {e}]");
                                    }
                                    needs_redraw = true;
                                    continue;
                                }
                                crate::keys::BindingTarget::Command(c) => {
                                    cmd = Some(c.clone());
                                }
                            }
                        }
                    }
                }
                let cmd = cmd.unwrap_or_else(|| translate(event));
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
                        update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                        match prefix_at_cmd {
                            Some(line) if line > 0 => viewport.goto_line(line - 1, src.as_ref(), &mut idx),
                            _ => viewport.goto_top(),
                        }
                        needs_redraw = true;
                    }
                    Command::GotoRecord => {
                        update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                        match prefix_at_cmd {
                            Some(rec) if rec > 0 => viewport.goto_record(rec - 1, src.as_ref(), &mut idx),
                            _ => viewport.goto_bottom(src.as_ref(), &mut idx),
                        }
                        needs_redraw = true;
                    }
                    Command::GotoPercent => {
                        update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
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
                    Command::ColonPrompt => {
                        mode = InputMode::ColonPrompt {
                            buffer: String::new(),
                            error: None,
                        };
                        needs_redraw = true;
                    }
                    Command::NextMatch => {
                        update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                        if viewport.search_repeat(src.as_ref(), &mut idx, false) {
                            needs_redraw = true;
                        }
                    }
                    Command::PreviousMatch => {
                        update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
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
                    Command::TagPrompt => {
                        if tag_file.is_none() {
                            transient_status = Some("[no tags file loaded]".into());
                            needs_redraw = true;
                        } else {
                            mode = InputMode::TagPrompt {
                                buffer: String::new(),
                                error: None,
                            };
                            needs_redraw = true;
                        }
                    }
                    Command::TagPop => match tag_stack.pop() {
                        Some((file_index, line)) => {
                            if file_index != current_file_index && file_index < file_set.len() {
                                file_set.set_current_index(file_index);
                                let path = file_set.current().unwrap().to_path_buf();
                                if let Err(e) = switch_file(
                                    &path,
                                    file_index,
                                    file_set.len(),
                                    &args,
                                    preprocessor.as_ref(),
                                    &mut viewport,
                                    &mut src,
                                    &mut idx,
                                    record_start_regex.as_ref(),
                                ) {
                                    transient_status = Some(format!("[open: {e}]"));
                                } else {
                                    current_file_index = file_index;
                                }
                            }
                            let clamped = line.min(idx.line_count().saturating_sub(1));
                            viewport.goto_line(clamped, src.as_ref(), &mut idx);
                            update_viewport_tag_indicator(&tag_stack, &mut viewport);
                            needs_redraw = true;
                        }
                        None => {
                            transient_status = Some("[tag stack empty]".into());
                            needs_redraw = true;
                        }
                    },
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_colon_n() {
        assert_eq!(parse_colon_command("n").unwrap(), ColonCommand::Next);
        assert_eq!(parse_colon_command("next").unwrap(), ColonCommand::Next);
    }

    #[test]
    fn parse_colon_p() {
        assert_eq!(parse_colon_command("p").unwrap(), ColonCommand::Prev);
        assert_eq!(parse_colon_command("prev").unwrap(), ColonCommand::Prev);
    }

    #[test]
    fn parse_colon_e_with_path() {
        match parse_colon_command("e /tmp/foo.log").unwrap() {
            ColonCommand::Edit(p) => assert_eq!(p, std::path::PathBuf::from("/tmp/foo.log")),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn parse_colon_e_with_tilde() {
        std::env::set_var("HOME", "/home/user");
        match parse_colon_command("e ~/foo.log").unwrap() {
            ColonCommand::Edit(p) => assert_eq!(p, std::path::PathBuf::from("/home/user/foo.log")),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn parse_colon_e_missing_path_errors() {
        assert_eq!(parse_colon_command("e").unwrap_err(), ColonParseError::MissingPath);
        assert_eq!(parse_colon_command("e ").unwrap_err(), ColonParseError::MissingPath);
    }

    #[test]
    fn parse_colon_f_q_d_x_t() {
        assert_eq!(parse_colon_command("f").unwrap(), ColonCommand::ShowFile);
        assert_eq!(parse_colon_command("q").unwrap(), ColonCommand::Quit);
        assert_eq!(parse_colon_command("d").unwrap(), ColonCommand::Delete);
        assert_eq!(parse_colon_command("x").unwrap(), ColonCommand::First);
        assert_eq!(parse_colon_command("t").unwrap(), ColonCommand::Last);
    }

    #[test]
    fn parse_unknown_command_errors() {
        let err = parse_colon_command("bogus").unwrap_err();
        match err {
            ColonParseError::UnknownCommand(name) => assert_eq!(name, "bogus"),
            other => panic!("expected UnknownCommand, got {other:?}"),
        }
    }

    #[test]
    fn parse_handles_whitespace() {
        // Trailing whitespace OK.
        assert_eq!(parse_colon_command("n  ").unwrap(), ColonCommand::Next);
        assert_eq!(parse_colon_command("  n").unwrap(), ColonCommand::Next);
    }

    #[test]
    fn parse_colon_tag_with_name() {
        assert_eq!(
            parse_colon_command("tag foo").unwrap(),
            ColonCommand::Tag("foo".into())
        );
    }

    #[test]
    fn parse_colon_tag_strips_trailing_whitespace() {
        assert_eq!(
            parse_colon_command("tag foo  ").unwrap(),
            ColonCommand::Tag("foo".into())
        );
    }

    #[test]
    fn parse_colon_tag_without_name_errors() {
        assert_eq!(
            parse_colon_command("tag").unwrap_err(),
            ColonParseError::TagRequiresName
        );
        assert_eq!(
            parse_colon_command("tag  ").unwrap_err(),
            ColonParseError::TagRequiresName
        );
    }

    #[test]
    fn parse_colon_tnext_and_tprev() {
        assert_eq!(parse_colon_command("tnext").unwrap(), ColonCommand::TagNext);
        assert_eq!(parse_colon_command("tprev").unwrap(), ColonCommand::TagPrev);
    }

    #[test]
    fn tag_stack_push_pop_lifo() {
        let mut s = TagStack::default();
        s.push(0, 10);
        s.push(1, 20);
        assert_eq!(s.pop(), Some((1, 20)));
        assert_eq!(s.pop(), Some((0, 10)));
        assert_eq!(s.pop(), None);
    }

    #[test]
    fn tag_stack_pop_clears_active() {
        let mut s = TagStack::default();
        s.push(0, 10);
        s.set_active(
            "foo".into(),
            vec![crate::tags::TagEntry {
                file: std::path::PathBuf::from("/a"),
                address: crate::tags::TagAddress::Line(1),
            }],
        );
        assert!(s.active.is_some());
        let _ = s.pop();
        assert!(s.active.is_none());
    }

    #[test]
    fn tag_stack_next_advances_then_clamps() {
        let mut s = TagStack::default();
        s.set_active(
            "foo".into(),
            vec![
                crate::tags::TagEntry {
                    file: std::path::PathBuf::from("/a"),
                    address: crate::tags::TagAddress::Line(1),
                },
                crate::tags::TagEntry {
                    file: std::path::PathBuf::from("/b"),
                    address: crate::tags::TagAddress::Line(2),
                },
            ],
        );
        assert_eq!(s.next(), TagStepResult::Moved(1));
        assert_eq!(s.next(), TagStepResult::AtBoundary);
    }

    #[test]
    fn tag_stack_prev_clamps_at_zero() {
        let mut s = TagStack::default();
        s.set_active(
            "foo".into(),
            vec![crate::tags::TagEntry {
                file: std::path::PathBuf::from("/a"),
                address: crate::tags::TagAddress::Line(1),
            }],
        );
        assert_eq!(s.prev(), TagStepResult::AtBoundary);
    }

    #[test]
    fn tag_stack_next_with_no_active_returns_no_active() {
        let mut s = TagStack::default();
        assert_eq!(s.next(), TagStepResult::NoActive);
        assert_eq!(s.prev(), TagStepResult::NoActive);
    }

    #[test]
    fn tag_stack_set_active_replaces_previous_list() {
        let mut s = TagStack::default();
        s.set_active(
            "foo".into(),
            vec![crate::tags::TagEntry {
                file: std::path::PathBuf::from("/a"),
                address: crate::tags::TagAddress::Line(1),
            }],
        );
        s.set_active(
            "bar".into(),
            vec![
                crate::tags::TagEntry {
                    file: std::path::PathBuf::from("/x"),
                    address: crate::tags::TagAddress::Line(5),
                },
                crate::tags::TagEntry {
                    file: std::path::PathBuf::from("/y"),
                    address: crate::tags::TagAddress::Line(6),
                },
            ],
        );
        let active = s.active.as_ref().unwrap();
        assert_eq!(active.name, "bar");
        assert_eq!(active.matches.len(), 2);
        assert_eq!(active.cursor, 0);
    }
}
