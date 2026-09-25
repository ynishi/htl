//! Which file `require("<name>")` resolves to, what that hides, and where each of them
//! came from — the report behind `htl resolve`.
//!
//! A name resolves to one file out of everything on the search path, and until this
//! existed the only way to learn which was to provoke a diagnostic: `duplicate-declaration`
//! fires only when there is more than one, only at a `require` site a check happened to
//! reach, and frames an override — the thing a search path is for — as a defect. The
//! ordinary question is the other one. Which of these is in effect, and why that one.
//!
//! Nothing here decides anything. For a name the project model has, the answer is the
//! model's [`Resolver`](crate::model::Resolver)'s — the one every command resolves with —
//! and the rows are every file of the model under that name, in the model's order. For a
//! name the model does not have, the rows are what `package.path` holds for it
//! ([`Htl::module_candidates`], in the searchers' order) and the winner is what the
//! checker asks ([`Htl::resolve_module`]); a model file the path would find under a name
//! the model does not give it is not among them.
//!
//! A name the host provides ([`Project::provides`](crate::model::Project::provides)) is
//! answered by no file: the report says which source provides it
//! ([`Resolution::provided_by`]), and its rows are at most the declaration the checker
//! types it from. So is a name the model has only a declaration for, which the
//! environment provides ([`Provider::Declared`](crate::model::Provider::Declared)): the
//! report says which declaration says so, and that declaration is the row read. Both come
//! from the resolver's [`provides`](crate::model::Resolver::provides), the answer the
//! linker leaves a name out of a bundle on. When a file of the model implements the name as well, the model refuses
//! it: those rows are [`Status::Refused`], the report carries the resolver's message
//! ([`Resolution::error`]), and the verdict is a failure, as for a name two files
//! implement.

use crate::{Htl, ModuleCandidate, ModuleKind, same_file};
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// What became of one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// The file the checker reads for this name.
    Read,
    /// Reachable, and not read: an earlier candidate answered first.
    Shadowed,
    /// Not read, but not hidden either: the `.lua` implementation a declaration types.
    /// The check reads the `.d.tl`, and at run time this file is what loads.
    Runtime,
    /// One of two implementations of the name, which is an error: no order picks one.
    Ambiguous,
    /// An implementation of a name the host provides, which is an error: every run with
    /// the host loads the host's module, so this file would be checked and never run. The
    /// check reads the name's declaration, when it has one, and never this.
    Refused,
}

impl Status {
    /// The lowercase word the `status` column prints and `--format json` carries. One
    /// spelling for both forms of the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Shadowed => "shadowed",
            Self::Runtime => "runtime",
            Self::Ambiguous => "ambiguous",
            Self::Refused => "refused",
        }
    }
}

/// How a file that is not the project's own arrived in the tree. The fact a reader of
/// `types/<crate>/` most wants is which crate it came from, and nothing in the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OriginKind {
    /// Materialised from a dependency crate's `[package.metadata.htl] dts` by `htl dts`.
    Crate,
    /// Installed under `.htl/modules` by `htl pkg install`.
    Dependency,
    /// A `target_dir` copy: a dependency's source committed in the tree, rewritten by
    /// every install.
    Vendored,
    /// A `patch_dir` dependency: its source taken into the tree, and the project's from
    /// then on.
    Patched,
}

/// Where a candidate came from, when it came from somewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Origin {
    /// Which of the four brought it here, which is what decides where an edit to it
    /// belongs — the crate, the lockfile, or the tree.
    pub kind: OriginKind,
    /// The crate or dependency that ships it.
    pub name: String,
    /// The version recorded beside a materialised declaration (`.htl-dts`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Origin {
    /// As a line of the report reads it: `shipped by htl-mq 0.2.0`.
    pub fn describe(&self) -> String {
        let what = match self.kind {
            OriginKind::Crate => "shipped by",
            OriginKind::Dependency => "installed from",
            OriginKind::Vendored => "vendored copy of",
            OriginKind::Patched => "patched",
        };
        match &self.version {
            Some(v) => format!("{what} {} {v}", self.name),
            None => format!("{what} {}", self.name),
        }
    }
}

/// One file the name could have resolved to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Candidate {
    /// Its place in the order the searchers consult, from 1.
    pub order: usize,
    /// The file itself, relative to the project root when there is one. This is the row's
    /// answer; every other field says what to make of it.
    pub path: String,
    /// The search-path directory it was found under.
    pub dir: String,
    /// Declaration, Teal or Lua — which decides what a reader gets from it, and whether
    /// something else has to supply the implementation.
    pub kind: ModuleKind,
    /// Whether the checker reads this one, and if not, why not.
    pub status: Status,
    /// The `order` of the candidate that is read instead of this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<usize>,
    /// Where it came from, when it is not the project's own file. `None` is the ordinary
    /// case: a path under `src/` or `types/` that somebody here wrote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

