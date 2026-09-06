//! An on-disk cache for a whole `htl check` run (issue #3).
//!
//! A run that changed nothing repeats every bit of work the run before it did: building
//! a checker costs about 13.5 ms, and type-checking a few thousand lines of Teal costs
//! about a second (`crates/htl-core/benches/check.rs`). This stores what a run printed,
//! keyed on everything that fed it, and replays it when none of that moved.
//!
//! # Whole run, not per module
//!
//! Caching individual modules is the obvious design and it is not sound in the current
//! code: `htl check` extends `package.path` as it walks the tree, so a module is checked
//! with the earlier directories visible, and checking a module seeds a store that later
//! modules read from. Skipping *some* modules changes what the rest see. An
//! all-or-nothing entry never runs a partial set, so neither problem arises.
//!
//! # What makes an entry
//!
//! The **key** is the invocation: the paths as the user wrote them (they appear verbatim
//! in diagnostics, so `htl check .` and `htl check src` are different runs), the flags
//! that change the report, and the format. The **inputs** are every file the run read,
//! by content hash. The **probes** are the directories it searched, by the set of module
//! names in each — a new `.tl` earlier on the search path changes what a `require`
//! resolves to while every recorded file hash still matches, and nothing else would catch
//! that. This is the hole ccache documents in its direct mode.
//!
//! # No mtimes anywhere
//!
//! Content hashes only. Timestamp-based invalidation is where this class of tool
//! historically breaks — second-granular filesystems on macOS, mtimes zeroed by Docker
//! layers, clock skew, fresh CI checkouts invalidating everything — and hashing 48
//! modules costs under a millisecond, which is noise against the second this saves.
//!
//! # Failure is a miss
//!
//! Every error path here returns "no entry" rather than propagating. A corrupt file, an
//! unreadable directory, a store on a read-only filesystem: the run proceeds as if the
//! cache were not there. The one invariant worth stating is mypy's: the entry is written
//! whole or not at all, via a temporary file and a rename, so a reader never sees half of
//! one. Set `HTL_CACHE_DEBUG=1` to print why a lookup missed.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Bumped by hand when anything below changes shape. An entry stamped with a different
/// value is a miss rather than an error: a fresh checkout and an upgrade both take that
/// path in normal operation, which is why rustc treats its own header mismatch the same
/// way.
const FORMAT: u32 = 1;

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
/// The binary's length and modification time stand in for "which checker is this":
/// during development the vendored Teal compiler, the prelude and the Rust side all
/// change many times within one released version number, and a stamp that only carried
/// the version would happily replay results from a checker that no longer exists.
/// Rebuilding always moves both. This is the same reasoning behind sccache hashing the
/// compiler binary into its key.
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
        let mtime = m
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            format: FORMAT,
            htl: env!("CARGO_PKG_VERSION").to_string(),
            exe_len: m.len(),
            exe_mtime_ns: mtime,
        })
    }
}

/// One file the run read, by content.
#[derive(Serialize, Deserialize, Debug)]
struct Input {
    path: String,
    hash: String,
}

/// One directory the run could have resolved a `require` in, by the set of module names
/// it held. Catches a file appearing where the checker would have found it, which no
/// hash of the files it *did* read can see.
#[derive(Serialize, Deserialize, Debug)]
struct Probe {
    dir: String,
    /// Hash of the sorted `.tl` / `.d.tl` / `.lua` names in the directory.
    names: String,
}

/// One diagnostic exactly as it was handed to the sink, so a replay goes through the
/// same printing code the original run did rather than through a reconstruction of it.
/// Reconstructing text from parsed fields is how a cache starts printing subtly
/// different output from the run it claims to reproduce.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Recorded {
    pub severity: String,
    pub text: String,
    pub fix: Option<crate::report::FixJson>,
}

/// Everything a `htl check` run printed and concluded.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Run {
    pub files: usize,
    pub diagnostics: Vec<Recorded>,
    pub errors: usize,
    pub warnings: usize,
    pub lints: usize,
    pub strict: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct Entry {
    stamp: Stamp,
    inputs: Vec<Input>,
    probes: Vec<Probe>,
    run: Run,
}

