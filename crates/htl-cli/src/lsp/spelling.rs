//! How a file is named to the client: the one place a path becomes a URI in an answer.
//!
//! The server keys everything it knows on the file's real path ([`canonical`](super::canonical):
//! symlinks resolved), because that is what makes two spellings of one file the same file.
//! A client does not know that path. It compares the URIs it is sent as text, so a location
//! it is given in a spelling it did not use (`/private/var/..` for the `/var/..` it opened,
//! the real directory for the symlink it named) opens a second buffer for a file it already
//! has, and a reference into an open file is not matched to it. So a path is translated back
//! to the client's spelling at the point where a URI is sent, and only there; nothing else in
//! the server builds a URI, and nothing outside this module may turn a path into one (a test
//! below reads the sources to say so).
//!
//! # The rules
//!
//! [`Spelling::client_uri`] takes the path of a file and applies the first rule that fits:
//!
//! 1. **An open file is named as it was opened.** If the file is one the client has open
//!    (whatever the path in hand is: the real one, a symlinked one, a differently written
//!    one), the answer is the key of that document, the URI the client sent. It is cloned,
//!    not rebuilt from a path, so what the client wrote is kept to the byte (an encoding of
//!    a drive letter, say). Where one file is open under two URIs, the one the request was
//!    made with is used when it is one of them ([`Spelling::asked_from`]), so an answer about
//!    the file of the request stays in the buffer it was asked from; else the smaller, so the
//!    answer does not depend on the order a map is walked. A file that is itself a symlink
//!    and is open is named by the URI it was opened with, not by its target.
//! 2. **A file under a workspace root is named under the root as the client gave it.** The
//!    roots are the `workspaceFolders` (else `rootUri`, else `rootPath`) of `initialize`, kept in
//!    the order the client sent them, each with its real path. The root whose real path is
//!    the longest prefix of the file's wins (a prefix by path component, not by text:
//!    `/a/proj` is not a prefix of `/a/project`), the earlier of two of the same length; the
//!    rest of the path is put after the root as the client wrote it.
//! 3. **Any other file is named by its real path.**
//!
//! `None` is the answer only when no URI can be made of the path (a path that is not
//! absolute, which [`url::Url`] refuses).
//!
//! What a file's real path is, for this purpose, is [`normal`]: the file's own when it is
//! there, and otherwise (a file deleted while open, a path a watcher reports) its directory's
//! real path with its own name kept.
//!
//! # What it does not cover
//!
//! A file reached through a symlink inside a root that points outside the root, and not
//! open, is not under the root once its symlinks are resolved: it is named by rule 3, by its
//! real path. The client never saw that spelling either, so no rule here could know it; a
//! client that opens it from the answer opens it under the real path. The workspace roots
//! are those of `initialize`; a folder added later (the `didChangeWorkspaceFolders`
//! notification) is not known, and files under it are named by rule 1 or 3.
//!
//! # Cost
//!
//! A [`Spelling`] is made for one request and dropped with it, from the server's fields
//! (`Spelling::new(&self.open, &self.roots)`, not through a method of the server, so the
//! other fields stay free to be borrowed mutably). It remembers the URI it made for each path
//! it was asked about, so a request that names a thousand symbols of one file asks the file
//! system about that file once. A path the caller knows is already real (an index key) goes
//! through [`Spelling::client_uri_canonical`] and is not resolved again.

use super::{Doc, canonical_dir_of};
use lsp_types::Uri;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// A workspace root: its real path ([`normal`]), and the path as the client gave it.
pub(super) type Root = (PathBuf, PathBuf);

/// The roots of a session, in the order the client named the folders.
pub(super) fn roots_of(workspace: &[PathBuf]) -> Vec<Root> {
    workspace.iter().map(|w| (normal(w), w.clone())).collect()
}

/// Where `p` really is, as a document is identified: the real path of the file when it is
/// there, else the real path of its directory with its own name kept. A symlink that is the
/// file itself is followed when it can be, so two spellings of one file are equal; a file
/// that is gone cannot be followed, and is at least in the right directory.
///
/// This is for telling whether two paths are one document (the key of an open [`Doc`], the
/// input of [`Spelling::client_uri`], a document compared with an index entry). It is not
/// for a manifest or a watcher's event, where a symlink is meant to stay where it is
/// (`canonical_dir_of` there).
pub(super) fn normal(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| canonical_dir_of(p))
}

/// The `file:` URI of a path, percent-encoded; `None` for a path that is not absolute.
fn uri_of(path: &Path) -> Option<Uri> {
    let url = url::Url::from_file_path(path).ok()?;
    Uri::from_str(url.as_str()).ok()
}

