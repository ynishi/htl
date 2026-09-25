//! `---@contract` / `---@required`: the contract a directory of modules must satisfy,
//! declared where the type is declared.
//!
//! ```tl
//! local record defs
//!    record Mod              ---@contract("mods")
//!       name: string         ---@required
//!       monsters: {Monster}  ---@required
//!       factions: {Faction}
//!    end
//! end
//! return defs
//! ```
//!
//! Every module under `mods/` (`foo.tl`, `foo/init.tl`, `sub/x.tl`: see [`held_name`]) must return a value assignable to `defs.Mod` and
//! set `name` and `monsters`; `factions` is there for the mods that want it. That last
//! part is why the fields are marked rather than counted: a record cannot say which of
//! its own fields are mandatory (every Teal record field is nilable and there is no `?`
//! for them), and taking all of them would break every module written before the field
//! was added. The same nilability is why a record literal may leave fields out and
//! `htl check` is right to pass it: a `#[derive(TealRecord)]` on the Rust side reports a
//! missing field only when the value crosses at run time, and a record marked
//! `---@contract` with `---@required` on the fields that must be there is the check-time
//! counterpart, for a module's table that is meant to be complete. The default is the
//! opposite of `---@struct`'s, and each marker says which regime its record is under:
//! `---@struct` is about a record the program builds itself, where a new field is
//! mandatory unless marked `---@optional` (`prelude.lua`, `struct_spec`); `---@contract`
//! is about a value arriving from outside, where a new field is optional unless marked
//! `---@required`.
//!
//! `htl.toml` holds the directory, and two narrowings the marker can also spell:
//!
//! ```toml
//! [[contract]]
//! dir = "mods"
//! # module = "Site"      # only this module name, in each matched dir, is held to it
//! # exclude = ["defs"]   # modules in dir not held to it: a helper, an SDK the host writes
//! ```
//!
//! A bare `---@contract` inherits the directory — one line in the file a reader opens
//! first, saying where this project accepts modules — and `---@contract("<dir>")`
//! overrides it, which is what a project with more than one contract writes.
//! `---@contract(module = "S")` and `---@contract(exclude = "defs modkit")` say the two
//! narrowings on the record ([`Contract::module`], [`Contract::exclude`]); a `.d.tl` is
//! never held to a contract and needs no listing, and any of these can be given at once.
//!
//! The markers are comments, so the file stays valid Teal and other tooling ignores them.
//! Reading them is a scan of the search paths, not a type-check: a record is found by the
//! line it is declared on, the way `---@struct` is (see `prelude.lua`).
//!
//! # What is published, and the two lints
//!
//! The declaring module is written out with its bodies removed, to `types/<module>.d.tl`
//! or where `---@contract(dts = "…")` says ([`publish`]):
//!
//! ```text
//! function defs.describe(m: Mod): string    -->    describe: function(m: Mod): string
//!    return m.name
//! end
//! ```
//!
//! `contract` reports a module under the directory whose return value is not assignable
//! to the record, or whose returned table literal leaves a `---@required` field out (the
//! literal is found through `return { … }`, `return define({ … })`, `return { … } as T`,
//! and `local m: T = { … } … m.f = … return m`), and a marker that cannot be turned into
//! a contract. `contract-unenforced` reports a contract the host never builds resolvers
//! for ([`crate::pkg::contract_resolvers`]); enforcement the scan cannot see is named
//! instead, and that contract alone is then not held to the scan:
//!
//! ```toml
//! [[contract]]
//! dir = "mods"
//! enforced_by = "mods/_validate.lua"   # relative to htl.toml; ~ and absolute paths work
//! ```
use crate::Diagnostic;
use crate::config::{Contract, HtlConfig, RequireFields};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A contract as it applies: which directory, which type, which fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Directory relative to the project root, `*` in one segment expanding to every
    /// subdirectory at that level (`"sites/*"`).
    pub dir: String,
    /// `"<module>.<Type>"`, e.g. `"defs.Mod"` — the module that declares the record and
    /// the path to it inside that module.
    pub type_path: String,
    /// The fields a module's returned table must set, from `---@required`.
    pub require_fields: RequireFields,
    /// Only this module name in each matched dir is held to the contract.
    pub module: Option<String>,
    /// Module names in the dir that are not held to the contract.
    pub exclude: Vec<String>,
    /// Where to publish the declaration, from `---@contract(dts = "…")`.
    pub dts: Option<String>,
    /// The project's declaration root (`[layout] types`) when the contract was read,
    /// absolute: where the declaration is published when the marker names no `dts`.
    pub types: PathBuf,
    /// Where this contract is enforced when the scan cannot see it, from
    /// `[[contract]] enforced_by`. Not a marker argument: enforcement is the host's
    /// business, and the record is published to authors who have no use for the path.
    pub enforced_by: Option<String>,
    /// The file the marker is in: where to point when something about this contract is
    /// wrong. Not where the contract applies — that is [`dir`](Self::dir) — but where the
    /// sentence that set it up was written, which is the line a person has to edit.
    pub declared_in: PathBuf,
    /// The line of that marker, counted from 1.
    pub declared_at: usize,
}

