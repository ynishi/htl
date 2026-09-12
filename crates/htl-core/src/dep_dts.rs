//! Declarations a *dependency* crate ships, materialised into the project's `types/`.
//!
//! [`dts::generate_crate`](crate::dts::generate_crate) writes the declarations one
//! crate's own `#[host_module]` / `#[derive(TealRecord)]` ask for, by scanning its Rust
//! sources. A host module that arrives *as a dependency* has no such path: its Rust
//! source is in someone else's package, and the checker searches `types/`, which until
//! now held only what a person put there by hand.
//!
//! So a crate ships its declarations as files and names them in its manifest:
//!
//! ```toml
//! [package.metadata.htl]
//! dts = ["dts/mq.d.tl"]
//! ```
//!
//! or, for a crate whose declarations are one per module in one directory, a `*` in the
//! file name — `dts = ["types/mlua_batteries/*.d.tl"]` — which is read here against the
//! crate's own tree, so the manifest does not repeat a list the crate's sources already
//! are. Only the file name may hold a `*`; the directory is spelled out. A pattern that
//! matches nothing is reported under the pattern's own name, like a file that is not there.
//!
//! and every project depending on it materialises those files under
//! `types/<crate>/<file>`, written with [`write_if_changed`](crate::write_if_changed) and
//! committed like the declarations the project generates itself.
//!
//! # A crate whose modules have a namespace says where its paths start
//!
//! `types/<crate>/` is on the search path, so a file's own name is the module name:
//! `htl-mq`'s `dts/mq.d.tl` is `require("mq")` however deep in its package it sat. That is
//! right for a crate with a module or two, and wrong for one that registers `mine.thing` —
//! the namespace is in the path and only the name survives, so the module a project has to
//! `require` is not the module the crate registered, and two modules called `log` in two
//! namespaces both want the same file.
//!
//! Such a crate names the directory its paths start at:
//!
//! ```toml
//! [package.metadata.htl]
//! dts_root = "types"
//! dts = ["types/mine/thing.d.tl", "types/other/log.d.tl"]
//! ```
//!
//! and what is below that root is the module path, kept: `types/<crate>/mine/thing.d.tl`,
//! `require("mine.thing")`. Without the key nothing changes — the file name alone, which
//! is what every crate shipping declarations today is written for. An entry that does not
//! start at the root is that manifest's mistake and is reported like a file it does not
//! ship.
//!
//! Copying rather than searching the dependency where cargo unpacked it is what makes a
//! fresh clone check: the registry cache is machine-local and empty until someone builds,
//! `types/` is in the repository. It is also what keeps `include_tl!` out of cargo — the
//! macro reads `types/` as it always has, and a proc macro shelling out to `cargo
//! metadata` on every build is the cost this avoids.
//!
//! # Why a subprocess, and why the graph
//!
//! Resolving which crates are dependencies is cargo's answer to give: a manifest names
//! versions, not the ones that were resolved, and a registry dependency's files are under
//! `$CARGO_HOME/registry/src/<registry>/<name>-<version>/`, a layout no project should be
//! reimplementing. `cargo metadata` reports both — the resolved graph, and each package's
//! `manifest_path` wherever cargo put it — and it neither builds nor, with everything
//! already fetched, touches the network.
//!
//! It is the whole graph rather than the direct dependencies because a runtime crate may
//! well be pulled in by the one the project names; a module is registered in the Lua state
//! either way.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One declaration file a dependency ships, resolved to where it is on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepDecl {
    /// The package that ships it.
    pub package: String,
    /// The resolved version, as `cargo metadata` reports it — so a problem names the
    /// version the run actually read, not the requirement the manifest wrote. It is what
    /// the [`Note`] records beside the copy, and what every message about this entry
    /// carries.
    pub version: String,
    /// The path inside the package, as that manifest wrote it.
    pub declared: String,
    /// That manifest's `dts_root`, when it set one: the directory [`declared`](Self::declared)
    /// starts at. Kept as the manifest wrote it rather than applied here, because what it
    /// cannot be applied to is a problem to report beside the others — see [`Self::under`].
    pub dts_root: Option<String>,
    /// Where cargo has that file on this machine.
    pub source: PathBuf,
}

