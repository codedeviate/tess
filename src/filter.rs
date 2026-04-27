use regex::Regex;

use crate::format::LogFormat;

/// Operator in a single filter spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterOp {
    /// `field=value` — exact match.
    Eq,
    /// `field!=value` — exact non-match.
    Ne,
    /// `field~regex` — regex match.
    Re,
    /// `field!~regex` — regex non-match.
    NotRe,
}

/// A parsed filter spec, before being bound to a format.
#[derive(Debug, Clone)]
pub struct FilterSpec {
    pub field: String,
    pub op: FilterOp,
    pub value: String,
}

impl FilterSpec {
    /// Parse a filter spec like `status=500`, `ip~^10\.`, `status!=200`,
    /// `agent!~bot`. Operator detection scans for the longest match first so
    /// `!=` and `!~` aren't confused with `=` or `~`.
    pub fn parse(input: &str) -> Result<Self, String> {
        for (op, sep) in &[
            (FilterOp::NotRe, "!~"),
            (FilterOp::Ne, "!="),
            (FilterOp::Re, "~"),
            (FilterOp::Eq, "="),
        ] {
            if let Some((field, value)) = input.split_once(sep) {
                if field.is_empty() {
                    return Err(format!("filter `{input}`: empty field name"));
                }
                return Ok(FilterSpec {
                    field: field.to_string(),
                    op: op.clone(),
                    value: value.to_string(),
                });
            }
        }
        Err(format!(
            "filter `{input}`: missing operator (expected =, !=, ~, or !~)"
        ))
    }
}

/// A single compiled predicate: an operator and (for regex ops) the compiled
/// regex.
#[derive(Debug)]
struct CompiledPredicate {
    field: String,
    op: FilterOp,
    /// Used for `Eq` / `Ne` (byte-exact comparison).
    literal: Option<String>,
    /// Used for `Re` / `NotRe`.
    regex: Option<Regex>,
}

/// A compiled filter bound to a specific format. Evaluating a line runs the
/// format's regex once and applies all predicates against the resulting
/// captures. AND semantics: a line matches iff every predicate matches.
#[derive(Debug)]
pub struct CompiledFilter {
    pub format_name: String,
    format_regex: Regex,
    predicates: Vec<CompiledPredicate>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum FilterMatch {
    /// Line matches every predicate.
    Matched,
    /// Line parsed against the format but at least one predicate didn't match.
    NotMatched,
    /// Line didn't parse against the format at all.
    NotParsed,
}

impl CompiledFilter {
    /// Compile the given specs against `format`. Validates that every spec's
    /// field is one of the format's named captures.
    pub fn compile(format: &LogFormat, specs: Vec<FilterSpec>) -> Result<Self, String> {
        let mut predicates = Vec::with_capacity(specs.len());
        for spec in specs {
            if !format.field_names.iter().any(|n| n == &spec.field) {
                return Err(format!(
                    "filter `{}{:?}{}`: field `{}` is not in format `{}` (available: {})",
                    spec.field,
                    spec.op,
                    spec.value,
                    spec.field,
                    format.name,
                    format.field_names.join(", "),
                ));
            }
            let (literal, regex) = match spec.op {
                FilterOp::Eq | FilterOp::Ne => (Some(spec.value.clone()), None),
                FilterOp::Re | FilterOp::NotRe => {
                    let r = Regex::new(&spec.value)
                        .map_err(|e| format!("filter `{}`: invalid regex `{}`: {e}", spec.field, spec.value))?;
                    (None, Some(r))
                }
            };
            predicates.push(CompiledPredicate {
                field: spec.field,
                op: spec.op,
                literal,
                regex,
            });
        }
        Ok(Self {
            format_name: format.name.clone(),
            format_regex: format.regex.clone(),
            predicates,
        })
    }

