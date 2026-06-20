//! The OR dimension of filtering: `--or-filter` / `--or-grep` grouped by
//! `--or-group`. A record passes the OR clause when *every* non-empty group
//! has *at least one* matching condition (OR within a group, AND across
//! groups). Required `--filter`/`--grep` are unchanged and AND'd on top.
//!
//! Each OR-filter compiles to a single-spec `CompiledFilter` and each OR-grep
//! to a single-pattern `GrepPredicate`, so the existing field/regex machinery
//! (including case policy and records-mode evaluation) is reused verbatim.

use crate::filter::{CompiledFilter, FilterMatch, FilterSpec};
use crate::format::LogFormat;
use crate::grep::GrepPredicate;
use crate::viewport::CaseMode;

/// Name of the implicit group that holds OR-conditions given without an
/// explicit `--or-group`.
pub const DEFAULT_GROUP: &str = "default";

/// Raw, ungrouped OR-conditions collected from argv or config, before
/// compilation. Groups are kept in first-seen order.
#[derive(Debug, Default, Clone)]
pub struct OrSpecRaw {
    groups: Vec<RawGroup>,
}

#[derive(Debug, Clone)]
struct RawGroup {
    name: String,
    filters: Vec<String>,
    greps: Vec<String>,
}

impl OrSpecRaw {
    pub fn new() -> Self {
        Self::default()
    }

    fn group_mut(&mut self, name: &str) -> &mut RawGroup {
        if let Some(i) = self.groups.iter().position(|g| g.name == name) {
            return &mut self.groups[i];
        }
        self.groups.push(RawGroup {
            name: name.to_string(),
            filters: Vec::new(),
            greps: Vec::new(),
        });
        self.groups.last_mut().unwrap()
    }

    pub fn add_filter(&mut self, group: &str, spec: String) {
        self.group_mut(group).filters.push(spec);
    }

    pub fn add_grep(&mut self, group: &str, pattern: String) {
        self.group_mut(group).greps.push(pattern);
    }

    /// True when there are no OR-conditions at all (the OR clause is then
    /// vacuously satisfied and callers can skip compilation).
    pub fn is_empty(&self) -> bool {
        self.groups
            .iter()
            .all(|g| g.filters.is_empty() && g.greps.is_empty())
    }

    /// True when any group carries a field-filter condition, which requires a
    /// `--format` to resolve named captures.
    pub fn has_filters(&self) -> bool {
        self.groups.iter().any(|g| !g.filters.is_empty())
    }
}

/// One compiled OR-group. Matches when ANY contained condition matches.
#[derive(Debug)]
struct OrGroup {
    filters: Vec<CompiledFilter>,
    greps: Vec<GrepPredicate>,
}

impl OrGroup {
    fn matches_line(&self, line: &[u8], enc: crate::charset::Encoding) -> bool {
        self.filters
            .iter()
            .any(|f| matches!(f.evaluate(line, enc), FilterMatch::Matched))
            || self.greps.iter().any(|g| g.matches(line, enc))
    }

    /// OR-filters use `evaluate_record` (dotall + multi-line) so captures span
    /// the whole record. OR-greps reuse `GrepPredicate::matches`, which scans
    /// the full record bytes — a literal pattern on any continuation line still
    /// matches — but is compiled single-line, so a `.` won't cross a newline.
    /// This is identical to how the required `--grep` behaves on records.
    fn matches_record(&self, record: &[u8], enc: crate::charset::Encoding) -> bool {
        self.filters
            .iter()
            .any(|f| matches!(f.evaluate_record(record, enc), FilterMatch::Matched))
            || self.greps.iter().any(|g| g.matches(record, enc))
    }
}

/// The compiled OR dimension. Empty (`is_active() == false`) means "no OR
/// constraint": both `matches_*` return true via `all` over an empty list.
#[derive(Debug, Default)]
pub struct OrGroups {
    groups: Vec<OrGroup>,
}

impl OrGroups {
    pub fn is_active(&self) -> bool {
        !self.groups.is_empty()
    }

    pub fn matches_line(&self, line: &[u8], enc: crate::charset::Encoding) -> bool {
        self.groups.iter().all(|g| g.matches_line(line, enc))
    }

    pub fn matches_record(&self, record: &[u8], enc: crate::charset::Encoding) -> bool {
        self.groups.iter().all(|g| g.matches_record(record, enc))
    }

    /// Compile raw specs against an optional `format` (required only when any
    /// OR-filter is present) and the active `case_mode`. Empty groups are
    /// dropped so they impose no constraint.
    pub fn compile(
        raw: &OrSpecRaw,
        format: Option<&LogFormat>,
        case_mode: CaseMode,
    ) -> Result<Self, String> {
        let mut groups = Vec::new();
        for rg in &raw.groups {
            if rg.filters.is_empty() && rg.greps.is_empty() {
                continue;
            }
            let mut filters = Vec::with_capacity(rg.filters.len());
            for spec_str in &rg.filters {
                let fmt = format.ok_or_else(|| "--or-filter requires --format".to_string())?;
                let spec = FilterSpec::parse(spec_str)?;
                filters.push(CompiledFilter::compile(fmt, vec![spec], case_mode)?);
            }
            let mut greps = Vec::with_capacity(rg.greps.len());
            for pat in &rg.greps {
                greps.push(GrepPredicate::compile(std::slice::from_ref(pat), case_mode)?);
            }
            groups.push(OrGroup { filters, greps });
        }
        Ok(Self { groups })
    }
}

