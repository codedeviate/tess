use std::collections::HashMap;
use std::path::PathBuf;

use regex::Regex;
use serde::Deserialize;

/// A named log format: a regex with named capture groups identifying the
/// fields of one log line. Used by filtering to look up field values by name.
#[derive(Debug)]
pub struct LogFormat {
    pub name: String,
    pub regex: Regex,
    /// Capture group names declared in the regex, in declaration order.
    /// Used by `--list-formats` to show users what fields are available.
    pub field_names: Vec<String>,
    /// Optional default display template (`display` key in formats.toml).
    /// When set and no CLI override is given, the viewer / batch output
    /// renders each parsed line through this template instead of the raw line.
    pub display: Option<DisplayTemplate>,
    pub record_start: Option<Regex>,
    /// Optional default status-line prompt template (`prompt` key in formats.toml).
    /// When set and no `--prompt` CLI flag is given, the viewport renders the
    /// status line through this template instead of the built-in default.
    pub prompt: Option<crate::prompt::ParsedPrompt>,
}

impl LogFormat {
    pub fn compile(name: &str, pattern: &str) -> Result<Self, String> {
        Self::compile_full(name, pattern, None, None, None)
    }

    pub fn compile_with_display(
        name: &str,
        pattern: &str,
        display: Option<&str>,
    ) -> Result<Self, String> {
        Self::compile_full(name, pattern, display, None, None)
    }

    pub fn compile_full(
        name: &str,
        pattern: &str,
        display: Option<&str>,
        record_start: Option<&str>,
        prompt: Option<&str>,
    ) -> Result<Self, String> {
        let regex = Regex::new(pattern).map_err(|e| format!("format `{name}`: {e}"))?;
        let field_names: Vec<String> = regex
            .capture_names()
            .flatten()
            .map(|s| s.to_string())
            .collect();
        if field_names.is_empty() {
            return Err(format!(
                "format `{name}`: regex must declare at least one named capture group"
            ));
        }
        let display = display
            .map(|s| {
                DisplayTemplate::compile(s, &field_names)
                    .map_err(|e| format!("format `{name}`: display: {e}"))
            })
            .transpose()?;
        let record_start = record_start
            .map(|s| Regex::new(s).map_err(|e| format!("format `{name}`: record_start: {e}")))
            .transpose()?;
        let prompt = prompt
            .map(|s| crate::prompt::ParsedPrompt::parse(s)
                .map_err(|e| format!("format `{name}`: prompt: {e}")))
            .transpose()?;
        Ok(Self {
            name: name.to_string(),
            regex,
            field_names,
            display,
            record_start,
            prompt,
        })
    }
}

/// Parsed display template (`display = '[<ts>] <level> <msg>'`).
///
/// Syntax:
/// - `<fieldname>` — replaced with the field's captured value (empty if
///   the regex didn't capture it on this line).
/// - `\<` — literal `<`.
/// - `\\` — literal `\`.
/// - Anything else — literal.
#[derive(Debug, Clone)]
pub struct DisplayTemplate {
    segments: Vec<DisplaySegment>,
    source: String,
}

#[derive(Debug, Clone)]
enum DisplaySegment {
    Literal(String),
    Field(String),
}

impl DisplayTemplate {
    pub fn compile(source: &str, field_names: &[String]) -> Result<Self, String> {
        if source.is_empty() {
            return Err("template is empty (would render every line as nothing)".to_string());
        }
        let mut segments: Vec<DisplaySegment> = Vec::new();
        let mut buf = String::new();
        let mut chars = source.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('<') => buf.push('<'),
                    Some('\\') => buf.push('\\'),
                    Some(other) => {
                        // Unknown escape: keep both bytes literally so users
                        // don't have to escape every backslash in regex-like
                        // strings.
                        buf.push('\\');
                        buf.push(other);
                    }
                    None => return Err("template ends with a lone `\\`".to_string()),
                },
                '<' => {
                    if !buf.is_empty() {
                        segments.push(DisplaySegment::Literal(std::mem::take(&mut buf)));
                    }
                    let mut name = String::new();
                    let mut closed = false;
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '>' { closed = true; break; }
                        name.push(nc);
                    }
                    if !closed {
                        return Err(format!("unterminated `<` (expected `<{name}>`)"));
                    }
                    if name.is_empty() {
                        return Err("empty field reference `<>`".to_string());
                    }
                    if !field_names.iter().any(|n| n == &name) {
                        return Err(format!(
                            "unknown field `{name}` (available: {})",
                            field_names.join(", ")
                        ));
                    }
                    segments.push(DisplaySegment::Field(name));
                }
                _ => buf.push(c),
            }
        }
        if !buf.is_empty() {
            segments.push(DisplaySegment::Literal(buf));
        }
        Ok(Self { segments, source: source.to_string() })
    }

    /// Render the template against a captures-lookup closure. Returns the
    /// rendered string. Missing fields render as empty.
    pub fn render(&self, lookup: impl Fn(&str) -> Option<String>) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                DisplaySegment::Literal(s) => out.push_str(s),
                DisplaySegment::Field(name) => {
                    if let Some(v) = lookup(name) { out.push_str(&v); }
                }
            }
        }
        out
    }

    pub fn source(&self) -> &str { &self.source }
}

