use std::path::PathBuf;
use clap::Parser;
use clap::builder::styling::{AnsiColor, Color, Style};
use clap::builder::Styles;

const HELP_STYLES: Styles = Styles::styled()
    .header(Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Yellow))))
    .usage(Style::new().bold().fg_color(Some(Color::Ansi(AnsiColor::Yellow))))
    .literal(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))))
    .placeholder(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))));

#[derive(Parser, Debug, Clone)]
#[command(name = "tess", version, about = "A less-style terminal pager.", styles = HELP_STYLES)]
// Repeated scalar flags take the last value rather than erroring. This makes
// `less`-style "last flag wins" work and, crucially, lets a CLI flag override
// the same flag injected by a group expansion (e.g. `--mygroup --display ...`
// overriding the group's own `display`). Repeatable flags (`--filter`,
// `--grep`) use Append actions and are unaffected — they still accumulate.
#[command(args_override_self = true)]
// Fields are ordered alphabetically by long-flag name (case-insensitive) so
// that `--help` lists them in that order — clap's derive renders options in
// field-declaration order. The `help_lists_flags_in_alphabetical_order` test
// enforces this; keep new flags in their sorted slot.
//
// `IGNORE_CASE` (-I) and `QUIT_AT_EOF` (-E) are intentionally upper-case to
// mirror less's case-distinguished flag pairs (`-i`/`-I`, `-e`/`-E`) and to
// stay distinct from their lower-case `ignore_case`/`quit_at_eof` siblings.
#[allow(non_snake_case)]
pub struct Args {
    /// Render images with Unicode half-blocks (▀, fg=top pixel, bg=bottom
    /// pixel) for ~2× vertical detail instead of the default character ramp.
    #[arg(long = "blocks")]
    pub blocks: bool,

    /// Chop long lines instead of wrapping.
    #[arg(short = 'S', long = "chop-long-lines")]
    pub chop: bool,

    /// Force the content type for `--prettify` (otherwise auto-detected from
    /// the filename extension and the first bytes). Values:
    /// `auto`, `raw`, `json`, `yaml`, `toml`, `xml`, `html`, `csv`.
    /// Setting this implies `--prettify` (unless the value is `raw`/`auto`).
    #[arg(long = "content-type", value_name = "TYPE")]
    pub content_type: Option<String>,

    /// With `--filter`, dim non-matching lines instead of hiding them. Keeps
    /// surrounding context visible.
    #[arg(long = "dim")]
    pub dim: bool,

    /// Render each parsed line through this template instead of showing the
    /// raw line. Syntax: `<fieldname>` placeholders, `\<` for literal `<`,
    /// `\\` for literal `\`. Example: `--display '[<time>] <status> <msg>'`.
    /// Overrides the format's `display` key (if set). Requires `--format`.
    /// Search still matches against the raw line.
    #[arg(long = "display", value_name = "TEMPLATE")]
    pub display: Option<String>,

    /// Print a curated list of usage examples and exit.
    #[arg(long = "examples")]
    pub examples: bool,

    /// In follow mode with piped stdin, exit when the upstream writer
    /// closes the pipe. Default behavior (off): tess remains open on
    /// the captured content after stdin EOF. Mirrors
    /// `less --exit-follow-on-close`.
    #[arg(long = "exit-follow-on-close")]
    pub exit_follow_on_close: bool,

    /// Filter visible lines by parsed field. Repeatable; multiple filters AND.
    /// Operators: `=` (exact), `!=` (exact ≠), `~` (regex), `!~` (regex ≠),
    /// `<`, `<=`, `>`, `>=` (numeric if both sides parse as numbers, else
    /// lexicographic). Examples: `--filter status=500`, `--filter ip~^10\.`,
    /// `--filter 'status>=500'` (quote `<` and `>` to avoid shell redirection).
    /// Requires `--format`.
    #[arg(long = "filter", value_name = "FIELD<op>VALUE")]
    pub filter: Vec<String>,

