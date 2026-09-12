//! What no entry reaches: the complement of the require closure `htl build` links.
//!
//! [`crate::project::check`] answers whether every file it was given is correct. It never
//! answers whether every file it was given is *reached*. A module nobody requires and a
//! dependency declared and never used both keep checking clean forever, and cost every
//! later reader the time to work out that they do not matter.
//!
//! The graph is not built here. `htl check` already resolves each file's `require`s and
//! keeps them — that is what the `require-cycle` lint is built from
//! ([`crate::require_cycles`]) and what a replayed entry carries — so a check hands back
//! the graph ([`crate::project::Report::requires`]) and this walks it from the entries the
//! project already declares. Nothing new is parsed, and a second run over an unchanged
//! project replays the check from the store rather than repeating it.
//!
//! # Entries
//!
//! The hard part elsewhere is guessing where to start; here the project has already said
//! it, in the files `htl test`, `htl build` and `[[contract]]` are pointed at:
//!
//! - `src/main.tl` (or `main.tl` at the root), the entry script,
//! - every test file, as `htl test` discovers them (`*_test.tl`, `tests/**/*.tl`),
//! - every module directly under a `[[contract]]` directory: those are loaded by name at
//!   run time, from a mods directory the project does not own,
//! - anything named in `[build] extra` / `[build] host`, which is where a dynamic
//!   `require(expr)` has to list its targets for `htl build` to bundle them,
//! - the file a Rust host embeds — the first argument of an `include_bundle!` /
//!   `include_tl!` / `include_tl_bytes!` in the crate around the project. A project whose
//!   `main` is in Rust has no `src/main.tl`, and its entry is named in Rust rather than in
//!   `htl.toml`; without reading it, the one file that is certainly reached is the one
//!   reported (measured on a 73-module game: it was the only finding, and it was wrong).
//!
//! A project with none of these is not reported on at all
//! ([`Summary::no_entry`]): everything would be listed, which says something about the
//! command rather than about the project.
//!
//! # Reachability is a property of the project, not of a path
//!
//! `paths` narrows what is *reported*, never what is walked: a module reached only from
//! `tests/` is reached, and asking about `src` alone must not turn that into a finding.
//! So the walk is the project root's whenever an `htl.toml` says where that is, and the
//! paths are applied to the answer.
//!
//! # Exports are not a kind here
//!
//! A third kind — a module-record field no reached module reads — was considered and left
//! out. htl's premise is a Rust host embedding Teal, so a module's caller is routinely
//! outside the Teal sources entirely: a `#[host_module]` calling into a preloaded module,
//! a `---@contract` type published for mod authors, an SDK a consumer requires. Every one
//! of those reads a field no walk of this project's `.tl` can see, and a rule that fires
//! on them is a rule nobody can act on.

use crate::cache;
use crate::project::{self, Config};
use anyhow::Result;
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// What a report is asked about: the paths, the config, and the store's switches.
pub struct Options<'a> {
    /// The paths as the command line spelled them. They narrow the report, not the walk
    /// (see the module doc); empty means the working directory, as everywhere else.
    pub paths: &'a [PathBuf],
    /// `htl.toml`, already loaded ([`crate::project::config_of`]).
    pub config: &'a Config,
    /// The run cache's switches ([`crate::project::cache_options`]): the graph comes from
    /// a check, and a check replays.
    pub cache: cache::Options,
}

/// Why a file is an entry — the four the project already declares (see the module doc).
#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// `src/main.tl`, or `main.tl` at the project root.
    Main,
    /// A test file, as `htl test` discovers them.
    Test,
    /// A module directly under a `[[contract]]` directory.
    Contract,
    /// Named in `[build] extra` or `[build] host`.
    Build,
    /// Embedded by a Rust host: the first argument of an `include_bundle!` /
    /// `include_tl!` / `include_tl_bytes!` in the crate around the project.
    Host,
}

impl EntryKind {
    /// The lowercase word this kind is written as, in `--format json` and in the text
    /// report's per-kind grouping. One spelling for both, so a reader of either is reading
    /// the same name.
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Main => "main",
            EntryKind::Test => "test",
            EntryKind::Contract => "contract",
            EntryKind::Build => "build",
            EntryKind::Host => "host",
        }
    }
}

/// A file the walk started from.
#[derive(Serialize, Debug, Clone)]
pub struct Entry {
    /// As the walk spelled it, which is what the report prints.
    pub path: String,
    /// The module name it answers to, when it has one (a test file under `tests/` has
    /// none: nothing requires it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Which of the four declarations made this an entry — the answer to "why is this
    /// reached", which is what a reader disputing a finding asks first.
    pub kind: EntryKind,
}

