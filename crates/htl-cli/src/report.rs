//! `--format json`: the same facts the text output prints, as one JSON document on
//! stdout (text goes to stderr, so the two never mix). Field names are stable; new
//! fields may be added, existing ones are not renamed.

use anyhow::Result;
use htl::testing::FileReport;
use htl::{CheckInfo, Fix};
use serde::Serialize;
use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};

// A diagnostic and its severity are the library's ([`htl::Diagnostic`]), and so is the
// one place their text is taken apart: an LSP, a `build.rs` and this printer read the
// same values rather than each splitting the string for itself.
pub use htl::{Diagnostic, Severity};

// The JSON shape of a dependency diagnostic is the run cache's, since the store reads it
// back; `--format json` prints the same shape.
pub use htl::cache::{DependencyJson, FixJson};

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

    pub fn diag(&mut self, severity: Severity, text: &str) {
        self.diag_with_fix(severity, text, None);
    }

    /// Same order as the text output has always used: warnings, lints, errors.
    pub fn checkinfo(&mut self, c: &CheckInfo) {
        for w in &c.warnings {
            self.diag(Severity::Warning, w);
        }
        for (i, l) in c.lints.iter().enumerate() {
            self.diag_with_fix(
                Severity::Lint,
                l,
                c.lint_fixes.get(i).and_then(|f| f.as_ref()),
            );
        }
        for (i, e) in c.errors.iter().enumerate() {
            self.diag_with_fix(
                Severity::Error,
                e,
                c.error_fixes.get(i).and_then(|f| f.as_ref()),
            );
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
                severity: Severity::Error.as_str().to_string(),
                text: e.text.clone(),
                fix: None,
                dependency: Some(dep.clone()),
            });
            self.emit(Severity::Error, &e.text, None, Some(&dep));
        }
    }

    /// Dependency errors this run reported (after the once-per-run rule), for the totals.
    pub fn dependency_error_count(&self) -> usize {
        self.dependency_errors
    }

    fn diag_with_fix(&mut self, severity: Severity, text: &str, fix: Option<&Fix>) {
        self.recorded.push(crate::cache::Recorded {
            severity: severity.as_str().to_string(),
            text: text.to_string(),
            fix: fix.map(FixJson::from_fix),
            dependency: None,
        });
        self.emit(severity, text, fix, None);
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
        severity: Severity,
        text: &str,
        fix: Option<&Fix>,
        dependency: Option<&DependencyJson>,
    ) {
        if let Some(d) = dependency {
            let file = canonical(std::path::Path::new(&d.file));
            if self.walked.contains(&file) || !self.reported.insert(file) {
                return;
            }
            self.dependency_errors += 1;
        }
        // A dependency's path is the one that does not read like the rest of the report:
        // it came from the resolver rather than from the command line. The cache keeps what
        // the checker said and this writes it for the reader, so an entry replayed from
        // another directory still reads against that one.
        let text = match dependency {
            Some(_) => shown(text),
            None => Cow::Borrowed(text),
        };
        let text = text.as_ref();
        if self.json {
            let mut d = Diagnostic::parse(severity, text);
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
            .map(|r| match Severity::parse(&r.severity) {
                Some(s) => Ok(s),
                None => anyhow::bail!("cache entry has an unknown severity: {}", r.severity),
            })
            .collect::<Result<Vec<Severity>>>()?;
        for (severity, r) in severities.into_iter().zip(recorded) {
            let fix = r.fix.as_ref().map(FixJson::to_fix);
            self.emit(severity, &r.text, fix.as_ref(), r.dependency.as_ref());
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

/// A dependency's diagnostic with its file written against the directory the command ran
/// in, the way the rest of the report already reads. Everything else is left alone.
///
/// A dependency's path comes from the resolver, which searches in absolute paths, and a
/// `[check] paths` entry keeps the `..` it was joined through — so a report would otherwise
/// carry `src/area.tl` and `/home/me/proj/../ext/extmod.tl` side by side, and
/// `--format json` would give two `file` spellings for what may be one directory. The walk
/// and `required_by` carry what the command line said, which is theirs to keep.
fn shown(text: &str) -> Cow<'_, str> {
    // The same reading of the text [`Diagnostic::parse`] makes, and the same one place
    // making it: a text with no position keeps every character it has.
    let Some((file, _, _, _)) = htl::diagnostic::position(text) else {
        return Cow::Borrowed(text);
    };
    let shown = display_path(Path::new(file));
    if shown == file {
        return Cow::Borrowed(text);
    }
    Cow::Owned(format!("{shown}{}", &text[file.len()..]))
}

/// Relative to the directory the command ran in when it is under it, normalised absolute
/// when it is not — a dependency outside the project reads better that way than as a stack
/// of `..`.
fn display_path(p: &Path) -> String {
    let norm = lexical(p);
    match std::env::current_dir()
        .ok()
        .and_then(|cwd| norm.strip_prefix(cwd).ok().map(Path::to_path_buf))
    {
        Some(rel) => rel.display().to_string(),
        None => norm.display().to_string(),
    }
}

/// Fold `.` and `..` without touching the filesystem.
///
/// [`std::fs::canonicalize`] would resolve symlinks too, and `.htl/modules/vendored/<dep>`
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