impl Resolved {
    /// Concrete contract directories under `root` (expands one `*` segment). Missing
    /// directories are dropped; a literal `dir` that does not exist yields nothing.
    pub fn dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut acc = vec![root.to_path_buf()];
        for seg in self.dir.split('/').filter(|s| !s.is_empty() && *s != ".") {
            let mut next = Vec::new();
            for base in &acc {
                if seg == "*" {
                    if let Ok(rd) = std::fs::read_dir(base) {
                        let mut subs: Vec<PathBuf> = rd
                            .flatten()
                            .map(|e| e.path())
                            .filter(|p| p.is_dir() && !crate::is_skipped_dir(p, &[]))
                            .collect();
                        subs.sort();
                        next.extend(subs);
                    }
                } else {
                    let p = base.join(seg);
                    if p.is_dir() {
                        next.push(p);
                    }
                }
            }
            acc = next;
        }
        acc
    }

    /// Every module the directories of this contract hold, as `(name, file)`, each
    /// directory walked in order and each in file-name order: what the host's resolver
    /// serves from them ([`crate::pkg::TealResolver::for_contract`]) and what `htl check`
    /// holds to the contract. The modules `exclude` names are among them — `exclude` says a
    /// module is not held to the type, not that the directory does not serve it.
    pub fn held_modules(&self, root: &Path) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        for dir in self.dirs(root) {
            let top = dir.clone();
            let walker = walkdir::WalkDir::new(&dir)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(move |e| e.path() == top || !crate::is_skipped_dir(e.path(), &[]));
            for e in walker.flatten() {
                if let Some(name) = held_name(&dir, e.path()) {
                    out.push((name, e.path().to_path_buf()));
                }
            }
        }
        out
    }

    /// Is the module named `module` under a contract dir held to this contract? The name
    /// is the one it is required under ([`held_name`]): `foo` for `foo.tl` and
    /// `foo/init.tl`, `sub.x` for `sub/x.tl`. The module that declares the type is not
    /// held to it.
    pub fn applies_to(&self, module: &str) -> bool {
        if self
            .type_path
            .split_once('.')
            .is_some_and(|(m, _)| m == module)
        {
            return false;
        }
        if self.exclude.iter().any(|e| e == module) {
            return false;
        }
        match &self.module {
            Some(only) => only == module,
            None => true,
        }
    }
}

/// The name the Teal module at `file` answers to when the contract directory `dir` serves
/// it: the naming rule every other name in htl comes from ([`crate::naming::name_of`], the
/// directory mounted at the top), so `foo.tl` and `foo/init.tl` are `foo` and `sub/x.tl`
/// is `sub.x` — what `require` writes, and what the host's resolver for `dir` serves the
/// file under.
///
/// `None` for a file the directory does not serve as a module: outside `dir`, below a
/// directory no walk enters ([`crate::is_skipped_dir`]: build output, dot-directories), or
/// not a `.tl` implementation (a `.d.tl` declares, it is not a module a host loads).
///
/// The one answer to "which files, under which names, does a contract hold": the
/// `contract` lint, [`Resolved::held_modules`] and `htl unused` all read it.
pub fn held_name(dir: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(dir).ok()?;
    if !crate::is_tl_source(file) {
        return None;
    }
    let mut at = dir.to_path_buf();
    if let Some(parent) = rel.parent() {
        for c in parent.components() {
            at.push(c);
            if crate::is_skipped_dir(&at, &[]) {
                return None;
            }
        }
    }
    crate::naming::name_of("", rel)
}

/// A problem with a contract, said under `contract` at `line` of `file`.
fn problem(file: &Path, line: usize, message: String) -> Diagnostic {
    Diagnostic::new(
        crate::Severity::Lint,
        file.display().to_string(),
        line,
        1,
        message,
        Some("contract"),
    )
}

/// What the `---@contract` on one record says, before the directory is settled.
struct Marker {
    dir: Option<String>,
    module: Option<String>,
    exclude: Option<Vec<String>>,
    dts: Option<String>,
}

