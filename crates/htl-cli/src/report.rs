//! `--format json`: the same facts the text output prints, as one JSON document on
//! stdout (text goes to stderr, so the two never mix), and the same exit code as in text
//! mode. Field names are stable; new fields may be added, existing ones are not renamed.

use anyhow::Result;
use htl::testing::FileReport;
use serde::Serialize;

// A diagnostic is the library's ([`htl::Diagnostic`]), and so is its text form (its
// `Display`): an LSP, a `build.rs` and this printer read the same values, and print them
// the one way the library does.
pub use htl::Diagnostic;

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
    fn diagnostic(&mut self, d: &Diagnostic) {
        let severity = d.severity;
        if self.json {
            self.diagnostics.push(d.clone());
        } else if let Some(by) = &d.required_by {
            eprintln!("{severity}: {d}\n  (required by {by})");
        } else {
            let text = d;
            // Text mode: say a fix exists, so `htl fix` is discoverable from the output.
            match d.fix.as_ref().map(|f| f.applicability.as_str()) {
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
    /// Every file the walk visited, a patched dependency's included — what it has always
    /// counted, left alone so that a consumer reading it is not moved by `patched`
    /// arriving beside it.
    pub files: usize,
    /// How many of `files` came out of a `patch_dir` dependency: the project's committed
    /// copy of somebody else's package, which a check reads and neither `htl fmt` nor
    /// `htl test` touches. Subtract it from `files` for the project's own, which is how
    /// the text summary prints the pair: `15 file(s) + 10 in patched dependencies`, the
    /// two adding up to `files`, and the one number alone when this is zero — a project
    /// with no patch reads as it read before the split existed.
    pub patched: usize,
    pub diagnostics: Vec<Diagnostic>,
    pub summary: CheckSummary,
}

#[derive(Serialize, Debug)]
pub struct CheckSummary {
    pub errors: usize,
    pub warnings: usize,
    pub lints: usize,
    /// How many of `warnings` + `lints` were said under a rule the project set to `deny`.
    /// A count of levels, so it overlaps those two rather than adding to them.
    pub denied: usize,
    /// Every `warn` counted as `deny` for this run.
    pub strict: bool,
    /// What the exit code says: no errors and nothing at `deny` — and under `strict`,
    /// nothing reported at all.
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
    /// Runtime error outside any test (the file itself raised): the text `htl test`
    /// prints for it, [`htl::developer_message`] with its frames, carried unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// No test library was used: the file passed by running to completion.
    pub file_level: bool,
    pub passed: usize,
    pub failed: usize,
    /// One entry per failing test: the message and traceback as the text output prints
    /// them under the file, unchanged.
    pub failures: Vec<String>,
    pub tests: Vec<TestCase>,
    pub duration_ms: f64,
    pub snapshots_written: Vec<String>,
    pub snapshots_updated: Vec<String>,
}

/// Paths as a report spells them ([`htl::diagnostic::display_path`]): the text output and
/// the document name one file the same way.
fn spelled(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|p| htl::diagnostic::display_path(std::path::Path::new(p)))
        .collect()
}

impl TestFile {
    pub fn from_report(rep: &FileReport, diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            path: htl::diagnostic::display_path(&rep.path),
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
            snapshots_written: spelled(&rep.snapshots_written),
            snapshots_updated: spelled(&rep.snapshots_updated),
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

// Coverage is the library's too ([`htl::project::coverage_report`]): which modules a run
// reached, which statements of them ran, and the lcov tracefile that says so. The JSON
// shape is these types', printed here beside the rest of the document.
pub use htl::project::CoverageReport;

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
    /// What the project says about itself as a whole — a name with two owners, a require
    /// cycle, a contract problem — which belongs to no one file.
    pub diagnostics: Vec<Diagnostic>,
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
    /// Teal's warnings and htl's lints left in the tree, as `htl check` would count them.
    pub warnings: usize,
    pub lints: usize,
    /// How many of those were said under a rule at `deny`.
    pub denied: usize,
    /// Whether every warning and lint counted as `deny` (`[lint] strict`).
    pub strict: bool,
    pub ok: bool,
}

/// Print one document to stdout.
pub fn emit<T: Serialize>(v: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(v)?);
    Ok(())
}
