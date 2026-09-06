//! An on-disk cache of what checking one module reported (issues #3, #19).
//!
//! Building a checker costs about 13.5 ms and type-checking a few thousand lines of Teal
//! costs about a second (`crates/htl-core/benches/check.rs`). A run that changed nothing
//! should pay neither, and a run that changed one module should pay for that module and
//! what depends on it rather than for the project.
//!
//! # One entry per module
//!
//! #3 shipped this as one entry per invocation, because at the time it looked as though
//! skipping individual modules would change what the rest of them saw. Two things settled
//! that: #21 made a file's result independent of its position in the walk, and measurement
//! showed the shared module store to be an optimisation rather than a precondition — a
//! dependency being checked first does not change what its requirer reports. So an entry
//! is now per module, and a run is the sum of them.
//!
//! # What makes an entry
//!
//! The **key** is the module as this invocation names it — the path spelling the checker
//! will put into the diagnostics, plus the lint selection and the working directory.
//! `--strict` and `--format` are deliberately absent: they change how a run is summarized
//! and what it exits with, not what any module reports, so runs that differ only in those
//! share their modules' entries.
//!
//! The **inputs** are the module and everything reading it required, by content hash. The
//! **probes** are the directories a `require` could resolve in, by the set of module names
//! in each — a new `.tl` appearing earlier on the search path changes what a name resolves
//! to while every recorded hash still matches, and nothing else would catch it. This is the
//! hole ccache documents in its direct mode.
//!
//! # No mtimes anywhere
//!
//! Content hashes only. Timestamp-based invalidation is where this class of tool
//! historically breaks — second-granular filesystems on macOS, mtimes zeroed by Docker
//! layers, clock skew, fresh CI checkouts invalidating everything — and hashing a module
//! costs microseconds against the milliseconds it saves.
//!
//! # Failure is a miss
//!
//! Every error path here returns "no entry" rather than propagating. A corrupt file, an
//! unreadable directory, a store on a read-only filesystem: the module gets checked, as it
//! would have been anyway. The one invariant worth stating is mypy's: an entry is written
//! whole or not at all, via a temporary file and a rename, so a reader never sees half of
//! one. Set `HTL_CACHE_DEBUG=1` to print why a lookup missed.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Bumped by hand when anything below changes shape. An entry stamped with a different
/// value is a miss rather than an error: a fresh checkout and an upgrade both take that
/// path in normal operation, which is why rustc treats its own header mismatch the same
/// way.
const FORMAT: u32 = 5;

/// Where the store lives under the project root. Generated, and `htl init` puts it in
/// `.gitignore`.
const DIR: &str = ".htl/cache";

/// How many entries a project's store may hold before a run starts dropping the ones it did
/// not use.
///
/// Scaled to the project, because the natural size is one entry per module per way of
/// invoking the check, and "a few ways" is what people do. The floor keeps a small project
/// from evicting itself on its second invocation.
fn default_bound(files: usize) -> usize {
    (files * 4).max(256)
}


/// What produced an entry. A cache is only as good as its ability to notice it was
/// written by something else.
///
/// The binary's length and modification time stand in for "which checker is this": during
/// development the vendored Teal compiler, the prelude and the Rust side all change many
/// times within one released version number, and a stamp that only carried the version
/// would happily replay results from a checker that no longer exists. Rebuilding always
/// moves both. This is the same reasoning behind sccache hashing the compiler binary into
/// its key.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct Stamp {
    format: u32,
    htl: String,
    exe_len: u64,
    exe_mtime_ns: u128,
}

impl Stamp {
    fn current() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;
        let m = std::fs::metadata(&exe).ok()?;
        let mtime = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
        Some(Self {
            format: FORMAT,
            htl: env!("CARGO_PKG_VERSION").to_string(),
            exe_len: m.len(),
            exe_mtime_ns: mtime,
        })
    }
}

/// One file the check read, by content.
#[derive(Serialize, Deserialize, Debug)]
struct Input {
    path: String,
    hash: String,
}

/// One directory a `require` could have resolved in, and what it offered *for the names
/// this module asked for*.
///
/// This catches a file appearing where the checker would now find it, which no hash of the
/// files it did read can see — the hole ccache documents in its direct mode. It used to
/// hash every module name in the directory, which meant adding any file at all invalidated
/// every module whose search path included it: writing one new module re-checked the whole
/// project (#24). A file appearing under a name nobody requires cannot change what anybody
/// resolved, so only the requested names go in.
#[derive(Serialize, Deserialize, Debug)]
struct Probe {
    dir: String,
    /// Hash of `(name, whether it resolves here)` over the module's own requires, in the
    /// order they are stored. Changing which directory a name resolves in changes this for
    /// both directories involved, which is how a shadowing file is caught.
    names: String,
}

