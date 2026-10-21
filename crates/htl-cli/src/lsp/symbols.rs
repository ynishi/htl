//! The project-wide symbol index behind the workspace-symbol request ("Workspace symbols" in
//! the [`lsp`](super) module doc is the account of what is indexed and how it is kept
//! current; this is the structure that does it).
//!
//! One [`SymbolIndex`] per project, holding for each `.tl` the walk of `htl check` visits the
//! flat list of [`Sym`]s it declares and what it takes to tell whether they are still the
//! file's: its [`Stamp`] (modification time and length) and a hash of the text they were read
//! from. The index does no parsing and knows nothing of the checker: [`SymbolIndex::refresh`]
//! is handed the list of files and a function that makes a file's symbols from its text, so
//! the server decides where they come from (an open file's last check, else
//! [`Htl::declarations_of`](htl::Htl::declarations_of)).

use super::{lines_of, position_at, read_lossy};
use htl::typeindex::{Decl, DeclKind};
use lsp_types::{Range, SymbolKind};
use std::collections::{BTreeMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// The most symbols one answer carries. The protocol has no paging for a workspace-symbol
/// request, so a project that has more matches than this gets the best ones: the order of
/// [`search`] puts the exact name matches first, so what is cut is the tail of the
/// substring matches, and a client that wants one of them narrows the query.
pub const MAX_RESULTS: usize = 1000;

/// One declaration, flattened out of a file's tree.
#[derive(Debug, Clone)]
pub struct Sym {
    /// The name as the file's outline shows it (`R.f` for a record function whose record
    /// is declared elsewhere).
    pub name: String,
    /// The name in lower case, without the `R.` of an `R.f`: what a query is matched with.
    pub lower: String,
    /// The whole name in lower case (`r.f`), matched too.
    pub full_lower: String,
    /// The name of the declaration this one is a child of: the record a field or a record
    /// function belongs to. `None` at the top of a file.
    pub container: Option<String>,
    pub kind: SymbolKind,
    /// The name, as LSP positions in the text the symbols were read from.
    pub range: Range,
}

/// The LSP kind of a declaration. The same table as the outline's.
pub fn kind_of(kind: DeclKind) -> SymbolKind {
    match kind {
        DeclKind::Local => SymbolKind::VARIABLE,
        DeclKind::Function => SymbolKind::FUNCTION,
        DeclKind::Method => SymbolKind::METHOD,
        DeclKind::Field => SymbolKind::FIELD,
        DeclKind::Record => SymbolKind::STRUCT,
        DeclKind::Interface => SymbolKind::INTERFACE,
        DeclKind::Enum => SymbolKind::ENUM,
        DeclKind::Alias => SymbolKind::TYPE_PARAMETER,
    }
}

/// The declarations of a file as a flat list, children after their parent and each carrying
/// its parent's name. `text` is the text the tree was read from.
pub fn flatten(text: &str, decls: &[Decl]) -> Vec<Sym> {
    fn walk(lines: &[&str], decls: &[Decl], container: Option<&str>, out: &mut Vec<Sym>) {
        for d in decls {
            let start = position_at(lines, d.name_line, d.name_col);
            let end = position_at(
                lines,
                d.name_line,
                d.name_col.saturating_add(d.short_name().len()),
            );
            out.push(Sym {
                name: d.name.clone(),
                lower: d.short_name().to_lowercase(),
                full_lower: d.name.to_lowercase(),
                container: container.map(str::to_string),
                kind: kind_of(d.kind),
                range: Range::new(start, end),
            });
            walk(lines, &d.children, Some(&d.name), out);
        }
    }
    let lines = lines_of(text);
    let mut out = Vec::new();
    walk(&lines, decls, None, &mut out);
    out
}

/// How well `query` (already lower case) matches a symbol: 0 for the name itself, 1 for a
/// name that starts with it, 2 for one that holds it, `None` for no match. An empty query
/// matches everything alike (2). The name is the short one (`f` of `R.f`), or the whole of
/// it (`R.f`), whichever matches better.
pub fn rank(query: &str, sym: &Sym) -> Option<u8> {
    if query.is_empty() {
        return Some(2);
    }
    [sym.lower.as_str(), sym.full_lower.as_str()]
        .into_iter()
        .filter_map(|n| {
            if n == query {
                Some(0)
            } else if n.starts_with(query) {
                Some(1)
            } else if n.contains(query) {
                Some(2)
            } else {
                None
            }
        })
        .min()
}

/// A symbol that matched, with where it is.
pub struct Hit<'a> {
    pub rank: u8,
    pub path: &'a Path,
    pub sym: &'a Sym,
}

