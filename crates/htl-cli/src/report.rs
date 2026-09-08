//! `--format json`: the same facts the text output prints, as one JSON document on
//! stdout (text goes to stderr, so the two never mix). Field names are stable; new
//! fields may be added, existing ones are not renamed.

use anyhow::Result;
use htl::CheckInfo;
use htl::testing::FileReport;
use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FixJson {
    /// `safe` / `unsafe` / `suggest`. Owned rather than `&'static str` because the run
    /// cache reads these back (`crate::cache`), and a borrowed field cannot be
    /// deserialized into. The JSON is unchanged either way.
    pub applicability: String,
    pub edits: Vec<EditJson>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
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
            applicability: f.applicability.as_str().to_string(),
            edits: f
                .edits
                .iter()
                .map(|e| EditJson {
                    line: e.line,
                    col: e.col,
                    end_line: e.end_line,
                    end_col: e.end_col,
                    text: e.text.clone(),
                })
                .collect(),
        }
    }
}

/// `"<file>:<line>:<col>: <message>"` (what the checker formats) into parts. A line
/// that does not have that shape keeps its whole text as the message.
pub fn parse_diag(severity: &'static str, text: &str) -> Diagnostic {
    let mut parts = text.splitn(4, ':');
    if let (Some(file), Some(l), Some(c), Some(msg)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
        && let (Ok(line), Ok(col)) = (l.trim().parse::<usize>(), c.trim().parse::<usize>())
    {
        let (message, rule) = split_rule(msg.trim_start());
        return Diagnostic {
            severity,
            file: file.to_string(),
            line,
            col,
            rule,
            message,
            fix: None,
            required_by: None,
            origin: None,
        };
    }
    let (message, rule) = split_rule(text);
    Diagnostic {
        severity,
        file: String::new(),
        line: 0,
        col: 0,
        rule,
        message,
        fix: None,
        required_by: None,
        origin: None,
    }
}

/// What a dependency's diagnostic carries besides its text: the file that required it
/// and where the file lives. Stored with the diagnostic (`crate::cache::Recorded`) so a
/// replay says exactly what the run said, and decides the same way whether to say it.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DependencyJson {
    /// The file the error is in, as the checker found it.
    pub file: String,
    pub required_by: String,
    /// `dependency` / `external`, or none for a file of the project's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
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
///
/// Everything handed over is also recorded verbatim, so one run can be stored and a
/// later one replayed through this same code (`crate::cache`). Replaying through the
/// printing path, rather than through a reconstruction of what it printed, is what makes
/// "a cached run prints what the original printed" a property of the code instead of a
/// promise in a comment.
pub struct Sink {
    pub json: bool,
    pub diagnostics: Vec<Diagnostic>,
    recorded: Vec<crate::cache::Recorded>,
    /// The files this run checks in their own right, canonical. A dependency error in one
    /// of them is that file's to report, and is not said a second time on behalf of a file
    /// that required it.
    walked: std::collections::HashSet<std::path::PathBuf>,
    /// Dependencies whose errors this run has already said, canonical. A module thirty
    /// files require is reported once, against the first of them.
    reported: std::collections::HashSet<std::path::PathBuf>,
    /// How many dependency errors reached the output — what the totals and the exit code
    /// count, as opposed to how many were handed over (the entries store every one).
    dependency_errors: usize,
}

impl Sink {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            diagnostics: Vec::new(),
            recorded: Vec::new(),
            walked: Default::default(),
            reported: Default::default(),
            dependency_errors: 0,
        }
    }

    /// The files the run checks itself. Their errors are reported as their own, so a
    /// dependency error pointing at one of them is dropped here rather than said twice.
    pub fn walking(&mut self, files: &[std::path::PathBuf]) {
        self.walked = files.iter().map(|f| canonical(f)).collect();
    }

    pub fn diag(&mut self, severity: &'static str, text: &str) {
        self.diag_with_fix(severity, text, None);
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

    /// The errors in what a file required, as errors, each against the file that required
    /// it. `origin_of` says where the dependency lives (`dependency` / `external` / the
    /// project's own).
    ///
    /// Every one is recorded, so the file's cache entry carries them all and a later run
    /// that replays only this file still hears about its dependency. Which of them are
    /// printed is decided at output time, once per run — see [`emit`](Self::emit).
    ///
    /// No fix rides along even when the checker found one: a fix under `.htl/` is
    /// overwritten at the next install, and one under a `[check] paths` directory is
    /// outside the project. `htl fix` never writes there, so the output does not say it can.
    pub fn dependency_errors(
        &mut self,
        c: &CheckInfo,
        origin_of: &dyn Fn(&std::path::Path) -> Option<&'static str>,
    ) {
        for e in &c.dependency_errors {
            let dep = DependencyJson {
                file: e.file.display().to_string(),
                required_by: e.required_by.display().to_string(),
                origin: origin_of(&e.file).map(str::to_string),
            };
            self.recorded.push(crate::cache::Recorded {
                severity: "error".to_string(),
                text: e.text.clone(),
                fix: None,
                dependency: Some(dep.clone()),
            });
            self.emit("error", &e.text, None, Some(&dep));
        }
    }

    /// Dependency errors this run reported (after the once-per-run rule), for the totals.
    pub fn dependency_error_count(&self) -> usize {
        self.dependency_errors
    }

    fn diag_with_fix(&mut self, severity: &'static str, text: &str, fix: Option<&htl::Fix>) {
        let fix = fix.map(FixJson::from_fix);
        self.recorded.push(crate::cache::Recorded {
            severity: severity.to_string(),
            text: text.to_string(),
            fix: fix.clone(),
            dependency: None,
        });
        self.emit(severity, text, fix.as_ref(), None);
    }

    /// The single place a diagnostic becomes output, whether it was just produced or
    /// recovered from the cache.
    ///
    /// A dependency's error is said once per run: not at all when the dependency is one
    /// of the files being checked (it reports its own), and not again after the first
    /// file that required it. Deciding here, rather than where the diagnostic was made,
    /// is what makes a replayed entry and a fresh check agree — both come through this.
    fn emit(
        &mut self,
        severity: &'static str,
        text: &str,
        fix: Option<&FixJson>,
        dependency: Option<&DependencyJson>,
    ) {
        if let Some(d) = dependency {
            let file = canonical(std::path::Path::new(&d.file));
            if self.walked.contains(&file) || !self.reported.insert(file) {
                return;
            }
            self.dependency_errors += 1;
        }
        if self.json {
            let mut d = parse_diag(severity, text);
            d.fix = fix.cloned();
            if let Some(dep) = dependency {
                d.required_by = Some(dep.required_by.clone());
                d.origin = dep.origin.clone();
            }
            self.diagnostics.push(d);
        } else if let Some(dep) = dependency {
            eprintln!("{severity}: {text}\n  (required by {})", dep.required_by);
        } else {
            // Text mode: say a fix exists, so `htl fix` is discoverable from the output.
            match fix.map(|f| f.applicability.as_str()) {
                Some("safe") => eprintln!("{severity}: {text} (fixable: htl fix)"),
                Some("unsafe") => eprintln!("{severity}: {text} (fixable: htl fix --unsafe)"),
                _ => eprintln!("{severity}: {text}"),
            }
        }
    }

    /// Print a run recovered from the cache.
    ///
    /// An entry carrying a severity this build does not know is refused rather than
    /// guessed at; the caller then runs the check, which is the right answer for a store
    /// written by something else.
    pub fn replay(&mut self, recorded: &[crate::cache::Recorded]) -> Result<()> {
        // Validate the whole entry before printing any of it. Text mode prints as it
        // goes, so a replay that gave up halfway would leave those lines on the terminal
        // and the caller — which falls back to running the check — would print them a
        // second time.
        let severities = recorded
            .iter()
            .map(|r| match r.severity.as_str() {
                "error" => Ok("error"),
                "warning" => Ok("warning"),
                "lint" => Ok("lint"),
                other => anyhow::bail!("cache entry has an unknown severity: {other}"),
            })
            .collect::<Result<Vec<&'static str>>>()?;
        for (severity, r) in severities.into_iter().zip(recorded) {
            self.emit(severity, &r.text, r.fix.as_ref(), r.dependency.as_ref());
        }
        Ok(())
    }

    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }

    /// What this run said, in the form the cache stores it.
    pub fn take_recorded(&mut self) -> Vec<crate::cache::Recorded> {
        std::mem::take(&mut self.recorded)
    }
}

