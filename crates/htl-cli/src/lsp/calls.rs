//! The call hierarchy, made of the reference index ("Call hierarchy" in the
//! [`lsp`](super) module doc is the account of what it answers and leaves out; this is the
//! shape of it).
//!
//! There is no index of its own. A file's [`FileRefs`] hold its uses, each with whether it
//! is a call, and the declarations of the same check as a flat list ([`Sym`]), which says
//! what is a function and how far its body reaches. The incoming calls of a function are the
//! uses of its declaration that are calls, grouped by the function each is inside of; the
//! outgoing calls of a function are the calls inside it, grouped by the function each is a
//! call of. Both are read off the same entries, kept current by the same rules, as
//! references are.
//!
//! An item names a declaration for the requests that follow `prepareCallHierarchy`, which
//! the protocol hands back to the server as the client received it: the file is the item's
//! URI, and `data` holds what finds the declaration in the file again, its two places and
//! its name ([`data_of`]), so an item whose declaration has moved or gone is told from one
//! that has not.

use super::refs::{FileRefs, Place};
use super::symbols::{self, Entry, Sym, SymbolIndex};
use super::{lines_of, position_at, read_lossy, uri_of};
use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Position, Range,
    SymbolKind, Uri,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a request is made from, for the URIs and the names an answer carries.
pub struct Ctx<'a> {
    /// The project's root, canonical: a file's item is named by its path from there.
    pub root: &'a Path,
    /// The file the request is made in, canonical, and the URI the client called it by: an
    /// answer names that file the way the client does.
    pub here: &'a Path,
    pub uri: &'a Uri,
}

impl Ctx<'_> {
    /// The URI of an indexed file: the client's own for the file of the request.
    fn uri_for(&self, canonical: &Path, e: &Entry) -> Option<Uri> {
        if canonical == self.here {
            Some(self.uri.clone())
        } else {
            std::path::absolute(&e.path).ok().and_then(|p| uri_of(&p))
        }
    }
}

/// What an item stands for: a declaration, or the top level of a file.
pub enum Which {
    /// The function declared at these places, with this name.
    Function {
        at: [(usize, usize); 2],
        name: String,
    },
    /// The statements of a file that are in no function.
    Chunk,
}

/// The `data` of an item for `sym`: the places the checker names it by and its name. Equal
/// places and name are one declaration; a file edited so that the declaration is elsewhere
/// finds none, and the request is answered `null`.
pub fn data_of(sym: &Sym) -> Value {
    json!({
        "at": [[sym.at[0].0, sym.at[0].1], [sym.at[1].0, sym.at[1].1]],
        "name": sym.name,
    })
}

/// The `data` of the item for a file's top level.
fn chunk_data() -> Value {
    json!({ "chunk": true })
}

/// What an item's `data` says it stands for; `None` for data this server did not make.
pub fn which(data: &Value) -> Option<Which> {
    if data.get("chunk") == Some(&Value::Bool(true)) {
        return Some(Which::Chunk);
    }
    let pair = |v: &Value| Some((v[0].as_u64()? as usize, v[1].as_u64()? as usize));
    let at = data.get("at")?;
    Some(Which::Function {
        at: [pair(&at[0])?, pair(&at[1])?],
        name: data.get("name")?.as_str()?.to_string(),
    })
}

/// The item for a function: its declaration's extent, its name, its type.
pub fn item(uri: Uri, sym: &Sym) -> CallHierarchyItem {
    CallHierarchyItem {
        name: sym.name.clone(),
        kind: sym.kind,
        tags: None,
        detail: sym.detail.clone(),
        uri,
        range: sym.full,
        selection_range: sym.range,
        data: Some(data_of(sym)),
    }
}

/// The item for the top level of a file: a `File`, named by its path from the project's
/// root (the file's name when it is not under it), as long as the file, with the start of it
/// as the selection.
fn chunk_item(ctx: &Ctx, canonical: &Path, e: &Entry, uri: Uri) -> CallHierarchyItem {
    let name = canonical
        .strip_prefix(ctx.root)
        .ok()
        .or_else(|| canonical.file_name().map(Path::new))
        .unwrap_or(canonical)
        .display()
        .to_string();
    let text = read_lossy(&e.path);
    let lines = lines_of(&text);
    let end = position_at(
        &lines,
        lines.len().max(1),
        lines.last().map_or(1, |l| l.len() + 1),
    );
    CallHierarchyItem {
        name,
        kind: SymbolKind::FILE,
        tags: None,
        detail: None,
        uri,
        range: Range::new(Position::new(0, 0), end),
        selection_range: Range::default(),
        data: Some(chunk_data()),
    }
}