    /// Follow mode: keep watching the source for new bytes (like `tail -f`).
    /// Jumps to the bottom on startup. Toggle with Shift-F at runtime.
    #[arg(short = 'f', long = "follow")]
    pub follow: bool,

    /// Follow the file by path rather than by descriptor (matches
    /// `tail -F` / `less --follow-name`). `tess` already does this —
    /// rotation and truncation are detected on every poll and the
    /// source re-opens by path (since 0.25.0). This flag is accepted
    /// for compatibility and currently has no behavioral effect.
    #[arg(long = "follow-name")]
    pub follow_name: bool,

    /// In follow mode, any user motion (scroll, page, goto-line) suspends
    /// following. Re-engage with Shift-F. Default off: today's behavior
    /// (movement keeps follow on; auto-scroll suspended while the viewport
    /// is not at bottom). Matches `less +F` semantics when enabled.
    #[arg(long = "follow-suspend-on-motion")]
    pub follow_suspend_on_motion: bool,

    /// Apply a named log format (built-in or user-defined in
    /// ~/.config/tess/formats.toml). Required by `--filter`.
    #[arg(long = "format", value_name = "NAME")]
    pub format: Option<String>,

    /// Filter visible lines by regex against the raw line. Repeatable;
    /// multiple `--grep` arguments AND. Works on any input — no `--format`
    /// required. Composes with `--filter` (both must match) and with
    /// `--dim` (non-matches stay visible but faded).
    /// Example: `--grep error --grep '^\['`.
    #[arg(long = "grep", value_name = "PATTERN")]
    pub grep: Vec<String>,

    /// Show only the first N lines of the source. Mutually exclusive with --tail.
    #[arg(long = "head", value_name = "N", conflicts_with = "tail")]
    pub head: Option<usize>,

    /// Pin the top L source lines (and the left C columns, when
    /// horizontal scroll is supported) at the top of the viewport.
    /// Form: `L` or `L,C`. Default `0,0` (off). Mirrors `less --header`.
    /// Runtime adjustment: `:header L [C]`.
    #[arg(long = "header", value_name = "L[,C]")]
    pub header: Option<String>,