/// Pairs a `DisplayTemplate` with the format's regex so callers can render
/// any single line in one call. Owns its inputs so it's `Send`-friendly.
#[derive(Debug, Clone)]
pub struct DisplayRenderer {
    template: DisplayTemplate,
    regex: Regex,
}

impl DisplayRenderer {
    pub fn new(template: DisplayTemplate, regex: Regex) -> Self {
        Self { template, regex }
    }

    pub fn template(&self) -> &DisplayTemplate { &self.template }

    /// Render `line` (raw bytes) through the template. If the line doesn't
    /// parse against the format regex, returns `None` — the caller decides
    /// whether to fall back to the raw line, skip it, or show an error.
    pub fn render_line(&self, line: &[u8]) -> Option<String> {
        let s = std::str::from_utf8(line).ok()?;
        let caps = self.regex.captures(s)?;
        Some(self.template.render(|name| {
            caps.name(name).map(|m| m.as_str().to_string())
        }))
    }
}

/// TOML schema for `~/.config/tess/formats.toml`:
///
/// ```toml
/// [format.myapp]
/// regex = "..."
///
/// [group.errorlog]
/// format = "myapp"
/// file = "/var/log/app.log"
/// follow = true
/// filter = ["level=ERROR"]
/// ```
#[derive(Debug, Deserialize)]
struct UserConfig {
    #[serde(default)]
    format: HashMap<String, FormatEntry>,
    #[serde(default)]
    group: HashMap<String, GroupEntry>,
}

#[derive(Debug, Deserialize)]
struct FormatEntry {
    regex: String,
    #[serde(default)]
    display: Option<String>,
    #[serde(default)]
    record_start: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

/// Raw group entry as deserialized from TOML. Promoted to `Group` after
/// validation.
#[derive(Debug, Deserialize, Default)]
struct GroupEntry {
    format: Option<String>,
    file: Option<String>,
    follow: Option<bool>,
    tail: Option<usize>,
    head: Option<usize>,
    dim: Option<bool>,
    line_numbers: Option<bool>,
    chop: Option<bool>,
    tab_width: Option<u8>,
    #[serde(default)]
    filter: Vec<String>,
    #[serde(default)]
    grep: Vec<String>,
}

/// A user-defined CLI shortcut. When `tess --<group_name>` appears in argv,
/// the group's flags are expanded inline and remaining positionals become
/// `--filter` arguments.
#[derive(Debug, Clone, Default)]
pub struct Group {
    pub name: String,
    pub format: Option<String>,
    pub file: Option<String>,
    pub follow: bool,
    pub tail: Option<usize>,
    pub head: Option<usize>,
    pub dim: bool,
    pub line_numbers: bool,
    pub chop: bool,
    pub tab_width: Option<u8>,
    pub filter: Vec<String>,
    pub grep: Vec<String>,
}

/// Long-form names of every built-in clap flag. A group cannot reuse one of
/// these names — it would shadow the real flag at expansion time.
const RESERVED_LONG_FLAGS: &[&str] = &[
    "format",
    "filter",
    "grep",
    "dim",
    "head",
    "tail",
    "follow",
    "LINE-NUMBERS",
    "chop-long-lines",
    "tab-width",
    "list-formats",
    "live",
    "manual",
    "examples",
    "prettify",
    "content-type",
    "help",
    "version",
    "record-start",
];

/// Built-in formats compiled from this list of (name, pattern). Patterns use
/// raw strings so backslashes don't need escaping.
const BUILTINS: &[(&str, &str)] = &[
    (
        "apache-common",
        r#"^(?P<ip>\S+) \S+ (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+)$"#,
    ),
    (
        "apache-combined",
        r#"^(?P<ip>\S+) \S+ (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+) "(?P<referer>[^"]*)" "(?P<agent>[^"]*)"$"#,
    ),
    (
        "nginx-combined",
        r#"^(?P<ip>\S+) - (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+) "(?P<referer>[^"]*)" "(?P<agent>[^"]*)"$"#,
    ),
];

fn user_config_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| {
        let mut p = PathBuf::from(h);
        p.push(".config");
        p.push("tess");
        p.push("formats.toml");
        p
    })
}