/// The symbols of `files` that match `query` (case-insensitive substring), best first: exact
/// name, then prefix, then the rest; within a rank by path, then line, then column, so that
/// the same query on the same files is the same answer. At most [`MAX_RESULTS`]; the second
/// number is how many matched before the cut.
pub fn search<'a>(
    files: impl IntoIterator<Item = &'a Entry>,
    query: &str,
) -> (Vec<Hit<'a>>, usize) {
    let query = query.to_lowercase();
    let mut hits: Vec<Hit<'a>> = Vec::new();
    for e in files {
        for sym in &e.syms {
            if let Some(rank) = rank(&query, sym) {
                hits.push(Hit {
                    rank,
                    path: &e.path,
                    sym,
                });
            }
        }
    }
    let order = |h: &Hit<'a>| {
        (
            h.rank,
            h.path,
            h.sym.range.start.line,
            h.sym.range.start.character,
        )
    };
    let total = hits.len();
    // Only what is answered is put in order: an empty query matches every symbol.
    if total > MAX_RESULTS {
        hits.select_nth_unstable_by(MAX_RESULTS, |a, b| order(a).cmp(&order(b)));
        hits.truncate(MAX_RESULTS);
    }
    hits.sort_by(|a, b| order(a).cmp(&order(b)));
    (hits, total)
}

/// What the file system says of a file, cheaply: when it was last written and how long it
/// is. Two files with the same stamp are taken to hold the same text, except when the write
/// was too recent to tell (see [`Entry::settled`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl Stamp {
    /// `None` when the file is not there.
    pub fn of(path: &Path) -> Option<Self> {
        let m = std::fs::metadata(path).ok()?;
        Some(Self {
            mtime: m.modified().ok(),
            len: m.len(),
        })
    }

    /// Whether a text read now, under this stamp, will be told apart from a later write by
    /// the stamp alone. The file system's clock has a granularity (and a write and a read
    /// can fall in one tick of it), so a file written within the last two seconds could be
    /// written again with the same length and the same stamp. Git's index has the same
    /// problem ("racily clean" entries) and the same answer: such an entry is not trusted
    /// by its stamp, and the next refresh reads it and compares the text.
    fn settled(&self) -> bool {
        self.mtime
            .is_some_and(|t| t + Duration::from_secs(2) <= SystemTime::now())
    }
}

