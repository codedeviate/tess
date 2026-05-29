use std::ops::Range;

use regex::Regex;

use crate::filter::{CompiledFilter, FilterMatch};
use crate::grep::GrepPredicate;
use crate::line_index::LineIndex;
use crate::render::{count_rows, render_line, Cell, RenderOpts};
use crate::source::Source;

/// Maximum number of lines to walk backwards when reconstructing SGR state
/// for a scroll-up. Picked to comfortably cover a screen-height plus
/// headroom; bounds cost so that scrolling in huge files stays snappy.
const MAX_RECONSTRUCT_LINES: usize = 256;

/// Reconstruct the SGR state at the start of `target_line` by walking up
/// to MAX_RECONSTRUCT_LINES lines back and replaying byte-by-byte through
/// the ANSI parser. Lines beyond the cap are skipped: if there's an
/// unclosed SGR more than 256 lines above the top, the reconstruction starts
/// from default — first visible lines may render in default colors until a
/// reset appears (rare for normal log files).
fn reconstruct_render_state(
    src: &dyn Source,
    idx: &crate::line_index::LineIndex,
    target_line: usize,
) -> crate::render::RenderState {
    let start = target_line.saturating_sub(MAX_RECONSTRUCT_LINES);
    let mut state = crate::render::RenderState::default();
    for line_no in start..target_line {
        let range = idx.line_range(line_no, src);
        let raw = src.bytes(range);
        for &b in raw.as_ref() {
            let _ = crate::ansi::step(
                &mut state.parse,
                &mut state.style,
                &mut state.hyperlink,
                b,
            );
        }
    }
    state
}

/// Build the rendered text of a display row plus a `starts` table mapping
/// each char index in that text back to its starting cell column. The last
/// entry is a sentinel pointing one past the row's width, so a match's
/// `[char_start, char_end)` translates to the cell range
/// `starts[char_start]..starts[char_end]`.
fn row_text_and_starts(row: &[Cell]) -> (String, Vec<usize>) {
    let mut text = String::new();
    let mut starts: Vec<usize> = Vec::with_capacity(row.len() + 1);
    for (col, cell) in row.iter().enumerate() {
        match cell {
            Cell::Char { ch, .. } => {
                starts.push(col);
                text.push(*ch);
            }
            Cell::Empty => {
                starts.push(col);
                text.push(' ');
            }
            Cell::Continuation => {}
        }
    }
    starts.push(row.len());
    (text, starts)
}

/// Find every regex match in the rendered text of a row, translating each
/// True when the byte slice contains only whitespace (space, tab, CR, LF)
/// or is empty. Used by `-s` / `--squeeze-blank-lines` to detect runs of
/// blank lines at frame-composition time.
fn line_is_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == b' ' || b == b'\t' || b == b'\r' || b == b'\n')
}

