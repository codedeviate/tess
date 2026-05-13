use std::path::PathBuf;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "tess", version, about = "A less-style terminal pager.")]
pub struct Args {
    /// Chop long lines instead of wrapping.
    #[arg(short = 'S', long = "chop-long-lines", display_order = 1)]
    pub chop: bool,

    /// Force the content type for `--prettify` (otherwise auto-detected from
    /// the filename extension and the first bytes). Values:
    /// `auto`, `raw`, `json`, `yaml`, `toml`, `xml`, `html`, `csv`.
    /// Setting this implies `--prettify` (unless the value is `raw`/`auto`).
    #[arg(long = "content-type", value_name = "TYPE", display_order = 2)]
    pub content_type: Option<String>,

    /// With `--filter`, dim non-matching lines instead of hiding them. Keeps
    /// surrounding context visible.
    #[arg(long = "dim", display_order = 3)]
    pub dim: bool,

    /// Render each parsed line through this template instead of showing the
    /// raw line. Syntax: `<fieldname>` placeholders, `\<` for literal `<`,
    /// `\\` for literal `\`. Example: `--display '[<time>] <status> <msg>'`.
    /// Overrides the format's `display` key (if set). Requires `--format`.
    /// Search still matches against the raw line.
    #[arg(long = "display", value_name = "TEMPLATE", display_order = 4)]
    pub display: Option<String>,

    /// Print a curated list of usage examples and exit.
    #[arg(long = "examples", display_order = 5)]
    pub examples: bool,

    /// Filter visible lines by parsed field. Repeatable; multiple filters AND.
    /// Operators: `=` (exact), `!=` (exact ≠), `~` (regex), `!~` (regex ≠),
    /// `<`, `<=`, `>`, `>=` (numeric if both sides parse as numbers, else
    /// lexicographic). Examples: `--filter status=500`, `--filter ip~^10\.`,
    /// `--filter 'status>=500'` (quote `<` and `>` to avoid shell redirection).
    /// Requires `--format`.
    #[arg(long = "filter", value_name = "FIELD<op>VALUE", display_order = 6)]
    pub filter: Vec<String>,

    /// Follow mode: keep watching the source for new bytes (like `tail -f`).
    /// Jumps to the bottom on startup. Toggle with Shift-F at runtime.
    #[arg(short = 'f', long = "follow", display_order = 7)]
    pub follow: bool,

    /// Apply a named log format (built-in or user-defined in
    /// ~/.config/tess/formats.toml). Required by `--filter`.
    #[arg(long = "format", value_name = "NAME", display_order = 8)]
    pub format: Option<String>,

    /// Filter visible lines by regex against the raw line. Repeatable;
    /// multiple `--grep` arguments AND. Works on any input — no `--format`
    /// required. Composes with `--filter` (both must match) and with
    /// `--dim` (non-matches stay visible but faded).
    /// Example: `--grep error --grep '^\['`.
    #[arg(long = "grep", value_name = "PATTERN", display_order = 9)]
    pub grep: Vec<String>,

    /// Show only the first N lines of the source. Mutually exclusive with --tail.
    #[arg(long = "head", value_name = "N", conflicts_with = "tail", display_order = 10)]
    pub head: Option<usize>,

    /// Show line numbers.
    #[arg(short = 'N', long = "LINE-NUMBERS", display_order = 11)]
    pub line_numbers: bool,

    /// Print available log formats and their named fields, then exit.
    #[arg(long = "list-formats", display_order = 12)]
    pub list_formats: bool,

    /// Live mode: re-read the file when its on-disk content changes (mtime,
    /// size, or inode). Use this for files rewritten in place — source files
    /// being edited, files saved by an editor or AI agent. Different from
    /// `--follow` (which watches for *appended* bytes); the two are mutually
    /// exclusive. Press `R` inside the pager to force a reload.
    #[arg(long = "live", conflicts_with = "follow", display_order = 13)]
    pub live: bool,

    /// Print the full user manual and exit.
    #[arg(long = "manual", display_order = 14)]
    pub manual: bool,

    /// Non-interactive batch mode: apply --filter / --grep / --head / --tail / --prettify
    /// to the source and write the resulting raw bytes to FILE, then exit.
    /// Use `-` for stdout (`--stdout` is a synonym). Skips the alt-screen and
    /// raw mode entirely. With `--follow`, doesn't exit — keeps appending
    /// matching new bytes to FILE as they arrive (Ctrl-C to stop). Not
    /// compatible with `--live`.
    #[arg(short = 'o', long = "output", value_name = "FILE", display_order = 15)]
    pub output: Option<String>,

    /// Pretty-print structured content (JSON, YAML, TOML, XML, HTML, CSV).
    /// Detects the type from the filename extension or the first bytes; use
    /// `--content-type=NAME` to override. Static files only — not allowed
    /// with `--follow`, `--live`, or `--filter`. Toggle interactively with
    /// `Shift-P`; force a type with `-P` then a letter (j/y/t/x/h/c).
    #[arg(long = "prettify", display_order = 16)]
    pub prettify: bool,

    /// Synonym for `--output -`: write the batch-mode output to stdout.
    #[arg(long = "stdout", conflicts_with = "output", display_order = 17)]
    pub stdout: bool,

    /// Tab stop width (default 8).
    #[arg(long = "tab-width", default_value_t = 8, display_order = 18)]
    pub tab_width: u8,

    /// Show only the last N lines of the source. For files this skips most of
    /// the index work — useful for huge logs. Combine with `-f` for `tail -f`.
    /// Mutually exclusive with --head. Streaming stdin is not supported.
    #[arg(long = "tail", value_name = "N", conflicts_with = "head", display_order = 19)]
    pub tail: Option<usize>,

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
    fn help_lists_flags_in_alphabetical_order() {
        use clap::CommandFactory;
        let mut cmd = Args::command();
        let help = cmd.render_help().to_string();

        let expected = [
            "--chop-long-lines",
            "--content-type",
            "--dim",
            "--display",
            "--examples",
            "--filter",
            "--follow",
            "--format",
            "--grep",
            "--head",
            "--LINE-NUMBERS",
            "--list-formats",
            "--live",
            "--manual",
            "--output",
            "--prettify",
            "--stdout",
            "--tab-width",
            "--tail",
        ];
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
