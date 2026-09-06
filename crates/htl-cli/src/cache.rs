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
const FORMAT: u32 = 2;

/// Where the store lives under the project root. Generated, and `htl init` puts it in
/// `.gitignore`.
const DIR: &str = ".htl/cache";

fn debugging() -> bool {
    std::env::var_os("HTL_CACHE_DEBUG").is_some()
}

fn miss(reason: &str) {
    if debugging() {
        eprintln!("htl cache: miss ({reason})");
    }
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

/// One directory a `require` could have resolved in, by the set of module names it held.
/// Catches a file appearing where the checker would now find it, which no hash of the
/// files it *did* read can see.
#[derive(Serialize, Deserialize, Debug)]
struct Probe {
    dir: String,
    /// Hash of the sorted `.tl` / `.d.tl` / `.lua` names in the directory.
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
}

#[derive(Serialize, Deserialize, Debug)]
struct Entry {
    stamp: Stamp,
    inputs: Vec<Input>,
    probes: Vec<Probe>,
    module: Module,
}

/// One entry holding the whole walk, for `Mode::WholeRun`.
#[derive(Serialize, Deserialize, Debug)]
struct RunEntry {
    stamp: Stamp,
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
}

/// The identity of one module under one invocation: same key, same question.
#[derive(Debug, Clone)]
pub struct Key(String);

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

/// The module names a directory offers, hashed. An unreadable directory hashes as empty,
/// which is the honest answer: it offers nothing right now, and if it offered something
/// when the entry was written the hashes differ and the module gets checked.
fn hash_dir_names(dir: &Path) -> String {
    let mut names: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".tl") || n.ends_with(".lua") {
                names.push(n);
            }
        }
    }
    names.sort();
    let mut h = blake3::Hasher::new();
    for n in &names {
        h.update(n.as_bytes());
        h.update(b"\0");
    }
    h.finalize().to_hex().to_string()
}

/// The key for one module in this invocation.
///
/// `spelling` is the path as the CLI will hand it to the checker, not a resolved one: the
/// checker echoes it into every diagnostic it prints, so `htl check .` and
/// `htl check src/a.tl` produce different text for the same module and cannot share an
/// entry. The working directory completes it, since the same relative spelling means a
/// different file from somewhere else.
pub fn module_key(spelling: &Path, lint: Option<&str>) -> Key {
    let mut h = blake3::Hasher::new();
    h.update(b"htl-module\0");
    h.update(spelling.to_string_lossy().as_bytes());
    h.update(b"\0");
    h.update(lint.unwrap_or("").as_bytes());
    h.update(b"\0");
    h.update(cwd().as_bytes());
    Key(h.finalize().to_hex().to_string())
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
    Key(h.finalize().to_hex().to_string())
}

/// The working directory, resolved once. This is called for every module in the walk, and
/// resolving it means a `canonicalize` syscall — doing that per module is a measurable part
/// of a run that replays everything and does nothing else.
fn cwd() -> &'static str {
    static CWD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CWD.get_or_init(|| std::env::current_dir().map(|p| normal(&p)).unwrap_or_default())
}

/// A store rooted at a project.
pub struct Cache {
    dir: PathBuf,
    mode: Mode,
    /// Content hashes taken during this run, by normalized path.
    ///
    /// Entries overlap heavily: every module in a project tends to require the same few
    /// leaves, so a leaf appears in the inputs of most entries. Without this, checking 48
    /// modules against the store reads and hashes the shared ones 48 times — which is most
    /// of what a fully replayed run spends. A file is assumed not to change while one
    /// `htl check` is running; if it does, the run's answer was undefined anyway.
    hashes: RefCell<HashMap<String, Option<String>>>,
    /// Directory listings taken during this run, by normalized path. Same reasoning.
    dirs: RefCell<HashMap<String, String>>,
}