    /// Evaluate the filter against a single logical line of bytes. Decodes the
    /// line as UTF-8 with a lossy fallback so non-UTF-8 bytes can still flow
    /// through (they just won't match string-equal predicates).
    pub fn evaluate(&self, line: &[u8]) -> FilterMatch {
        let line_str = match std::str::from_utf8(line) {
            Ok(s) => s,
            Err(_) => return FilterMatch::NotParsed,
        };
        let Some(caps) = self.format_regex.captures(line_str) else {
            return FilterMatch::NotParsed;
        };
        for p in &self.predicates {
            let Some(m) = caps.name(&p.field) else {
                return FilterMatch::NotMatched;
            };
            let captured = m.as_str();
            let ok = match p.op {
                FilterOp::Eq => p.literal.as_deref() == Some(captured),
                FilterOp::Ne => p.literal.as_deref() != Some(captured),
                FilterOp::Re => p.regex.as_ref().map_or(false, |r| r.is_match(captured)),
                FilterOp::NotRe => p.regex.as_ref().map_or(false, |r| !r.is_match(captured)),
            };
            if !ok {
                return FilterMatch::NotMatched;
            }
        }
        FilterMatch::Matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apache_combined() -> LogFormat {
        LogFormat::compile(
            "apache-combined",
            r#"^(?P<ip>\S+) \S+ (?P<user>\S+) \[(?P<time>[^\]]+)\] "(?P<method>\S+) (?P<url>\S+) (?P<protocol>[^"]+)" (?P<status>\d+) (?P<size>\S+) "(?P<referer>[^"]*)" "(?P<agent>[^"]*)"$"#,
        )
        .unwrap()
    }

    const SAMPLE_200: &[u8] = br#"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] "GET /index.html HTTP/1.1" 200 2326 "-" "Mozilla/5.0""#;
    const SAMPLE_500: &[u8] = br#"127.0.0.1 - alice [10/Oct/2023:13:55:36 +0000] "GET /api/data HTTP/1.1" 500 512 "-" "curl/7.0""#;
    const NON_PARSING: &[u8] = b"this line does not match the format at all";

    #[test]
    fn parse_eq() {
        let s = FilterSpec::parse("status=500").unwrap();
        assert_eq!(s.field, "status");
        assert_eq!(s.op, FilterOp::Eq);
        assert_eq!(s.value, "500");
    }

    #[test]
    fn parse_ne_before_eq() {
        let s = FilterSpec::parse("status!=200").unwrap();
        assert_eq!(s.op, FilterOp::Ne);
        assert_eq!(s.value, "200");
    }

    #[test]
    fn parse_re() {
        let s = FilterSpec::parse(r"ip~^10\.").unwrap();
        assert_eq!(s.op, FilterOp::Re);
        assert_eq!(s.value, r"^10\.");
    }

    #[test]
    fn parse_not_re_before_re() {
        let s = FilterSpec::parse("agent!~bot").unwrap();
        assert_eq!(s.op, FilterOp::NotRe);
        assert_eq!(s.value, "bot");
    }

    #[test]
    fn parse_rejects_no_operator() {
        let err = FilterSpec::parse("status").unwrap_err();
        assert!(err.contains("missing operator"), "{err}");
    }

    #[test]
    fn parse_rejects_empty_field() {
        let err = FilterSpec::parse("=500").unwrap_err();
        assert!(err.contains("empty field"), "{err}");
    }

    #[test]
    fn compile_rejects_unknown_field() {
        let fmt = apache_combined();
        let specs = vec![FilterSpec::parse("notafield=x").unwrap()];
        let err = CompiledFilter::compile(&fmt, specs).unwrap_err();
        assert!(err.contains("not in format"), "{err}");
    }

    #[test]
    fn evaluate_eq_matches() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status=500").unwrap()]).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_re_matches_5xx() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status~^5").unwrap()]).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_ne_excludes_200() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status!=200").unwrap()]).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_multiple_filters_and() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(
            &fmt,
            vec![
                FilterSpec::parse("status~^5").unwrap(),
                FilterSpec::parse(r"url~/api/").unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(f.evaluate(SAMPLE_500), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_unparseable_line_is_not_parsed() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status=200").unwrap()]).unwrap();
        assert_eq!(f.evaluate(NON_PARSING), FilterMatch::NotParsed);
    }
}