fn load_user_config() -> Result<UserConfig, String> {
    let Some(path) = user_config_path() else {
        return Ok(UserConfig { format: HashMap::new(), group: HashMap::new() });
    };
    if !path.exists() {
        return Ok(UserConfig { format: HashMap::new(), group: HashMap::new() });
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))
}

struct FormatSource {
    regex: String,
    display: Option<String>,
    record_start: Option<String>,
    prompt: Option<String>,
}

fn load_user_formats() -> Result<HashMap<String, FormatSource>, String> {
    let cfg = load_user_config()?;
    Ok(cfg.format.into_iter().map(|(k, v)| (k, FormatSource {
        regex: v.regex,
        display: v.display,
        record_start: v.record_start,
        prompt: v.prompt,
    })).collect())
}

/// Load all user-defined groups from `~/.config/tess/formats.toml`. Built-ins
/// are not provided — groups are entirely user-defined. Validates that group
/// names don't shadow built-in flag names.
pub fn load_groups() -> Result<HashMap<String, Group>, String> {
    let cfg = load_user_config()?;
    let mut out = HashMap::with_capacity(cfg.group.len());
    for (name, entry) in cfg.group {
        if RESERVED_LONG_FLAGS.contains(&name.as_str()) {
            return Err(format!(
                "group `{name}`: name collides with built-in --{name} flag"
            ));
        }
        out.insert(
            name.clone(),
            Group {
                name,
                format: entry.format,
                file: entry.file,
                follow: entry.follow.unwrap_or(false),
                tail: entry.tail,
                head: entry.head,
                dim: entry.dim.unwrap_or(false),
                line_numbers: entry.line_numbers.unwrap_or(false),
                chop: entry.chop.unwrap_or(false),
                tab_width: entry.tab_width,
                filter: entry.filter,
                grep: entry.grep,
            },
        );
    }
    Ok(out)
}

/// Load all formats: built-ins first, then any in `~/.config/tess/formats.toml`
/// (which override built-ins of the same name). Returns the compiled map keyed
/// by format name.
pub fn load_all() -> Result<HashMap<String, LogFormat>, String> {
    let mut sources: HashMap<String, FormatSource> = HashMap::new();
    for (name, pat) in BUILTINS {
        sources.insert(name.to_string(), FormatSource {
            regex: pat.to_string(),
            display: None,
            record_start: None,
            prompt: None,
        });
    }
    let user = load_user_formats()?;
    for (name, src) in user {
        sources.insert(name, src);
    }
    let mut compiled = HashMap::new();
    for (name, src) in sources {
        let fmt = LogFormat::compile_full(
            &name,
            &src.regex,
            src.display.as_deref(),
            src.record_start.as_deref(),
            src.prompt.as_deref(),
        )?;
        compiled.insert(name, fmt);
    }
    Ok(compiled)
}

/// Pre-process an argv vector before clap sees it. For every `--<name>`
/// token that matches a defined group, expand the group's flags inline and
/// switch into "filter mode" — bare positionals after the group token become
/// `--filter <arg>` pairs. Group tokens before any flag still expand
/// correctly; positionals before a group remain as-is.
///
/// CLI flags coming after the expansion override the group's values for
/// `Option<T>` flags (clap takes the last occurrence) and add to repeatable
/// flags like `--filter` (clap accumulates the `Vec<String>`).
/// Long flags that take a separate value as the next argv token (e.g.
/// `--tail 1000` rather than `--tail=1000`). Used by `expand_argv` so it
/// doesn't mistake a flag's value for a positional.
const VALUE_TAKING_LONG_FLAGS: &[&str] = &[
    "--format",
    "--filter",
    "--grep",
    "--head",
    "--tail",
    "--tab-width",
    "--record-start",
];

