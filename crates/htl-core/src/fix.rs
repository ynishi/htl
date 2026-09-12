//! Applying the fixes that diagnostics carry (`htl fix`).
//!
//! The shape follows what cargo fix, ESLint and Ruff settled on: a fix travels with
//! its diagnostic and has an applicability class; only `safe` applies unless asked;
//! edits that overlap within one pass are deferred to the next, which re-checks the
//! file; passes are capped; a fix that leaves the file with an error it did not have
//! is reverted; everything applied is reported, not only what remains. A file the
//! parser rejects is never touched; type errors do not block (their positions are
//! sound, and a fix may be what removes them), the revert is the guard.

use crate::{Applicability, CheckInfo, Diagnostic, Edit, Htl};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Passes per file before giving up (cargo fix uses 4).
pub const MAX_PASSES: usize = 4;

/// What a run of [`fix_file`] is allowed to do: which fixes count as applicable, which
/// rules are in scope, and whether anything is written.
///
/// The default is the conservative one — safe fixes of every rule, written — so a caller
/// that sets nothing gets what `htl fix` with no flags does.
#[derive(Debug, Clone, Default)]
pub struct FixOptions {
    /// Apply `unsafe` fixes too.
    pub unsafe_fixes: bool,
    /// Rules promoted to safe by `[fix] unsafe` in htl.toml.
    pub promoted: Vec<String>,
    /// Rules whose fixes are never applied (`[fix] disable`).
    pub disabled: Vec<String>,
    /// Only these rules (`--rule a,b`); empty = all.
    pub only: Vec<String>,
    /// Compute everything, write nothing.
    pub dry_run: bool,
}

impl FixOptions {
    /// Refuse a rule name none of these filters could ever match.
    ///
    /// The filters are string comparisons against the name a candidate was filed under
    /// (the rule a diagnostic is filed under), so a name that is not a rule quietly matches nothing: `--rule
    /// nil-idex` would report a run that fixed nothing, which is what a project with
    /// nothing to fix also reports. The names come from a person either way — a flag or
    /// `[fix]` in `htl.toml` — so they are held to the registry the way a lint spec is
    /// ([`crate::lint::check_fix_rules`]).
    pub fn validate(&self) -> Result<()> {
        crate::lint::check_fix_rules(&self.only, "htl fix --rule")?;
        crate::lint::check_fix_rules(&self.disabled, "[fix] disable")?;
        crate::lint::check_fix_rules(&self.promoted, "[fix] unsafe")?;
        Ok(())
    }
}

/// One fix that was (or would be) applied.
///
/// Reported even on a dry run, and even when a later pass reverted the file: what was
/// applied is what the run did, and a reader who is told only what remains cannot tell a
/// quiet run from a busy one that undid itself.
#[derive(Debug, Clone)]
pub struct Applied {
    /// The file as the caller named it — not the scratch path a dry run writes to.
    pub file: PathBuf,
    /// The line the diagnostic was on, before this pass's edits moved anything.
    pub line: usize,
    /// The rule the fix was filed under, which is what `--rule` and `[fix]` match on.
    pub rule: String,
    /// The class it was applied under. `Unsafe` here means the run was asked for it,
    /// through `--unsafe` or `[fix] unsafe`.
    pub applicability: Applicability,
    /// Which pass applied it, counted from 1. More than one means an edit could not land
    /// until an overlapping one had been applied and the file re-checked.
    pub pass: usize,
}

/// One fix that was not applied, and why.
#[derive(Debug, Clone)]
pub struct Skipped {
    /// The file the diagnostic was in.
    pub file: PathBuf,
    /// The line it was on.
    pub line: usize,
    /// The rule it was filed under.
    pub rule: String,
    /// Why it was passed over — a class the run was not asked for, a rule `--rule` or
    /// `[fix] disable` excluded, a `suggest` that is never applied. A sentence rather
    /// than a code, because it is printed as one.
    pub reason: String,
}

/// Everything one file's run of [`fix_file`] did, and everything it declined to do.
///
/// A run that changed nothing still fills this in: the skips are the answer to "why did
/// `htl fix` do nothing", and without them a filtered run and a clean file look alike.
#[derive(Debug, Default)]
pub struct FileOutcome {
    /// The file this is about.
    pub file: PathBuf,
    /// Fixes that landed, in the order the passes applied them.
    pub applied: Vec<Applied>,
    /// Fixes that did not, each with its reason.
    pub skipped: Vec<Skipped>,
    /// Edits deferred because they overlapped an applied one and the pass cap hit.
    pub deferred: usize,
    /// The file was put back as it was because a fix introduced an error.
    pub reverted: Option<String>,
    /// Two passes produced the same edit set: the rules named undo each other.
    pub oscillation: Option<String>,
    /// The new contents (dry run: what would be written); `None` when unchanged.
    pub contents: Option<String>,
    /// The file as the `suggest` fixes would additionally leave it, over whatever was
    /// applied. Never written: a suggestion is shown and the value in it is the author's
    /// to choose (`htl fix --diff` is where it is shown). `None` when there are none.
    pub suggested: Option<String>,
    /// Diagnostics after the last pass (what `htl check` would now say).
    pub check: CheckInfo,
}

