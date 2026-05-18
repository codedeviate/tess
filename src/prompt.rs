//! Status-line prompt customization. Wraps the existing `DisplayTemplate`
//! parser from `format.rs`, validating against a fixed set of prompt-only
//! placeholder names. Rendered against a `PromptContext` populated by the
//! viewport on every frame.

use crate::format::DisplayTemplate;

/// All placeholders that resolve in a prompt template. Validation against
/// this list happens at parse time; unknown placeholders produce a clear
/// startup error.
const PROMPT_FIELDS: &[&str] = &[
    "label",
    "top",
    "bottom",
    "total",
    "pct",
    "rec-top",
    "rec-bottom",
    "rec-total",
    "rec-block",
    "wrap-offset",
    "format-tag",
    "filter-tag",
    "grep-tag",
    "hide-tag",
    "search-tag",
    "pretty-tag",
    "live-tag",
    "follow-tag",
    "preprocess-failed-tag",
];

#[derive(Debug, Clone)]
pub struct ParsedPrompt {
    template: DisplayTemplate,
}

impl ParsedPrompt {
    /// Parse a prompt template. Validates that all `<field>` placeholders
    /// are known prompt fields. Returns the parse error on failure.
    pub fn parse(source: &str) -> Result<Self, String> {
        let field_names: Vec<String> =
            PROMPT_FIELDS.iter().map(|s| s.to_string()).collect();
        let template = DisplayTemplate::compile(source, &field_names)?;
        Ok(Self { template })
    }

    /// Render the prompt against a context. Missing fields render as empty.
    pub fn render(&self, ctx: &PromptContext) -> String {
        self.template.render(|name| ctx.lookup(name))
    }

    pub fn source(&self) -> &str {
        self.template.source()
    }
}

/// All data the prompt template can resolve. Populated by the viewport
/// once per frame and passed to `ParsedPrompt::render`.
#[derive(Debug, Default)]
pub struct PromptContext {
    pub label: String,
    pub top: usize,
    pub bottom: usize,
    pub total: usize,
    pub pct: u8,
    pub rec_top: usize,
    pub rec_bottom: usize,
    pub rec_total: usize,
    pub records_mode: bool,
    pub wrap_offset: String,
    pub format_tag: String,
    pub filter_tag: String,
    pub grep_tag: String,
    pub hide_tag: String,
    pub search_tag: String,
    pub pretty_tag: String,
    pub live_tag: String,
    pub follow_tag: String,
    pub preprocess_failed_tag: String,
}

impl PromptContext {
    fn lookup(&self, name: &str) -> Option<String> {
        match name {
            "label" => Some(self.label.clone()),
            "top" => Some(self.top.to_string()),
            "bottom" => Some(self.bottom.to_string()),
            "total" => Some(self.total.to_string()),
            "pct" => Some(self.pct.to_string()),
            "rec-top" => Some(self.rec_top.to_string()),
            "rec-bottom" => Some(self.rec_bottom.to_string()),
            "rec-total" => Some(self.rec_total.to_string()),
            "rec-block" => Some(if self.records_mode {
                format!(
                    "L{}-{}/{}  R{}-{}/{}",
                    self.top, self.bottom, self.total,
                    self.rec_top, self.rec_bottom, self.rec_total,
                )
            } else {
                format!("{}-{}/{}", self.top, self.bottom, self.total)
            }),
            "wrap-offset" => Some(self.wrap_offset.clone()),
            "format-tag" => Some(self.format_tag.clone()),
            "filter-tag" => Some(self.filter_tag.clone()),
            "grep-tag" => Some(self.grep_tag.clone()),
            "hide-tag" => Some(self.hide_tag.clone()),
            "search-tag" => Some(self.search_tag.clone()),
            "pretty-tag" => Some(self.pretty_tag.clone()),
            "live-tag" => Some(self.live_tag.clone()),
            "follow-tag" => Some(self.follow_tag.clone()),
            "preprocess-failed-tag" => Some(self.preprocess_failed_tag.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_literal_only_template() {
        let p = ParsedPrompt::parse("hello").unwrap();
        let ctx = PromptContext::default();
        assert_eq!(p.render(&ctx), "hello");
    }

    #[test]
    fn parse_field_template() {
        let p = ParsedPrompt::parse("<label> <pct>%").unwrap();
        let ctx = PromptContext {
            label: "file.log".into(),
            pct: 42,
            ..Default::default()
        };
        assert_eq!(p.render(&ctx), "file.log 42%");
    }

    #[test]
    fn parse_rejects_unknown_field() {
        let err = ParsedPrompt::parse("<bogus>").unwrap_err();
        assert!(err.contains("bogus"), "error mentions field name: {err}");
    }

    #[test]
    fn parse_handles_escaped_left_angle() {
        // `\<` is the escape for a literal `<`; `>` outside a field is always literal.
        let p = ParsedPrompt::parse(r"\<not a field>").unwrap();
        let ctx = PromptContext::default();
        assert_eq!(p.render(&ctx), "<not a field>");
    }

    #[test]
    fn parse_handles_escaped_backslash() {
        let p = ParsedPrompt::parse(r"a\\b").unwrap();
        let ctx = PromptContext::default();
        assert_eq!(p.render(&ctx), "a\\b");
    }

    #[test]
    fn render_resolves_empty_tags_to_nothing() {
        let p = ParsedPrompt::parse("<label><filter-tag><grep-tag>").unwrap();
        let ctx = PromptContext { label: "x".into(), ..Default::default() };
        assert_eq!(p.render(&ctx), "x");
    }

    #[test]
    fn render_resolves_populated_tags() {
        let p = ParsedPrompt::parse("<grep-tag><hide-tag>").unwrap();
        let ctx = PromptContext {
            grep_tag: "  [grep]".into(),
            hide_tag: "  [hide]".into(),
            ..Default::default()
        };
        assert_eq!(p.render(&ctx), "  [grep]  [hide]");
    }

    #[test]
    fn rec_block_renders_records_mode_form() {
        let p = ParsedPrompt::parse("<rec-block>").unwrap();
        let ctx = PromptContext {
            top: 1, bottom: 3, total: 3,
            rec_top: 1, rec_bottom: 2, rec_total: 2,
            records_mode: true,
            ..Default::default()
        };
        assert_eq!(p.render(&ctx), "L1-3/3  R1-2/2");
    }

    #[test]
    fn rec_block_renders_line_mode_form() {
        let p = ParsedPrompt::parse("<rec-block>").unwrap();
        let ctx = PromptContext {
            top: 1, bottom: 3, total: 3,
            records_mode: false,
            ..Default::default()
        };
        assert_eq!(p.render(&ctx), "1-3/3");
    }

    #[test]
    fn render_preprocess_failed_tag_resolves_when_populated() {
        let p = ParsedPrompt::parse("<preprocess-failed-tag>").unwrap();
        let ctx = PromptContext {
            preprocess_failed_tag: "  [preprocess-failed: bad cmd]".into(),
            ..Default::default()
        };
        assert_eq!(p.render(&ctx), "  [preprocess-failed: bad cmd]");
    }
}
