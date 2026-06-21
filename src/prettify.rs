//! Content-type detection and pretty-printing for structured data.
//!
//! Used by `--prettify` and `--content-type` to lay out JSON, YAML, TOML,
//! XML, HTML, and CSV inputs in a readable form. The transformation runs once
//! at startup (or on toggle) and produces a fresh byte buffer that the line
//! index treats as the new source content. No syntax highlighting / color —
//! layout only — so search and filter stay byte-clean.

use std::path::Path;

use quick_xml::events::Event;
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use std::io::Cursor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrettifyMode {
    Off,
    Json,
    Yaml,
    Toml,
    Xml,
    Html,
    Csv,
}

impl PrettifyMode {
    /// Status-line label, e.g. `"json"`. Empty when off.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Csv => "csv",
        }
    }

    pub fn is_active(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// Result of resolving the user's content-type intent against the available
/// signals (explicit flag → extension → byte sniff → raw fallback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedType {
    Mode(PrettifyMode),
    /// Auto-detect was requested but nothing matched. Caller should warn and
    /// fall through to `Off`.
    Undetected,
}

/// Parse a `--content-type=NAME` value. Case-insensitive. `auto` returns
/// `None` (caller should run detection); `raw` maps to `Off`.
pub fn parse_content_type(name: &str) -> Result<Option<PrettifyMode>, String> {
    let lc = name.trim().to_ascii_lowercase();
    let mode = match lc.as_str() {
        "auto" => return Ok(None),
        "raw" | "off" | "none" => PrettifyMode::Off,
        "json" => PrettifyMode::Json,
        "yaml" | "yml" => PrettifyMode::Yaml,
        "toml" => PrettifyMode::Toml,
        "xml" => PrettifyMode::Xml,
        "html" | "htm" => PrettifyMode::Html,
        "csv" => PrettifyMode::Csv,
        other => {
            return Err(format!(
                "unknown content type `{other}` (try one of: \
auto, raw, json, yaml, toml, xml, html, csv)"
            ));
        }
    };
    Ok(Some(mode))
}

/// Detect from filename extension. Returns `None` if nothing matches.
pub fn detect_from_path(path: &Path) -> Option<PrettifyMode> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "json" => PrettifyMode::Json,
        "yaml" | "yml" => PrettifyMode::Yaml,
        "toml" => PrettifyMode::Toml,
        "xml" => PrettifyMode::Xml,
        "html" | "htm" => PrettifyMode::Html,
        "csv" => PrettifyMode::Csv,
        _ => return None,
    })
}

/// Detect from leading bytes. Returns `None` if nothing matches. Cheap;
/// inspects up to ~512 bytes.
pub fn detect_from_bytes(bytes: &[u8]) -> Option<PrettifyMode> {
    let head_len = bytes.len().min(512);
    let head = &bytes[..head_len];
    // Skip leading whitespace.
    let trimmed_start = head.iter().position(|b| !b.is_ascii_whitespace())?;
    let trimmed = &head[trimmed_start..];
    if trimmed.is_empty() {
        return None;
    }
    // XML declaration.
    if trimmed.starts_with(b"<?xml") {
        return Some(PrettifyMode::Xml);
    }
    // HTML doctype or root element. Lowercase comparison on the first ~200 bytes.
    let head_lc: Vec<u8> = trimmed.iter().take(200).map(|b| b.to_ascii_lowercase()).collect();
    if head_lc.starts_with(b"<!doctype html") || head_lc.starts_with(b"<html") {
        return Some(PrettifyMode::Html);
    }
    // Generic XML element start.
    if trimmed[0] == b'<' {
        return Some(PrettifyMode::Xml);
    }
    // JSON object or array.
    if trimmed[0] == b'{' || trimmed[0] == b'[' {
        return Some(PrettifyMode::Json);
    }
    // YAML document marker on its own line (after optional whitespace).
    if trimmed.starts_with(b"---") {
        let rest = &trimmed[3..];
        if rest.is_empty() || rest[0] == b'\n' || rest[0] == b'\r' {
            return Some(PrettifyMode::Yaml);
        }
    }
    None
}