pub fn expand_argv(argv: Vec<String>, groups: &HashMap<String, Group>) -> Vec<String> {
    if argv.is_empty() {
        return argv;
    }
    let mut out = Vec::with_capacity(argv.len() * 2);
    let mut iter = argv.into_iter();
    out.push(iter.next().unwrap()); // argv[0] = program name
    let mut filter_mode = false;
    let mut pass_next = false;
    for arg in iter {
        if pass_next {
            pass_next = false;
            out.push(arg);
            continue;
        }
        if let Some(name) = arg.strip_prefix("--") {
            // `--flag=value` is a single token: don't try to match groups
            // against `flag=value`.
            if !name.contains('=') {
                if let Some(g) = groups.get(name) {
                    expand_group(g, &mut out);
                    filter_mode = true;
                    continue;
                }
                if VALUE_TAKING_LONG_FLAGS.contains(&arg.as_str()) {
                    // The next token is this flag's value; pass it through
                    // even in filter mode.
                    out.push(arg);
                    pass_next = true;
                    continue;
                }
            }
        }
        if filter_mode && !arg.starts_with('-') {
            out.push("--filter".into());
            out.push(arg);
            continue;
        }
        out.push(arg);
    }
    out
}

fn expand_group(g: &Group, out: &mut Vec<String>) {
    if let Some(format) = &g.format {
        out.push("--format".into());
        out.push(format.clone());
    }
    if g.follow {
        out.push("--follow".into());
    }
    if let Some(t) = g.tail {
        out.push("--tail".into());
        out.push(t.to_string());
    }
    if let Some(h) = g.head {
        out.push("--head".into());
        out.push(h.to_string());
    }
    if g.dim {
        out.push("--dim".into());
    }
    if g.line_numbers {
        out.push("-N".into());
    }
    if g.chop {
        out.push("-S".into());
    }
    if let Some(t) = g.tab_width {
        out.push("--tab-width".into());
        out.push(t.to_string());
    }
    for f in &g.filter {
        out.push("--filter".into());
        out.push(f.clone());
    }
    for g_pat in &g.grep {
        out.push("--grep".into());
        out.push(g_pat.clone());
    }
    if let Some(file) = &g.file {
        out.push(file.clone());
    }
}