/// What an answer names a file by (module doc).
pub(super) struct Spelling<'a> {
    open: &'a HashMap<Uri, Doc>,
    roots: &'a [Root],
    /// The URI the request was made with, preferred over another URI of the same file.
    here: Option<&'a Uri>,
    /// The URI made for each path asked about (the path as it was given, before it is
    /// resolved): one request asks about the same few files again and again.
    made: RefCell<HashMap<PathBuf, Option<Uri>>>,
}

impl<'a> Spelling<'a> {
    /// The spelling over the server's open documents and workspace roots, taken as the two
    /// fields they are (see "Cost" in the module doc).
    // The server's own `open` map is keyed by `Uri` as well; the interior mutability clippy
    // finds in it is a cache the key's hash and equality do not read.
    #[allow(clippy::mutable_key_type)]
    pub(super) fn new(open: &'a HashMap<Uri, Doc>, roots: &'a [Root]) -> Self {
        Self {
            open,
            roots,
            here: None,
            made: RefCell::new(HashMap::new()),
        }
    }

    /// The same spelling for a request made with `uri`: a file open under `uri` and another
    /// URI as well is named by `uri` (rule 1).
    pub(super) fn asked_from(self, uri: &'a Uri) -> Self {
        Self {
            here: Some(uri),
            ..self
        }
    }

    /// The URI to name the file at `p` by, however `p` is written: a path relative to the
    /// working directory is made absolute first, symlinks are resolved ([`normal`]), and the
    /// rules of the module doc apply.
    pub(super) fn client_uri(&self, p: &Path) -> Option<Uri> {
        self.remembered(p, || {
            normal(&std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()))
        })
    }

    /// [`client_uri`](Self::client_uri) for a path the caller knows is real already (an
    /// index key), which is not resolved again.
    pub(super) fn client_uri_canonical(&self, canonical: &Path) -> Option<Uri> {
        self.remembered(canonical, || canonical.to_path_buf())
    }

    /// The URI for `p`, made from `real()` unless `p` was asked about before.
    fn remembered(&self, p: &Path, real: impl FnOnce() -> PathBuf) -> Option<Uri> {
        if let Some(known) = self.made.borrow().get(p) {
            return known.clone();
        }
        let uri = self.spelled(&real());
        self.made.borrow_mut().insert(p.to_path_buf(), uri.clone());
        uri
    }

    /// The rules, for a real path.
    fn spelled(&self, real: &Path) -> Option<Uri> {
        // 1. An open file keeps the URI the client opened it with: the request's, if it is
        // one of them.
        if let Some(here) = self.here
            && self.open.get(here).is_some_and(|d| d.canonical == real)
        {
            return Some(here.clone());
        }
        if let Some(uri) = self
            .open
            .iter()
            .filter(|(_, d)| d.canonical == real)
            .map(|(u, _)| u)
            .min()
        {
            return Some(uri.clone());
        }
        // 2. Under a root: the longest, the first of equals.
        let mut best: Option<(usize, PathBuf)> = None;
        for (root_real, root_spelled) in self.roots {
            let Ok(rest) = real.strip_prefix(root_real) else {
                continue;
            };
            let depth = root_real.components().count();
            if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                let under = if rest.as_os_str().is_empty() {
                    root_spelled.clone()
                } else {
                    root_spelled.join(rest)
                };
                best = Some((depth, under));
            }
        }
        if let Some(uri) = best.and_then(|(_, under)| uri_of(&under)) {
            return Some(uri);
        }
        // 3. Anywhere else, as it really is.
        uri_of(real)
    }
}

// The maps here are keyed by `Uri`, as the server's are.
#[cfg(test)]
#[allow(clippy::mutable_key_type)]
mod tests {
    use super::super::path_of;
    use super::*;
    use std::collections::HashSet;

    fn doc(path: &str, canonical: &str) -> Doc {
        Doc {
            path: PathBuf::from(path),
            canonical: PathBuf::from(canonical),
            project: None,
            deps: HashSet::new(),
            buffer: None,
            unsynced: false,
            checked: None,
        }
    }

    fn uri(s: &str) -> Uri {
        Uri::from_str(s).unwrap()
    }

    fn root(real: &str, spelled: &str) -> Root {
        (PathBuf::from(real), PathBuf::from(spelled))
    }