/// A `.tl` no entry's closure reaches.
#[derive(Serialize, Debug, Clone)]
pub struct Module {
    /// As the walk spelled it, so it matches the paths the rest of the report prints.
    pub path: String,
    /// The name a `require` would have to spell to reach it, when the search path gives
    /// it one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// A name in `mlua-pkg.toml` `[deps]` that no reached module requires.
#[derive(Serialize, Debug, Clone)]
pub struct Dependency {
    /// The `[deps]` key, which is the name a `require` would spell — not the package's
    /// own name or its git URL. It is the line to delete.
    pub name: String,
}

/// The counts an exit code and a summary line are made of.
#[derive(Serialize, Debug, Clone, Default)]
pub struct Summary {
    /// Modules considered: the project's own `.tl` under the reported paths,
    /// declarations aside.
    pub considered: usize,
    /// How many of them an entry reaches.
    pub reached: usize,
    /// How many files the walk started from. Printed beside the other two counts because
    /// a surprising number of findings is usually a surprising number of entries.
    pub entries: usize,
    /// Findings of the first kind: the length of [`Report::modules`].
    pub modules: usize,
    /// Findings of the second: the length of [`Report::dependencies`].
    pub dependencies: usize,
    /// Nothing to start from, so nothing was reported (see the module doc).
    pub no_entry: bool,
    /// Errors the check behind the graph reported. A file that does not check contributes
    /// no edges, so a report from a run with these is a guess and says so.
    pub check_errors: usize,
    /// Nothing unused. What `--exit-non-zero-on-unused` reads.
    pub ok: bool,
}

/// What no entry reaches.
#[derive(Serialize, Debug, Clone, Default)]
pub struct Report {
    /// The modules under the reported paths that no entry's closure reaches.
    pub modules: Vec<Module>,
    /// The `[deps]` names no reached module requires.
    pub dependencies: Vec<Dependency>,
    /// What the walk started from. Carried even though nothing unused is listed here: a
    /// finding is only as good as the entries behind it, and this is how a reader checks
    /// them.
    pub entries: Vec<Entry>,
    /// The counts, and the two flags a caller decides an exit code from.
    pub summary: Summary,
}

/// Walk the require graph from the project's declared entries and report the complement:
/// the modules nothing reaches, and the declared dependencies nothing reached requires.
pub fn unused(opts: &Options<'_>) -> Result<Report> {
    let given: Vec<PathBuf> = if opts.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.paths.to_vec()
    };
    let root = match opts.config {
        Some((r, _, _)) => r.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    // Reachability is the project's, not the path's: walk the root when one is known, and
    // let `given` decide only what is reported.
    let walk: Vec<PathBuf> = match opts.config {
        Some(_) => vec![root.clone()],
        None => given.clone(),
    };
    // A patched dependency is the project's code to check but not the project's to judge
    // unused: what reaches it lives upstream.
    let skip = project::patched(&walk);
    let files = crate::collect_tl_skipping(&walk, &skip)?;

    // The graph, from the check that already resolves every `require` — replayed from the
    // store when nothing moved. The diagnostics are the check's business, not this
    // command's, so they are collected and dropped; the error count is kept, since a file
    // that did not check has no edges and the report would be missing them.
    let mut sink = project::Sink::new(project::Collect::default());
    let checked = project::check(
        &mut sink,
        &files,
        &project::Options {
            paths: &walk,
            config: opts.config,
            lint: None,
            cache: opts.cache,
        },
    )?;

    let mut edges: HashMap<PathBuf, Vec<cache::RequireJson>> = HashMap::new();
    for (f, reqs) in &checked.requires {
        edges
            .entry(canon(f))
            .or_default()
            .extend(reqs.iter().cloned());
    }

    // Every walked file by its canonical path, with the spelling to report it under and
    // the name a `require` reaches it by.
    let dirs: Vec<PathBuf> = match opts.config {
        Some((r, _, c)) => c.search_paths(r).iter().map(|d| canon(d)).collect(),
        None => vec![canon(&root)],
    };
    let mut shown: HashMap<PathBuf, String> = HashMap::new();
    let mut name_of: HashMap<PathBuf, String> = HashMap::new();
    let mut by_name: HashMap<String, PathBuf> = HashMap::new();
    for f in &files {
        let c = canon(f);
        shown.insert(c.clone(), display(f));
        if crate::is_declaration(f) {
            continue;
        }
        for d in &dirs {
            if !c.starts_with(d) {
                continue;
            }
            let Ok(n) = crate::module_name(d, &c) else {
                continue;
            };
            // The shortest name wins the display: `src/` is on the search path as well as
            // the root, and `foo` is what a `require` says, not `src.foo`.
            match name_of.get(&c) {
                Some(prev) if prev.len() <= n.len() => {}
                _ => {
                    name_of.insert(c.clone(), n.clone());
                }
            }
            by_name.entry(n).or_insert_with(|| c.clone());
        }
    }

    let entries = entries(opts.config, &root, &walk, &skip, &shown, &by_name)?;
    let mut reached: HashSet<PathBuf> = HashSet::new();
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    for (file, _) in &entries {
        if reached.insert(file.clone()) {
            queue.push_back(file.clone());
        }
    }
    while let Some(f) = queue.pop_front() {
        let Some(reqs) = edges.get(&f) else { continue };
        for r in reqs {
            let Some(p) = &r.path else { continue };
            let p = canon(Path::new(p));
            if reached.insert(p.clone()) {
                queue.push_back(p);
            }
        }
    }

    // What the paths asked about, of the project's own modules.
    let asked: Vec<PathBuf> = given.iter().map(|p| canon(p)).collect();
    let candidates: Vec<PathBuf> = files
        .iter()
        .filter(|f| !crate::is_declaration(f))
        .map(|f| canon(f))
        .filter(|c| asked.iter().any(|a| c == a || c.starts_with(a)))
        .collect();

    let no_entry = entries.is_empty();
    let mut modules: Vec<Module> = Vec::new();
    if !no_entry {
        for c in &candidates {
            if reached.contains(c) {
                continue;
            }
            modules.push(Module {
                path: shown.get(c).cloned().unwrap_or_else(|| display(c)),
                module: name_of.get(c).cloned(),
            });
        }
        modules.sort_by(|a, b| a.path.cmp(&b.path));
    }

    let dependencies = if no_entry {
        Vec::new()
    } else {
        unused_deps(&root, &edges, &reached)
    };

    let summary = Summary {
        considered: candidates.len(),
        reached: candidates.iter().filter(|c| reached.contains(*c)).count(),
        entries: entries.len(),
        modules: modules.len(),
        dependencies: dependencies.len(),
        no_entry,
        check_errors: checked.errors,
        ok: modules.is_empty() && dependencies.is_empty(),
    };
    Ok(Report {
        modules,
        dependencies,
        entries: entries
            .into_iter()
            .map(|(c, kind)| Entry {
                path: shown.get(&c).cloned().unwrap_or_else(|| display(&c)),
                module: name_of.get(&c).cloned(),
                kind,
            })
            .collect(),
        summary,
    })
}

/// The files the walk starts from, canonical, each with why it is one. Only files the
/// walk itself covered: an entry outside it contributes no edges, so counting it would
/// claim a closure that was never followed.
fn entries(
    cfg: &Config,
    root: &Path,
    walk: &[PathBuf],
    skip: &[PathBuf],
    shown: &HashMap<PathBuf, String>,
    by_name: &HashMap<String, PathBuf>,
) -> Result<Vec<(PathBuf, EntryKind)>> {
    let mut out: Vec<(PathBuf, EntryKind)> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut add = |p: PathBuf, kind: EntryKind| {
        if shown.contains_key(&p) && seen.insert(p.clone()) {
            out.push((p, kind));
        }
    };

    for p in [root.join("src").join("main.tl"), root.join("main.tl")] {
        if p.is_file() {
            add(canon(&p), EntryKind::Main);
        }
    }
    for t in crate::testing::discover_tests_skipping(walk, skip)? {
        add(canon(&t), EntryKind::Test);
    }
    for p in host_entries(root) {
        add(canon(&p), EntryKind::Host);
    }
    if let Some((r, _, c)) = cfg {
        // A module under a contract dir is loaded by name at run time, from a directory
        // the project does not own. Every module there is an entry, the `exclude`d ones
        // too: `exclude` says a module is not held to the contract, not that nothing
        // loads it.
        let (contracts, _) = crate::contract::resolve(r, c);
        for con in &contracts {
            for dir in con.dirs(root) {
                let Ok(rd) = std::fs::read_dir(&dir) else {
                    continue;
                };
                let mut found: Vec<PathBuf> = rd
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file() && crate::is_tl_source(p) && !crate::is_declaration(p))
                    .collect();
                found.sort();
                for p in found {
                    add(canon(&p), EntryKind::Contract);
                }
            }
        }
        // A dynamic `require(expr)` has already had to list its targets for `htl build`;
        // this reads the same two lists.
        for name in c.build.extra.iter().chain(&c.build.host) {
            if let Some(p) = by_name.get(name) {
                add(p.clone(), EntryKind::Build);
            }
        }
    }
    Ok(out)
}

