//! `--format json`: the same facts the text output prints, as one JSON document on
//! stdout (text goes to stderr, so the two never mix). Field names are stable; new
//! fields may be added, existing ones are not renamed.

use anyhow::Result;
use htl::CheckInfo;
use htl::testing::FileReport;
use serde::Serialize;

/// One `error:` / `warning:` / `lint:` line, split into its parts.
#[derive(Serialize, Debug, Clone)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub file: String,
    pub line: usize,
    pub col: usize,
    /// The lint rule (`nil-index`, `contract`, ...) for `lint` diagnostics.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub message: String,
    /// A mechanical rewrite `htl fix` may apply, when the diagnostic has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<FixJson>,
}

#[derive(Serialize, Debug, Clone)]
pub struct FixJson {
    /// `safe` / `unsafe` / `suggest`.
    pub applicability: &'static str,
    pub edits: Vec<EditJson>,
}

#[derive(Serialize, Debug, Clone)]
pub struct EditJson {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub text: String,
}

impl FixJson {
    pub fn from_fix(f: &htl::Fix) -> Self {
        Self {
            applicability: f.applicability.as_str(),
            edits: f
                .edits
                .iter()
                .map(|e| EditJson { line: e.line, col: e.col, end_line: e.end_line, end_col: e.end_col, text: e.text.clone() })
                .collect(),
        }
    }
}

/// `"<file>:<line>:<col>: <message>"` (what the checker formats) into parts. A line
/// that does not have that shape keeps its whole text as the message.
pub fn parse_diag(severity: &'static str, text: &str) -> Diagnostic {
    let mut parts = text.splitn(4, ':');
    if let (Some(file), Some(l), Some(c), Some(msg)) = (parts.next(), parts.next(), parts.next(), parts.next())
        && let (Ok(line), Ok(col)) = (l.trim().parse::<usize>(), c.trim().parse::<usize>())
    {
        let (message, rule) = split_rule(msg.trim_start());
        return Diagnostic { severity, file: file.to_string(), line, col, rule, message, fix: None };
    }
    let (message, rule) = split_rule(text);
    Diagnostic { severity, file: String::new(), line: 0, col: 0, rule, message, fix: None }
}

/// Lint lines end with ` [htl <rule>]`.
fn split_rule(msg: &str) -> (String, Option<String>) {
    if msg.ends_with(']')
        && let Some(start) = msg.rfind(" [htl ")
    {
        let rule = &msg[start + " [htl ".len()..msg.len() - 1];
        if !rule.is_empty() && !rule.contains(' ') {
            return (msg[..start].to_string(), Some(rule.to_string()));
        }
    }
    (msg.to_string(), None)
}

/// Where diagnostics go: printed as they come (text) or kept for the document (json).
pub struct Sink {
    pub json: bool,
    pub diagnostics: Vec<Diagnostic>,
}

impl Sink {
    pub fn new(json: bool) -> Self {
        Self { json, diagnostics: Vec::new() }
    }

    pub fn diag(&mut self, severity: &'static str, text: &str) {
        if self.json {
            self.diagnostics.push(parse_diag(severity, text));
        } else {
            eprintln!("{severity}: {text}");
        }
    }

    /// Same order as the text output has always used: warnings, lints, errors.
    pub fn checkinfo(&mut self, c: &CheckInfo) {
        for w in &c.warnings {
            self.diag("warning", w);
        }
        for (i, l) in c.lints.iter().enumerate() {
            self.diag_with_fix("lint", l, c.lint_fixes.get(i).and_then(|f| f.as_ref()));
        }
        for (i, e) in c.errors.iter().enumerate() {
            self.diag_with_fix("error", e, c.error_fixes.get(i).and_then(|f| f.as_ref()));
        }
    }