/// Contracts this project declares, and what is wrong with the ones it does not.
///
/// The scan covers the roots of the project's modules ([`HtlConfig::marker_roots`]: the
/// source root, the declaration root, `[check] paths`) one level deep, plus
/// `<sub>/init.tl`, which is the shape Teal's own path templates resolve. A marker
/// anywhere else is not found, and the module name of a file nested deeper cannot be
/// written as `<module>.<Type>` anyway. The declaring module is named by
/// [`crate::naming`], as every module of the project is.
///
/// The project root is not scanned unless it is the source root: a file there is no
/// module of the project. A marker left in one is not silently dropped, though. It is
/// reported, with where to move it.
///
/// The second half of the pair is diagnostics: a bare marker with no `[[contract]]` to
/// inherit from, two markers claiming one directory, a marker on a record that is not
/// nested inside its module. They are returned rather than raised because one broken
/// contract should not take the other contracts of the project with it.
pub fn resolve(root: &Path, cfg: &HtlConfig) -> (Vec<Resolved>, Vec<Diagnostic>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    problems.extend(markers_outside_the_project(root, cfg));
    for (dir, file) in scan_targets(root, cfg) {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !src.contains("---@contract") {
            continue;
        }
        match read_file(root, &dir, &file, &src, cfg) {
            Ok(found) => out.extend(found),
            Err(msgs) => problems.extend(msgs),
        }
    }
    // A published declaration carries the marker it was copied from, so the same record
    // is found twice — once in the source, once in the file written from it. That is one
    // record making one claim, and the source is the one to keep (a `.tl` beats a `.d.tl`
    // everywhere else too).
    out.sort_by_key(|c| crate::is_declaration(&c.declared_in));
    let mut kept: Vec<Resolved> = Vec::with_capacity(out.len());
    for c in out {
        if !kept.iter().any(|k| same_record(root, k, &c)) {
            kept.push(c);
        }
    }
    let out = kept;

    // Two *different* types for one directory would each have to be the one enforced
    // there.
    for i in 0..out.len() {
        if let Some(j) = out[..i].iter().position(|c| c.dir == out[i].dir) {
            problems.push(problem(
                &out[i].declared_in,
                out[i].declared_at,
                format!(
                    "{} claims directory {:?}, which {} already claims at {}:{}",
                    out[i].type_path,
                    out[i].dir,
                    out[j].type_path,
                    out[j].declared_in.display(),
                    out[j].declared_at,
                ),
            ));
        }
    }
    (out, problems)
}

/// Where a contract publishes its declaration: `---@contract(dts = "…")` relative to the
/// project root, or `<module>.d.tl` in the project's declaration root by default
/// ([`Resolved::types`], `[layout] types`) — the directory a project keeps declarations
/// for other people in.
pub fn dts_target(root: &Path, c: &Resolved) -> Option<PathBuf> {
    let module = c.type_path.split_once('.')?.0;
    Some(match &c.dts {
        Some(p) => crate::config::resolve_path(root, p),
        None => c.types.join(format!("{module}.d.tl")),
    })
}

/// Is `later` the record `earlier` already is, found a second time? The claim check is
/// between records, and a record's declaration is that record rather than a second
/// claimant of the directory.
///
/// Two ways one record turns up twice. It was published: `htl dts` writes the module a
/// `---@contract` type is declared in, and `types/` is on the search path, so the scan
/// reads the claim back out of the file it just wrote. Or the same file was reached
/// through two search paths. Which file a publication is is the publish's own answer
/// ([`dts_target`]) — nothing here recognises a `types/` directory or a `.d.tl` name, so
/// a project that publishes somewhere else is the same case — and the directory is what
/// says the two claims are one claim, since that is what the published marker carries.
///
/// A module of the same name in each of two contract directories is *not* this: a
/// contract directory is resolved as a directory ([`crate::pkg::TealResolver::for_contract_dir`]
/// roots one resolver at each) and is not on the project's search path, so `mods_a/one.tl`
/// and `mods_b/one.tl` are two modules, each held to the record its own directory is
/// under. There is nothing there for the claim check to report.
fn same_record(root: &Path, earlier: &Resolved, later: &Resolved) -> bool {
    if earlier.dir != later.dir {
        return false;
    }
    if crate::same_file(&earlier.declared_in, &later.declared_in) {
        return earlier.type_path == later.type_path;
    }
    dts_target(root, earlier).is_some_and(|t| crate::same_file(&t, &later.declared_in))
}