/// One spelling of a file for the once-per-run rule: the checker names a dependency by
/// the search-path template that found it, the walk names a file as it was given.
fn canonical(p: &std::path::Path) -> std::path::PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
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
    /// Every module came from the cache, so no checker was built. The diagnostics are the
    /// same either way; this says nothing ran to produce them.
    pub cached: bool,
    /// How many of `files` were replayed rather than checked. `cached` is this reaching
    /// `files`; between the two you can tell a wholly cached run from a mostly cached one.
    pub replayed: usize,
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
            tests: rep
                .tests
                .iter()
                .map(|t| TestCase {
                    name: t.name.clone(),
                    ok: t.ok,
                    ms: t.ms,
                })
                .collect(),
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
    /// Files whose check and codegen came from the cache. They still ran: only the work
    /// before the run is reusable.
    pub replayed: usize,
    pub duration_ms: f64,
    pub ok: bool,
    /// The run's random seed. Passing it back as `--seed` repeats what every file drew.
    pub seed: u64,
}

/// A function of a module no statement of which ran.
#[derive(Serialize, Debug)]
pub struct NeverRan {
    /// As the source writes it: `f`, `M.f`, `M:f`.
    pub name: String,
    pub line: usize,
}

#[derive(Serialize, Debug)]
pub struct CoverageModule {
    pub path: String,
    pub executed: usize,
    pub total: usize,
    /// Unexecuted statements as `[first_line, last_line]` ranges.
    pub unexecuted: Vec<(usize, usize)>,
    /// Functions nothing in the run entered. A percentage says how much of a module
    /// was missed; this says what was missed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub never_ran: Vec<NeverRan>,
    /// The module's file, absolute. Not part of the JSON (`path` is the reported
    /// spelling); the lcov writer resolves it against the project root instead.
    #[serde(skip)]
    pub source: std::path::PathBuf,
    /// Every statement as `(first line, ran)`, in source order. What `executed` /
    /// `total` count, kept for the lcov `DA` records.
    #[serde(skip)]
    pub statements: Vec<(usize, bool)>,
    /// Every function with a body as `(name, line, entered)`, in source order;
    /// `never_ran` is the `entered == false` subset.
    #[serde(skip)]
    pub functions: Vec<(String, usize, bool)>,
}

