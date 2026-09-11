//! Which file `require("<name>")` resolves to, what that hides, and where each of them
//! came from — the report behind `htl resolve`.
//!
//! A name resolves to one file out of everything on the search path, and until this
//! existed the only way to learn which was to provoke a diagnostic: `duplicate-declaration`
//! fires only when there is more than one, only at a `require` site a check happened to
//! reach, and frames an override — the thing a search path is for — as a defect. The
//! ordinary question is the other one. Which of these is in effect, and why that one.
//!
//! Nothing here decides anything. The order is [`Htl::module_candidates`]'s, which is the
//! searchers' own; the winner is [`Htl::resolve_module`]'s, which is what the checker asks;
//! this walks the two side by side and says which row is which.

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
    /// The check reads the `.d.tl`, and at run time the searcher steps aside for this
    /// file (`prelude.lua`, `resolve_for_require`).
    Runtime,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Shadowed => "shadowed",
            Self::Runtime => "runtime",
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
    pub path: String,
    /// The search-path directory it was found under.
    pub dir: String,
    pub kind: ModuleKind,
    pub status: Status,
    /// The `order` of the candidate that is read instead of this one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shadowed_by: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<Origin>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub candidates: usize,
    pub shadowed: usize,
    /// The name resolves to a file. What the exit code says.
    pub ok: bool,
}

/// What `htl resolve <module>` answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Resolution {
    pub module: String,
    /// The file the checker reads, absent when the name resolves to nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read: Option<String>,
    pub candidates: Vec<Candidate>,
    /// Every directory the search path consults, in order, whether or not it held
    /// anything for this name. A directory that holds nothing is half the answer when
    /// the name resolves to nothing at all.
    pub searched: Vec<String>,
    pub summary: Summary,
}

/// Ask the checker `h` — set up as the project's, by whoever built it — what `name`
/// resolves to, and report the whole chain.
///
/// `root` is the directory holding `htl.toml`, and only decides how paths are printed:
/// what is inside the project reads relative to it, and what is not stays absolute.
/// `project` is the mlua-pkg project when there is one, for naming the dependency a
/// candidate was installed or vendored from.
pub fn resolve(
    h: &Htl,
    name: &str,
    root: Option<&Path>,
    project: Option<&crate::pkg::Project>,
) -> Result<Resolution> {
    let candidates = h.module_candidates(name)?;
    let dirs = h.search_path_dirs()?;
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
            let dir = dir_of(c, &dirs);
            Candidate {
                order,
                path: show(&c.path, root),
                dir: show(&dir, root),
                kind: c.kind,
                status,
                shadowed_by: (status == Status::Shadowed).then_some(read_at).flatten(),
                origin: origin_of(&c.path, &dir, project),
            }
        })
        .collect::<Vec<_>>();

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
        searched,
        summary: Summary {
            candidates: rows.len(),
            shadowed,
            ok: read.is_some(),
        },
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

/// Where the file came from, when it came from anywhere but the project's own tree.
fn origin_of(path: &Path, dir: &Path, project: Option<&crate::pkg::Project>) -> Option<Origin> {
    // A declaration materialised from a crate: the note beside it names the crate and the
    // version, which is the whole reason `htl dts` writes one.
    if let Some(note) = crate::dep_dts::Note::read(dir) {
        return Some(Origin {
            kind: OriginKind::Crate,
            name: note.package,
            version: Some(note.version),
        });
    }
    // A hand-laid `types/<lib>/` carries no note, and the path below `types/` is then the
    // module name rather than a package: nothing to attribute it to.
    let p = project?;
    // `entries/` is where `require` reads a dep; `vendored/` is the root beside it, and a
    // path through either names the dependency the same way.
    if let Some(name) = under(path, &p.entries).or_else(|| under(path, &p.vendored)) {
        return Some(Origin {
            kind: OriginKind::Dependency,
            name,
            version: None,
        });
    }
    for copy in &p.vendored_copies {
        if starts_with(path, copy) {
            return Some(Origin {
                kind: OriginKind::Vendored,
                name: dir_name(copy),
                version: None,
            });
        }
    }
    for patch in &p.patches {
        if starts_with(path, &patch.dir) {
            return Some(Origin {
                kind: OriginKind::Patched,
                name: patch.name.clone(),
                version: None,
            });
        }
    }
    None
}

/// The name of the first directory of `path` below `dir`, when `path` is below it: the
/// dependency an installed file belongs to.
fn under(path: &Path, dir: &Path) -> Option<String> {
    let (p, d) = (canon(path), canon(dir));
    let rest = p.strip_prefix(&d).ok()?;
    Some(
        rest.components()
            .next()?
            .as_os_str()
            .to_string_lossy()
            .into(),
    )
}

fn starts_with(path: &Path, dir: &Path) -> bool {
    canon(path).starts_with(canon(dir))
}

fn dir_name(p: &Path) -> String {
    p.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
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