/// Publish each contract's declaration: the module that declares the contract type is
/// what an outside author writes against, so `htl` writes it out rather than leaving the
/// host to copy the file at run time — to `types/<module>.d.tl` beside the `.d.tl` a Rust
/// host's `#[host_module]` writes, or where `---@contract(dts = "…")` says. `htl dts`
/// and every command that reads the project (check / run / test / build / fix / unused /
/// resolve / gen) regenerate it; the result is committed, the same as the Rust-generated
/// ones, which is what makes a fresh clone check before anything has been built. Returns
/// the targets it wrote (`true`) or found already current (`false`), and what it could
/// not publish.
///
/// The declaring module is written out as a declaration: bodies removed, and each
/// function that was part of the module's interface folded into its record as a field
/// (`function m.f(a: integer): string` -> `f: function(a: integer): string`), which is
/// what a hand-written `.d.tl` says. A module of declarations is copied unchanged,
/// because there is nothing to remove.
///
/// One write per file, not one per contract. Two records in one module publish that one
/// module, and a pass per record would write the file twice over — each pass rewriting
/// its own marker and leaving the other's as the author left it, so the two passes
/// disagree, `dts: wrote` is said twice, and the file never settles. A file's markers are
/// made self-contained together, once.
pub fn publish(root: &Path, contracts: &[Resolved]) -> (Vec<(PathBuf, bool)>, Vec<Diagnostic>) {
    publish_to(root, contracts, true)
}

/// [`publish`], writing only when `write` is set. Without it everything is worked out the
/// same way — the problems are the same ones — and each pair says whether the file would
/// change: what a dry run reports.
pub fn publish_to(
    root: &Path,
    contracts: &[Resolved],
    write: bool,
) -> (Vec<(PathBuf, bool)>, Vec<Diagnostic>) {
    let mut written = Vec::new();
    let mut problems = Vec::new();
    // Each file to write and the module it is written from, in the order the contracts
    // came in.
    let mut targets: Vec<(PathBuf, &Resolved)> = Vec::new();
    for c in contracts {
        let Some(target) = dts_target(root, c) else {
            continue;
        };
        // A contract already declared in a `.d.tl` is its own publication.
        if crate::same_file(&target, &c.declared_in) {
            continue;
        }
        match targets.iter().find(|(t, _)| crate::same_file(t, &target)) {
            // Two modules cannot both be one declaration: whichever was written last
            // would be the file, and the other would have been published and lost.
            Some((_, first)) if !crate::same_file(&first.declared_in, &c.declared_in) => problems
                .push(problem(
                    &c.declared_in,
                    c.declared_at,
                    format!(
                        "{} publishes to {}, where {} is already published from {}",
                        c.type_path,
                        target.display(),
                        first.type_path,
                        first.declared_in.display(),
                    ),
                )),
            Some(_) => {}
            None => targets.push((target, c)),
        }
    }
    for (target, c) in targets {
        let Ok(src) = std::fs::read_to_string(&c.declared_in) else {
            continue;
        };
        // Every marker the file carries, not only the one that sent it here: what is
        // published has to stand on its own whichever record a reader opens it for.
        let src = contracts
            .iter()
            .filter(|o| crate::same_file(&o.declared_in, &c.declared_in))
            .fold(src, |s, o| self_contained_marker(&s, o));
        let text = match declaration_of(&src) {
            Ok(t) => t,
            Err(msgs) => {
                problems.extend(msgs.into_iter().map(|(line, m)| {
                    problem(
                        &c.declared_in,
                        line,
                        format!("{m}: publishing {} to {}", c.type_path, target.display()),
                    )
                }));
                continue;
            }
        };
        match crate::write_if_changed_when(&target, &text, write) {
            Ok(w) => written.push((target, w)),
            Err(e) => problems.push(problem(
                &c.declared_in,
                1,
                format!("writing {}: {e}", target.display()),
            )),
        }
    }
    (written, problems)
}