/// The `.tl` files a Rust host embeds, from the crate around the project.
///
/// `include_bundle!("src/boot.tl", ..)`, `include_tl!("src/main.tl")` and
/// `include_tl_bytes!` are how a host says where its Teal starts, and their paths are
/// relative to `CARGO_MANIFEST_DIR` — the crate root, which in every layout htl scaffolds
/// is the directory `htl.toml` sits in. A project whose `main` is in Rust declares its
/// entry there and nowhere else.
///
/// The first string literal of the call, read out of the source rather than parsed: a
/// name in a comment costs at most an entry for a file that exists, which makes the
/// report quieter, and a call whose first argument is not a literal is not one the macro
/// accepts either.
fn host_entries(root: &Path) -> Vec<PathBuf> {
    const CALLS: [&str; 3] = ["include_bundle!", "include_tl!", "include_tl_bytes!"];
    let Some(crate_root) = crate::dts::find_cargo_package_root(root) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = Vec::new();
    let walker = walkdir::WalkDir::new(&crate_root)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.path() == crate_root || !crate::is_skipped_dir(e.path(), &[]));
    for e in walker.flatten() {
        if e.path().extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(e.path()) else {
            continue;
        };
        for call in CALLS {
            for (at, _) in src.match_indices(call) {
                let Some(arg) = first_literal(&src[at + call.len()..]) else {
                    continue;
                };
                let p = crate_root.join(arg);
                if p.is_file() && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// The string literal a macro call opens with: `("src/main.tl", ..` -> `src/main.tl`.
/// `None` when the call does not open with one (a `concat!`, a constant).
fn first_literal(rest: &str) -> Option<&str> {
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('(')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// The `[deps]` names no reached module requires.
///
/// A name counts as required when a reached module says it — `require("mathx")` or
/// `require("mathx.vec")` — or when what a reached module required resolved to a file
/// inside that dependency: the installed copy under `.htl/modules/vendored/<name>`, a
/// `target_dir` copy in the tree, or a `patch_dir` the project took over. Both, because a
/// dependency that is declared but not installed resolves to nothing and is still
/// required by name.
fn unused_deps(
    root: &Path,
    edges: &HashMap<PathBuf, Vec<cache::RequireJson>>,
    reached: &HashSet<PathBuf>,
) -> Vec<Dependency> {
    let Some(project) = crate::pkg::Project::find(root) else {
        return Vec::new();
    };
    let Ok(manifest) = crate::pkg::mlua_pkg::manifest::Manifest::from_path(&project.manifest)
    else {
        return Vec::new();
    };
    let mut names: HashSet<String> = HashSet::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for f in reached {
        let Some(reqs) = edges.get(f) else { continue };
        for r in reqs {
            names.insert(r.module.clone());
            if let Some(p) = &r.path {
                paths.push(canon(Path::new(p)));
            }
        }
    }
    let mut out: Vec<Dependency> = Vec::new();
    for name in manifest.deps.keys() {
        let prefix = format!("{name}.");
        if names
            .iter()
            .any(|m| m == name || m.starts_with(prefix.as_str()))
        {
            continue;
        }
        let mut dirs = vec![canon(&project.vendored.join(name))];
        dirs.extend(
            project
                .patches
                .iter()
                .filter(|p| &p.name == name)
                .map(|p| canon(&p.dir)),
        );
        dirs.extend(
            project
                .vendored_copies
                .iter()
                .filter(|d| d.file_name().and_then(|s| s.to_str()) == Some(name.as_str()))
                .map(|d| canon(d)),
        );
        if paths.iter().any(|p| dirs.iter().any(|d| p.starts_with(d))) {
            continue;
        }
        out.push(Dependency { name: name.clone() });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// A path as the report prints it: against the directory the command ran in, which is how
/// every other path in this tool's output reads ([`project::display_path`]).
fn display(p: &Path) -> String {
    project::display_path(p)
}
