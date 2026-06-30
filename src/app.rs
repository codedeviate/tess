use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::{Print, ResetColor, SetAttribute, SetForegroundColor, SetBackgroundColor, Attribute};
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

/// Split orientation: vertical (side-by-side, today's default) or horizontal
/// (stacked). Threaded through pane sizing, frame composition, and mouse
/// routing so the same N-pane machinery serves both layouts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation { Vertical, Horizontal }

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
    /// User pressed `]`; the next keystroke `c` triggers DiffNextChange.
    BracketClosePending,
    /// User pressed `[`; the next keystroke `c` triggers DiffPrevChange.
    BracketOpenPending,
    /// User pressed `:`. The next keystrokes build a colon command in
    /// `buffer`; Enter dispatches, Esc cancels.
    ColonPrompt { buffer: String, error: Option<String> },
    /// User pressed Ctrl-]. The next keystrokes build a tag name in
    /// `buffer`; Enter dispatches, Esc cancels.
    /// `buffer` accumulates the tag name; `error` holds an error or hint
    /// message (e.g. "[3 matches]" after a second consecutive Tab).
    /// `last_tab_matches` carries the prefix-match list from the most
    /// recent Tab so a second Tab can show the count without re-querying.
    TagPrompt {
        buffer: String,
        error: Option<String>,
        last_tab_matches: Option<Vec<String>>,
    },
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
    /// `:tselect [NAME]` — open the tag picker overlay. With a name, look
    /// up matches; without, use the currently-active TagStack matches.
    TagSelect(Option<String>),
    OpenPicker,
    OpenHelp,
    /// `:hex N` — set hex group width to N hex characters (2/4/8/16/32).
    HexGroup(usize),
    /// `:color [strict|interpret|raw]` — set or cycle the ANSI render mode.
    Color(Option<crate::render::AnsiMode>),
    /// `:case [sensitive|smart|insensitive]` — set or cycle the search
    /// case-sensitivity policy.
    Case(Option<crate::viewport::CaseMode>),
    /// `:hlsearch` (true) / `:nohlsearch` (false) — toggle search-match
    /// highlighting at runtime.
    HlSearch(bool),
    /// `:incsearch` — toggle incremental search (preview-as-you-type in the
    /// `/`/`?` prompt) at runtime. Bare command; flips the current state.
    IncSearch,
    /// `:search-skip-screen` — toggle whether forward search skips the screen.
    SearchSkipScreen,
    /// `:tilde` — toggle `~` display on lines past EOF.
    Tilde,
    /// `:header L [C]` — pin top L source rows and left C cols.
    Header(usize, usize),
    /// `:yank` — copy the current top logical line to the system clipboard
    /// (only acts when `--clipboard` was passed).
    Yank,
    /// `:vsplit [file]` / `:split [file]` — open a vertical split. With a path,
    /// open that file beside the current pane; without, duplicate the focused
    /// file at its current scroll position.
    VSplit(Option<String>),
    /// `:hsplit [file]` / `:hsp [file]` — open a horizontal (stacked) split.
    /// With a path, open that file below the current pane; without, duplicate
    /// the focused file at its current scroll position.
    HSplit(Option<String>),
    /// `:rotate` — flip the live split's orientation (vertical ↔ horizontal).
    Rotate,
    /// `:only` / `:close` — collapse a split back to the focused pane.
    Only,
    /// `:layout NAME` — build a named layout's panes live and swap the view.
    Layout(String),
    /// `:scrolllock` — toggle synchronized scroll between the two split panes.
    ScrollLock,
    /// `:encoding [LABEL]` — switch active encoding (e.g. `iso-8859-1`), or
    /// display the current encoding label when no argument is given.
    SetEncoding(Option<String>),
    /// `:diff` / `:diff!` — enter aligned diff mode. `force: true` when `!`
    /// is appended (re-align even if already in diff mode). Wired in diff task.
    Diff { force: bool },
    /// `:nodiff` — exit aligned diff mode. Wired in diff task.
    NoDiff,
    /// `:diffws` — toggle whitespace-significance in the diff alignment.
    /// Wired in diff task.
    DiffToggleWs,
    /// `:grep PAT` (Some) / `:nogrep` (None) — raw-regex predicate on the focused pane.
    Grep(Option<String>),
    /// `:filter SPEC` (Some) / `:nofilter` (None) — field filter on the focused pane (needs a format).
    Filter(Option<String>),
    /// `:format NAME` (Some) / `:noformat` (None) — set/clear the focused pane's format.
    Format(Option<String>),
    /// `:display TMPL` (Some) / `:nodisplay` (None) — set/clear the focused pane's display template.
    Display(Option<String>),
    /// `:mouse` toggles mouse capture; `:mouse on` / `:mouse off` set it.
    Mouse(Option<bool>),
    /// `:zoom` toggles the focused pane between full-screen and split size.
    Zoom,
}

#[derive(Debug, Clone, PartialEq)]
enum ColonParseError {
    UnknownCommand(String),
    MissingPath,
    TagRequiresName,
    LayoutRequiresName,
    HexGroupRequiresValue,
    HexGroupInvalid(String),
    ColorInvalid(String),
    CaseInvalid(String),
    MouseInvalid(String),
    HeaderInvalid(String),
}

impl std::fmt::Display for ColonParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ColonParseError::UnknownCommand(t) => write!(f, "unknown command: :{t}"),
            ColonParseError::MissingPath => write!(f, ":e requires a path"),
            ColonParseError::TagRequiresName => write!(f, ":tag requires a name"),
            ColonParseError::LayoutRequiresName => write!(f, ":layout requires a name"),
            ColonParseError::HexGroupRequiresValue => {
                write!(f, ":hex requires N (one of 2, 4, 8, 16, 32)")
            }
            ColonParseError::HexGroupInvalid(v) => {
                write!(f, ":hex N must be one of 2, 4, 8, 16, 32 (got {v})")
            }
            ColonParseError::ColorInvalid(v) => {
                write!(f, ":color mode must be strict, interpret, or raw (got {v})")
            }
            ColonParseError::CaseInvalid(v) => {
                write!(f, ":case mode must be sensitive, smart, or insensitive (got {v})")
            }
            ColonParseError::MouseInvalid(v) => {
                write!(f, ":mouse expects on or off (got {v})")
            }
            ColonParseError::HeaderInvalid(v) => {
                write!(f, ":header expects `L` or `L C` (got {v})")
            }
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
                Ok(ColonCommand::Edit(expand_tilde(rest)))
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
        "tselect" => {
            if rest.is_empty() {
                Ok(ColonCommand::TagSelect(None))
            } else {
                Ok(ColonCommand::TagSelect(Some(rest.to_string())))
            }
        }
        "b" | "buffers" => Ok(ColonCommand::OpenPicker),
        "h" | "help"    => Ok(ColonCommand::OpenHelp),
        "hex" => {
            if rest.is_empty() {
                Err(ColonParseError::HexGroupRequiresValue)
            } else {
                match rest.parse::<usize>() {
                    Ok(n) if matches!(n, 2 | 4 | 8 | 16 | 32) => Ok(ColonCommand::HexGroup(n)),
                    _ => Err(ColonParseError::HexGroupInvalid(rest.to_string())),
                }
            }
        }
        "color" => {
            if rest.is_empty() {
                Ok(ColonCommand::Color(None))
            } else {
                match rest {
                    "strict" => Ok(ColonCommand::Color(Some(crate::render::AnsiMode::Strict))),
                    "interpret" => Ok(ColonCommand::Color(Some(crate::render::AnsiMode::Interpret))),
                    "raw" => Ok(ColonCommand::Color(Some(crate::render::AnsiMode::Raw))),
                    other => Err(ColonParseError::ColorInvalid(other.to_string())),
                }
            }
        }
        "vsplit" | "split" => {
            if rest.is_empty() {
                Ok(ColonCommand::VSplit(None))
            } else {
                Ok(ColonCommand::VSplit(Some(rest.to_string())))
            }
        }
        "hsplit" | "hsp" => {
            if rest.is_empty() {
                Ok(ColonCommand::HSplit(None))
            } else {
                Ok(ColonCommand::HSplit(Some(rest.to_string())))
            }
        }
        "rotate" => Ok(ColonCommand::Rotate),
        "only" | "close" => Ok(ColonCommand::Only),
        "layout" => {
            if rest.is_empty() {
                Err(ColonParseError::LayoutRequiresName)
            } else {
                Ok(ColonCommand::Layout(rest.to_string()))
            }
        }
        "scrolllock" => Ok(ColonCommand::ScrollLock),
        "zoom" => Ok(ColonCommand::Zoom),
        "hlsearch"   => Ok(ColonCommand::HlSearch(true)),
        "nohlsearch" => Ok(ColonCommand::HlSearch(false)),
        "incsearch"  => Ok(ColonCommand::IncSearch),
        "search-skip-screen" => Ok(ColonCommand::SearchSkipScreen),
        "tilde"              => Ok(ColonCommand::Tilde),
        "yank"       => Ok(ColonCommand::Yank),
        "header" => {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            match parts.as_slice() {
                [l] => {
                    let n: usize = l.parse()
                        .map_err(|_| ColonParseError::HeaderInvalid(l.to_string()))?;
                    Ok(ColonCommand::Header(n, 0))
                }
                [l, c] => {
                    let nl: usize = l.parse()
                        .map_err(|_| ColonParseError::HeaderInvalid(l.to_string()))?;
                    let nc: usize = c.parse()
                        .map_err(|_| ColonParseError::HeaderInvalid(c.to_string()))?;
                    Ok(ColonCommand::Header(nl, nc))
                }
                _ => Err(ColonParseError::HeaderInvalid(rest.to_string())),
            }
        }
        "case" => {
            if rest.is_empty() {
                Ok(ColonCommand::Case(None))
            } else {
                match rest {
                    "sensitive" => Ok(ColonCommand::Case(Some(crate::viewport::CaseMode::Sensitive))),
                    "smart" => Ok(ColonCommand::Case(Some(crate::viewport::CaseMode::Smart))),
                    "insensitive" => Ok(ColonCommand::Case(Some(crate::viewport::CaseMode::Insensitive))),
                    other => Err(ColonParseError::CaseInvalid(other.to_string())),
                }
            }
        }
        "mouse" => match rest {
            "" => Ok(ColonCommand::Mouse(None)),
            "on" => Ok(ColonCommand::Mouse(Some(true))),
            "off" => Ok(ColonCommand::Mouse(Some(false))),
            other => Err(ColonParseError::MouseInvalid(other.to_string())),
        },
        "encoding" => Ok(ColonCommand::SetEncoding(
            (!rest.is_empty()).then(|| rest.to_string())
        )),
        "diff"  => Ok(ColonCommand::Diff { force: false }),
        "diff!" => Ok(ColonCommand::Diff { force: true }),
        "nodiff"  => Ok(ColonCommand::NoDiff),
        "diffws"  => Ok(ColonCommand::DiffToggleWs),
        // Bare `:grep`/`:filter`/`:format`/`:display` with no argument parses to the
        // None (clear) form, same as the explicit `:nogrep`/`:nofilter`/etc. aliases.
        "grep" => Ok(ColonCommand::Grep((!rest.is_empty()).then(|| rest.to_string()))),
        "nogrep" => Ok(ColonCommand::Grep(None)),
        "filter" => Ok(ColonCommand::Filter((!rest.is_empty()).then(|| rest.to_string()))),
        "nofilter" => Ok(ColonCommand::Filter(None)),
        "format" => Ok(ColonCommand::Format((!rest.is_empty()).then(|| rest.to_string()))),
        "noformat" => Ok(ColonCommand::Format(None)),
        "display" => Ok(ColonCommand::Display((!rest.is_empty()).then(|| rest.to_string()))),
        "nodisplay" => Ok(ColonCommand::Display(None)),
        other => Err(ColonParseError::UnknownCommand(other.to_string())),
    }
}

enum ColonOutcome {
    Continue(Option<String>),  // Some(msg) = transient status to show
    Quit,
    /// Hand a command to the outer dispatch loop. Used so colon commands
    /// like `:b` can install overlays via the same Command path as their
    /// keymap counterparts, without taking a `&mut overlay` argument.
    DispatchCommand(Command),
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
/// Stat the tag file and reload it if its mtime moved. Returns a transient
/// status message when a reload happened so the caller can surface it.
/// Errors are swallowed (the previously-loaded state stays valid).
fn refresh_tag_file(tag_file: &mut Option<crate::tags::TagFile>) -> Option<String> {
    match tag_file.as_mut()?.reload_if_changed() {
        Ok(true) => Some("[tags reloaded]".into()),
        _ => None,
    }
}

/// Longest common prefix among a slice of strings. Returns "" for an empty
/// slice or when the items don't share any prefix. Used by Tab-completion
/// in the `:tag` / `Ctrl-]` prompt.
fn longest_common_prefix(items: &[String]) -> String {
    let mut iter = items.iter();
    let Some(first) = iter.next() else { return String::new() };
    let mut prefix = first.clone();
    for s in iter {
        while !s.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                return prefix;
            }
        }
    }
    prefix
}

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

    let (line, hint) = match resolve_tag_address(&entry.address, src.as_ref(), idx, 0) {
        AddressResult::Line(l) => (l, None),
        AddressResult::NotFound => (0, Some("[tag pattern not found]".into())),
        AddressResult::Unsupported(raw) => (
            0,
            Some(format!("[tag address not supported: {raw}]")),
        ),
    };

    let clamped = line.min(idx.line_count().saturating_sub(1));
    viewport.goto_line(clamped, src.as_ref(), idx);
    hint
}

enum AddressResult {
    Line(usize),
    NotFound,
    Unsupported(String),
}

/// Resolve a `TagAddress` to a 0-based line number, starting the search at
/// `from_line` (used for the chained-address case where the second step
/// resumes from the line matched by the first).
fn resolve_tag_address(
    addr: &crate::tags::TagAddress,
    src: &dyn crate::source::Source,
    idx: &mut crate::line_index::LineIndex,
    from_line: usize,
) -> AddressResult {
    match addr {
        crate::tags::TagAddress::Line(n) => AddressResult::Line(n.saturating_sub(1)),
        crate::tags::TagAddress::Pattern(p) => {
            let re_src = crate::tags::pattern_to_regex(p);
            let re = match regex::bytes::Regex::new(&re_src) {
                Ok(r) => r,
                Err(_) => return AddressResult::NotFound,
            };
            match find_pattern_line(src, idx, &re, from_line) {
                Some(l) => AddressResult::Line(l),
                None => AddressResult::NotFound,
            }
        }
        crate::tags::TagAddress::Chained(parts) => {
            let mut here = from_line;
            for step in parts {
                match resolve_tag_address(step, src, idx, here) {
                    AddressResult::Line(l) => here = l + 1,
                    other => return other,
                }
            }
            // Subtract 1 from the final "next-search start" to land on the
            // last matched line itself.
            AddressResult::Line(here.saturating_sub(1))
        }
        crate::tags::TagAddress::Unsupported(raw) => {
            AddressResult::Unsupported(raw.clone())
        }
    }
}