impl DepDecl {
    /// The path it takes under `types/<package>/` — which is what `require` says — or why
    /// the manifest does not name one.
    ///
    /// A method rather than the field it used to be, now that a crate can have a
    /// namespace: without a root it is the file name, one component that cannot fail to
    /// exist; under one it is the rest of the path below that root, which is several, and
    /// which an entry the root does not cover has none of. That is the manifest's mistake,
    /// and it reads as one rather than as an empty name.
    pub fn under(&self) -> Result<PathBuf, String> {
        let declared = Path::new(&self.declared);
        let Some(root) = &self.dts_root else {
            return match declared.file_name() {
                Some(name) => Ok(PathBuf::from(name)),
                None => Err(self.says("is not a path to a file")),
            };
        };
        declared
            .strip_prefix(root)
            .map(Path::to_path_buf)
            .map_err(|_| {
                self.says(&format!(
                    "does not start at the dts_root {root:?} that manifest declares"
                ))
            })
    }

    /// Where it is materialised, given the directory `types/` sits in.
    pub fn target(&self, root: &Path) -> Result<PathBuf, String> {
        Ok(root.join("types").join(&self.package).join(self.under()?))
    }

    /// `<crate> <version> names <entry> in [package.metadata.htl] dts, which …` — the one
    /// sentence every problem about an entry is a tail of, so that a reader always learns
    /// whose manifest to open and which of its lines.
    fn says(&self, tail: &str) -> String {
        format!(
            "{} {} names {} in [package.metadata.htl] dts, which {tail}",
            self.package, self.version, self.declared
        )
    }
}

/// Whether this build carries `name`'s declarations itself, so that a copy of them under
/// `types/<name>/` is not the one a script reaches.
///
/// True for exactly one crate and only with the `std` feature: mlua-batteries, whose
/// modules [`Htl::install_std`](crate::Htl::install_std) preloads as `std.*` and whose
/// declarations it writes under [`lib_dir`](crate::lib_dir) with that prefix.
///
/// Asked in two places — [`decls`], which does not materialise such a crate, and
/// [`orphans`], which says so about a directory left from a build that did. It is a fact
/// about how this binary was compiled rather than anything read out of `cargo metadata`,
/// so both ask it directly instead of one carrying the answer to the other: threading it
/// through would change [`resolve`]'s signature to pass along something neither the graph
/// nor the manifest said.
fn carried_by_std(name: &str) -> bool {
    #[cfg(feature = "std")]
    {
        name == crate::batteries::CRATE
    }
    #[cfg(not(feature = "std"))]
    {
        let _ = name;
        false
    }
}

/// A path as it goes into the note and into a comparison: `/` whatever the platform
/// separates with, so a note written on Windows reads the same everywhere and an orphan
/// is told from a live declaration by the same string on both.
fn slashed(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The note beside a materialised set, read back to tell a directory this command wrote
/// from one a person laid out by hand (a hand-written `types/a/b.d.tl` is `require("a.b")`
/// and none of this command's business).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    /// The crate the copy came from, which is where a change to it belongs. Written as
    /// `crate` in the file, since that is the word a reader of the directory wants and
    /// `crate` is not a field name Rust will take.
    #[serde(rename = "crate")]
    pub package: String,
    /// The version it was taken from, so that a stale copy can be told from a current one
    /// by reading rather than by re-resolving.
    pub version: String,
    /// What this run wrote, as paths below `types/<crate>/`. A record for whoever opens
    /// the directory: [`orphans`] walks the disk instead of trusting it, because the note
    /// is rewritten whenever anything is materialised and a file dropped from the
    /// manifest would otherwise stop being reported by the very run that noticed it.
    pub files: Vec<String>,
}

impl Note {
    fn text(&self) -> String {
        let files: Vec<String> = self.files.iter().map(|f| format!("{f:?}")).collect();
        format!(
            "# Written by `htl dts`. The declarations in this directory were taken from the\n\
             # crate named here, which is where a change to them belongs; this copy is\n\
             # committed so that a checkout checks before anything is built.\n\
             crate = {:?}\nversion = {:?}\nfiles = [{}]\n",
            self.package,
            self.version,
            files.join(", ")
        )
    }

    /// The note in `dir`, when there is one to read.
    pub fn read(dir: &Path) -> Option<Note> {
        let text = std::fs::read_to_string(dir.join(crate::DEP_TYPES_NOTE)).ok()?;
        toml::from_str(&text).ok()
    }
}

pub use crate::materialised_types_dirs as materialised_dirs;

