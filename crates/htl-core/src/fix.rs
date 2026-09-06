//! Applying the fixes that diagnostics carry (`htl fix`).
//!
//! The shape follows what cargo fix, ESLint and Ruff settled on: a fix travels with
//! its diagnostic and has an applicability class; only `safe` applies unless asked;
//! edits that overlap within one pass are deferred to the next, which re-checks the
//! file; passes are capped; a fix that leaves the file with an error it did not have
//! is reverted; everything applied is reported, not only what remains. A file the
//! parser rejects is never touched; type errors do not block (their positions are
//! sound, and a fix may be what removes them), the revert is the guard.

use crate::{Applicability, CheckInfo, Edit, Htl};
use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Passes per file before giving up (cargo fix uses 4).
pub const MAX_PASSES: usize = 4;

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

/// One fix that was (or would be) applied.
#[derive(Debug, Clone)]
pub struct Applied {
    pub file: PathBuf,
    pub line: usize,
    pub rule: String,
    pub applicability: Applicability,
    pub pass: usize,
}

/// One fix that was not applied, and why.
#[derive(Debug, Clone)]
pub struct Skipped {
    pub file: PathBuf,
    pub line: usize,
    pub rule: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct FileOutcome {
    pub file: PathBuf,
    pub applied: Vec<Applied>,
    pub skipped: Vec<Skipped>,
    /// Edits deferred because they overlapped an applied one and the pass cap hit.
    pub deferred: usize,
    /// The file was put back as it was because a fix introduced an error.
    pub reverted: Option<String>,
    /// Two passes produced the same edit set: the rules named undo each other.
    pub oscillation: Option<String>,
    /// The new contents (dry run: what would be written); `None` when unchanged.
    pub contents: Option<String>,
    /// Diagnostics after the last pass (what `htl check` would now say).
    pub check: CheckInfo,
}

/// Fix one file in place (or in memory with `dry_run`). The checker's search path
/// must already cover the project.
pub fn fix_file(h: &Htl, path: &Path, opts: &FixOptions) -> Result<FileOutcome> {
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

/// Which of the file's fixes may be applied under `opts`; the rest go to `skipped`.
fn candidates(
    check: &CheckInfo,
    opts: &FixOptions,
    skipped: &mut Vec<Skipped>,
    path: &Path,
) -> Vec<Candidate> {
    let mut out = Vec::new();
    let items = check
        .errors
        .iter()
        .zip(check.error_fixes.iter())
        .map(|(m, f)| (m, f, true))
        .chain(
            check
                .lints
                .iter()
                .zip(check.lint_fixes.iter())
                .map(|(m, f)| (m, f, false)),
        );
    for (msg, fix, is_error) in items {
        let Some(fix) = fix else { continue };
        let rule = rule_of(msg, is_error);
        let line = line_of(msg);
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
        let key = fix
            .edits
            .iter()
            .map(|e| {
                format!(
                    "{}:{}:{}:{}:{}",
                    e.line, e.col, e.end_line, e.end_col, e.text
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        out.push(Candidate {
            rule,
            line,
            key,
            is_error,
            applicability,
            edits: fix.edits.clone(),
        });
    }
    out
}

/// The rule name of a lint line (`... [htl <rule>]`), or a class name for errors.
fn rule_of(msg: &str, is_error: bool) -> String {
    if !is_error
        && msg.ends_with(']')
        && let Some(start) = msg.rfind(" [htl ")
    {
        return msg[start + 6..msg.len() - 1].to_string();
    }
    if msg.contains("invalid key '") && msg.contains("is defined at line") {
        return "forward-ref".into();
    }
    "error".into()
}

fn line_of(msg: &str) -> usize {
    msg.split(':')
        .nth(1)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Apply the candidates whose edits do not overlap an already accepted edit, in
/// diagnostic order. Returns (new text, applied candidate indexes, deferred count).
fn apply_non_overlapping(src: &str, candidates: &[Candidate]) -> (String, Vec<usize>, usize) {
    let index = LineIndex::new(src);
    let mut accepted: Vec<(usize, usize, &str, usize)> = Vec::new(); // (start, end, text, candidate)
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
            for (as_, at, _, _) in &accepted {
                let disjoint = *t <= *as_ || *at <= *s || (*s == *t && *as_ == *at && *s == *as_);
                let touching_insert = (*s == *t && (*s == *as_ || *s == *at))
                    || (*as_ == *at && (*as_ == *s || *as_ == *t));
                if !(disjoint || touching_insert) {
                    deferred += 1;
                    continue 'cand;
                }
            }
        }
        for (s, t, text) in spans {
            accepted.push((s, t, text, ci));
        }
        applied.push(ci);
    }
    // Apply from the end so earlier offsets stay valid; equal starts keep insertion order.
    accepted.sort_by(|a, b| b.0.cmp(&a.0).then(b.3.cmp(&a.3)));
    let mut out = src.to_string();
    for (s, t, text, _) in accepted {
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
