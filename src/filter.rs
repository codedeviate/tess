use regex::{Regex, RegexBuilder};

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
    /// `field<value` — less than (numeric if both sides parse as f64, else lex).
    Lt,
    /// `field<=value` — less-than-or-equal.
    Le,
    /// `field>value` — greater than.
    Gt,
    /// `field>=value` — greater-than-or-equal.
    Ge,
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
    /// `agent!~bot`, `status>=500`, `hour<12`. Operator detection scans for
    /// the longest match first so multi-char operators (`!=`, `!~`, `<=`,
    /// `>=`) aren't confused with their single-char prefixes.
    pub fn parse(input: &str) -> Result<Self, String> {
        for (op, sep) in &[
            (FilterOp::NotRe, "!~"),
            (FilterOp::Ne, "!="),
            (FilterOp::Le, "<="),
            (FilterOp::Ge, ">="),
            (FilterOp::Re, "~"),
            (FilterOp::Eq, "="),
            (FilterOp::Lt, "<"),
            (FilterOp::Gt, ">"),
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
            "filter `{input}`: missing operator (expected =, !=, ~, !~, <, <=, >, or >=)"
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
///
/// `format_regex_record` is a sibling compiled from the same source pattern
/// with dotall + multi-line flags enabled. Records-mode callers use it so
/// greedy `.` / `.+` captures span across newlines within a single record
/// (e.g. `(?P<message>.*)$` captures the entire record body instead of
/// failing because the original `$` only matches end-of-input).
#[derive(Debug)]
pub struct CompiledFilter {
    pub format_name: String,
    format_regex: Regex,
    format_regex_record: Regex,
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
    /// field is one of the format's named captures. The `case_mode` applies
    /// to the regex operators (`~` / `!~`); literal-comparison operators
    /// (`=`, `!=`, `<`, `<=`, `>`, `>=`) ignore it.
    pub fn compile(
        format: &LogFormat,
        specs: Vec<FilterSpec>,
        case_mode: crate::viewport::CaseMode,
    ) -> Result<Self, String> {
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
                FilterOp::Eq
                | FilterOp::Ne
                | FilterOp::Lt
                | FilterOp::Le
                | FilterOp::Gt
                | FilterOp::Ge => (Some(spec.value.clone()), None),
                FilterOp::Re | FilterOp::NotRe => {
                    let compiled = case_mode.apply_to_pattern(&spec.value);
                    let r = Regex::new(&compiled)
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
        let format_regex_record = RegexBuilder::new(format.regex.as_str())
            .dot_matches_new_line(true)
            .multi_line(true)
            .build()
            .map_err(|e| {
                format!("format `{}`: rebuilding regex for records mode: {e}", format.name)
            })?;

        Ok(Self {
            format_name: format.name.clone(),
            format_regex: format.regex.clone(),
            format_regex_record,
            predicates,
        })
    }

    /// Evaluate the filter against a single logical line of bytes, decoded via `enc`.
    pub fn evaluate(&self, line: &[u8], enc: crate::charset::Encoding) -> FilterMatch {
        self.evaluate_with(&self.format_regex, line, enc)
    }

    /// Records-mode evaluation: runs the format regex with dotall + multi-line
    /// flags enabled against the full multi-line record bytes. Greedy
    /// captures like `(?P<message>.*)$` consume the whole body of the record,
    /// so predicates can match content on any continuation line.
    pub fn evaluate_record(&self, record: &[u8], enc: crate::charset::Encoding) -> FilterMatch {
        self.evaluate_with(&self.format_regex_record, record, enc)
    }

    fn evaluate_with(&self, regex: &Regex, bytes: &[u8], enc: crate::charset::Encoding) -> FilterMatch {
        let line_str = crate::charset::decode_line(bytes, enc);
        let Some(caps) = regex.captures(&line_str) else {
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
                FilterOp::Re => p.regex.as_ref().is_some_and(|r| r.is_match(captured)),
                FilterOp::NotRe => p.regex.as_ref().is_some_and(|r| !r.is_match(captured)),
                FilterOp::Lt | FilterOp::Le | FilterOp::Gt | FilterOp::Ge => {
                    let rhs = p.literal.as_deref().unwrap_or("");
                    compare(&p.op, captured, rhs)
                }
            };
            if !ok {
                return FilterMatch::NotMatched;
            }
        }
        FilterMatch::Matched
    }
}

/// Compare `lhs` against `rhs` under the given ordering operator.
///
/// Tries numeric comparison first (both sides parse as f64); falls back to
/// lexicographic byte order. Numeric is intentionally lossy on integer
/// overflow — log fields are typically small numbers (status codes, sizes,
/// hours), and f64 covers the practical range.
fn compare(op: &FilterOp, lhs: &str, rhs: &str) -> bool {
    let order = match (lhs.parse::<f64>(), rhs.parse::<f64>()) {
        (Ok(a), Ok(b)) => a.partial_cmp(&b),
        _ => Some(lhs.cmp(rhs)),
    };
    let Some(order) = order else { return false; };
    use std::cmp::Ordering::{Equal, Greater, Less};
    matches!(
        (op, order),
        (FilterOp::Lt, Less)
            | (FilterOp::Le, Less | Equal)
            | (FilterOp::Gt, Greater)
            | (FilterOp::Ge, Greater | Equal)
    )
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
        let err = CompiledFilter::compile(&fmt, specs, crate::viewport::CaseMode::Sensitive).unwrap_err();
        assert!(err.contains("not in format"), "{err}");
    }

    #[test]
    fn evaluate_eq_matches() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status=500").unwrap()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500, crate::charset::Encoding::utf8()), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200, crate::charset::Encoding::utf8()), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_re_matches_5xx() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status~^5").unwrap()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500, crate::charset::Encoding::utf8()), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200, crate::charset::Encoding::utf8()), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_ne_excludes_200() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status!=200").unwrap()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500, crate::charset::Encoding::utf8()), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200, crate::charset::Encoding::utf8()), FilterMatch::NotMatched);
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
            crate::viewport::CaseMode::Sensitive,
        )
        .unwrap();
        assert_eq!(f.evaluate(SAMPLE_500, crate::charset::Encoding::utf8()), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200, crate::charset::Encoding::utf8()), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_unparseable_line_is_not_parsed() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status=200").unwrap()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert_eq!(f.evaluate(NON_PARSING, crate::charset::Encoding::utf8()), FilterMatch::NotParsed);
    }

    // ----- Comparison operators -----

    #[test]
    fn parse_le_before_lt() {
        let s = FilterSpec::parse("status<=200").unwrap();
        assert_eq!(s.op, FilterOp::Le);
        assert_eq!(s.value, "200");
    }

    #[test]
    fn parse_ge_before_gt() {
        let s = FilterSpec::parse("status>=500").unwrap();
        assert_eq!(s.op, FilterOp::Ge);
        assert_eq!(s.value, "500");
    }

    #[test]
    fn parse_lt() {
        let s = FilterSpec::parse("size<1000").unwrap();
        assert_eq!(s.op, FilterOp::Lt);
        assert_eq!(s.value, "1000");
    }

    #[test]
    fn parse_gt() {
        let s = FilterSpec::parse("size>0").unwrap();
        assert_eq!(s.op, FilterOp::Gt);
        assert_eq!(s.value, "0");
    }

    #[test]
    fn evaluate_ge_numeric() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status>=500").unwrap()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert_eq!(f.evaluate(SAMPLE_500, crate::charset::Encoding::utf8()), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_200, crate::charset::Encoding::utf8()), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_lt_numeric() {
        let fmt = apache_combined();
        let f = CompiledFilter::compile(&fmt, vec![FilterSpec::parse("status<400").unwrap()], crate::viewport::CaseMode::Sensitive).unwrap();
        assert_eq!(f.evaluate(SAMPLE_200, crate::charset::Encoding::utf8()), FilterMatch::Matched);
        assert_eq!(f.evaluate(SAMPLE_500, crate::charset::Encoding::utf8()), FilterMatch::NotMatched);
    }

    #[test]
    fn evaluate_lex_fallback() {
        // `size` of "-" means missing in CLF. Numeric parse fails, lex compare
        // applies: "-" vs "100". Verify lex semantics produce the right answer.
        // ASCII: '-' (0x2D) < '0' (0x30), so "-" < "100" lexicographically.
        assert!(compare(&FilterOp::Lt, "-", "100"));
        assert!(!compare(&FilterOp::Gt, "-", "100"));
    }

    #[test]
    fn evaluate_lex_string_compare() {
        // `level>warn` — both sides are strings, neither numeric.
        assert!(compare(&FilterOp::Gt, "warning", "warn"));
        assert!(!compare(&FilterOp::Gt, "info", "warn"));
        assert!(compare(&FilterOp::Ge, "warn", "warn"));
        assert!(compare(&FilterOp::Le, "warn", "warn"));
    }

    #[test]
    fn parse_rejects_no_op_mentions_new_ops() {
        let err = FilterSpec::parse("status").unwrap_err();
        assert!(err.contains(">=") && err.contains("<="), "{err}");
    }

    #[test]
    fn filter_evaluate_decoded_latin1() {
        // Format with a `user` field that could contain non-ASCII Latin-1 bytes.
        // We build a simple log format: "<ip> <user>" so the user field is the 2nd token.
        let fmt = LogFormat::compile(
            "simple2",
            r"^(?P<ip>\S+) (?P<user>\S+)$",
        )
        .unwrap();
        let f = CompiledFilter::compile(
            &fmt,
            vec![FilterSpec::parse("user=café").unwrap()],
            crate::viewport::CaseMode::Sensitive,
        )
        .unwrap();
        let l1 = crate::charset::parse_label("iso-8859-1").unwrap();
        // "127.0.0.1 café" in Latin-1 (0xE9 = é in iso-8859-1)
        let latin1_line: Vec<u8> = b"127.0.0.1 caf\xE9".to_vec();
        assert_eq!(f.evaluate(&latin1_line, l1), FilterMatch::Matched);
        // Same bytes under UTF-8: 0xE9 is invalid → lossy replacement → user field != "café"
        assert_eq!(
            f.evaluate(&latin1_line, crate::charset::Encoding::utf8()),
            FilterMatch::NotMatched
        );
    }
}