/// The `cargo` to run: the one running us when cargo is what started this, so a toolchain
/// selected for the build is the toolchain the graph is resolved with.
fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Every declaration shipped by a dependency of the package at `manifest_dir`.
///
/// The package's own `[package.metadata.htl] dts` is not among them: its declarations are
/// generated from its sources, and materialising a crate into itself would be writing the
/// same file twice under two names.
pub fn resolve(manifest_dir: &Path) -> Result<Vec<DepDecl>, String> {
    let manifest = manifest_dir.join("Cargo.toml");
    let out = std::process::Command::new(cargo())
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest)
        .output()
        .map_err(|e| format!("running `cargo metadata` for {}: {e}", manifest.display()))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no output");
        return Err(format!(
            "`cargo metadata` for {}: {first}",
            manifest.display()
        ));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("reading `cargo metadata`: {e}"))?;
    decls(&meta)
}

/// The shipped declarations named in one `cargo metadata` document.
fn decls(meta: &serde_json::Value) -> Result<Vec<DepDecl>, String> {
    let Some(root) = meta["resolve"]["root"].as_str() else {
        // A virtual workspace manifest resolves no root package; nothing depends on
        // anything here.
        return Ok(Vec::new());
    };
    let mut edges: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for node in meta["resolve"]["nodes"].as_array().into_iter().flatten() {
        let Some(id) = node["id"].as_str() else {
            continue;
        };
        edges.insert(
            id,
            node["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|d| d.as_str())
                .collect(),
        );
    }
    // The whole graph below the root, the root itself excluded.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: Vec<&str> = edges.get(root).cloned().unwrap_or_default();
    while let Some(id) = queue.pop() {
        if !seen.insert(id) {
            continue;
        }
        queue.extend(edges.get(id).into_iter().flatten().copied());
    }
    let mut out = Vec::new();
    for pkg in meta["packages"].as_array().into_iter().flatten() {
        if !pkg["id"].as_str().is_some_and(|id| seen.contains(id)) {
            continue;
        }
        let (Some(name), Some(version), Some(mpath)) = (
            pkg["name"].as_str(),
            pkg["version"].as_str(),
            pkg["manifest_path"].as_str(),
        ) else {
            continue;
        };
        // The crate behind `std.*` is in every graph that has the feature on, and its
        // declarations are already under `lib_dir()` with the prefix the preload uses.
        // A copy under `types/mlua-batteries/` would be the same modules a second time
        // under a name nothing answers `require` for. `orphans` says that about one a
        // build without the feature already wrote.
        if carried_by_std(name) {
            continue;
        }
        let dts = &pkg["metadata"]["htl"]["dts"];
        if dts.is_null() {
            continue;
        }
        let Some(list) = dts.as_array() else {
            return Err(format!(
                "{name} {version}: [package.metadata.htl] dts is not a list of paths"
            ));
        };
        // Whether the key is a string is the manifest's shape, like `dts` itself, and is
        // settled here; whether it covers an entry is about that entry and is settled
        // where the entry is written.
        let dts_root = match &pkg["metadata"]["htl"]["dts_root"] {
            serde_json::Value::Null => None,
            serde_json::Value::String(s) => Some(s.clone()),
            _ => {
                return Err(format!(
                    "{name} {version}: [package.metadata.htl] dts_root is not a path"
                ));
            }
        };
        let dir = crate::parent_dir(Path::new(mpath));
        for entry in list {
            let Some(rel) = entry.as_str() else {
                return Err(format!(
                    "{name} {version}: [package.metadata.htl] dts holds a value that is not a path"
                ));
            };
            for rel in expand(&dir, rel) {
                out.push(DepDecl {
                    package: name.to_string(),
                    version: version.to_string(),
                    source: dir.join(&rel),
                    dts_root: dts_root.clone(),
                    declared: rel,
                });
            }
        }
    }
    // In the order they are written and reported in, which is where they land rather than
    // where they came from: the file name when that is all there is, the path below the
    // root when there is one. An entry no root covers has no place to land and sorts by
    // what the manifest said, beside the entries it was written next to.
    out.sort_by_cached_key(|d| {
        (
            d.package.clone(),
            d.under().unwrap_or_else(|_| PathBuf::from(&d.declared)),
        )
    });
    Ok(out)
}

/// One manifest entry as the paths it stands for, relative to the crate's directory.
///
/// A plain path is itself, untouched — no directory is read for it, so a graph whose
/// declarations are named one by one costs nothing here. A file name with a `*` in it is
/// matched against the names in its directory, in name order. When the directory cannot be
/// read or nothing in it matches, the pattern comes back as written: [`materialise`] then
/// reports it the way it reports a named file that is not there, and the message carries
/// the pattern, which is the line in the manifest the reader has to fix.
fn expand(dir: &Path, rel: &str) -> Vec<String> {
    let path = Path::new(rel);
    let Some(pattern) = path.file_name().and_then(|f| f.to_str()) else {
        return vec![rel.to_string()];
    };
    if !pattern.contains('*') {
        return vec![rel.to_string()];
    }
    let parent = path.parent().unwrap_or(Path::new(""));
    let Ok(read) = std::fs::read_dir(dir.join(parent)) else {
        return vec![rel.to_string()];
    };
    let mut names: Vec<String> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| wildcard(pattern, n))
        .collect();
    if names.is_empty() {
        return vec![rel.to_string()];
    }
    names.sort();
    names
        .into_iter()
        .map(|n| parent.join(n).to_string_lossy().into_owned())
        .collect()
}