fn find_pattern_line(
    src: &dyn crate::source::Source,
    idx: &mut crate::line_index::LineIndex,
    re: &regex::bytes::Regex,
    from_line: usize,
) -> Option<usize> {
    idx.extend_to_end(src);
    for line_no in from_line..idx.line_count() {
        let bytes = idx.line_bytes_stripped(line_no, src);
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

/// Open whatever file is at `file_set.current()`, updating viewport and
/// `current_file_index`. Returns `Some(msg)` if anything went wrong (for
/// transient status). The cursor in `file_set` must be set before calling.
#[allow(clippy::too_many_arguments)]
fn switch_to_current_file(
    file_set: &mut crate::file_set::FileSet,
    current_file_index: &mut usize,
    args: &crate::cli::Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
    viewport: &mut crate::viewport::Viewport,
    src: &mut Box<dyn crate::source::Source>,
    idx: &mut crate::line_index::LineIndex,
) -> Option<String> {
    let path = match file_set.current() {
        Some(p) => p.to_path_buf(),
        None => return Some("[empty file set]".into()),
    };
    let new_idx_val = file_set.current_index();
    match switch_file(&path, new_idx_val, file_set.len(), args, preprocessor, viewport, src, idx, record_start_regex) {
        Ok(()) => {
            *current_file_index = new_idx_val;
            None
        }
        Err(e) => Some(format!("[open: {e}]")),
    }
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
    viewport.reset_hscroll(); // new file: drop any horizontal scroll offset

    Ok(())
}

/// Copy the current top logical line to the system clipboard, decoded as UTF-8
/// via the viewport's active encoding so Latin-1 (and other charsets) are
/// correctly represented in the clipboard. Returns a human-facing status string
/// for the caller to flash. When `--clipboard` wasn't passed, reports that and
/// copies nothing.
fn yank_current_line(
    clipboard_enabled: bool,
    viewport: &crate::viewport::Viewport,
    src: &dyn crate::source::Source,
    idx: &mut crate::line_index::LineIndex,
) -> String {
    if !clipboard_enabled {
        return "[clipboard not enabled (pass --clipboard)]".to_string();
    }
    if idx.line_count() == 0 {
        return "[nothing to copy]".to_string();
    }
    let line = viewport.top_line();
    let enc = viewport.encoding();
    let raw = current_line_bytes(idx, src, line);
    // Decode from the source encoding into UTF-8 so the clipboard receives
    // proper Unicode text regardless of the file's charset.
    let decoded = crate::charset::decode_line(&raw, enc);
    let utf8_bytes = decoded.as_bytes().to_vec();
    match crate::clipboard::write(&utf8_bytes) {
        Ok(()) => format!("[copied {} bytes]", utf8_bytes.len()),
        Err(e) => format!("[{e}]"),
    }
}

/// Raw bytes of the logical line at `line` (trailing newline already stripped
/// by `LineIndex::line_range`). Pulled out of `yank_current_line` so it can be
/// unit-tested without touching the OS clipboard.
fn current_line_bytes(
    idx: &crate::line_index::LineIndex,
    src: &dyn crate::source::Source,
    line: usize,
) -> Vec<u8> {
    let range = idx.line_range(line, src);
    src.bytes(range).into_owned()
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
        ColonCommand::TagSelect(name) => {
            let prepared = match name {
                Some(n) => {
                    let tf = match tag_file {
                        Some(t) => t,
                        None => {
                            return ColonOutcome::Continue(Some(
                                "[no tags file loaded]".into(),
                            ))
                        }
                    };
                    let matches: Vec<crate::tags::TagEntry> = tf.lookup(&n).to_vec();
                    if matches.is_empty() {
                        return ColonOutcome::Continue(Some(
                            format!("[no matches for `{n}`]"),
                        ));
                    }
                    tag_stack.set_active(n, matches);
                    true
                }
                None => tag_stack.active.is_some(),
            };
            if prepared {
                ColonOutcome::DispatchCommand(Command::OpenTagPicker)
            } else {
                ColonOutcome::Continue(Some("[no active tag]".into()))
            }
        }
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
        // Hand off to the outer command dispatcher so the same install path
        // services both `:b` and the (future) F2 keybinding.
        ColonCommand::OpenPicker => ColonOutcome::DispatchCommand(Command::OpenPicker),
        ColonCommand::OpenHelp => ColonOutcome::DispatchCommand(Command::OpenHelp),
        ColonCommand::HexGroup(hex_chars) => {
            if !viewport.hex_mode() {
                return ColonOutcome::Continue(Some(
                    "[:hex requires --hex mode]".into(),
                ));
            }
            // Already validated in parse_colon_command, so unwrap is safe.
            let bpg = crate::hex::hex_chars_to_bytes_per_group(hex_chars).unwrap();
            viewport.set_hex_group_size(bpg);
            ColonOutcome::Continue(Some(format!("[hex group: {hex_chars} chars]")))
        }
        ColonCommand::Color(mode) => {
            use crate::render::AnsiMode;
            let next = mode.unwrap_or_else(|| match viewport.ansi_mode() {
                AnsiMode::Strict => AnsiMode::Interpret,
                AnsiMode::Interpret => AnsiMode::Raw,
                AnsiMode::Raw => AnsiMode::Strict,
            });
            viewport.set_ansi_mode(next);
            let label = match next {
                AnsiMode::Strict => "strict",
                AnsiMode::Interpret => "interpret",
                AnsiMode::Raw => "raw",
            };
            ColonOutcome::Continue(Some(format!("[color: {label}]")))
        }
        ColonCommand::Header(l, c) => {
            viewport.set_header(l, c);
            ColonOutcome::Continue(Some(format!("[header: {l} rows, {c} cols]")))
        }
        ColonCommand::HlSearch(on) => {
            viewport.set_hilite_search(on);
            let msg = if on { "[hlsearch on]" } else { "[hlsearch off]" };
            ColonOutcome::Continue(Some(msg.into()))
        }
        ColonCommand::IncSearch => {
            let on = !viewport.incsearch();
            viewport.set_incsearch(on);
            let msg = if on { "[incsearch on]" } else { "[incsearch off]" };
            ColonOutcome::Continue(Some(msg.into()))
        }
        ColonCommand::SearchSkipScreen => {
            let on = !viewport.search_skip_screen();
            viewport.set_search_skip_screen(on);
            ColonOutcome::Continue(Some(
                if on { "[search-skip-screen on]" } else { "[search-skip-screen off]" }.into()))
        }
        ColonCommand::Tilde => {
            let on = !viewport.tilde_eof();
            viewport.set_tilde_eof(on);
            ColonOutcome::Continue(Some(if on { "[tilde on]" } else { "[tilde off]" }.into()))
        }
        ColonCommand::Yank => {
            ColonOutcome::Continue(Some(yank_current_line(args.clipboard, viewport, src.as_ref(), idx)))
        }
        ColonCommand::Case(mode) => {
            use crate::viewport::CaseMode;
            let next = mode.unwrap_or_else(|| match viewport.case_mode() {
                CaseMode::Sensitive => CaseMode::Smart,
                CaseMode::Smart => CaseMode::Insensitive,
                CaseMode::Insensitive => CaseMode::Sensitive,
            });
            viewport.set_case_mode(next);
            let label = match next {
                CaseMode::Sensitive => "sensitive",
                CaseMode::Smart => "smart",
                CaseMode::Insensitive => "insensitive",
            };
            ColonOutcome::Continue(Some(format!("[case: {label}]")))
        }
        // Split commands and encoding are intercepted in the event loop (they
        // need the loose `others`/`cols`/`rows`/`focused_pos` locals or must
        // apply to every pane) and never reach this file-set-only dispatcher.
        ColonCommand::VSplit(_) | ColonCommand::HSplit(_) | ColonCommand::Rotate
        | ColonCommand::Only | ColonCommand::Layout(_) | ColonCommand::ScrollLock
        | ColonCommand::SetEncoding(_) | ColonCommand::Mouse(_) | ColonCommand::Zoom
        | ColonCommand::Diff { .. } | ColonCommand::NoDiff | ColonCommand::DiffToggleWs => {
            unreachable!("split/scroll-lock/encoding/diff/mouse/zoom commands are handled in the run() event loop")
        }
        ColonCommand::Grep(arg) => match arg {
            None => { viewport.set_grep(None); ColonOutcome::Continue(Some("[grep cleared]".into())) }
            Some(pat) => {
                match crate::grep::GrepPredicate::compile(&[pat.clone()], viewport.case_mode()) {
                    Ok(g) => { viewport.set_grep(Some(g)); ColonOutcome::Continue(Some(format!("[grep: {pat}]"))) }
                    Err(e) => ColonOutcome::Continue(Some(format!("grep: {e}"))),
                }
            }
        },
        ColonCommand::Display(arg) => match arg {
            None => { viewport.set_display(None); ColonOutcome::Continue(Some("[display cleared]".into())) }
            Some(tmpl) => {
                match viewport.format_label().map(|s| s.to_string()) {
                    None => ColonOutcome::Continue(Some("display needs a format (:format NAME)".into())),
                    Some(name) => match crate::format::load_all() {
                        Err(e) => ColonOutcome::Continue(Some(format!("display: {e}"))),
                        Ok(formats) => match formats.get(&name) {
                            None => ColonOutcome::Continue(Some(format!("unknown format: {name}"))),
                            Some(fmt) => match crate::format::DisplayTemplate::compile(&tmpl, &fmt.field_names) {
                                Ok(t) => {
                                    viewport.set_display(Some(crate::format::DisplayRenderer::new(t, fmt.regex.clone())));
                                    ColonOutcome::Continue(Some("[display set]".into()))
                                }
                                Err(e) => ColonOutcome::Continue(Some(format!("display: {e}"))),
                            },
                        },
                    },
                }
            }
        },
        ColonCommand::Format(arg) => match arg {
            None => {
                viewport.set_format_label(None);
                viewport.set_filter(None); // filter depends on the format
                idx.clear_record_start();
                ColonOutcome::Continue(Some("[format cleared]".into()))
            }
            Some(name) => match crate::format::load_all() {
                Err(e) => ColonOutcome::Continue(Some(format!("format: {e}"))),
                Ok(formats) => match formats.get(&name) {
                    None => ColonOutcome::Continue(Some(format!("unknown format: {name}"))),
                    Some(fmt) => {
                        // Convert the text-mode `regex::Regex` to `regex::bytes::Regex`
                        // by round-tripping through the pattern string, matching what
                        // main.rs does when building the startup record_start_regex.
                        let bytes_re: Option<regex::bytes::Regex> =
                            fmt.record_start.as_ref().and_then(|re| {
                                regex::bytes::Regex::new(re.as_str()).ok()
                            });
                        idx.reset_record_start_opt(bytes_re);
                        viewport.set_format_label(Some(name.clone()));
                        ColonOutcome::Continue(Some(format!("[format: {name}]")))
                    }
                },
            },
        },
        ColonCommand::Filter(arg) => match arg {
            None => {
                viewport.set_filter(None);
                ColonOutcome::Continue(Some("[filter cleared]".into()))
            }
            Some(spec_str) => match viewport.format_label().map(|s| s.to_string()) {
                None => {
                    ColonOutcome::Continue(Some("filter needs a format (:format NAME)".into()))
                }
                Some(name) => match crate::format::load_all() {
                    Err(e) => ColonOutcome::Continue(Some(format!("filter: {e}"))),
                    Ok(formats) => match formats.get(&name) {
                        None => ColonOutcome::Continue(Some(format!("unknown format: {name}"))),
                        Some(fmt) => match crate::filter::FilterSpec::parse(&spec_str) {
                            Err(e) => ColonOutcome::Continue(Some(format!("filter: {e}"))),
                            Ok(spec) => {
                                match crate::filter::CompiledFilter::compile(
                                    fmt,
                                    vec![spec],
                                    viewport.case_mode(),
                                ) {
                                    Ok(f) => {
                                        viewport.set_filter(Some(f));
                                        ColonOutcome::Continue(Some(format!(
                                            "[filter: {spec_str}]"
                                        )))
                                    }
                                    Err(e) => {
                                        ColonOutcome::Continue(Some(format!("filter: {e}")))
                                    }
                                }
                            }
                        },
                    },
                },
            },
        },
    }
}

fn sync_mouse_badge(viewport: &mut crate::viewport::Viewport, others: &mut [crate::pane::Pane], enabled: bool) {
    viewport.set_mouse_off(!enabled);
    for o in others.iter_mut() {
        o.viewport.set_mouse_off(!enabled);
    }
}

/// Apply a new mouse-capture state: issue the terminal escape, update the
/// live flag, and refresh the `[nomouse]` badge on every pane.
fn apply_mouse(
    stdout: &mut io::Stdout,
    mouse_enabled: &mut bool,
    on: bool,
    viewport: &mut crate::viewport::Viewport,
    others: &mut [crate::pane::Pane],
) {
    use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
    if on != *mouse_enabled {
        let _ = if on {
            crossterm::execute!(stdout, EnableMouseCapture)
        } else {
            crossterm::execute!(stdout, DisableMouseCapture)
        };
        *mouse_enabled = on;
    }
    sync_mouse_badge(viewport, others, *mouse_enabled);
}

/// In split mode the compositor is cell-based: force ASCII images and disable
/// raw passthrough so every row is cells.
fn force_cell_mode(vp: &mut Viewport) {
    #[cfg(feature = "image")]
    vp.set_image_protocol(crate::viewport::ImageProtocol::Ascii, None);
    vp.set_ansi_mode_cells();
}

/// Physical column → `others` slot index. Physical columns include the focused
/// pane at `focused_pos`; an `others` slot maps to the physical column on its
/// own side of the focused pane. Caller must ensure `phys != focused_pos`.
fn other_slot(phys: usize, focused_pos: usize) -> usize {
    if phys < focused_pos { phys } else { phys - 1 }
}

/// `others` slot index → physical column. Inverse of `other_slot`.
fn phys_of_slot(slot: usize, focused_pos: usize) -> usize {
    if slot < focused_pos { slot } else { slot + 1 }
}

/// Initialize split layout: size the focused viewport + every other pane to its
/// physical column width and force cell mode. With `focused_pos` the focused
/// pane occupies physical column `focused_pos`. Mirrors the legacy 2-pane
/// `other_pane_init` when `others.len() == 1`.
fn panes_init(
    others: &mut [crate::pane::Pane],
    focused_vp: &mut Viewport,
    focused_pos: usize,
    cols: u16,
    rows: u16,
    orientation: Orientation,
    sizes: &[Option<u16>],
) {
    let n = others.len() + 1;
    let norm: Vec<Option<u16>> = (0..n).map(|i| sizes.get(i).copied().flatten()).collect();
    let sizes = match orientation {
        Orientation::Vertical => crate::pane::split_widths_n_weighted(cols, &norm),
        Orientation::Horizontal => crate::pane::split_heights_n_weighted(rows, &norm),
    };
    if sizes.len() == 1 {
        focused_vp.resize(cols, rows); // too-small fallback (both orientations)
    } else {
        let resize = |vp: &mut Viewport, sz: u16| match orientation {
            Orientation::Vertical => vp.resize(sz, rows),
            Orientation::Horizontal => vp.resize(cols, sz),
        };
        resize(focused_vp, sizes[focused_pos]);
        for (slot, other) in others.iter_mut().enumerate() {
            let phys = phys_of_slot(slot, focused_pos);
            resize(&mut other.viewport, sizes[phys]);
        }
    }
    force_cell_mode(focused_vp);
    for other in others.iter_mut() {
        force_cell_mode(&mut other.viewport);
    }
}

/// Resize the focused viewport, sizing each other pane's viewport to its
/// physical column width when a split is active (focused-pane-only fallback
/// when the terminal is too narrow to split into N panes).
fn resize_split_aware(
    focused_vp: &mut Viewport,
    others: &mut [crate::pane::Pane],
    cols: u16,
    rows: u16,
    focused_pos: usize,
    orientation: Orientation,
    sizes: &[Option<u16>],
) {
    if others.is_empty() {
        focused_vp.resize(cols, rows);
        return;
    }
    let n = others.len() + 1;
    let norm: Vec<Option<u16>> = (0..n).map(|i| sizes.get(i).copied().flatten()).collect();
    let sizes = match orientation {
        Orientation::Vertical => crate::pane::split_widths_n_weighted(cols, &norm),
        Orientation::Horizontal => crate::pane::split_heights_n_weighted(rows, &norm),
    };
    if sizes.len() == 1 {
        focused_vp.resize(cols, rows); // too-small fallback (both orientations)
    } else {
        let resize = |vp: &mut Viewport, sz: u16| match orientation {
            Orientation::Vertical => vp.resize(sz, rows),
            Orientation::Horizontal => vp.resize(cols, sz),
        };
        resize(focused_vp, sizes[focused_pos]);
        for (slot, other) in others.iter_mut().enumerate() {
            let phys = phys_of_slot(slot, focused_pos);
            resize(&mut other.viewport, sizes[phys]);
        }
    }
}

/// Toggle the focused pane between full-screen and its split size. No-op (with
/// a flash) in a single pane or in diff mode, where the layout is fixed. Only
/// changes rendering/sizing — the other panes stay in memory.
fn toggle_zoom(
    zoomed: &mut bool,
    focused_vp: &mut Viewport,
    others: &mut [crate::pane::Pane],
    diff_active: bool,
    cols: u16,
    rows: u16,
    focused_pos: usize,
    orientation: Orientation,
    sizes: &[Option<u16>],
) {
    if others.is_empty() {
        focused_vp.flash("zoom needs a split", 40);
        return;
    }
    if diff_active {
        focused_vp.flash("zoom not available in diff", 40);
        return;
    }
    *zoomed = !*zoomed;
    if *zoomed {
        focused_vp.resize(cols, rows);
    } else {
        resize_split_aware(focused_vp, others, cols, rows, focused_pos, orientation, sizes);
    }
}

/// Clear zoom and restore split sizing. No-op when not zoomed. Called before
/// focus switches, structural split commands, and entering diff so the screen
/// is never left maximized over a layout that no longer matches.
fn unzoom(
    zoomed: &mut bool,
    focused_vp: &mut Viewport,
    others: &mut [crate::pane::Pane],
    cols: u16,
    rows: u16,
    focused_pos: usize,
    orientation: Orientation,
    sizes: &[Option<u16>],
) {
    if *zoomed {
        *zoomed = false;
        resize_split_aware(focused_vp, others, cols, rows, focused_pos, orientation, sizes);
    }
}

/// Capture per-physical-column scroll-lock offsets relative to the leftmost
/// pane. The focused viewport sits at physical column `focused_pos`; each other
/// pane at its own physical column. `offsets[i] = top_i - top_0`, so the
/// leftmost pane's offset is always 0. Independent of which pane is focused, so
/// a `Tab` rotation never disturbs the captured alignment (Tab-invariant).
fn capture_lock_offsets(
    focused_vp: &Viewport,
    others: &[crate::pane::Pane],
    focused_pos: usize,
) -> Vec<isize> {
    let n = others.len() + 1;
    let mut tops = vec![0isize; n];
    tops[focused_pos] = focused_vp.top_line() as isize;
    for (slot, other) in others.iter().enumerate() {
        tops[phys_of_slot(slot, focused_pos)] = other.viewport.top_line() as isize;
    }
    let base = tops[0];
    tops.iter().map(|t| t - base).collect()
}

/// Next physical pane index when cycling focus over `n` panes. `forward` →
/// Tab (next, wrapping); else BackTab (previous, wrapping). Pure.
fn rotate_index(focused_pos: usize, n: usize, forward: bool) -> usize {
    if forward {
        (focused_pos + 1) % n
    } else {
        (focused_pos + n - 1) % n
    }
}

/// Rotate focus among the N physical panes. `focused` is the currently-focused
/// pane (built from the loose locals); `others` are the rest in slot order.
/// Returns the new focused pane, the new others list, and the new `focused_pos`.
/// `forward` cycles to the next physical pane (Tab), else previous (BackTab);
/// both wrap. Move-by-value: no placeholders.
fn rotate_focus(
    focused: crate::pane::Pane,
    others: Vec<crate::pane::Pane>,
    focused_pos: usize,
    forward: bool,
) -> (crate::pane::Pane, Vec<crate::pane::Pane>, usize) {
    let n = others.len() + 1;
    let target = rotate_index(focused_pos, n, forward);
    set_focus(focused, others, focused_pos, target)
}

/// Move focus to physical pane `target`. `focused` is the currently-focused
/// pane (built from the loose locals); `others` are the rest in slot order.
/// Returns the new focused pane, the new others list (physical order minus the
/// target), and the new `focused_pos`. `target` is clamped to a valid index.
/// Move-by-value: no placeholders. Shared by `Tab`/`BackTab` (via
/// `rotate_focus`) and mouse click-to-focus.
fn set_focus(
    focused: crate::pane::Pane,
    others: Vec<crate::pane::Pane>,
    focused_pos: usize,
    target: usize,
) -> (crate::pane::Pane, Vec<crate::pane::Pane>, usize) {
    let n = others.len() + 1;
    // Materialize the full physical order with `focused` reinserted at its slot.
    let mut all: Vec<crate::pane::Pane> = Vec::with_capacity(n);
    all.extend(others);
    all.insert(focused_pos, focused);
    let target = target.min(n - 1);
    let new_focused = all.remove(target);
    (new_focused, all, target)
}

/// Expand a leading `~/` against `$HOME`. Shared by `:e`/`:edit` and the
/// `:vsplit`/`:split` path argument, which both capture a raw remainder string.
fn expand_tilde(arg: &str) -> std::path::PathBuf {
    if let Some(stripped) = arg.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let mut p = std::path::PathBuf::from(home);
            p.push(stripped);
            return p;
        }
    }
    std::path::PathBuf::from(arg)
}

