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
use std::path::{Component, Path, PathBuf};

/// How loud a diagnostic is. `error` fails a check; `warning` and `lint` do not unless
/// the caller promotes them (`htl check --strict`, `include_tl!`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The file did not type-check. From the Teal checker, and the only severity that
    /// fails a run on its own.
    Error,
    /// A kind the Teal compiler reports for itself — an unused local, a redeclaration —
    /// which htl surfaces under the rule name of its kind (`tl:unused`, ...) rather than
    /// inventing one.
    Warning,
    /// A finding of one of htl's own rules. The only severity whose diagnostics carry a
    /// [`rule`](Diagnostic::rule), because a rule is what a project sets a level for.
    Lint,
}

impl Severity {
    /// The lowercase word this severity is written as: in the `<severity>:` a report
    /// prints, in `--format json`, and in the run cache. [`parse`](Self::parse) reads it
    /// back, so the two are the one spelling that crosses between a run and a replay of it.
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
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Which of the three this is, and so whether a run that reported it fails — the
    /// caller's to decide for a `warning` or a `lint`, settled for an `error`.
    pub severity: Severity,
    /// The file the diagnostic is in, as the report spells it. Empty when the text
    /// carried no position (a failure that is about a file rather than a place in one).
    pub file: String,
    /// The line, counted from 1 as the checker counts it. `0` alongside an empty
    /// [`file`](Self::file): the text carried no position, rather than pointing at a
    /// first line.
    pub line: usize,
    /// The column, counted from 1, and `0` under the same condition as
    /// [`line`](Self::line).
    pub col: usize,
    /// The lint rule (`nil-index`, `contract`, ...) for `lint` diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    /// What the finding says, with the position prefix and the ` [htl <rule>]` suffix
    /// taken off — the sentence alone, so a reader that formats its own line does not
    /// have to unpick one.
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

/// The text form, and the only place it is made: `<file>:<line>:<col>: <message>`, the
/// position left out when there is none, and ` [htl <rule>]` after a warning's or a lint's
/// message. An error's rule is the class its fix is filed under and is not printed: the
/// text of an error has never carried one.
///
/// Every line htl prints about a finding comes from here, whether the finding was just made
/// or read back from the store, so the two cannot drift apart.
impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.file.is_empty() || self.line != 0 {
            write!(f, "{}:{}:{}: ", self.file, self.line, self.col)?;
        }
        f.write_str(&self.message)?;
        if self.severity != Severity::Error
            && let Some(rule) = &self.rule
        {
            write!(f, " [htl {rule}]")?;
        }
        Ok(())
    }
}

impl Diagnostic {
    /// A finding made by htl rather than read from the checker: `file` as the report
    /// should spell it, the 1-based position, the sentence alone, and the rule it is said
    /// under (`None` for an error that is not a lint's). `fix`, `required_by` and `origin`
    /// start empty.
    pub fn new(
        severity: Severity,
        file: impl Into<String>,
        line: usize,
        col: usize,
        message: impl Into<String>,
        rule: Option<&str>,
    ) -> Self {
        Self {
            severity,
            file: file.into(),
            line,
            col,
            rule: rule.map(str::to_string),
            message: message.into(),
            fix: None,
            required_by: None,
            origin: None,
        }
    }

    /// The same finding with its paths — [`file`](Self::file) and
    /// [`required_by`](Self::required_by) — spelled as every report spells a path
    /// ([`display_path`]). The checker names a file as it was handed it (`./src/a.tl`, or
    /// absolute from a search path); a reader that prints a diagnostic takes it through
    /// here first, so a failed `include_tl!`, a junit body and `htl check` name one file
    /// one way.
    pub fn spelled(mut self) -> Self {
        if !self.file.is_empty() {
            self.file = display_path(Path::new(&self.file));
        }
        if let Some(by) = &self.required_by {
            self.required_by = Some(display_path(Path::new(by)));
        }
        self
    }

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

/// Relative to the directory the command ran in when it is under it, normalised absolute
/// when it is not — a dependency outside the project reads better that way than as a stack
/// of `..`.
///
/// Public because it is how *a* path reads in this tool's output, not how a diagnostic's
/// does: `crate::unused` reports files the walk found rather than diagnostics, and the
/// two must spell one file the same way.
pub fn display_path(p: &Path) -> String {
    let Ok(cwd) = std::env::current_dir() else {
        return lexical(p).display().to_string();
    };
    // Against the working directory first: a relative path's leading `..` says where it
    // is from here, and folding it without that — `../src/a.tl` to `src/a.tl` — names a
    // different file.
    let norm = lexical(&cwd.join(p));
    match norm.strip_prefix(&cwd) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => norm.display().to_string(),
    }
}

/// Fold `.` and `..` without touching the filesystem.
///
/// [`std::fs::canonicalize`] would resolve symlinks too, and `.htl/modules/entries/<dep>`
/// is one: following it names mlua-pkg's cache directory rather than the dependency, which
/// is the opposite of what a report wants to say.
fn lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c.as_os_str()),
        }
    }
    out
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

    #[test]
    fn spelled_folds_the_file_and_the_requirer_and_leaves_a_positionless_one_alone() {
        let cwd = std::env::current_dir().unwrap();
        let mut d = Diagnostic::new(Severity::Error, "./src/x/../a.tl", 1, 2, "boom", None);
        d.required_by = Some(cwd.join("src/b.tl").display().to_string());
        let d = d.spelled();
        assert_eq!(d.file, "src/a.tl");
        assert_eq!(d.required_by.as_deref(), Some("src/b.tl"));
        assert_eq!(d.to_string(), "src/a.tl:1:2: boom");

        let bare = Diagnostic::new(Severity::Error, "", 0, 0, "a.tl: generate failed", None);
        assert_eq!(bare.clone().spelled(), bare);
    }
}