impl Cache {
    /// `None` when the caller turned caching off, or when this build cannot identify
    /// itself well enough to be sure an entry is its own.
    pub fn open(root: &Path, enabled: bool, mode: Mode) -> Option<Self> {
        if !enabled {
            return None;
        }
        Stamp::current()?;
        Some(Self {
            dir: root.join(DIR),
            mode,
            hashes: RefCell::new(HashMap::new()),
            dirs: RefCell::new(HashMap::new()),
        })
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

    /// The module names in a directory, listed once per run. The project root, `src/` and
    /// `types/` are probed by every entry, so this overlaps even more than the file hashes.
    fn dir_names_of(&self, dir: &str) -> String {
        if let Some(n) = self.dirs.borrow().get(dir) {
            return n.clone();
        }
        let n = hash_dir_names(Path::new(dir));
        self.dirs.borrow_mut().insert(dir.to_string(), n.clone());
        n
    }

    fn entry_path(&self, key: &Key) -> PathBuf {
        self.dir.join(format!("{}.json", key.0))
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// A truncated or hand-edited entry is a miss, not a crash.
    fn parse<T: serde::de::DeserializeOwned>(raw: &str) -> Option<T> {
        match serde_json::from_str(raw) {
            Ok(e) => Some(e),
            Err(e) => {
                miss(&format!("unreadable entry: {e}"));
                None
            }
        }
    }

    /// Whether an entry still describes the world: written by this build, every file it read
    /// unchanged, every directory it could have resolved in holding the same modules.
    fn still_valid(&self, stamp: &Stamp, inputs: &[Input], probes: &[Probe]) -> bool {
        let Some(current) = Stamp::current() else { return false };
        if *stamp != current {
            miss("written by a different build");
            return false;
        }
        for i in inputs {
            match self.hash_of(&i.path) {
                Some(h) if h == i.hash => {}
                Some(_) => {
                    miss(&format!("changed: {}", i.path));
                    return false;
                }
                None => {
                    miss(&format!("gone: {}", i.path));
                    return false;
                }
            }
        }
        for p in probes {
            if self.dir_names_of(&p.dir) != p.names {
                miss(&format!("directory contents changed: {}", p.dir));
                return false;
            }
        }
        true
    }

    /// What each file in the walk reported last time; `None` where it has to be checked.
    ///
    /// Under `WholeRun` this is all-or-nothing by construction: one entry covers the walk,
    /// so a single changed input means every module is checked.
    pub fn lookup_all(&self, keys: &[Key], run: &Key, files: usize) -> Vec<Option<Module>> {
        match self.mode {
            Mode::PerModule => keys.iter().map(|k| self.lookup(k)).collect(),
            Mode::WholeRun => match self.lookup_run(run, files) {
                Some(ms) => ms.into_iter().map(Some).collect(),
                None => vec![None; files],
            },
        }
    }

    /// What the module reported last time, if every input and probe still matches.
    fn lookup(&self, key: &Key) -> Option<Module> {
        let raw = std::fs::read_to_string(self.entry_path(key)).ok()?;
        let entry: Entry = Self::parse(&raw)?;
        self.still_valid(&entry.stamp, &entry.inputs, &entry.probes).then_some(entry.module)
    }

    fn lookup_run(&self, key: &Key, files: usize) -> Option<Vec<Module>> {
        let raw = std::fs::read_to_string(self.entry_path(key)).ok()?;
        let entry: RunEntry = Self::parse(&raw)?;
        if entry.modules.len() != files {
            miss("the walk visits a different number of files");
            return None;
        }
        self.still_valid(&entry.stamp, &entry.inputs, &entry.probes).then_some(entry.modules)
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

    fn probes_for(&self, dirs: &[PathBuf]) -> Vec<Probe> {
        let mut dirs: Vec<String> = dirs.iter().map(|p| normal(p)).collect();
        dirs.sort();
        dirs.dedup();
        dirs.into_iter()
            .map(|d| {
                let names = self.dir_names_of(&d);
                Probe { dir: d, names }
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
        let entry = Entry { stamp, inputs, probes: self.probes_for(dirs), module: module.clone() };
        if let Err(e) = self.write(key, &entry)
            && debugging()
        {
            eprintln!("htl cache: not stored ({e})");
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
        let entry = RunEntry { stamp, inputs, probes: self.probes_for(dirs), modules: modules.to_vec() };
        if let Err(e) = self.write(key, &entry)
            && debugging()
        {
            eprintln!("htl cache: not stored ({e})");
        }
    }

    /// Whole or not at all: a temporary file in the same directory, then a rename. On every
    /// filesystem htl runs on that rename is atomic, so a concurrent reader sees either the
    /// previous entry or this one, never half of this one. ccache relies on exactly this
    /// and takes no locks at all.
    fn write<T: Serialize>(&self, key: &Key, entry: &T) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let final_path = self.entry_path(key);
        let tmp = self.dir.join(format!(".{}.{}.tmp", key.0, std::process::id()));
        std::fs::write(&tmp, serde_json::to_vec(entry)?)?;
        if let Err(e) = std::fs::rename(&tmp, &final_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}