/// Fix one file in place (or in memory with `dry_run`). The checker's search path
/// must already cover the project.
pub fn fix_file(h: &Htl, path: &Path, opts: &FixOptions) -> Result<FileOutcome> {
    // Before reading the file: a misspelt rule is a fact about the request, and answering
    // it after the first file has been rewritten would be the wrong way round.
    opts.validate()?;
    let original =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut current = original.clone();
    let mut out = FileOutcome {
        file: path.to_path_buf(),
        ..Default::default()
    };
    let mut check = h.check(path)?;
    let mut last_set: Option<BTreeSet<String>> = None;
    // A dry run checks from a scratch copy so the tree stays untouched.
    let scratch = if opts.dry_run {
        Some(scratch_path(path)?)
    } else {
        None
    };

    for pass in 1..=MAX_PASSES {
        if has_syntax_error(&check) {
            out.skipped.push(Skipped {
                file: path.to_path_buf(),
                line: 0,
                rule: String::new(),
                reason: "file has a syntax error; nothing is applied to a tree the parser rejected"
                    .into(),
            });
            break;
        }
        // Type errors elsewhere in the file do not block: positions come from the parse,
        // which succeeded, and a lint's fix is often what removes the type error (an
        // `explicit-number` annotation). The re-check below reverts anything that made
        // the file worse. Only a syntax error (above) blocks.
        let candidates = candidates(&check, opts, &mut out.skipped, path);
        if candidates.is_empty() {
            break;
        }
        let set: BTreeSet<String> = candidates
            .iter()
            .map(|c| format!("{}:{}:{}", c.rule, c.line, c.key))
            .collect();
        if last_set.as_ref() == Some(&set) {
            let rules: BTreeSet<&str> = candidates.iter().map(|c| c.rule.as_str()).collect();
            out.oscillation = Some(rules.into_iter().collect::<Vec<_>>().join(", "));
            break;
        }
        last_set = Some(set);

        let (next, applied_idx, deferred) = apply_non_overlapping(&current, &candidates);
        if applied_idx.is_empty() {
            out.deferred = deferred;
            break;
        }
        // Write, re-check, keep or revert.
        let target = scratch.as_deref().unwrap_or(path);
        std::fs::write(target, &next).with_context(|| format!("writing {}", target.display()))?;
        // Ask about the file that was just written, not about the one the checker's store
        // remembers. A dry run got the right answer by accident — it writes to a scratch path
        // no store entry names — while a real run re-checked the path the store knew and was
        // handed the result from before the write, then reverted a correct fix for leaving
        // the error count unchanged.
        let recheck = h.check_written(target)?;
        let new_errors = recheck.errors.len();
        let fixed_errors = applied_idx
            .iter()
            .filter(|&&i| candidates[i].is_error)
            .count();
        // Errors other than the ones just fixed must not have grown.
        if new_errors > check.errors.len().saturating_sub(fixed_errors) {
            std::fs::write(target, &current)
                .with_context(|| format!("restoring {}", target.display()))?;
            out.reverted = Some(format!(
                "pass {pass} left {} error(s) where there were {}; the file was put back",
                new_errors,
                check.errors.len()
            ));
            break;
        }
        for &i in &applied_idx {
            let c = &candidates[i];
            out.applied.push(Applied {
                file: path.to_path_buf(),
                line: c.line,
                rule: c.rule.clone(),
                applicability: c.applicability,
                pass,
            });
        }
        current = next;
        out.deferred = deferred;
        check = recheck;
        if deferred == 0 {
            // Nothing waited on this pass; a further pass would only rediscover new
            // findings the rewrite created, which the next `htl fix` can take.
            break;
        }
    }
    if let Some(s) = &scratch {
        let _ = std::fs::remove_file(s);
        if let Some(d) = s.parent() {
            let _ = std::fs::remove_dir(d);
        }
    }
    // What the suggestions would insert, computed once from the last check and never
    // written. `candidates` skipped them with a reason; this is the same set from the
    // other side, so `--diff` can show the edit a person is meant to finish.
    if !has_syntax_error(&check) {
        let sug = suggestions(&check, opts);
        if !sug.is_empty() {
            let (text, applied, _) = apply_non_overlapping(&current, &sug);
            if !applied.is_empty() && text != current {
                out.suggested = Some(text);
            }
        }
    }
    if current != original {
        out.contents = Some(current);
    }
    out.check = if opts.dry_run && out.contents.is_some() {
        check
    } else {
        // Same reason as the re-check above: this file may have been written during the loop.
        h.check_written(path)?
    };
    Ok(out)
}