/// Who answered: the project model, or — for a name the model does not have — the search
/// path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnsweredBy {
    /// The project model's [`Resolver`](crate::model::Resolver): the rows are every file of
    /// the model under the name.
    Model,
    /// `package.path`, which holds only what the model does not have — the directories
    /// Lua searches for libraries installed on the machine, or, outside a project, the
    /// directory of the file asked from.
    Path,
}

/// The counts a summary line is made of, and the verdict an exit code reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    /// How many files the name could have resolved to — the length of
    /// [`Resolution::candidates`].
    pub candidates: usize,
    /// How many of them an earlier candidate answers for. Not a defect on its own: an
    /// override is what a search path is for, and this is the number that says one is
    /// happening.
    pub shadowed: usize,
    /// The name resolves to a file, or the host provides it, and the model does not
    /// refuse it. What the exit code says: a name that resolves to nothing says so and
    /// exits non-zero, so a script can ask, and `--format json` carries the same rows.
    pub ok: bool,
}

/// What `htl resolve <module>` answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolution {
    /// The name as it was asked about — what a `require` in the sources spells, echoed so
    /// a stored or piped report says what question it answers.
    pub module: String,
    /// The file the checker reads, absent when the name resolves to nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<String>,
    /// Every file the name could have resolved to, in the order the searchers consult
    /// them — the read one among them rather than pulled out, because its place in the
    /// order is the explanation.
    pub candidates: Vec<Candidate>,
    /// Who answered. The rows of a model answer are the model's files, and `searched` has
    /// nothing to do with them.
    pub answered_by: AnsweredBy,
    /// Every directory the search path consults, in order, whether or not it held
    /// anything for this name. A directory that holds nothing is half the answer when
    /// the name resolves to nothing at all. In a project none of them is the project's:
    /// the model answers for its own directories.
    pub searched: Vec<String>,
    /// Who provides the name at run time, when no file of the project does — the wording
    /// the resolver's messages use ([`Resolver::provided_by`](crate::model::Resolver::provided_by)):
    /// `#[host_module] in Cargo.toml's crate`, `[build] host in htl.toml`, `htl's std` for
    /// the host, `declared by types/socket/http.d.tl` for a name the model has only a
    /// declaration for, which the environment provides. Such a name is answered at run
    /// time by the host or the environment, not by any row: a row read for it is the
    /// declaration it is typed from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provided_by: Option<String>,
    /// Which of the sources [`provided_by`](Self::provided_by) words, for a reader that
    /// branches on it rather than on the wording — the text report heads a name the host
    /// provides differently from one the environment does. Not in the JSON, which carries
    /// the wording alone.
    #[serde(skip)]
    pub provider: Option<crate::model::Provider>,
    /// Why the name is an error, when it is one the model refuses: the resolver's message,
    /// the one a check reports at a `require` of it. Set for a name the host provides that
    /// a file of the model implements as well (the rows marked [`Status::Refused`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The counts and the verdict, for a caller that wants the answer without walking the
    /// rows.
    pub summary: Summary,
}

/// Ask the checker `h` — set up as the project's, by whoever built it — what `name`
/// resolves to, and report the whole chain.
///
/// `root` is the directory holding `htl.toml`, and only decides how paths are printed:
/// what is inside the project reads relative to it, and what is not stays absolute.
/// `model` is the project's [model](crate::model) when there is one, for naming the
/// dependency or crate a candidate came from.
pub fn resolve(
    h: &Htl,
    name: &str,
    root: Option<&Path>,
    model: Option<&crate::model::Project>,
) -> Result<Resolution> {
    let dirs = h.search_path_dirs()?;
    let resolver = model.map(crate::model::Resolver::new);
    let (rows, read, error, answered_by) = match resolver
        .as_ref()
        .zip(model)
        .and_then(|(r, m)| model_rows(r, name, root, m))
    {
        Some(m) => (m.rows, m.read, m.error, AnsweredBy::Model),
        None => {
            let (rows, read) = path_rows(h, name, &dirs, root, model, resolver.as_ref())?;
            (rows, read, None, AnsweredBy::Path)
        }
    };
    let provider = resolver.as_ref().and_then(|r| r.provides(name));
    let provided_by = resolver
        .as_ref()
        .zip(provider)
        .map(|(r, p)| r.provided_by(name, p));

    let shadowed = rows.iter().filter(|c| c.status == Status::Shadowed).count();
    // One entry per directory as it is printed: the project root reached through
    // `htl.toml` and reached again as the working directory is one directory, and saying
    // it twice would read as two places to look.
    let mut searched: Vec<String> = Vec::new();
    for d in &dirs {
        let d = show(d, root);
        if !searched.contains(&d) {
            searched.push(d);
        }
    }
    Ok(Resolution {
        module: name.to_string(),
        read: read.as_ref().map(|p| show(p, root)),
        answered_by,
        searched,
        summary: Summary {
            candidates: rows.len(),
            shadowed,
            ok: error.is_none() && (read.is_some() || provided_by.is_some()),
        },
        provided_by,
        provider,
        error,
        candidates: rows,
    })
}

