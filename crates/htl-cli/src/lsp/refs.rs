//! What the reference index holds for one file: its uses, each with the declaration it
//! stands for ("References" in the [`lsp`](super) module doc is the account of what is
//! indexed, kept current and not resolved; this is the shape of it).
//!
//! The uses of a file are [`htl::typeindex::TypeIndex::uses`] of its last check, made into
//! what a request compares and answers with: the place each use declares itself by in a form
//! two spellings of one file cannot differ in (the canonical path), and the range in LSP's
//! terms, converted once against the text the check read. Where they are kept, and when they
//! are thrown away, is the [`SymbolIndex`](super::symbols::SymbolIndex)'s: one list of files,
//! one freshness rule, for the symbols and for the references.

use super::{canonical, position_at};
use htl::typeindex::{TypeIndex, Use};
use lsp_types::Range;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A declaration: the canonical file, and the line and byte column (from 1) the checker
/// names it by ([`htl::typeindex::Place`]). Two uses are of one declaration when these are
/// equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub file: Arc<Path>,
    pub line: usize,
    pub col: usize,
}

impl Place {
    /// The place the checker named, as a canonical one; `None` when the file is not one on
    /// disk (the standard library's declarations are in a file Teal carries inside itself).
    pub fn of(declared: &htl::typeindex::Place) -> Option<Self> {
        let path = canonical(&declared.file);
        path.is_file().then(|| Self {
            file: path.into(),
            line: declared.line,
            col: declared.col,
        })
    }
}

/// One use of a declaration.
#[derive(Debug, Clone)]
pub struct Ref {
    /// Where the name is, as the checker counts (1-based line, byte column), for comparing
    /// with a declaration's place.
    pub line: usize,
    pub col: usize,
    /// Where the name is, as LSP positions in the text the check read.
    pub range: Range,
    /// The declaration it stands for.
    pub declared: Place,
    /// Whether it is called there (`f(`, `o:f(`, `f "s"`, `f {`): what tells a call site
    /// from the other uses, for the call hierarchy that is built on these. No request reads
    /// it yet.
    #[allow(dead_code)]
    pub call: bool,
}

/// The uses of one file, and the files its check read (canonical): what a change to makes
/// them stale. `deps` is `None` when that is not known (the check failed): a change to any
/// file then makes them stale.
#[derive(Debug, Clone, Default)]
pub struct FileRefs {
    pub refs: Vec<Ref>,
    pub deps: Option<HashSet<PathBuf>>,
}

/// The uses of the file whose text is `text` and whose check made `types`, `deps` being the
/// files it read through `require`. A use whose declaration is not in a file on disk is left
/// out: nothing can be answered with it.
pub fn collect(text: &str, types: &TypeIndex, deps: impl IntoIterator<Item = PathBuf>) -> FileRefs {
    let lines = super::lines_of(text);
    let mut files: HashMap<PathBuf, Option<Arc<Path>>> = HashMap::new();
    let mut refs = Vec::new();
    for Use {
        line,
        col,
        end,
        declared,
        call,
    } in types.uses(text)
    {
        // One canonical path per file the uses name, not one per use.
        let file = files
            .entry(declared.file.clone())
            .or_insert_with(|| Place::of(&declared).map(|p| p.file))
            .clone();
        let Some(file) = file else {
            continue;
        };
        refs.push(Ref {
            line,
            col,
            range: Range::new(
                position_at(&lines, line, col),
                position_at(&lines, line, end),
            ),
            declared: Place {
                file,
                line: declared.line,
                col: declared.col,
            },
            call,
        });
    }
    FileRefs {
        refs,
        deps: Some(deps.into_iter().collect()),
    }
}