/// The identity of an invocation: same key, same question being asked.
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
/// while the same file reached as an import recorded an absolute one, so its hash
/// depended on how it was reached. Everything that goes into an entry comes through here.
fn normal(p: &Path) -> String {
    std::fs::canonicalize(p)
        .unwrap_or_else(|_| p.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// The module names a directory offers, hashed. Unreadable directory hashes as empty,
/// which is the honest answer: it offers nothing right now, and if it offered something
/// when the entry was written the hashes differ and the run happens.
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

/// The key for one `htl check` invocation.
///
/// `paths` goes in as written, not resolved: the checker puts those spellings into the
/// diagnostics it prints, so two invocations that name the same files differently do not
/// produce the same output and must not share an entry.
pub fn key(paths: &[PathBuf], strict: bool, lint: Option<&str>, json: bool) -> Key {
    let mut h = blake3::Hasher::new();
    h.update(b"htl-check\0");
    for p in paths {
        h.update(p.to_string_lossy().as_bytes());
        h.update(b"\0");
    }
    h.update(if strict { b"strict\0" } else { b"lax\0" });
    h.update(lint.unwrap_or("").as_bytes());
    h.update(b"\0");
    h.update(if json { b"json\0" } else { b"text\0" });
    // The working directory completes the identity: the same relative paths mean
    // different files from somewhere else.
    if let Ok(cwd) = std::env::current_dir() {
        h.update(normal(&cwd).as_bytes());
    }
    Key(h.finalize().to_hex().to_string())
}

/// A store rooted at a project.
pub struct Cache {
    dir: PathBuf,
}

impl Cache {
    /// `None` when the caller turned caching off, or when this build cannot identify
    /// itself well enough to be sure an entry is its own.
    pub fn open(root: &Path, enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }
        Stamp::current()?;
        Some(Self { dir: root.join(DIR) })
    }

    fn entry_path(&self, key: &Key) -> PathBuf {
        self.dir.join(format!("{}.json", key.0))
    }

    /// The stored run, if there is one whose inputs and probes all still match.
    pub fn lookup(&self, key: &Key) -> Option<Run> {
        let raw = std::fs::read_to_string(self.entry_path(key)).ok()?;
        let entry: Entry = match serde_json::from_str(&raw) {
            Ok(e) => e,
            Err(e) => {
                // A truncated or hand-edited entry is a miss, not a crash.
                miss(&format!("unreadable entry: {e}"));
                return None;
            }
        };
        let stamp = Stamp::current()?;
        if entry.stamp != stamp {
            miss("written by a different build");
            return None;
        }
        for i in &entry.inputs {
            match hash_file(Path::new(&i.path)) {
                Some(h) if h == i.hash => {}
                Some(_) => {
                    miss(&format!("changed: {}", i.path));
                    return None;
                }
                None => {
                    miss(&format!("gone: {}", i.path));
                    return None;
                }
            }
        }
        for p in &entry.probes {
            if hash_dir_names(Path::new(&p.dir)) != p.names {
                miss(&format!("directory contents changed: {}", p.dir));
                return None;
            }
        }
        Some(entry.run)
    }

    /// Record a run. Best-effort: a store that cannot be written leaves the next run to
    /// do the work again, which is slow rather than wrong.
    pub fn store(&self, key: &Key, files: &[PathBuf], deps: &[PathBuf], dirs: &[PathBuf], run: &Run) {
        let Some(stamp) = Stamp::current() else { return };

        // Inputs: everything read, deduplicated by normalized path so a file that is both
        // checked and depended on is hashed once and compared once.
        let mut paths: Vec<String> = files.iter().chain(deps).map(|p| normal(p)).collect();
        paths.sort();
        paths.dedup();
        let mut inputs = Vec::with_capacity(paths.len());
        for p in paths {
            let Some(hash) = hash_file(Path::new(&p)) else { return };
            inputs.push(Input { path: p, hash });
        }

        let mut dirs: Vec<String> = dirs.iter().map(|p| normal(p)).collect();
        dirs.sort();
        dirs.dedup();
        let probes = dirs
            .into_iter()
            .map(|d| {
                let names = hash_dir_names(Path::new(&d));
                Probe { dir: d, names }
            })
            .collect();

        let entry = Entry { stamp, inputs, probes, run: run.clone() };
        if let Err(e) = self.write(key, &entry)
            && debugging()
        {
            eprintln!("htl cache: not stored ({e})");
        }
    }

    /// Whole or not at all: a temporary file in the same directory, then a rename. On
    /// every filesystem htl runs on that rename is atomic, so a concurrent reader sees
    /// either the previous entry or this one, never half of this one. ccache relies on
    /// exactly this and takes no locks at all.
    fn write(&self, key: &Key, entry: &Entry) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let final_path = self.entry_path(key);
        let tmp = self.dir.join(format!(
            ".{}.{}.tmp",
            key.0,
            std::process::id()
        ));
        std::fs::write(&tmp, serde_json::to_vec(entry)?)?;
        if let Err(e) = std::fs::rename(&tmp, &final_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }
}