/// Combined resolver: explicit override (already parsed) → path extension
/// → byte sniff → undetected.
pub fn resolve(
    explicit: Option<PrettifyMode>,
    path: Option<&Path>,
    bytes: &[u8],
) -> ResolvedType {
    if let Some(m) = explicit {
        return ResolvedType::Mode(m);
    }
    if let Some(p) = path {
        if let Some(m) = detect_from_path(p) {
            return ResolvedType::Mode(m);
        }
    }
    if let Some(m) = detect_from_bytes(bytes) {
        return ResolvedType::Mode(m);
    }
    ResolvedType::Undetected
}

/// Run the transform for `mode` over `input` using `enc` to decode bytes to
/// text where required. `Off` returns the input verbatim (still allocates —
/// callers can short-circuit if they care). On parse failure, returns the
/// error string for the status line.
pub fn prettify(mode: PrettifyMode, input: &[u8], enc: crate::charset::Encoding) -> Result<Vec<u8>, String> {
    match mode {
        PrettifyMode::Off => Ok(input.to_vec()),
        PrettifyMode::Json => prettify_json(input, enc),
        PrettifyMode::Yaml => prettify_yaml(input, enc),
        PrettifyMode::Toml => prettify_toml(input, enc),
        PrettifyMode::Xml => prettify_xml(input, false),
        PrettifyMode::Html => prettify_xml(input, true),
        PrettifyMode::Csv => prettify_csv(input, enc),
    }
}

fn prettify_json(input: &[u8], enc: crate::charset::Encoding) -> Result<Vec<u8>, String> {
    let s = crate::charset::decode_line(input, enc);
    let value: serde_json::Value =
        serde_json::from_str(&s).map_err(|e| format!("json parse: {e}"))?;
    let mut out = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    Ok(out)
}

fn prettify_yaml(input: &[u8], enc: crate::charset::Encoding) -> Result<Vec<u8>, String> {
    let s = crate::charset::decode_line(input, enc);
    let value: serde_yml::Value =
        serde_yml::from_str(&s).map_err(|e| format!("yaml parse: {e}"))?;
    serde_yml::to_string(&value)
        .map(|s| s.into_bytes())
        .map_err(|e| format!("yaml emit: {e}"))
}

fn prettify_toml(input: &[u8], enc: crate::charset::Encoding) -> Result<Vec<u8>, String> {
    let s = crate::charset::decode_line(input, enc);
    let value: toml::Value = s.parse().map_err(|e: toml::de::Error| format!("toml parse: {e}"))?;
    toml::to_string_pretty(&value)
        .map(|s| s.into_bytes())
        .map_err(|e| format!("toml emit: {e}"))
}

/// Pretty-print XML/HTML by streaming through quick-xml events and re-emitting
/// with two-space indentation. `lenient = true` for HTML — turns off the strict
/// closing-tag-name check so unclosed void elements (`<br>`, `<img>`) and
/// case-insensitive close tags don't abort the parse.
fn prettify_xml(input: &[u8], lenient: bool) -> Result<Vec<u8>, String> {
    let mut reader = Reader::from_reader(input);
    let cfg = reader.config_mut();
    cfg.trim_text(true);
    if lenient {
        cfg.check_end_names = false;
    }
    let mut writer = Writer::new_with_indent(Cursor::new(Vec::new()), b' ', 2);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(e) => writer
                .write_event(e)
                .map_err(|e| format!("xml emit: {e}"))?,
            Err(e) => return Err(format!("xml parse: {e}")),
        }
        buf.clear();
    }
    let mut out = writer.into_inner().into_inner();
    if !out.ends_with(b"\n") {
        out.push(b'\n');
    }
    Ok(out)
}