/// tl's parser errors carry "syntax error" in their text; type errors never do.
fn has_syntax_error(c: &CheckInfo) -> bool {
    c.errors.iter().any(|e| e.contains("syntax error"))
}

struct Candidate {
    rule: String,
    line: usize,
    key: String,
    is_error: bool,
    applicability: Applicability,
    edits: Vec<Edit>,
}

/// Every diagnostic of `check` that carries a fix, errors before lints. Each one arrives
/// with its position and its rule already read off it (`crate::Diagnostic`), so nothing
/// here goes back to the printed line to find them.
fn fixable(check: &CheckInfo) -> Vec<(Diagnostic, crate::Fix, bool)> {
    check
        .error_diagnostics()
        .into_iter()
        .map(|d| (d, true))
        .chain(check.lint_diagnostics().into_iter().map(|d| (d, false)))
        .filter_map(|(mut d, is_error)| d.fix.take().map(|fix| (d, fix, is_error)))
        .collect()
}

/// The edits of one fix as a candidate's key: what tells two passes apart.
fn edit_key(fix: &crate::Fix) -> String {
    fix.edits
        .iter()
        .map(|e| {
            format!(
                "{}:{}:{}:{}:{}",
                e.line, e.col, e.end_line, e.end_col, e.text
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Which of the file's fixes may be applied under `opts`; the rest go to `skipped`.
fn candidates(
    check: &CheckInfo,
    opts: &FixOptions,
    skipped: &mut Vec<Skipped>,
    path: &Path,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (d, fix, is_error) in fixable(check) {
        let rule = rule_of(&d, is_error);
        let line = d.line;
        if !opts.only.is_empty() && !opts.only.iter().any(|r| r == &rule) {
            continue;
        }
        if opts.disabled.iter().any(|r| r == &rule) {
            skipped.push(Skipped {
                file: path.into(),
                line,
                rule,
                reason: "disabled by [fix] disable".into(),
            });
            continue;
        }
        let promoted = opts.promoted.iter().any(|r| r == &rule);
        let applicability = if promoted && fix.applicability == Applicability::Unsafe {
            Applicability::Safe
        } else {
            fix.applicability
        };
        match applicability {
            Applicability::Suggest => {
                skipped.push(Skipped {
                    file: path.into(),
                    line,
                    rule,
                    reason: "suggestion only; not applied automatically".into(),
                });
                continue;
            }
            Applicability::Unsafe if !opts.unsafe_fixes => {
                skipped.push(Skipped {
                    file: path.into(),
                    line,
                    rule,
                    reason: "unsafe fix; apply with --unsafe or promote it under [fix] unsafe"
                        .into(),
                });
                continue;
            }
            _ => {}
        }
        out.push(Candidate {
            rule,
            line,
            key: edit_key(&fix),
            is_error,
            applicability,
            edits: fix.edits,
        });
    }
    out
}

/// The `suggest` fixes of `check`, under the same `--rule` and `[fix] disable` filtering
/// as the rest. They are never applied to the file; `fix_file` renders them onto a copy
/// so that what they would insert can be shown.
fn suggestions(check: &CheckInfo, opts: &FixOptions) -> Vec<Candidate> {
    let mut out = Vec::new();
    for (d, fix, is_error) in fixable(check) {
        if fix.applicability != Applicability::Suggest {
            continue;
        }
        let rule = rule_of(&d, is_error);
        if !opts.only.is_empty() && !opts.only.iter().any(|r| r == &rule) {
            continue;
        }
        if opts.disabled.iter().any(|r| r == &rule) {
            continue;
        }
        out.push(Candidate {
            line: d.line,
            rule,
            key: edit_key(&fix),
            is_error,
            applicability: fix.applicability,
            edits: fix.edits,
        });
    }
    out
}

/// What `--rule` and `[fix] disable` name a diagnostic by: a lint's own rule, or a class
/// name for an error, which has none of its own.
///
/// Both classes are registered rules ([`crate::lint::RULES`], `Surfaces::FixOnly`), so a
/// filter naming one is a filter naming something that exists. `tl:error` is spelt in the
/// compiler's namespace like its warning kinds, and for the same reason: it is the
/// compiler speaking, and bare `error` as a rule name would collide with everything.
fn rule_of(d: &Diagnostic, is_error: bool) -> String {
    if !is_error && let Some(rule) = &d.rule {
        return rule.clone();
    }
    if d.message.contains("invalid key '") && d.message.contains("is defined at line") {
        return "forward-ref".into();
    }
    "tl:error".into()
}

/// Apply the candidates whose edits do not overlap an already accepted edit, in
/// diagnostic order. Returns (new text, applied candidate indexes, deferred count).
fn apply_non_overlapping(src: &str, candidates: &[Candidate]) -> (String, Vec<usize>, usize) {
    let index = LineIndex::new(src);
    // (start, end, text, candidate, edit within it)
    let mut accepted: Vec<(usize, usize, &str, usize, usize)> = Vec::new();
    let mut applied = Vec::new();
    let mut deferred = 0usize;
    'cand: for (ci, c) in candidates.iter().enumerate() {
        let mut spans = Vec::new();
        for e in &c.edits {
            let (Some(s), Some(t)) = (
                index.offset(e.line, e.col),
                index.offset(e.end_line, e.end_col),
            ) else {
                deferred += 1;
                continue 'cand;
            };
            if t < s {
                deferred += 1;
                continue 'cand;
            }
            spans.push((s, t, e.text.as_str()));
        }
        // Overlap = a non-empty intersection with an accepted span; two insertions at
        // one point are fine and keep their order.
        for (s, t, _) in &spans {
            for (as_, at, _, _, _) in &accepted {
                let disjoint = *t <= *as_ || *at <= *s || (*s == *t && *as_ == *at && *s == *as_);
                let touching_insert = (*s == *t && (*s == *as_ || *s == *at))
                    || (*as_ == *at && (*as_ == *s || *as_ == *t));
                if !(disjoint || touching_insert) {
                    deferred += 1;
                    continue 'cand;
                }
            }
        }
        for (ei, (s, t, text)) in spans.into_iter().enumerate() {
            accepted.push((s, t, text, ci, ei));
        }
        applied.push(ci);
    }
    // Apply from the end so earlier offsets stay valid. Insertions at one point are
    // applied back to front — by candidate, then by edit within it — which is what leaves
    // them in the text in the order they were listed: one fix inserting a field per edit
    // gets them in the order it named them.
    accepted.sort_by(|a, b| b.0.cmp(&a.0).then(b.3.cmp(&a.3)).then(b.4.cmp(&a.4)));
    let mut out = src.to_string();
    for (s, t, text, _, _) in accepted {
        out.replace_range(s..t, text);
    }
    (out, applied, deferred)
}

struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    fn new(src: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        Self {
            starts,
            len: src.len(),
        }
    }

    /// Byte offset of 1-based (line, col); a col past the line's end clamps to it.
    fn offset(&self, line: usize, col: usize) -> Option<usize> {
        if line == 0 || col == 0 {
            return None;
        }
        // One past the last line is allowed for an insertion at the end of the file.
        if line == self.starts.len() + 1 {
            return Some(self.len);
        }
        let start = *self.starts.get(line - 1)?;
        let end = self.starts.get(line).map(|e| e - 1).unwrap_or(self.len);
        Some((start + col - 1).min(end.max(start)))
    }
}

fn scratch_path(path: &Path) -> Result<PathBuf> {
    let stem = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("file.tl");
    let dir = std::env::temp_dir().join(format!("htl-fix-{}-{}", std::process::id(), nanos()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join(stem))
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// A unified diff of `before` -> `after` (LCS on lines, 3 lines of context).
pub fn unified_diff(name: &str, before: &str, after: &str) -> String {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let (n, m) = (a.len(), b.len());
    let mut l = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            l[i][j] = if a[i] == b[j] {
                l[i + 1][j + 1] + 1
            } else {
                l[i + 1][j].max(l[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut ops: Vec<(char, &str)> = Vec::new();
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            ops.push((' ', a[i]));
            i += 1;
            j += 1;
        } else if i < n && (j >= m || l[i + 1][j] >= l[i][j + 1]) {
            ops.push(('-', a[i]));
            i += 1;
        } else {
            ops.push(('+', b[j]));
            j += 1;
        }
    }
    let mut keep = vec![false; ops.len()];
    for (k, op) in ops.iter().enumerate() {
        if op.0 != ' ' {
            let hi = (k + 4).min(ops.len());
            for slot in &mut keep[k.saturating_sub(3)..hi] {
                *slot = true;
            }
        }
    }
    let mut out = format!("--- {name}\n+++ {name}\n");
    let mut last = usize::MAX;
    for (k, op) in ops.iter().enumerate() {
        if keep[k] {
            if last != usize::MAX && k > last + 1 {
                out.push_str("@@\n");
            }
            out.push(op.0);
            out.push_str(op.1);
            out.push('\n');
            last = k;
        }
    }
    out
}

/// Is `path` clean in git? `Ok(None)` when it is not inside a repository.
pub fn git_dirty(path: &Path) -> Result<Option<bool>> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain", "--"])
        .arg(path.file_name().unwrap_or_default())
        .current_dir(dir)
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(Some(!o.stdout.is_empty())),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => bail!("running git status: {e}"),
    }
}
