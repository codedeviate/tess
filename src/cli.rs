use std::path::PathBuf;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "tess", version, about = "A less-style terminal pager.")]
pub struct Args {
    /// Show line numbers.
    #[arg(short = 'N', long = "LINE-NUMBERS")]
    pub line_numbers: bool,

    /// Chop long lines instead of wrapping.
    #[arg(short = 'S', long = "chop-long-lines")]
    pub chop: bool,

    /// Tab stop width (default 8).
    #[arg(long = "tab-width", default_value_t = 8)]
    pub tab_width: u8,

    /// Follow mode: keep watching the source for new bytes (like `tail -f`).
    /// Jumps to the bottom on startup. Toggle with Shift-F at runtime.
    #[arg(short = 'f', long = "follow")]
    pub follow: bool,

    /// Show only the first N lines of the source. Mutually exclusive with --tail.
    #[arg(long = "head", value_name = "N", conflicts_with = "tail")]
    pub head: Option<usize>,

    /// Show only the last N lines of the source. For files this skips most of
    /// the index work — useful for huge logs. Combine with `-f` for `tail -f`.
    /// Mutually exclusive with --head. Streaming stdin is not supported.
    #[arg(long = "tail", value_name = "N", conflicts_with = "head")]
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
}