/// A hash of the text the symbols of an entry were read from.
pub fn hash_of(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// One file of the index.
#[derive(Debug, Clone)]
pub struct Entry {
    /// The file as the walk spelled it, which is how its URI is made.
    pub path: PathBuf,
    stamp: Stamp,
    /// A hash of the text `syms` were read from.
    hash: u64,
    /// Whether `stamp` alone is a reason to believe the file is unchanged ([`Stamp::settled`],
    /// worked out when the entry was made).
    settled: bool,
    /// Empty for a file that does not parse, and for one that declares nothing.
    pub syms: Vec<Sym>,
}

impl Entry {
    /// An entry for `path` whose symbols were read from `text`, the text the stamp was taken
    /// of (a stamp taken *before* the text was read, so that a write between the two leaves
    /// the entry out of date and not wrongly current).
    pub fn new(path: PathBuf, stamp: Stamp, text: &str, syms: Vec<Sym>) -> Self {
        Self {
            path,
            stamp,
            hash: hash_of(text),
            settled: stamp.settled(),
            syms,
        }
    }
}

/// The symbols of one project's files, by canonical path (so a file is one entry however it
/// is spelled, and the order is the same every time).
#[derive(Debug)]
pub struct SymbolIndex {
    files: BTreeMap<PathBuf, Entry>,
    /// Files a watcher said changed since the last refresh.
    dirty: HashSet<PathBuf>,
    /// Set when the list of files may have changed: the first time, and when a watcher names
    /// a `.tl` the index does not hold (a file made, or one the walk does not visit).
    relist: bool,
}

impl Default for SymbolIndex {
    /// An index of nothing yet, due for its first walk.
    fn default() -> Self {
        Self {
            files: BTreeMap::new(),
            dirty: HashSet::new(),
            relist: true,
        }
    }
}

impl SymbolIndex {
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.files.values()
    }

    /// Make the next refresh walk the project again: something the watchers did not name
    /// changed (a file of the index is gone, and the directory it was in may have moved).
    pub fn relist(&mut self) {
        self.relist = true;
    }

    /// A watcher says `canonical` was made, changed or deleted. Nothing is read now: a file
    /// the index holds is looked at again at the next refresh (and dropped if it is gone), and
    /// one it does not hold makes the next refresh walk the project again.
    pub fn event(&mut self, canonical: &Path) {
        if self.files.contains_key(canonical) {
            self.dirty.insert(canonical.to_path_buf());
        } else {
            self.relist = true;
        }
    }

    /// Put `entry` in for `canonical`, if the index holds that file: a file just checked,
    /// whose symbols are fresher than anything read from the disk (they carry the kinds
    /// types decide) and need not be read again.
    pub fn put(&mut self, canonical: &Path, mut entry: Entry) {
        if let Some(slot) = self.files.get_mut(canonical) {
            // Spelled as the walk spelled it, whatever the client called the file.
            entry.path = slot.path.clone();
            *slot = entry;
            self.dirty.remove(canonical);
        }
    }

    /// Bring the index up to date.
    ///
    /// `sweep` is for a client that does not say when files change: the project is walked
    /// again (`list`) and every file is stamped, whether or not anything was said. Without
    /// it only what the watchers named is looked at, and the walk happens when they named a
    /// file the index does not hold. A file is read again only if its stamp moved, or it is
    /// not [settled](Stamp::settled), and its symbols are made again only if the text it
    /// holds is not the text they were made from.
    ///
    /// `build(canonical, path, text)` is the symbols of a file whose text is `text`. An error
    /// is the walk's (`list`), and leaves the index as it was.
    pub fn refresh(
        &mut self,
        sweep: bool,
        list: impl FnOnce() -> anyhow::Result<Vec<PathBuf>>,
        build: &mut dyn FnMut(&Path, &Path, &str) -> Vec<Sym>,
    ) -> anyhow::Result<()> {
        if sweep || self.relist {
            // A walk that fails leaves the index as it was, and still due for one.
            let listed = list()?;
            let mut kept = BTreeMap::new();
            for path in listed {
                let canonical = super::canonical(&path);
                let old = self.files.remove(&canonical);
                let force = self.dirty.contains(&canonical);
                if let Some(e) = revalidate(old, &canonical, path, force, build) {
                    kept.insert(canonical, e);
                }
            }
            self.files = kept;
        } else {
            for canonical in std::mem::take(&mut self.dirty) {
                if let Some(old) = self.files.remove(&canonical) {
                    let path = old.path.clone();
                    if let Some(e) = revalidate(Some(old), &canonical, path, true, build) {
                        self.files.insert(canonical, e);
                    }
                }
            }
        }
        self.dirty.clear();
        self.relist = false;
        Ok(())
    }
}

/// `old`, if it still describes the file; else a new entry; `None` if the file is gone.
fn revalidate(
    old: Option<Entry>,
    canonical: &Path,
    path: PathBuf,
    force: bool,
    build: &mut dyn FnMut(&Path, &Path, &str) -> Vec<Sym>,
) -> Option<Entry> {
    // The stamp before the text: a write between the two leaves the stamp older than the
    // text, and the next refresh sees it move.
    let stamp = Stamp::of(&path)?;
    if let Some(e) = &old
        && !force
        && e.stamp == stamp
        && e.settled
    {
        return old;
    }
    let text = read_lossy(&path);
    if let Some(mut e) = old
        && e.hash == hash_of(&text)
    {
        e.stamp = stamp;
        e.settled = stamp.settled();
        return Some(e);
    }
    let syms = build(canonical, &path, &text);
    Some(Entry::new(path, stamp, &text, syms))
}
