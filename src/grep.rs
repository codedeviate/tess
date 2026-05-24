use regex::Regex;

/// AND-combined regex predicate applied to raw line bytes. Lines that fail
/// UTF-8 decoding never match (mirrors what the interactive `/` search does).
#[derive(Debug)]
pub struct GrepPredicate {
    regexes: Vec<Regex>,
}

impl GrepPredicate {
    /// Compile each pattern under the given case policy. Returns the first
    /// invalid pattern's error, prefixed so the user can tell which
    /// `--grep` argument was bad. The case policy is applied via
    /// `CaseMode::apply_to_pattern`, so the `(?i)` flag is prepended when
    /// insensitivity is wanted.
    pub fn compile(
        patterns: &[String],
        case_mode: crate::viewport::CaseMode,
    ) -> Result<Self, String> {
        let mut regexes = Vec::with_capacity(patterns.len());
        for p in patterns {
            let compiled = case_mode.apply_to_pattern(p);
            let r = Regex::new(&compiled).map_err(|e| format!("--grep `{p}`: {e}"))?;
            regexes.push(r);
        }
        Ok(Self { regexes })
    }

    pub fn is_empty(&self) -> bool { self.regexes.is_empty() }

    /// True iff every compiled pattern matches the line. Empty predicate
    /// vacuously matches (callers should treat that as "no grep configured"
    /// via `is_empty` and not even ask).
    pub fn matches(&self, line: &[u8]) -> bool {
        let s = match std::str::from_utf8(line) {
            Ok(s) => s,
            Err(_) => return false,
        };
        self.regexes.iter().all(|r| r.is_match(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_predicate_is_empty() {
        let g = GrepPredicate::compile(&[], crate::viewport::CaseMode::Sensitive).unwrap();
        assert!(g.is_empty());
    }

    #[test]
    fn single_pattern_matches() {
        let g = GrepPredicate::compile(&["error".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert!(g.matches(b"something failed: error 42"));
        assert!(!g.matches(b"all good"));
    }

    #[test]
    fn multiple_patterns_are_anded() {
        let g = GrepPredicate::compile(
            &["error".to_string(), r"^\[\d{4}".to_string()],
            crate::viewport::CaseMode::Sensitive,
        ).unwrap();
        assert!(g.matches(b"[2026-05-13] error occurred"));
        assert!(!g.matches(b"[2026-05-13] all good"));
        assert!(!g.matches(b"error occurred (no timestamp)"));
    }

    #[test]
    fn invalid_regex_is_reported_with_arg() {
        let err = GrepPredicate::compile(&["[unclosed".to_string()], crate::viewport::CaseMode::Sensitive).unwrap_err();
        assert!(err.contains("--grep `[unclosed`"), "{err}");
    }

    #[test]
    fn non_utf8_line_never_matches() {
        let g = GrepPredicate::compile(&[".".to_string()], crate::viewport::CaseMode::Sensitive).unwrap();
        // Lone 0xFF is invalid UTF-8.
        assert!(!g.matches(&[0xFF, b'a', b'b']));
    }

    #[test]
    fn insensitive_mode_matches_uppercase_input() {
        let g = GrepPredicate::compile(
            &["foo".to_string()],
            crate::viewport::CaseMode::Insensitive,
        )
        .unwrap();
        assert!(g.matches(b"FooBar"));
        assert!(g.matches(b"FOO"));
        assert!(g.matches(b"foo"));
    }

    #[test]
    fn smart_mode_lowercase_is_insensitive() {
        let g = GrepPredicate::compile(
            &["foo".to_string()],
            crate::viewport::CaseMode::Smart,
        )
        .unwrap();
        assert!(g.matches(b"FOO bar"));
    }

    #[test]
    fn smart_mode_uppercase_is_sensitive() {
        let g = GrepPredicate::compile(
            &["Foo".to_string()],
            crate::viewport::CaseMode::Smart,
        )
        .unwrap();
        assert!(g.matches(b"Foo bar"));
        assert!(!g.matches(b"foo bar"));
        assert!(!g.matches(b"FOO bar"));
    }
}