    #[test]
    fn a_path_and_its_uri_name_each_other() {
        let path = Path::new("/work/my project/é.tl");
        let uri = uri_of(path).unwrap();
        assert_eq!(uri.as_str(), "file:///work/my%20project/%C3%A9.tl");
        assert_eq!(path_of(&uri).as_deref(), Some(path));
        assert_eq!(
            path_of(&Uri::from_str("untitled:Untitled-1").unwrap()),
            None
        );
        // A path that is not absolute has no URI.
        assert_eq!(uri_of(Path::new("a.tl")), None);
    }

    #[test]
    fn an_open_file_is_named_by_the_uri_it_was_opened_with() {
        // The key is kept to the byte, whatever path it stands for.
        let key = uri("file:///c%3A/work/a.tl");
        let open = HashMap::from([(key.clone(), doc("/c:/work/a.tl", "/real/work/a.tl"))]);
        let roots = [root("/real/work", "/link")];
        let s = Spelling::new(&open, &roots);
        assert_eq!(
            s.client_uri(Path::new("/real/work/a.tl")),
            Some(key.clone())
        );
        assert_eq!(
            s.client_uri_canonical(Path::new("/real/work/a.tl")),
            Some(key)
        );
        // Not open: under the root.
        assert_eq!(
            s.client_uri(Path::new("/real/work/b.tl")),
            Some(uri("file:///link/b.tl"))
        );
    }

    #[test]
    fn a_file_open_under_two_uris_is_named_by_the_smaller() {
        let (a, b) = (uri("file:///link/a.tl"), uri("file:///real/a.tl"));
        let open = HashMap::from([
            (b.clone(), doc("/real/a.tl", "/real/a.tl")),
            (a.clone(), doc("/link/a.tl", "/real/a.tl")),
        ]);
        let s = Spelling::new(&open, &[]);
        assert_eq!(s.client_uri(Path::new("/real/a.tl")), Some(a.clone()));
        // A request made with the larger is answered with it.
        let s = Spelling::new(&open, &[]).asked_from(&b);
        assert_eq!(s.client_uri(Path::new("/real/a.tl")), Some(b));
        // A request made with a URI of another file changes nothing.
        let other = uri("file:///real/other.tl");
        let s = Spelling::new(&open, &[]).asked_from(&other);
        assert_eq!(s.client_uri(Path::new("/real/a.tl")), Some(a));
    }

    #[test]
    fn the_longest_root_wins_and_the_first_of_equals() {
        let open = HashMap::new();
        let roots = [
            root("/real", "/outer"),
            root("/real/inner", "/inner"),
            root("/real/inner", "/second"),
            // A prefix of the text and not of the path.
            root("/real/in", "/wrong"),
        ];
        let s = Spelling::new(&open, &roots);
        assert_eq!(
            s.client_uri(Path::new("/real/inner/x/y.tl")),
            Some(uri("file:///inner/x/y.tl"))
        );
        assert_eq!(
            s.client_uri(Path::new("/real/other.tl")),
            Some(uri("file:///outer/other.tl"))
        );
        // The root itself has no trailing slash added.
        assert_eq!(
            s.client_uri(Path::new("/real/inner")),
            Some(uri("file:///inner"))
        );
    }

    #[test]
    fn a_file_under_no_root_is_named_by_its_real_path() {
        let open = HashMap::new();
        let roots = [root("/real/work", "/link")];
        let s = Spelling::new(&open, &roots);
        assert_eq!(
            s.client_uri(Path::new("/elsewhere/a.tl")),
            Some(uri("file:///elsewhere/a.tl"))
        );
        // `/real/workshop` is not under `/real/work`.
        assert_eq!(
            s.client_uri(Path::new("/real/workshop/a.tl")),
            Some(uri("file:///real/workshop/a.tl"))
        );
        // A path relative to the working directory is made absolute first.
        assert!(s.client_uri(Path::new("relative/a.tl")).is_some());
    }

    /// `Url::from_file_path` is called here and nowhere else in the language server, so that
    /// no answer can name a file by a path of its own.
    #[test]
    fn no_other_file_of_the_server_makes_a_uri_from_a_path() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = vec![src.join("lsp.rs")];
        for e in std::fs::read_dir(src.join("lsp")).unwrap() {
            let path = e.unwrap().path();
            if path.extension().is_some_and(|x| x == "rs")
                && path.file_name().is_some_and(|n| n != "spelling.rs")
            {
                files.push(path);
            }
        }
        // `lsp.rs` and at least `calls`, `refs` and `symbols`: the walk found the directory.
        assert!(files.len() >= 4, "{files:?}");
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            assert!(!text.contains("from_file_path"), "{}", f.display());
        }
    }
}