/// Render CSV as a fixed-width aligned table with `|` separators.
/// Wide cells are truncated at 60 characters with an ellipsis so a single
/// runaway free-text column doesn't blow up the layout.
fn prettify_csv(input: &[u8], enc: crate::charset::Encoding) -> Result<Vec<u8>, String> {
    const COL_CAP: usize = 60;
    // Decode via the active charset so non-UTF-8 CSV (e.g. Latin-1) is parsed
    // as text rather than raw bytes.
    let decoded = crate::charset::decode_line(input, enc);
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(decoded.as_bytes());
    let records: Vec<csv::StringRecord> = rdr
        .records()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("csv parse: {e}"))?;
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let cols = records.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for r in &records {
        for (i, cell) in r.iter().enumerate() {
            let w = cell.chars().count().min(COL_CAP);
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }
    let mut out = String::new();
    for r in &records {
        let mut parts: Vec<String> = Vec::with_capacity(cols);
        for (i, width) in widths.iter().enumerate().take(cols) {
            let cell = r.get(i).unwrap_or("");
            let truncated: String = if cell.chars().count() > COL_CAP {
                let mut s: String = cell.chars().take(COL_CAP - 1).collect();
                s.push('…');
                s
            } else {
                cell.to_string()
            };
            let pad = width.saturating_sub(truncated.chars().count());
            parts.push(format!("{truncated}{}", " ".repeat(pad)));
        }
        out.push_str(&parts.join(" | "));
        out.push('\n');
    }
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_type_recognizes_aliases() {
        assert_eq!(parse_content_type("auto").unwrap(), None);
        assert_eq!(parse_content_type("raw").unwrap(), Some(PrettifyMode::Off));
        assert_eq!(parse_content_type("JSON").unwrap(), Some(PrettifyMode::Json));
        assert_eq!(parse_content_type(" yml ").unwrap(), Some(PrettifyMode::Yaml));
        assert_eq!(parse_content_type("htm").unwrap(), Some(PrettifyMode::Html));
        assert!(parse_content_type("nonsense").is_err());
    }

    #[test]
    fn detect_from_path_recognizes_known_extensions() {
        assert_eq!(detect_from_path(Path::new("a.json")), Some(PrettifyMode::Json));
        assert_eq!(detect_from_path(Path::new("a.YAML")), Some(PrettifyMode::Yaml));
        assert_eq!(detect_from_path(Path::new("a.yml")), Some(PrettifyMode::Yaml));
        assert_eq!(detect_from_path(Path::new("a.toml")), Some(PrettifyMode::Toml));
        assert_eq!(detect_from_path(Path::new("page.HTML")), Some(PrettifyMode::Html));
        assert_eq!(detect_from_path(Path::new("data.csv")), Some(PrettifyMode::Csv));
        assert_eq!(detect_from_path(Path::new("README")), None);
        assert_eq!(detect_from_path(Path::new("a.txt")), None);
    }

    #[test]
    fn detect_from_bytes_sniffs_json() {
        assert_eq!(detect_from_bytes(b"{\"a\":1}"), Some(PrettifyMode::Json));
        assert_eq!(detect_from_bytes(b"   [1,2,3]"), Some(PrettifyMode::Json));
    }

    #[test]
    fn detect_from_bytes_sniffs_xml_declaration() {
        assert_eq!(detect_from_bytes(b"<?xml version=\"1.0\"?>"), Some(PrettifyMode::Xml));
    }

    #[test]
    fn detect_from_bytes_sniffs_html_doctype_case_insensitive() {
        assert_eq!(detect_from_bytes(b"<!DOCTYPE html>"), Some(PrettifyMode::Html));
        assert_eq!(detect_from_bytes(b"<html><body>"), Some(PrettifyMode::Html));
    }

    #[test]
    fn detect_from_bytes_sniffs_yaml_doc_marker() {
        assert_eq!(detect_from_bytes(b"---\nkey: value\n"), Some(PrettifyMode::Yaml));
        // Triple-dash followed by other text is NOT a YAML doc marker.
        assert_eq!(detect_from_bytes(b"---changelog"), None);
    }

    #[test]
    fn detect_from_bytes_falls_back_to_none() {
        assert_eq!(detect_from_bytes(b"plain text"), None);
        assert_eq!(detect_from_bytes(b""), None);
        assert_eq!(detect_from_bytes(b"   \n\t  "), None);
    }

    fn utf8() -> crate::charset::Encoding { crate::charset::Encoding::utf8() }

    #[test]
    fn prettify_json_indents_compact_input() {
        let out = prettify(PrettifyMode::Json, b"{\"a\":1,\"b\":[2,3]}", utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("\"a\": 1"));
        assert!(s.contains("\"b\":"));
        // Result has newlines.
        assert!(s.matches('\n').count() >= 4);
    }

    #[test]
    fn prettify_json_returns_error_on_bad_input() {
        assert!(prettify(PrettifyMode::Json, b"{not json", utf8()).is_err());
    }

    #[test]
    fn prettify_yaml_round_trips() {
        let out = prettify(PrettifyMode::Yaml, b"a: 1\nb:\n  - 2\n  - 3\n", utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("a:"));
        assert!(s.contains("b:"));
    }

    #[test]
    fn prettify_toml_indents_compact_input() {
        let out = prettify(PrettifyMode::Toml, b"a=1\nb=2\n[s]\nc=3\n", utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("a = 1"));
        assert!(s.contains("[s]"));
    }

    #[test]
    fn prettify_xml_indents_with_text_preservation() {
        let out = prettify(PrettifyMode::Xml, b"<root><a>x</a><b/></root>", utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<root>"));
        assert!(s.contains("<a>x</a>"));
        // Check there's at least one newline + indentation pattern.
        assert!(s.contains("\n  "), "expected indented child, got: {s}");
    }

    #[test]
    fn prettify_html_handles_unclosed_void_tags() {
        // <br> and <img> are void in HTML but not self-closed in source — strict
        // XML mode would error; html mode (lenient) tolerates it.
        let html = b"<html><body><br><img src=\"x\"></body></html>";
        let out = prettify(PrettifyMode::Html, html, utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("<html>"));
        assert!(s.contains("<br"));
    }

    #[test]
    fn prettify_csv_aligns_columns() {
        let out = prettify(PrettifyMode::Csv, b"name,age\nalice,30\nbob,4\n", utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        // Each row should have the same byte width up to the separator.
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 3);
        // The "name" column gets padded so "bob  " has the same visual width as "alice".
        // Verify by checking that the " | " separator appears at the same byte offset on each line.
        let first_pipe: Vec<usize> = lines.iter().map(|l| l.find(" | ").unwrap()).collect();
        assert!(first_pipe.windows(2).all(|w| w[0] == w[1]),
                "expected aligned columns, got: {lines:?}");
    }

    #[test]
    fn prettify_csv_truncates_long_cells() {
        let big = "x".repeat(200);
        let input = format!("a,{big}\n1,2\n");
        let out = prettify(PrettifyMode::Csv, input.as_bytes(), utf8()).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains('…'), "expected ellipsis truncation, got: {s}");
    }

    #[test]
    fn prettify_off_passes_through() {
        let raw = b"arbitrary bytes\nwith newlines\n";
        let out = prettify(PrettifyMode::Off, raw, utf8()).unwrap();
        assert_eq!(&out, raw);
    }

    #[test]
    fn prettify_json_latin1_decodes_non_ascii() {
        // JSON value "caf\xe9" encoded as Latin-1 (0xE9 = é).
        // prettify with iso-8859-1 should decode to "café" before parsing.
        let enc = crate::charset::parse_label("iso-8859-1").unwrap();
        let input = b"{\"name\":\"caf\xe9\"}";
        let out = prettify(PrettifyMode::Json, input, enc).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("café"), "expected café in output, got: {s}");
    }

    #[test]
    fn resolve_prefers_explicit_then_path_then_sniff() {
        // Explicit wins.
        assert_eq!(
            resolve(Some(PrettifyMode::Yaml), Some(Path::new("a.json")), b"{\"x\":1}"),
            ResolvedType::Mode(PrettifyMode::Yaml)
        );
        // No explicit: path next.
        assert_eq!(
            resolve(None, Some(Path::new("a.json")), b"plain text"),
            ResolvedType::Mode(PrettifyMode::Json)
        );
        // No explicit, no path: sniff.
        assert_eq!(
            resolve(None, None, b"<?xml version=\"1.0\"?><r/>"),
            ResolvedType::Mode(PrettifyMode::Xml)
        );
        // Nothing matches.
        assert_eq!(resolve(None, None, b"plain text"), ResolvedType::Undetected);
    }
}