/// The source with its `---@contract` rewritten so the published copy stands on its own:
/// the directory written out (a bare marker inherits from an `htl.toml` the reader of the
/// declaration does not have), and `dts` dropped (the copy is not itself a publisher, and
/// the path was the publisher's).
fn self_contained_marker(src: &str, c: &Resolved) -> String {
    let mut args = format!("{:?}", c.dir);
    if let Some(m) = &c.module {
        args.push_str(&format!(", module = {m:?}"));
    }
    if !c.exclude.is_empty() {
        args.push_str(&format!(", exclude = {:?}", c.exclude.join(" ")));
    }
    let want = format!("---@contract({args})");
    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    // The marker is on the record's line or the one above it, the same two places it was
    // read from.
    for i in [
        c.declared_at.saturating_sub(1),
        c.declared_at.saturating_sub(2),
    ] {
        let Some(line) = lines.get_mut(i) else {
            continue;
        };
        let Some(at) = line.find("---@contract") else {
            continue;
        };
        let tail = &line[at + "---@contract".len()..];
        let rest = match tail.split_once(')') {
            Some((_, after)) if tail.trim_start().starts_with('(') => after.to_string(),
            _ => tail.to_string(),
        };
        *line = format!("{}{want}{rest}", &line[..at]);
        break;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// One `function` statement of a module: where it sits, what to write instead, and where
/// that goes.
struct Implementation {
    /// Lines to remove, `[first, last]`, zero-based, doc comment included.
    span: (usize, usize),
    /// The doc comment, trimmed, to reindent above the field.
    doc: Vec<String>,
    /// `Some((record path, field text))` for a function on the module's own table, `None`
    /// for a `local function` — module-local, and not part of what the module declares.
    field: Option<(Vec<String>, String)>,
}

/// The `.d.tl` for a module's source: every function body removed and its signature moved
/// into the record it belongs to. `Err` when a `function` statement cannot be placed,
/// rather than a file with a silently missing function in it.
pub fn declaration_of(src: &str) -> Result<String, Vec<(usize, String)>> {
    let lines: Vec<&str> = src.lines().collect();
    let mut problems = Vec::new();
    let mut found: Vec<Implementation> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(kind) = function_start(lines[i]) else {
            i += 1;
            continue;
        };
        // The doc comment above a function is part of it, and belongs wherever it goes.
        let mut first = i;
        while first > 0 && lines[first - 1].trim_start().starts_with("--") {
            first -= 1;
        }
        let Some(end) = body_end(&lines, i) else {
            problems.push((
                i + 1,
                "this function has no `end` at its own indentation, so its body cannot be \
                 told from what follows"
                    .to_string(),
            ));
            break;
        };
        match kind {
            FnKind::Local => found.push(Implementation {
                span: (first, end),
                doc: Vec::new(),
                field: None,
            }),
            FnKind::Exported => match signature(&lines, i) {
                Ok((path, field)) => found.push(Implementation {
                    span: (first, end),
                    doc: lines[first..i]
                        .iter()
                        .map(|l| l.trim().to_string())
                        .collect(),
                    field: Some((path, field)),
                }),
                Err(e) => problems.push((i + 1, e)),
            },
        }
        i = end + 1;
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    if found.is_empty() {
        // Already a declaration: nothing to strip, and copying it as it is keeps the
        // comments and the layout the author wrote.
        return Ok(src.to_string());
    }

    let mut out: Vec<Option<String>> = lines.iter().map(|l| Some(l.to_string())).collect();
    // Fields first, while the line numbers still mean what they meant.
    for imp in &found {
        let Some((path, field)) = &imp.field else {
            continue;
        };
        match record_close(&lines, path) {
            Some(at) => {
                // A record that already declares the field has the author's own version
                // of this signature; a second one would be a duplicate key.
                let name = field.split(':').next().unwrap_or_default();
                if declares_field(&lines, at, name) {
                    continue;
                }
                let indent = " ".repeat(indent_of(lines[at]) + 3);
                let existing = out[at].take().unwrap_or_default();
                let doc: String = imp.doc.iter().map(|l| format!("{indent}{l}\n")).collect();
                out[at] = Some(format!("{doc}{indent}{field}\n{existing}"));
            }
            None => problems.push((
                imp.span.0 + 1,
                format!(
                    "nothing declares a record {} for this function to be a field of",
                    path.join(".")
                ),
            )),
        }
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    for imp in &found {
        let (first, last) = imp.span;
        for l in out.iter_mut().take(last + 1).skip(first) {
            *l = None;
        }
        // The blank line that separated this function from the next belongs to it: left
        // behind, every removal leaves a gap where a function used to be.
        if (first == 0 || lines[first - 1].trim().is_empty())
            && let Some(after) = out.get_mut(last + 1)
            && after.as_deref().is_some_and(|l| l.trim().is_empty())
        {
            *after = None;
        }
    }
    let mut text: String = out
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string();
    text.push('\n');
    Ok(text)
}

enum FnKind {
    Exported,
    Local,
}

fn function_start(line: &str) -> Option<FnKind> {
    let t = line.trim_start();
    if t.starts_with("local function ") {
        Some(FnKind::Local)
    } else if t.starts_with("function ") {
        Some(FnKind::Exported)
    } else {
        None
    }
}

/// The line closing the function that starts at `i`: the first `end` indented no deeper
/// than the `function` itself.
fn body_end(lines: &[&str], i: usize) -> Option<usize> {
    let base = indent_of(lines[i]);
    (i + 1..lines.len()).find(|&j| {
        let t = lines[j].trim_start();
        (t == "end" || t.starts_with("end ") || t.starts_with("end-"))
            && indent_of(lines[j]) <= base
    })
}

/// `function m.f(a: integer): string` -> (`["m"]`, `f: function(a: integer): string`).
///
/// The signature is taken as written, over as many lines as it spans: a `.d.tl` names
/// parameters in a function type just as the implementation does, so there is nothing to
/// rewrite. A `:` method gains the `self` its definition left implicit.
fn signature(lines: &[&str], i: usize) -> Result<(Vec<String>, String), String> {
    let head = lines[i].trim_start().strip_prefix("function ").unwrap();
    let (name, rest) = head
        .split_once('(')
        .ok_or("a function with no parameter list")?;
    let method = name.contains(':');
    let mut path: Vec<String> = name
        .split(['.', ':'])
        .map(|s| s.trim().to_string())
        .collect();
    let field = path.pop().filter(|f| !f.is_empty()).ok_or("no name")?;
    if path.is_empty() {
        return Err("a function on no module table".into());
    }
    // Parameters may run over several lines; the signature ends with the line on which
    // the parentheses close, return type and all.
    let mut sig = rest.to_string();
    let mut depth = 1i32 + count(rest);
    let mut j = i;
    while depth > 0 {
        j += 1;
        let next = *lines.get(j).ok_or("a parameter list that never closes")?;
        depth += count(next);
        sig.push('\n');
        sig.push_str(next);
    }
    let sig = sig.trim_end();
    let self_arg = if !method {
        String::new()
    } else if sig.trim_start().starts_with(')') {
        // `function M:f()` takes only its receiver: no comma to separate it from.
        format!("self: {}", path.last().unwrap())
    } else {
        format!("self: {}, ", path.last().unwrap())
    };
    Ok((path, format!("{field}: function({self_arg}{sig}")))
}

/// Net change in parenthesis depth over a line, ignoring what is inside a comment.
fn count(line: &str) -> i32 {
    let code = line.split("--").next().unwrap_or(line);
    code.chars().filter(|c| *c == '(').count() as i32
        - code.chars().filter(|c| *c == ')').count() as i32
}

/// Does the record closed at `close` already declare a field called `name`? Its body is
/// what lies between its `record` line and that `end`, at one level of nesting.
fn declares_field(lines: &[&str], close: usize, name: &str) -> bool {
    let base = indent_of(lines[close]);
    for j in (0..close).rev() {
        let t = lines[j].trim_start();
        // Its own `record` line: the body is behind us, and a field of that name further
        // up belongs to some other record.
        if indent_of(lines[j]) <= base
            && (t.starts_with("record ") || t.starts_with("local record "))
        {
            return false;
        }
        if t.strip_prefix(name)
            .is_some_and(|r| r.trim_start().starts_with(':'))
        {
            return true;
        }
    }
    false
}

/// The `end` closing the record named by `path` (`["defs", "Mod"]` = `Mod` inside
/// `defs`), searched from the outside in.
fn record_close(lines: &[&str], path: &[String]) -> Option<usize> {
    let mut from = 0usize;
    let mut to = lines.len();
    for name in path {
        let at = (from..to).find(|&j| record_name(lines[j]).as_deref() == Some(name.as_str()))?;
        let base = indent_of(lines[at]);
        to = (at + 1..to)
            .find(|&j| lines[j].trim_start().starts_with("end") && indent_of(lines[j]) <= base)?;
        from = at + 1;
    }
    Some(to)
}

/// Files a marker can be found in: those directly under a root of the project's modules
/// ([`HtlConfig::marker_roots`]), and the `init.tl` of an immediate subdirectory
/// (`require("sub")` resolves to `sub/init.tl`).
fn scan_targets(root: &Path, cfg: &HtlConfig) -> Vec<(PathBuf, PathBuf)> {
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    for dir in cfg.marker_roots(root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<PathBuf> = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && is_teal(&p) {
                here.push(p);
            } else if p.is_dir() && !crate::is_skipped_dir(&p, &[]) {
                for name in ["init.tl", "init.d.tl"] {
                    let init = p.join(name);
                    if init.is_file() {
                        here.push(init);
                    }
                }
            }
        }
        here.sort();
        out.extend(here.into_iter().map(|f| (dir.clone(), f)));
    }
    out.dedup_by(|a, b| a.1 == b.1);
    out
}

/// A `---@contract` in a file directly under the project root when the root is no module's
/// root ([`HtlConfig::marker_roots`]): not read, and said so, with where the file belongs.
/// It was read before the project model decided which directories hold the project's
/// modules, so a project written then finds its contract gone. This is how it finds out
/// why rather than finding a contract that silently holds nothing.
fn markers_outside_the_project(root: &Path, cfg: &HtlConfig) -> Vec<Diagnostic> {
    let top = crate::config::without_cur_dir(root);
    if cfg.marker_roots(root).contains(&top) {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_teal(p))
        .collect();
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let Ok(src) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Some(at) = src.lines().position(|l| l.contains("---@contract")) else {
            continue;
        };
        out.push(problem(
            &f,
            at + 1,
            format!(
                "this ---@contract is not read: the project root is not where the project's \
                 modules are, so a file there declares nothing. Move it under [layout] source \
                 ({}) or types ({})",
                cfg.layout.source, cfg.layout.types,
            ),
        ));
    }
    out
}

fn is_teal(p: &Path) -> bool {
    p.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.ends_with(".tl"))
}

