use std::collections::HashMap;
use std::path::PathBuf;

use regex::Regex;
use serde::Deserialize;

use crate::config_path;

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
    /// Optional default style for the status row when this format's prompt
    /// is active. Per-format value; CLI `--prompt-style` overrides.
    pub prompt_style: Option<crate::ansi::Style>,
    pub(crate) source: crate::config_path::ConfigSource,
    pub(crate) overrides: Option<crate::config_path::ConfigSource>,
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
            prompt_style: None,
            source: crate::config_path::ConfigSource::Builtin,
            overrides: None,
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
                    Some('n') => buf.push('\n'),
                    Some('t') => buf.push('\t'),
                    Some('r') => buf.push('\r'),
                    Some('e') => buf.push('\x1b'),
                    Some('x') => {
                        let h1 = chars.next().ok_or_else(|| "incomplete `\\xHH` escape".to_string())?;
                        let h2 = chars.next().ok_or_else(|| "incomplete `\\xHH` escape".to_string())?;
                        let hex: String = [h1, h2].iter().collect();
                        let byte = u8::from_str_radix(&hex, 16)
                            .map_err(|_| format!("invalid `\\x{hex}` escape"))?;
                        buf.push(byte as char);
                    }
                    Some('0') => {
                        let d1 = chars.next().ok_or_else(|| "incomplete `\\NNN` escape".to_string())?;
                        let d2 = chars.next().ok_or_else(|| "incomplete `\\NNN` escape".to_string())?;
                        let oct: String = ['0', d1, d2].iter().collect();
                        let byte = u8::from_str_radix(&oct, 8)
                            .map_err(|_| format!("invalid `\\{oct}` escape"))?;
                        buf.push(byte as char);
                    }
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

    /// Render `line` (raw bytes) through the template, decoded via `enc`. If
    /// the line doesn't parse against the format regex, returns `None` — the
    /// caller decides whether to fall back to the raw line, skip it, or show
    /// an error.
    pub fn render_line(&self, line: &[u8], enc: crate::charset::Encoding) -> Option<String> {
        let s = crate::charset::decode_line(line, enc);
        let caps = self.regex.captures(&s)?;
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
#[derive(Debug, Default, Deserialize)]
struct UserConfig {
    #[serde(default)]
    format: HashMap<String, FormatEntry>,
    #[serde(default)]
    group: HashMap<String, GroupEntry>,
    #[serde(default)]
    layout: HashMap<String, LayoutEntry>,
}

/// Raw layout entry as deserialized from TOML. A `[layout.NAME]` carries an
/// `orientation` plus an ordered array of `[[layout.NAME.pane]]` tables, each
/// reusing the `GroupEntry` shape. Promoted to `Layout` after validation.
#[derive(Debug, Deserialize, Default)]
struct LayoutEntry {
    orientation: Option<String>,
    #[serde(default)]
    pane: Vec<GroupEntry>,
}

/// A saved split arrangement: an orientation plus an ordered list of pane
/// view-specs (each a promoted `Group`).
#[derive(Debug, Clone)]
pub struct Layout {
    pub name: String,
    pub horizontal: bool,
    pub panes: Vec<Group>,
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
    /// Optional style for the status row when this format is active and a
    /// custom prompt is rendered. Parsed via `crate::style_spec`. CLI
    /// `--prompt-style` overrides this.
    #[serde(default)]
    prompt_style: Option<String>,
}

/// Named OR-sub-group inside a `[group.X.or.<name>]` table. Inside the `or`
/// namespace the keys are bare `filter`/`grep` because the surrounding table
/// already marks them as OR-conditions.
#[derive(Debug, Deserialize, Default, Clone)]
struct OrSubGroup {
    #[serde(default)]
    filter: Vec<String>,
    #[serde(default)]
    grep: Vec<String>,
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
    display: Option<String>,
    #[serde(default)]
    filter: Vec<String>,
    #[serde(default)]
    grep: Vec<String>,
    #[serde(default)]
    or_filter: Vec<String>,
    #[serde(default)]
    or_grep: Vec<String>,
    #[serde(default)]
    or: std::collections::HashMap<String, OrSubGroup>,
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
    /// Default `--display` template for this group. Emitted as `--display
    /// <value>` at expansion time; a later CLI `--display` overrides it
    /// (clap takes the last occurrence). Requires the group (or CLI) to also
    /// set a `--format`, same as the bare `--display` flag.
    pub display: Option<String>,
    pub filter: Vec<String>,
    pub grep: Vec<String>,
    /// Default OR-group conditions (no name). Emitted as bare --or-filter /
    /// --or-grep (default group) at expansion time.
    pub or_filter: Vec<String>,
    pub or_grep: Vec<String>,
    /// Named OR-groups: (name, filter specs, grep patterns). Emitted as
    /// `--or-group <name>` followed by that group's conditions.
    pub or_named: Vec<(String, Vec<String>, Vec<String>)>,
    // Populated by the layered loader to track which config layer a group came
    // from (and what it overrode); reserved for group source annotation in
    // `--list-formats`. Not yet read, hence the allow.
    #[allow(dead_code)]
    pub(crate) source: crate::config_path::ConfigSource,
    #[allow(dead_code)]
    pub(crate) overrides: Option<crate::config_path::ConfigSource>,
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
    "hex",
    "prompt",
    "display",
    "or-filter",
    "or-grep",
    "or-group",
    "preprocess",
    "no-preprocess",
    "no-color",
    "raw-control-chars",
    "tag",
    "tag-file",
    "split",
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

fn formats_path_in(dir: &std::path::Path) -> PathBuf {
    dir.join("formats.toml")
}

/// Parsed contents of both global and local `formats.toml`. Empty
/// `UserConfig` represents "layer absent or unreadable".
#[derive(Debug, Default)]
struct LayeredConfig {
    global: UserConfig,
    local: UserConfig,
}

fn read_formats_toml(path: &std::path::Path) -> Result<UserConfig, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    toml::from_str(&text)
        .map_err(|e| format!("parsing {}: {e}", path.display()))
}

fn load_layered_config() -> Result<LayeredConfig, String> {
    let mut layered = LayeredConfig::default();

    // Global: warn-and-continue on parse error.
    if let Some(dir) = config_path::global_config_dir() {
        let path = formats_path_in(&dir);
        if path.exists() {
            match read_formats_toml(&path) {
                Ok(cfg) => layered.global = cfg,
                Err(e) => eprintln!(
                    "tess: warning: {e}; ignoring global config"
                ),
            }
        }
    }

    // Local: fail-startup on parse error (unchanged behavior).
    if let Some(dir) = config_path::user_config_dir() {
        let path = formats_path_in(&dir);
        if path.exists() {
            layered.local = read_formats_toml(&path)?;
        }
    }

    Ok(layered)
}

struct FormatSource {
    regex: String,
    display: Option<String>,
    record_start: Option<String>,
    prompt: Option<String>,
    prompt_style: Option<String>,
    source: crate::config_path::ConfigSource,
    overrides: Option<crate::config_path::ConfigSource>,
}

fn load_user_formats() -> Result<HashMap<String, FormatSource>, String> {
    let cfg = load_layered_config()?;
    let mut out: HashMap<String, FormatSource> = HashMap::new();
    for (k, v) in cfg.global.format {
        out.insert(k, FormatSource {
            regex: v.regex,
            display: v.display,
            record_start: v.record_start,
            prompt: v.prompt,
            prompt_style: v.prompt_style,
            source: crate::config_path::ConfigSource::Global,
            overrides: None,
        });
    }
    for (k, v) in cfg.local.format {
        let overrides = out.get(&k).map(|prev| prev.source);
        out.insert(k, FormatSource {
            regex: v.regex,
            display: v.display,
            record_start: v.record_start,
            prompt: v.prompt,
            prompt_style: v.prompt_style,
            source: crate::config_path::ConfigSource::Local,
            overrides,
        });
    }
    Ok(out)
}

/// Load all user-defined groups from global and local `formats.toml`. Built-ins
/// are not provided — groups are entirely user-defined. Validates that group
/// names don't shadow built-in flag names.
pub fn load_groups() -> Result<HashMap<String, Group>, String> {
    let cfg = load_layered_config()?;

    struct StagedGroup {
        entry: GroupEntry,
        source: crate::config_path::ConfigSource,
        overrides: Option<crate::config_path::ConfigSource>,
    }

    let mut staged: HashMap<String, StagedGroup> = HashMap::new();
    for (k, v) in cfg.global.group {
        staged.insert(k, StagedGroup {
            entry: v,
            source: crate::config_path::ConfigSource::Global,
            overrides: None,
        });
    }
    for (k, v) in cfg.local.group {
        let overrides = staged.get(&k).map(|prev| prev.source);
        staged.insert(k, StagedGroup {
            entry: v,
            source: crate::config_path::ConfigSource::Local,
            overrides,
        });
    }

    let mut out = HashMap::with_capacity(staged.len());
    for (name, sg) in staged {
        if RESERVED_LONG_FLAGS.contains(&name.as_str()) {
            return Err(format!(
                "group `{name}`: name collides with built-in --{name} flag"
            ));
        }
        let mut group = promote_group(name.clone(), sg.entry);
        group.source = sg.source;
        group.overrides = sg.overrides;
        out.insert(name, group);
    }
    Ok(out)
}

/// Load all user-defined layouts from global and local `formats.toml`. Local
/// layouts override global ones of the same name. Validates orientation, pane
/// presence, and that each pane names a `file`; rejects names that collide with
/// built-in flags. Each pane is promoted to a `Group` via `promote_group`.
pub fn load_layouts() -> Result<HashMap<String, Layout>, String> {
    let cfg = load_layered_config()?;

    let mut staged: HashMap<String, LayoutEntry> = HashMap::new();
    for (k, v) in cfg.global.layout {
        staged.insert(k, v);
    }
    for (k, v) in cfg.local.layout {
        staged.insert(k, v);
    }

    let mut out = HashMap::with_capacity(staged.len());
    for (name, entry) in staged {
        if RESERVED_LONG_FLAGS.contains(&name.as_str()) {
            return Err(format!(
                "layout `{name}`: name collides with built-in --{name} flag"
            ));
        }
        let horizontal = match entry.orientation.as_deref() {
            None | Some("vertical") => false,
            Some("horizontal") => true,
            Some(other) => {
                return Err(format!(
                    "layout `{name}`: bad orientation `{other}` (expected vertical or horizontal)"
                ))
            }
        };
        if entry.pane.is_empty() {
            return Err(format!("layout `{name}`: needs at least one pane"));
        }
        let mut panes = Vec::with_capacity(entry.pane.len());
        for (i, pane) in entry.pane.into_iter().enumerate() {
            if pane.file.is_none() {
                return Err(format!("layout `{name}` pane {i}: missing `file`"));
            }
            panes.push(promote_group(format!("{name}.pane{i}"), pane));
        }
        out.insert(name.clone(), Layout { name, horizontal, panes });
    }
    Ok(out)
}

/// Promote a raw `GroupEntry` into a `Group`: assign the name, unwrap the
/// `Option<bool>` defaults, and flatten the named OR-sub-groups into a sorted
/// `or_named` vec. The `source`/`overrides` provenance fields are left at their
/// `Default` and must be set by the caller when known.
fn promote_group(name: String, entry: GroupEntry) -> Group {
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
        display: entry.display,
        filter: entry.filter,
        grep: entry.grep,
        or_filter: entry.or_filter,
        or_grep: entry.or_grep,
        or_named: {
            let mut v: Vec<(String, Vec<String>, Vec<String>)> = entry
                .or
                .into_iter()
                .map(|(name, sub)| (name, sub.filter, sub.grep))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic emission order
            v
        },
        ..Group::default()
    }
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
            prompt_style: None,
            source: crate::config_path::ConfigSource::Builtin,
            overrides: None,
        });
    }
    let user = load_user_formats()?;
    for (name, mut src) in user {
        // load_user_formats doesn't know about built-ins, so we detect
        // direct built-in shadowing here. If `src.overrides` is already
        // set, local was shadowing global — leave that alone.
        if src.overrides.is_none() && sources.contains_key(&name) {
            src.overrides = Some(crate::config_path::ConfigSource::Builtin);
        }
        sources.insert(name, src);
    }
    let mut compiled = HashMap::new();
    for (name, src) in sources {
        let mut fmt = LogFormat::compile_full(
            &name,
            &src.regex,
            src.display.as_deref(),
            src.record_start.as_deref(),
            src.prompt.as_deref(),
        )?;
        if let Some(spec) = src.prompt_style.as_deref() {
            fmt.prompt_style = Some(
                crate::style_spec::parse(spec)
                    .map_err(|e| format!("format `{name}`: prompt_style: {e}"))?,
            );
        }
        fmt.source = src.source;
        fmt.overrides = src.overrides;
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
/// doesn't mistake a flag's value for a positional in filter mode — without
/// this, `--errs --display '<msg>'` would rewrite the template into a
/// `--filter`. Must list *every* value-taking long flag clap defines.
const VALUE_TAKING_LONG_FLAGS: &[&str] = &[
    "--content-type",
    "--display",
    "--filter",
    "--format",
    "--grep",
    "--head",
    "--header",
    "--hex-group",
    "--image-width",
    "--or-filter",
    "--or-grep",
    "--or-group",
    "--output",
    "--preprocess",
    "--prompt",
    "--prompt-style",
    "--record-start",
    "--rscroll",
    "--status-style",
    "--tab-width",
    "--tag",
    "--tag-file",
    "--tail",
    "--truecolor",
    "--window",
];

/// Short flags that take a separate value as the next argv token (`-o FILE`,
/// `-z N`, `-t NAME`, `-T PATH`). The boolean short flags (`-N`, `-S`, `-f`,
/// …) are intentionally absent — they must not swallow the following token.
/// The attached form (`-ovalue`) is a single token and needs no entry here.
const VALUE_TAKING_SHORT_FLAGS: &[&str] = &[
    "-o",
    "-z",
    "-t",
    "-T",
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
            }
        }
        // A value-taking flag (long or short, separated form): emit it and
        // pass its value through untouched, even in filter mode, so the value
        // isn't mistaken for a positional and rewritten into a `--filter`.
        // The `--flag=value` / `-ovalue` attached forms are single tokens and
        // fall through harmlessly (they start with `-`, so aren't converted).
        if VALUE_TAKING_LONG_FLAGS.contains(&arg.as_str())
            || VALUE_TAKING_SHORT_FLAGS.contains(&arg.as_str())
        {
            out.push(arg);
            pass_next = true;
            continue;
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

/// If a `--<layoutname>` token is present (a name found in `layouts`), replace it
/// in place with the layout's panes rendered as `--`-form sections: for each pane,
/// its group-style flags (via `expand_group`) followed by its `file` positional,
/// with `--` separators between panes. Tokens before the layout token (program
/// name + globals) stay ahead of section 0; tokens after it are appended after the
/// last section. Returns `(rewritten_argv, Some(horizontal))`. No layout token →
/// `(argv, None)`.
pub fn expand_layout_argv(argv: Vec<String>, layouts: &std::collections::HashMap<String, Layout>)
    -> (Vec<String>, Option<bool>)
{
    for i in 1..argv.len() {
        if let Some(name) = argv[i].strip_prefix("--") {
            if let Some(layout) = layouts.get(name) {
                let mut out: Vec<String> = argv[..i].to_vec();
                for (p, pane) in layout.panes.iter().enumerate() {
                    if p > 0 {
                        out.push("--".into());
                    }
                    expand_group(pane, &mut out);
                }
                out.extend_from_slice(&argv[i + 1..]);
                return (out, Some(layout.horizontal));
            }
        }
    }
    (argv, None)
}

/// Public wrapper over `expand_group` for reuse by the runtime `:layout` command.
/// Emits the group's view flags followed by its `file` positional into `out`.
pub fn expand_group_tokens(g: &Group, out: &mut Vec<String>) {
    expand_group(g, out)
}

fn expand_group(g: &Group, out: &mut Vec<String>) {
    if let Some(format) = &g.format {
        out.push("--format".into());
        out.push(format.clone());
    }
    if let Some(display) = &g.display {
        out.push("--display".into());
        out.push(display.clone());
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
    // Default OR-group (unlabeled): no --or-group marker.
    for f in &g.or_filter {
        out.push("--or-filter".into());
        out.push(f.clone());
    }
    for p in &g.or_grep {
        out.push("--or-grep".into());
        out.push(p.clone());
    }
    // Named OR-groups (already sorted by name in load_groups for determinism).
    for (name, filters, greps) in &g.or_named {
        out.push("--or-group".into());
        out.push(name.clone());
        for f in filters {
            out.push("--or-filter".into());
            out.push(f.clone());
        }
        for p in greps {
            out.push("--or-grep".into());
            out.push(p.clone());
        }
    }
    if let Some(file) = &g.file {
        out.push(file.clone());
    }
}

/// Render the bracketed source annotation for a format. The `overrides`
/// argument is the immediately-replaced layer produced by `load_all`.
fn format_source_label(
    source: crate::config_path::ConfigSource,
    overrides: Option<crate::config_path::ConfigSource>,
) -> String {
    use crate::config_path::ConfigSource::*;
    let layer = match source {
        Builtin => "built-in",
        Global => "global",
        Local => "local",
    };
    match overrides {
        None => format!("[{layer}]"),
        Some(Builtin) => format!("[{layer}, overrides built-in]"),
        Some(Global) => format!("[{layer}, overrides global]"),
        // Lower layers can't replace local; this arm is unreachable in
        // practice but kept for total-match completeness.
        Some(Local) => format!("[{layer}, overrides local]"),
    }
}

/// Print one line per format, with the named field list and source
/// label, to stdout. Used by `--list-formats`.
pub fn print_format_list(formats: &HashMap<String, LogFormat>) {
    let mut names: Vec<&String> = formats.keys().collect();
    names.sort();

    // Column-align names for readability when field lists vary.
    let name_width = names.iter().map(|n| n.len()).max().unwrap_or(0);

    for name in names {
        let fmt = &formats[name];
        let fields: Vec<&str> = fmt.field_names.iter().map(|s| s.as_str()).collect();
        let label = format_source_label(fmt.source, fmt.overrides);
        println!(
            "{:<width$}  {}  {}",
            name,
            label,
            fields.join(", "),
            width = name_width
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn display_template_escape_e_emits_esc() {
        let t = DisplayTemplate::compile(r"\e[31m<level>\e[0m", &fields()).unwrap();
        let mut map = std::collections::HashMap::new();
        map.insert("level".to_string(), "X".to_string());
        let out = t.render(|n| map.get(n).cloned());
        assert_eq!(out, "\x1b[31mX\x1b[0m");
    }

    #[test]
    fn display_template_escape_x1b_emits_esc() {
        let t = DisplayTemplate::compile(r"\x1b[1m<level>", &fields()).unwrap();
        let out = t.render(|_| Some("Y".to_string()));
        assert_eq!(out, "\x1b[1mY");
    }

    #[test]
    fn display_template_escape_octal_emits_esc() {
        let t = DisplayTemplate::compile(r"\033[1m<level>", &fields()).unwrap();
        let out = t.render(|_| Some("Z".to_string()));
        assert_eq!(out, "\x1b[1mZ");
    }

    #[test]
    fn display_template_escape_n_t_r() {
        let t = DisplayTemplate::compile(r"\n\t\r<level>", &fields()).unwrap();
        let out = t.render(|_| Some("Q".to_string()));
        assert_eq!(out, "\n\t\rQ");
    }

    #[test]
    fn display_template_escape_unknown_preserves_backslash() {
        let t = DisplayTemplate::compile(r"\q<level>", &fields()).unwrap();
        let out = t.render(|_| Some("Q".to_string()));
        assert_eq!(out, r"\qQ");
    }

    #[test]
    fn display_template_escape_x_incomplete_errors() {
        let err = DisplayTemplate::compile(r"\x1", &fields()).unwrap_err();
        assert!(err.contains("incomplete"), "{err}");
    }

    #[test]
    fn display_template_escape_invalid_hex_errors() {
        let err = DisplayTemplate::compile(r"\xZZ", &fields()).unwrap_err();
        assert!(err.contains("invalid"), "{err}");
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
        let _g = crate::test_env::lock();
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
display = "<status> <url>"

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
        assert_eq!(err.display.as_deref(), Some("<status> <url>"));
        let min = &groups["minimal"];
        assert!(!min.follow);
        assert!(min.tail.is_none());
        assert_eq!(min.filter, Vec::<String>::new());
        assert!(min.display.is_none());
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
    fn expand_argv_passes_display_template_through_after_group() {
        // Regression: a `--display` template after a group must NOT be
        // rewritten into a `--filter` (which previously left `--display`
        // value-less and clap erroring "a value is required").
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert("errorlog".into(), group("errorlog"));
        let out = expand_argv(
            argv(&["tess", "--errorlog", "--display", "<lvl>: <msg>", "lvl=ERROR"]),
            &groups,
        );
        assert_eq!(
            out,
            argv(&[
                "tess",
                "--display", "<lvl>: <msg>",
                "--filter", "lvl=ERROR",
            ])
        );
    }

    #[test]
    fn expand_argv_passes_short_value_flag_through_after_group() {
        // `-o FILE` (and the other separated short value flags) must keep
        // their value instead of converting it to a filter in filter mode.
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert("errorlog".into(), group("errorlog"));
        let out = expand_argv(
            argv(&["tess", "--errorlog", "-o", "out.txt", "lvl=ERROR"]),
            &groups,
        );
        assert_eq!(
            out,
            argv(&["tess", "-o", "out.txt", "--filter", "lvl=ERROR"])
        );
    }

    #[test]
    fn expand_group_emits_display_when_set() {
        let g = Group {
            name: "errs".into(),
            format: Some("simple".into()),
            display: Some("<lvl>!! <msg>".into()),
            filter: vec!["lvl=ERROR".into()],
            ..Group::default()
        };
        let out = expand_argv(argv(&["tess", "--errs"]), &{
            let mut m = HashMap::new();
            m.insert("errs".to_string(), g);
            m
        });
        assert_eq!(
            out,
            argv(&[
                "tess",
                "--format", "simple",
                "--display", "<lvl>!! <msg>",
                "--filter", "lvl=ERROR",
            ])
        );
    }

    #[test]
    fn expand_argv_cli_display_overrides_group_display() {
        // Group sets a display; a later CLI `--display` is emitted after it,
        // so clap's last-occurrence wins (the CLI value).
        let g = Group {
            name: "errs".into(),
            format: Some("simple".into()),
            display: Some("group-tmpl".into()),
            ..Group::default()
        };
        let out = expand_argv(argv(&["tess", "--errs", "--display", "cli-tmpl"]), &{
            let mut m = HashMap::new();
            m.insert("errs".to_string(), g);
            m
        });
        let pos_group = out.iter().position(|x| x == "group-tmpl").unwrap();
        let pos_cli = out.iter().position(|x| x == "cli-tmpl").unwrap();
        assert!(pos_group < pos_cli, "CLI display must come after group's so it wins");
    }

    #[test]
    fn load_groups_rejects_reserved_name() {
        let _g = crate::test_env::lock();
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

    /// Write `toml` as the local `formats.toml` under a temp HOME, run
    /// `load_layouts()`, and restore HOME. Mirrors the `load_groups_*` harness.
    fn load_layouts_with(toml: &str) -> Result<HashMap<String, Layout>, String> {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(cfg_dir.join("formats.toml"), toml).unwrap();
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let result = load_layouts();
        if let Some(h) = saved { std::env::set_var("HOME", h); } else { std::env::remove_var("HOME"); }
        result
    }

    #[test]
    fn load_layouts_parses_panes_and_orientation() {
        let _g = crate::test_env::lock();
        let layouts = load_layouts_with(
            r#"
[layout.dash]
orientation = "horizontal"
[[layout.dash.pane]]
file = "a.log"
format = "myapp"
filter = ["x=1"]
[[layout.dash.pane]]
file = "b.log"
grep = ["5.."]
"#,
        )
        .unwrap();
        let dash = &layouts["dash"];
        assert!(dash.horizontal);
        assert_eq!(dash.panes.len(), 2);
        assert_eq!(dash.panes[0].file.as_deref(), Some("a.log"));
        assert_eq!(dash.panes[0].format.as_deref(), Some("myapp"));
        assert_eq!(dash.panes[0].filter, vec!["x=1".to_string()]);
        assert_eq!(dash.panes[1].file.as_deref(), Some("b.log"));
        assert_eq!(dash.panes[1].grep, vec!["5..".to_string()]);
    }

    #[test]
    fn load_layouts_defaults_orientation_vertical() {
        let _g = crate::test_env::lock();
        let layouts = load_layouts_with(
            r#"
[layout.dash]
[[layout.dash.pane]]
file = "a.log"
"#,
        )
        .unwrap();
        assert!(!layouts["dash"].horizontal);
    }

    #[test]
    fn load_layouts_rejects_pane_without_file() {
        let _g = crate::test_env::lock();
        let result = load_layouts_with(
            r#"
[layout.dash]
[[layout.dash.pane]]
format = "myapp"
"#,
        );
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn load_layouts_rejects_reserved_name() {
        let _g = crate::test_env::lock();
        let result = load_layouts_with(
            r#"
[layout.split]
[[layout.split.pane]]
file = "a.log"
"#,
        );
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn load_layouts_rejects_bad_orientation() {
        let _g = crate::test_env::lock();
        let result = load_layouts_with(
            r#"
[layout.dash]
orientation = "diagonal"
[[layout.dash.pane]]
file = "a.log"
"#,
        );
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn load_layouts_rejects_empty() {
        let _g = crate::test_env::lock();
        let result = load_layouts_with(
            r#"
[layout.dash]
orientation = "vertical"
"#,
        );
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn user_config_overrides_builtin_via_load_all() {
        let _g = crate::test_env::lock();
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
    fn layered_loader_local_overrides_global() {
        let _guard = crate::test_env::lock();
        let prev_home = std::env::var_os("HOME");
        let prev_global = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");

        let home = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();

        std::env::set_var("HOME", home.path());
        std::env::set_var("TESS_GLOBAL_CONFIG_DIR", global.path());

        std::fs::write(
            global.path().join("formats.toml"),
            r#"
[format.shared]
regex = "^GLOBAL (?P<msg>.+)$"

[format.both]
regex = "^GLOBAL_BOTH (?P<msg>.+)$"
"#,
        )
        .unwrap();

        let cfg_dir = home.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("formats.toml"),
            r#"
[format.both]
regex = "^LOCAL_BOTH (?P<msg>.+)$"

[format.local-only]
regex = "^LOCAL (?P<msg>.+)$"
"#,
        )
        .unwrap();

        let cfg = load_layered_config().unwrap();

        // Global-only format survives.
        assert!(cfg.global.format.contains_key("shared"));
        assert!(!cfg.local.format.contains_key("shared"));

        // Same-name format: both layers carry it, merge step (next task)
        // is responsible for resolving. Here we just verify both files
        // parsed correctly.
        assert_eq!(
            cfg.global.format.get("both").unwrap().regex,
            "^GLOBAL_BOTH (?P<msg>.+)$"
        );
        assert_eq!(
            cfg.local.format.get("both").unwrap().regex,
            "^LOCAL_BOTH (?P<msg>.+)$"
        );

        // Local-only format present.
        assert!(cfg.local.format.contains_key("local-only"));

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_global {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }

    #[test]
    fn layered_loader_warns_on_bad_global_toml() {
        let _guard = crate::test_env::lock();
        let prev_home = std::env::var_os("HOME");
        let prev_global = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");

        let home = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();

        std::env::set_var("HOME", home.path());
        std::env::set_var("TESS_GLOBAL_CONFIG_DIR", global.path());

        std::fs::write(
            global.path().join("formats.toml"),
            "this is not valid toml = = =",
        )
        .unwrap();

        // Should NOT error — global parse failures are warnings, not errors.
        let cfg = load_layered_config().unwrap();
        assert!(cfg.global.format.is_empty());
        assert!(cfg.global.group.is_empty());

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_global {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }

    #[test]
    fn layered_loader_fails_on_bad_local_toml() {
        let _guard = crate::test_env::lock();
        let prev_home = std::env::var_os("HOME");
        let prev_global = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");

        let home = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", home.path());
        std::env::remove_var("TESS_GLOBAL_CONFIG_DIR");

        let cfg_dir = home.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("formats.toml"),
            "this is not valid toml = = =",
        )
        .unwrap();

        let err = load_layered_config().unwrap_err();
        assert!(err.contains("formats.toml"), "got: {err}");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_global {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
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

    #[test]
    fn load_all_tags_source_correctly() {
        let _guard = crate::test_env::lock();
        let prev_home = std::env::var_os("HOME");
        let prev_global = std::env::var_os("TESS_GLOBAL_CONFIG_DIR");

        let home = tempfile::tempdir().unwrap();
        let global = tempfile::tempdir().unwrap();

        std::env::set_var("HOME", home.path());
        std::env::set_var("TESS_GLOBAL_CONFIG_DIR", global.path());

        std::fs::write(
            global.path().join("formats.toml"),
            r#"
[format.global-only]
regex = "^G (?P<msg>.+)$"

[format.both]
regex = "^GLOBAL (?P<msg>.+)$"
"#,
        )
        .unwrap();

        let cfg_dir = home.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("formats.toml"),
            r#"
[format.local-only]
regex = "^L (?P<msg>.+)$"

[format.both]
regex = "^LOCAL (?P<msg>.+)$"
"#,
        )
        .unwrap();

        let all = load_all().unwrap();

        // Built-in still tagged builtin.
        assert_eq!(
            all["apache-common"].source,
            crate::config_path::ConfigSource::Builtin
        );
        assert!(all["apache-common"].overrides.is_none());

        // Global-only.
        assert_eq!(
            all["global-only"].source,
            crate::config_path::ConfigSource::Global
        );
        assert!(all["global-only"].overrides.is_none());

        // Local-only.
        assert_eq!(
            all["local-only"].source,
            crate::config_path::ConfigSource::Local
        );
        assert!(all["local-only"].overrides.is_none());

        // Same-name: local wins, marked as overriding global.
        assert_eq!(
            all["both"].source,
            crate::config_path::ConfigSource::Local
        );
        assert_eq!(
            all["both"].overrides,
            Some(crate::config_path::ConfigSource::Global)
        );

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match prev_global {
            Some(v) => std::env::set_var("TESS_GLOBAL_CONFIG_DIR", v),
            None => std::env::remove_var("TESS_GLOBAL_CONFIG_DIR"),
        }
    }

    #[test]
    fn display_renderer_decoded_latin1() {
        // A format with a `msg` field; the display template echoes it.
        let fmt = LogFormat::compile_with_display(
            "simple3",
            r"^(?P<msg>.+)$",
            Some("<msg>"),
        )
        .unwrap();
        let renderer = DisplayRenderer::new(
            fmt.display.unwrap(),
            fmt.regex,
        );
        let l1 = crate::charset::parse_label("iso-8859-1").unwrap();
        // "café" in Latin-1 bytes
        let latin1_line: &[u8] = b"caf\xE9";
        // With Latin-1 decoding, "café" → regex matches, msg = "café"
        assert_eq!(
            renderer.render_line(latin1_line, l1).as_deref(),
            Some("café")
        );
        // With UTF-8 decoding: 0xE9 alone is invalid UTF-8 → lossy → U+FFFD present
        // The lossy string won't equal "café", but it WILL match the `.+` pattern → Some(replacement)
        // We just check it doesn't equal "café":
        let utf8_result = renderer.render_line(latin1_line, crate::charset::Encoding::utf8());
        assert!(utf8_result.is_some());
        assert_ne!(utf8_result.as_deref(), Some("café"));
    }

    #[test]
    fn source_label_renders_correctly() {
        use crate::config_path::ConfigSource;
        assert_eq!(format_source_label(ConfigSource::Builtin, None), "[built-in]");
        assert_eq!(format_source_label(ConfigSource::Global, None), "[global]");
        assert_eq!(format_source_label(ConfigSource::Local, None), "[local]");
        assert_eq!(
            format_source_label(ConfigSource::Local, Some(ConfigSource::Global)),
            "[local, overrides global]"
        );
        assert_eq!(
            format_source_label(ConfigSource::Local, Some(ConfigSource::Builtin)),
            "[local, overrides built-in]"
        );
        assert_eq!(
            format_source_label(ConfigSource::Global, Some(ConfigSource::Builtin)),
            "[global, overrides built-in]"
        );
    }

    #[test]
    fn load_groups_reads_or_conditions() {
        let _g = crate::test_env::lock();
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join(".config").join("tess");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("formats.toml"),
            r#"
[group.intrusion]
format = "app"
or_filter = ["lvl=ERROR"]
or_grep = ["panic"]

[group.intrusion.or.svc]
filter = ["status=403"]
grep = ["ssh", "sshd"]
"#,
        )
        .unwrap();
        let saved = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        let result = load_groups();
        if let Some(h) = saved { std::env::set_var("HOME", h); } else { std::env::remove_var("HOME"); }
        let groups = result.unwrap();
        let g = &groups["intrusion"];
        assert_eq!(g.or_filter, vec!["lvl=ERROR".to_string()]);
        assert_eq!(g.or_grep, vec!["panic".to_string()]);
        assert_eq!(g.or_named, vec![("svc".to_string(), vec!["status=403".to_string()], vec!["ssh".to_string(), "sshd".to_string()])]);
    }

    #[test]
    fn expand_layout_argv_expands_token_to_sections() {
        let mut layouts = std::collections::HashMap::new();
        layouts.insert("dash".to_string(), Layout {
            name: "dash".into(),
            horizontal: true,
            panes: vec![
                Group { name: "p0".into(), file: Some("a.log".into()), format: Some("myapp".into()),
                        filter: vec!["x=1".into()], ..Default::default() },
                Group { name: "p1".into(), file: Some("b.log".into()),
                        grep: vec!["5..".into()], ..Default::default() },
            ],
        });
        let argv: Vec<String> = ["tess", "--mouse", "--dash"].iter().map(|s| s.to_string()).collect();
        let (out, horiz) = expand_layout_argv(argv, &layouts);
        assert_eq!(horiz, Some(true));
        let expected: Vec<String> = ["tess", "--mouse", "--format", "myapp", "--filter", "x=1", "a.log",
            "--", "--grep", "5..", "b.log"].iter().map(|s| s.to_string()).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn expand_layout_argv_noop_without_layout_token() {
        let layouts: std::collections::HashMap<String, Layout> = std::collections::HashMap::new();
        let argv: Vec<String> = ["tess", "a.log"].iter().map(|s| s.to_string()).collect();
        let (out, horiz) = expand_layout_argv(argv.clone(), &layouts);
        assert_eq!(out, argv);
        assert_eq!(horiz, None);
    }

    #[test]
    fn expand_group_emits_or_conditions_in_marker_form() {
        let mut groups: HashMap<String, Group> = HashMap::new();
        groups.insert(
            "intrusion".into(),
            Group {
                name: "intrusion".into(),
                format: Some("app".into()),
                or_grep: vec!["panic".into()],
                or_named: vec![("svc".into(), vec!["status=403".into()], vec!["ssh".into()])],
                ..Group::default()
            },
        );
        let out = expand_argv(argv(&["tess", "--intrusion"]), &groups);
        assert_eq!(
            out,
            argv(&[
                "tess",
                "--format", "app",
                "--or-grep", "panic",
                "--or-group", "svc",
                "--or-filter", "status=403",
                "--or-grep", "ssh",
            ])
        );
    }
}