#[derive(Serialize, Debug, Default)]
pub struct CoverageReport {
    pub modules: Vec<CoverageModule>,
    pub executed: usize,
    pub total: usize,
}

impl CoverageReport {
    /// The run as an lcov tracefile, one record per module in the report's order.
    ///
    /// `DA` is one entry per line a statement starts on, with a count of `1` or `0`:
    /// the hook records whether a line ran, not how often, and a number it does not
    /// have is not invented. Two statements starting on one line share the entry, so
    /// `LF` / `LH` equal the table's `total` / `executed` except on such lines. `FN` /
    /// `FNDA` are the classic two-field forms every consumer reads; there is no branch
    /// data, so no `BRDA`. `SF` is relative to `root` (the project root, so the file
    /// resolves against the repository wherever CI ran the command), absolute when the
    /// module is outside it.
    pub fn lcov(&self, root: &std::path::Path) -> String {
        use std::collections::BTreeMap;
        use std::fmt::Write as _;
        let mut out = String::new();
        for m in &self.modules {
            let sf = m
                .source
                .strip_prefix(root)
                .unwrap_or(&m.source)
                .to_string_lossy();
            out.push_str("TN:\n");
            let _ = writeln!(out, "SF:{sf}");
            for (name, line, _) in &m.functions {
                let _ = writeln!(out, "FN:{line},{name}");
            }
            for (name, _, entered) in &m.functions {
                let _ = writeln!(out, "FNDA:{},{name}", u8::from(*entered));
            }
            let _ = writeln!(out, "FNF:{}", m.functions.len());
            let _ = writeln!(
                out,
                "FNH:{}",
                m.functions.iter().filter(|(_, _, e)| *e).count()
            );
            let mut lines: BTreeMap<usize, bool> = BTreeMap::new();
            for &(line, ran) in &m.statements {
                *lines.entry(line).or_default() |= ran;
            }
            for (line, ran) in &lines {
                let _ = writeln!(out, "DA:{line},{}", u8::from(*ran));
            }
            let _ = writeln!(out, "LF:{}", lines.len());
            let _ = writeln!(out, "LH:{}", lines.values().filter(|r| **r).count());
            out.push_str("end_of_record\n");
        }
        out
    }
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