/// One diagnostic exactly as it was handed to the sink, so a replay goes through the same
/// printing code the original run did rather than through a reconstruction of it.
/// Reconstructing text from parsed fields is how a cache starts printing subtly different
/// output from the run it claims to reproduce.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Recorded {
    pub severity: String,
    pub text: String,
    pub fix: Option<crate::report::FixJson>,
}

/// One literal `require` and where the checker resolved it. Kept because the project-level
/// cycle lint runs over every file's requires, replayed ones included — a cycle that closes
/// through a module nobody edited is still a cycle.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequireJson {
    pub module: String,
    pub path: Option<String>,
    pub line: usize,
    pub col: usize,
}

/// What checking one module reported.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Module {
    /// Its diagnostics, its contract lints included, in the order they were printed.
    pub diagnostics: Vec<Recorded>,
    pub errors: usize,
    pub warnings: usize,
    pub lints: usize,
    /// What the checker resolved this module's requires to. The next run keys on these,
    /// which is how a dependency's edit invalidates its dependents.
    pub deps: Vec<String>,
    pub requires: Vec<RequireJson>,
    /// The Lua this module generates, for entries under [`gen_key`]. `htl check` never needs
    /// it and stores `None`; `htl test` stores it so a replay can go straight to running.
    /// Absent when checking produced errors, since there is nothing to run then.
    #[serde(default)]
    pub code: Option<String>,
    /// What checking reported, in the form the test runner needs it back.
    ///
    /// `htl check` replays `diagnostics` straight into the sink and never reconstructs a
    /// `CheckInfo`; the test runner puts one into its report, so a replayed test file needs
    /// the structured form rather than the printed one. Only `gen_key` entries carry it.
    #[serde(default)]
    pub check: Option<CheckInfoJson>,
}

/// A `CheckInfo` as an entry stores it.
///
/// Deliberately a separate type from the diagnostics `htl check` replays: those are text
/// on their way to a terminal, these are the fields the runner reads back.
#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct CheckInfoJson {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub lints: Vec<String>,
    pub deps: Vec<String>,
    pub requires: Vec<RequireJson>,
    pub error_fixes: Vec<Option<crate::report::FixJson>>,
    pub lint_fixes: Vec<Option<crate::report::FixJson>>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Entry {
    stamp: Stamp,
    /// The file this entry is about, and which question it answers.
    ///
    /// Neither is needed to use the entry — the key already decided both. They are here so
    /// the store can be described: the files on disk are named by hash, and a cache nobody
    /// can read the contents of is one nobody can reason about (`htl cache status`).
    subject: String,
    kind: String,
    inputs: Vec<Input>,
    probes: Vec<Probe>,
    module: Module,
}

/// One entry holding the whole walk, for `Mode::WholeRun`.
#[derive(Serialize, Deserialize, Debug)]
struct RunEntry {
    stamp: Stamp,
    /// The files the walk visited, and which question this answers. See `Entry`.
    subjects: Vec<String>,
    kind: String,
    inputs: Vec<Input>,
    probes: Vec<Probe>,
    /// One per file in the walk, in walk order. A different count means the walk itself
    /// changed, which is a miss before any hash is compared.
    modules: Vec<Module>,
}

/// How much of a run one entry covers.
///
/// The two are a real trade rather than one being a refinement of the other, and which
/// wins depends on where in the dependency graph the edit lands — see the Caching section
/// of the README. `PerModule` is the default because editing is the common case.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// One entry per module. An edit costs the module, its dependents, and what those pull
    /// in; everything else replays.
    #[default]
    PerModule,
    /// One entry for the walk. Any edit anywhere re-checks everything, but a run where
    /// nothing moved reads one file instead of one per module.
    WholeRun,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "per-module" => Some(Mode::PerModule),
            "whole-run" => Some(Mode::WholeRun),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::PerModule => "per-module",
            Mode::WholeRun => "whole-run",
        }
    }
}