/// The display-config subset shared by every pane, focused or not. Both the
/// startup `--split` pane (`main::build_second_pane`) and the runtime `:vsplit`
/// pane call this so they stay in lockstep. Format-specific predicates
/// (filter/grep/format/display) and the `--tabs`/`--header` spec parsers live
/// in the `main` binary and are NOT applied here: `build_second_pane` layers
/// them on after this call, while runtime panes show the plain file (documented
/// v1 limitation). Images are pinned to ASCII by `force_cell_mode`, which the
/// caller invokes.
pub fn apply_pane_display_config(viewport: &mut Viewport, args: &crate::cli::Args) {
    if args.line_numbers { viewport.toggle_line_numbers(); }
    if args.chop { viewport.toggle_chop(); }
    viewport.opts.tab_width = args.tab_width;
    viewport.set_follow_mode(args.follow);
    viewport.set_live_mode(args.live);
    if args.hex {
        viewport.set_hex_mode(true);
        if let Some(bpg) = crate::hex::hex_chars_to_bytes_per_group(args.hex_group) {
            viewport.set_hex_group_size(bpg);
        }
    }
    viewport.set_squeeze_blanks(args.squeeze_blanks);
    viewport.set_status_column(args.status_column);
    viewport.opts.rscroll_char = args.rscroll.chars().next();
    viewport.opts.word_wrap = args.word_wrap;
    viewport.set_page_size(args.window);
    viewport.set_search_skip_screen(args.search_skip_screen);
    viewport.set_hilite_only_match(args.hilite_only_match);
    viewport.set_tilde_eof(args.tilde);
    viewport.set_file_index(0, 1);
}

