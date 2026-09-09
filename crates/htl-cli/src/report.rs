//! `--format json`: the same facts the text output prints, as one JSON document on
//! stdout (text goes to stderr, so the two never mix). Field names are stable; new
//! fields may be added, existing ones are not renamed.

use anyhow::Result;
use htl::Fix;
use htl::testing::FileReport;
use serde::Serialize;

// A diagnostic and its severity are the library's ([`htl::Diagnostic`]), and so is the
// one place their text is taken apart: an LSP, a `build.rs` and this printer read the
// same values rather than each splitting the string for itself.
pub use htl::{Diagnostic, Severity};

// The JSON shape of a dependency diagnostic is the run cache's, since the store reads it
// back; `--format json` prints the same shape.
pub use htl::cache::DependencyJson;

/// Where a diagnostic goes once the run has decided to say it: printed as it comes
/// (text) or kept for the document (json).
///
/// Which diagnostics a run says, and in what words, is the library's
/// ([`htl::project::Sink`]) — a dependency's error is said once per run, and a replayed
/// entry comes through the same decisions a fresh check does. What is left here is the
/// part that is genuinely a command line's: how a finding reads on a terminal.
pub struct Out {
    pub json: bool,
    diagnostics: Vec<Diagnostic>,
}

impl Out {
    pub fn new(json: bool) -> Self {
        Self {
            json,
            diagnostics: Vec::new(),
        }
    }

    pub fn take(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.diagnostics)
    }
}

impl htl::project::Output for Out {
    fn diagnostic(
        &mut self,
        severity: Severity,
        text: &str,
        fix: Option<&Fix>,
        dependency: Option<&DependencyJson>,
    ) {
        if self.json {
            self.diagnostics
                .push(htl::project::diagnostic_of(severity, text, fix, dependency));
        } else if let Some(dep) = dependency {
            eprintln!("{severity}: {text}\n  (required by {})", dep.required_by);
        } else {
            // Text mode: say a fix exists, so `htl fix` is discoverable from the output.
            match fix.map(|f| f.applicability.as_str()) {
                // A suggestion is shown by `htl fix` and never written by it; the command
                // that carries the edit is still `htl fix` (`--diff` prints it), and what
                // it does with it is what it says when it runs.
                Some("safe") | Some("suggest") => {
                    eprintln!("{severity}: {text} (fixable: htl fix)")
                }
                Some("unsafe") => eprintln!("{severity}: {text} (fixable: htl fix --unsafe)"),
                _ => eprintln!("{severity}: {text}"),
            }
        }
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