/// Everything the store needs from outside itself.
///
/// Built once, by the caller that is already reading flags and the environment, and passed
/// in. Nothing in this module reaches for `std::env`: a run's behaviour is decided in one
/// place, so reading that place tells you what the run will do.
#[derive(Copy, Clone, Debug)]
pub struct Options {
    /// `--no-cache` turns this off. Separate from how the cache is grained.
    pub enabled: bool,
    pub mode: Mode,
    /// Say why lookups missed, and what the run did with the store.
    pub explain: bool,
    /// Entries a project's store may hold. `None` scales it to the project size.
    pub max_entries: Option<usize>,
}

impl Default for Options {
    fn default() -> Self {
        Self { enabled: true, mode: Mode::default(), explain: false, max_entries: None }
    }
}

/// What one run did with the store.
///
/// Kept by the store rather than by its callers, so a report cannot claim a number the store
/// disagrees with.
#[derive(Default, Copy, Clone, Debug)]
pub struct Stats {
    pub hits: usize,
    pub misses: usize,
    pub stored: usize,
    pub evicted: usize,
}

impl Stats {
    /// One line, for the end of a run. `None` when the run did not touch the store at all,
    /// which is worth saying by saying nothing.
    pub fn summary(&self, mode: Mode) -> Option<String> {
        let touched = self.hits + self.misses + self.stored + self.evicted;
        (touched > 0).then(|| {
            format!(
                "htl cache: {} hit, {} missed, {} written, {} evicted ({})",
                self.hits,
                self.misses,
                self.stored,
                self.evicted,
                mode.as_str()
            )
        })
    }
}

/// The identity of one module under one invocation: same key, same question.
///
/// Carries which question it is, so an entry written under it can record that too — the file
/// on disk is named by the hash, and a store whose contents cannot be described is one
/// nobody can reason about.
#[derive(Debug, Clone)]
pub struct Key {
    hash: String,
    kind: &'static str,
}

/// Hash of a file's contents, or `None` if it cannot be read — an unreadable input is a
/// miss, never a silent hit.
fn hash_file(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    Some(blake3::hash(&bytes).to_hex().to_string())
}

/// A path in the form it is stored and compared in: absolute and symlink-resolved where
/// possible.
///
/// Normalizing at one producer and not another is a real bug rather than a tidiness
/// concern: mypy shipped a version where a file checked directly recorded a relative path
/// while the same file reached as an import recorded an absolute one, so its hash depended
/// on how it was reached. Everything that goes into an entry comes through here.
pub fn normal(p: &Path) -> String {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned()
}

/// Whether `name` resolves to a file in `dir`.
///
/// The shapes are the ones `H.add_path` puts on the search path (`dir/?.tl`,
/// `dir/?/init.tl`) plus the declaration and Lua forms the checker falls back to. A dot in
/// a module name is a directory separator, as it is for Lua's own searcher.
///
/// This is an existence question, not a resolution: it says the name *could* be found here,
/// and comparing the answer across every directory on the path is what makes a change of
/// resolution visible. Getting it wrong in the direction of "present" costs a re-check;
/// there is no direction in which it produces a wrong answer.
fn resolves_in(dir: &Path, name: &str) -> bool {
    let stem = name.replace('.', "/");
    ["tl", "d.tl", "lua"].iter().any(|ext| dir.join(format!("{stem}.{ext}")).is_file())
        || ["init.tl", "init.d.tl", "init.lua"].iter().any(|f| dir.join(&stem).join(f).is_file())
}

/// The key for one module in this invocation.
///
/// `spelling` is the path as the CLI will hand it to the checker, not a resolved one: the
/// checker echoes it into every diagnostic it prints, so `htl check .` and
/// `htl check src/a.tl` produce different text for the same module and cannot share an
/// entry. The working directory completes it, since the same relative spelling means a
/// different file from somewhere else.
pub fn module_key(spelling: &Path, lint: Option<&str>) -> Key {
    key_with(CHECK, spelling, lint)
}

/// An entry holding what checking a module reported.
pub const CHECK: &str = "check";
/// An entry holding that, plus the Lua the module generates.
pub const GEN: &str = "gen";

/// The key for one test file's checked-and-generated form.
///
/// The same material as [`module_key`] under a different prefix, so the two never collide.
/// They are separate entries on purpose: a `htl check` entry holds diagnostics and nothing
/// else, and making it carry a few KB of generated Lua would cost every check run a parse it
/// has no use for.
pub fn gen_key(spelling: &Path, lint: Option<&str>) -> Key {
    key_with(GEN, spelling, lint)
}