/// Build a `Pane` at runtime for `:vsplit`. `path_arg = Some(p)` opens that
/// file; `None` duplicates the focused file (`focused_label`/`focused_top`) at
/// its current scroll position — which requires the focused source to be
/// file-backed (`focused_path = Some(_)`); stdin yields an error.
#[allow(clippy::too_many_arguments)]
fn build_runtime_pane(
    path_arg: Option<&str>,
    focused_path: Option<&std::path::Path>,
    focused_top: usize,
    cols: u16,
    rows: u16,
    args: &crate::cli::Args,
    ansi_mode: crate::render::AnsiMode,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
    record_start_regex: Option<&regex::bytes::Regex>,
) -> crate::error::Result<crate::pane::Pane> {
    // Resolve the path and whether we duplicate the focused scroll position.
    let (path, dup_top): (std::path::PathBuf, Option<usize>) = match path_arg {
        Some(arg) => (expand_tilde(arg), None),
        None => match focused_path {
            Some(p) => (p.to_path_buf(), Some(focused_top)),
            None => {
                return Err(crate::error::Error::Runtime(
                    "can't duplicate stdin; give a file".to_string(),
                ));
            }
        },
    };

    let (src, label, preprocess_failure) =
        crate::open::open_source_for_path(&path, args, preprocessor)?;

    let mut idx = crate::line_index::LineIndex::new();
    if let Some(re) = record_start_regex {
        idx.set_record_start(re.clone());
    }

    let mut viewport = Viewport::new(cols, rows, label);
    apply_pane_display_config(&mut viewport, args);
    viewport.set_ansi_mode(ansi_mode);
    viewport.set_preprocess_failure(preprocess_failure);
    if let Some(top) = dup_top {
        viewport.goto_line(top, src.as_ref(), &mut idx);
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

// ── Diff mode ────────────────────────────────────────────────────────────────

/// Snapshot of the aligned diff computed from the two split panes.
struct DiffState {
    pairs: Vec<crate::diff::DiffPair>,
    /// Scroll position: (pair_index, sub_row within that pair).
    pos: (usize, usize),
    ignore_ws: bool,
    hunk_total: usize,
}

const DIFF_MAX_LINES: usize = 500_000;

fn build_diff(
    src: &dyn crate::source::Source,
    idx: &mut crate::line_index::LineIndex,
    osrc: &dyn crate::source::Source,
    oidx: &mut crate::line_index::LineIndex,
    ignore_ws: bool,
    force: bool,
) -> std::result::Result<Vec<crate::diff::DiffPair>, String> {
    idx.extend_to_line(usize::MAX, src);
    oidx.extend_to_line(usize::MAX, osrc);
    let (ln, rn) = (idx.line_count(), oidx.line_count());
    if !force && (ln > DIFF_MAX_LINES || rn > DIFF_MAX_LINES) {
        return Err(format!(
            "diff: files too large (>{}k lines); showing plain split",
            DIFF_MAX_LINES / 1000
        ));
    }
    let lk: Vec<u64> = (0..ln)
        .map(|n| crate::diff::line_key(&src.bytes(idx.line_range(n, src)), ignore_ws))
        .collect();
    let rk: Vec<u64> = (0..rn)
        .map(|n| crate::diff::line_key(&osrc.bytes(oidx.line_range(n, osrc)), ignore_ws))
        .collect();
    Ok(crate::diff::align(&lk, &rk))
}

/// Returns the 1-based hunk index of the hunk containing/just-before `d.pos.0`,
/// or 0 if `pos` is before the first hunk.
fn current_hunk(d: &DiffState) -> usize {
    let hs = crate::diff::hunks(&d.pairs);
    // Find the last hunk whose start <= pos.0
    hs.iter()
        .enumerate()
        .rev()
        .find(|(_, h)| h.start <= d.pos.0)
        .map(|(i, _)| i + 1)
        .unwrap_or(0)
}

/// Build a `RenderOpts` for diff rendering: wrap=true, ANSI interpret, and the
/// focused viewport's charset. v1 scoping: `--tabs`/`--tab-width`, `--no-color`
/// (byte-faithful), and other per-view display flags are NOT carried into diff
/// mode — only the encoding is, so non-UTF-8 files decode correctly.
fn diff_pane_opts(viewport: &crate::viewport::Viewport, cols: u16) -> crate::render::RenderOpts {
    crate::render::RenderOpts {
        cols,
        wrap: true,
        tab_width: 8,
        mode: crate::render::AnsiMode::Interpret,
        encoding: viewport.encoding(),
        ..Default::default()
    }
}

/// Walk `d.pos` by `delta` screen rows (positive = down, negative = up),
/// clamping at both ends. Uses `pair_height` to compute how many rows each
/// pair occupies.
#[allow(clippy::too_many_arguments)]
fn diff_scroll(
    d: &mut DiffState,
    delta: isize,
    lsrc: &dyn crate::source::Source,
    lidx: &mut crate::line_index::LineIndex,
    rsrc: &dyn crate::source::Source,
    ridx: &mut crate::line_index::LineIndex,
    lopts: &crate::render::RenderOpts,
    ropts: &crate::render::RenderOpts,
) {
    if d.pairs.is_empty() {
        return;
    }
    if delta == 0 {
        return;
    }
    if delta > 0 {
        let mut rem = delta as usize;
        loop {
            if d.pos.0 >= d.pairs.len() {
                // Past the end — clamp
                d.pos.0 = d.pairs.len() - 1;
                let h = crate::diff_view::pair_height(
                    &d.pairs[d.pos.0], lsrc, lidx, rsrc, ridx, lopts, ropts,
                );
                d.pos.1 = h.saturating_sub(1);
                break;
            }
            let h = crate::diff_view::pair_height(
                &d.pairs[d.pos.0], lsrc, lidx, rsrc, ridx, lopts, ropts,
            );
            let rows_left_in_pair = h.saturating_sub(d.pos.1);
            if rem < rows_left_in_pair {
                d.pos.1 += rem;
                break;
            }
            rem -= rows_left_in_pair;
            d.pos.0 += 1;
            d.pos.1 = 0;
            if d.pos.0 >= d.pairs.len() {
                d.pos.0 = d.pairs.len() - 1;
                let h2 = crate::diff_view::pair_height(
                    &d.pairs[d.pos.0], lsrc, lidx, rsrc, ridx, lopts, ropts,
                );
                d.pos.1 = h2.saturating_sub(1);
                break;
            }
        }
    } else {
        let mut rem = (-delta) as usize;
        loop {
            if rem <= d.pos.1 {
                d.pos.1 -= rem;
                break;
            }
            rem -= d.pos.1 + 1;
            if d.pos.0 == 0 {
                d.pos = (0, 0);
                break;
            }
            d.pos.0 -= 1;
            let h = crate::diff_view::pair_height(
                &d.pairs[d.pos.0], lsrc, lidx, rsrc, ridx, lopts, ropts,
            );
            d.pos.1 = h.saturating_sub(1);
        }
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
    mouse_on: bool,
    preprocessor: Option<crate::preprocess::Preprocessor>,
    mut tag_file: Option<crate::tags::TagFile>,
    extra_panes: Vec<crate::pane::Pane>,
    orientation: Orientation,
    #[cfg(feature = "image")]
    startup_image_protocol: (crate::viewport::ImageProtocol, Option<(u16, u16)>),
) -> Result<()> {
    let mut orientation = orientation;
    let (mut cols, mut rows) = size().unwrap_or((80, 24));
    viewport.resize(cols, rows);

    let truecolor = match args.truecolor.as_str() {
        "always" => true,
        "never" => false,
        _ => crate::render::TrueColor::Auto.resolve(),
    };

    let mut stdout = io::stdout();
    const BASE_POLL: Duration = Duration::from_millis(250);
    #[cfg(feature = "image")]
    let mut last_tick = std::time::Instant::now();
    let mut last_revision = src.revision();

    // N-pane model: the focused pane lives in the loose locals
    // (`src`/`idx`/`viewport`/`last_revision`/`last_tick`); every other pane is
    // a `Pane` in `others` (0 panes = single view, 1 = 2-pane split). `Tab`
    // rotates which physical pane is focused; `focused_pos` is its physical
    // column index (0 = leftmost). Each physical side keeps its own width across
    // a focus rotation (matters on odd widths where columns differ).
    let mut others: Vec<crate::pane::Pane> = extra_panes;
    let mut focused_pos: usize = 0;
    // Per-pane size as a percentage of the split axis; None = auto (equal share).
    // Physical-pane-indexed, parallel to the focused-pane + `others`. Empty/all-None
    // = today's even split. Seeded by --widths/--heights; mutated by :width/:height.
    let mut pane_sizes: Vec<Option<u16>> = Vec::new();
    panes_init(&mut others, &mut viewport, focused_pos, cols, rows, orientation, &pane_sizes);
    let mut scroll_lock = false;
    let mut zoomed = false;
    // Per-physical-column scroll-lock offsets relative to the leftmost pane,
    // captured at lock enable. Empty when scroll-lock is off.
    let mut lock_offsets: Vec<isize> = Vec::new();

    // Diff mode: Some when aligned diff is active.
    let mut diff: Option<DiffState> = None;

    // Startup --diff: build initial diff state if exactly one other pane exists.
    if args.diff || args.gitdiff {
        if others.len() == 1 {
            let other = &mut others[0];
            let ignore_ws = args.diff_ignore_whitespace;
            match build_diff(
                src.as_ref(), &mut idx,
                other.src.as_ref(), &mut other.idx,
                ignore_ws, args.diff_force,
            ) {
                Ok(pairs) => {
                    let hunk_total = crate::diff::hunks(&pairs).len();
                    diff = Some(DiffState { pairs, pos: (0, 0), ignore_ws, hunk_total });
                }
                Err(msg) => {
                    viewport.flash(msg, 60);
                }
            }
        }
        // If not exactly one other pane: leave diff=None, user will see plain view.
    }

    if args.scroll_lock && !others.is_empty() {
        lock_offsets = capture_lock_offsets(&viewport, &others, focused_pos);
        scroll_lock = true;
    }

    // If hide-mode filtering is active (--filter or --grep without --dim),
    // we need to scan the whole source up front to find matching lines.
    // Without any predicate this is intentionally skipped — lazy indexing
    // keeps `tess` fast on huge files.
    if (viewport.filter_active() || viewport.grep_active()) && !viewport.dim_mode() {
        idx.extend_to_end(src.as_ref());
        viewport.extend_visible_lines(&idx, src.as_ref());
    }

    // If follow or live mode is on at startup, snap to the bottom of the
    // (possibly filtered) source so the user sees the newest content
    // (tail-style). Live mode tracks whole-file rewrites; starting at the end
    // keeps the latest content in view as the file is regenerated.
    if viewport.follow_mode() || viewport.live_mode() {
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
    // Scroll position captured when an incremental-search prompt opens, so a
    // mid-type preview searches from there and Esc can restore it.
    let mut incsearch_origin: (usize, usize) = (0, 0);
    let mut current_file_index: usize = file_set.current_index();
    let mut transient_status: Option<String> = None;
    let mut tag_stack = TagStack::default();
    let mut overlay: Option<Box<dyn crate::overlay::Overlay>> = None;
    let mut overlay_flash: Option<(&'static str, std::time::Instant)> = None;
    let mut mouse_enabled = mouse_on;
    let clipboard_enabled = args.clipboard;
    let hscroll_shift = args.shift.unwrap_or(0);
    let wheel_lines = args.wheel_lines.unwrap_or(3).max(1);

    if let Some(tag_name) = args.tag.as_deref() {
        let _ = refresh_tag_file(&mut tag_file);
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

    sync_mouse_badge(&mut viewport, &mut others, mouse_enabled);

    loop {
        if sigterm.load(Ordering::SeqCst) {
            break;
        }

        if needs_redraw {
            if let Some(ov) = overlay.as_ref() {
                let w = cols;
                let h = viewport.body_rows() + 1;
                let mut ovframe = ov.render(w, h);
                if let Some((msg, started)) = overlay_flash {
                    if started.elapsed() < std::time::Duration::from_millis(1500) {
                        ovframe.status = format!("[{msg}]");
                    } else {
                        overlay_flash = None;
                    }
                }
                render_overlay(&mut stdout, &ovframe, w, h)
                    .map_err(|e| crate::error::Error::Runtime(format!("stdout: {}", e)))?;
                needs_redraw = false;
                continue;
            }
            // `-J` status column: feed the current file's marks (line → letter)
            // into the viewport so it can render mark glyphs in the far-left
            // gutter. Empty when the feature is off.
            if viewport.status_column() {
                let status_marks: HashMap<usize, char> = marks
                    .iter()
                    .filter(|(_, (fi, _))| *fi == current_file_index)
                    .map(|(ch, (_, line))| (*line, *ch))
                    .collect();
                viewport.set_status_marks(status_marks);
            }
            viewport.set_scroll_lock(scroll_lock);
            viewport.set_zoomed(zoomed);
            let mut frame = if zoomed && !others.is_empty() && diff.is_none() {
                viewport.frame(src.as_ref(), &mut idx)
            } else if diff.is_some() && others.len() == 1 {
                // Diff stays a 2-pane feature: render the focused pane against
                // the single other pane, ordered by `focused_pos` (0 → focused
                // left). `compose_diff`'s `focused_left` flag picks the side.
                let d = diff.as_ref().unwrap();
                let other = &mut others[0];
                let focused_left = focused_pos == 0;
                let (lw, _rw) = crate::pane::split_widths(cols);
                let pane_opts = diff_pane_opts(&viewport, lw);
                let body_rows = (rows - 1) as usize;
                let mut f = crate::diff_view::compose_diff(
                    &d.pairs,
                    src.as_ref(), &mut idx,
                    other.src.as_ref(), &mut other.idx,
                    d.pos, cols, lw, body_rows, &pane_opts, focused_left,
                );
                let cur = current_hunk(d);
                f.status = format!(
                    "[diff {}/{}]{}",
                    cur, d.hunk_total,
                    if d.ignore_ws { " [ws]" } else { "" }
                );
                f
            } else if !others.is_empty() {
                let n = others.len() + 1;
                let norm: Vec<Option<u16>> = (0..n).map(|i| pane_sizes.get(i).copied().flatten()).collect();
                let sizes = match orientation {
                    Orientation::Vertical => crate::pane::split_widths_n_weighted(cols, &norm),
                    Orientation::Horizontal => crate::pane::split_heights_n_weighted(rows, &norm),
                };
                if sizes.len() == 1 {
                    // Too narrow to split: focused pane full-width.
                    viewport.frame(src.as_ref(), &mut idx)
                } else {
                    // Re-derive each other pane's top under scroll-lock from the
                    // focused pane's top and the fixed per-column offsets.
                    if scroll_lock {
                        let focused_top = viewport.top_line();
                        for (slot, other) in others.iter_mut().enumerate() {
                            let phys = phys_of_slot(slot, focused_pos);
                            let pane_max = other.idx.line_count().saturating_sub(1);
                            let target = crate::pane::locked_pane_top(
                                focused_top,
                                lock_offsets[focused_pos],
                                lock_offsets[phys],
                                pane_max,
                            );
                            other.viewport.goto_line(target, other.src.as_ref(), &mut other.idx);
                        }
                    }
                    for other in others.iter_mut() {
                        other.viewport.set_scroll_lock(false);
                    }
                    // Render the focused frame first (borrows the loose locals),
                    // then build the physical-ordered frame list.
                    let focused_frame = viewport.frame(src.as_ref(), &mut idx);
                    let other_frames: Vec<Frame> = others
                        .iter_mut()
                        .map(|o| o.viewport.frame(o.src.as_ref(), &mut o.idx))
                        .collect();
                    let mut frames: Vec<Frame> = Vec::with_capacity(n);
                    for phys in 0..n {
                        if phys == focused_pos {
                            frames.push(focused_frame.clone());
                        } else {
                            frames.push(other_frames[other_slot(phys, focused_pos)].clone());
                        }
                    }
                    match orientation {
                        Orientation::Vertical => crate::pane::compose_panes(&frames, &sizes, cols, focused_pos),
                        Orientation::Horizontal => crate::pane::compose_panes_horizontal(&frames, &sizes, rows, focused_pos),
                    }
                }
            } else {
                viewport.frame(src.as_ref(), &mut idx)
            };
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
                InputMode::TagPrompt { buffer, error, .. } => {
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
            write_frame(&mut stdout, &frame, cols, rows, truecolor)
                .map_err(|e| crate::error::Error::Runtime(format!("stdout: {}", e)))?;
            needs_redraw = false;
        }

        // Poll with timeout so stdin sources can be re-checked. When an
        // animation is playing, shorten the wait to its next-frame deadline
        // so the timeout branch can advance frames on time.
        #[cfg(feature = "image")]
        let timeout = {
            let mut d = viewport.anim_deadline();
            for other in others.iter() {
                d = match (d, other.viewport.anim_deadline()) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
            }
            d.map(|x| x.min(BASE_POLL)).unwrap_or(BASE_POLL)
        };
        #[cfg(not(feature = "image"))]
        let timeout = BASE_POLL;
        match poll(timeout) {
            Ok(true) => {
                let event = read().map_err(|e| crate::error::Error::Runtime(format!("input: {}", e)))?;
                // Modal input handling: the search prompt and option prefix
                // intercept keys before they're translated to commands.
                match &mut mode {
                    InputMode::SearchPrompt { direction, buffer, error } => {
                        if let Event::Key(KeyEvent { code, .. }) = event {
                            match code {
                                KeyCode::Esc => {
                                    if viewport.incsearch() {
                                        viewport.set_top(incsearch_origin.0, incsearch_origin.1);
                                    }
                                    mode = InputMode::Normal;
                                    needs_redraw = true;
                                }
                                KeyCode::Enter => {
                                    if viewport.incsearch() {
                                        viewport.set_top(incsearch_origin.0, incsearch_origin.1);
                                    }
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
                                    if viewport.incsearch() {
                                        viewport.incsearch_preview(
                                            src.as_ref(), &mut idx, buffer, *direction, incsearch_origin);
                                    }
                                    needs_redraw = true;
                                }
                                KeyCode::Char(c) => {
                                    buffer.push(c);
                                    *error = None;
                                    if viewport.incsearch() {
                                        viewport.incsearch_preview(
                                            src.as_ref(), &mut idx, buffer, *direction, incsearch_origin);
                                    }
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
                        let is_z = matches!(
                            event,
                            Event::Key(KeyEvent { code: KeyCode::Char('z'), .. })
                        );
                        if is_z {
                            toggle_zoom(&mut zoomed, &mut viewport, &mut others, diff.is_some(), cols, rows, focused_pos, orientation, &pane_sizes);
                            needs_redraw = true;
                            mode = InputMode::Normal;
                            continue;
                        }
                        // Anything else: cancel and fall through to normal dispatch.
                        mode = InputMode::Normal;
                        // Don't `continue` — let the event fall through.
                    }
                    InputMode::BracketClosePending => {
                        // `]c` → DiffNextChange; any other key cancels.
                        if matches!(event, Event::Key(KeyEvent { code: KeyCode::Char('c'), .. })) {
                            if let Some(d) = diff.as_mut() {
                                match crate::diff::next_hunk(&d.pairs, d.pos.0) {
                                    Some(t) => { d.pos = (t, 0); }
                                    None => { viewport.flash("no more changes", 30); }
                                }
                            }
                        }
                        mode = InputMode::Normal;
                        needs_redraw = true;
                        continue;
                    }
                    InputMode::BracketOpenPending => {
                        // `[c` → DiffPrevChange; any other key cancels.
                        if matches!(event, Event::Key(KeyEvent { code: KeyCode::Char('c'), .. })) {
                            if let Some(d) = diff.as_mut() {
                                match crate::diff::prev_hunk(&d.pairs, d.pos.0) {
                                    Some(t) => { d.pos = (t, 0); }
                                    None => { viewport.flash("no more changes", 30); }
                                }
                            }
                        }
                        mode = InputMode::Normal;
                        needs_redraw = true;
                        continue;
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
                                            Ok(ColonCommand::VSplit(path_arg)) => {
                                                unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                if !others.is_empty() {
                                                    viewport.flash("already split (`:only` first)", 30);
                                                } else {
                                                    let (lw, rw) = crate::pane::split_widths(cols);
                                                    if rw == 0 {
                                                        viewport.flash("terminal too narrow to split", 30);
                                                    } else {
                                                        let focused_path = file_set.current().map(|p| p.to_path_buf());
                                                        let focused_ansi = viewport.ansi_mode();
                                                        let built = build_runtime_pane(
                                                            path_arg.as_deref(),
                                                            focused_path.as_deref(),
                                                            viewport.top_line(),
                                                            rw,
                                                            rows,
                                                            &args,
                                                            focused_ansi,
                                                            preprocessor.as_ref(),
                                                            record_start_regex.as_ref(),
                                                        );
                                                        match built {
                                                            Ok(mut pane) => {
                                                                force_cell_mode(&mut viewport);
                                                                force_cell_mode(&mut pane.viewport);
                                                                // New pane to the right; focused stays leftmost.
                                                                focused_pos = 0;
                                                                orientation = Orientation::Vertical;
                                                                viewport.resize(lw, rows);
                                                                pane.viewport.resize(rw, rows);
                                                                others.push(pane);
                                                                // New pane defaults to auto-size.
                                                                pane_sizes.resize(others.len() + 1, None);
                                                                sync_mouse_badge(&mut viewport, &mut others, mouse_enabled);
                                                            }
                                                            Err(e) => viewport.flash(format!("vsplit: {e}"), 40),
                                                        }
                                                    }
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::HSplit(path_arg)) => {
                                                unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                if !others.is_empty() {
                                                    viewport.flash("already split (`:only` first)", 30);
                                                } else {
                                                    let heights = crate::pane::split_heights_n(rows, 2);
                                                    if heights.len() == 1 {
                                                        viewport.flash("terminal too short to split", 30);
                                                    } else {
                                                        let focused_path = file_set.current().map(|p| p.to_path_buf());
                                                        let focused_ansi = viewport.ansi_mode();
                                                        let built = build_runtime_pane(
                                                            path_arg.as_deref(),
                                                            focused_path.as_deref(),
                                                            viewport.top_line(),
                                                            cols,
                                                            heights[1],
                                                            &args,
                                                            focused_ansi,
                                                            preprocessor.as_ref(),
                                                            record_start_regex.as_ref(),
                                                        );
                                                        match built {
                                                            Ok(mut pane) => {
                                                                force_cell_mode(&mut viewport);
                                                                force_cell_mode(&mut pane.viewport);
                                                                // New pane below; focused stays topmost.
                                                                focused_pos = 0;
                                                                orientation = Orientation::Horizontal;
                                                                viewport.resize(cols, heights[0]);
                                                                pane.viewport.resize(cols, heights[1]);
                                                                others.push(pane);
                                                                // New pane defaults to auto-size.
                                                                pane_sizes.resize(others.len() + 1, None);
                                                                sync_mouse_badge(&mut viewport, &mut others, mouse_enabled);
                                                            }
                                                            Err(e) => viewport.flash(format!("hsplit: {e}"), 40),
                                                        }
                                                    }
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::Rotate) => {
                                                unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                if others.is_empty() {
                                                    viewport.flash("no split to rotate", 30);
                                                } else if diff.is_some() {
                                                    viewport.flash("can't rotate in diff mode", 30);
                                                } else {
                                                    orientation = match orientation {
                                                        Orientation::Vertical => Orientation::Horizontal,
                                                        Orientation::Horizontal => Orientation::Vertical,
                                                    };
                                                    resize_split_aware(
                                                        &mut viewport, &mut others, cols, rows,
                                                        focused_pos, orientation, &pane_sizes,
                                                    );
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::Only) => {
                                                unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                if !others.is_empty() {
                                                    others.clear();
                                                    scroll_lock = false;
                                                    lock_offsets.clear();
                                                    pane_sizes.clear();
                                                    diff = None;
                                                    viewport.resize(cols, rows);
                                                    #[cfg(feature = "image")]
                                                    {
                                                        let (proto, cell_px) = startup_image_protocol;
                                                        viewport.set_image_protocol(proto, cell_px);
                                                    }
                                                    focused_pos = 0;
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::Layout(name)) => {
                                                unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                let layouts = crate::format::load_layouts().unwrap_or_default();
                                                match layouts.get(&name) {
                                                    None => { viewport.flash(format!("unknown layout: {name}"), 40); }
                                                    Some(layout) => {
                                                        let ansi = viewport.ansi_mode();
                                                        // Synthesize a per-pane Args from each pane spec. Use
                                                        // try_parse_from (NOT parse_from): a pane value starting
                                                        // with `-` (e.g. file="-x.log", display="-> {m}") makes
                                                        // clap fail, and parse_from would `process::exit` mid-loop
                                                        // — bypassing the terminal-restore Drop and garbling the
                                                        // screen. Route the error to a flash instead.
                                                        let mut pane_args: Vec<crate::cli::Args> = Vec::with_capacity(layout.panes.len());
                                                        let mut parse_err: Option<String> = None;
                                                        for pane in &layout.panes {
                                                            let mut toks = vec!["tess".to_string()];
                                                            // Emits the pane's view flags AND its file positional.
                                                            crate::format::expand_group_tokens(pane, &mut toks);
                                                            match <crate::cli::Args as clap::Parser>::try_parse_from(toks) {
                                                                Ok(a) => pane_args.push(a),
                                                                Err(e) => {
                                                                    parse_err = Some(e.to_string().lines().next().unwrap_or("parse error").to_string());
                                                                    break;
                                                                }
                                                            }
                                                        }
                                                        let built = if let Some(e) = parse_err {
                                                            Err(crate::error::Error::Runtime(e))
                                                        } else {
                                                            crate::layout::build_panes_from_sections(
                                                                &pane_args, ansi, cols, rows, preprocessor.as_ref(),
                                                            )
                                                        };
                                                        match built {
                                                            Ok(mut panes) if !panes.is_empty() => {
                                                                // Collapse any current split, install the new panes.
                                                                others.clear();
                                                                scroll_lock = false;
                                                                lock_offsets.clear();
                                                                // Layout rebuild: fresh pane_sizes (all auto for now;
                                                                // Task 5 will wire per-pane width/height from the layout).
                                                                pane_sizes.clear();
                                                                diff = None;
                                                                // pane 0 -> focused loose locals.
                                                                let p0 = panes.remove(0);
                                                                src = p0.src;
                                                                idx = p0.idx;
                                                                viewport = p0.viewport;
                                                                others = panes;
                                                                focused_pos = 0;
                                                                orientation = if layout.horizontal {
                                                                    Orientation::Horizontal
                                                                } else {
                                                                    Orientation::Vertical
                                                                };
                                                                force_cell_mode(&mut viewport);
                                                                for o in others.iter_mut() { force_cell_mode(&mut o.viewport); }
                                                                resize_split_aware(&mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                                sync_mouse_badge(&mut viewport, &mut others, mouse_enabled);
                                                            }
                                                            Ok(_) => { viewport.flash("layout produced no panes", 40); }
                                                            Err(e) => { viewport.flash(format!("layout: {e}"), 40); }
                                                        }
                                                    }
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::ScrollLock) => {
                                                if !others.is_empty() {
                                                    if scroll_lock {
                                                        scroll_lock = false;
                                                        lock_offsets.clear();
                                                    } else {
                                                        lock_offsets = capture_lock_offsets(
                                                            &viewport, &others, focused_pos,
                                                        );
                                                        scroll_lock = true;
                                                    }
                                                } else {
                                                    viewport.flash("scroll-lock needs a split", 40);
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::Zoom) => {
                                                toggle_zoom(&mut zoomed, &mut viewport, &mut others, diff.is_some(), cols, rows, focused_pos, orientation, &pane_sizes);
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::Mouse(arg)) => {
                                                let on = arg.unwrap_or(!mouse_enabled);
                                                apply_mouse(&mut stdout, &mut mouse_enabled, on, &mut viewport, &mut others);
                                                viewport.flash(if mouse_enabled { "mouse on" } else { "mouse off" }, 40);
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::SetEncoding(arg)) => {
                                                match arg {
                                                    Some(label) => {
                                                        match crate::charset::parse_label(&label) {
                                                            Some(enc) => {
                                                                viewport.set_encoding(enc);
                                                                for other in others.iter_mut() {
                                                                    other.viewport.set_encoding(enc);
                                                                }
                                                            }
                                                            None => viewport.flash(
                                                                format!("unknown encoding: {label}"),
                                                                40,
                                                            ),
                                                        }
                                                    }
                                                    None => viewport.flash(
                                                        format!("encoding: {}", viewport.encoding_label()),
                                                        40,
                                                    ),
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::Diff { force }) => {
                                                unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                                if others.len() >= 2 {
                                                    viewport.flash("diff needs exactly 2 panes", 40);
                                                } else if others.len() == 1 {
                                                    let other = &mut others[0];
                                                    let ignore_ws = diff.as_ref()
                                                        .map(|d| d.ignore_ws)
                                                        .unwrap_or(args.diff_ignore_whitespace);
                                                    match build_diff(
                                                        src.as_ref(), &mut idx,
                                                        other.src.as_ref(), &mut other.idx,
                                                        ignore_ws, force,
                                                    ) {
                                                        Ok(pairs) => {
                                                            let hunk_total = crate::diff::hunks(&pairs).len();
                                                            diff = Some(DiffState { pairs, pos: (0, 0), ignore_ws, hunk_total });
                                                        }
                                                        Err(msg) => {
                                                            viewport.flash(msg, 60);
                                                        }
                                                    }
                                                } else {
                                                    viewport.flash("diff needs a split of two files", 40);
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::NoDiff) => {
                                                diff = None;
                                                mode = InputMode::Normal;
                                            }
                                            Ok(ColonCommand::DiffToggleWs) => {
                                                if let Some(d) = diff.as_ref() {
                                                    let new_ws = !d.ignore_ws;
                                                    if others.len() == 1 {
                                                        let other = &mut others[0];
                                                        match build_diff(
                                                            src.as_ref(), &mut idx,
                                                            other.src.as_ref(), &mut other.idx,
                                                            new_ws, true,
                                                        ) {
                                                            Ok(pairs) => {
                                                                let hunk_total = crate::diff::hunks(&pairs).len();
                                                                diff = Some(DiffState { pairs, pos: (0, 0), ignore_ws: new_ws, hunk_total });
                                                            }
                                                            Err(msg) => {
                                                                viewport.flash(msg, 60);
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    viewport.flash("diff: not in diff mode", 40);
                                                }
                                                mode = InputMode::Normal;
                                            }
                                            Ok(cmd) => {
                                                let is_tag_cmd = matches!(
                                                    &cmd,
                                                    ColonCommand::Tag(_)
                                                        | ColonCommand::TagNext
                                                        | ColonCommand::TagPrev
                                                        | ColonCommand::TagSelect(_),
                                                );
                                                let reload_msg = if is_tag_cmd {
                                                    refresh_tag_file(&mut tag_file)
                                                } else {
                                                    None
                                                };
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
                                                // A runtime predicate command (:grep/:filter/:format)
                                                // clears the focused pane's visible-line cache; rebuild
                                                // it now so a hide-mode pane isn't left blank on a
                                                // static source. No-op when not hide-filtering or when
                                                // the cache is already current.
                                                if (viewport.filter_active() || viewport.grep_active())
                                                    && !viewport.dim_mode()
                                                {
                                                    idx.extend_to_end(src.as_ref());
                                                    viewport.extend_visible_lines(&idx, src.as_ref());
                                                }
                                                match outcome {
                                                    ColonOutcome::Continue(msg) => {
                                                        transient_status = msg.or(reload_msg);
                                                    }
                                                    ColonOutcome::Quit => break,
                                                    ColonOutcome::DispatchCommand(Command::OpenPicker) => {
                                                        let saved = (0..file_set.len())
                                                            .map(|i| if i == current_file_index { viewport.top_line() } else { 0 })
                                                            .collect::<Vec<_>>();
                                                        overlay = Some(Box::new(
                                                            crate::overlay::picker::FilePicker::new(&file_set, saved)
                                                        ));
                                                    }
                                                    ColonOutcome::DispatchCommand(Command::OpenHelp) => {
                                                        let remaps = keymap.user_keys_by_command_name();
                                                        overlay = Some(Box::new(
                                                            crate::overlay::help::HelpOverlay::new(remaps)
                                                        ));
                                                    }
                                                    ColonOutcome::DispatchCommand(Command::OpenTagPicker) => {
                                                        if let Some(active) = tag_stack.active.as_ref() {
                                                            overlay = Some(Box::new(
                                                                crate::overlay::tag_picker::TagPicker::new(
                                                                    active.name.clone(),
                                                                    active.matches.clone(),
                                                                    active.cursor,
                                                                )
                                                            ));
                                                        }
                                                    }
                                                    ColonOutcome::DispatchCommand(cmd) => {
                                                        debug_assert!(false, "colon dispatcher emitted unexpected Command: {cmd:?}");
                                                        // In release builds, silently no-op.
                                                    }
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
                    InputMode::TagPrompt { buffer, error, last_tab_matches } => {
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
                                        let reload_msg = refresh_tag_file(&mut tag_file);
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
                                        transient_status = msg.or(reload_msg);
                                        mode = InputMode::Normal;
                                    }
                                    needs_redraw = true;
                                }
                                KeyCode::Backspace => {
                                    buffer.pop();
                                    *error = None;
                                    *last_tab_matches = None;
                                    needs_redraw = true;
                                }
                                KeyCode::Tab => {
                                    let _ = refresh_tag_file(&mut tag_file);
                                    let names: Vec<String> = match tag_file.as_ref() {
                                        Some(tf) => tf
                                            .names()
                                            .filter(|n| n.starts_with(buffer.as_str()))
                                            .map(String::from)
                                            .collect(),
                                        None => Vec::new(),
                                    };
                                    match (names.len(), last_tab_matches.as_ref()) {
                                        (0, _) => {
                                            *error = Some("no tags match".into());
                                            *last_tab_matches = None;
                                        }
                                        (1, _) => {
                                            *buffer = names.into_iter().next().unwrap();
                                            *error = None;
                                            *last_tab_matches = None;
                                        }
                                        (n, Some(prev)) if prev.len() == n => {
                                            *error = Some(format!("{n} matches"));
                                        }
                                        (n, _) => {
                                            let lcp = longest_common_prefix(&names);
                                            if lcp.len() > buffer.len() {
                                                *buffer = lcp;
                                                *error = None;
                                            } else {
                                                *error = Some(format!("{n} matches"));
                                            }
                                            *last_tab_matches = Some(names);
                                        }
                                    }
                                    needs_redraw = true;
                                }
                                KeyCode::Char(c) => {
                                    buffer.push(c);
                                    *error = None;
                                    *last_tab_matches = None;
                                    needs_redraw = true;
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                    InputMode::Normal => {}
                }
                // Resize must update stored dims even when an overlay is active —
                // otherwise the overlay renders at stale dimensions until it closes.
                if let crossterm::event::Event::Resize(c, r) = event {
                    // Pin the bottom across resizes: a viewport sitting at the
                    // end (follow/live, or just scrolled to the bottom) must
                    // stay there when body height changes — otherwise the
                    // newest content drifts off-screen. Terminals/multiplexers
                    // often emit a resize right after startup, which is what
                    // made --follow/--live land short of the end.
                    let was_at_bottom = viewport.is_at_bottom(src.as_ref(), &idx);
                    cols = c;
                    rows = r;
                    if zoomed {
                        viewport.resize(cols, rows);
                    } else {
                        resize_split_aware(&mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                    }
                    if was_at_bottom {
                        viewport.goto_bottom(src.as_ref(), &mut idx);
                    }
                    needs_redraw = true;
                    if overlay.is_some() {
                        // Overlay still owns the screen; nothing else to do this tick.
                        continue;
                    }
                    // No overlay: fall through to normal handling so the
                    // existing Command::Resize path can do whatever else it does.
                }
                // Active overlay swallows input. Apply/Refuse/Close outcomes
                // are handled inline; CloseAnd defers to the normal command
                // dispatcher below.
                if let Some(ov) = overlay.as_mut() {
                    let outcome = match &event {
                        Event::Key(ke) => ov.handle_key(*ke),
                        Event::Mouse(me) => ov.handle_mouse(*me, viewport.body_rows()),
                        Event::Resize(_, _) => crate::overlay::OverlayOutcome::Stay,
                        _ => crate::overlay::OverlayOutcome::Stay,
                    };
                    match outcome {
                        crate::overlay::OverlayOutcome::Stay => {
                            needs_redraw = true;
                            continue;
                        }
                        crate::overlay::OverlayOutcome::Close => {
                            overlay = None;
                            overlay_flash = None;
                            needs_redraw = true;
                            continue;
                        }
                        crate::overlay::OverlayOutcome::CloseAnd(cmd) => {
                            overlay = None;
                            overlay_flash = None;
                            if let Command::SelectFile(i) = cmd {
                                if i < file_set.len() {
                                    file_set.set_current_index(i);
                                    if let Some(msg) = switch_to_current_file(
                                        &mut file_set, &mut current_file_index,
                                        &args, preprocessor.as_ref(),
                                        record_start_regex.as_ref(),
                                        &mut viewport, &mut src, &mut idx,
                                    ) {
                                        transient_status = Some(msg);
                                    }
                                }
                            } else if let Command::SelectTagMatch(idx_pick) = cmd {
                                if let Some(active) = tag_stack.active.as_mut() {
                                    if idx_pick < active.matches.len() {
                                        active.cursor = idx_pick;
                                        let entry = active.matches[idx_pick].clone();
                                        let msg = dispatch_match(
                                            &entry,
                                            &mut file_set,
                                            &mut current_file_index,
                                            &args,
                                            preprocessor.as_ref(),
                                            record_start_regex.as_ref(),
                                            &mut viewport,
                                            &mut src,
                                            &mut idx,
                                        );
                                        update_viewport_tag_indicator(&tag_stack, &mut viewport);
                                        if let Some(m) = msg {
                                            transient_status = Some(m);
                                        }
                                    }
                                }
                            }
                            needs_redraw = true;
                            continue;
                        }
                        crate::overlay::OverlayOutcome::Apply(cmd) => {
                            if let Command::DropFileAt(target) = cmd {
                                if file_set.len() > 1 && target < file_set.len() {
                                    let saved_cur = file_set.current_index();
                                    file_set.set_current_index(target);
                                    let _ = file_set.delete_current();
                                    // delete_current() moved the cursor itself; restore
                                    // the pre-drop position when the deletion was not OF
                                    // the saved cursor.
                                    if target < saved_cur {
                                        let restored = saved_cur.saturating_sub(1);
                                        file_set.set_current_index(restored);
                                    } else if target > saved_cur {
                                        file_set.set_current_index(saved_cur);
                                    }
                                    // (target == saved_cur: delete_current already landed on the nearest
                                    //  surviving file; nothing to restore.)
                                    if let Some(msg) = switch_to_current_file(
                                        &mut file_set, &mut current_file_index,
                                        &args, preprocessor.as_ref(),
                                        record_start_regex.as_ref(),
                                        &mut viewport, &mut src, &mut idx,
                                    ) {
                                        transient_status = Some(msg);
                                    }
                                    if let Some(ov) = overlay.as_mut() {
                                        ov.refresh(crate::overlay::OverlayContext { file_set: &file_set });
                                    }
                                }
                            }
                            needs_redraw = true;
                            continue;
                        }
                        crate::overlay::OverlayOutcome::Refuse(msg) => {
                            overlay_flash = Some((msg, std::time::Instant::now()));
                            needs_redraw = true;
                            continue;
                        }
                    }
                }
                // No-overlay mouse: scrollwheel scrolls the body. Other mouse
                // events are ignored to keep the body inert when --mouse is on
                // but no overlay is active.
                if let crossterm::event::Event::Mouse(me) = &event {
                    if mouse_enabled {
                        use crossterm::event::{KeyModifiers, MouseButton, MouseEventKind};
                        // Left-click focuses the pane under the cursor. Works in
                        // vertical and horizontal splits and under scroll-lock
                        // (like Tab — only focus moves); focus is locked in diff
                        // mode and a single pane has nothing to focus. The press
                        // is consumed either way, so it never falls through to the
                        // wheel/key paths below.
                        if let MouseEventKind::Down(MouseButton::Left) = me.kind {
                            if !others.is_empty() && diff.is_none() {
                                let n = others.len() + 1;
                                let norm: Vec<Option<u16>> = (0..n).map(|i| pane_sizes.get(i).copied().flatten()).collect();
                                let target = match orientation {
                                    Orientation::Vertical => {
                                        let widths = crate::pane::split_widths_n_weighted(cols, &norm);
                                        crate::pane::pane_at_column(me.column, &widths)
                                    }
                                    Orientation::Horizontal => {
                                        let heights = crate::pane::split_heights_n_weighted(rows, &norm);
                                        crate::pane::pane_at_row(me.row, &heights)
                                    }
                                };
                                if target != focused_pos {
                                    unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                    // Same move-by-value swap as Tab/FocusOtherPane:
                                    // the loop owns the loose locals, so they can't
                                    // be factored behind a `&mut` helper without a
                                    // placeholder for the non-Default source box.
                                    let focused = crate::pane::Pane {
                                        src,
                                        idx,
                                        viewport,
                                        last_revision,
                                        #[cfg(feature = "image")]
                                        last_tick,
                                    };
                                    let taken = std::mem::take(&mut others);
                                    let (nf, no, np) = set_focus(focused, taken, focused_pos, target);
                                    src = nf.src;
                                    idx = nf.idx;
                                    viewport = nf.viewport;
                                    last_revision = nf.last_revision;
                                    #[cfg(feature = "image")]
                                    let _ = nf.last_tick;
                                    others = no;
                                    focused_pos = np;
                                    resize_split_aware(&mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                                    needs_redraw = true;
                                }
                            }
                            continue;
                        }
                        // Route the wheel to the pane under the cursor when a
                        // split is active and independent. Scroll-locked panes
                        // move together and diff scrolls as one view, so those
                        // bypass routing (drive the focused pane / group as
                        // before). Single pane → only pane is the focused one.
                        let route = !others.is_empty() && !scroll_lock && diff.is_none();
                        let vis = if route {
                            let n = others.len() + 1;
                            let norm: Vec<Option<u16>> = (0..n).map(|i| pane_sizes.get(i).copied().flatten()).collect();
                            match orientation {
                                Orientation::Vertical => {
                                    let widths = crate::pane::split_widths_n_weighted(cols, &norm);
                                    crate::pane::pane_at_column(me.column, &widths)
                                }
                                Orientation::Horizontal => {
                                    let heights = crate::pane::split_heights_n_weighted(rows, &norm);
                                    crate::pane::pane_at_row(me.row, &heights)
                                }
                            }
                        } else {
                            focused_pos
                        };
                        // Resolve the visible pane index to the focused
                        // loose-locals or an `others` entry. The focused pane is
                        // composited in at `focused_pos`, so `others` (focused
                        // removed) is indexed by `vis` shifted past it.
                        let (tvp, tidx, tsrc): (&mut Viewport, &mut LineIndex, &dyn Source) =
                            if vis == focused_pos {
                                (&mut viewport, &mut idx, src.as_ref())
                            } else {
                                let j = vis - if vis > focused_pos { 1 } else { 0 };
                                let p = &mut others[j];
                                (&mut p.viewport, &mut p.idx, p.src.as_ref())
                            };
                        let hshift = me.modifiers.contains(KeyModifiers::SHIFT)
                            && tvp.hscroll_active();
                        match me.kind {
                            MouseEventKind::ScrollDown if hshift => {
                                tvp.hscroll_right_step();
                                needs_redraw = true;
                            }
                            MouseEventKind::ScrollUp if hshift => {
                                tvp.hscroll_left_step();
                                needs_redraw = true;
                            }
                            MouseEventKind::ScrollDown => {
                                tvp.scroll_lines(wheel_lines as i64, tsrc, tidx);
                                needs_redraw = true;
                            }
                            MouseEventKind::ScrollUp => {
                                tvp.scroll_lines(-(wheel_lines as i64), tsrc, tidx);
                                needs_redraw = true;
                            }
                            MouseEventKind::ScrollLeft => {
                                tvp.hscroll_left_step();
                                needs_redraw = true;
                            }
                            MouseEventKind::ScrollRight => {
                                tvp.hscroll_right_step();
                                needs_redraw = true;
                            }
                            _ => {}
                        }
                    }
                    continue;
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
                        if let Some(d) = diff.as_mut() {
                            // G/gg: go to top (gg) or bottom
                            match prefix_at_cmd {
                                Some(_) => {
                                    // numbered goto: go to top as approximation in diff mode
                                    d.pos = (0, 0);
                                }
                                None => {
                                    d.pos = (0, 0);
                                }
                            }
                            needs_redraw = true;
                        } else {
                            update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                            match prefix_at_cmd {
                                Some(line) if line > 0 => {
                                    viewport.goto_line(line - 1, src.as_ref(), &mut idx);
                                    viewport.suspend_follow_if(args.follow_suspend_on_motion);
                                }
                                _ => {
                                    viewport.goto_top();
                                    viewport.suspend_follow_if(args.follow_suspend_on_motion);
                                }
                            }
                            needs_redraw = true;
                        }
                    }
                    Command::GotoRecord => {
                        if let Some(d) = diff.as_mut() {
                            // G: go to last pair
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                let last = d.pairs.len().saturating_sub(1);
                                d.pos.0 = last;
                                let h = crate::diff_view::pair_height(
                                    &d.pairs[last], src.as_ref(), &mut idx,
                                    other.src.as_ref(), &mut other.idx, &lopts, &ropts,
                                );
                                d.pos.1 = h.saturating_sub(1);
                            }
                            needs_redraw = true;
                        } else {
                            update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                            match prefix_at_cmd {
                                Some(rec) if rec > 0 => {
                                    viewport.goto_record(rec - 1, src.as_ref(), &mut idx);
                                    viewport.suspend_follow_if(args.follow_suspend_on_motion);
                                }
                                _ => viewport.goto_bottom(src.as_ref(), &mut idx),
                            }
                            needs_redraw = true;
                        }
                    }
                    Command::GotoPercent => {
                        update_prev_position(&mut previous_position, current_file_index, viewport.top_line());
                        match prefix_at_cmd {
                            Some(p) if p <= 100 => viewport.goto_percent(p as u8, src.as_ref(), &mut idx),
                            _ => viewport.goto_top(),
                        }
                        viewport.suspend_follow_if(args.follow_suspend_on_motion);
                        needs_redraw = true;
                    }
                    Command::Quit => break,
                    Command::Resize(c, r) => {
                        let was_at_bottom = viewport.is_at_bottom(src.as_ref(), &idx);
                        cols = c; rows = r;
                        resize_split_aware(&mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                        if was_at_bottom {
                            viewport.goto_bottom(src.as_ref(), &mut idx);
                        }
                        needs_redraw = true;
                    }
                    Command::ScrollLines(n) => {
                        if let Some(d) = diff.as_mut() {
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                diff_scroll(d, n as isize, src.as_ref(), &mut idx, other.src.as_ref(), &mut other.idx, &lopts, &ropts);
                            }
                        } else {
                            viewport.scroll_lines(n, src.as_ref(), &mut idx);
                            viewport.suspend_follow_if(args.follow_suspend_on_motion);
                            if viewport.note_motion_for_eof(n > 0, src.as_ref(), &idx) { break; }
                        }
                        needs_redraw = true;
                    }
                    Command::ScrollLogicalLines(n) => {
                        if let Some(d) = diff.as_mut() {
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                diff_scroll(d, n as isize, src.as_ref(), &mut idx, other.src.as_ref(), &mut other.idx, &lopts, &ropts);
                            }
                        } else {
                            viewport.scroll_logical_lines(n, src.as_ref(), &mut idx);
                            viewport.suspend_follow_if(args.follow_suspend_on_motion);
                            if viewport.note_motion_for_eof(n > 0, src.as_ref(), &idx) { break; }
                        }
                        needs_redraw = true;
                    }
                    Command::PageDown => {
                        if let Some(d) = diff.as_mut() {
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                let body = (rows - 1) as isize;
                                diff_scroll(d, body, src.as_ref(), &mut idx, other.src.as_ref(), &mut other.idx, &lopts, &ropts);
                            }
                        } else {
                            viewport.page_down(src.as_ref(), &mut idx);
                            viewport.suspend_follow_if(args.follow_suspend_on_motion);
                            if viewport.note_motion_for_eof(true, src.as_ref(), &idx) { break; }
                        }
                        needs_redraw = true;
                    }
                    Command::PageUp => {
                        if let Some(d) = diff.as_mut() {
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                let body = (rows - 1) as isize;
                                diff_scroll(d, -body, src.as_ref(), &mut idx, other.src.as_ref(), &mut other.idx, &lopts, &ropts);
                            }
                        } else {
                            viewport.page_up(src.as_ref(), &mut idx);
                            viewport.suspend_follow_if(args.follow_suspend_on_motion);
                            viewport.note_motion_for_eof(false, src.as_ref(), &idx);
                        }
                        needs_redraw = true;
                    }
                    Command::HalfPageDown => {
                        if let Some(d) = diff.as_mut() {
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                let half = ((rows - 1) / 2) as isize;
                                diff_scroll(d, half, src.as_ref(), &mut idx, other.src.as_ref(), &mut other.idx, &lopts, &ropts);
                            }
                        } else {
                            viewport.half_page_down(src.as_ref(), &mut idx);
                            viewport.suspend_follow_if(args.follow_suspend_on_motion);
                            if viewport.note_motion_for_eof(true, src.as_ref(), &idx) { break; }
                        }
                        needs_redraw = true;
                    }
                    Command::HalfPageUp => {
                        if let Some(d) = diff.as_mut() {
                            if others.len() == 1 {
                                let other = &mut others[0];
                                let (lw, rw) = crate::pane::split_widths(cols);
                                let lopts = diff_pane_opts(&viewport, lw);
                                let ropts = diff_pane_opts(&viewport, rw);
                                let half = ((rows - 1) / 2) as isize;
                                diff_scroll(d, -half, src.as_ref(), &mut idx, other.src.as_ref(), &mut other.idx, &lopts, &ropts);
                            }
                        } else {
                            viewport.half_page_up(src.as_ref(), &mut idx);
                            viewport.suspend_follow_if(args.follow_suspend_on_motion);
                            viewport.note_motion_for_eof(false, src.as_ref(), &idx);
                        }
                        needs_redraw = true;
                    }
                    cmd @ (Command::FocusOtherPane | Command::FocusPrevPane) => {
                        let forward = cmd == Command::FocusOtherPane;
                        unzoom(&mut zoomed, &mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                        if diff.is_some() {
                            // In diff mode the panes are aligned and scroll as one
                            // unit, so "focus" is meaningless — and a swap would
                            // re-point src/idx out from under DiffState.pairs (whose
                            // line numbers are keyed to the original assignment),
                            // panicking on unequal-length files. Lock focus here.
                            viewport.flash("focus is locked in diff mode (:nodiff to exit)", 40);
                            needs_redraw = true;
                        } else if !others.is_empty() {
                            // Rotate focus by value: bundle the loose locals into a
                            // `Pane`, rotate the full physical order, then unpack the
                            // new focused pane back into the loose locals. No
                            // placeholders — `rotate_focus` moves everything.
                            let focused = crate::pane::Pane {
                                src,
                                idx,
                                viewport,
                                last_revision,
                                #[cfg(feature = "image")]
                                last_tick,
                            };
                            let taken = std::mem::take(&mut others);
                            let (nf, no, np) = rotate_focus(focused, taken, focused_pos, forward);
                            src = nf.src;
                            idx = nf.idx;
                            viewport = nf.viewport;
                            last_revision = nf.last_revision;
                            // The newly-focused pane's `last_tick` is intentionally
                            // dropped: the end-of-input block below resets the
                            // focused pane's tick clock to `now()` regardless (same
                            // as the pre-N-pane swap path did).
                            #[cfg(feature = "image")]
                            let _ = nf.last_tick;
                            others = no;
                            focused_pos = np;
                            // Re-assert physical-side widths: each viewport must be
                            // resized to the column it now occupies (matters on odd
                            // widths where columns differ).
                            resize_split_aware(&mut viewport, &mut others, cols, rows, focused_pos, orientation, &pane_sizes);
                            needs_redraw = true;
                        }
                    }
                    Command::ToggleScrollLock => {
                        if !others.is_empty() {
                            if scroll_lock {
                                scroll_lock = false;
                                lock_offsets.clear();
                            } else {
                                lock_offsets = capture_lock_offsets(&viewport, &others, focused_pos);
                                scroll_lock = true;
                            }
                            needs_redraw = true;
                        } else {
                            viewport.flash("scroll-lock needs a split", 40);
                            needs_redraw = true;
                        }
                    }
                    Command::ZoomPane => {
                        toggle_zoom(&mut zoomed, &mut viewport, &mut others, diff.is_some(), cols, rows, focused_pos, orientation, &pane_sizes);
                        needs_redraw = true;
                    }
                    Command::ToggleMouse => {
                        let on = !mouse_enabled;
                        apply_mouse(&mut stdout, &mut mouse_enabled, on, &mut viewport, &mut others);
                        viewport.flash(if mouse_enabled { "mouse on" } else { "mouse off" }, 40);
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
                        incsearch_origin = (viewport.top_line(), viewport.top_row());
                        mode = InputMode::SearchPrompt {
                            direction: SearchDirection::Forward,
                            buffer: String::new(),
                            error: None,
                        };
                        needs_redraw = true;
                    }
                    Command::SearchBackward => {
                        incsearch_origin = (viewport.top_line(), viewport.top_row());
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
                    Command::BracketClosePrefix => {
                        // Only arm the `]c` chord in diff mode; otherwise a bare
                        // `]` stays a no-op (don't swallow the next keystroke).
                        if diff.is_some() {
                            mode = InputMode::BracketClosePending;
                        }
                    }
                    Command::BracketOpenPrefix => {
                        if diff.is_some() {
                            mode = InputMode::BracketOpenPending;
                        }
                    }
                    Command::DiffNextChange => {
                        if let Some(d) = diff.as_mut() {
                            match crate::diff::next_hunk(&d.pairs, d.pos.0) {
                                Some(t) => { d.pos = (t, 0); needs_redraw = true; }
                                None => { viewport.flash("no more changes", 30); needs_redraw = true; }
                            }
                        }
                    }
                    Command::DiffPrevChange => {
                        if let Some(d) = diff.as_mut() {
                            match crate::diff::prev_hunk(&d.pairs, d.pos.0) {
                                Some(t) => { d.pos = (t, 0); needs_redraw = true; }
                                None => { viewport.flash("no more changes", 30); needs_redraw = true; }
                            }
                        }
                    }
                    Command::TagPrompt => {
                        if tag_file.is_none() {
                            transient_status = Some("[no tags file loaded]".into());
                            needs_redraw = true;
                        } else {
                            mode = InputMode::TagPrompt {
                                buffer: String::new(),
                                error: None,
                                last_tab_matches: None,
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
                    Command::OpenPicker => {
                        let saved = (0..file_set.len())
                            .map(|i| if i == current_file_index { viewport.top_line() } else { 0 })
                            .collect::<Vec<_>>();
                        overlay = Some(Box::new(
                            crate::overlay::picker::FilePicker::new(&file_set, saved)
                        ));
                        needs_redraw = true;
                    }
                    Command::OpenHelp => {
                        let remaps = keymap.user_keys_by_command_name();
                        overlay = Some(Box::new(
                            crate::overlay::help::HelpOverlay::new(remaps)
                        ));
                        needs_redraw = true;
                    }
                    Command::SelectFile(_)
                    | Command::DropFileAt(_)
                    | Command::SelectTagMatch(_)
                    | Command::OpenTagPicker => {
                        // Overlay-only outcomes; consumed by the routing block above.
                    }
                    Command::MouseEvent(_) => {
                        // Mouse handling lives in the event-routing block, not here.
                    }
                    Command::HScrollLeft => {
                        if hscroll_shift != 0 {
                            viewport.hscroll_left_cols(hscroll_shift);
                        } else {
                            viewport.hscroll_left_half();
                        }
                        needs_redraw = true;
                    }
                    Command::HScrollRight => {
                        if hscroll_shift != 0 {
                            viewport.hscroll_right_cols(hscroll_shift);
                        } else {
                            viewport.hscroll_right_half();
                        }
                        needs_redraw = true;
                    }
                    Command::HScrollLeftStep => {
                        viewport.hscroll_left_step();
                        needs_redraw = true;
                    }
                    Command::HScrollRightStep => {
                        viewport.hscroll_right_step();
                        needs_redraw = true;
                    }
                    Command::YankLine => {
                        let msg = yank_current_line(clipboard_enabled, &viewport, src.as_ref(), &mut idx);
                        transient_status = Some(msg);
                        needs_redraw = true;
                    }
                    Command::AnimPause => {
                        #[cfg(feature = "image")]
                        viewport.anim_toggle_pause();
                        needs_redraw = true;
                    }
                    Command::AnimStepForward => {
                        #[cfg(feature = "image")]
                        viewport.anim_step(1);
                        needs_redraw = true;
                    }
                    Command::AnimStepBack => {
                        #[cfg(feature = "image")]
                        viewport.anim_step(-1);
                        needs_redraw = true;
                    }
                    Command::AnimRestart => {
                        #[cfg(feature = "image")]
                        viewport.anim_restart();
                        needs_redraw = true;
                    }
                    Command::Noop => {}
                }
                // Reset the tick clock after handling input so a long idle
                // between keystrokes doesn't collapse into a burst of frame
                // advances on the next timeout.
                #[cfg(feature = "image")]
                {
                    last_tick = std::time::Instant::now();
                }
                #[cfg(feature = "image")]
                for other in others.iter_mut() {
                    other.last_tick = std::time::Instant::now();
                }
            }
            Ok(false) => {
                // Advance any playing animation by the elapsed time since the
                // last tick or input. tick() returns true when the visible
                // frame changed and a redraw is needed.
                #[cfg(feature = "image")]
                {
                    let dt = last_tick.elapsed();
                    last_tick = std::time::Instant::now();
                    if viewport.tick(dt) { needs_redraw = true; }
                }
                #[cfg(feature = "image")]
                for other in others.iter_mut() {
                    let odt = other.last_tick.elapsed();
                    other.last_tick = std::time::Instant::now();
                    if other.viewport.tick(odt) { needs_redraw = true; }
                }
                // Timeout — check whether the source has grown or been rewritten.
                // While diff mode is active, skip follow/live pumping: diff shows a
                // snapshot of both files; live-reloading underneath it would cause
                // the index to grow while the diff alignment is stale.
                if diff.is_some() {
                    // No-op: diff is a snapshot; let the user `:diff` to rebuild.
                } else if viewport.live_mode() {
                    let was_at_bottom = viewport.is_at_bottom(src.as_ref(), &idx);
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
                    let was_at_bottom = viewport.is_at_bottom(src.as_ref(), &idx);
                    src.pump();
                    if src.take_rotated() {
                        // File was rotated or truncated. Re-open from offset 0
                        // and reset the line index so we're not staring at
                        // stale mmap content. Snap to bottom of the fresh
                        // content (follow mode is on, so that's the natural
                        // place to land).
                        if let Some(path) = src.path().map(|p| p.to_path_buf()) {
                            match crate::open::open_source_for_path(
                                &path, &args, preprocessor.as_ref(),
                            ) {
                                Ok((new_src, _label, _err)) => {
                                    src = new_src;
                                    idx = LineIndex::new();
                                    if let Some(n) = rebuild_spec.head {
                                        idx.set_head_cap(n);
                                    }
                                    viewport.invalidate_filter_cache();
                                    idx.notice_new_bytes(src.as_ref());
                                    viewport.extend_visible_lines(&idx, src.as_ref());
                                    viewport.goto_bottom(src.as_ref(), &mut idx);
                                    viewport.flash("(F reopened)", 4);
                                    needs_redraw = true;
                                    continue;
                                }
                                Err(e) => {
                                    transient_status = Some(format!("[reopen failed: {e}]"));
                                    needs_redraw = true;
                                }
                            }
                        }
                    }
                    let lines_before = idx.line_count();
                    idx.notice_new_bytes(src.as_ref());
                    viewport.extend_visible_lines(&idx, src.as_ref());
                    if idx.line_count() != lines_before {
                        needs_redraw = true;
                        viewport.note_growth();
                        if was_at_bottom {
                            viewport.goto_bottom(src.as_ref(), &mut idx);
                        }
                    } else {
                        viewport.tick_idle();
                    }
                    viewport.tick_flash();
                    // `--exit-follow-on-close`: when the source signals
                    // that the upstream writer has finished (streaming
                    // stdin's reader thread exited), exit the pager.
                    // File sources are always complete from open, so this
                    // condition only fires for piped stdin.
                    if args.exit_follow_on_close && src.is_complete() {
                        break;
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
                // Drive follow/live growth for every background pane too, so a
                // split with growing files updates all halves.
                for other in others.iter_mut() {
                    if pump_pane(
                        &mut other.src,
                        &mut other.idx,
                        &mut other.viewport,
                        &mut other.last_revision,
                        &rebuild_spec,
                        &args,
                        preprocessor.as_ref(),
                    ) {
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

/// Pump a background pane's source for follow/live growth. Returns true if
/// anything changed (so the caller can flag a redraw).
///
/// This is the minimal subset of the focused pane's live/follow handling that
/// applies to a non-focused pane: whole-file rewrite (`--live`) rebuilds the
/// index and re-snaps to bottom if the pane was at bottom; otherwise (follow
/// mode or a still-streaming stdin source) new bytes are folded in and the
/// pane auto-scrolls when it was already at the bottom. A background follow
/// pane also reopens its source on rotation/truncation, mirroring the focused
/// path (minus the focused-only `(F reopened)` flash). The focused pane keeps
/// its own inlined logic verbatim because it also emits transient status
/// messages and uses loop control (`continue`/`break`) that can't move into a
/// free function — so this is a deliberate minimal duplication to avoid
/// disturbing the byte-identical focused path.
fn pump_pane(
    src: &mut Box<dyn Source>,
    idx: &mut LineIndex,
    viewport: &mut Viewport,
    last_revision: &mut u64,
    rebuild_spec: &RebuildSpec,
    args: &crate::cli::Args,
    preprocessor: Option<&crate::preprocess::Preprocessor>,
) -> bool {
    let mut changed = false;
    if viewport.live_mode() {
        let was_at_bottom = viewport.is_at_bottom(src.as_ref(), idx);
        src.pump();
        if src.revision() != *last_revision {
            rebuild_after_replace(src.as_ref(), viewport, idx, *rebuild_spec);
            if was_at_bottom {
                viewport.goto_bottom(src.as_ref(), idx);
            }
            *last_revision = src.revision();
            changed = true;
        }
    } else if viewport.follow_mode() || !src.is_complete() {
        let was_at_bottom = viewport.is_at_bottom(src.as_ref(), idx);
        src.pump();
        if src.take_rotated() {
            // File was rotated or truncated. Re-open from offset 0 and reset
            // the line index so we're not staring at stale mmap content, then
            // snap to bottom of the fresh content. Mirrors the focused pane's
            // inline reopen (same args + preprocessor so the reopened source
            // behaves identically), minus the focused-only `(F reopened)`
            // flash. A stdin source has no path and can't rotate anyway, so
            // the reopen is naturally skipped for it.
            if let Some(path) = src.path().map(|p| p.to_path_buf()) {
                if let Ok((new_src, _label, _err)) =
                    crate::open::open_source_for_path(&path, args, preprocessor)
                {
                    *src = new_src;
                    *idx = LineIndex::new();
                    if let Some(n) = rebuild_spec.head {
                        idx.set_head_cap(n);
                    }
                    viewport.invalidate_filter_cache();
                    idx.notice_new_bytes(src.as_ref());
                    viewport.extend_visible_lines(idx, src.as_ref());
                    viewport.goto_bottom(src.as_ref(), idx);
                    *last_revision = src.revision();
                    return true;
                }
                // Reopen failed: unlike the focused pane (which surfaces
                // `[reopen failed: ...]`), a background pane has no status
                // surface of its own, so we intentionally swallow the Err and
                // fall through to the normal growth path on the stale source.
            }
        }
        let lines_before = idx.line_count();
        idx.notice_new_bytes(src.as_ref());
        viewport.extend_visible_lines(idx, src.as_ref());
        if idx.line_count() != lines_before {
            changed = true;
            viewport.note_growth();
            if was_at_bottom {
                viewport.goto_bottom(src.as_ref(), idx);
            }
        } else {
            viewport.tick_idle();
        }
        viewport.tick_flash();
    }
    changed
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

fn to_crossterm_color(c: crate::ansi::Color, truecolor: bool) -> crossterm::style::Color {
    use crossterm::style::Color as CC;
    use crate::ansi::Color;
    match c {
        Color::Ansi(0) => CC::Black,
        Color::Ansi(1) => CC::DarkRed,
        Color::Ansi(2) => CC::DarkGreen,
        Color::Ansi(3) => CC::DarkYellow,
        Color::Ansi(4) => CC::DarkBlue,
        Color::Ansi(5) => CC::DarkMagenta,
        Color::Ansi(6) => CC::DarkCyan,
        Color::Ansi(7) => CC::Grey,
        Color::Ansi(8) => CC::DarkGrey,
        Color::Ansi(9) => CC::Red,
        Color::Ansi(10) => CC::Green,
        Color::Ansi(11) => CC::Yellow,
        Color::Ansi(12) => CC::Blue,
        Color::Ansi(13) => CC::Magenta,
        Color::Ansi(14) => CC::Cyan,
        Color::Ansi(15) => CC::White,
        Color::Ansi(_) => CC::Reset,
        Color::Indexed(n) => CC::AnsiValue(n),
        Color::Rgb(r, g, b) => {
            if truecolor {
                CC::Rgb { r, g, b }
            } else {
                CC::AnsiValue(crate::render::rgb_to_256(r, g, b))
            }
        }
        Color::Default => CC::Reset,
    }
}

/// Emit crossterm commands to transition `prev` → `next`. Caller must
/// already have written prior cells using `prev`'s state.
fn emit_style_diff<W: Write>(
    out: &mut W,
    prev: &crate::ansi::Style,
    next: &crate::ansi::Style,
    truecolor: bool,
) -> io::Result<()> {
    // For attribute toggles, crossterm has individual on/off pairs.
    // `NormalIntensity` cancels both bold AND dim — handle them together
    // to avoid emitting it twice when only one changed.
    let intensity_changed = prev.bold != next.bold || prev.dim != next.dim;

    // Color changes. ResetColor clears BOTH fg and bg simultaneously, so
    // if either changed to None we emit ResetColor first and then re-emit
    // the other if it's Some.
    let fg_changed = prev.fg != next.fg;
    let bg_changed = prev.bg != next.bg;

    if (fg_changed && next.fg.is_none()) || (bg_changed && next.bg.is_none()) {
        out.queue(ResetColor)?;
        // After ResetColor, re-emit any color that should remain set.
        if let Some(c) = next.fg {
            out.queue(SetForegroundColor(to_crossterm_color(c, truecolor)))?;
        }
        if let Some(c) = next.bg {
            out.queue(SetBackgroundColor(to_crossterm_color(c, truecolor)))?;
        }
    } else {
        if fg_changed {
            if let Some(c) = next.fg {
                out.queue(SetForegroundColor(to_crossterm_color(c, truecolor)))?;
            }
        }
        if bg_changed {
            if let Some(c) = next.bg {
                out.queue(SetBackgroundColor(to_crossterm_color(c, truecolor)))?;
            }
        }
    }

    if intensity_changed {
        if next.bold {
            out.queue(SetAttribute(Attribute::Bold))?;
        } else if next.dim {
            out.queue(SetAttribute(Attribute::Dim))?;
        } else {
            out.queue(SetAttribute(Attribute::NormalIntensity))?;
        }
    }
    if prev.italic != next.italic {
        out.queue(SetAttribute(if next.italic { Attribute::Italic } else { Attribute::NoItalic }))?;
    }
    if prev.underline != next.underline {
        out.queue(SetAttribute(if next.underline { Attribute::Underlined } else { Attribute::NoUnderline }))?;
    }
    if prev.reverse != next.reverse {
        out.queue(SetAttribute(if next.reverse { Attribute::Reverse } else { Attribute::NoReverse }))?;
    }
    if prev.strike != next.strike {
        out.queue(SetAttribute(if next.strike { Attribute::CrossedOut } else { Attribute::NotCrossedOut }))?;
    }
    Ok(())
}

fn emit_hyperlink_diff<W: Write>(
    out: &mut W,
    prev: &Option<Arc<str>>,
    next: &Option<Arc<str>>,
) -> io::Result<()> {
    if prev == next {
        return Ok(());
    }
    if prev.is_some() {
        out.write_all(b"\x1b]8;;\x1b\\")?;
    }
    if let Some(uri) = next {
        out.write_all(b"\x1b]8;;")?;
        out.write_all(uri.as_bytes())?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

/// DEC private mode 2026: synchronized output. Terminals that support it
/// (iTerm2, Kitty, WezTerm, Alacritty, Ghostty, foot, recent VTE,
/// Windows Terminal) buffer everything between `BEGIN` and `END` and
/// present the whole frame atomically; terminals that don't recognize the
/// sequence silently ignore it. This kills the flicker that would
/// otherwise appear during a frame's per-row repaint.
const SYNC_UPDATE_BEGIN: &[u8] = b"\x1b[?2026h";
const SYNC_UPDATE_END: &[u8] = b"\x1b[?2026l";

/// Fit `s` to exactly `cols` display columns: truncate on a char boundary at
/// the column limit, then pad with spaces. Used so the status row can be
/// overwritten in place by a single full-width write.
fn fit_status_to_cols(s: &str, cols: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let mut out = String::with_capacity(cols);
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > cols {
            break;
        }
        out.push(ch);
        w += cw;
    }
    for _ in w..cols {
        out.push(' ');
    }
    out
}

/// Draw the status row at the bottom. For plain-text status (the common case,
/// including the animation `[play i/n]` badge) we write a width-padded string
/// **in place with no preceding `Clear`** — so on terminals that ignore
/// synchronized output (DEC 2026), the row never blanks mid-repaint, which is
/// what caused the status-bar flicker during animation. A status carrying raw
/// escape sequences (a custom `--prompt` with `\e…`) can't be measured by
/// display width, so it keeps the clear-then-print path.
fn write_status_row(
    out: &mut impl Write,
    status: &str,
    status_style: &crate::ansi::Style,
    cols: u16,
    rows: u16,
    truecolor: bool,
) -> io::Result<()> {
    out.queue(MoveTo(0, rows.saturating_sub(1)))?;
    if status.contains('\x1b') {
        // Embedded-escape prompt: clear first, then emit verbatim (truncated on
        // a char boundary as a loose width bound — matches prior behavior).
        out.queue(Clear(ClearType::UntilNewLine))?;
        emit_style_diff(out, &crate::ansi::Style::default(), status_style, truecolor)?;
        let mut s = status.to_string();
        if s.len() > cols as usize {
            let mut end = cols as usize;
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
        }
        out.queue(Print(s))?;
    } else {
        emit_style_diff(out, &crate::ansi::Style::default(), status_style, truecolor)?;
        out.queue(Print(fit_status_to_cols(status, cols as usize)))?;
    }
    out.queue(ResetColor)?;
    out.queue(SetAttribute(Attribute::Reset))?;
    Ok(())
}

fn write_frame(out: &mut impl Write, frame: &Frame, cols: u16, rows: u16, truecolor: bool) -> io::Result<()> {
    // Raw mode (`-r` / `:color raw`) routes each visible body row through the
    // `frame.raw_rows` slot (populated by `Viewport::frame` when ansi_mode is
    // Raw). The writer below blasts those bytes to the terminal verbatim so
    // escape sequences like cursor moves and SGR pass through. Wrap math is
    // best-effort — terminal-driven wrapping may shift sub-rows under long
    // lines, matching `less -r`'s documented limitation.

    // Begin a synchronized update so the whole frame is presented atomically
    // (see SYNC_UPDATE_BEGIN). Paired with per-row `Clear(UntilNewLine)`
    // below, this replaces the previous global `Clear(All)` redraw and
    // eliminates the visible blank-frame flicker on every scroll keystroke.
    out.write_all(SYNC_UPDATE_BEGIN)?;

    // Reset attributes once before drawing so the first row starts clean.
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(ResetColor)?;

    if let Some(blob) = &frame.image_blob {
        // Clear the body region once so a prior frame's cells don't bleed
        // around/under the image, then emit the graphics escape verbatim.
        for r in 0..frame.body.len() as u16 {
            out.queue(MoveTo(0, r))?;
            out.queue(Clear(ClearType::UntilNewLine))?;
        }
        out.queue(MoveTo(0, 0))?;
        out.write_all(blob)?;
        write_status_row(out, &frame.status, &frame.status_style, cols, rows, truecolor)?;
        out.write_all(SYNC_UPDATE_END)?;
        return out.flush();
    }

    for (i, row) in frame.body.iter().enumerate() {
        out.queue(MoveTo(0, i as u16))?;
        // Wipe whatever was on this row in the previous frame. Cursor is
        // at col 0 so UntilNewLine clears the full row width, which also
        // covers the shrink-on-resize case (old cells past the new edge).
        out.queue(Clear(ClearType::UntilNewLine))?;
        // Defensive: every row begins with a full attribute reset, so a
        // mis-handled reset on the previous row can't bleed forward.
        out.queue(SetAttribute(Attribute::Reset))?;

        // Raw passthrough: when the viewport set this row's `raw_rows` entry,
        // write the original source bytes directly to the terminal instead of
        // rendering cells. Empty Vec means "skip" (mid-line wrap continuation
        // whose first row already emitted the bytes).
        if let Some(Some(raw)) = frame.raw_rows.get(i) {
            if !raw.is_empty() {
                out.write_all(raw)?;
            }
            // Trailing reset so a bare `\x1b[31m` doesn't leak into the next row.
            out.queue(ResetColor)?;
            out.queue(SetAttribute(Attribute::Reset))?;
            continue;
        }

        let row_style = frame.row_styles.get(i).copied().unwrap_or(RowStyle::Normal);
        // Build the base style representing the terminal state after the
        // defensive reset above. Dim rows get a dim base so the style-diff
        // tracker inside write_row_with_highlights starts from the correct
        // live terminal state.
        let base_style = if matches!(row_style, RowStyle::Dim) {
            out.queue(SetAttribute(Attribute::Dim))?;
            crate::ansi::Style { dim: true, ..Default::default() }
        } else {
            crate::ansi::Style::default()
        };
        let no_highlights = Vec::new();
        let highlights = frame.highlights.get(i).unwrap_or(&no_highlights);
        write_row_with_highlights(out, row, cols, highlights, base_style, truecolor)?;
    }
    // Status row
    write_status_row(out, &frame.status, &frame.status_style, cols, rows, truecolor)?;

    // End the synchronized update. The terminal flushes the buffered frame
    // atomically on receipt of this sequence.
    out.write_all(SYNC_UPDATE_END)?;
    out.flush()
}


/// Emit a single row with per-cell color/attribute transitions and
/// reverse-video highlights. Walks each cell, diffing style and hyperlink
/// from the previous cell, emitting only the transitions needed.
///
/// `base_style` is the terminal's live style state when this function is
/// entered (reflects any row-level attribute the caller already emitted,
/// e.g. `Dim` for `--dim` rows).
///
/// Highlight ranges toggle each cell's `reverse` attribute so highlights
/// compose correctly with cells that are already reverse-video.
fn write_row_with_highlights(
    out: &mut impl Write,
    row: &[Cell],
    cols: u16,
    highlights: &[std::ops::Range<usize>],
    base_style: crate::ansi::Style,
    truecolor: bool,
) -> io::Result<()> {
    let cols_usize = cols as usize;

    let mut ranges: Vec<std::ops::Range<usize>> = highlights
        .iter()
        .filter_map(|r| {
            let s = r.start.min(cols_usize);
            let e = r.end.min(cols_usize);
            if e > s { Some(s..e) } else { None }
        })
        .collect();
    ranges.sort_by_key(|r| r.start);

    // Style register starts at `base_style` — what the terminal currently
    // has live after any row-level attribute the caller emitted.
    let mut prev_style = base_style;
    let mut prev_link: Option<Arc<str>> = None;

    let mut col = 0usize;
    let mut i = 0usize;
    while col < cols_usize && i < row.len() {
        let in_highlight = ranges.iter().any(|r| r.start <= col && col < r.end);

        match &row[i] {
            Cell::Char { ch, width, style, hyperlink } => {
                // Effective style: cell's style with reverse toggled when in
                // a highlight, so highlight composes with already-reverse content.
                // Row-level dim (from `--dim` non-matching rows) is OR'd into
                // each cell unless the cell explicitly sets bold (bold and dim
                // share the SGR intensity slot; bold wins).
                let mut eff = *style;
                if in_highlight {
                    eff.reverse = !eff.reverse;
                }
                if base_style.dim && !eff.bold {
                    eff.dim = true;
                }
                emit_style_diff(out, &prev_style, &eff, truecolor)?;
                emit_hyperlink_diff(out, &prev_link, hyperlink)?;
                out.queue(Print(*ch))?;
                prev_style = eff;
                prev_link = hyperlink.clone();
                col += *width as usize;
            }
            Cell::Continuation => {
                // Already accounted for by the preceding wide char.
            }
            Cell::Empty => {
                // Background padding. Reset style to default so we don't
                // paint the rest of the line in the last active color —
                // but preserve the row-level dim so trailing padding on a
                // dim row stays dim.
                let default = if base_style.dim {
                    crate::ansi::Style { dim: true, ..Default::default() }
                } else {
                    crate::ansi::Style::default()
                };
                emit_style_diff(out, &prev_style, &default, truecolor)?;
                emit_hyperlink_diff(out, &prev_link, &None)?;
                out.queue(Print(' '))?;
                prev_style = default;
                prev_link = None;
                col += 1;
            }
        }
        i += 1;
    }

    // End-of-row: close any open hyperlink and reset color/attrs so the
    // next row's defensive Reset is a true no-op.
    emit_hyperlink_diff(out, &prev_link, &None)?;
    out.queue(ResetColor)?;
    out.queue(SetAttribute(Attribute::Reset))?;

    Ok(())
}

fn render_overlay(
    out: &mut impl Write,
    frame: &crate::overlay::OverlayFrame,
    width: u16,
    height: u16,
) -> io::Result<()> {
    // Mirror write_frame's atomic-frame discipline: synchronized update +
    // per-row clear, with a reverse-video status row to match the regular
    // viewport's look.
    out.write_all(SYNC_UPDATE_BEGIN)?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.queue(ResetColor)?;
    for row in 0..height.saturating_sub(1) {
        out.queue(MoveTo(0, row))?;
        out.queue(Clear(ClearType::UntilNewLine))?;
        out.queue(SetAttribute(Attribute::Reset))?;
        if let Some(line) = frame.body.get(row as usize) {
            let mut written = 0usize;
            for ch in line.chars() {
                let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if written + w > width as usize { break; }
                write!(out, "{ch}")?;
                written += w;
            }
        }
    }
    out.queue(MoveTo(0, height.saturating_sub(1)))?;
    out.queue(Clear(ClearType::UntilNewLine))?;
    out.queue(SetAttribute(Attribute::Reverse))?;
    let mut status = frame.status.clone();
    // TODO: use display width (not byte count) — mirrors write_frame's latent limitation.
    if status.len() > width as usize {
        status.truncate(width as usize);
    } else {
        let pad = width as usize - status.len();
        status.push_str(&" ".repeat(pad));
    }
    out.queue(Print(status))?;
    out.queue(ResetColor)?;
    out.queue(SetAttribute(Attribute::Reset))?;
    out.write_all(SYNC_UPDATE_END)?;
    out.flush()
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
    fn pane_phys_slot_mapping_roundtrips() {
        // focused at physical column 2 over 4 panes (others slots 0,1,2 map to
        // physical 0,1,3). other_slot is the inverse of phys_of_slot off-focus.
        for focused_pos in 0..4 {
            for slot in 0..3 {
                let phys = phys_of_slot(slot, focused_pos);
                assert_ne!(phys, focused_pos, "a slot never maps to the focused column");
                assert_eq!(other_slot(phys, focused_pos), slot, "round-trip slot↔phys");
            }
        }
    }

    #[test]
    fn rotate_index_wraps_both_directions() {
        // 3 panes: forward 0→1→2→0; backward 0→2→1→0.
        assert_eq!(rotate_index(0, 3, true), 1);
        assert_eq!(rotate_index(2, 3, true), 0);
        assert_eq!(rotate_index(0, 3, false), 2);
        assert_eq!(rotate_index(1, 3, false), 0);
        // 2 panes (the behavior-preserving case): Tab and BackTab both toggle.
        assert_eq!(rotate_index(0, 2, true), 1);
        assert_eq!(rotate_index(1, 2, true), 0);
        assert_eq!(rotate_index(0, 2, false), 1);
        assert_eq!(rotate_index(1, 2, false), 0);
    }

    // Build a Pane identifiable by its viewport's top_line, so a swap can be
    // verified without a real source or label getter.
    fn pane_with_id(id: usize) -> crate::pane::Pane {
        let mut vp = Viewport::new(80, 24, format!("p{id}"));
        vp.set_top(id, 0);
        crate::pane::Pane {
            src: Box::new(crate::source::MockSource::new()),
            idx: crate::line_index::LineIndex::new(),
            viewport: vp,
            last_revision: 0,
            #[cfg(feature = "image")]
            last_tick: std::time::Instant::now(),
        }
    }

    fn other_ids(others: &[crate::pane::Pane]) -> Vec<usize> {
        others.iter().map(|p| p.viewport.top_line()).collect()
    }

    #[test]
    fn set_focus_targets_specific_pane() {
        // Physical order p0,p1,p2; currently focused p1 (focused_pos=1),
        // so `others` is [p0, p2] in slot order.

        // Target the last pane.
        let (nf, no, np) = set_focus(pane_with_id(1), vec![pane_with_id(0), pane_with_id(2)], 1, 2);
        assert_eq!(np, 2);
        assert_eq!(nf.viewport.top_line(), 2);
        assert_eq!(other_ids(&no), vec![0, 1]);

        // Target the first pane.
        let (nf, no, np) = set_focus(pane_with_id(1), vec![pane_with_id(0), pane_with_id(2)], 1, 0);
        assert_eq!(np, 0);
        assert_eq!(nf.viewport.top_line(), 0);
        assert_eq!(other_ids(&no), vec![1, 2]);

        // Target the already-focused pane: focused and others unchanged.
        let (nf, no, np) = set_focus(pane_with_id(1), vec![pane_with_id(0), pane_with_id(2)], 1, 1);
        assert_eq!(np, 1);
        assert_eq!(nf.viewport.top_line(), 1);
        assert_eq!(other_ids(&no), vec![0, 2]);
    }

    #[test]
    fn toggle_zoom_flips_only_with_a_split_and_not_in_diff() {
        let mut vp = Viewport::new(80, 24, "f".into());
        let no_sizes: Vec<Option<u16>> = Vec::new();

        // Single pane (no others): no-op.
        let mut z = false;
        let mut none: Vec<crate::pane::Pane> = Vec::new();
        toggle_zoom(&mut z, &mut vp, &mut none, false, 80, 24, 0, Orientation::Vertical, &no_sizes);
        assert!(!z, "single pane must not zoom");

        // Split present but diff active: no-op.
        let mut others = vec![pane_with_id(1)];
        toggle_zoom(&mut z, &mut vp, &mut others, true, 80, 24, 0, Orientation::Vertical, &no_sizes);
        assert!(!z, "diff must not zoom");

        // Split present, no diff: flips on, then off.
        toggle_zoom(&mut z, &mut vp, &mut others, false, 80, 24, 0, Orientation::Vertical, &no_sizes);
        assert!(z, "split should zoom");
        toggle_zoom(&mut z, &mut vp, &mut others, false, 80, 24, 0, Orientation::Vertical, &no_sizes);
        assert!(!z, "second toggle unzooms");
    }

    #[test]
    fn unzoom_clears_and_is_idempotent() {
        let mut vp = Viewport::new(80, 24, "f".into());
        let mut others = vec![pane_with_id(1)];
        let mut z = true;
        let no_sizes: Vec<Option<u16>> = Vec::new();
        unzoom(&mut z, &mut vp, &mut others, 80, 24, 0, Orientation::Vertical, &no_sizes);
        assert!(!z);
        // Already unzoomed: stays false, no panic.
        unzoom(&mut z, &mut vp, &mut others, 80, 24, 0, Orientation::Vertical, &no_sizes);
        assert!(!z);
    }

    #[test]
    fn parse_colon_zoom() {
        assert_eq!(parse_colon_command("zoom").unwrap(), ColonCommand::Zoom);
    }

    #[test]
    fn current_line_bytes_strips_trailing_newline() {
        use crate::line_index::LineIndex;
        use crate::source::MockSource;
        let m = MockSource::new();
        // Three lines; the last has no trailing newline.
        m.append(b"alpha\nbravo\ncharlie");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        assert_eq!(idx.line_count(), 3);
        assert_eq!(current_line_bytes(&idx, &m, 0), b"alpha");
        assert_eq!(current_line_bytes(&idx, &m, 1), b"bravo");
        // Last line, no trailing newline, returned verbatim.
        assert_eq!(current_line_bytes(&idx, &m, 2), b"charlie");
    }

    #[test]
    fn write_frame_brackets_with_sync_update_and_no_full_clear() {
        // Locks in the flicker fix: every frame is wrapped in DEC mode 2026
        // begin/end escapes, and the previous global `Clear(All)` is gone
        // (replaced by per-row `Clear(UntilNewLine)`). If any of these
        // assumptions changes, flicker is likely to come back.
        use crate::ansi::Style;
        use crate::render::Cell;
        use crate::viewport::{Frame, RowStyle};

        let row: Vec<Cell> = (0..3)
            .map(|_| Cell::Char { ch: 'a', width: 1, style: Style::default(), hyperlink: None })
            .collect();
        let frame = Frame {
            body: vec![row.clone(), row],
            row_styles: vec![RowStyle::Normal, RowStyle::Normal],
            highlights: vec![Vec::new(), Vec::new()],
            status: "status".into(),
            status_style: crate::ansi::Style { reverse: true, ..Default::default() },
            raw_rows: vec![None, None],
            image_blob: None,
        };

        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &frame, 3, 3, true).unwrap();
        let s = std::str::from_utf8(&buf).expect("ascii");

        // Begin and end synchronized-update markers, in that order.
        let begin = s.find("\x1b[?2026h").expect("begin sync update");
        let end = s.find("\x1b[?2026l").expect("end sync update");
        assert!(begin < end, "begin must precede end");
        // Body content must sit between the markers.
        let first_a = s.find('a').expect("body char");
        assert!(begin < first_a && first_a < end, "body must be inside sync update");

        // Full-screen `Clear(All)` (`\x1b[2J`) must NOT appear — it was the
        // source of the flicker. Per-row clear-to-EOL (`\x1b[K`) is fine.
        assert!(
            !s.contains("\x1b[2J"),
            "full-screen Clear(All) reintroduced — flicker fix regressed: {s:?}",
        );
        assert!(s.contains("\x1b[K"), "expected at least one Clear(UntilNewLine)");
    }

    #[test]
    fn write_frame_emits_image_blob_verbatim_and_skips_cell_rows() {
        use crate::viewport::{Frame, RowStyle};
        let body_rows = 3usize;
        let cols = 10u16;
        let blob = b"\x1bPqDATA\x1b\\".to_vec();
        // Seed a body cell with a distinctive printable char; the image-blob
        // path must skip the per-row cell loop, so this char must NOT appear.
        let mut body = vec![vec![crate::render::Cell::Empty; cols as usize]; body_rows];
        body[0][0] = crate::render::Cell::Char { ch: 'Z', width: 1, style: crate::ansi::Style::default(), hyperlink: None };
        let frame = Frame {
            body,
            row_styles: vec![RowStyle::Normal; body_rows],
            highlights: vec![Vec::new(); body_rows],
            status: "img".to_string(),
            status_style: crate::ansi::Style::default(),
            raw_rows: vec![None; body_rows],
            image_blob: Some(blob.clone()),
        };
        let mut out: Vec<u8> = Vec::new();
        write_frame(&mut out, &frame, cols, (body_rows + 1) as u16, true).unwrap();
        let needle = b"\x1bPqDATA\x1b\\";
        assert!(out.windows(needle.len()).any(|w| w == needle), "image blob emitted verbatim");
        assert!(String::from_utf8_lossy(&out).contains("img"), "status still drawn");
        assert!(!String::from_utf8_lossy(&out).contains('Z'), "cell loop skipped: body cell not rendered");
    }

    #[test]
    fn fit_status_pads_by_display_width_not_bytes() {
        assert_eq!(fit_status_to_cols("ab", 5), "ab   ");
        // `×` is 2 bytes but 1 display column: must pad to 4 columns (3 spaces),
        // not byte-length (which would give only 2 spaces).
        assert_eq!(fit_status_to_cols("\u{00d7}", 4), "\u{00d7}   ");
        assert_eq!(fit_status_to_cols("hello", 3), "hel"); // truncate by columns
        assert_eq!(fit_status_to_cols("", 3), "   ");
    }

    #[test]
    fn plain_status_row_drawn_in_place_without_clear() {
        // image_blob path: the N body rows are cleared, but the plain status row
        // is drawn in place with NO Clear — so total Clear(UntilNewLine) == N.
        // (Pre-fix this was N+1; the extra clear was the status-bar flicker window.)
        let body_rows = 3usize;
        let cols = 12u16;
        let frame = Frame {
            body: vec![vec![crate::render::Cell::Empty; cols as usize]; body_rows],
            row_styles: vec![RowStyle::Normal; body_rows],
            highlights: vec![Vec::new(); body_rows],
            status: "[play 1/8]".to_string(),
            status_style: crate::ansi::Style::default(),
            raw_rows: vec![None; body_rows],
            image_blob: Some(b"\x1bPqX\x1b\\".to_vec()),
        };
        let mut out: Vec<u8> = Vec::new();
        write_frame(&mut out, &frame, cols, (body_rows + 1) as u16, true).unwrap();
        let clears = out.windows(3).filter(|w| *w == b"\x1b[K").count();
        assert_eq!(clears, body_rows, "status row must not be cleared (no blank-window flicker)");
        assert!(String::from_utf8_lossy(&out).contains("[play 1/8]"), "status text present");
    }

    #[test]
    fn escaped_status_keeps_clear_then_print() {
        // A status carrying raw escape sequences (custom --prompt) can't be
        // width-measured, so it retains the clear-then-print path: N body clears
        // + 1 status clear.
        let body_rows = 2usize;
        let cols = 20u16;
        let frame = Frame {
            body: vec![vec![crate::render::Cell::Empty; cols as usize]; body_rows],
            row_styles: vec![RowStyle::Normal; body_rows],
            highlights: vec![Vec::new(); body_rows],
            status: "\x1b[31mred\x1b[0m".to_string(),
            status_style: crate::ansi::Style::default(),
            raw_rows: vec![None; body_rows],
            image_blob: None,
        };
        let mut out: Vec<u8> = Vec::new();
        write_frame(&mut out, &frame, cols, (body_rows + 1) as u16, true).unwrap();
        let clears = out.windows(3).filter(|w| *w == b"\x1b[K").count();
        assert_eq!(clears, body_rows + 1, "embedded-escape status keeps the clear");
    }

    #[test]
    fn raw_rows_passthrough_emits_original_bytes_and_skips_continuation() {
        use crate::ansi::Style;
        use crate::render::Cell;
        use crate::viewport::{Frame, RowStyle};

        // One body row (since rows=2 means body_rows=1).
        let placeholder_row: Vec<Cell> = (0..3)
            .map(|_| Cell::Char { ch: 'X', width: 1, style: Style::default(), hyperlink: None })
            .collect();
        let frame = Frame {
            body: vec![placeholder_row.clone(), placeholder_row],
            row_styles: vec![RowStyle::Normal, RowStyle::Normal],
            highlights: vec![Vec::new(), Vec::new()],
            status: "s".into(),
            status_style: Style { reverse: true, ..Default::default() },
            // Row 0 emits raw bytes (with an embedded ESC); row 1 is a
            // continuation and emits nothing.
            raw_rows: vec![Some(b"\x1b[31mABC\x1b[0m".to_vec()), Some(Vec::new())],
            image_blob: None,
        };

        let mut buf: Vec<u8> = Vec::new();
        write_frame(&mut buf, &frame, 3, 3, true).unwrap();
        let s = std::str::from_utf8(&buf).expect("ascii");

        // The original SGR bytes must appear (raw passthrough).
        assert!(s.contains("\x1b[31mABC\x1b[0m"), "raw bytes missing in output: {s:?}");
        // The placeholder cells must NOT appear — we bypassed the cell pipeline.
        assert!(!s.contains("XXX"), "cell content leaked through despite raw passthrough: {s:?}");
    }

    #[test]
    fn dim_row_keeps_dim_through_plain_cells_and_padding() {
        // Regression: a row with base_style.dim=true and Cell::Char carrying
        // Style::default() used to emit `\x1b[22m` (NormalIntensity) on the
        // first char, killing the row-level dim and rendering the whole
        // line at normal intensity. Same for Cell::Empty padding cells.
        use crate::ansi::Style;
        use crate::render::Cell;
        let row = vec![
            Cell::Char { ch: 'h', width: 1, style: Style::default(), hyperlink: None },
            Cell::Char { ch: 'i', width: 1, style: Style::default(), hyperlink: None },
            Cell::Empty,
            Cell::Empty,
        ];
        let mut buf: Vec<u8> = Vec::new();
        let base = Style { dim: true, ..Default::default() };
        write_row_with_highlights(&mut buf, &row, 4, &[], base, true).unwrap();
        let s = String::from_utf8_lossy(&buf);

        // Locate every emitted character; before any of them is printed, the
        // dim attribute must NOT have been cleared.
        for needle in ['h', 'i'] {
            let pos = s.find(needle).expect("char printed");
            let before = &s[..pos];
            assert!(
                !before.contains("\x1b[22m"),
                "dim cleared before {needle:?}: {before:?}",
            );
        }
        // The Cell::Empty padding shouldn't clear dim either. Look at the
        // bytes between 'i' and the end-of-row Reset.
        let after_i = s.find('i').unwrap() + 1;
        let eor = s[after_i..].find("\x1b[0m").unwrap_or(s.len() - after_i);
        let pad = &s[after_i..after_i + eor];
        assert!(
            !pad.contains("\x1b[22m"),
            "dim cleared in padding region: {pad:?}",
        );
    }

    #[test]
    fn dim_row_yields_to_explicit_bold_cell() {
        // If a cell carries bold=true from ANSI, that wins over row-level
        // dim (bold and dim share the SGR intensity slot).
        use crate::ansi::Style;
        use crate::render::Cell;
        let row = vec![
            Cell::Char {
                ch: 'B',
                width: 1,
                style: Style { bold: true, ..Default::default() },
                hyperlink: None,
            },
        ];
        let mut buf: Vec<u8> = Vec::new();
        let base = Style { dim: true, ..Default::default() };
        write_row_with_highlights(&mut buf, &row, 1, &[], base, true).unwrap();
        let s = String::from_utf8_lossy(&buf);
        // Bold should be emitted (\x1b[1m); dim should not re-appear.
        assert!(s.contains("\x1b[1m"), "expected Bold escape, got {s:?}");
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
        let _guard = crate::test_env::lock();
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", "/home/user");
        let result = parse_colon_command("e ~/foo.log");
        match saved {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match result.unwrap() {
            ColonCommand::Edit(p) => assert_eq!(p, std::path::PathBuf::from("/home/user/foo.log")),
            other => panic!("expected Edit, got {other:?}"),
        }
    }

    #[test]
    fn parse_colon_vsplit_and_only() {
        assert_eq!(parse_colon_command("vsplit").unwrap(), ColonCommand::VSplit(None));
        assert_eq!(parse_colon_command("split a.log").unwrap(), ColonCommand::VSplit(Some("a.log".into())));
        assert_eq!(parse_colon_command("only").unwrap(), ColonCommand::Only);
        assert_eq!(parse_colon_command("close").unwrap(), ColonCommand::Only);
    }

    #[test]
    fn parse_colon_layout() {
        assert_eq!(parse_colon_command("layout dash").unwrap(), ColonCommand::Layout("dash".into()));
        assert!(parse_colon_command("layout").is_err()); // name required
    }

    #[test]
    fn parse_colon_scrolllock() {
        assert_eq!(parse_colon_command("scrolllock").unwrap(), ColonCommand::ScrollLock);
    }

    #[test]
    fn parse_colon_hsplit_and_rotate() {
        assert_eq!(parse_colon_command("hsplit").unwrap(), ColonCommand::HSplit(None));
        assert_eq!(parse_colon_command("hsplit a.log").unwrap(), ColonCommand::HSplit(Some("a.log".into())));
        assert_eq!(parse_colon_command("rotate").unwrap(), ColonCommand::Rotate);
    }

    #[test]
    fn parse_colon_search_skip_screen_and_tilde() {
        assert_eq!(parse_colon_command("search-skip-screen").unwrap(), ColonCommand::SearchSkipScreen);
        assert_eq!(parse_colon_command("tilde").unwrap(), ColonCommand::Tilde);
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
    fn parse_colon_tselect_without_arg_uses_active() {
        assert_eq!(parse_colon_command("tselect").unwrap(), ColonCommand::TagSelect(None));
    }

    #[test]
    fn parse_colon_tselect_with_name() {
        assert_eq!(
            parse_colon_command("tselect foo").unwrap(),
            ColonCommand::TagSelect(Some("foo".into())),
        );
    }

    #[test]
    fn parse_colon_b_opens_picker() {
        assert_eq!(parse_colon_command("b").unwrap(), ColonCommand::OpenPicker);
        assert_eq!(parse_colon_command("buffers").unwrap(), ColonCommand::OpenPicker);
    }

    #[test]
    fn parse_colon_help_opens_help() {
        assert_eq!(parse_colon_command("h").unwrap(), ColonCommand::OpenHelp);
        assert_eq!(parse_colon_command("help").unwrap(), ColonCommand::OpenHelp);
    }

    #[test]
    fn parse_colon_hex_with_valid_widths() {
        for n in [2usize, 4, 8, 16, 32] {
            assert_eq!(
                parse_colon_command(&format!("hex {n}")).unwrap(),
                ColonCommand::HexGroup(n),
            );
        }
    }

    #[test]
    fn parse_colon_hex_without_value_errors() {
        assert_eq!(
            parse_colon_command("hex").unwrap_err(),
            ColonParseError::HexGroupRequiresValue,
        );
    }

    #[test]
    fn parse_colon_hex_with_invalid_value_errors() {
        match parse_colon_command("hex 3").unwrap_err() {
            ColonParseError::HexGroupInvalid(v) => assert_eq!(v, "3"),
            other => panic!("expected HexGroupInvalid, got {other:?}"),
        }
        match parse_colon_command("hex banana").unwrap_err() {
            ColonParseError::HexGroupInvalid(v) => assert_eq!(v, "banana"),
            other => panic!("expected HexGroupInvalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_colon_color_without_arg_cycles() {
        assert_eq!(parse_colon_command("color").unwrap(), ColonCommand::Color(None));
    }

    #[test]
    fn parse_colon_color_with_named_mode() {
        use crate::render::AnsiMode;
        assert_eq!(
            parse_colon_command("color strict").unwrap(),
            ColonCommand::Color(Some(AnsiMode::Strict)),
        );
        assert_eq!(
            parse_colon_command("color interpret").unwrap(),
            ColonCommand::Color(Some(AnsiMode::Interpret)),
        );
        assert_eq!(
            parse_colon_command("color raw").unwrap(),
            ColonCommand::Color(Some(AnsiMode::Raw)),
        );
    }

    #[test]
    fn parse_colon_color_with_unknown_mode_errors() {
        match parse_colon_command("color rainbow").unwrap_err() {
            ColonParseError::ColorInvalid(v) => assert_eq!(v, "rainbow"),
            other => panic!("expected ColorInvalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_colon_case_without_arg_cycles() {
        assert_eq!(parse_colon_command("case").unwrap(), ColonCommand::Case(None));
    }

    #[test]
    fn parse_colon_case_with_named_mode() {
        use crate::viewport::CaseMode;
        assert_eq!(parse_colon_command("case smart").unwrap(),
                   ColonCommand::Case(Some(CaseMode::Smart)));
        assert_eq!(parse_colon_command("case sensitive").unwrap(),
                   ColonCommand::Case(Some(CaseMode::Sensitive)));
        assert_eq!(parse_colon_command("case insensitive").unwrap(),
                   ColonCommand::Case(Some(CaseMode::Insensitive)));
    }

    #[test]
    fn parse_colon_case_unknown_errors() {
        match parse_colon_command("case rainbow").unwrap_err() {
            ColonParseError::CaseInvalid(v) => assert_eq!(v, "rainbow"),
            other => panic!("expected CaseInvalid, got {other:?}"),
        }
    }

    #[test]
    fn parse_colon_hlsearch_on_off() {
        assert_eq!(parse_colon_command("hlsearch").unwrap(), ColonCommand::HlSearch(true));
        assert_eq!(parse_colon_command("nohlsearch").unwrap(), ColonCommand::HlSearch(false));
    }

    #[test]
    fn parse_colon_incsearch_toggle() {
        assert_eq!(parse_colon_command("incsearch").unwrap(), ColonCommand::IncSearch);
    }

    #[test]
    fn parse_colon_encoding() {
        assert_eq!(parse_colon_command("encoding iso-8859-1").unwrap(),
                   ColonCommand::SetEncoding(Some("iso-8859-1".to_string())));
        assert_eq!(parse_colon_command("encoding").unwrap(), ColonCommand::SetEncoding(None));
    }

    #[test]
    fn lcp_empty_slice() {
        assert_eq!(longest_common_prefix(&[]), "");
    }

    #[test]
    fn lcp_single_item_returns_self() {
        assert_eq!(longest_common_prefix(&["foo".into()]), "foo");
    }

    #[test]
    fn lcp_finds_shared_prefix() {
        let v: Vec<String> = vec!["foobar".into(), "foobaz".into(), "fooqux".into()];
        assert_eq!(longest_common_prefix(&v), "foo");
    }

    #[test]
    fn lcp_no_shared_prefix_returns_empty() {
        let v: Vec<String> = vec!["abc".into(), "xyz".into()];
        assert_eq!(longest_common_prefix(&v), "");
    }

    #[test]
    fn lcp_one_item_is_prefix_of_others() {
        let v: Vec<String> = vec!["foo".into(), "foobar".into(), "foobaz".into()];
        assert_eq!(longest_common_prefix(&v), "foo");
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

    #[test]
    fn writer_emits_color_for_red_cell() {
        let cells = vec![Cell::Char {
            ch: 'h',
            width: 1,
            style: crate::ansi::Style {
                fg: Some(crate::ansi::Color::Ansi(1)),
                ..Default::default()
            },
            hyperlink: None,
        }];
        let mut buf: Vec<u8> = Vec::new();
        write_row_with_highlights(&mut buf, &cells, 80, &[], crate::ansi::Style::default(), true).unwrap();
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("\x1b["), "expected ANSI escape in output: {s:?}");
        assert!(s.contains('h'));
    }

    #[test]
    fn writer_emits_osc8_for_hyperlink_cell() {
        let link: std::sync::Arc<str> = std::sync::Arc::from("https://example.com");
        let cells = vec![Cell::Char {
            ch: 'c',
            width: 1,
            style: crate::ansi::Style::default(),
            hyperlink: Some(link),
        }];
        let mut buf: Vec<u8> = Vec::new();
        write_row_with_highlights(&mut buf, &cells, 80, &[], crate::ansi::Style::default(), true).unwrap();
        let s = String::from_utf8_lossy(&buf);
        assert!(s.contains("\x1b]8;;https://example.com\x1b\\"), "got: {s:?}");
    }

    #[test]
    fn parse_colon_diff_commands() {
        assert_eq!(parse_colon_command("diff").unwrap(), ColonCommand::Diff { force: false });
        assert_eq!(parse_colon_command("diff!").unwrap(), ColonCommand::Diff { force: true });
        assert_eq!(parse_colon_command("nodiff").unwrap(), ColonCommand::NoDiff);
        assert_eq!(parse_colon_command("diffws").unwrap(), ColonCommand::DiffToggleWs);
    }

    #[test]
    fn parse_colon_predicate_commands() {
        assert_eq!(parse_colon_command("grep ERROR").unwrap(), ColonCommand::Grep(Some("ERROR".into())));
        assert_eq!(parse_colon_command("nogrep").unwrap(), ColonCommand::Grep(None));
        assert_eq!(parse_colon_command("filter status=404").unwrap(), ColonCommand::Filter(Some("status=404".into())));
        assert_eq!(parse_colon_command("nofilter").unwrap(), ColonCommand::Filter(None));
        assert_eq!(parse_colon_command("format nginx-combined").unwrap(), ColonCommand::Format(Some("nginx-combined".into())));
        assert_eq!(parse_colon_command("noformat").unwrap(), ColonCommand::Format(None));
        assert_eq!(parse_colon_command("display <status>").unwrap(), ColonCommand::Display(Some("<status>".into())));
        assert_eq!(parse_colon_command("nodisplay").unwrap(), ColonCommand::Display(None));
    }

    #[test]
    fn parse_colon_mouse() {
        assert_eq!(parse_colon_command("mouse").unwrap(), ColonCommand::Mouse(None));
        assert_eq!(parse_colon_command("mouse on").unwrap(), ColonCommand::Mouse(Some(true)));
        assert_eq!(parse_colon_command("mouse off").unwrap(), ColonCommand::Mouse(Some(false)));
        assert!(parse_colon_command("mouse bogus").is_err());
    }
}