/// Every contract declared in one file. `Err` carries what is wrong with the markers it
/// does have, one message per marker, so a file with two of them reports both.
fn read_file(
    root: &Path,
    dir: &Path,
    file: &Path,
    src: &str,
    cfg: &HtlConfig,
) -> Result<Vec<Resolved>, Vec<Diagnostic>> {
    let lines: Vec<&str> = src.lines().collect();
    // Named as every module of the project is: its path below the root it was found in.
    let Some(module) = file
        .strip_prefix(dir)
        .ok()
        .and_then(|rel| crate::naming::name_of("", rel))
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut problems = Vec::new();
    // A marker on its own line belongs to the line below it, so the record is what is
    // looked for first: asking about the marker line by line would attribute a trailing
    // `record Mod ---@contract` to the field declared under it as well.
    for (i, line) in lines.iter().enumerate() {
        let Some(record) = record_name(line) else {
            continue;
        };
        let Some(marker) = marker_on(&lines, i, "contract") else {
            continue;
        };
        let marker = match parse_marker(&marker) {
            Ok(m) => m,
            Err(e) => {
                problems.push(problem(file, i + 1, e));
                continue;
            }
        };
        let Some(path) = type_path(&lines, i, &module, &record) else {
            problems.push(problem(
                file,
                i + 1,
                format!(
                    "{record} is the module {module} returns, not a type inside it: a \
                     contract type is written as <module>.<Type>, so declare it as a record \
                     within one"
                ),
            ));
            continue;
        };
        let dir = match (marker.dir, cfg.contract.as_slice()) {
            (Some(d), _) => d,
            (None, [one]) => one.dir.clone(),
            (None, []) => {
                problems.push(problem(
                    file,
                    i + 1,
                    "---@contract names no directory and htl.toml declares none: write \
                     ---@contract(\"<dir>\") here, or a [[contract]] dir = \"<dir>\" in htl.toml"
                        .to_string(),
                ));
                continue;
            }
            (None, many) => {
                problems.push(problem(
                    file,
                    i + 1,
                    format!(
                        "---@contract names no directory and htl.toml declares {}: write the \
                         directory on the marker",
                        many.len()
                    ),
                ));
                continue;
            }
        };
        let inherited: Option<&Contract> = cfg.contract.iter().find(|c| c.dir == dir);
        out.push(Resolved {
            dir,
            type_path: path,
            require_fields: required_fields(&lines, i),
            module: marker
                .module
                .or_else(|| inherited.and_then(|c| c.module.clone())),
            exclude: marker
                .exclude
                .or_else(|| inherited.map(|c| c.exclude.clone()))
                .unwrap_or_default(),
            dts: marker.dts,
            types: crate::config::resolve_path(root, &cfg.layout.types),
            enforced_by: inherited.and_then(|c| c.enforced_by.clone()),
            declared_in: file.to_path_buf(),
            declared_at: i + 1,
        });
    }
    // A marker nothing picked up: it reads as a contract and does nothing, which is worse
    // than either being one or not being written.
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("---@contract")
            || record_name(line).is_some()
            || lines.get(i + 1).and_then(|l| record_name(l)).is_some()
        {
            continue;
        }
        problems.push(problem(
            file,
            i + 1,
            "---@contract is not on a record declaration".to_string(),
        ));
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

/// The text after `---@<name>` on line `i` or the line above it, `None` when the marker
/// is not there. `Some("")` for a bare marker; `Some("(…)")` when it has arguments.
///
/// The line above counts only when the marker is the whole of it. A marker trailing a
/// declaration belongs to that declaration, and reading it from the line below as well
/// would make one `---@required` mark two fields.
///
/// What is read stops at the next `---@`: a declaration may carry two markers on one line
/// (`record Mod   ---@contract ---@extensible`), and the second one is the other marker's
/// text rather than this one's argument list.
fn marker_on(lines: &[&str], i: usize, name: &str) -> Option<String> {
    let needle = format!("---@{name}");
    let above = i
        .checked_sub(1)
        .and_then(|p| lines.get(p))
        .filter(|l| l.trim_start().starts_with("---"));
    for line in [lines.get(i), above].into_iter().flatten() {
        if let Some(rest) = line.split(&needle).nth(1) {
            // `---@contracts` is not `---@contract`.
            if rest
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            {
                let mine = rest.split("---@").next().unwrap_or(rest);
                return Some(mine.trim().to_string());
            }
        }
    }
    None
}

/// `("dir", module = "X", dts = "path")` — every part optional, and the whole thing
/// optional. The first argument, if it is a bare string, is the directory.
fn parse_marker(rest: &str) -> Result<Marker, String> {
    let mut m = Marker {
        dir: None,
        module: None,
        exclude: None,
        dts: None,
    };
    if rest.is_empty() {
        return Ok(m);
    }
    let Some(args) = rest.strip_prefix('(').and_then(|r| r.split(')').next()) else {
        return Err(format!(
            "---@contract takes no arguments or a parenthesised list, got {rest:?}"
        ));
    };
    for (n, arg) in args.split(',').map(str::trim).enumerate() {
        if arg.is_empty() {
            continue;
        }
        match arg.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("module", v)) => m.module = Some(unquote(v)?),
            // Space-separated inside one string: a Lua comment is not a place for a list
            // literal, and the names are module names, which have no spaces in them.
            Some(("exclude", v)) => {
                m.exclude = Some(unquote(v)?.split_whitespace().map(str::to_string).collect())
            }
            Some(("dts", v)) => m.dts = Some(unquote(v)?),
            Some((k, _)) => return Err(format!("---@contract has no {k:?} argument")),
            None if n == 0 => m.dir = Some(unquote(arg)?),
            None => return Err(format!("---@contract: {arg:?} is not <name> = <value>")),
        }
    }
    Ok(m)
}