fn key_with(kind: &'static str, spelling: &Path, lint: Option<&str>) -> Key {
    let mut h = blake3::Hasher::new();
    h.update(kind.as_bytes());
    h.update(b"\0");
    h.update(spelling.to_string_lossy().as_bytes());
    h.update(b"\0");
    h.update(lint.unwrap_or("").as_bytes());
    h.update(b"\0");
    h.update(cwd().as_bytes());
    Key { hash: h.finalize().to_hex().to_string(), kind }
}

/// The key for the walk as a whole, under `Mode::WholeRun`.
///
/// Every file's spelling goes in, in order: a walk that visits a different set, or the same
/// set named differently, is a different run and prints different text.
pub fn run_key(files: &[PathBuf], lint: Option<&str>) -> Key {
    let mut h = blake3::Hasher::new();
    h.update(b"htl-run\0");
    for f in files {
        h.update(f.to_string_lossy().as_bytes());
        h.update(b"\0");
    }
    h.update(lint.unwrap_or("").as_bytes());
    h.update(b"\0");
    h.update(cwd().as_bytes());
    Key { hash: h.finalize().to_hex().to_string(), kind: RUN }
}

/// One entry covering a whole walk (`Mode::WholeRun`).
pub const RUN: &str = "run";

/// The working directory, resolved once. This is called for every module in the walk, and
/// resolving it means a `canonicalize` syscall — doing that per module is a measurable part
/// of a run that replays everything and does nothing else.
fn cwd() -> &'static str {
    static CWD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CWD.get_or_init(|| std::env::current_dir().map(|p| normal(&p)).unwrap_or_default())
}

/// One entry, as `htl cache status` describes it.
#[derive(Serialize, Debug)]
pub struct EntrySummary {
    /// `check`, `gen`, `run` — or `unreadable` for a file this build cannot parse, which is
    /// worth showing rather than hiding, since it is also a permanent miss.
    pub kind: String,
    /// The files it is about. One, except for a whole-run entry.
    pub subjects: Vec<String>,
    pub bytes: u64,
    /// Seconds since the entry was last written or replayed. The sweep drops the oldest.
    pub age_secs: u64,
}

/// What a project's store holds.
#[derive(Serialize, Debug)]
pub struct Contents {
    pub dir: String,
    pub entries: Vec<EntrySummary>,
    pub bytes: u64,
}

/// Read the store and say what is in it.
///
/// Every entry is parsed, which is why this is a command rather than something a run does on
/// the side. Nothing here validates: an entry listed may still be a miss on the next run
/// because its inputs moved. This answers "what is stored", not "what would be reused".
pub fn describe(root: &Path) -> Contents {
    let dir = root.join(DIR);
    let mut out = Contents { dir: dir.to_string_lossy().into_owned(), entries: Vec::new(), bytes: 0 };
    let Ok(rd) = std::fs::read_dir(&dir) else { return out };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        out.bytes += meta.len();
        let age_secs = meta
            .modified()
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let v: serde_json::Value = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or(serde_json::Value::Null);
        let kind = v.get("kind").and_then(|k| k.as_str()).unwrap_or("unreadable").to_string();
        let subjects = match (v.get("subject"), v.get("subjects")) {
            (Some(s), _) if s.is_string() => vec![s.as_str().unwrap_or_default().to_string()],
            (_, Some(a)) if a.is_array() => a
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        out.entries.push(EntrySummary { kind, subjects, bytes: meta.len(), age_secs });
    }
    out.entries.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.subjects.cmp(&b.subjects)));
    out
}

/// A store rooted at a project.
pub struct Cache {
    dir: PathBuf,
    opts: Options,
    stats: RefCell<Stats>,
    /// Content hashes taken during this run, by normalized path.
    ///
    /// Entries overlap heavily: every module in a project tends to require the same few
    /// leaves, so a leaf appears in the inputs of most entries. Without this, checking 48
    /// modules against the store reads and hashes the shared ones 48 times — which is most
    /// of what a fully replayed run spends. A file is assumed not to change while one
    /// `htl check` is running; if it does, the run's answer was undefined anyway.
    hashes: RefCell<HashMap<String, Option<String>>>,
    /// Name-resolves-here answers taken during this run, by (directory, name). Same
    /// reasoning.
    dirs: RefCell<HashMap<(String, String), bool>>,
}

