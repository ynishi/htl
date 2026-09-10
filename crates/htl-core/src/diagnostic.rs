//! What a check reported, as a value rather than as a line of text.
//!
//! The checker formats its findings as `"<file>:<line>:<col>: <message>"`, and a lint
//! adds ` [htl <rule>]`. Every reader that wants the position back — `--format json`,
//! `htl fix`, and whatever comes next (an LSP, `--watch`, a `build.rs` that fails on a
//! lint) — used to take that string apart for itself. [`Diagnostic::parse`] is the one
//! place that does it now, and it lives here, beside the fix and the severity it hands
//! over, rather than in each caller.
//!
//! The text is still what the checker produces and what the run cache stores, so it is
//! parsed here rather than reconstructed: a report and a replay of it print the string
//! the checker wrote, and only readers that ask for the parts pay for the split.

use crate::Fix;
use serde::Serialize;

/// How loud a diagnostic is. `error` fails a check; `warning` and `lint` do not unless
/// the caller promotes them (`htl check --strict`, `include_tl!`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Lint,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Lint => "lint",
        }
    }

    /// The inverse of [`as_str`](Self::as_str). `None` for anything else, which is the
    /// answer a reader of stored diagnostics wants: an entry written by a build that
    /// knew a fourth severity is refused rather than guessed at.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            "lint" => Some(Severity::Lint),
            _ => None,
        }
    }
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `error:` / `warning:` / `lint:` finding, split into its parts.
///
/// The serialized form is what `htl check --format json` prints. Field names are stable;
/// new fields may be added, existing ones are not renamed.
#[derive(Serialize, Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// The file the diagnostic is in, as the report spells it. Empty when the text
    /// carried no position (a failure that is about a file rather than a place in one).
    pub file: String,
    pub line: usize,
    pub col: usize,
    /// The lint rule (`nil-index`, `contract`, ...) for `lint` diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub message: String,
    /// A mechanical rewrite `htl fix` may apply, when the diagnostic has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    /// For an error in a module the check reached through `require`: the file whose
    /// require pulled it in. Absent on the project's own diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required_by: Option<String>,
    /// Where such a file lives: `dependency` (installed under `.htl/modules`, or a
    /// vendored copy) or `external` (a `[check] paths` or contract directory). Absent for
    /// a file of the project's own, and on the project's own diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl Diagnostic {
    /// `"<file>:<line>:<col>: <message>"` — what the checker formats — into its parts.
    /// Text that is not in that shape keeps the whole of itself as the message, with no
    /// file and no position.
    ///
    /// `fix`, `required_by` and `origin` are the caller's to fill in: they are not in the
    /// text, they travel beside it.
    pub fn parse(severity: Severity, text: &str) -> Self {
        let Some((file, line, col, msg)) = position(text) else {
            let (message, rule) = split_rule(text);
            return Self {
                severity,
                file: String::new(),
                line: 0,
                col: 0,
                rule,
                message,
                fix: None,
                required_by: None,
                origin: None,
            };
        };
        let (message, rule) = split_rule(msg.trim_start());
        Self {
            severity,
            file: file.to_string(),
            line,
            col,
            rule,
            message,
            fix: None,
            required_by: None,
            origin: None,
        }
    }
}

/// The `<file>`, `<line>`, `<col>` and the rest of `"<file>:<line>:<col>: <message>"`.
/// `None` when the text is not in that shape.
///
/// The only place a diagnostic's text is taken apart. `file` is a prefix of `text`, so a
/// caller that wants to rewrite the file and keep the rest can slice by its length.
pub fn position(text: &str) -> Option<(&str, usize, usize, &str)> {
    let (file, rest) = text.split_once(':')?;
    let (line, rest) = rest.split_once(':')?;
    let (col, message) = rest.split_once(':')?;
    Some((
        file,
        line.trim().parse().ok()?,
        col.trim().parse().ok()?,
        message,
    ))
}

/// The rule name a finding's text ends with (` [htl <rule>]`), borrowed from it.
///
/// For a caller that has the text and wants only the name: which rule a run reported
/// under, so its level can be asked for. Reading the whole diagnostic is
/// [`Diagnostic::parse`], and both take the suffix apart here.
pub fn rule_of(text: &str) -> Option<&str> {
    if !text.ends_with(']') {
        return None;
    }
    let start = text.rfind(" [htl ")?;
    let rule = &text[start + " [htl ".len()..text.len() - 1];
    (!rule.is_empty() && !rule.contains(' ')).then_some(rule)
}

/// Lint messages end with ` [htl <rule>]`. Splitting it off leaves the message reading
/// as a sentence and the rule available as a name to filter on.
fn split_rule(msg: &str) -> (String, Option<String>) {
    match rule_of(msg) {
        // The suffix is ` [htl ` + the name + `]`: seven characters around it.
        Some(rule) => (
            msg[..msg.len() - rule.len() - 7].to_string(),
            Some(rule.to_string()),
        ),
        None => (msg.to_string(), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_position_and_message() {
        let d = Diagnostic::parse(
            Severity::Error,
            "src/a.tl:12:3: expected string, got integer",
        );
        assert_eq!(d.file, "src/a.tl");
        assert_eq!((d.line, d.col), (12, 3));
        assert_eq!(d.message, "expected string, got integer");
        assert_eq!(d.rule, None);
    }

    #[test]
    fn splits_the_lint_rule_off_the_message() {
        let d = Diagnostic::parse(
            Severity::Lint,
            "src/a.tl:4:1: t.x may be nil [htl nil-index]",
        );
        assert_eq!(d.rule.as_deref(), Some("nil-index"));
        assert_eq!(d.message, "t.x may be nil");
    }

    #[test]
    fn text_without_a_position_keeps_all_of_itself() {
        let d = Diagnostic::parse(Severity::Error, "src/a.tl: generate failed: boom");
        assert_eq!(d.file, "");
        assert_eq!((d.line, d.col), (0, 0));
        assert_eq!(d.message, "src/a.tl: generate failed: boom");
    }

    #[test]
    fn a_bracket_that_is_not_a_rule_stays_in_the_message() {
        let d = Diagnostic::parse(Severity::Lint, "src/a.tl:1:1: see [htl two words]");
        assert_eq!(d.rule, None);
        assert_eq!(d.message, "see [htl two words]");
    }

    #[test]
    fn severity_round_trips_through_its_name() {
        for s in [Severity::Error, Severity::Warning, Severity::Lint] {
            assert_eq!(Severity::parse(s.as_str()), Some(s));
        }
        assert_eq!(Severity::parse("fatal"), None);
    }
}