/// `*` for any run of characters, everything else itself. The whole of what a file name in
/// `dts` may ask for: `*.d.tl`, `mq_*.d.tl`. No `?`, no classes, no `**` — a declaration
/// list is not a search, and a pattern that needs more than this is a list.
fn wildcard(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        let Some(i) = rest.find(part) else {
            return false;
        };
        rest = &rest[i + part.len()..];
    }
    rest.ends_with(last)
}

/// Write every shipped declaration under `root/types/<crate>/`, and a note beside each
/// crate's own directory saying where the files came from.
///
/// Returns `(target, written)` pairs in the shape `htl dts` reports, and what could not be
/// materialised. A crate naming a file it does not ship is a problem rather than a
/// silence: the declaration is missing from the project either way, and the project cannot
/// fix a manifest it does not own without being told which one is wrong.
///
/// The second list is this command's report on its own job — a declaration it was asked
/// for and did not write — not a finding about the project's code. It carries no rule
/// name: there is nothing to configure or silence, and none of it reaches the `Sink`, so
/// `htl check` neither counts one nor reports one in `--format json`. `htl dts` prints
/// them under `not written` and exits non-zero on them.
pub fn materialise(root: &Path, decls: &[DepDecl]) -> (Vec<(PathBuf, bool)>, Vec<String>) {
    let mut written = Vec::new();
    let mut problems = Vec::new();
    let mut notes: BTreeMap<&str, Note> = BTreeMap::new();
    let mut taken: BTreeMap<PathBuf, &DepDecl> = BTreeMap::new();
    for d in decls {
        let under = match d.under() {
            Ok(u) => u,
            Err(e) => {
                problems.push(e);
                continue;
            }
        };
        if !crate::is_declaration(&under) {
            problems.push(d.says("is not a `.d.tl` file"));
            continue;
        }
        let target = root.join("types").join(&d.package).join(&under);
        if let Some(first) = taken.get(&target) {
            problems.push(format!(
                "{} {} names both {} and {} in [package.metadata.htl] dts, which would be one \
                 file under types/{}/",
                d.package, d.version, first.declared, d.declared, d.package
            ));
            continue;
        }
        let text = match std::fs::read_to_string(&d.source) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!(
                    "{} {} names {} in [package.metadata.htl] dts: reading {}: {e}",
                    d.package,
                    d.version,
                    d.declared,
                    d.source.display()
                ));
                continue;
            }
        };
        match crate::write_if_changed(&target, &text) {
            Ok(w) => {
                taken.insert(target.clone(), d);
                notes
                    .entry(&d.package)
                    .or_insert_with(|| Note {
                        package: d.package.clone(),
                        version: d.version.clone(),
                        files: Vec::new(),
                    })
                    .files
                    .push(slashed(&under));
                written.push((target, w));
            }
            Err(e) => problems.push(format!(
                "{} {} names {} in [package.metadata.htl] dts: writing {}: {e}",
                d.package,
                d.version,
                d.declared,
                target.display()
            )),
        }
    }
    // The note is a record beside the files, not one of them: `.src` notes are written
    // the same way, and reporting it as a declaration would report one file too many.
    for (pkg, note) in &notes {
        let dir = root.join("types").join(pkg);
        if let Err(e) = crate::write_if_changed(&dir.join(crate::DEP_TYPES_NOTE), &note.text()) {
            problems.push(format!(
                "writing {}: {e}",
                dir.join(crate::DEP_TYPES_NOTE).display()
            ));
        }
    }
    (written, problems)
}