    fn diag_with_fix(&mut self, severity: &'static str, text: &str, fix: Option<&htl::Fix>) {
        if self.json {
            let mut d = parse_diag(severity, text);
            d.fix = fix.map(FixJson::from_fix);
            self.diagnostics.push(d);
        } else {
            // Text mode: say a fix exists, so `htl fix` is discoverable from the output.
            match fix.map(|f| f.applicability) {
                Some(htl::Applicability::Safe) => eprintln!("{severity}: {text} (fixable: htl fix)"),
                Some(htl::Applicability::Unsafe) => eprintln!("{severity}: {text} (fixable: htl fix --unsafe)"),
                _ => eprintln!("{severity}: {text}"),
            }
        }
    }

    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }
}

#[derive(Serialize, Debug)]
pub struct CheckReport {
    pub files: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub summary: CheckSummary,
}

#[derive(Serialize, Debug)]
pub struct CheckSummary {
    pub errors: usize,
    pub warnings: usize,
    pub lints: usize,
    pub strict: bool,
    /// What the exit code says: no errors, and under `strict` no warnings or lints.
    pub ok: bool,
}

#[derive(Serialize, Debug)]
pub struct TestCase {
    pub name: String,
    pub ok: bool,
    pub ms: f64,
}

#[derive(Serialize, Debug)]
pub struct TestFile {
    pub path: String,
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
    /// Runtime error outside any test (the file itself raised).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// No test library was used: the file passed by running to completion.
    pub file_level: bool,
    pub passed: usize,
    pub failed: usize,
    pub failures: Vec<String>,
    pub tests: Vec<TestCase>,
    pub duration_ms: f64,
    pub snapshots_written: Vec<String>,
    pub snapshots_updated: Vec<String>,
}

impl TestFile {
    pub fn from_report(rep: &FileReport, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            path: rep.path.display().to_string(),
            ok: rep.ok(),
            diagnostics,
            error: rep.error.clone(),
            file_level: rep.file_level,
            passed: rep.passed,
            failed: rep.failed,
            failures: rep.failures.clone(),
            tests: rep.tests.iter().map(|t| TestCase { name: t.name.clone(), ok: t.ok, ms: t.ms }).collect(),
            duration_ms: rep.duration_ms,
            snapshots_written: rep.snapshots_written.clone(),
            snapshots_updated: rep.snapshots_updated.clone(),
        }
    }
}

#[derive(Serialize, Debug)]
pub struct TestSummary {
    /// Files discovered.
    pub files: usize,
    /// Files actually run (`--fail-fast` may stop early).
    pub files_run: usize,
    pub passed: usize,
    pub failed: usize,
    pub files_with_errors: usize,
    pub duration_ms: f64,
    pub ok: bool,
}

#[derive(Serialize, Debug)]
pub struct CoverageModule {
    pub path: String,
    pub executed: usize,
    pub total: usize,
    /// Unexecuted statements as `[first_line, last_line]` ranges.
    pub unexecuted: Vec<(usize, usize)>,
}

#[derive(Serialize, Debug, Default)]
pub struct CoverageReport {
    pub modules: Vec<CoverageModule>,
    pub executed: usize,
    pub total: usize,
}

#[derive(Serialize, Debug)]
pub struct TestReport {
    pub files: Vec<TestFile>,
    pub summary: TestSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<CoverageReport>,
}

#[derive(Serialize, Debug)]
pub struct FixApplied {
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub applicability: &'static str,
    pub pass: usize,
}

#[derive(Serialize, Debug)]
pub struct FixSkipped {
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub reason: String,
}

#[derive(Serialize, Debug)]
pub struct FixFile {
    pub path: String,
    pub changed: bool,
    pub deferred: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oscillation: Option<String>,
    /// Diagnostics after fixing.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Serialize, Debug)]
pub struct FixReport {
    pub dry_run: bool,
    pub applied: Vec<FixApplied>,
    pub skipped: Vec<FixSkipped>,
    pub files: Vec<FixFile>,
    pub summary: FixSummary,
}

#[derive(Serialize, Debug)]
pub struct FixSummary {
    pub files: usize,
    pub files_changed: usize,
    pub applied: usize,
    pub skipped: usize,
    pub deferred: usize,
    pub reverted: usize,
    pub errors_remaining: usize,
    pub ok: bool,
}

/// Print one document to stdout.
pub fn emit<T: Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}