/// Print one line per format, with the named field list, to stdout. Used by
/// `--list-formats`.
pub fn print_format_list(formats: &HashMap<String, LogFormat>) {
    let mut names: Vec<&String> = formats.keys().collect();
    names.sort();
    for name in names {
        let fmt = &formats[name];
        let fields: Vec<&str> = fmt.field_names.iter().map(|s| s.as_str()).collect();
        println!("{}: {}", name, fields.join(", "));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate the `HOME` env var; otherwise they
    /// trample each other when cargo runs tests in parallel.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn builtins_all_compile() {
        for (name, pat) in BUILTINS {
            LogFormat::compile(name, pat)
                .unwrap_or_else(|e| panic!("built-in {name} should compile: {e}"));
        }
    }

    // ----- DisplayTemplate -----

    fn fields() -> Vec<String> {
        vec!["ts".into(), "level".into(), "msg".into()]
    }

    #[test]
    fn display_template_compiles_basic() {
        let t = DisplayTemplate::compile("[<ts>] <level> <msg>", &fields()).unwrap();
        assert_eq!(t.source(), "[<ts>] <level> <msg>");
    }

    #[test]
    fn display_template_renders_substitutions() {
        let t = DisplayTemplate::compile("<level>: <msg>", &fields()).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("level".to_string(), "ERROR".to_string());
        map.insert("msg".to_string(), "boom".to_string());
        let out = t.render(|n| map.get(n).cloned());
        assert_eq!(out, "ERROR: boom");
    }

    #[test]
    fn display_template_missing_field_renders_empty() {
        let t = DisplayTemplate::compile("<level>:<msg>", &fields()).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("level".to_string(), "ERROR".to_string());
        // msg is absent
        let out = t.render(|n| map.get(n).cloned());
        assert_eq!(out, "ERROR:");
    }

    #[test]
    fn display_template_escape_sequences() {
        // Only `\<` and `\\` are recognized escapes; `>` is always literal
        // (a stray `>` outside `<...>` is fine).
        let t = DisplayTemplate::compile(r"\<not a field> <level>", &fields()).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("level".to_string(), "X".to_string());
        let out = t.render(|n| map.get(n).cloned());
        assert_eq!(out, "<not a field> X");
    }

    #[test]
    fn display_template_escape_backslash() {
        let t = DisplayTemplate::compile(r"a\\b <level>", &fields()).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("level".to_string(), "X".to_string());
        let out = t.render(|n| map.get(n).cloned());
        assert_eq!(out, r"a\b X");
    }

    #[test]
    fn display_template_rejects_empty() {
        let err = DisplayTemplate::compile("", &fields()).unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn display_template_rejects_unknown_field() {
        let err = DisplayTemplate::compile("<bogus>", &fields()).unwrap_err();
        assert!(err.contains("unknown field"), "{err}");
    }

    #[test]
    fn display_template_rejects_unterminated() {
        let err = DisplayTemplate::compile("<level", &fields()).unwrap_err();
        assert!(err.contains("unterminated"), "{err}");
    }

    #[test]
    fn display_template_rejects_empty_ref() {
        let err = DisplayTemplate::compile("<>", &fields()).unwrap_err();
        assert!(err.contains("empty field reference"), "{err}");
    }

    #[test]
    fn apache_common_extracts_fields() {
        let fmt = LogFormat::compile("apache-common", BUILTINS[0].1).unwrap();
        let line = r#"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] "GET /index.html HTTP/1.1" 200 2326"#;
        let caps = fmt.regex.captures(line).expect("should match");
        assert_eq!(&caps["ip"], "127.0.0.1");
        assert_eq!(&caps["user"], "alice");
        assert_eq!(&caps["method"], "GET");
        assert_eq!(&caps["url"], "/index.html");
        assert_eq!(&caps["status"], "200");
        assert_eq!(&caps["size"], "2326");
    }

    #[test]
    fn apache_combined_extracts_referer_and_agent() {
        let fmt = LogFormat::compile("apache-combined", BUILTINS[1].1).unwrap();
        let line = r#"10.1.2.3 - bob [10/Oct/2023:13:55:36 +0000] "POST /api/login HTTP/1.1" 401 512 "https://example.com/" "Mozilla/5.0""#;
        let caps = fmt.regex.captures(line).expect("should match");
        assert_eq!(&caps["status"], "401");
        assert_eq!(&caps["url"], "/api/login");
        assert_eq!(&caps["referer"], "https://example.com/");
        assert_eq!(&caps["agent"], "Mozilla/5.0");
    }

    #[test]
    fn field_names_listed_in_order() {
        let fmt = LogFormat::compile("apache-common", BUILTINS[0].1).unwrap();
        assert_eq!(
            fmt.field_names,
            vec!["ip", "user", "time", "method", "url", "protocol", "status", "size"]
        );
    }

    #[test]
    fn compile_rejects_regex_without_named_groups() {
        let err = LogFormat::compile("bare", r"^\d+$").unwrap_err();
        assert!(err.contains("at least one named capture"), "{err}");
    }

    #[test]
    fn compile_rejects_invalid_regex() {
        let err = LogFormat::compile("bad", r"(?P<x>[").unwrap_err();
        assert!(err.contains("bad"), "{err}");
    }

    #[test]
    fn load_groups_reads_user_config() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("formats.toml"),
            r#"
[group.errorlog]
format = "apache-combined"
file = "/var/log/access.log"
follow = true
tail = 1000
filter = ["status~^5"]

[group.minimal]
file = "/tmp/x.log"
"#,
        )
        .unwrap();
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let result = load_groups();
        if let Some(h) = saved { std::env::set_var("HOME", h); } else { std::env::remove_var("HOME"); }
        let groups = result.unwrap();
        let err = &groups["errorlog"];
        assert_eq!(err.format.as_deref(), Some("apache-combined"));
        assert_eq!(err.file.as_deref(), Some("/var/log/access.log"));
        assert!(err.follow);
        assert_eq!(err.tail, Some(1000));
        assert_eq!(err.filter, vec!["status~^5".to_string()]);
        let min = &groups["minimal"];
        assert!(!min.follow);
        assert!(min.tail.is_none());
        assert_eq!(min.filter, Vec::<String>::new());
    }

    fn group(name: &str) -> Group {
        Group { name: name.into(), ..Group::default() }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn expand_argv_passes_through_when_no_group_matches() {
        let groups: HashMap<String, Group> = HashMap::new();
        let out = expand_argv(argv(&["tess", "-f", "log.txt"]), &groups);
        assert_eq!(out, argv(&["tess", "-f", "log.txt"]));
    }

    #[test]
    fn expand_argv_inserts_group_flags_and_file() {
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert(
            "errorlog".into(),
            Group {
                name: "errorlog".into(),
                format: Some("apache-combined".into()),
                file: Some("/var/log/access.log".into()),
                follow: true,
                tail: Some(1000),
                filter: vec!["status~^5".into()],
                ..Group::default()
            },
        );
        let out = expand_argv(argv(&["tess", "--errorlog"]), &groups);
        assert_eq!(
            out,
            argv(&[
                "tess",
                "--format", "apache-combined",
                "--follow",
                "--tail", "1000",
                "--filter", "status~^5",
                "/var/log/access.log",
            ])
        );
    }

    #[test]
    fn expand_argv_converts_positionals_to_filters_after_group() {
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert(
            "errorlog".into(),
            Group {
                name: "errorlog".into(),
                format: Some("apache-combined".into()),
                file: Some("/log".into()),
                ..Group::default()
            },
        );
        let out = expand_argv(
            argv(&["tess", "--errorlog", "msg~test", "url~/api/"]),
            &groups,
        );
        assert_eq!(
            out,
            argv(&[
                "tess",
                "--format", "apache-combined",
                "/log",
                "--filter", "msg~test",
                "--filter", "url~/api/",
            ])
        );
    }

    #[test]
    fn expand_argv_leaves_flags_alone_after_group() {
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert("errorlog".into(), group("errorlog"));
        let out = expand_argv(
            argv(&["tess", "--errorlog", "--tail", "50", "msg=hi"]),
            &groups,
        );
        // Group is empty so no insertion; --tail 50 stays; "msg=hi" becomes a filter.
        assert_eq!(
            out,
            argv(&["tess", "--tail", "50", "--filter", "msg=hi"])
        );
    }

    #[test]
    fn expand_argv_user_flag_after_group_can_override_tail() {
        // Group sets tail=1000, user passes --tail 50 after; clap takes last,
        // so user's 50 wins.
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert(
            "errorlog".into(),
            Group { name: "errorlog".into(), tail: Some(1000), ..Group::default() },
        );
        let out = expand_argv(argv(&["tess", "--errorlog", "--tail", "50"]), &groups);
        // --tail 1000 from group, then --tail 50 from user. Order preserved.
        assert!(out.windows(2).any(|w| w == ["--tail", "1000"]));
        assert!(out.windows(2).any(|w| w == ["--tail", "50"]));
        let pos_1000 = out.iter().position(|x| x == "1000").unwrap();
        let pos_50 = out.iter().position(|x| x == "50").unwrap();
        assert!(pos_1000 < pos_50, "user's value must come after group's");
    }

    #[test]
    fn expand_argv_treats_grep_value_as_flag_arg_not_filter() {
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert("errorlog".into(), group("errorlog"));
        let out = expand_argv(
            argv(&["tess", "--errorlog", "--grep", "timeout", "msg=hi"]),
            &groups,
        );
        // `timeout` is --grep's value, not a positional → not converted to --filter.
        assert_eq!(
            out,
            argv(&["tess", "--grep", "timeout", "--filter", "msg=hi"])
        );
    }

    #[test]
    fn expand_argv_unknown_double_dash_passes_through() {
        let groups: HashMap<String, Group> = HashMap::new();
        let out = expand_argv(argv(&["tess", "--unknown"]), &groups);
        assert_eq!(out, argv(&["tess", "--unknown"]));
    }

    #[test]
    fn load_groups_rejects_reserved_name() {
        let _g = HOME_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("formats.toml"),
            r#"
[group.follow]
file = "/x.log"
"#,
        )
        .unwrap();
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let result = load_groups();
        if let Some(h) = saved { std::env::set_var("HOME", h); } else { std::env::remove_var("HOME"); }
        let err = result.unwrap_err();
        assert!(err.contains("collides with built-in --follow"), "{err}");
    }

    #[test]
    fn user_config_overrides_builtin_via_load_all() {
        let _g = HOME_LOCK.lock().unwrap();
        // Use a temp HOME to avoid touching the real user's config.
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        let cfg_file = cfg_dir.join("formats.toml");
        std::fs::write(
            &cfg_file,
            r#"
[format.apache-common]
regex = "^(?P<custom>\\S+)$"
"#,
        )
        .unwrap();
        // Save and replace HOME for the duration of this test.
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let result = load_all();
        if let Some(h) = saved { std::env::set_var("HOME", h); } else { std::env::remove_var("HOME"); }
        let formats = result.unwrap();
        let common = &formats["apache-common"];
        assert_eq!(common.field_names, vec!["custom"], "user config should win");
    }

    #[test]
    fn format_entry_parses_record_start() {
        let toml_text = r#"
            [format.myapp]
            regex = '^(?P<line>.*)$'
            record_start = '^\['
        "#;
        let cfg: UserConfig = toml::from_str(toml_text).expect("parse");
        let entry = cfg.format.get("myapp").expect("myapp present");
        assert_eq!(entry.regex, "^(?P<line>.*)$");
        assert_eq!(entry.record_start.as_deref(), Some("^\\["));
    }

    #[test]
    fn format_entry_record_start_optional() {
        let toml_text = r#"
            [format.myapp]
            regex = '^(?P<line>.*)$'
        "#;
        let cfg: UserConfig = toml::from_str(toml_text).expect("parse");
        let entry = cfg.format.get("myapp").expect("myapp present");
        assert!(entry.record_start.is_none());
    }

    #[test]
    fn log_format_compile_full_with_record_start() {
        let fmt = LogFormat::compile_full(
            "test",
            r"^(?P<msg>.+)$",
            None,
            Some(r"^\["),
            None,
        ).expect("compile");
        assert!(fmt.record_start.is_some());
        assert!(fmt.record_start.as_ref().unwrap().is_match("[2026-05-15"));
        assert!(!fmt.record_start.as_ref().unwrap().is_match("  continuation"));
    }

    #[test]
    fn log_format_compile_full_bad_record_start_errors() {
        let err = LogFormat::compile_full(
            "test",
            r"^(?P<msg>.+)$",
            None,
            Some(r"["),  // unclosed bracket
            None,
        ).expect_err("should fail");
        assert!(err.contains("record_start"), "error mentions record_start: {err}");
    }

    #[test]
    fn group_with_grep_field_deserializes() {
        let toml_text = r#"
            [group.errorlog]
            format = "app"
            grep = ["timeout", "deadlock"]
        "#;
        let cfg: UserConfig = toml::from_str(toml_text).expect("parse");
        let entry = cfg.group.get("errorlog").expect("errorlog present");
        assert_eq!(entry.grep, vec!["timeout".to_string(), "deadlock".to_string()]);
    }

    #[test]
    fn expand_argv_emits_group_grep_flags() {
        let mut groups = HashMap::new();
        groups.insert("errorlog".to_string(), Group {
            name: "errorlog".to_string(),
            grep: vec!["timeout".to_string(), "deadlock".to_string()],
            ..Default::default()
        });
        let out = expand_argv(
            argv(&["tess", "--errorlog", "logs.txt"]),
            &groups,
        );
        let joined = out.join(" ");
        assert!(joined.contains("--grep timeout"), "got: {joined}");
        assert!(joined.contains("--grep deadlock"), "got: {joined}");
    }

    #[test]
    fn user_grep_after_group_accumulates() {
        let mut groups = HashMap::new();
        groups.insert("errorlog".to_string(), Group {
            name: "errorlog".to_string(),
            grep: vec!["timeout".to_string()],
            ..Default::default()
        });
        let out = expand_argv(
            argv(&["tess", "--errorlog", "--grep", "extra", "logs.txt"]),
            &groups,
        );
        let joined = out.join(" ");
        assert!(joined.contains("--grep timeout"));
        assert!(joined.contains("--grep extra"));
    }

    #[test]
    fn format_entry_parses_prompt() {
        let toml_text = r#"
            [format.myapp]
            regex = '^(?P<line>.*)$'
            prompt = '<label> <pct>%'
        "#;
        let cfg: UserConfig = toml::from_str(toml_text).expect("parse");
        let entry = cfg.format.get("myapp").expect("myapp present");
        assert_eq!(entry.prompt.as_deref(), Some("<label> <pct>%"));
    }
}