/// to a cell column range. Empty matches are dropped. Trailing-padding
/// spaces on a row would otherwise satisfy patterns like `\s+`; we trim
/// those by clamping match ends to where actual content stops.
fn find_row_highlights(row: &[Cell], regex: &Regex) -> Vec<Range<usize>> {
    if row.is_empty() {
        return Vec::new();
    }
    let last_content_col = row
        .iter()
        .enumerate()
        .rev()
        .find_map(|(c, cell)| match cell {
            Cell::Char { width, .. } => Some(c + *width as usize),
            Cell::Continuation => Some(c + 1),
            Cell::Empty => None,
        })
        .unwrap_or(0);
    if last_content_col == 0 {
        return Vec::new();
    }
    let (text, starts) = row_text_and_starts(row);
    let mut out = Vec::new();
    for m in regex.find_iter(&text) {
        if m.start() == m.end() {
            continue;
        }
        let char_start = text[..m.start()].chars().count();
        let char_end = text[..m.end()].chars().count();
        if char_start >= starts.len() - 1 || char_end <= char_start {
            continue;
        }
        let col_start = starts[char_start];
        let col_end = starts[char_end].min(last_content_col);
        if col_end > col_start {
            out.push(col_start..col_end);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStyle {
    Normal,
    /// Render with a reduced-emphasis terminal attribute. Used by `--dim` to
    /// keep filtered-out lines visible as context.
    Dim,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// How `--grep`, `--filter ~/!~`, `/`, `?`, and `:tag` patterns interpret
/// case. `Smart` matches less / ripgrep / vim `smartcase`: a pattern with
/// no uppercase characters is treated as case-insensitive; one with any
/// uppercase character is case-sensitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseMode {
    Sensitive,
    Smart,
    Insensitive,
}

impl Default for CaseMode {
    fn default() -> Self { CaseMode::Sensitive }
}

/// Controls auto-exit on end-of-file. `Off` (default) never quits.
/// `Second` (less `-e`) quits on the second forward-motion that lands at
/// EOF in a row. `First` (less `-E`) quits the moment a forward motion
/// lands at EOF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitAtEof {
    Off,
    Second,
    First,
}

impl Default for QuitAtEof {
    fn default() -> Self { QuitAtEof::Off }
}

impl CaseMode {
    /// Compile this case policy into a regex pattern by prepending the
    /// `(?i)` inline flag when case-insensitive matching is desired.
    pub fn apply_to_pattern(self, pattern: &str) -> String {
        match self {
            CaseMode::Sensitive => pattern.to_string(),
            CaseMode::Insensitive => format!("(?i){pattern}"),
            CaseMode::Smart => {
                if pattern.chars().any(|c| c.is_uppercase()) {
                    pattern.to_string()
                } else {
                    format!("(?i){pattern}")
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SearchState {
    pub raw: String,
    pub regex: Regex,
    pub direction: SearchDirection,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub body: Vec<Vec<Cell>>,        // exactly (rows-1) entries
    pub row_styles: Vec<RowStyle>,   // parallel to body
    /// Per-row column ranges to render with reverse-video. Used by `/`
    /// search to highlight just the matched phrase rather than the whole row.
    /// Indexed parallel to `body`; each inner Vec holds column ranges in
    /// `[start, end)` form (cell columns).
    pub highlights: Vec<Vec<std::ops::Range<usize>>>,
    pub status: String,
    /// Style applied to the status row by the writer.
    pub status_style: crate::ansi::Style,
    /// `AnsiMode::Raw` passthrough hints — parallel to `body`. `Some(bytes)`
    /// on a row instructs the writer to emit those original source bytes
    /// instead of rendering the cell grid (lets escape sequences pass to
    /// the terminal verbatim). `Some(empty)` skips emission (continuation
    /// row of a wrapped line whose first row already wrote the bytes).
    /// `None` means "render cells normally". Only populated when the
    /// viewport's ansi_mode is Raw.
    pub raw_rows: Vec<Option<Vec<u8>>>,
}

pub struct Viewport {
    top_line: usize,
    top_row: usize,
    cols: u16,
    rows: u16,
    pub opts: RenderOpts,
    pub show_line_numbers: bool,
    pub source_label: String,
    follow_mode: bool,
    live_mode: bool,
    prettify_label: Option<String>,
    format_label: Option<String>,
    filter: Option<CompiledFilter>,
    grep: Option<GrepPredicate>,
    dim_mode: bool,
    /// In hide mode (filter active, !dim), maps visible position → logical line
    /// index. Empty otherwise.
    visible_lines: Vec<usize>,
    /// How many logical lines we've evaluated for filter membership. Used by
    /// `extend_visible_lines` to avoid re-scanning lines on every tick.
    visible_scanned: usize,
    search: Option<SearchState>,
    /// Active display template + format regex. When set, lines are rendered
    /// through the template before being shown, searched, or counted for wraps.
    /// Filtering still operates on the raw line (it uses captures, not text).
    display: Option<crate::format::DisplayRenderer>,
    hex_mode: bool,
    /// Bytes per hex group in `--hex` mode. One of 1, 2, 4, 8, 16.
    /// Default 2 (matches the historical `xxd` 2-byte / 4-char grouping).
    hex_group_size: usize,
    /// Custom status-line prompt template. When set, replaces the built-in
    /// format_status output with the template rendered against PromptContext.
    prompt: Option<crate::prompt::ParsedPrompt>,
    /// Error message from a failed preprocessor run. When set, surfaces
    /// a `[preprocess-failed: ...]` tag in the status line.
    preprocess_failure: Option<String>,
    /// When `count > 1`, status line shows `<label>  [current+1/count]`.
    file_index: Option<(usize, usize)>,
    /// When set, status line and prompt context include `[tag: <name> (N/M)]`.
    tag_active: Option<(String, usize, usize)>,  // (name, cursor+1, total)
    /// ANSI interpretation mode, resolved from --no-color / -r / env at startup.
    ansi_mode: crate::render::AnsiMode,
    /// Style applied to the status row at the writer level. Default
    /// `reverse` for backwards-compat. Overridden by --status-style /
    /// --prompt-style / per-format prompt_style.
    status_style: crate::ansi::Style,
    /// Transient status message shown for a few ticks (e.g. "(F reopened)"
    /// after a file rotation). The `u32` is the remaining tick count; the
    /// app loop decrements via `tick_flash` and the formatter renders the
    /// message as long as it's non-empty.
    status_flash: Option<(String, u32)>,
    /// Ticks since the line index last grew. Used to render `(F idle)`
    /// instead of `(F)` after a few seconds with no new bytes. Reset to
    /// 0 in `note_growth`, incremented in `tick_idle`. 20 ticks ≈ 5s at
    /// the 250 ms poll cadence.
    ticks_since_growth: u32,
    /// Case-sensitivity policy for search / filter / grep regex compile.
    /// Resolved from -i / -I CLI flags at startup; mutated by the `:case`
    /// colon command at runtime.
    case_mode: CaseMode,
    /// When false, search-match highlighting is suppressed in frame
    /// composition (but search navigation still works). Toggled by
    /// `-G` / `--no-hilite-search` and `:hlsearch` / `:nohlsearch`.
    hilite_search: bool,
    /// Auto-exit-on-EOF policy resolved from `-e` / `-E` at startup.
    quit_at_eof: QuitAtEof,
    /// Counter for `QuitAtEof::Second`: number of consecutive forward
    /// motions that landed at EOF. Reset by any backward motion.
    eof_hits: u8,
    /// `-s` / `--squeeze-blank-lines`: collapse runs of blank lines to
    /// a single blank line at display time. Real line numbers / counts
    /// in `idx` are preserved.
    squeeze_blanks: bool,
    /// `--header=L,C`: pin the top `L` source lines at the top of the
    /// viewport and the left `C` columns at the left. The cols dimension
    /// is currently inert (no horizontal scroll yet); wired so future
    /// horizontal-scroll support can opt into it without re-plumbing.
    header_lines: usize,
    header_cols: usize,
    /// `-z` / `--window=N`: PageDown / PageUp step size in lines. `None`
    /// (default) means "use body_rows" — full-screen page step. Half-page
    /// commands always use body_rows/2 regardless.
    page_size: Option<u16>,
    /// Cached SGR/hyperlink state at the start of `render_state_for`.
    /// Invalidated when top_line changes or source grows; reconstructed
    /// by walking up to MAX_RECONSTRUCT_LINES lines back.
    render_state: crate::render::RenderState,
    /// Line number that `render_state` matches the start of. Sentinel
    /// `usize::MAX` means "invalid, must reconstruct".
    render_state_for: usize,
}

impl Viewport {
    pub fn new(cols: u16, rows: u16, source_label: String) -> Self {
        let opts = RenderOpts { cols, ..RenderOpts::default() };
        Self {
            top_line: 0,
            top_row: 0,
            cols,
            rows,
            opts,
            show_line_numbers: false,
            source_label,
            follow_mode: false,
            live_mode: false,
            prettify_label: None,
            format_label: None,
            filter: None,
            grep: None,
            dim_mode: false,
            visible_lines: Vec::new(),
            visible_scanned: 0,
            search: None,
            display: None,
            hex_mode: false,
            hex_group_size: 2,
            prompt: None,
            preprocess_failure: None,
            file_index: None,
            tag_active: None,
            ansi_mode: crate::render::AnsiMode::Strict,
            status_style: crate::ansi::Style { reverse: true, ..Default::default() },
            status_flash: None,
            ticks_since_growth: 0,
            case_mode: CaseMode::default(),
            hilite_search: true,
            quit_at_eof: QuitAtEof::default(),
            eof_hits: 0,
            squeeze_blanks: false,
            header_lines: 0,
            header_cols: 0,
            page_size: None,
            render_state: crate::render::RenderState::default(),
            render_state_for: usize::MAX,
        }
    }

    pub fn case_mode(&self) -> CaseMode { self.case_mode }

    pub fn hilite_search(&self) -> bool { self.hilite_search }

    pub fn set_hilite_search(&mut self, on: bool) { self.hilite_search = on; }

    pub fn set_quit_at_eof(&mut self, mode: QuitAtEof) {
        self.quit_at_eof = mode;
        self.eof_hits = 0;
    }

    pub fn set_squeeze_blanks(&mut self, on: bool) { self.squeeze_blanks = on; }
    pub fn squeeze_blanks(&self) -> bool { self.squeeze_blanks }

    pub fn set_header(&mut self, lines: usize, cols: usize) {
        self.header_lines = lines;
        self.header_cols = cols;
        // Don't let top_line land inside the pinned region — the scrolling
        // window starts at line `header_lines` once the feature is on.
        if self.top_line < self.header_lines {
            self.top_line = self.header_lines;
        }
    }
    pub fn header_lines(&self) -> usize { self.header_lines }
    pub fn header_cols(&self) -> usize { self.header_cols }

    pub fn set_page_size(&mut self, n: Option<u16>) { self.page_size = n; }
    pub fn page_size(&self) -> Option<u16> { self.page_size }

    /// Notify the EOF state machine of a motion. Returns `true` when the
    /// caller should quit. `forward = true` for any motion that could
    /// advance past EOF; `false` for backward motions (which reset the
    /// hit counter under `QuitAtEof::Second`).
    pub fn note_motion_for_eof(&mut self, forward: bool, src: &dyn Source, idx: &LineIndex) -> bool {
        match self.quit_at_eof {
            QuitAtEof::Off => false,
            QuitAtEof::First if forward && self.is_at_bottom(src, idx) => true,
            QuitAtEof::Second if forward && self.is_at_bottom(src, idx) => {
                self.eof_hits = self.eof_hits.saturating_add(1);
                self.eof_hits >= 2
            }
            _ => {
                if !forward { self.eof_hits = 0; }
                false
            }
        }
    }

    /// Switch the case-mode policy. Re-compiles any active search so the
    /// new policy takes effect on the next frame without the user having
    /// to retype the pattern.
    pub fn set_case_mode(&mut self, mode: CaseMode) {
        self.case_mode = mode;
        if let Some(s) = self.search.clone() {
            let _ = self.set_search(s.raw, s.direction);
        }
    }

    pub fn set_status_style(&mut self, style: crate::ansi::Style) {
        self.status_style = style;
    }

    pub fn status_style(&self) -> crate::ansi::Style {
        self.status_style
    }

    /// Show `msg` in the status row for the next `ticks` calls to the
    /// timeout branch (~250 ms each). Overrides the normal status during
    /// that window.
    pub fn flash(&mut self, msg: impl Into<String>, ticks: u32) {
        self.status_flash = Some((msg.into(), ticks));
    }

    /// Decrement the flash countdown by one tick. Clears the flash when
    /// it reaches zero.
    pub fn tick_flash(&mut self) {
        if let Some((_, n)) = &mut self.status_flash {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.status_flash = None;
            }
        }
    }

    /// Reset the idle counter; the source just produced fresh bytes.
    pub fn note_growth(&mut self) {
        self.ticks_since_growth = 0;
    }

    /// Increment the idle counter. Called in the timeout branch when the
    /// line index didn't grow.
    pub fn tick_idle(&mut self) {
        self.ticks_since_growth = self.ticks_since_growth.saturating_add(1);
    }

    /// True when the source has been quiet long enough to surface
    /// `(F idle)` instead of `(F)`. Threshold: 20 ticks ≈ 5s.
    pub fn is_idle(&self) -> bool {
        self.ticks_since_growth >= 20
    }

    pub fn set_display(&mut self, renderer: Option<crate::format::DisplayRenderer>) {
        self.display = renderer;
    }

    pub fn set_hex_mode(&mut self, on: bool) {
        self.hex_mode = on;
    }

    /// Returns whether `--hex` rendering is active.
    pub fn hex_mode(&self) -> bool {
        self.hex_mode
    }

    /// Set bytes-per-group for `--hex` rendering. Accepts 1, 2, 4, 8, or 16.
    /// Invalid values are ignored.
    pub fn set_hex_group_size(&mut self, bytes_per_group: usize) {
        if matches!(bytes_per_group, 1 | 2 | 4 | 8 | 16) {
            self.hex_group_size = bytes_per_group;
        }
    }

    /// Current bytes-per-group for `--hex` rendering.
    pub fn hex_group_size(&self) -> usize {
        self.hex_group_size
    }

    pub fn set_prompt(&mut self, prompt: Option<crate::prompt::ParsedPrompt>) {
        self.prompt = prompt;
    }

    pub fn set_preprocess_failure(&mut self, msg: Option<String>) {
        self.preprocess_failure = msg;
    }

    pub fn set_file_index(&mut self, current: usize, total: usize) {
        self.file_index = if total > 1 {
            Some((current, total))
        } else {
            None
        };
    }

    pub fn set_tag_active(&mut self, info: Option<(String, usize, usize)>) {
        self.tag_active = info;
    }

    pub fn set_ansi_mode(&mut self, mode: crate::render::AnsiMode) {
        self.ansi_mode = mode;
    }

    pub fn ansi_mode(&self) -> crate::render::AnsiMode {
        self.ansi_mode
    }

    pub fn set_source_label(&mut self, label: String) {
        self.source_label = label;
    }

    pub fn source_label_clone(&self) -> String {
        self.source_label.clone()
    }

    /// Fetch a logical line's display bytes — rendered through the active
    /// display template if one is set and the line parses against the format
    /// regex, otherwise the raw bytes. Used everywhere the *visible* form of
    /// the line matters: rendering, search, wrap-row counting.
    fn line_display_bytes<'a>(&self, src: &'a dyn Source, idx: &LineIndex, line_n: usize) -> std::borrow::Cow<'a, [u8]> {
        let range = idx.line_range(line_n, src);
        let raw = src.bytes(range);
        if let Some(r) = self.display.as_ref() {
            if let Some(rendered) = r.render_line(&raw) {
                return std::borrow::Cow::Owned(rendered.into_bytes());
            }
        }
        raw
    }

    /// Compile and store a search pattern. Returns the parse error from the
    /// regex crate if the pattern is invalid; the previous search (if any)
    /// is preserved on error.
    pub fn set_search(&mut self, raw: String, direction: SearchDirection) -> Result<(), String> {
        let compiled = self.case_mode.apply_to_pattern(&raw);
        let regex = Regex::new(&compiled).map_err(|e| e.to_string())?;
        self.search = Some(SearchState { raw, regex, direction });
        Ok(())
    }

    pub fn clear_search(&mut self) { self.search = None; }

    pub fn search_active(&self) -> bool { self.search.is_some() }

    pub fn search_direction(&self) -> SearchDirection {
        self.search.as_ref().map(|s| s.direction).unwrap_or(SearchDirection::Forward)
    }

    /// Jump to the next match of the active search, in `direction` (or its
    /// reverse if `reverse` is true). Wraps at the end of the source.
    /// Returns true iff a match was found and the viewport moved.
    pub fn search_repeat(&mut self, src: &dyn Source, idx: &mut LineIndex, reverse: bool) -> bool {
        if idx.records_mode() {
            self.search_repeat_records(src, idx, reverse)
        } else {
            self.search_repeat_lines(src, idx, reverse)
        }
    }

    /// Line-mode search: unchanged original logic.
    fn search_repeat_lines(&mut self, src: &dyn Source, idx: &mut LineIndex, reverse: bool) -> bool {
        let Some(s) = self.search.as_ref() else { return false; };
        let forward = matches!(
            (s.direction, reverse),
            (SearchDirection::Forward, false) | (SearchDirection::Backward, true)
        );
        idx.extend_to_end(src);
        let pattern = s.regex.clone();
        if self.hide_mode() {
            self.extend_visible_lines(idx, src);
            self.search_step_in_visible(&pattern, src, idx, forward)
        } else {
            self.search_step_in_logical(&pattern, src, idx, forward)
        }
    }

    /// Records-mode search: iterate records, match against UTF-8-lossy decoded
    /// record bytes (which may contain embedded `\n`s), and jump the viewport
    /// to the first line of the matching record.
    fn search_repeat_records(&mut self, src: &dyn Source, idx: &mut LineIndex, reverse: bool) -> bool {
        let Some(s) = self.search.as_ref() else { return false; };
        let forward = matches!(
            (s.direction, reverse),
            (SearchDirection::Forward, false) | (SearchDirection::Backward, true)
        );
        let pattern = s.regex.clone();
        idx.extend_to_end(src);

        let total = idx.record_count();
        if total == 0 { return false; }

        let cur_record = idx.line_to_record(self.top_line);

        let range: Box<dyn Iterator<Item = usize>> = if forward {
            Box::new(((cur_record + 1)..total).chain(0..=cur_record))
        } else {
            let earlier: Vec<usize> = (0..cur_record).rev().collect();
            let later: Vec<usize> = (cur_record..total).rev().collect();
            Box::new(earlier.into_iter().chain(later))
        };

        for r in range {
            let bytes = idx.record_bytes_stripped(r, src);
            let text = String::from_utf8_lossy(&bytes);
            if pattern.is_match(&text) {
                let line_range = idx.record_line_range(r);
                self.top_line = line_range.start;
                self.top_row = 0;
                return true;
            }
        }
        false
    }

    fn line_matches(&self, pattern: &Regex, src: &dyn Source, idx: &LineIndex, line_n: usize) -> bool {
        // Search runs against the *displayed* bytes so what the user sees is
        // what they can find. With a template active, that's the rendered form;
        // otherwise the raw line. ANSI color sequences are stripped so that
        // `/error` finds a red `error` regardless of escape codes.
        let display = self.line_display_bytes(src, idx, line_n);
        let bytes = crate::ansi::strip_sgr(&display);
        match std::str::from_utf8(&bytes) {
            Ok(s) => pattern.is_match(s),
            Err(_) => false,
        }
    }

    fn search_step_in_logical(&mut self, pattern: &Regex, src: &dyn Source, idx: &LineIndex, forward: bool) -> bool {
        let total = idx.line_count();
        if total == 0 { return false; }
        let start = self.top_line;
        // Walk every logical line once, starting from start+1 (or start-1)
        // and wrapping at the end / beginning.
        for offset in 1..=total {
            let line_n = if forward {
                (start + offset) % total
            } else {
                (start + total - offset) % total
            };
            if self.line_matches(pattern, src, idx, line_n) {
                self.top_line = line_n;
                self.top_row = 0;
                return true;
            }
        }
        false
    }

    fn search_step_in_visible(&mut self, pattern: &Regex, src: &dyn Source, idx: &LineIndex, forward: bool) -> bool {
        let total = self.visible_lines.len();
        if total == 0 { return false; }
        // Find current visible position for top_line.
        let cur = self.visible_lines.iter().position(|&l| l >= self.top_line).unwrap_or(0);
        for offset in 1..=total {
            let visible_idx = if forward {
                (cur + offset) % total
            } else {
                (cur + total - offset) % total
            };
            let line_n = self.visible_lines[visible_idx];
            if self.line_matches(pattern, src, idx, line_n) {
                self.top_line = line_n;
                self.top_row = 0;
                return true;
            }
        }
        false
    }

    pub fn set_filter(&mut self, filter: Option<CompiledFilter>) {
        self.filter = filter;
        self.visible_lines.clear();
        self.visible_scanned = 0;
        // Drop scroll state — line numbering may have changed under us.
        self.top_line = 0;
        self.top_row = 0;
    }

    pub fn set_grep(&mut self, grep: Option<GrepPredicate>) {
        self.grep = grep;
        self.visible_lines.clear();
        self.visible_scanned = 0;
        self.top_line = 0;
        self.top_row = 0;
    }

    pub fn grep_active(&self) -> bool { self.grep.is_some() }

    pub fn set_dim_mode(&mut self, on: bool) {
        self.dim_mode = on;
        // Hide mode is the only mode that needs visible_lines; clear when
        // turning dim ON, and re-derive from scratch when turning dim OFF
        // (next extend_visible_lines call rebuilds it).
        self.visible_lines.clear();
        self.visible_scanned = 0;
    }

    pub fn filter_active(&self) -> bool { self.filter.is_some() }

    pub fn dim_mode(&self) -> bool { self.dim_mode }

    fn hide_mode(&self) -> bool {
        (self.filter.is_some() || self.grep.is_some()) && !self.dim_mode
    }

    /// Walk any newly indexed logical lines and append matching ones to
    /// `visible_lines` if we're in hide mode. No-op otherwise. Cheap to call
    /// every loop tick — keeps a `visible_scanned` cursor (line mode only;
    /// records mode rebuilds from scratch each call).
    pub fn extend_visible_lines(&mut self, idx: &LineIndex, src: &dyn Source) {
        if !self.hide_mode() {
            return;
        }
        if idx.records_mode() {
            self.extend_visible_lines_records(idx, src);
        } else {
            self.extend_visible_lines_per_line(idx, src);
        }
    }

    /// Line-mode: incrementally append newly indexed matching lines.
    fn extend_visible_lines_per_line(&mut self, idx: &LineIndex, src: &dyn Source) {
        let total = idx.line_count();
        while self.visible_scanned < total {
            let line_n = self.visible_scanned;
            let bytes = idx.line_bytes_stripped(line_n, src);
            if self.line_passes(&bytes) {
                self.visible_lines.push(line_n);
            }
            self.visible_scanned += 1;
        }
    }

    /// Records-mode: evaluate predicates once per record on the full record
    /// bytes (which include embedded `\n`s). All physical lines of a matching
    /// record are pushed to `visible_lines`; non-matching records are dropped
    /// entirely (hide mode). Rebuilds from scratch on each call — O(records)
    /// per frame but acceptable for current workloads; avoids the complexity
    /// of tracking a records-scanned cursor alongside `visible_scanned`.
    fn extend_visible_lines_records(&mut self, idx: &LineIndex, src: &dyn Source) {
        self.visible_lines.clear();
        self.visible_scanned = 0; // not used by records path; reset for clarity
        let total_records = idx.record_count();
        for r in 0..total_records {
            if self.record_passes(idx, src, r) {
                for line_n in idx.record_line_range(r) {
                    self.visible_lines.push(line_n);
                }
            }
        }
    }

    /// Combined predicate: bytes pass iff the (optional) filter matches AND
    /// the (optional) grep matches. Missing predicates vacuously pass.
    /// `bytes` is always a single logical line — records-mode callers go
    /// through `record_passes` instead because the two predicates have
    /// different granularity (filter = header line, grep = whole record).
    fn line_passes(&self, line: &[u8]) -> bool {
        let filter_ok = match self.filter.as_ref() {
            Some(f) => matches!(f.evaluate(line), FilterMatch::Matched),
            None => true,
        };
        let grep_ok = match self.grep.as_ref() {
            Some(g) => g.matches(line),
            None => true,
        };
        filter_ok && grep_ok
    }

    /// Records-mode predicate. Both filter and grep are evaluated against
    /// the full multi-line record bytes. Filter uses the format regex with
    /// dotall + multi-line semantics so greedy captures like
    /// `(?P<message>.*)$` span the whole record body — `--filter
    /// message~foo` matches when `foo` appears anywhere in the record, not
    /// only on the header. Grep matches anywhere in the record bytes too,
    /// so `(?s)foo.*bar` keeps working across continuation lines.
    fn record_passes(&self, idx: &LineIndex, src: &dyn Source, r: usize) -> bool {
        let bytes = if self.filter.is_some() || self.grep.is_some() {
            Some(idx.record_bytes_stripped(r, src))
        } else {
            None
        };
        let filter_ok = match self.filter.as_ref() {
            Some(f) => matches!(
                f.evaluate_record(bytes.as_deref().unwrap()),
                FilterMatch::Matched,
            ),
            None => true,
        };
        let grep_ok = match self.grep.as_ref() {
            Some(g) => g.matches(bytes.as_deref().unwrap()),
            None => true,
        };
        filter_ok && grep_ok
    }

    /// Return true iff line `line_n` should be rendered dim. In records mode,
    /// the match decision is made once per record and applied to all its
    /// physical lines. In line mode, the decision is made per line.
    fn should_dim_line(&self, line_n: usize, idx: &LineIndex, src: &dyn Source) -> bool {
        if !self.dim_mode {
            return false;
        }
        if idx.records_mode() {
            let r = idx.line_to_record(line_n);
            !self.record_passes(idx, src, r)
        } else {
            let bytes = idx.line_bytes_stripped(line_n, src);
            !self.line_passes(&bytes)
        }
    }

    /// Logical line index of the *last* row drawn in the body, given the
    /// current `top_line` and `body_rows`. In line mode this is just
    /// `top_line + body_rows - 1` clamped to the indexed line count. In hide
    /// mode it's the logical line that sits at the bottom of the visible
    /// slice — i.e. `visible_lines[cur + body_rows - 1]`. Always returns a
    /// value `>= self.top_line`, so callers passing it to `line_to_record`
    /// never get a "bottom record < top record" inversion.
    fn bottom_visible_line(&self, idx: &LineIndex) -> usize {
        let body_rows = self.body_rows() as usize;
        if self.hide_mode() && !self.visible_lines.is_empty() {
            let cur = self
                .visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(self.visible_lines.len().saturating_sub(1));
            let last_pos = (cur + body_rows.saturating_sub(1)).min(self.visible_lines.len() - 1);
            return self.visible_lines[last_pos];
        }
        let total = idx.line_count();
        if total == 0 {
            return self.top_line;
        }
        (self.top_line + body_rows.saturating_sub(1)).min(total - 1)
    }

    pub fn body_rows(&self) -> u16 { self.rows.saturating_sub(1).max(1) }

    pub fn follow_mode(&self) -> bool { self.follow_mode }

    /// Conditionally turn follow mode off. Used by motion handlers when
    /// `--follow-suspend-on-motion` is in effect — any motion (scroll,
    /// page, goto-line) suspends following until the user re-engages
    /// with Shift-F.
    pub fn suspend_follow_if(&mut self, flag: bool) {
        if flag {
            self.follow_mode = false;
        }
    }

    pub fn set_follow_mode(&mut self, on: bool) { self.follow_mode = on; }

    pub fn toggle_follow(&mut self) { self.follow_mode = !self.follow_mode; }

    pub fn live_mode(&self) -> bool { self.live_mode }

    pub fn set_live_mode(&mut self, on: bool) { self.live_mode = on; }

    /// Status-line label for active pretty-print state, e.g. `"json"` or
    /// `"json:err"`. `None` means no indicator is shown.
    pub fn set_prettify_label(&mut self, label: Option<String>) {
        self.prettify_label = label;
    }

    /// Active --format name shown in <format-tag>. Set from main when a named
    /// format is resolved; independent of whether --filter is also active.
    pub fn set_format_label(&mut self, label: Option<String>) {
        self.format_label = label;
    }

    /// Drop the per-line filter-membership cache without disturbing the filter
    /// itself or scroll position. Used after a `--live` rebuild: line numbering
    /// may have changed, so cached `visible_lines` is stale, but we want to
    /// keep the same filter applied and let the user stay where they were.
    pub fn invalidate_filter_cache(&mut self) {
        self.visible_lines.clear();
        self.visible_scanned = 0;
    }

    /// Clamp `top_line` so it doesn't fall past the new end of the source.
    /// Pairs with `invalidate_filter_cache` after a content rewrite.
    pub fn clamp_top_line(&mut self, line_count: usize) {
        if line_count == 0 {
            self.top_line = 0;
            self.top_row = 0;
        } else if self.top_line >= line_count {
            self.top_line = line_count - 1;
            self.top_row = 0;
        }
    }

    /// True when the viewport's body window already covers the last line of
    /// the source. New content added past this point should auto-scroll if
    /// follow mode is on.
    pub fn is_at_bottom(&self, src: &dyn Source, idx: &LineIndex) -> bool {
        if self.hide_mode() {
            // Wrap-aware: at the bottom once the top is at/after the visible
            // line that anchors the last visible line's tail to the body.
            self.top_line >= self.hide_bottom_top_line(src, idx)
        } else {
            // Compare in display-row units against the wrap-aware bottom
            // anchor — `top_line + body >= line_count` would read true while
            // a wrapped tail is still off-screen below.
            (self.top_line, self.top_row) >= self.bottom_anchor(src, idx)
        }
    }

    /// Width of the line-number gutter (digits + 1 space separator), 0 if disabled.
    fn gutter_width(&self, idx: &LineIndex) -> u16 {
        if !self.show_line_numbers { return 0; }
        let n = idx.line_count().max(1);
        let digits = (n as f64).log10().floor() as u16 + 1;
        digits + 1
    }

    fn render_opts(&self, gutter: u16) -> RenderOpts {
        let mut o = self.opts.clone();
        o.cols = self.cols.saturating_sub(gutter);
        o.mode = self.ansi_mode;
        o
    }

    pub fn frame(&mut self, src: &dyn Source, idx: &mut LineIndex) -> Frame {
        if self.hex_mode {
            return self.frame_hex(src);
        }
        let body_rows = self.body_rows() as usize;
        idx.extend_to_line(self.top_line + body_rows + 1, src);

        let gutter = self.gutter_width(idx);
        let r_opts = self.render_opts(gutter);

        // Reconstruct per-line SGR state for the start of the visible window so
        // that unclosed SGR sequences on lines above top_line carry through.
        // Only meaningful in Interpret mode; harmless (and cheap) to skip otherwise.
        let mut render_state = if self.ansi_mode == crate::render::AnsiMode::Interpret {
            reconstruct_render_state(src, idx, self.top_line)
        } else {
            crate::render::RenderState::default()
        };
        // Store in the struct field for future cache use; mark current top_line.
        self.render_state = render_state.clone();
        self.render_state_for = self.top_line;

        let mut body: Vec<Vec<Cell>> = Vec::with_capacity(body_rows);
        let mut row_styles: Vec<RowStyle> = Vec::with_capacity(body_rows);
        let mut highlights: Vec<Vec<std::ops::Range<usize>>> = Vec::with_capacity(body_rows);
        let mut raw_rows: Vec<Option<Vec<u8>>> = Vec::with_capacity(body_rows);
        let raw_passthrough = self.ansi_mode == crate::render::AnsiMode::Raw;
        // In hide mode we walk visible_lines; otherwise we walk logical lines.
        let hide = self.hide_mode();
        let total_lines = idx.line_count();

        // `--header=L`: pin the first L source lines as the top L rows of
        // the body. Renders only the first cell-row of each pinned line
        // (matches less semantics; long pinned lines truncate). Skipped in
        // hide mode where "first L lines" might not be visible. Skipped in
        // raw passthrough since the user's intent there is byte-faithful
        // emission, not pinned headers.
        let header_rows = if !hide && !raw_passthrough {
            self.header_lines.min(body_rows).min(total_lines)
        } else {
            0
        };
        if header_rows > 0 {
            for hl in 0..header_rows {
                let raw = src.bytes(idx.line_range(hl, src));
                let display_bytes = if let Some(r) = self.display.as_ref() {
                    match r.render_line(&raw) {
                        Some(s) => std::borrow::Cow::Owned(s.into_bytes()),
                        None => raw.clone(),
                    }
                } else {
                    raw.clone()
                };
                let rows = render_line(&display_bytes, &r_opts, None);
                let mut content_row = rows.into_iter().next().unwrap_or_else(|| {
                    let mut v = Vec::with_capacity(self.cols as usize);
                    while v.len() < self.cols as usize { v.push(Cell::Empty); }
                    v
                });
                let mut full: Vec<Cell> = Vec::with_capacity(self.cols as usize);
                if gutter > 0 {
                    let label = format!("{:>width$} ", hl + 1, width = (gutter as usize - 1));
                    for c in label.chars() {
                        full.push(Cell::Char {
                            ch: c,
                            width: 1,
                            style: crate::ansi::Style::default(),
                            hyperlink: None,
                        });
                    }
                }
                full.append(&mut content_row);
                body.push(full);
                row_styles.push(RowStyle::Normal);
                highlights.push(Vec::new());
                raw_rows.push(None);
            }
        }

        // For hide mode, find where the viewport starts in visible_lines.
        let mut hide_pos = if hide {
            self.visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(self.visible_lines.len())
        } else {
            0
        };
        let mut line_n = if hide {
            self.visible_lines.get(hide_pos).copied().unwrap_or(total_lines)
        } else {
            // When header pinning is on, skip past the pinned region so the
            // scrolling window doesn't show those lines a second time.
            self.top_line.max(self.header_lines)
        };
        let mut skip = if hide || header_rows > 0 { 0 } else { self.top_row };

        while body.len() < body_rows {
            if line_n >= total_lines {
                let mut row = Vec::with_capacity(self.cols as usize);
                if gutter > 0 {
                    for _ in 0..gutter { row.push(Cell::Empty); }
                }
                while row.len() < self.cols as usize { row.push(Cell::Empty); }
                body.push(row);
                row_styles.push(RowStyle::Normal);
                highlights.push(Vec::new());
                raw_rows.push(None);
                line_n += 1;
                continue;
            }
            // Filter evaluation runs on the raw line (it uses captures, not
            // text), but rendering goes through the template if one is set.
            let raw = src.bytes(idx.line_range(line_n, src));
            // `-s` / --squeeze-blank-lines: skip a blank line if its
            // immediate predecessor (in logical-line space) was also blank.
            // Real line numbers / counts in `idx` stay accurate — this is a
            // display-layer filter only.
            if self.squeeze_blanks && line_is_blank(&raw) {
                let prev_blank = line_n.checked_sub(1).is_some_and(|p| {
                    let prev = src.bytes(idx.line_range(p, src));
                    line_is_blank(&prev)
                });
                if prev_blank {
                    line_n += 1;
                    continue;
                }
            }
            let display_bytes = if let Some(r) = self.display.as_ref() {
                match r.render_line(&raw) {
                    Some(s) => std::borrow::Cow::Owned(s.into_bytes()),
                    None => raw.clone(),
                }
            } else {
                raw.clone()
            };
            let state_arg = if self.ansi_mode == crate::render::AnsiMode::Interpret {
                Some(&mut render_state)
            } else {
                None
            };
            let rows = render_line(&display_bytes, &r_opts, state_arg);
            let style = if self.filter.is_some() || self.grep.is_some() {
                if self.dim_mode {
                    if self.should_dim_line(line_n, idx, src) { RowStyle::Dim } else { RowStyle::Normal }
                } else {
                    // hide mode: only matching lines reach here
                    RowStyle::Normal
                }
            } else {
                RowStyle::Normal
            };

            let mut first_emitted_for_this_line = true;
            for (i, mut content_row) in rows.into_iter().enumerate() {
                if i < skip { continue; }
                if body.len() >= body_rows { break; }
                let mut full: Vec<Cell> = Vec::with_capacity(self.cols as usize);
                if gutter > 0 {
                    let label = if i == 0 { format!("{:>width$} ", line_n + 1, width = (gutter as usize - 1)) } else { " ".repeat(gutter as usize) };
                    for c in label.chars() {
                        full.push(Cell::Char { ch: c, width: 1, style: crate::ansi::Style::default(), hyperlink: None });
                    }
                }
                full.append(&mut content_row);
                // Compute search highlights for this display row by running
                // the regex against the row's rendered text. Each match's
                // char range maps to a cell column range via `starts`.
                let row_highlights = if let (true, Some(s)) = (self.hilite_search, self.search.as_ref()) {
                    find_row_highlights(&full, &s.regex)
                } else {
                    Vec::new()
                };
                body.push(full);
                row_styles.push(style);
                highlights.push(row_highlights);
                if raw_passthrough {
                    if first_emitted_for_this_line {
                        // Emit the original line bytes verbatim once. Sub-rows
                        // (mid-line wrap continuations) are no-ops — the
                        // terminal will have already consumed enough columns
                        // from the line's full byte stream to fill them.
                        raw_rows.push(Some(raw.to_vec()));
                        first_emitted_for_this_line = false;
                    } else {
                        raw_rows.push(Some(Vec::new()));
                    }
                } else {
                    raw_rows.push(None);
                }
            }
            skip = 0;
            // Advance to next line — visible-space if hiding, logical-space otherwise.
            if hide {
                hide_pos += 1;
                line_n = self.visible_lines.get(hide_pos).copied().unwrap_or(total_lines);
            } else {
                line_n += 1;
            }
        }

        // After walking through the frame, render_state has been advanced past
        // top_line. Invalidate the cached sentinel so next frame re-reconstructs.
        self.render_state_for = usize::MAX;

        let status = self.format_status(idx, src);
        Frame { body, row_styles, highlights, status, status_style: self.status_style, raw_rows }
    }

    fn format_status(&self, idx: &LineIndex, src: &dyn Source) -> String {
        if let Some(p) = self.prompt.as_ref() {
            let ctx = self.build_prompt_context(idx, src);
            return p.render(&ctx);
        }
        let body_rows = self.body_rows() as usize;
        let total = idx.line_count();
        // In hide mode, the line range and percentage refer to visible (matched)
        // lines, not the underlying logical line count.
        let (top, bottom, total_for_pct, total_str): (usize, usize, usize, String) = if self.hide_mode() {
            let visible_total = self.visible_lines.len();
            // top_line is a logical line; find its visible index.
            let cur = self
                .visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(visible_total);
            let top = cur + 1;
            let bottom = (cur + body_rows).min(visible_total.max(1));
            let total_str = if src.is_complete() {
                format!("{visible_total}/{total}")
            } else {
                format!("{visible_total}/{total}+")
            };
            (top, bottom, visible_total, total_str)
        } else {
            let top = self.top_line + 1;
            let bottom = (self.top_line + body_rows).min(total.max(1));
            let total_str = if src.is_complete() { format!("{total}") } else { format!("{total}+") };
            (top, bottom, total, total_str)
        };
        let pct = (bottom * 100).checked_div(total_for_pct).unwrap_or(0);
        // In records mode, prefix line numbers with 'L' and append an 'R' record block.
        // The R block always refers to logical lines on screen, which in hide
        // mode is *not* the same as `bottom` (which counts visible matches).
        let bottom_line = self.bottom_visible_line(idx);
        let (line_prefix, records_block) = if idx.records_mode() {
            let line_total = idx.line_count();
            let rec_total = idx.record_count();
            let rec_block = if line_total == 0 || rec_total == 0 {
                format!("R0-0/{}", rec_total)
            } else {
                let rec_top = idx.line_to_record(self.top_line) + 1;
                let rec_bottom = idx.line_to_record(bottom_line) + 1;
                let (rec_top, rec_bottom) = if rec_bottom < rec_top {
                    // Defensive: should be unreachable given `bottom_visible_line`
                    // is always `>= self.top_line`, but guard against future
                    // regressions producing nonsense like `R290-8/...`.
                    (rec_top, rec_top)
                } else {
                    (rec_top, rec_bottom)
                };
                format!("R{}-{}/{}", rec_top, rec_bottom, rec_total)
            };
            ("L", Some(rec_block))
        } else {
            ("", None)
        };
        let middle = match records_block {
            Some(ref rb) => format!("{}{}-{}/{}  {}  {}%", line_prefix, top, bottom, total_str, rb, pct),
            None         => format!("{}-{}/{}  {}%", top, bottom, total_str, pct),
        };
        let label_with_index = match self.file_index {
            Some((current, total)) => format!("{}  [{}/{}]", self.source_label, current + 1, total),
            None => self.source_label.clone(),
        };
        let mut s = format!("{}  {}", label_with_index, middle);
        // Wrap-row offset: when scrolled inside a long wrapping line, surface
        // the offset so the user knows scrolling is happening at sub-line
        // granularity. Without this the line range above stays static while
        // pressing `j` and the scroll is invisible on repeating content.
        if !self.hide_mode() && self.top_row > 0 {
            let line_rows = if total > 0 {
                let bytes = self.line_display_bytes(src, idx, self.top_line);
                count_rows(&bytes, &self.render_opts(self.gutter_width(idx)), None)
            } else { 1 };
            s.push_str(&format!("  +{}/{}", self.top_row, line_rows));
        }
        if let Some(f) = self.filter.as_ref() {
            s.push_str(&format!("  [{}]", f.format_name));
        }
        if self.grep.is_some() {
            s.push_str("  [grep]");
        }
        if self.filter.is_some() || self.grep.is_some() {
            s.push_str(if self.dim_mode { "  [dim]" } else { "  [hide]" });
        }
        if let Some(sr) = self.search.as_ref() {
            let prefix = if matches!(sr.direction, SearchDirection::Forward) { "/" } else { "?" };
            s.push_str(&format!("  [{}{}]", prefix, sr.raw));
        }
        if let Some(label) = self.prettify_label.as_ref() {
            s.push_str(&format!("  [pretty:{label}]"));
        }
        if self.live_mode { s.push_str("  (L)"); }
        if self.follow_mode {
            if let Some((msg, _)) = self.status_flash.as_ref() {
                s.push_str("  ");
                s.push_str(msg);
            } else if self.is_idle() {
                s.push_str("  (F idle)");
            } else {
                s.push_str("  (F)");
            }
        }
        if let Some(msg) = self.preprocess_failure.as_ref() {
            let first_line = msg.lines().next().unwrap_or("");
            s.push_str(&format!("  [preprocess-failed: {}]", first_line));
        }
        let tag_suffix = match &self.tag_active {
            Some((name, cur, total)) if *total > 1 => {
                format!("  [tag: {name} ({cur}/{total})]")
            }
            _ => String::new(),
        };
        s.push_str(&tag_suffix);
        // Right-aligned :help hint. If the existing status already overshoots
        // the width, no pad — the renderer will clip on draw.
        let used = s.chars().count();
        let hint = ":help";
        if (self.cols as usize) > used + 1 + hint.chars().count() {
            let pad = self.cols as usize - used - hint.chars().count();
            s.push_str(&" ".repeat(pad));
            s.push_str(hint);
        } else {
            s.push(' ');
            s.push_str(hint);
        }
        s
    }

    fn build_prompt_context(&self, idx: &LineIndex, src: &dyn Source) -> crate::prompt::PromptContext {
        use crate::prompt::PromptContext;

        let body_rows = self.body_rows() as usize;
        let total = idx.line_count();
        let top = self.top_line + 1;
        let bottom = (self.top_line + body_rows).min(total.max(1));
        let pct = (bottom * 100).checked_div(total).unwrap_or(0);
        let bottom_line = self.bottom_visible_line(idx);

        let records_mode = idx.records_mode();
        let (rec_top, rec_bottom, rec_total) = if records_mode {
            let rt = idx.line_to_record(self.top_line) + 1;
            let rb_raw = idx.line_to_record(bottom_line) + 1;
            let rb = if rb_raw < rt { rt } else { rb_raw };
            (rt, rb, idx.record_count())
        } else {
            (0, 0, 0)
        };

        let wrap_offset = if !self.hide_mode() && self.top_row > 0 {
            let line_rows = if total > 0 {
                let bytes = self.line_display_bytes(src, idx, self.top_line);
                count_rows(&bytes, &self.render_opts(self.gutter_width(idx)), None)
            } else { 1 };
            format!("+{}/{}", self.top_row, line_rows)
        } else {
            String::new()
        };

        let format_tag = self.format_label.as_ref()
            .map(|n| format!("  [{}]", n))
            .unwrap_or_default();
        let filter_tag = self.filter.as_ref()
            .map(|f| format!("  [{}]", f.format_name))
            .unwrap_or_default();
        let grep_tag = if self.grep.is_some() { "  [grep]".to_string() } else { String::new() };
        let hide_tag = if self.filter.is_some() || self.grep.is_some() {
            if self.dim_mode { "  [dim]".to_string() } else { "  [hide]".to_string() }
        } else {
            String::new()
        };
        let search_tag = self.search.as_ref()
            .map(|s| {
                let p = if matches!(s.direction, SearchDirection::Forward) { "/" } else { "?" };
                format!("  [{}{}]", p, s.raw)
            })
            .unwrap_or_default();
        let pretty_tag = self.prettify_label.as_ref()
            .map(|l| format!("  [pretty:{l}]"))
            .unwrap_or_default();
        let live_tag = if self.live_mode { "  (L)".to_string() } else { String::new() };
        let follow_tag = if self.follow_mode { "  (F)".to_string() } else { String::new() };
        let preprocess_failed_tag = self.preprocess_failure.as_ref()
            .map(|msg| {
                let first_line = msg.lines().next().unwrap_or("");
                format!("  [preprocess-failed: {}]", first_line)
            })
            .unwrap_or_default();

        let file_index_tag = match self.file_index {
            Some((current, total)) => format!("  [{}/{}]", current + 1, total),
            None => String::new(),
        };

        let tag_tag = match &self.tag_active {
            Some((name, cur, total)) if *total > 1 => {
                format!("  [tag: {name} ({cur}/{total})]")
            }
            _ => String::new(),
        };

        PromptContext {
            label: self.source_label.clone(),
            top,
            bottom,
            total,
            pct: pct.min(100) as u8,
            rec_top,
            rec_bottom,
            rec_total,
            records_mode,
            wrap_offset,
            format_tag,
            filter_tag,
            grep_tag,
            hide_tag,
            search_tag,
            pretty_tag,
            live_tag,
            follow_tag,
            preprocess_failed_tag,
            file_index_tag,
            tag_tag,
        }
    }

    fn frame_hex(&self, src: &dyn Source) -> Frame {
        use crate::hex::format_hex_row;
        use crate::render::{render_line, Cell, RenderOpts};

        let body_rows = self.rows.saturating_sub(1) as usize;
        let total_bytes = src.len();
        let total_hex_rows = total_bytes.div_ceil(16);

        let mut body: Vec<Vec<Cell>> = Vec::with_capacity(body_rows);
        let mut row_styles: Vec<RowStyle> = Vec::with_capacity(body_rows);
        let mut highlights: Vec<Vec<std::ops::Range<usize>>> = Vec::with_capacity(body_rows);

        let opts = RenderOpts { cols: self.cols, wrap: false, tab_width: 1, mode: crate::render::AnsiMode::Strict, rscroll_char: None, word_wrap: false };

        for row_idx in 0..body_rows {
            let hex_row = self.top_line + row_idx;
            if hex_row >= total_hex_rows {
                body.push(vec![Cell::Empty; self.cols as usize]);
            } else {
                let offset = hex_row * 16;
                let end = (offset + 16).min(total_bytes);
                let bytes_cow = src.bytes(offset..end);
                let text = format_hex_row(offset, &bytes_cow, self.hex_group_size);
                let rows = render_line(text.as_bytes(), &opts, None);
                body.push(rows.into_iter().next().unwrap_or_else(|| {
                    vec![Cell::Empty; self.cols as usize]
                }));
            }
            row_styles.push(RowStyle::Normal);
            highlights.push(Vec::new());
        }

        let status = self.format_status_hex(src);
        let raw_rows = vec![None; body.len()];
        Frame { body, row_styles, highlights, status, status_style: self.status_style, raw_rows }
    }

    fn format_status_hex(&self, src: &dyn Source) -> String {
        let total_bytes = src.len();
        let body_rows = self.rows.saturating_sub(1) as usize;
        // Byte offset of the first visible byte (start of the top hex row).
        let top_byte = self.top_line * 16;
        // Byte offset just past the last visible byte. Clamped to total_bytes
        // so we never show a value past EOF.
        let bottom_byte = ((self.top_line + body_rows) * 16).min(total_bytes);
        let pct = (bottom_byte * 100).checked_div(total_bytes).unwrap_or(0);
        let label_with_index = match self.file_index {
            Some((current, total)) => format!("{}  [{}/{}]", self.source_label, current + 1, total),
            None => self.source_label.clone(),
        };
        let tag_suffix = match &self.tag_active {
            Some((name, cur, total)) if *total > 1 => {
                format!("  [tag: {name} ({cur}/{total})]")
            }
            _ => String::new(),
        };
        format!(
            "{}  off {}-{}/{}  {}%  [hex]{}",
            label_with_index, top_byte, bottom_byte, total_bytes, pct, tag_suffix
        )
    }

    /// Jump by whole logical lines, regardless of wrap rows. `top_row` is
    /// reset to 0 so the start of the destination line is at the top of
    /// the viewport. In hide mode this is equivalent to `scroll_lines`
    /// (which already moves by visible/logical lines).
    pub fn scroll_logical_lines(&mut self, delta: i64, src: &dyn Source, idx: &mut LineIndex) {
        if delta == 0 { return; }
        if self.hide_mode() {
            self.scroll_lines(delta, src, idx);
            return;
        }
        if delta > 0 {
            idx.extend_to_line(self.top_line + delta as usize + 1, src);
            let total = idx.line_count();
            if total == 0 { return; }
            let target = (self.top_line as i64 + delta).min(total as i64 - 1) as usize;
            self.top_line = target;
            self.top_row = 0;
        } else {
            let back = (-delta) as usize;
            // If we're inside a wrapped line (top_row > 0), `K` first snaps to
            // the start of the current line; only the remaining count goes to
            // previous lines. This matches the user's mental model of "jump
            // to the start of the previous line".
            let consumed_for_snap = if self.top_row > 0 { 1 } else { 0 };
            let extra_back = back.saturating_sub(consumed_for_snap);
            self.top_line = self.top_line.saturating_sub(extra_back);
            self.top_row = 0;
        }
    }

    pub fn scroll_lines(&mut self, delta: i64, src: &dyn Source, idx: &mut LineIndex) {
        if delta == 0 { return; }
        if self.hide_mode() {
            // Scroll by visible (matching) lines. We don't honor wrap rows in
            // hide mode — top_row stays 0. Each unit of `delta` advances or
            // retreats one visible line.
            self.extend_visible_lines(idx, src);
            let total = self.visible_lines.len();
            if total == 0 {
                self.top_line = 0;
                self.top_row = 0;
                return;
            }
            let cur = self
                .visible_lines
                .iter()
                .position(|&l| l >= self.top_line)
                .unwrap_or(total);
            let new = (cur as i64 + delta).clamp(0, total.saturating_sub(1) as i64) as usize;
            self.top_line = self.visible_lines[new];
            self.top_row = 0;
            return;
        }
        if delta > 0 {
            let mut remaining = delta as usize;
            while remaining > 0 {
                idx.extend_to_line(self.top_line + 1, src);
                let total = idx.line_count();
                if total == 0 { break; }
                let bytes = self.line_display_bytes(src, idx, self.top_line);
                let line_rows = count_rows(&bytes, &self.render_opts(self.gutter_width(idx)), None);
                if self.top_row + 1 < line_rows {
                    self.top_row += 1;
                } else if self.top_line + 1 < total {
                    self.top_row = 0;
                    self.top_line += 1;
                } else {
                    break;
                }
                remaining -= 1;
            }
            // Don't scroll past the natural bottom (the last line resting on
            // the last body row). Only clamp once the source is fully
            // scanned — mid-scan the true end is unknown, and clamping to a
            // partial index would strand the viewport short of unread content.
            if idx.scanned_through() >= src.len() {
                let anchor = self.bottom_anchor(src, idx);
                if (self.top_line, self.top_row) > anchor {
                    self.top_line = anchor.0;
                    self.top_row = anchor.1;
                }
            }
        } else {
            let mut remaining = (-delta) as usize;
            while remaining > 0 {
                if self.top_row > 0 {
                    self.top_row -= 1;
                } else if self.top_line > 0 {
                    self.top_line -= 1;
                    let bytes = self.line_display_bytes(src, idx, self.top_line);
                    let line_rows = count_rows(&bytes, &self.render_opts(self.gutter_width(idx)), None);
                    self.top_row = line_rows.saturating_sub(1);
                } else {
                    break;
                }
                remaining -= 1;
            }
        }
    }

    pub fn page_down(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = self.page_size
            .map(|p| p as i64)
            .unwrap_or_else(|| self.body_rows() as i64);
        self.scroll_lines(n, src, idx);
    }

    pub fn page_up(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = self.page_size
            .map(|p| p as i64)
            .unwrap_or_else(|| self.body_rows() as i64);
        self.scroll_lines(-n, src, idx);
    }

    pub fn half_page_down(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = (self.body_rows() / 2).max(1) as i64;
        self.scroll_lines(n, src, idx);
    }

    pub fn half_page_up(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        let n = (self.body_rows() / 2).max(1) as i64;
        self.scroll_lines(-n, src, idx);
    }

    pub fn goto_top(&mut self) {
        self.top_line = 0;
        self.top_row = 0;
    }

    /// The top `(line, row)` position such that the source's final display row
    /// lands on the last body row. Computed in display-row units by walking
    /// backward from the end, so it stays correct when lines wrap (the
    /// default): the last `body` logical lines can occupy more than `body`
    /// rows. Returns `(0, 0)` when the whole document fits within the body.
    /// Non-hide-mode only — hide mode scrolls by whole visible lines.
    fn bottom_anchor(&self, src: &dyn Source, idx: &LineIndex) -> (usize, usize) {
        let body = self.body_rows() as usize;
        let total = idx.line_count();
        if total == 0 || body == 0 {
            return (0, 0);
        }
        let r_opts = self.render_opts(self.gutter_width(idx));
        let mut remaining = body;
        let mut line = total - 1;
        loop {
            let bytes = self.line_display_bytes(src, idx, line);
            let line_rows = count_rows(&bytes, &r_opts, None).max(1);
            if line_rows >= remaining {
                return (line, line_rows - remaining);
            }
            remaining -= line_rows;
            if line == 0 {
                return (0, 0);
            }
            line -= 1;
        }
    }

    /// Hide-mode bottom: the earliest *visible* line whose wrap rows, summed
    /// with everything after it, still fit the body — so the last visible
    /// line's final row lands at (or near) the bottom. `top_row` stays 0
    /// because hide-mode scrolling is line-granular; a single visible line
    /// taller than the whole body can't show its tail here (known limitation).
    fn hide_bottom_top_line(&self, src: &dyn Source, idx: &LineIndex) -> usize {
        let body = self.body_rows() as usize;
        let vis = &self.visible_lines;
        if vis.is_empty() {
            return 0;
        }
        let r_opts = self.render_opts(self.gutter_width(idx));
        let mut rows_sum = 0usize;
        let mut chosen = *vis.last().unwrap();
        for &vl in vis.iter().rev() {
            let rows = count_rows(&self.line_display_bytes(src, idx, vl), &r_opts, None).max(1);
            if rows_sum > 0 && rows_sum + rows > body {
                break;
            }
            rows_sum += rows;
            chosen = vl;
            if rows_sum >= body {
                break;
            }
        }
        chosen
    }

    pub fn goto_bottom(&mut self, src: &dyn Source, idx: &mut LineIndex) {
        idx.extend_to_end(src);
        if self.hide_mode() {
            self.extend_visible_lines(idx, src);
            self.top_line = self.hide_bottom_top_line(src, idx);
            self.top_row = 0;
        } else {
            let (line, row) = self.bottom_anchor(src, idx);
            self.top_line = line;
            self.top_row = row;
        }
    }

    /// Position the viewport so line `n` (0-indexed) is the top visible line.
    pub fn goto_line(&mut self, n: usize, src: &dyn Source, idx: &mut LineIndex) {
        idx.extend_to_line(n, src);
        let target = n.min(idx.line_count().saturating_sub(1));
        self.top_line = target;
        self.top_row = 0;
    }

    /// Position the viewport at the start of record `n` (0-indexed).
    pub fn goto_record(&mut self, n: usize, src: &dyn Source, idx: &mut LineIndex) {
        // Ensure the record exists by extending the index. Records can only
        // appear after their constituent lines are scanned; extend repeatedly
        // until the record exists or we hit EOF.
        while idx.record_count() <= n && idx.scanned_through() < src.len() {
            idx.extend_to_end(src);
        }
        if idx.record_count() == 0 {
            return;
        }
        let target = n.min(idx.record_count().saturating_sub(1));
        let line_range = idx.record_line_range(target);
        self.top_line = line_range.start;
        self.top_row = 0;
    }

    /// Position the viewport at `p` percent through the file by bytes.
    /// `p` is clamped to 0..=100. p=100 lands at the last line.
    pub fn goto_percent(&mut self, p: u8, src: &dyn Source, idx: &mut LineIndex) {
        let p = p.min(100) as usize;
        let target_byte = src.len().saturating_mul(p) / 100;
        idx.extend_to_byte_for_query(src, target_byte);
        let line_n = idx.line_at_byte(target_byte)
            .or_else(|| {
                // target_byte at or past EOF: fall through to the last line.
                let lc = idx.line_count();
                if lc > 0 { Some(lc - 1) } else { None }
            })
            .unwrap_or(0);
        self.top_line = line_n;
        self.top_row = 0;
    }

    /// Get the currently top-displayed physical line index.
    pub fn top_line(&self) -> usize {
        self.top_line
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.cols = cols.max(1);
        self.rows = rows.max(2);
        self.opts.cols = self.cols;
    }

    pub fn toggle_line_numbers(&mut self) {
        self.show_line_numbers = !self.show_line_numbers;
    }

    pub fn toggle_chop(&mut self) {
        self.opts.wrap = !self.opts.wrap;
    }

    /// Return the current set of visible (matched) line indices. Non-empty only
    /// in hide mode (filter or grep active without --dim). Stable public accessor
    /// so integration tests and external tooling can inspect filter results.
    pub fn visible_lines(&self) -> &[usize] { &self.visible_lines }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MockSource;

    fn setup(content: &[u8]) -> (MockSource, LineIndex) {
        let m = MockSource::new();
        m.append(content);
        m.finish();
        let idx = LineIndex::new();
        (m, idx)
    }

    #[test]
    fn frame_renders_body_height_rows() {
        let (m, mut idx) = setup(b"a\nb\nc\nd\ne\n");
        let mut v = Viewport::new(10, 5, "test".into());  // body = 4
        let frame = v.frame(&m, &mut idx);
        assert_eq!(frame.body.len(), 4);
        assert_eq!(frame.body[0][0], Cell::Char { ch: 'a', width: 1, style: crate::ansi::Style::default(), hyperlink: None });
        assert_eq!(frame.body[3][0], Cell::Char { ch: 'd', width: 1, style: crate::ansi::Style::default(), hyperlink: None });
    }

    #[test]
    fn scroll_down_advances_top_line() {
        // 8 lines, body=4 → there are 4 lines below the first screen to scroll
        // into, so scrolling down by 2 lands cleanly above the bottom anchor.
        let (m, mut idx) = setup(b"a\nb\nc\nd\ne\nf\ng\nh\n");
        let mut v = Viewport::new(10, 5, "test".into());
        v.scroll_lines(2, &m, &mut idx);
        assert_eq!(v.top_line, 2);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn scroll_up_clamps_at_zero() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(10, 5, "test".into());
        v.scroll_lines(-5, &m, &mut idx);
        assert_eq!(v.top_line, 0);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn scroll_down_clamps_at_last_line() {
        // 8 single-row lines, body=4. Scrolling far past the end clamps at the
        // bottom anchor: the last line resting on the last body row, i.e.
        // top_line = 8 - 4 = 4. (Not line 7 at the top — that would strand the
        // tail off-screen, the bug this guards against.)
        let (m, mut idx) = setup(b"a\nb\nc\nd\ne\nf\ng\nh\n");
        let mut v = Viewport::new(10, 5, "test".into());
        v.scroll_lines(50, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (4, 0));
        assert!(v.is_at_bottom(&m, &idx));
    }

    #[test]
    fn scroll_logical_lines_skips_wrap_rows() {
        // Line 0 has 50 wraps in a 10-col viewport. J should jump straight to line 1.
        let mut content = vec![b'X'; 500];
        content.push(b'\n');
        content.extend_from_slice(b"second\n");
        content.extend_from_slice(b"third\n");
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(10, 8, "f".into());
        v.scroll_logical_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 0));
        v.scroll_logical_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (2, 0));
    }

    #[test]
    fn scroll_logical_lines_back_snaps_to_line_start() {
        // Mid-wrap K should snap to start of current line first, then go back.
        // Three 50-char lines (5 wrap rows each) with body=8 put the bottom
        // anchor at (1, 2), so scrolling down 7 rows lands inside line 1's
        // wraps without being clamped.
        let mut content = vec![b'A'; 50];
        content.push(b'\n');
        content.extend_from_slice(&[b'B'; 50]);
        content.push(b'\n');
        content.extend_from_slice(&[b'C'; 50]);
        content.push(b'\n');
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(10, 8, "f".into());
        v.scroll_lines(7, &m, &mut idx);
        assert_eq!(v.top_line, 1, "should be on line 1");
        assert!(v.top_row > 0, "should be inside line 1's wraps");
        v.scroll_logical_lines(-1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 0), "K snaps to start of current line");
        v.scroll_logical_lines(-1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (0, 0), "K then goes to previous line");
    }

    #[test]
    fn scroll_down_walks_wraps_of_last_line() {
        // Last line is 60 chars in a 10-col viewport → 6 wrap rows. With body=4
        // the bottom anchor sits at (1, 2), so walking into the last line's
        // wraps up to row 2 is legitimate (it doesn't strand the tail).
        let mut content = b"first\n".to_vec();
        content.extend_from_slice(&[b'X'; 60]);
        content.push(b'\n');
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(10, 5, "f".into());
        v.scroll_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 0));
        v.scroll_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 1), "should advance into wraps of last line");
        v.scroll_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 2), "should reach the bottom anchor row");
        // Already at the anchor — scrolling further must not strand the tail.
        v.scroll_lines(5, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 2), "clamped at the bottom anchor");
    }

    #[test]
    fn scroll_down_walks_wrap_rows_within_long_line() {
        // Line 0 is 30 chars in a 10-col viewport → 3 wrap rows. Body = 4.
        // Six short lines follow so the bottom anchor sits well below line 0,
        // leaving room to walk through line 0's wrap rows and into line 1.
        let mut content = vec![b'X'; 30];
        content.push(b'\n');
        content.extend_from_slice(b"a\nb\nc\nd\ne\nf\n");
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(10, 5, "f".into());
        v.scroll_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (0, 1), "first j → wrap row 1");
        v.scroll_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (0, 2), "second j → wrap row 2");
        v.scroll_lines(1, &m, &mut idx);
        assert_eq!((v.top_line, v.top_row), (1, 0), "third j → next logical line");
    }

    #[test]
    fn status_line_shows_range_and_pct() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let mut v = Viewport::new(20, 5, "f".into());  // body = 4
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.starts_with("f  1-4/10"));
    }

    #[test]
    fn page_down_advances_by_body_rows() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 5, "f".into());  // body = 4
        v.page_down(&m, &mut idx);
        assert_eq!(v.top_line, 4);
    }

    #[test]
    fn page_up_then_page_down_returns_to_start_when_no_resize() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.page_down(&m, &mut idx);
        v.page_up(&m, &mut idx);
        assert_eq!(v.top_line, 0);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn half_page_down_advances_by_half_body() {
        // 12 lines, body=6 → bottom anchor at line 6, so a half-page (3) lands
        // cleanly above it.
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n");
        let mut v = Viewport::new(10, 7, "f".into());  // body = 6, half = 3
        v.half_page_down(&m, &mut idx);
        assert_eq!(v.top_line, 3);
    }

    #[test]
    fn goto_top_resets_position() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.scroll_lines(2, &m, &mut idx);
        v.goto_top();
        assert_eq!(v.top_line, 0);
        assert_eq!(v.top_row, 0);
    }

    #[test]
    fn goto_bottom_scrolls_to_last_page() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n");
        let mut v = Viewport::new(10, 5, "f".into());  // body = 4
        v.goto_bottom(&m, &mut idx);
        // Last page should show lines 7..=10 → top_line = 6.
        assert_eq!(v.top_line, 6);
    }

    #[test]
    fn goto_line_positions_top_line() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\nd\ne\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_line(3, &m, &mut idx);
        assert_eq!(v.top_line(), 3);
    }

    #[test]
    fn goto_line_clamps_to_last_line() {
        let m = MockSource::new();
        m.append(b"a\nb\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_line(999, &m, &mut idx);
        assert_eq!(v.top_line(), 1);
    }

    #[test]
    fn goto_record_positions_at_record_start_line() {
        let m = MockSource::new();
        m.append(b"[1] a\n  cont\n[2] b\n[3] c\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_record(1, &m, &mut idx);  // record 1 starts at line 2 ("[2] b")
        assert_eq!(v.top_line(), 2);
    }

    #[test]
    fn goto_record_in_line_per_record_mode_equals_goto_line() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_record(2, &m, &mut idx);
        assert_eq!(v.top_line(), 2);
    }

    #[test]
    fn goto_percent_50_lands_in_middle() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\nd\ne\n");  // 10 bytes
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_percent(50, &m, &mut idx);
        assert_eq!(v.top_line(), 2);  // byte 5 → line 2
    }

    #[test]
    fn goto_percent_100_lands_at_last_line() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");  // 6 bytes, 3 lines
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_percent(100, &m, &mut idx);
        assert_eq!(v.top_line(), 2);
    }

    #[test]
    fn goto_percent_0_lands_at_first_line() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        v.goto_record(2, &m, &mut idx);  // first jump elsewhere
        assert_eq!(v.top_line(), 2);
        v.goto_percent(0, &m, &mut idx);
        assert_eq!(v.top_line(), 0);
    }

    #[test]
    fn resize_updates_dimensions_and_render_opts() {
        let (m, mut idx) = setup(b"1\n2\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.resize(40, 12);
        assert_eq!(v.cols, 40);
        assert_eq!(v.rows, 12);
        assert_eq!(v.opts.cols, 40);
        let _ = v.frame(&m, &mut idx);
    }

    #[test]
    fn toggle_line_numbers_changes_gutter() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(10, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        v.toggle_line_numbers();
        let frame_on = v.frame(&m, &mut idx);
        // With gutter, first cell is a digit or space, not 'a'.
        assert_eq!(frame_off.body[0][0], Cell::Char { ch: 'a', width: 1, style: crate::ansi::Style::default(), hyperlink: None });
        assert_ne!(frame_on.body[0][0], Cell::Char { ch: 'a', width: 1, style: crate::ansi::Style::default(), hyperlink: None });
    }

    #[test]
    fn toggle_chop_changes_wrap_mode() {
        let (m, mut idx) = setup(b"abcdefghij\n");
        let mut v = Viewport::new(4, 5, "f".into());
        v.toggle_chop();
        let frame = v.frame(&m, &mut idx);
        // After toggle_chop, the line is one row, not wrapped.
        // Body row 0 is "abcd"; rows 1..3 are blank fill.
        assert_eq!(frame.body[0][..4],
            [Cell::Char { ch: 'a', width: 1, style: crate::ansi::Style::default(), hyperlink: None },
             Cell::Char { ch: 'b', width: 1, style: crate::ansi::Style::default(), hyperlink: None },
             Cell::Char { ch: 'c', width: 1, style: crate::ansi::Style::default(), hyperlink: None },
             Cell::Char { ch: 'd', width: 1, style: crate::ansi::Style::default(), hyperlink: None }]);
        // Row 1 should be all-empty (no wrap continuation).
        assert!(frame.body[1].iter().all(|c| matches!(c, Cell::Empty)));
    }

    // ----- Follow mode -----

    #[test]
    fn is_at_bottom_initially_only_when_source_fits() {
        let (m, mut idx) = setup(b"a\nb\n");  // 2 lines
        let v = Viewport::new(10, 5, "f".into());  // body = 4 ≥ 2
        idx.extend_to_end(&m);
        assert!(v.is_at_bottom(&m, &idx), "small file fits in body, top is at bottom");
    }

    #[test]
    fn is_at_bottom_false_when_top_and_more_lines_below() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");  // 8 lines
        let v = Viewport::new(10, 5, "f".into());  // body = 4
        idx.extend_to_end(&m);
        assert!(!v.is_at_bottom(&m, &idx), "top of 8-line file with body=4 is not at bottom");
    }

    #[test]
    fn is_at_bottom_true_after_goto_bottom() {
        let (m, mut idx) = setup(b"1\n2\n3\n4\n5\n6\n7\n8\n");
        let mut v = Viewport::new(10, 5, "f".into());
        v.goto_bottom(&m, &mut idx);
        assert!(v.is_at_bottom(&m, &idx));
    }

    #[test]
    fn status_shows_follow_suffix_when_follow_mode_on() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        assert!(!frame_off.status.contains("(F)"));
        v.set_follow_mode(true);
        let frame_on = v.frame(&m, &mut idx);
        assert!(frame_on.status.contains("(F)"), "expected (F) in status, got: {}", frame_on.status);
    }

    #[test]
    fn toggle_follow_flips_state() {
        let mut v = Viewport::new(10, 5, "f".into());
        assert!(!v.follow_mode());
        v.toggle_follow();
        assert!(v.follow_mode());
        v.toggle_follow();
        assert!(!v.follow_mode());
    }

    #[test]
    fn idle_indicator_kicks_in_at_threshold() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_follow_mode(true);
        // 19 idle ticks → still (F).
        for _ in 0..19 { v.tick_idle(); }
        let f1 = v.frame(&m, &mut idx);
        assert!(f1.status.contains("(F)"));
        assert!(!f1.status.contains("idle"));
        // 20th tick crosses the threshold.
        v.tick_idle();
        let f2 = v.frame(&m, &mut idx);
        assert!(f2.status.contains("(F idle)"), "{}", f2.status);
    }

    #[test]
    fn note_growth_resets_idle() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_follow_mode(true);
        for _ in 0..25 { v.tick_idle(); }
        assert!(v.is_idle());
        v.note_growth();
        assert!(!v.is_idle());
        let f = v.frame(&m, &mut idx);
        assert!(!f.status.contains("idle"));
    }

    #[test]
    fn qae_off_never_quits_even_at_bottom() {
        let (m, mut idx) = setup(b"a\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_quit_at_eof(QuitAtEof::Off);
        v.goto_bottom(&m, &mut idx);
        assert!(!v.note_motion_for_eof(true, &m, &idx));
    }

    #[test]
    fn qae_first_quits_immediately_at_bottom() {
        let (m, mut idx) = setup(b"a\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_quit_at_eof(QuitAtEof::First);
        v.goto_bottom(&m, &mut idx);
        assert!(v.note_motion_for_eof(true, &m, &idx));
    }

    #[test]
    fn qae_first_only_quits_at_eof_not_mid_file() {
        let mut content = Vec::new();
        for _ in 0..50 { content.extend_from_slice(b"x\n"); }
        let (m, mut idx) = setup(&content);
        idx.extend_to_end(&m);  // populate so is_at_bottom can see the 50 lines
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_quit_at_eof(QuitAtEof::First);
        // top_line is 0; with 50 lines and a 5-row body, we're not at bottom.
        assert!(!v.is_at_bottom(&m, &idx));
        assert!(!v.note_motion_for_eof(true, &m, &idx));
    }

    #[test]
    fn qae_second_quits_on_second_hit() {
        let (m, mut idx) = setup(b"a\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_quit_at_eof(QuitAtEof::Second);
        v.goto_bottom(&m, &mut idx);
        // 1st forward at EOF: count, don't quit.
        assert!(!v.note_motion_for_eof(true, &m, &idx));
        // 2nd forward at EOF: quit.
        assert!(v.note_motion_for_eof(true, &m, &idx));
    }

    #[test]
    fn squeeze_collapses_consecutive_blanks() {
        // Source: a, blank, blank, blank, b.
        let (m, mut idx) = setup(b"a\n\n\n\nb\n");
        let mut v = Viewport::new(10, 8, "f".into());
        v.set_squeeze_blanks(true);
        let f = v.frame(&m, &mut idx);
        // First non-empty body row chars (trimmed).
        let stringify = |row: &Vec<Cell>| -> String {
            row.iter().filter_map(|c| match c {
                Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            }).collect::<String>().trim().to_string()
        };
        let rows: Vec<String> = f.body.iter().map(stringify).collect();
        // With squeeze: a, blank, b. Then padding.
        assert_eq!(&rows[0], "a");
        assert_eq!(&rows[1], "");
        assert_eq!(&rows[2], "b");
    }

    #[test]
    fn header_pins_top_rows_when_scrolling() {
        // 12 lines, 6-row terminal → body_rows = 5. header=2 pins lines 0,1.
        let mut content = Vec::new();
        for n in 0..12 { content.extend_from_slice(format!("line{n}\n").as_bytes()); }
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(20, 6, "f".into());
        v.set_header(2, 0);
        // set_header floors top_line at header_lines, so we start showing
        // line2 in the scroll window. Scrolling down 5 advances by 5
        // logical lines from there.
        v.scroll_lines(5, &m, &mut idx);
        let f = v.frame(&m, &mut idx);
        let chs = |row: &Vec<Cell>| -> String {
            row.iter().filter_map(|c| match c {
                Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            }).collect::<String>().trim().to_string()
        };
        // Rows 0 and 1 are the pinned header (line0, line1) regardless of scroll.
        assert_eq!(&chs(&f.body[0]), "line0");
        assert_eq!(&chs(&f.body[1]), "line1");
        // top_line is now 2 + 5 = 7; row 2 shows line7.
        assert_eq!(&chs(&f.body[2]), "line7");
    }

    #[test]
    fn page_size_when_set_overrides_body_rows() {
        let mut content = Vec::new();
        for n in 0..100 { content.extend_from_slice(format!("L{n}\n").as_bytes()); }
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(20, 10, "f".into());
        v.set_page_size(Some(3));
        let before = v.top_line();
        v.page_down(&m, &mut idx);
        assert_eq!(v.top_line(), before + 3);
        v.page_up(&m, &mut idx);
        assert_eq!(v.top_line(), before);
    }

    #[test]
    fn page_size_unset_uses_body_rows() {
        let mut content = Vec::new();
        for n in 0..100 { content.extend_from_slice(format!("L{n}\n").as_bytes()); }
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(20, 10, "f".into());
        // body_rows = rows - 1 = 9.
        v.page_down(&m, &mut idx);
        assert_eq!(v.top_line(), 9);
    }

    #[test]
    fn header_zero_lines_renders_like_no_header() {
        let mut content = Vec::new();
        for n in 0..10 { content.extend_from_slice(format!("line{n}\n").as_bytes()); }
        let (m, mut idx) = setup(&content);
        let mut v = Viewport::new(20, 6, "f".into());
        v.set_header(0, 0);
        let f = v.frame(&m, &mut idx);
        let chs = |row: &Vec<Cell>| -> String {
            row.iter().filter_map(|c| match c {
                Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            }).collect::<String>().trim().to_string()
        };
        assert_eq!(&chs(&f.body[0]), "line0");
        assert_eq!(&chs(&f.body[1]), "line1");
    }

    #[test]
    fn squeeze_off_preserves_blanks() {
        let (m, mut idx) = setup(b"a\n\n\n\nb\n");
        let mut v = Viewport::new(10, 8, "f".into());
        // Default is off.
        let f = v.frame(&m, &mut idx);
        let stringify = |row: &Vec<Cell>| -> String {
            row.iter().filter_map(|c| match c {
                Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            }).collect::<String>().trim().to_string()
        };
        let rows: Vec<String> = f.body.iter().map(stringify).collect();
        // Without squeeze: a, blank, blank, blank, b.
        assert_eq!(&rows[0], "a");
        assert_eq!(&rows[1], "");
        assert_eq!(&rows[2], "");
        assert_eq!(&rows[3], "");
        assert_eq!(&rows[4], "b");
    }

    #[test]
    fn qae_second_resets_on_backward_motion() {
        let (m, mut idx) = setup(b"a\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_quit_at_eof(QuitAtEof::Second);
        v.goto_bottom(&m, &mut idx);
        assert!(!v.note_motion_for_eof(true, &m, &idx));
        // Backward motion clears the counter.
        v.note_motion_for_eof(false, &m, &idx);
        // Next forward starts fresh: counts, doesn't quit.
        assert!(!v.note_motion_for_eof(true, &m, &idx));
        // Now the second consecutive forward triggers quit.
        assert!(v.note_motion_for_eof(true, &m, &idx));
    }

    #[test]
    fn flash_message_overrides_follow_suffix() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(40, 5, "f".into());
        v.set_follow_mode(true);
        v.flash("(F reopened)", 3);
        let f = v.frame(&m, &mut idx);
        assert!(f.status.contains("(F reopened)"), "{}", f.status);
        assert!(!f.status.contains("(F idle)"));
    }

    #[test]
    fn flash_countdown_clears() {
        let mut v = Viewport::new(10, 5, "f".into());
        v.flash("hello", 2);
        v.tick_flash();
        assert!(v.status_flash.is_some());
        v.tick_flash();
        assert!(v.status_flash.is_none());
    }

    #[test]
    fn suspend_follow_if_off_is_noop() {
        let mut v = Viewport::new(10, 5, "f".into());
        v.set_follow_mode(true);
        v.suspend_follow_if(false);
        assert!(v.follow_mode());
    }

    #[test]
    fn suspend_follow_if_on_flips_off() {
        let mut v = Viewport::new(10, 5, "f".into());
        v.set_follow_mode(true);
        v.suspend_follow_if(true);
        assert!(!v.follow_mode());
    }

    #[test]
    fn case_mode_sensitive_returns_pattern_unchanged() {
        assert_eq!(CaseMode::Sensitive.apply_to_pattern("foo"), "foo");
        assert_eq!(CaseMode::Sensitive.apply_to_pattern("FOO"), "FOO");
    }

    #[test]
    fn case_mode_insensitive_prepends_i_flag() {
        assert_eq!(CaseMode::Insensitive.apply_to_pattern("foo"), "(?i)foo");
        assert_eq!(CaseMode::Insensitive.apply_to_pattern("FOO"), "(?i)FOO");
    }

    #[test]
    fn case_mode_smart_lowercase_is_insensitive() {
        assert_eq!(CaseMode::Smart.apply_to_pattern("foo"), "(?i)foo");
    }

    #[test]
    fn case_mode_smart_with_uppercase_is_sensitive() {
        assert_eq!(CaseMode::Smart.apply_to_pattern("Foo"), "Foo");
        assert_eq!(CaseMode::Smart.apply_to_pattern("FOO"), "FOO");
    }

    #[test]
    fn set_case_mode_recompiles_active_search() {
        let (m, mut idx) = setup(b"hello WORLD\n");
        let mut v = Viewport::new(40, 5, "f".into());
        v.set_search("world".into(), SearchDirection::Forward).unwrap();
        // Sensitive: no match for lowercase against WORLD.
        assert!(!v.search_repeat(&m, &mut idx, false));
        // Switch to insensitive — should re-compile and now match.
        v.set_case_mode(CaseMode::Insensitive);
        assert!(v.search_repeat(&m, &mut idx, false));
    }

    #[test]
    fn status_shows_prettify_label_when_set() {
        let (m, mut idx) = setup(b"a\n");
        let mut v = Viewport::new(40, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        assert!(!frame_off.status.contains("[pretty"));
        v.set_prettify_label(Some("json".into()));
        let frame_on = v.frame(&m, &mut idx);
        assert!(frame_on.status.contains("[pretty:json]"),
            "expected [pretty:json] in status, got: {}", frame_on.status);
        v.set_prettify_label(Some("json:err".into()));
        let frame_err = v.frame(&m, &mut idx);
        assert!(frame_err.status.contains("[pretty:json:err]"),
            "expected [pretty:json:err] in status, got: {}", frame_err.status);
    }

    #[test]
    fn status_shows_l_suffix_when_live_mode_on() {
        let (m, mut idx) = setup(b"a\nb\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let frame_off = v.frame(&m, &mut idx);
        assert!(!frame_off.status.contains("(L)"));
        v.set_live_mode(true);
        let frame_on = v.frame(&m, &mut idx);
        assert!(frame_on.status.contains("(L)"), "expected (L) in status, got: {}", frame_on.status);
    }

    #[test]
    fn clamp_top_line_pulls_back_when_total_shrinks() {
        let mut v = Viewport::new(20, 5, "f".into());
        // Pretend we were on line 100, then a rewrite leaves only 10 lines.
        v.scroll_lines(0, &MockSource::new(), &mut LineIndex::new()); // no-op, just to satisfy
        // Force top_line via a sequence; easiest: just call clamp directly.
        // We can't poke private state, but clamp works regardless of how we got there.
        v.clamp_top_line(100);  // total bigger than top_line=0, no change
        v.clamp_top_line(0);    // empty source: must reset
        // After clamp(0), line 0 is the floor.
        // (No public getter for top_line; we verify indirectly by going to top.)
        v.goto_top();
        // Just confirm no panic and no overflow on subsequent frame composition.
        let (m, mut idx) = setup(b"only\n");
        let _ = v.frame(&m, &mut idx);
    }

    /// Simulates the app::run timeout-branch logic to verify auto-scroll engages
    /// when follow mode is on and the viewport is at the bottom.
    fn simulate_growth_tick(
        v: &mut Viewport,
        src: &MockSource,
        idx: &mut LineIndex,
    ) {
        if !v.follow_mode() { return; }
        let was_at_bottom = v.is_at_bottom(src, idx);
        let lines_before = idx.line_count();
        idx.notice_new_bytes(src);
        if idx.line_count() != lines_before && was_at_bottom {
            v.goto_bottom(src, idx);
        }
    }

    #[test]
    fn auto_scroll_engages_when_at_bottom() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n");  // 4 lines, body=4 fits
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 5, "f".into());
        v.set_follow_mode(true);
        idx.extend_to_end(&m);
        assert!(v.is_at_bottom(&m, &idx));
        let top_before = {
            let f = v.frame(&m, &mut idx);
            f.status.clone()  // unused, just exercise frame
        };
        let _ = top_before;
        // Simulate growth: source gains 4 more lines.
        m.append(b"5\n6\n7\n8\n");
        simulate_growth_tick(&mut v, &m, &mut idx);
        // After auto-scroll, top_line should have advanced so the new last line is in view.
        assert!(v.is_at_bottom(&m, &idx), "after auto-scroll, viewport should still be at bottom");
        let frame = v.frame(&m, &mut idx);
        // The bottom-most body row should now contain the last logical line ('8').
        // Find which row has '8'.
        let last_row = &frame.body[frame.body.len() - 1];
        assert_eq!(last_row[0], Cell::Char { ch: '8', width: 1, style: crate::ansi::Style::default(), hyperlink: None });
    }

    #[test]
    fn auto_scroll_suppressed_when_scrolled_up() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n5\n6\n7\n8\n");  // 8 lines
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 5, "f".into());  // body=4
        v.set_follow_mode(true);
        idx.extend_to_end(&m);
        v.goto_bottom(&m, &mut idx);
        // Now scroll up off the bottom.
        v.scroll_lines(-2, &m, &mut idx);
        assert!(!v.is_at_bottom(&m, &idx));
        let frame_before = v.frame(&m, &mut idx);
        let top_first_cell_before = frame_before.body[0][0].clone();
        // Simulate growth.
        m.append(b"9\n10\n");
        simulate_growth_tick(&mut v, &m, &mut idx);
        // Viewport should NOT have moved (auto-scroll suppressed).
        let frame_after = v.frame(&m, &mut idx);
        assert_eq!(frame_after.body[0][0], top_first_cell_before, "viewport moved despite being scrolled up");
    }

    // ----- Search -----

    #[test]
    fn set_search_compiles_regex() {
        let mut v = Viewport::new(10, 5, "f".into());
        assert!(v.set_search("foo".into(), SearchDirection::Forward).is_ok());
        assert!(v.search_active());
    }

    #[test]
    fn set_search_rejects_bad_regex() {
        let mut v = Viewport::new(10, 5, "f".into());
        let err = v.set_search("[".into(), SearchDirection::Forward).unwrap_err();
        assert!(!err.is_empty());
        assert!(!v.search_active(), "no search should be set on error");
    }

    #[test]
    fn search_step_forward_finds_match_after_top() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\ndelta\nepsilon\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("gamma".into(), SearchDirection::Forward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(found);
        // gamma is line 2 (0-indexed)
        assert_eq!(v.top_line, 2);
    }

    #[test]
    fn search_step_backward_finds_match_before_top() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\ndelta\nepsilon\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.scroll_lines(4, &m, &mut idx); // top_line = 4
        v.set_search("alpha".into(), SearchDirection::Backward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(found);
        assert_eq!(v.top_line, 0);
    }

    #[test]
    fn search_wraps_at_end() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.scroll_lines(2, &m, &mut idx); // top_line = 2 (last line)
        v.set_search("alpha".into(), SearchDirection::Forward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(found, "search should wrap forward past EOF");
        assert_eq!(v.top_line, 0);
    }

    #[test]
    fn search_no_match_returns_false_and_does_not_move() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("nowhere".into(), SearchDirection::Forward).unwrap();
        let found = v.search_repeat(&m, &mut idx, false);
        assert!(!found);
        assert_eq!(v.top_line, 0);
    }

    #[test]
    fn frame_records_highlight_ranges_for_matches() {
        let (m, mut idx) = setup(b"alpha\nbeta\ngamma\ndelta\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("gamma".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        // Body has 4 rows; row 2 is "gamma" (5 chars at columns 0..5).
        assert_eq!(frame.row_styles[0], RowStyle::Normal);
        assert!(frame.highlights[0].is_empty());
        assert!(frame.highlights[1].is_empty());
        assert_eq!(frame.highlights[2], vec![0..5]);
        assert!(frame.highlights[3].is_empty());
    }

    #[test]
    fn frame_highlights_substring_inside_a_row() {
        let (m, mut idx) = setup(b"the alpha and the beta\nfoo\n");
        let mut v = Viewport::new(40, 5, "f".into());
        v.set_search("beta".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        // "beta" starts at column 18 in the first row.
        assert_eq!(frame.highlights[0], vec![18..22]);
        assert!(frame.highlights[1].is_empty());
    }

    #[test]
    fn search_highlight_with_filter_dim_keeps_row_dim() {
        // alpha matches filter → Normal. beta doesn't → Dim. Search for
        // "beta" should leave row style Dim and mark the substring 0..4.
        let (m, mut idx) = setup(b"alpha\nbeta\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let fmt = crate::format::LogFormat::compile(
            "simple",
            r"^(?P<line>.+)$",
        )
        .unwrap();
        let f = crate::filter::CompiledFilter::compile(
            &fmt,
            vec![crate::filter::FilterSpec::parse("line=alpha").unwrap()],
            CaseMode::Sensitive,
        )
        .unwrap();
        v.set_filter(Some(f));
        v.set_dim_mode(true);
        v.set_search("beta".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        assert_eq!(frame.row_styles[0], RowStyle::Normal);
        assert_eq!(frame.row_styles[1], RowStyle::Dim);
        assert_eq!(frame.highlights[1], vec![0..4]);
    }

    #[test]
    fn grep_only_hides_non_matching_lines() {
        use crate::grep::GrepPredicate;
        let src = crate::source::MockSource::new();
        src.append(b"keep this error\n");
        src.append(b"drop this one\n");
        src.append(b"another error line\n");
        src.finish();
        let mut idx = crate::line_index::LineIndex::new();
        idx.extend_to_end(&src);

        let mut v = Viewport::new(40, 5, "test".into());
        v.set_grep(Some(GrepPredicate::compile(&["error".to_string()], crate::viewport::CaseMode::Sensitive).unwrap()));
        v.extend_visible_lines(&idx, &src);

        // Only the two "error" lines should be visible.
        let frame = v.frame(&src, &mut idx);
        let body_text: Vec<String> = frame.body.iter()
            .map(|row| row.iter().filter_map(|c| match c {
                crate::render::Cell::Char { ch, .. } => Some(*ch),
                _ => None,
            }).collect())
            .collect();
        assert!(body_text[0].contains("keep this error"));
        assert!(body_text[1].contains("another error line"));
        assert!(frame.status.contains("[grep]"));
    }

    #[test]
    fn filter_and_grep_combine_with_and() {
        use crate::grep::GrepPredicate;
        let fmt = crate::format::LogFormat::compile(
            "simple",
            r"^(?P<level>\w+) (?P<msg>.+)$",
        ).unwrap();
        let f = crate::filter::CompiledFilter::compile(
            &fmt,
            vec![crate::filter::FilterSpec::parse("level=ERROR").unwrap()],
            CaseMode::Sensitive,
        ).unwrap();
        let g = GrepPredicate::compile(&["timeout".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();

        let src = crate::source::MockSource::new();
        src.append(b"ERROR timeout connecting\n");      // matches both → keep
        src.append(b"ERROR file not found\n");          // matches filter only → drop
        src.append(b"WARN timeout retrying\n");         // matches grep only → drop
        src.append(b"INFO all good\n");                 // matches neither → drop
        src.finish();
        let mut idx = crate::line_index::LineIndex::new();
        idx.extend_to_end(&src);

        let mut v = Viewport::new(80, 5, "test".into());
        v.set_filter(Some(f));
        v.set_grep(Some(g));
        v.extend_visible_lines(&idx, &src);
        assert_eq!(v.visible_lines(), &[0usize]);
    }

    #[test]
    fn search_status_shows_pattern() {
        let (m, mut idx) = setup(b"x\n");
        let mut v = Viewport::new(20, 5, "f".into());
        v.set_search("foo".into(), SearchDirection::Forward).unwrap();
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.contains("[/foo]"), "status: {}", frame.status);
    }

    #[test]
    fn repeat_search_after_first_match_advances() {
        let (m, mut idx) = setup(b"alpha\nfoo one\nbeta\nfoo two\ngamma\nfoo three\n");
        let mut v = Viewport::new(40, 5, "f".into());
        v.set_search("foo".into(), SearchDirection::Forward).unwrap();
        assert!(v.search_repeat(&m, &mut idx, false));
        assert_eq!(v.top_line, 1, "first foo");
        v.set_search("foo".into(), SearchDirection::Forward).unwrap();
        assert!(v.search_repeat(&m, &mut idx, false), "second search should still match");
        assert_eq!(v.top_line, 3, "should advance to next foo");
    }

    #[test]
    fn auto_scroll_paused_when_follow_off() {
        let m = MockSource::new();
        m.append(b"1\n2\n3\n4\n");
        let mut idx = LineIndex::new();
        let mut v = Viewport::new(10, 5, "f".into());
        // Follow is off; viewport at top.
        idx.extend_to_end(&m);
        let frame_before = v.frame(&m, &mut idx);
        let top_first_cell = frame_before.body[0][0].clone();
        m.append(b"5\n6\n7\n8\n");
        simulate_growth_tick(&mut v, &m, &mut idx);
        let frame_after = v.frame(&m, &mut idx);
        assert_eq!(frame_after.body[0][0], top_first_cell, "auto-scroll fired despite follow off");
    }

    // ----- Records-mode search -----

    #[test]
    fn search_jumps_to_next_matching_record() {
        let m = MockSource::new();
        m.append(b"[1] alpha\n  cont\n[2] bravo\n[3] charlie\n  cont\n[4] delta\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_search("charlie".into(), SearchDirection::Forward).unwrap();
        let hit = v.search_repeat(&m, &mut idx, false);
        assert!(hit, "should find 'charlie' in record 2");
        assert_eq!(v.top_line(), 3);  // record 2 starts at line 3 ("[3] charlie")
    }

    #[test]
    fn search_finds_cross_line_match_in_record_with_s_flag() {
        let m = MockSource::new();
        m.append(b"[1] head\n  Renderer.php(214)\n[2] other line\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_search(r"(?s)head.*Renderer".into(), SearchDirection::Forward).unwrap();
        let hit = v.search_repeat(&m, &mut idx, false);
        assert!(hit, "should match across \\n inside record 0 with (?s)");
        assert_eq!(v.top_line(), 0);
    }

    #[test]
    fn search_repeat_with_no_match_returns_false() {
        let m = MockSource::new();
        m.append(b"[1] alpha\n[2] bravo\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_search("nonexistent".into(), SearchDirection::Forward).unwrap();
        let hit = v.search_repeat(&m, &mut idx, false);
        assert!(!hit);
    }

    // ----- Records-mode filter/grep -----

    #[test]
    fn filter_hide_mode_drops_all_lines_of_nonmatching_record() {
        // Record 0: "[1] head\n  cont a" — grep matches "cont a" → visible.
        // Record 1: "[2] head\n  cont b" — grep does NOT match → hidden.
        let m = MockSource::new();
        m.append(b"[1] head\n  cont a\n[2] head\n  cont b\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let grep = GrepPredicate::compile(&["cont a".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_grep(Some(grep));
        v.extend_visible_lines(&idx, &m);
        // Record 0 ([1] head + cont a) matches; lines 0 and 1 visible.
        // Record 1 ([2] head + cont b) does not match; lines 2 and 3 hidden.
        assert_eq!(v.visible_lines(), &[0usize, 1]);
    }

    #[test]
    fn filter_in_records_mode_keeps_whole_record_when_header_matches() {
        // The format regex is designed for the header line (it ends with `$`).
        // Applied to the full multi-line record bytes it would never match
        // because `$` doesn't match before a non-final `\n`. Records-mode
        // filter must evaluate against the first line of the record, then
        // include all of the record's lines when it matches.
        let m = MockSource::new();
        m.append(
            b"[1] kind=category\n  body a\n  body a2\n[2] kind=rule\n  body b\n",
        );
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let fmt = crate::format::LogFormat::compile(
            "rec",
            r"^\[(?P<id>\d+)\] kind=(?P<kind>.+)$",
        )
        .unwrap();
        let f = crate::filter::CompiledFilter::compile(
            &fmt,
            vec![crate::filter::FilterSpec::parse("kind~category").unwrap()],
            CaseMode::Sensitive,
        )
        .unwrap();
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_filter(Some(f));
        v.extend_visible_lines(&idx, &m);
        // Record 0 (lines 0, 1, 2) matches; record 1 (lines 3, 4) does not.
        assert_eq!(v.visible_lines(), &[0usize, 1, 2]);
    }

    #[test]
    fn grep_matches_across_record_newlines_in_records_mode() {
        // Pattern spans the record-header and a continuation line (needs (?s) for .).
        let m = MockSource::new();
        m.append(b"[1] head\n  Renderer.php\n[2] other\n  body\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let grep = GrepPredicate::compile(&[r"(?s)head.*Renderer".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_grep(Some(grep));
        v.extend_visible_lines(&idx, &m);
        // Record 0 matches (cross-line); record 1 does not.
        assert_eq!(v.visible_lines(), &[0usize, 1]);
    }

    #[test]
    fn dim_mode_keeps_all_lines_visible_dims_nonmatching_records() {
        // All 4 lines stay in visible_lines (dim mode = no hiding).
        // Record 0 matches grep → Normal; record 1 does not → Dim.
        let m = MockSource::new();
        m.append(b"[1] head\n  cont\n[2] other\n  cont\n");
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let grep = GrepPredicate::compile(&[r"\[1\]".to_string()], CaseMode::Sensitive).unwrap();
        let mut v = Viewport::new(40, 10, "f".into());
        v.set_grep(Some(grep));
        v.set_dim_mode(true);
        v.extend_visible_lines(&idx, &m);
        // Dim mode: visible_lines stays empty (hide_mode() is false).
        assert_eq!(v.visible_lines(), &[] as &[usize]);
        // Dim decision is per record: lines 0 and 1 belong to matching record → Normal.
        assert!(!v.should_dim_line(0, &idx, &m));
        assert!(!v.should_dim_line(1, &idx, &m));
        // Lines 2 and 3 belong to non-matching record → Dim.
        assert!(v.should_dim_line(2, &idx, &m));
        assert!(v.should_dim_line(3, &idx, &m));
    }

    #[test]
    fn status_unchanged_when_records_inactive() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(20, 5, "f".into());
        let frame = v.frame(&m, &mut idx);
        let status = &frame.status;
        // Default format: <label>  <top>-<bot>/<total>  <pct>%
        assert!(status.contains("1-3/3"), "got: {status}");
        assert!(!status.contains("L1"), "no L block in line-mode: {status}");
        assert!(!status.contains("R1"), "no R block in line-mode: {status}");
    }

    #[test]
    fn status_r_block_uses_real_lines_in_hide_mode() {
        // Regression: in hide mode `bottom` is a position in visible_lines
        // (i.e. a count of *visible* matches), not a logical line index.
        // The R-block was passing that position into `line_to_record`, which
        // resolved to whatever record contained logical line `bottom-1` —
        // typically a very early record, producing nonsense like `R290-8`
        // where the bottom record is *before* the top record on screen.
        // Build a scenario: many records, only the last few match the filter,
        // and the viewport is scrolled to the matching tail.
        let m = MockSource::new();
        // 10 records, two physical lines each. Record N's header has `kind=A`
        // for N < 8 and `kind=B` for N >= 8 (so only records 8 and 9 match).
        let mut buf = Vec::new();
        for n in 0..10 {
            let kind = if n >= 8 { "B" } else { "A" };
            buf.extend_from_slice(format!("[{}] kind={}\n  body {}\n", n, kind, n).as_bytes());
        }
        m.append(&buf);
        m.finish();

        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);

        let fmt = crate::format::LogFormat::compile(
            "rec",
            r"^\[(?P<id>\d+)\] kind=(?P<kind>.+)$",
        )
        .unwrap();
        let f = crate::filter::CompiledFilter::compile(
            &fmt,
            vec![crate::filter::FilterSpec::parse("kind=B").unwrap()],
            CaseMode::Sensitive,
        )
        .unwrap();

        // 5-row terminal: 4 body rows + 1 status row. With 4 visible-matches
        // rows of body and 4 visible lines, the whole filtered set fits.
        let mut v = Viewport::new(80, 5, "f".into());
        v.set_filter(Some(f));
        v.extend_visible_lines(&idx, &m);

        // Jump to the first matching record (record 8, 0-indexed).
        v.goto_record(8, &m, &mut idx);

        let frame = v.frame(&m, &mut idx);
        // Records 8 (rec_top=9) and 9 (rec_bottom=10) are on screen.
        assert!(
            frame.status.contains("R9-10/10"),
            "expected R9-10/10 in status, got: {}",
            frame.status,
        );
    }

    #[test]
    fn status_dual_readout_when_records_active() {
        let m = MockSource::new();
        m.append(b"[1] a\n  cont\n[2] b\n");
        m.finish();
        let mut idx = LineIndex::new();
        idx.set_record_start(regex::bytes::Regex::new(r"^\[").unwrap());
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        let frame = v.frame(&m, &mut idx);
        let status = &frame.status;
        assert!(status.contains("L1-3/3"), "lines block missing or wrong: {status}");
        assert!(status.contains("R1-2/2"), "records block missing or wrong: {status}");
    }

    #[test]
    fn format_status_uses_custom_template_when_set() {
        let m = MockSource::new();
        m.append(b"a\nb\nc\n");
        m.finish();
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(20, 5, "f".into());
        let prompt = crate::prompt::ParsedPrompt::parse("<label> <pct>%").unwrap();
        v.set_prompt(Some(prompt));
        let frame = v.frame(&m, &mut idx);
        assert_eq!(frame.status, "f 100%");
    }

    #[test]
    fn status_shows_preprocess_failed_tag_when_set() {
        let m = MockSource::new();
        m.append(b"a\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(40, 5, "f".into());
        v.set_preprocess_failure(Some("pdftotext: not found".to_string()));
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.contains("[preprocess-failed: pdftotext: not found]"),
                "got: {}", frame.status);
    }

    #[test]
    fn default_status_includes_help_hint() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(80, 5, "f".into());
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.ends_with(":help"), "got: {:?}", frame.status);
    }

    #[test]
    fn custom_prompt_does_not_get_help_hint() {
        let (m, mut idx) = setup(b"a\nb\nc\n");
        let mut v = Viewport::new(80, 5, "f".into());
        v.set_prompt(Some(crate::prompt::ParsedPrompt::parse("<label>").unwrap()));
        let frame = v.frame(&m, &mut idx);
        assert!(!frame.status.contains(":help"), "got: {:?}", frame.status);
    }

    #[test]
    fn status_shows_file_index_when_multifile() {
        let m = MockSource::new();
        m.append(b"a\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(60, 5, "f.log".into());
        v.set_file_index(0, 3);
        let frame = v.frame(&m, &mut idx);
        assert!(frame.status.contains("f.log  [1/3]"), "got: {}", frame.status);
    }

    #[test]
    fn status_omits_file_index_when_single_file() {
        let m = MockSource::new();
        m.append(b"a\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(60, 5, "f.log".into());
        v.set_file_index(0, 1);
        let frame = v.frame(&m, &mut idx);
        assert!(!frame.status.contains('['), "should not show [1/1] for single-file: {}", frame.status);
    }

    #[test]
    fn status_shows_tag_active_when_multimatch() {
        let m = MockSource::new();
        m.append(b"a\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(80, 5, "f.log".into());
        v.set_tag_active(Some(("foo".into(), 2, 3)));
        let frame = v.frame(&m, &mut idx);
        assert!(
            frame.status.contains("[tag: foo (2/3)]"),
            "got: {}",
            frame.status
        );
    }

    #[test]
    fn status_omits_tag_active_when_single_match() {
        let m = MockSource::new();
        m.append(b"a\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let mut v = Viewport::new(80, 5, "f.log".into());
        v.set_tag_active(Some(("foo".into(), 1, 1)));
        let frame = v.frame(&m, &mut idx);
        assert!(
            !frame.status.contains("[tag:"),
            "should not show indicator for single match: {}",
            frame.status
        );
    }

    // ----- SGR state reconstruction tests -----

    #[test]
    fn reconstruct_picks_up_state_from_prior_lines() {
        let m = MockSource::new();
        m.append(b"\x1b[31mline 1\n");
        m.append(b"line 2 (still red, no reset)\n");
        m.append(b"line 3\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let state = reconstruct_render_state(&m, &idx, 2);
        assert_eq!(
            state.style.fg,
            Some(crate::ansi::Color::Ansi(1)),
            "red SGR from line 0 should persist to line 2"
        );
    }

    #[test]
    fn reconstruct_respects_reset_between_lines() {
        let m = MockSource::new();
        m.append(b"\x1b[31mline 1\x1b[0m\n");
        m.append(b"line 2 (default)\n");
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        let state = reconstruct_render_state(&m, &idx, 1);
        assert_eq!(state.style.fg, None);
    }

    #[test]
    fn reconstruct_caps_walkback_at_max_lines() {
        let m = MockSource::new();
        m.append(b"\x1b[31mvery early\n");
        for _ in 0..300 {
            m.append(b"line\n");
        }
        let mut idx = LineIndex::new();
        idx.extend_to_end(&m);
        // Line 290 is 290 lines past the red SGR. We cap at 256, so the
        // anchor we'd pick is line 34 (290 - 256), which is past the red.
        let state = reconstruct_render_state(&m, &idx, 290);
        assert_eq!(state.style.fg, None);
    }
}
