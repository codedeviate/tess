use std::path::PathBuf;
use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "rustless", version, about = "A less-style terminal pager.")]
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

    /// Files to view (only the first is opened in MVP).
    pub files: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_no_flags_no_files() {
        let a = Args::parse_from(["rustless"]);
        assert!(!a.line_numbers);
        assert!(!a.chop);
        assert_eq!(a.tab_width, 8);
        assert!(a.files.is_empty());
    }

    #[test]
    fn parses_short_flags_and_file() {
        let a = Args::parse_from(["rustless", "-N", "-S", "foo.txt"]);
        assert!(a.line_numbers);
        assert!(a.chop);
        assert_eq!(a.files, vec![PathBuf::from("foo.txt")]);
    }

    #[test]
    fn parses_tab_width() {
        let a = Args::parse_from(["rustless", "--tab-width", "4", "x"]);
        assert_eq!(a.tab_width, 4);
    }

    #[test]
    fn collects_multiple_files() {
        let a = Args::parse_from(["rustless", "a", "b", "c"]);
        assert_eq!(a.files.len(), 3);
    }
}