/// Walk an already-`expand_argv`'d argv and collect OR-conditions grouped by
/// the most recent `--or-group` marker (default group before any marker).
/// Handles `--flag value` and `--flag=value`. Does not strip tokens — clap
/// still parses them for `--help` and error reporting; this is the source of
/// truth for grouping because clap's `Vec` collection drops ordering.
pub fn extract_from_argv(argv: &[String]) -> OrSpecRaw {
    let mut raw = OrSpecRaw::new();
    let mut current = DEFAULT_GROUP.to_string();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        let (flag, inline): (&str, Option<String>) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        // Resolve this flag's value: inline (`--flag=value`) or the next token.
        // `--or-grep --foo` treats `--foo` as the pattern, matching clap's
        // own two-token parse; clap owns validation, we only track ordering.
        let value: Option<String> = if inline.is_some() {
            inline
        } else if matches!(flag, "--or-group" | "--or-filter" | "--or-grep") {
            match argv.get(i + 1) {
                Some(v) => {
                    i += 1;
                    Some(v.clone())
                }
                None => None,
            }
        } else {
            None
        };
        match flag {
            "--or-group" => {
                if let Some(v) = value {
                    current = v;
                }
            }
            "--or-filter" => {
                if let Some(v) = value {
                    raw.add_filter(&current, v);
                }
            }
            "--or-grep" => {
                if let Some(v) = value {
                    raw.add_grep(&current, v);
                }
            }
            _ => {}
        }
        i += 1;
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt() -> LogFormat {
        LogFormat::compile("app", r"^(?P<lvl>\w+) (?P<msg>.+)$").unwrap()
    }

    #[test]
    fn empty_spec_is_inactive_and_matches_everything() {
        let raw = OrSpecRaw::new();
        let og = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap();
        assert!(!og.is_active());
        assert!(og.matches_line(b"anything", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn default_group_is_a_single_or_pool() {
        let mut raw = OrSpecRaw::new();
        raw.add_grep(DEFAULT_GROUP, "failed".into());
        raw.add_grep(DEFAULT_GROUP, "invalid".into());
        let og = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap();
        assert!(og.is_active());
        assert!(og.matches_line(b"login failed", crate::charset::Encoding::utf8()));
        assert!(og.matches_line(b"invalid user", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"all good", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn groups_are_anded_conditions_within_group_ored() {
        let mut raw = OrSpecRaw::new();
        raw.add_grep("a", "failed".into());
        raw.add_grep("a", "denied".into());
        raw.add_grep("b", "ssh".into());
        let og = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"ssh login failed", crate::charset::Encoding::utf8()));
        assert!(og.matches_line(b"ssh access denied", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"login failed", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"ssh login ok", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn or_filter_and_or_grep_share_a_group() {
        let mut raw = OrSpecRaw::new();
        raw.add_filter(DEFAULT_GROUP, "lvl=ERROR".into());
        raw.add_grep(DEFAULT_GROUP, "panic".into());
        let og = OrGroups::compile(&raw, Some(&fmt()), CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"ERROR disk full", crate::charset::Encoding::utf8()));
        assert!(og.matches_line(b"INFO panic trace", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"INFO ok", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn or_filter_without_format_errors() {
        let mut raw = OrSpecRaw::new();
        raw.add_filter(DEFAULT_GROUP, "lvl=ERROR".into());
        let err = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap_err();
        assert!(err.contains("requires --format"), "{err}");
    }

    #[test]
    fn has_filters_detects_field_conditions() {
        let mut raw = OrSpecRaw::new();
        raw.add_grep(DEFAULT_GROUP, "x".into());
        assert!(!raw.has_filters());
        raw.add_filter("g", "lvl=ERROR".into());
        assert!(raw.has_filters());
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn extract_unlabeled_go_to_default() {
        let raw = extract_from_argv(&argv(&[
            "tess", "--or-grep", "failed", "--or-filter", "lvl=ERROR",
        ]));
        let og = OrGroups::compile(&raw, Some(&fmt()), CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"INFO failed", crate::charset::Encoding::utf8()));
        assert!(og.matches_line(b"ERROR x", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"INFO ok", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn extract_or_group_marker_scopes_following_conditions() {
        let raw = extract_from_argv(&argv(&[
            "tess",
            "--or-grep", "failed",
            "--or-group", "svc",
            "--or-grep", "ssh",
        ]));
        let og = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"ssh failed", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"ssh ok", crate::charset::Encoding::utf8()));
        assert!(!og.matches_line(b"http failed", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn extract_handles_attached_equals_form() {
        let raw = extract_from_argv(&argv(&[
            "tess", "--or-group=svc", "--or-grep=ssh", "--or-filter=lvl=ERROR",
        ]));
        let og = OrGroups::compile(&raw, Some(&fmt()), CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"ssh ERROR", crate::charset::Encoding::utf8()));
    }

    #[test]
    fn extract_ignores_non_or_flags() {
        let raw = extract_from_argv(&argv(&["tess", "--follow", "-N", "file.log"]));
        assert!(raw.is_empty());
    }

    #[test]
    fn extract_or_group_at_eof_does_not_panic() {
        // A dangling `--or-group` (no following value) leaves the current group
        // unchanged and must not panic. (clap rejects this in production, so the
        // function never actually sees it, but be robust regardless.)
        let raw = extract_from_argv(&argv(&["tess", "--or-grep", "x", "--or-group"]));
        assert!(!raw.is_empty());
    }
}