    /// Render the source as an xxd-style hex dump instead of byte-faithful
    /// text. 16 bytes per row, offset prefix, ASCII gutter. Mutually
    /// exclusive with parsing- and rendering-oriented flags.
    #[arg(
        long = "hex",
        conflicts_with_all = ["filter", "grep", "prettify", "format", "display", "record_start", "prompt", "preprocess", "or_filter", "or_grep", "or_group"],
    )]
    pub hex: bool,

    /// Hex characters per group in `--hex` mode. One of 2, 4, 8, 16, 32
    /// (default 4, matching `xxd`). 32 means the whole row as a single
    /// group with no spacing between hex pairs. Requires `--hex`. Can be
    /// changed at runtime with `:hex N`.
    #[arg(
        long = "hex-group",
        value_name = "N",
        default_value_t = 4,
        requires = "hex",
    )]
    pub hex_group: usize,

    /// Smart-case search. `/`, `?`, `--grep`, and `--filter`'s `~` / `!~`
    /// operators match case-insensitively unless the pattern contains an
    /// uppercase character. Mirrors `less -i` / ripgrep / vim smartcase.
    /// Mutually exclusive with `-I`. Runtime toggle: `:case`.
    #[arg(short = 'i', long = "ignore-case", conflicts_with = "IGNORE_CASE")]
    pub ignore_case: bool,

    /// Force case-insensitive search regardless of pattern case. Mirrors
    /// `less -I`. Mutually exclusive with `-i`.
    #[arg(short = 'I', long = "IGNORE-CASE")]
    pub IGNORE_CASE: bool,

    /// Target width in columns for image rendering. Defaults to the terminal
    /// width interactively, or 80 when exporting to a file/stdout.
    #[arg(long = "image-width", value_name = "N")]
    pub image_width: Option<usize>,

    /// Show line numbers.
    #[arg(short = 'N', long = "LINE-NUMBERS")]
    pub line_numbers: bool,

    /// Print available log formats and their named fields, then exit.
    #[arg(long = "list-formats")]
    pub list_formats: bool,

    /// Live mode: re-read the file when its on-disk content changes (mtime,
    /// size, or inode). Use this for files rewritten in place — source files
    /// being edited, files saved by an editor or AI agent. Different from
    /// `--follow` (which watches for *appended* bytes); the two are mutually
    /// exclusive. Press `R` inside the pager to force a reload.
    #[arg(long = "live", conflicts_with = "follow")]
    pub live: bool,

    /// Print the full user manual and exit.
    #[arg(long = "manual")]
    pub manual: bool,

    /// Enable mouse capture: click rows in the file picker / help overlay,
    /// and scrollwheel scrolls the body. Trade-off: most terminals disable
    /// their native text selection while mouse capture is on.
    #[arg(long = "mouse")]
    pub mouse: bool,

    /// Show raw control bytes as `^X` glyphs (pre-0.18 default). Disables
    /// SGR / OSC interpretation. Honoured also by the `NO_COLOR` environment
    /// variable (any non-empty value) and `CLICOLOR=0`.
    #[arg(long = "no-color")]
    pub no_color: bool,

    /// Disable search-match highlighting by default. Search still
    /// navigates (`n` / `N` jump to matches); the visual reverse-video
    /// highlight is suppressed. Runtime toggle: `:hlsearch` / `:nohlsearch`.
    /// Mirrors `less -G`.
    #[arg(short = 'G', long = "no-hilite-search")]
    pub no_hilite_search: bool,

    /// Treat a detected image file as raw/normal text instead of rendering it
    /// as ASCII art. Has no effect on non-image inputs.
    #[arg(long = "no-image")]
    pub no_image: bool,

    /// Don't enter the alt-screen on startup. Content remains in
    /// terminal scrollback after exit. Crucial for piped use and
    /// debugging. Mirrors `less -X` / `--no-init`.
    #[arg(short = 'X', long = "no-init")]
    pub no_init: bool,

    /// Ignore $LESSOPEN. Useful when LESSOPEN is exported but not wanted
    /// for one invocation.
    #[arg(long = "no-preprocess", conflicts_with = "preprocess")]
    pub no_preprocess: bool,

    /// OR-filter: a field condition where matching ANY condition in its
    /// OR-group is enough (the group is satisfied). AND'd with the required
    /// --filter/--grep. Joins the group set by the most recent --or-group, or
    /// `default` if none. Requires --format. Repeatable.
    #[arg(long = "or-filter", value_name = "FIELD<op>VALUE")]
    pub or_filter: Vec<String>,

    /// OR-grep: a raw-regex condition where matching ANY condition in its
    /// OR-group is enough. Works on any input. Joins the group set by the most
    /// recent --or-group, or `default` if none. Repeatable.
    #[arg(long = "or-grep", value_name = "PATTERN")]
    pub or_grep: Vec<String>,

    /// Open an OR-group: subsequent --or-filter/--or-grep join NAME until the
    /// next --or-group. Conditions before any marker form the `default` group.
    /// Every non-empty group must have ≥1 match (groups are AND'd). Repeatable.
    #[arg(long = "or-group", value_name = "NAME")]
    pub or_group: Vec<String>,

    /// Non-interactive batch mode: apply --filter / --grep / --head / --tail / --prettify
    /// to the source and write the resulting raw bytes to FILE, then exit.
    /// Use `-` for stdout (`--stdout` is a synonym). Skips the alt-screen and
    /// raw mode entirely. With `--follow`, doesn't exit — keeps appending
    /// matching new bytes to FILE as they arrive (Ctrl-C to stop). Not
    /// compatible with `--live`.
    #[arg(short = 'o', long = "output", value_name = "FILE")]
    pub output: Option<String>,

    /// Pipe the source file through this command before rendering.
    /// Must start with `|`; `%s` is substituted with the file path.
    /// Example: `--preprocess '|pdftotext %s -'`. Overrides $LESSOPEN.
    #[arg(
        long = "preprocess",
        value_name = "CMD",
        conflicts_with_all = ["no_preprocess", "hex", "follow", "live"],
    )]
    pub preprocess: Option<String>,

    /// Pretty-print structured content (JSON, YAML, TOML, XML, HTML, CSV).
    /// Detects the type from the filename extension or the first bytes; use
    /// `--content-type=NAME` to override. Static files only — not allowed
    /// with `--follow`, `--live`, or `--filter`. Toggle interactively with
    /// `Shift-P`; force a type with `-P` then a letter (j/y/t/x/h/c).
    #[arg(long = "prettify")]
    pub prettify: bool,

    /// Replace the hardcoded status format with a templated string.
    /// Uses the same `<field>` syntax as `--display`. Available fields:
    /// label, top, bottom, total, pct, rec-top, rec-bottom, rec-total,
    /// rec-block, wrap-offset, format-tag, filter-tag, grep-tag,
    /// hide-tag, search-tag, pretty-tag, live-tag, follow-tag.
    /// Per-format default can be set via `prompt = '...'` in formats.toml.
    /// Mutually exclusive with --hex.
    #[arg(long = "prompt", value_name = "TEMPLATE", conflicts_with = "hex")]
    pub prompt: Option<String>,

    /// Style for `--prompt` output (and per-format `prompt_style`). Same
    /// grammar as `--status-style`. Default: empty (no extra styling on top
    /// of what the prompt template itself emits).
    #[arg(long = "prompt-style", value_name = "SPEC", default_value = "")]
    pub prompt_style: String,

    /// Quit when the user tries to scroll forward past end-of-file for
    /// the second time. Mirrors `less -e`. Mutually exclusive with `-E`.
    #[arg(short = 'e', long = "quit-at-eof", conflicts_with = "QUIT_AT_EOF")]
    pub quit_at_eof: bool,

    /// Quit the first time end-of-file is reached. Mirrors `less -E`.
    #[arg(short = 'E', long = "QUIT-AT-EOF")]
    pub QUIT_AT_EOF: bool,

    /// Exit immediately (without paging) if the entire source fits on
    /// one screen. Ignored with piped stdin in follow mode. Mirrors
    /// `less -F`.
    #[arg(short = 'F', long = "quit-if-one-screen")]
    pub quit_if_one_screen: bool,

    /// Accepted for `less` compatibility. tess always exits on Ctrl-C
    /// (Ctrl-C → Command::Quit in the input table), so this flag is a
    /// no-op. Provided so existing `less` invocations work unchanged.
    #[arg(short = 'K', long = "quit-on-intr")]
    pub quit_on_intr: bool,

    /// Pass every byte to the terminal raw, including cursor moves and
    /// non-SGR escape sequences. Risky: scroll math may break on long lines.
    /// Less-style -r. Mutually exclusive with --no-color.
    #[arg(short = 'r', long = "raw-control-chars", conflicts_with = "no_color")]
    pub raw_control_chars: bool,

    /// Accept `less -R`: interpret SGR/OSC color escapes, strip other control
    /// sequences — which is tess's default. Provided for drop-in `less -R`
    /// compatibility. Distinct from `-r` (full raw) and `--no-color`.
    #[arg(short = 'R', long = "RAW-CONTROL-CHARS",
        conflicts_with_all = ["raw_control_chars", "no_color"])]
    pub RAW_CONTROL_CHARS: bool,

    /// Treat lines matching REGEX as record boundaries. Lines that don't
    /// match are joined to the preceding record. Affects search, filter,
    /// grep, and the status line — all operate on whole records when set.
    /// Overrides the active --format's record_start if both are present.
    /// Without --format, this is the only way to enable records mode for
    /// plain text. Example: --record-start '^\['
    #[arg(long = "record-start", value_name = "REGEX")]
    pub record_start: Option<String>,

    /// Character to show at the right edge of a chopped line (`-S` chop
    /// mode) indicating "more content right". Default `>`. Pass an empty
    /// string to disable. Mirrors `less --rscroll=c`.
    #[arg(long = "rscroll", value_name = "CHAR", default_value = ">")]
    pub rscroll: String,

    /// Column count for the ←/→ horizontal-scroll commands (default: half
    /// screen). `0` keeps the half-screen default. Mirrors `less -#`/`--shift`.
    #[arg(short = '#', long = "shift", value_name = "N")]
    pub shift: Option<u16>,

    /// Collapse runs of two or more consecutive blank lines into a
    /// single blank line at display time. Real line numbers, search,
    /// and tag jumps are unaffected (they reference the original
    /// count). Mirrors `less -s`.
    #[arg(short = 's', long = "squeeze-blank-lines")]
    pub squeeze_blanks: bool,

    /// Style for the status row. Comma-separated tokens: `bold`, `dim`,
    /// `italic`, `underline`, `reverse`, `fg=COLOR`, `bg=COLOR`. COLOR is a
    /// named color (`black`..`white`, optional `bright-` prefix), `#RRGGBB`,
    /// or an indexed value (0–255). Empty string disables theming.
    /// Default: `reverse`.
    #[arg(long = "status-style", value_name = "SPEC", default_value = "reverse")]
    pub status_style: String,

    /// Synonym for `--output -`: write the batch-mode output to stdout.
    #[arg(long = "stdout", conflicts_with = "output")]
    pub stdout: bool,

    /// Tab stop width (default 8).
    #[arg(long = "tab-width", default_value_t = 8)]
    pub tab_width: u8,

    /// Tab stops as a comma-separated column list, e.g. `-x4,8,16`. A single
    /// value is equivalent to `--tab-width`. With multiple values, tabs advance
    /// to the next listed column; past the last stop the final interval
    /// repeats. Overrides `--tab-width`. Mirrors `less -x`.
    #[arg(short = 'x', long = "tabs", value_name = "LIST")]
    pub tabs: Option<String>,

    /// Jump to the tag NAME at startup (requires a tags file).
    #[arg(short = 't', long = "tag", value_name = "NAME")]
    pub tag: Option<String>,

    /// Path to the tags file. Default: walk up from CWD looking for `tags`.
    #[arg(short = 'T', long = "tag-file", value_name = "PATH")]
    pub tag_file: Option<std::path::PathBuf>,

    /// Show only the last N lines of the source. For files this skips most of
    /// the index work — useful for huge logs. Combine with `-f` for `tail -f`.
    /// Mutually exclusive with --head. Streaming stdin is not supported.
    #[arg(long = "tail", value_name = "N", conflicts_with = "head")]
    pub tail: Option<usize>,

    /// Truecolor (24-bit RGB) handling. `auto` (default) checks `$COLORTERM`
    /// and downsamples when truecolor isn't advertised; `never` always
    /// downsamples to the 256-color palette; `always` passes RGB through
    /// regardless of terminal capability.
    #[arg(long = "truecolor", value_name = "MODE", default_value = "auto")]
    pub truecolor: String,

    /// Body lines scrolled per mouse-wheel notch under `--mouse` (default 1).
    #[arg(long = "wheel-lines", value_name = "N")]
    pub wheel_lines: Option<u16>,

    /// PageDown / PageUp step size in lines. Default: full screen
    /// height (body rows). Half-page commands always advance by half
    /// the screen regardless. Mirrors `less -zn` / `--window=n`.
    #[arg(short = 'z', long = "window", value_name = "N")]
    pub window: Option<u16>,

    /// In wrap mode, break lines on whitespace boundaries instead of
    /// mid-character when possible. Falls back to mid-character break
    /// when no whitespace fits in the row. Mirrors `less --wordwrap`.
    #[arg(long = "wordwrap")]
    pub word_wrap: bool,

    /// Files to view (only the first is opened in MVP).
    pub files: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_no_flags_no_files() {
        let a = Args::parse_from(["tess"]);
        assert!(!a.line_numbers);
        assert!(!a.chop);
        assert_eq!(a.tab_width, 8);
        assert!(a.files.is_empty());
    }

    #[test]
    fn parses_raw_control_chars_alias() {
        let a = Args::parse_from(["tess", "-R", "f"]);
        assert!(a.RAW_CONTROL_CHARS);
    }
    #[test]
    fn raw_control_chars_conflicts_with_raw_and_no_color() {
        assert!(Args::try_parse_from(["tess", "-R", "-r", "f"]).is_err());
        assert!(Args::try_parse_from(["tess", "-R", "--no-color", "f"]).is_err());
    }
    #[test]
    fn parses_shift_and_wheel_lines() {
        let a = Args::parse_from(["tess", "--shift", "12", "--wheel-lines", "5", "f"]);
        assert_eq!(a.shift, Some(12));
        assert_eq!(a.wheel_lines, Some(5));
    }

    #[test]
    fn parses_short_flags_and_file() {
        let a = Args::parse_from(["tess", "-N", "-S", "foo.txt"]);
        assert!(a.line_numbers);
        assert!(a.chop);
        assert_eq!(a.files, vec![PathBuf::from("foo.txt")]);
    }

    #[test]
    fn parses_tab_width() {
        let a = Args::parse_from(["tess", "--tab-width", "4", "x"]);
        assert_eq!(a.tab_width, 4);
    }

    #[test]
    fn parses_tabs_list() {
        let a = Args::parse_from(["tess", "--tabs", "4,8,16", "f"]);
        assert_eq!(a.tabs.as_deref(), Some("4,8,16"));
        let b = Args::parse_from(["tess", "-x", "4", "f"]);
        assert_eq!(b.tabs.as_deref(), Some("4"));
    }

    #[test]
    fn collects_multiple_files() {
        let a = Args::parse_from(["tess", "a", "b", "c"]);
        assert_eq!(a.files.len(), 3);
    }

    #[test]
    fn parses_follow_short_flag() {
        let a = Args::parse_from(["tess", "-f", "log.txt"]);
        assert!(a.follow);
        assert_eq!(a.files, vec![PathBuf::from("log.txt")]);
    }

    #[test]
    fn parses_follow_long_flag() {
        let a = Args::parse_from(["tess", "--follow"]);
        assert!(a.follow);
    }

    #[test]
    fn follow_defaults_off() {
        let a = Args::parse_from(["tess", "x"]);
        assert!(!a.follow);
    }

    #[test]
    fn parses_head() {
        let a = Args::parse_from(["tess", "--head", "100", "x"]);
        assert_eq!(a.head, Some(100));
        assert_eq!(a.tail, None);
    }

    #[test]
    fn parses_tail() {
        let a = Args::parse_from(["tess", "--tail", "50", "x"]);
        assert_eq!(a.tail, Some(50));
        assert_eq!(a.head, None);
    }

    #[test]
    fn head_and_tail_are_mutually_exclusive() {
        let r = Args::try_parse_from(["tess", "--head", "10", "--tail", "20", "x"]);
        assert!(r.is_err(), "clap should reject combining --head and --tail");
    }

    #[test]
    fn head_tail_default_to_none() {
        let a = Args::parse_from(["tess", "x"]);
        assert!(a.head.is_none());
        assert!(a.tail.is_none());
    }

    #[test]
    fn parses_grep_repeatable_and_no_format_required() {
        let a = Args::parse_from([
            "tess",
            "--grep", "error",
            "--grep", r"^\[",
            "log",
        ]);
        assert_eq!(a.grep.len(), 2);
        assert_eq!(a.grep[0], "error");
        assert_eq!(a.grep[1], r"^\[");
        assert_eq!(a.format, None);
    }

    #[test]
    fn parses_format_and_filter() {
        let a = Args::parse_from([
            "tess", "--format", "apache-combined",
            "--filter", "status=500",
            "--filter", "ip~^10\\.",
            "log",
        ]);
        assert_eq!(a.format.as_deref(), Some("apache-combined"));
        assert_eq!(a.filter.len(), 2);
        assert_eq!(a.filter[0], "status=500");
    }

    #[test]
    fn parses_dim() {
        let a = Args::parse_from(["tess", "--format", "x", "--filter", "y=z", "--dim", "f"]);
        assert!(a.dim);
    }

    #[test]
    fn parses_list_formats() {
        let a = Args::parse_from(["tess", "--list-formats"]);
        assert!(a.list_formats);
    }

    #[test]
    fn parses_manual() {
        let a = Args::parse_from(["tess", "--manual"]);
        assert!(a.manual);
    }

    #[test]
    fn parses_examples() {
        let a = Args::parse_from(["tess", "--examples"]);
        assert!(a.examples);
    }

    #[test]
    fn parses_live() {
        let a = Args::parse_from(["tess", "--live", "f"]);
        assert!(a.live);
        assert!(!a.follow);
    }

    #[test]
    fn live_and_follow_are_mutually_exclusive() {
        let r = Args::try_parse_from(["tess", "--live", "--follow", "f"]);
        assert!(r.is_err(), "clap should reject combining --live and --follow");
    }

    #[test]
    fn parses_prettify() {
        let a = Args::parse_from(["tess", "--prettify", "f.json"]);
        assert!(a.prettify);
        assert_eq!(a.content_type, None);
    }

    #[test]
    fn parses_content_type() {
        let a = Args::parse_from(["tess", "--content-type", "json", "f"]);
        assert_eq!(a.content_type.as_deref(), Some("json"));
    }

    #[test]
    fn parses_output_long_and_short() {
        let a = Args::parse_from(["tess", "-o", "/tmp/out.txt", "f"]);
        assert_eq!(a.output.as_deref(), Some("/tmp/out.txt"));
        let b = Args::parse_from(["tess", "--output", "/tmp/out.txt", "f"]);
        assert_eq!(b.output.as_deref(), Some("/tmp/out.txt"));
    }

    #[test]
    fn parses_stdout_flag() {
        let a = Args::parse_from(["tess", "--stdout", "f"]);
        assert!(a.stdout);
        assert_eq!(a.output, None);
    }

    #[test]
    fn output_and_stdout_are_mutually_exclusive() {
        let r = Args::try_parse_from(["tess", "-o", "x", "--stdout", "f"]);
        assert!(r.is_err(), "clap should reject combining --output and --stdout");
    }

    #[test]
    fn parses_mouse_flag() {
        let a = Args::parse_from(["tess", "--mouse", "f"]);
        assert!(a.mouse);
    }

    #[test]
    fn mouse_defaults_off() {
        let a = Args::parse_from(["tess", "f"]);
        assert!(!a.mouse);
    }

    #[test]
    fn parses_no_image_flag() {
        let a = Args::parse_from(["tess", "--no-image", "cat.png"]);
        assert!(a.no_image);
    }

    #[test]
    fn parses_blocks_flag() {
        let a = Args::parse_from(["tess", "--blocks", "cat.png"]);
        assert!(a.blocks);
    }

    #[test]
    fn parses_image_width() {
        let a = Args::parse_from(["tess", "--image-width", "120", "cat.png"]);
        assert_eq!(a.image_width, Some(120));
    }

    #[test]
    fn image_flags_default_off() {
        let a = Args::parse_from(["tess", "f"]);
        assert!(!a.no_image);
        assert!(!a.blocks);
        assert_eq!(a.image_width, None);
    }

    #[test]
    fn repeated_scalar_flag_takes_last_value() {
        // args_override_self: a group can inject `--display X` and a later CLI
        // `--display Y` wins instead of clap erroring "cannot be used multiple
        // times". Also covers plain `less`-style last-wins.
        let a = Args::parse_from(["tess", "--display", "X", "--display", "Y", "--format", "f"]);
        assert_eq!(a.display.as_deref(), Some("Y"));
        let b = Args::parse_from(["tess", "--tail", "5", "--tail", "1", "x"]);
        assert_eq!(b.tail, Some(1));
    }

    #[test]
    fn repeatable_flags_still_accumulate_with_override_self() {
        // args_override_self must not collapse Append-action Vec flags.
        let a = Args::parse_from(["tess", "--grep", "a", "--grep", "b", "x"]);
        assert_eq!(a.grep, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn parses_or_flags_repeatable() {
        let a = Args::parse_from([
            "tess",
            "--or-grep", "failed",
            "--or-group", "svc",
            "--or-filter", "lvl=ERROR",
            "x",
        ]);
        assert_eq!(a.or_grep, vec!["failed".to_string()]);
        assert_eq!(a.or_group, vec!["svc".to_string()]);
        assert_eq!(a.or_filter, vec!["lvl=ERROR".to_string()]);
    }

    #[test]
    fn or_flags_conflict_with_hex() {
        let r = Args::try_parse_from(["tess", "--hex", "--or-grep", "x", "f"]);
        assert!(r.is_err(), "clap should reject --hex with --or-grep");
    }

    #[test]
    fn help_lists_flags_in_alphabetical_order() {
        use clap::CommandFactory;
        let mut cmd = Args::command();
        let help = cmd.render_help().to_string();

        // The full set of long flags, in the order we expect `--help` to list
        // them: alphabetical by long name, case-insensitive. clap's auto-added
        // --help / --version are excluded (they're not in this list, so the
        // first-token scan below skips their lines).
        let expected = [
            "--blocks",
            "--chop-long-lines",
            "--content-type",
            "--dim",
            "--display",
            "--examples",
            "--exit-follow-on-close",
            "--filter",
            "--follow",
            "--follow-name",
            "--follow-suspend-on-motion",
            "--format",
            "--grep",
            "--head",
            "--header",
            "--hex",
            "--hex-group",
            "--ignore-case",
            "--IGNORE-CASE",
            "--image-width",
            "--LINE-NUMBERS",
            "--list-formats",
            "--live",
            "--manual",
            "--mouse",
            "--no-color",
            "--no-hilite-search",
            "--no-image",
            "--no-init",
            "--no-preprocess",
            "--or-filter",
            "--or-grep",
            "--or-group",
            "--output",
            "--preprocess",
            "--prettify",
            "--prompt",
            "--prompt-style",
            "--quit-at-eof",
            "--QUIT-AT-EOF",
            "--quit-if-one-screen",
            "--quit-on-intr",
            "--raw-control-chars",
            "--RAW-CONTROL-CHARS",
            "--record-start",
            "--rscroll",
            "--shift",
            "--squeeze-blank-lines",
            "--status-style",
            "--stdout",
            "--tab-width",
            "--tabs",
            "--tag",
            "--tag-file",
            "--tail",
            "--truecolor",
            "--wheel-lines",
            "--window",
            "--wordwrap",
        ];

        // Confirm `expected` is itself sorted case-insensitively — this guards
        // against a typo here masking a real ordering regression in the struct.
        let mut sorted = expected.to_vec();
        sorted.sort_by_key(|s| s.trim_start_matches('-').to_ascii_lowercase());
        assert_eq!(
            expected.to_vec(),
            sorted,
            "the `expected` list must itself be in case-insensitive alphabetical order"
        );

        // Walk the rendered help line by line. For each option line (after
        // trimming, it starts with '-'), take the first `--long` token that is
        // one of our flags. Because the flag name always precedes its own
        // description on the line, embedded `--flag` references in descriptions
        // are never matched first.
        let listed: Vec<&str> = help
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with('-'))
            .filter_map(|l| {
                l.split(|c: char| c.is_whitespace() || c == ',')
                    .find(|tok| expected.contains(tok))
            })
            .collect();
        assert_eq!(listed, expected, "help long-flag order should be alphabetical");
    }
}
