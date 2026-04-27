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
}

impl LogFormat {
    pub fn compile(name: &str, pattern: &str) -> Result<Self, String> {
        let regex = Regex::new(pattern).map_err(|e| format!("format `{name}`: {e}"))?;
        // capture_names() includes all groups (including the implicit whole-match
        // group as None at index 0); skip those.
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
        Ok(Self { name: name.to_string(), regex, field_names })
    }
}

/// TOML schema for `~/.config/tess/formats.toml`:
///
/// ```toml
/// [format.myapp]
/// regex = "..."
/// ```
#[derive(Debug, Deserialize)]
struct UserConfig {
    #[serde(default)]
    format: HashMap<String, FormatEntry>,
}

#[derive(Debug, Deserialize)]
struct FormatEntry {
    regex: String,
}

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

fn load_user_formats() -> Result<HashMap<String, String>, String> {
    let Some(path) = user_config_path() else {
        return Ok(HashMap::new());
    };
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("reading {}: {e}", path.display()))?;
    let parsed: UserConfig = toml::from_str(&text)
        .map_err(|e| format!("parsing {}: {e}", path.display()))?;
    Ok(parsed.format.into_iter().map(|(k, v)| (k, v.regex)).collect())
}

/// Load all formats: built-ins first, then any in `~/.config/tess/formats.toml`
/// (which override built-ins of the same name). Returns the compiled map keyed
/// by format name.
pub fn load_all() -> Result<HashMap<String, LogFormat>, String> {
    let mut sources: HashMap<String, String> = HashMap::new();
    for (name, pat) in BUILTINS {
        sources.insert(name.to_string(), pat.to_string());
    }
    let user = load_user_formats()?;
    for (name, pat) in user {
        sources.insert(name, pat);
    }
    let mut compiled = HashMap::new();
    for (name, pat) in sources {
        let fmt = LogFormat::compile(&name, &pat)?;
        compiled.insert(name, fmt);
    }
    Ok(compiled)
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

    #[test]
    fn builtins_all_compile() {
        for (name, pat) in BUILTINS {
            LogFormat::compile(name, pat)
                .unwrap_or_else(|e| panic!("built-in {name} should compile: {e}"));
        }
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
    fn user_config_overrides_builtin_via_load_all() {
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
}