impl ModuleKind {
    /// What a path is by its name, for a file the checker has already chosen.
    fn of_path(p: &Path) -> Self {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.ends_with(".d.tl") {
            Self::Declaration
        } else if name.ends_with(".tl") {
            Self::Source
        } else {
            Self::Lua
        }
    }
}

/// What the model answers for a name it has: the rows, the file read, and the error when
/// it refuses the name.
struct ModelRows {
    rows: Vec<Candidate>,
    read: Option<PathBuf>,
    error: Option<String>,
}

/// The rows for a name the model has: every file of the model under it, the one the
/// resolver answers with marked `read`. `None` when the model does not have the name.
/// The rows are in the order a name is answered, by kind first — a source, then a
/// declaration, then plain Lua — so row 1 is not necessarily the earliest directory: a
/// source beats a declaration wherever the two sit.
///
/// A name the host provides that a file implements as well reads its declaration, when
/// it has one — the check types the `require` from it — and marks the implementations
/// [`Status::Refused`], with the resolver's message as the error.
fn model_rows(
    r: &crate::model::Resolver,
    name: &str,
    root: Option<&Path>,
    model: &crate::model::Project,
) -> Option<ModelRows> {
    use crate::model::Resolution as Answer;
    let mut refused: Vec<PathBuf> = Vec::new();
    let mut error = None;
    let (read, lua, ambiguous) = match r.resolve(None, name) {
        Answer::Found(f) => {
            let read = f
                .implementation
                .clone()
                .or_else(|| f.declaration.clone())
                .or_else(|| f.lua.clone());
            (read, f.lua, Vec::new())
        }
        Answer::Ambiguous(claims) => (None, None, claims.into_iter().map(|(_, f)| f).collect()),
        Answer::HostShadowed(s) => {
            refused = s.files;
            error = Some(s.message);
            (s.declaration, None, Vec::new())
        }
        _ => return None,
    };
    let mut files = r.claims(name);
    // A name only the resolver can spell (`@<dependency>/…`) has no plain claims.
    for f in read.iter().chain(lua.iter()).chain(refused.iter()) {
        if !files.iter().any(|x| same_file(x, f)) {
            files.push(f.clone());
        }
    }
    // In the order a name is answered: a source, then a declaration, then plain Lua —
    // the model's order within each, so the project's own comes first.
    let rank = |f: &PathBuf| match ModuleKind::of_path(f) {
        ModuleKind::Source => 0,
        ModuleKind::Declaration => 1,
        ModuleKind::Lua => 2,
    };
    files.sort_by_key(rank);
    let read_kind = read.as_deref().map(ModuleKind::of_path);
    let read_at = read
        .as_ref()
        .and_then(|p| files.iter().position(|f| same_file(f, p)))
        .map(|i| i + 1);
    let rows = files
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let order = i + 1;
            let status = if read_at == Some(order) {
                Status::Read
            } else if ambiguous.iter().any(|a| same_file(a, f)) {
                Status::Ambiguous
            } else if refused.iter().any(|a| same_file(a, f)) {
                Status::Refused
            } else if read_kind == Some(ModuleKind::Declaration)
                && lua.as_ref().is_some_and(|l| same_file(f, l))
            {
                Status::Runtime
            } else {
                Status::Shadowed
            };
            Candidate {
                order,
                path: show(f, root),
                dir: show(&root_holding(f, model), root),
                kind: ModuleKind::of_path(f),
                status,
                shadowed_by: (status == Status::Shadowed).then_some(read_at).flatten(),
                origin: origin_of(f, Some(model)),
            }
        })
        .collect();
    Some(ModelRows { rows, read, error })
}

