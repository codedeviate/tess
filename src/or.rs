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
    fn matches_line(&self, line: &[u8]) -> bool {
        self.filters
            .iter()
            .any(|f| matches!(f.evaluate(line), FilterMatch::Matched))
            || self.greps.iter().any(|g| g.matches(line))
    }

    /// OR-filters use `evaluate_record` (dotall + multi-line) so captures span
    /// the whole record. OR-greps reuse `GrepPredicate::matches`, which scans
    /// the full record bytes — a literal pattern on any continuation line still
    /// matches — but is compiled single-line, so a `.` won't cross a newline.
    /// This is identical to how the required `--grep` behaves on records.
    fn matches_record(&self, record: &[u8]) -> bool {
        self.filters
            .iter()
            .any(|f| matches!(f.evaluate_record(record), FilterMatch::Matched))
            || self.greps.iter().any(|g| g.matches(record))
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

    pub fn matches_line(&self, line: &[u8]) -> bool {
        self.groups.iter().all(|g| g.matches_line(line))
    }

    pub fn matches_record(&self, record: &[u8]) -> bool {
        self.groups.iter().all(|g| g.matches_record(record))
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
        assert!(og.matches_line(b"anything"));
    }

    #[test]
    fn default_group_is_a_single_or_pool() {
        let mut raw = OrSpecRaw::new();
        raw.add_grep(DEFAULT_GROUP, "failed".into());
        raw.add_grep(DEFAULT_GROUP, "invalid".into());
        let og = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap();
        assert!(og.is_active());
        assert!(og.matches_line(b"login failed"));
        assert!(og.matches_line(b"invalid user"));
        assert!(!og.matches_line(b"all good"));
    }

    #[test]
    fn groups_are_anded_conditions_within_group_ored() {
        let mut raw = OrSpecRaw::new();
        raw.add_grep("a", "failed".into());
        raw.add_grep("a", "denied".into());
        raw.add_grep("b", "ssh".into());
        let og = OrGroups::compile(&raw, None, CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"ssh login failed"));
        assert!(og.matches_line(b"ssh access denied"));
        assert!(!og.matches_line(b"login failed"));
        assert!(!og.matches_line(b"ssh login ok"));
    }

    #[test]
    fn or_filter_and_or_grep_share_a_group() {
        let mut raw = OrSpecRaw::new();
        raw.add_filter(DEFAULT_GROUP, "lvl=ERROR".into());
        raw.add_grep(DEFAULT_GROUP, "panic".into());
        let og = OrGroups::compile(&raw, Some(&fmt()), CaseMode::Sensitive).unwrap();
        assert!(og.matches_line(b"ERROR disk full"));
        assert!(og.matches_line(b"INFO panic trace"));
        assert!(!og.matches_line(b"INFO ok"));
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
}