fn unquote(v: &str) -> Result<String, String> {
    let t = v.trim();
    t.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .map(str::to_string)
        .ok_or_else(|| format!("---@contract: {v:?} is not a quoted string"))
}

/// The record declared on `line`, if it declares one.
fn record_name(line: &str) -> Option<String> {
    let after = line.split("record").nth(1)?;
    let name: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `<module>.<Type>` for the record declared at `lines[i]`: the module, then every
/// record enclosing this one *except* the outermost, then the record itself. `None` when
/// the record is the outermost one — that is the module, and a contract type has to be a
/// type inside a module for `require("<module>")` to reach it.
fn type_path(lines: &[&str], i: usize, module: &str, record: &str) -> Option<String> {
    let mut names = vec![record.to_string()];
    let mut depth = indent_of(lines[i]);
    for line in lines[..i].iter().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let d = indent_of(line);
        if d < depth
            && let Some(n) = record_name(line)
        {
            names.push(n);
            depth = d;
        }
    }
    // The outermost record is the module itself, whatever it is called.
    names.pop()?;
    if names.is_empty() {
        return None;
    }
    names.reverse();
    Some(format!("{module}.{}", names.join(".")))
}

/// Fields marked `---@required` in the record declared at `lines[i]`, in the order they
/// are declared. The record ends at the first `end` indented no deeper than it.
fn required_fields(lines: &[&str], i: usize) -> RequireFields {
    let base = indent_of(lines[i]);
    let mut names = Vec::new();
    for j in i + 1..lines.len() {
        let line = lines[j];
        if line.trim_start().starts_with("end") && indent_of(line) <= base {
            break;
        }
        let name: String = line
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty()
            || !line.trim_start()[name.len()..]
                .trim_start()
                .starts_with(':')
        {
            continue;
        }
        if marker_on(lines, j, "required").is_some() {
            names.push(name);
        }
    }
    RequireFields::Named(names)
}