impl Cache {
    /// `None` when the caller turned caching off, or when this build cannot identify
    /// itself well enough to be sure an entry is its own.
    pub fn open(root: &Path, opts: Options) -> Option<Self> {
        if !opts.enabled {
            return None;
        }
        Stamp::current()?;
        Some(Self {
            dir: root.join(DIR),
            opts,
            stats: RefCell::new(Stats::default()),
            hashes: RefCell::new(HashMap::new()),
            dirs: RefCell::new(HashMap::new()),
        })
    }

    /// What this run has done with the store so far.
    pub fn stats(&self) -> Stats {
        *self.stats.borrow()
    }

    /// Say why something was not reused. Only when asked; the caller decided that once.
    fn miss(&self, reason: &str) {
        self.stats.borrow_mut().misses += 1;
        if self.opts.explain {
            eprintln!("htl cache: miss ({reason})");
        }
    }

    /// The hash of a file, taken once per run.
    fn hash_of(&self, path: &str) -> Option<String> {
        if let Some(h) = self.hashes.borrow().get(path) {
            return h.clone();
        }
        let h = hash_file(Path::new(path));
        self.hashes.borrow_mut().insert(path.to_string(), h.clone());
        h
    }

    /// Whether one name resolves in one directory, answered once per run. The project root,
    /// `src/` and `types/` are probed by every module, and modules share the names they
    /// require, so this overlaps even more than the file hashes do.
    fn resolves(&self, dir: &str, name: &str) -> bool {
        let k = (dir.to_string(), name.to_string());
        if let Some(v) = self.dirs.borrow().get(&k) {
            return *v;
        }
        let v = resolves_in(Path::new(dir), name);
        self.dirs.borrow_mut().insert(k, v);
        v
    }

    /// What `dir` offers for `names`, hashed.
    fn probe_hash(&self, dir: &str, names: &[String]) -> String {
        let mut h = blake3::Hasher::new();
        for n in names {
            h.update(n.as_bytes());
            h.update(if self.resolves(dir, n) { b"\x01" } else { b"\x00" });
        }
        h.finalize().to_hex().to_string()
    }

    fn entry_path(&self, key: &Key) -> PathBuf {
        self.dir.join(format!("{}.json", key.hash))
    }

    pub fn mode(&self) -> Mode {
        self.opts.mode
    }

    /// A truncated or hand-edited entry is a miss, not a crash.
    fn parse<T: serde::de::DeserializeOwned>(&self, raw: &str) -> Option<T> {
        match serde_json::from_str(raw) {
            Ok(e) => Some(e),
            Err(e) => {
                self.miss(&format!("unreadable entry: {e}"));
                None
            }
        }
    }

    /// The module names an entry's modules asked for, sorted and deduplicated.
    ///
    /// Both sides of a probe comparison have to derive this the same way, which is why it is
    /// one function rather than two loops.
    fn required_names(modules: &[Module]) -> Vec<String> {
        let mut names: Vec<String> =
            modules.iter().flat_map(|m| m.requires.iter().map(|r| r.module.clone())).collect();
        names.sort();
        names.dedup();
        names
    }

    /// Whether an entry still describes the world: written by this build, every file it read
    /// unchanged, and every directory it could have resolved in still answering the same way
    /// for the names it asked about.
    fn still_valid(&self, stamp: &Stamp, inputs: &[Input], probes: &[Probe], names: &[String]) -> bool {
        let Some(current) = Stamp::current() else { return false };
        if *stamp != current {
            self.miss("written by a different build");
            return false;
        }
        for i in inputs {
            match self.hash_of(&i.path) {
                Some(h) if h == i.hash => {}
                Some(_) => {
                    self.miss(&format!("changed: {}", i.path));
                    return false;
                }
                None => {
                    self.miss(&format!("gone: {}", i.path));
                    return false;
                }
            }
        }
        for p in probes {
            if self.probe_hash(&p.dir, names) != p.names {
                self.miss(&format!("what {} offers for this module's requires changed", p.dir));
                return false;
            }
        }
        self.stats.borrow_mut().hits += 1;
        true
    }

    /// What each file in the walk reported last time; `None` where it has to be checked.
    ///
    /// Under `WholeRun` this is all-or-nothing by construction: one entry covers the walk,
    /// so a single changed input means every module is checked.
    pub fn lookup_all(&self, keys: &[Key], run: &Key, files: usize) -> Vec<Option<Module>> {
        match self.opts.mode {
            Mode::PerModule => keys.iter().map(|k| self.lookup(k)).collect(),
            Mode::WholeRun => match self.lookup_run(run, files) {
                Some(ms) => ms.into_iter().map(Some).collect(),
                None => vec![None; files],
            },
        }
    }