/// The places a use of the function `sym` is declared at, in the file `file`: where the
/// checker puts the declaration of a function, of a variable that holds one, and the type
/// the record's body wrote for it ([`Sym::at`]).
pub fn places(file: &Path, sym: &Sym) -> Vec<Place> {
    let file: Arc<Path> = file.into();
    let mut at = vec![sym.at[0], sym.at[1]];
    at.extend(sym.same.map(|pair| pair[1]));
    at.into_iter()
        .map(|(line, col)| Place {
            file: file.clone(),
            line,
            col,
        })
        .collect()
}

/// The declaration an item stands for: the file's entry and uses, and the index of the
/// function in them (`None` for the file's top level). `None` when the file is not in the
/// index, was not checked, or holds no such function now.
pub fn resolve<'a>(
    index: &'a SymbolIndex,
    canonical: &Path,
    data: &Value,
) -> Option<(&'a Entry, &'a FileRefs, Option<usize>)> {
    let e = index.entry(canonical)?;
    let fr = e.refs.as_ref()?;
    match which(data)? {
        Which::Chunk => Some((e, fr, None)),
        Which::Function { at, name } => {
            let i = fr
                .syms
                .iter()
                .position(|s| s.is_function() && s.at == at && s.name == name)?;
            Some((e, fr, Some(i)))
        }
    }
}

/// The calls of the function `class` lists the places of, grouped by the function each is
/// inside of, in the order of the files, the top level of a file before its functions. The
/// flag says that a call was found in a file that is no longer on disk (a watcher has not
/// said so): the index is to be built again.
pub fn incoming(
    index: &SymbolIndex,
    ctx: &Ctx,
    class: &[Place],
) -> (Vec<CallHierarchyIncomingCall>, bool) {
    let (mut out, mut gone) = (Vec::new(), false);
    for (canonical, e) in index.iter() {
        let Some(fr) = &e.refs else {
            continue;
        };
        let mut groups: BTreeMap<Option<usize>, Vec<Range>> = BTreeMap::new();
        for r in fr
            .refs
            .iter()
            .filter(|r| r.call && class.contains(&r.declared))
        {
            groups
                .entry(fr.enclosing(r.range.start))
                .or_default()
                .push(r.range);
        }
        if groups.is_empty() {
            continue;
        }
        let Some(uri) = e
            .path
            .is_file()
            .then(|| ctx.uri_for(canonical, e))
            .flatten()
        else {
            gone = true;
            continue;
        };
        for (from, from_ranges) in groups {
            out.push(CallHierarchyIncomingCall {
                from: match from {
                    Some(i) => item(uri.clone(), &fr.syms[i]),
                    None => chunk_item(ctx, canonical, e, uri.clone()),
                },
                from_ranges,
            });
        }
    }
    (out, gone)
}

/// The calls inside the function `from` of `fr` (the file's top level for `None`) whose
/// declaration is a function of a file of the index, grouped by that function, in the order
/// of the files and of the declarations in them. A call to the standard library, to a name
/// the check did not resolve, to a parameter or a local the tree of declarations does not
/// hold, or to a function of a file the walk does not visit is not among them. The flag is
/// as for [`incoming`].
pub fn outgoing(
    index: &SymbolIndex,
    ctx: &Ctx,
    fr: &FileRefs,
    from: Option<usize>,
) -> (Vec<CallHierarchyOutgoingCall>, bool) {
    let mut groups: BTreeMap<(PathBuf, usize), Vec<Range>> = BTreeMap::new();
    for r in fr
        .refs
        .iter()
        .filter(|r| r.call && fr.enclosing(r.range.start) == from)
    {
        let Some(to) = index.entry(&r.declared.file).and_then(|e| e.refs.as_ref()) else {
            continue;
        };
        let Some(i) = symbols::find(&to.syms, (r.declared.line, r.declared.col)) else {
            continue;
        };
        if to.syms[i].is_function() {
            groups
                .entry((r.declared.file.to_path_buf(), i))
                .or_default()
                .push(r.range);
        }
    }
    let (mut out, mut gone) = (Vec::new(), false);
    for ((file, i), from_ranges) in groups {
        let Some(e) = index.entry(&file) else {
            continue;
        };
        let Some(sym) = e
            .refs
            .as_ref()
            .and_then(|f| f.syms.get(i))
            .filter(|_| e.path.is_file())
        else {
            gone = true;
            continue;
        };
        if let Some(uri) = ctx.uri_for(&file, e) {
            out.push(CallHierarchyOutgoingCall {
                to: item(uri, sym),
                from_ranges,
            });
        }
    }
    (out, gone)
}
