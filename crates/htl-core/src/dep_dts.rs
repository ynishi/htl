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
    pub version: String,
    /// The path inside the package, as that manifest wrote it.
    pub declared: String,
    /// Where cargo has that file on this machine.
    pub source: PathBuf,
    /// The name it takes under `types/<package>/`, which is what `require` says.
    pub file: String,
}

impl DepDecl {
    /// Where it is materialised, given the directory `types/` sits in.
    pub fn target(&self, root: &Path) -> PathBuf {
        root.join("types").join(&self.package).join(&self.file)
    }
}

/// The note beside a materialised set, read back to tell a directory this command wrote
/// from one a person laid out by hand (a hand-written `types/a/b.d.tl` is `require("a.b")`
/// and none of this command's business).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    #[serde(rename = "crate")]
    pub package: String,
    pub version: String,
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
        // under a name nothing answers `require` for.
        #[cfg(feature = "std")]
        if name == crate::batteries::CRATE {
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
                    file: Path::new(&rel)
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    declared: rel,
                });
            }
        }
    }
    out.sort_by(|a, b| (&a.package, &a.file).cmp(&(&b.package, &b.file)));
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
        let target = d.target(root);
        if d.file.is_empty() || !crate::is_declaration(Path::new(&d.file)) {
            problems.push(format!(
                "{} {} names {} in [package.metadata.htl] dts, which is not a `.d.tl` file",
                d.package, d.version, d.declared
            ));
            continue;
        }
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
                    .push(d.file.clone());
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
/// dependency, or that the crate no longer ships.
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
    let mut current: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for d in decls {
        current
            .entry(&d.package)
            .or_default()
            .insert(d.file.as_str());
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
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_file() && crate::is_declaration(p))
            .collect();
        files.sort();
        for path in files {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if live.is_some_and(|files| files.contains(name.as_ref())) {
                continue;
            }
            let why = match live {
                Some(_) => format!("{} no longer ships it", note.package),
                None => format!("{} is no longer a dependency", note.package),
            };
            out.push(format!(
                "{}: {why}; delete it when nothing requires the module",
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
        assert_eq!(d[0].file, "mq.d.tl");
        assert_eq!(d[0].source, PathBuf::from("/w/mq/dts/mq.d.tl"));
        assert_eq!(
            d[0].target(Path::new("/p")),
            PathBuf::from("/p/types/htl-mq/mq.d.tl")
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
        assert_eq!(d[0].file, "env.d.tl");
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