    /// What the module reported last time, if every input and probe still matches.
    pub fn lookup(&self, key: &Key) -> Option<Module> {
        let raw = std::fs::read_to_string(self.entry_path(key)).ok()?;
        let entry: Entry = self.parse(&raw)?;
        let names = Self::required_names(std::slice::from_ref(&entry.module));
        if !self.still_valid(&entry.stamp, &entry.inputs, &entry.probes, &names) {
            return None;
        }
        self.touch(key);
        Some(entry.module)
    }

    /// Mark an entry as used, so that the sweep drops what is stale rather than what is
    /// merely old.
    ///
    /// Without this the shape you run most often is the one whose entries were written
    /// first, so it ages out while a lint flag you tried once survives — measured on a
    /// 58-module project, running four other shapes left the plain one replaying 24 of its
    /// 58 modules. Failing to touch costs a later re-check and nothing else, so every error
    /// here is ignored.
    fn touch(&self, key: &Key) {
        if let Ok(f) = std::fs::File::options().write(true).open(self.entry_path(key)) {
            let _ = f.set_modified(std::time::SystemTime::now());
        }
    }

    fn lookup_run(&self, key: &Key, files: usize) -> Option<Vec<Module>> {
        let raw = std::fs::read_to_string(self.entry_path(key)).ok()?;
        let entry: RunEntry = self.parse(&raw)?;
        if entry.modules.len() != files {
            self.miss("the walk visits a different number of files");
            return None;
        }
        let names = Self::required_names(&entry.modules);
        if !self.still_valid(&entry.stamp, &entry.inputs, &entry.probes, &names) {
            return None;
        }
        self.touch(key);
        Some(entry.modules)
    }

    /// Hash every path an entry has to compare next time, once each: a dependency reached
    /// from two modules is compared once. `None` if anything is unreadable, which drops the
    /// entry rather than storing one that would hit wrongly.
    fn inputs_for(&self, paths: impl Iterator<Item = String>) -> Option<Vec<Input>> {
        let mut paths: Vec<String> = paths.collect();
        paths.sort();
        paths.dedup();
        let mut inputs = Vec::with_capacity(paths.len());
        for p in paths {
            inputs.push(Input { path: p.clone(), hash: self.hash_of(&p)? });
        }
        Some(inputs)
    }

    fn probes_for(&self, dirs: &[PathBuf], names: &[String]) -> Vec<Probe> {
        let mut dirs: Vec<String> = dirs.iter().map(|p| normal(p)).collect();
        dirs.sort();
        dirs.dedup();
        dirs.into_iter()
            .map(|d| {
                let h = self.probe_hash(&d, names);
                Probe { dir: d, names: h }
            })
            .collect()
    }

    /// Record what one module reported. Best-effort: a store that cannot be written leaves
    /// the next run to do the work again, which is slow rather than wrong.
    pub fn store_module(&self, key: &Key, file: &Path, extra_inputs: &[PathBuf], dirs: &[PathBuf], module: &Module) {
        let Some(stamp) = Stamp::current() else { return };
        let paths = std::iter::once(normal(file))
            .chain(module.deps.iter().cloned())
            .chain(extra_inputs.iter().map(|p| normal(p)));
        let Some(inputs) = self.inputs_for(paths) else { return };
        let names = Self::required_names(std::slice::from_ref(module));
        let entry = Entry {
            stamp,
            subject: normal(file),
            kind: key.kind.to_string(),
            inputs,
            probes: self.probes_for(dirs, &names),
            module: module.clone(),
        };
        match self.write(key, &entry) {
            Ok(()) => self.stats.borrow_mut().stored += 1,
            Err(e) if self.opts.explain => eprintln!("htl cache: not stored ({e})"),
            Err(_) => {}
        }
    }