/// Declarations under `types/` this command materialised for a crate that is no longer a
/// dependency, that the crate no longer ships, or whose modules this build carries itself.
///
/// Reported, never deleted. What a file under `types/` is for is the project's to say —
/// scripts may still require the module, the dependency may be coming back on the next
/// branch — and a command that regenerates declarations is the wrong thing to be removing
/// committed files behind someone's back.
///
/// Like [`materialise`]'s problems these are `htl dts` saying what it did, not findings
/// about the project's code: no rule name, nothing to configure or silence, and nothing
/// that reaches the `Sink` to be counted or reported in `--format json`. `htl dts` prints
/// them under `left in place` and fails nothing — the file it names is still there and
/// still checked, exactly as before.
pub fn orphans(root: &Path, decls: &[DepDecl]) -> Vec<String> {
    let types = root.join("types");
    let mut current: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for d in decls {
        if let Ok(under) = d.under() {
            current
                .entry(&d.package)
                .or_default()
                .insert(slashed(&under));
        }
    }
    let mut out = Vec::new();
    for dir in materialised_dirs(&types) {
        let Some(note) = Note::read(&dir) else {
            continue;
        };
        let live = current.get(note.package.as_str());
        // What is on disk, not what the note lists: the note is rewritten whenever
        // anything is materialised, and a file dropped from a crate's manifest would
        // otherwise stop being reported by the very run that noticed it was gone.
        //
        // The whole tree below the crate's directory, not its top level: under a
        // `dts_root` a declaration lands at `mine/thing.d.tl`, and a namespace the crate
        // has stopped shipping is a directory of files nobody would otherwise hear about.
        let mut files: Vec<PathBuf> = walkdir::WalkDir::new(&dir)
            .sort_by_file_name()
            .into_iter()
            .filter_map(Result::ok)
            .map(|e| e.into_path())
            .filter(|p| p.is_file() && crate::is_declaration(p))
            .collect();
        files.sort();
        for path in files {
            let name = slashed(path.strip_prefix(&dir).unwrap_or(&path));
            if live.is_some_and(|files| files.contains(&name)) {
                continue;
            }
            // A crate this build carries is absent from `decls` because it was skipped,
            // not because it left the graph, and `live` cannot tell those apart — so ask
            // the same question `decls` asked. What is true of such a copy is not what is
            // true of an orphan: the crate is still a dependency, its modules are on the
            // path under `std.`, and this directory is on the path too, offering the same
            // modules under bare names that nothing preloads. A file that type-checks and
            // has no implementation is worth a firmer word than "when nothing requires it".
            let (why, advice) = if carried_by_std(&note.package) {
                (
                    format!(
                        "{} is on the path as std.* instead, and nothing preloads this copy's \
                         module name",
                        note.package
                    ),
                    "delete it",
                )
            } else {
                let why = match live {
                    Some(_) => format!("{} no longer ships it", note.package),
                    None => format!("{} is no longer a dependency", note.package),
                };
                (why, "delete it when nothing requires the module")
            };
            out.push(format!(
                "{}: {why}; {advice}",
                path.strip_prefix(root).unwrap_or(&path).display()
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(json: &str) -> serde_json::Value {
        serde_json::from_str(json).unwrap()
    }

    /// The graph, not the direct dependencies: `mid` is what the root names, and `leaf` is
    /// what ships a declaration.
    #[test]
    fn a_shipped_declaration_is_found_through_the_whole_graph() {
        let d = decls(&meta(
            r#"{
              "packages": [
                {"id": "root", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml"},
                {"id": "mid", "name": "mid", "version": "0.2.0", "manifest_path": "/w/mid/Cargo.toml"},
                {"id": "leaf", "name": "htl-mq", "version": "0.3.0", "manifest_path": "/w/mq/Cargo.toml",
                 "metadata": {"htl": {"dts": ["dts/mq.d.tl"]}}}
              ],
              "resolve": {"root": "root", "nodes": [
                {"id": "root", "dependencies": ["mid"]},
                {"id": "mid", "dependencies": ["leaf"]},
                {"id": "leaf", "dependencies": []}
              ]}
            }"#,
        ))
        .unwrap();
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].package, "htl-mq");
        assert_eq!(d[0].under(), Ok(PathBuf::from("mq.d.tl")));
        assert_eq!(d[0].source, PathBuf::from("/w/mq/dts/mq.d.tl"));
        assert_eq!(
            d[0].target(Path::new("/p")),
            Ok(PathBuf::from("/p/types/htl-mq/mq.d.tl"))
        );
    }

    /// The root's own key is not a dependency of itself: `htl dts` generates those
    /// declarations from its sources.
    #[test]
    fn the_root_package_ships_to_itself_through_its_sources_not_through_this() {
        let d = decls(&meta(
            r#"{
              "packages": [
                {"id": "root", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml",
                 "metadata": {"htl": {"dts": ["dts/app.d.tl"]}}}
              ],
              "resolve": {"root": "root", "nodes": [{"id": "root", "dependencies": []}]}
            }"#,
        ))
        .unwrap();
        assert!(d.is_empty(), "{d:?}");
    }

    /// A package that is in `packages` but not in the resolved graph (another platform's
    /// dependency, a workspace sibling nobody depends on) ships nothing here.
    #[test]
    fn a_package_outside_the_resolved_graph_is_not_a_dependency() {
        let d = decls(&meta(
            r#"{
              "packages": [
                {"id": "root", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml"},
                {"id": "other", "name": "other", "version": "0.1.0", "manifest_path": "/w/o/Cargo.toml",
                 "metadata": {"htl": {"dts": ["dts/o.d.tl"]}}}
              ],
              "resolve": {"root": "root", "nodes": [{"id": "root", "dependencies": []}]}
            }"#,
        ))
        .unwrap();
        assert!(d.is_empty(), "{d:?}");
    }

    #[test]
    fn a_dts_key_that_is_not_a_list_of_paths_is_an_error_naming_the_crate() {
        let e = decls(&meta(
            r#"{
              "packages": [
                {"id": "root", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml"},
                {"id": "dep", "name": "dep", "version": "0.1.0", "manifest_path": "/w/d/Cargo.toml",
                 "metadata": {"htl": {"dts": "dts/dep.d.tl"}}}
              ],
              "resolve": {"root": "root", "nodes": [
                {"id": "root", "dependencies": ["dep"]}, {"id": "dep", "dependencies": []}]}
            }"#,
        ))
        .unwrap_err();
        assert!(e.contains("dep 0.1.0"), "{e}");
        assert!(e.contains("not a list of paths"), "{e}");
    }

    /// A `*` in the file name is the files in that directory that match it, in name order,
    /// each with its own `declared` — so a problem with one of them names that one file. A
    /// plain path beside it is untouched, and no directory is read for it.
    #[test]
    fn a_star_in_the_file_name_is_every_matching_file_in_that_directory() {
        let dir = std::env::temp_dir().join(format!("htl-dep-dts-glob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("types/b")).unwrap();
        for f in ["json.d.tl", "env.d.tl", "README.md", "init.tl"] {
            std::fs::write(dir.join("types/b").join(f), "").unwrap();
        }
        let manifest = dir.join("Cargo.toml").display().to_string();
        let d = decls(&meta(&format!(
            r#"{{
              "packages": [
                {{"id": "root", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml"}},
                {{"id": "dep", "name": "b", "version": "0.7.0", "manifest_path": "{manifest}",
                 "metadata": {{"htl": {{"dts": ["types/b/*.d.tl", "extra/one.d.tl"]}}}}}}
              ],
              "resolve": {{"root": "root", "nodes": [
                {{"id": "root", "dependencies": ["dep"]}}, {{"id": "dep", "dependencies": []}}]}}
            }}"#
        )))
        .unwrap();
        let declared: Vec<&str> = d.iter().map(|d| d.declared.as_str()).collect();
        assert_eq!(
            declared,
            ["types/b/env.d.tl", "types/b/json.d.tl", "extra/one.d.tl"],
            "{d:?}"
        );
        assert_eq!(d[0].source, dir.join("types/b/env.d.tl"));
        assert_eq!(d[0].under(), Ok(PathBuf::from("env.d.tl")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pattern nothing matches is passed on as written, so the report names the pattern
    /// — the manifest line to fix — rather than nothing at all.
    #[test]
    fn a_star_that_matches_nothing_is_reported_as_the_pattern() {
        let dir = std::env::temp_dir().join(format!("htl-dep-dts-noglob-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("dts")).unwrap();
        std::fs::write(dir.join("dts/notes.txt"), "").unwrap();
        assert_eq!(expand(&dir, "dts/*.d.tl"), ["dts/*.d.tl"]);
        assert_eq!(expand(&dir, "missing/*.d.tl"), ["missing/*.d.tl"]);
        assert_eq!(expand(&dir, "dts/plain.d.tl"), ["dts/plain.d.tl"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_star_matches_a_run_of_anything_and_nothing_else_is_special() {
        assert!(wildcard("*.d.tl", "json.d.tl"));
        assert!(wildcard("*.d.tl", ".d.tl"));
        assert!(!wildcard("*.d.tl", "json.tl"));
        assert!(wildcard("mq_*.d.tl", "mq_a.d.tl"));
        assert!(!wildcard("mq_*.d.tl", "a_mq_a.d.tl"));
        assert!(wildcard("a*b*c", "abc"));
        assert!(wildcard("a*b*c", "axxbyyc"));
        assert!(!wildcard("a*bc*c", "abc"));
        assert!(!wildcard("?.d.tl", "a.d.tl"));
    }

    /// One `dts_root` fixture, so that every question below is asked of the same manifest.
    fn rooted(root: &str, entries: &str, manifest: &str) -> Result<Vec<DepDecl>, String> {
        decls(&meta(&format!(
            r#"{{
              "packages": [
                {{"id": "root", "name": "app", "version": "0.1.0", "manifest_path": "/w/app/Cargo.toml"}},
                {{"id": "dep", "name": "my-mod", "version": "0.1.0", "manifest_path": "{manifest}",
                 "metadata": {{"htl": {{"dts_root": {root}, "dts": {entries}}}}}}}
              ],
              "resolve": {{"root": "root", "nodes": [
                {{"id": "root", "dependencies": ["dep"]}}, {{"id": "dep", "dependencies": []}}]}}
            }}"#
        )))
    }

    /// What is below the root is the module path, kept: the namespace survives the copy,
    /// and two modules of the same name in two namespaces are two files.
    #[test]
    fn a_dts_root_keeps_the_path_below_it() {
        let d = rooted(
            "\"types\"",
            "[\"types/mine/thing.d.tl\", \"types/a/log.d.tl\", \"types/b/log.d.tl\"]",
            "/w/d/Cargo.toml",
        )
        .unwrap();
        let under: Vec<String> = d.iter().map(|d| slashed(&d.under().unwrap())).collect();
        assert_eq!(
            under,
            ["a/log.d.tl", "b/log.d.tl", "mine/thing.d.tl"],
            "{d:?}"
        );
        assert_eq!(
            d[2].target(Path::new("/p")),
            Ok(PathBuf::from("/p/types/my-mod/mine/thing.d.tl"))
        );
        assert_eq!(d[2].source, PathBuf::from("/w/d/types/mine/thing.d.tl"));
    }

    /// An entry the root does not cover is that manifest's mistake, named with the entry
    /// and the root, and it is a problem beside the others rather than a stop: the crate's
    /// other declarations are still written.
    #[test]
    fn an_entry_outside_the_dts_root_is_reported_naming_both() {
        let d = rooted(
            "\"types\"",
            "[\"types/mine/thing.d.tl\", \"dts/stray.d.tl\"]",
            "/w/d/Cargo.toml",
        )
        .unwrap();
        assert_eq!(d.len(), 2, "{d:?}");
        let stray = d.iter().find(|d| d.declared == "dts/stray.d.tl").unwrap();
        let e = stray.under().unwrap_err();
        assert!(e.contains("my-mod 0.1.0"), "{e}");
        assert!(e.contains("dts/stray.d.tl"), "{e}");
        assert!(e.contains("dts_root \"types\""), "{e}");
        assert!(d.iter().any(|d| d.under().is_ok()), "{d:?}");
    }

    /// The key says what the paths start at, so it is a path; anything else is the same
    /// kind of mistake as a `dts` that is not a list, and is answered the same way.
    #[test]
    fn a_dts_root_that_is_not_a_path_is_an_error_naming_the_crate() {
        let e = rooted("42", "[\"types/a.d.tl\"]", "/w/d/Cargo.toml").unwrap_err();
        assert!(e.contains("my-mod 0.1.0"), "{e}");
        assert!(e.contains("dts_root is not a path"), "{e}");
    }

    /// The two keys compose: the `*` names the files, the root says where their paths
    /// start, and what lands keeps the namespace between them.
    #[test]
    fn a_dts_root_and_a_star_compose() {
        let dir = std::env::temp_dir().join(format!("htl-dep-dts-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("types/mine")).unwrap();
        for f in ["thing.d.tl", "other.d.tl", "notes.md"] {
            std::fs::write(dir.join("types/mine").join(f), "").unwrap();
        }
        let d = rooted(
            "\"types\"",
            "[\"types/mine/*.d.tl\"]",
            &dir.join("Cargo.toml").display().to_string(),
        )
        .unwrap();
        let under: Vec<String> = d.iter().map(|d| slashed(&d.under().unwrap())).collect();
        assert_eq!(under, ["mine/other.d.tl", "mine/thing.d.tl"], "{d:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A materialised directory and its note, for asking `orphans` what it says about one.
    fn materialised(root: &Path, package: &str, files: &[&str]) {
        let dir = root.join("types").join(package);
        std::fs::create_dir_all(&dir).unwrap();
        for f in files {
            std::fs::write(dir.join(f), "return {}\n").unwrap();
        }
        let note = Note {
            package: package.to_string(),
            version: "0.1.0".into(),
            files: files.iter().map(|f| f.to_string()).collect(),
        };
        std::fs::write(dir.join(crate::DEP_TYPES_NOTE), note.text()).unwrap();
    }

    /// Two ways to be absent from `decls`, and they do not read alike. The crate this
    /// build carries was skipped and is still a dependency; the other one left the graph.
    /// Both are `left in place` and neither fails anything — what differs is what the line
    /// tells the reader to believe.
    #[cfg(feature = "std")]
    #[test]
    fn a_crate_this_build_carries_is_not_reported_as_a_departed_dependency() {
        let dir = std::env::temp_dir().join(format!("htl-dep-dts-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        materialised(&dir, crate::batteries::CRATE, &["json.d.tl"]);
        materialised(&dir, "gone", &["gone.d.tl"]);

        let out = orphans(&dir, &[]);
        assert_eq!(out.len(), 2, "{out:?}");
        let carried = out
            .iter()
            .find(|l| l.contains(crate::batteries::CRATE))
            .unwrap();
        assert!(
            carried.contains("is on the path as std.* instead"),
            "{carried}"
        );
        assert!(
            carried.contains("nothing preloads this copy's module name"),
            "{carried}"
        );
        assert!(!carried.contains("no longer"), "{carried}");

        let departed = out.iter().find(|l| l.contains("types/gone/")).unwrap();
        assert!(
            departed.contains("gone is no longer a dependency"),
            "{departed}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The skip and the report ask one question, so a crate that is still shipping its own
    /// declarations is unaffected by either: it is in `decls`, and `orphans` passes over
    /// the files it lists.
    #[test]
    fn a_live_crates_own_files_are_not_orphans() {
        let dir = std::env::temp_dir().join(format!("htl-dep-dts-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        materialised(&dir, "dep", &["dep.d.tl", "old.d.tl"]);
        let live = DepDecl {
            package: "dep".into(),
            version: "0.1.0".into(),
            declared: "dts/dep.d.tl".into(),
            dts_root: None,
            source: PathBuf::from("/w/d/dts/dep.d.tl"),
        };
        let out = orphans(&dir, std::slice::from_ref(&live));
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].contains("types/dep/old.d.tl"), "{out:?}");
        assert!(out[0].contains("dep no longer ships it"), "{out:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_note_round_trips_and_marks_the_directory() {
        let dir = std::env::temp_dir().join(format!("htl-dep-dts-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let note = Note {
            package: "dep".into(),
            version: "0.1.0".into(),
            files: vec!["dep.d.tl".into()],
        };
        std::fs::create_dir_all(dir.join("types/dep")).unwrap();
        crate::write_if_changed(
            &dir.join("types/dep").join(crate::DEP_TYPES_NOTE),
            &note.text(),
        )
        .unwrap();
        // A directory a person laid out by hand carries no note and is not one of ours.
        std::fs::create_dir_all(dir.join("types/socket")).unwrap();
        std::fs::write(dir.join("types/socket/http.d.tl"), "return {}\n").unwrap();
        assert_eq!(
            materialised_dirs(&dir.join("types")),
            vec![dir.join("types/dep")]
        );
        assert_eq!(Note::read(&dir.join("types/dep")), Some(note));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