/// The rows for a name outside the model: what `package.path` holds for it, a model file
/// found under a name the model does not give it left out.
fn path_rows(
    h: &Htl,
    name: &str,
    dirs: &[PathBuf],
    root: Option<&Path>,
    model: Option<&crate::model::Project>,
    resolver: Option<&crate::model::Resolver>,
) -> Result<(Vec<Candidate>, Option<PathBuf>)> {
    let candidates: Vec<ModuleCandidate> = h
        .module_candidates(name)?
        .into_iter()
        .filter(|c| resolver.is_none_or(|r| !r.owns(&c.path)))
        .collect();
    // The winner is not recomputed here: it is what the checker itself answers, so a row
    // marked `read` is the file a check reads even if the two walks ever disagreed.
    let (read, lua) = h.resolve_module(name)?;
    let read_kind = read.as_deref().map(ModuleKind::of_path);
    let read_at = read
        .as_ref()
        .and_then(|r| candidates.iter().position(|c| same_file(&c.path, r)))
        .map(|i| i + 1);
    let rows = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let order = i + 1;
            let status = if read_at == Some(order) {
                Status::Read
            } else if read_kind == Some(ModuleKind::Declaration)
                && lua.as_ref().is_some_and(|l| same_file(&c.path, l))
            {
                // A declaration was read and this is the `.lua` behind it: the run loads
                // this file. Calling that shadowed would be the wrong answer to the
                // question the command is for.
                Status::Runtime
            } else {
                Status::Shadowed
            };
            let dir = dir_of(c, dirs);
            Candidate {
                order,
                path: show(&c.path, root),
                dir: show(&dir, root),
                kind: c.kind,
                status,
                shadowed_by: (status == Status::Shadowed).then_some(read_at).flatten(),
                origin: origin_of(&c.path, model),
            }
        })
        .collect();
    Ok((rows, read))
}

/// The model's root that holds `path` — the most specific one, the root that names it —
/// or its parent when none does.
fn root_holding(path: &Path, model: &crate::model::Project) -> PathBuf {
    let p = canon(path);
    model
        .modules
        .iter()
        .flat_map(|m| m.roots.iter().map(|(_, r)| r.to_path_buf()))
        .filter(|r| p.starts_with(canon(r)))
        .max_by_key(|r| canon(r).components().count())
        .unwrap_or_else(|| crate::parent_dir(path))
}

/// Which search-path directory a candidate belongs to: the most specific one that holds
/// it.
///
/// Not the template it was found through. `add_path` puts `<dir>/?/?.lua` on the path as
/// well as `<dir>/?.lua`, so `types/dep/dep.d.tl` is reached through `types/` before
/// `types/dep/` is ever consulted — and answering `types` would drop the very thing a
/// reader of `types/<crate>/` is asking about. The order the row is printed in is the
/// searchers' own either way; this only decides what the row is attributed to.
fn dir_of(c: &ModuleCandidate, dirs: &[PathBuf]) -> PathBuf {
    let p = canon(&c.path);
    dirs.iter()
        .filter(|d| p.starts_with(canon(d)))
        .max_by_key(|d| canon(d).components().count())
        .cloned()
        .unwrap_or_else(|| c.dir.clone())
}

/// Where the file came from, when it came from anywhere but the project's own tree: the
/// module that owns it in the [model](crate::model), when that module is a dependency or
/// a crate's declarations.
///
/// The model answers for the file, not for the directory the searcher found it through,
/// so a declaration under `types/<crate>/` is the crate's however the path reached it.
/// The project's own modules, contract and `[check] paths` directories and htl's library
/// have no origin to report.
fn origin_of(path: &Path, model: Option<&crate::model::Project>) -> Option<Origin> {
    use crate::model::Owner;
    let module = model?.locate(path)?.module;
    let (kind, version) = match &module.owner {
        Owner::Crate { version } => (OriginKind::Crate, version.clone()),
        Owner::Installed => (OriginKind::Dependency, None),
        Owner::Vendored => (OriginKind::Vendored, None),
        Owner::Patched => (OriginKind::Patched, None),
        Owner::Own | Owner::Contract | Owner::External | Owner::Lib => return None,
    };
    Some(Origin {
        kind,
        name: module.name.clone(),
        version,
    })
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// How a path is printed: relative to the project root when it is inside it, and as it is
/// when it is not. What a reader recognises is `types/mq.d.tl`, and where the project sits
/// on this machine is not part of the answer.
fn show(p: &Path, root: Option<&Path>) -> String {
    let Some(root) = root else {
        return p.display().to_string();
    };
    let (a, b) = (canon(p), canon(root));
    match a.strip_prefix(&b) {
        Ok(rest) if rest.as_os_str().is_empty() => ".".to_string(),
        Ok(rest) => rest.display().to_string(),
        Err(_) => p.display().to_string(),
    }
}