    /// Record the whole walk as one entry. Its inputs are the union of every module's, so
    /// any edit anywhere misses — which is the behaviour this mode is chosen for.
    pub fn store_run(
        &self,
        key: &Key,
        files: &[PathBuf],
        extra_inputs: &[PathBuf],
        dirs: &[PathBuf],
        modules: &[Module],
    ) {
        let Some(stamp) = Stamp::current() else { return };
        let paths = files
            .iter()
            .map(|f| normal(f))
            .chain(modules.iter().flat_map(|m| m.deps.iter().cloned()))
            .chain(extra_inputs.iter().map(|p| normal(p)));
        let Some(inputs) = self.inputs_for(paths) else { return };
        let names = Self::required_names(modules);
        let entry = RunEntry {
            stamp,
            subjects: files.iter().map(|f| normal(f)).collect(),
            kind: key.kind.to_string(),
            inputs,
            probes: self.probes_for(dirs, &names),
            modules: modules.to_vec(),
        };
        match self.write(key, &entry) {
            Ok(()) => self.stats.borrow_mut().stored += 1,
            Err(e) if self.opts.explain => eprintln!("htl cache: not stored ({e})"),
            Err(_) => {}
        }
    }

    /// Drop entries this run did not use, once the store has outgrown what the project
    /// warrants.
    ///
    /// Nothing else removes an entry. The key covers the paths as written, the lint
    /// selection and the working directory, so every distinct way of invoking the check
    /// leaves a full set of module entries behind — a lint flag tried once doubles the store
    /// permanently, and a deleted module's entry is never read again. `htl check` sees the
    /// whole graph in one process, which is what lets this be exact about the orphans rather
    /// than sampling the way ccache has to.
    ///
    /// Two passes. Entries whose recorded inputs have all gone describe modules that no
    /// longer exist, and go first. If the store is still over, the oldest go until it fits.
    ///
    /// **This uses mtimes, and #3's rule against them still holds.** That rule is about
    /// invalidation, where trusting a timestamp means replaying a stale result and reporting
    /// something untrue. Dropping an entry that was still good costs the check it would have
    /// skipped and nothing else. The two questions deserve different tools.
    ///
    /// "Oldest" is least recently *used*, because a hit touches its entry ([`Self::touch`]).
    /// Without that it would mean least recently written, and the shape run most often —
    /// written first — would age out while a shape tried once survived.
    pub fn sweep(&self, keep: &[Key], files: usize) {
        let bound = self.opts.max_entries.unwrap_or_else(|| default_bound(files));
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let keep: std::collections::HashSet<&str> = keep.iter().map(|k| k.hash.as_str()).collect();

        let mut all = 0usize;
        let mut candidates: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "json") {
                continue;
            }
            all += 1;
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if keep.contains(stem.as_str()) {
                continue;
            }
            let mtime = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
            candidates.push((path, mtime));
        }
        if all <= bound {
            return;
        }

        let mut removed = 0usize;
        // Orphans first: an entry none of whose inputs still exist cannot be read again.
        candidates.retain(|(p, _)| {
            if all - removed <= bound || !self.is_orphan(p) {
                return true;
            }
            if std::fs::remove_file(p).is_ok() {
                removed += 1;
            }
            false
        });
        // Then by age, oldest first, until the store fits.
        if all - removed > bound {
            candidates.sort_by_key(|(_, t)| *t);
            for (p, _) in &candidates {
                if all - removed <= bound {
                    break;
                }
                if std::fs::remove_file(p).is_ok() {
                    removed += 1;
                }
            }
        }
        self.stats.borrow_mut().evicted += removed;
        if removed > 0 && self.opts.explain {
            eprintln!("htl cache: dropped {removed} of {all} entries (bound {bound})");
        }
    }

    /// Whether every file an entry recorded as an input has gone. Unreadable entries count as
    /// orphans: nothing can use them either.
    fn is_orphan(&self, path: &Path) -> bool {
        let Ok(raw) = std::fs::read_to_string(path) else { return true };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { return true };
        let Some(inputs) = v.get("inputs").and_then(|i| i.as_array()) else { return true };
        !inputs
            .iter()
            .filter_map(|i| i.get("path").and_then(|p| p.as_str()))
            .any(|p| Path::new(p).exists())
    }

    /// Whole or not at all: a temporary file in the same directory, then a rename. On every
    /// filesystem htl runs on that rename is atomic, so a concurrent reader sees either the
    /// previous entry or this one, never half of this one. ccache relies on exactly this
    /// and takes no locks at all.
    fn write<T: Serialize>(&self, key: &Key, entry: &T) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let final_path = self.entry_path(key);
        let tmp = self.dir.join(format!(".{}.{}.tmp", key.hash, std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(entry)?)?;
        if let Err(e) = std::fs::rename(&tmp, &final_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}
