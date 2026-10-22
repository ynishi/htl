//! The run-time shape of each record and enum a file declares, lowered from what the
//! checker knows of the file (`htl shapes`).
//!
//! A Teal record says what a table is, statically; at run time the record is gone, and a
//! value that arrives from outside (a decoded body, a table a host returns, an event
//! payload typed `any`) can only enter a record through `as`, which checks nothing. The
//! library [lshape](https://github.com/ynishi/lshape) checks a table at run time against a
//! shape, a plain Lua table built from `T.shape({...})`. This module is the step between
//! the two: **the record is the source, and the shape is generated from it**, as data, into
//! files the project commits — the way `htl dts` writes declarations. Nothing here reads a
//! file or runs a process; the caller (the `htl shapes` subcommand) hands over what a check
//! produced ([`Source`]) and writes what comes back ([`Lowered`]).
//!
//! # What is written
//!
//! For each module `m` of the project that declares a record or an enum, three kinds of
//! text:
//!
//! - `<source>/shapes/m.lua`, a Lua module returning one shape per declaration, written
//!   against `require("lshape")` ([`Output::lua`]);
//! - `<types>/shapes/m.d.tl`, the declaration of that module, `Request:
//!   lshape.Schema<m.Request>` per shape, so a Teal caller gets the record back from the
//!   schema value ([`Output::dtl`]);
//! - `<types>/lshape.d.tl`, the declaration of lshape the generated shapes are written
//!   against ([`LSHAPE_DTL`]), once per project. It declares lshape 0.3's public surface,
//!   and says which lshape that is on its second line, `-- lshape: 0.3`, a Cargo
//!   requirement ([`declared_requirement`] reads it from any file that starts with
//!   [`HEADER`], a copy or one an older htl wrote included). The lshape the project
//!   supplies states its own version as a `_VERSION` field ([`runtime_version`] reads it
//!   from the source without running it), and [`lshape_version_finding`] is the verdict of
//!   the two, a lint; [`lshape_runtime`] picks which candidate of a resolution is that
//!   lshape. None of these opens a file: the caller reads the text.
//!   What holds shapes side by side (`T.shape`, the `registry` of a check; the comment on
//!   `AnySchema` in that file lists the rest) takes `lshape.AnySchema`, a supertype with no
//!   type argument, because Teal's generics are invariant and a `Schema<string>` is not a
//!   `Schema<any>`; every `Schema<T>` is an `AnySchema`.
//!
//! The boundary is then one call, and the casts after it go:
//!
//! ```text
//! local S = require("shapes.kernel")
//! local lshape = require("lshape")
//! local r = lshape.check.assert(v, S.Request)   -- v: any; r: kernel.Request
//! r.messages                                    -- {any}, no cast
//! ```
//!
//! # What is lowered
//!
//! In position order, every record (top level or nested) except the one the file returns,
//! and every enum. The record a file returns (`return M`) is the module itself, not a
//! type a value can have; its nested records are the module's types, and are lowered. A
//! record with only function fields is lowered like any other (a callbacks table a host
//! hands in is a thing to check). A declaration's shape is named by its path with the
//! returned record's name dropped: `M.Request` is `Request`, `M.Outer.Inner` is
//! `Outer_Inner`, and a top-level `Request` is `Request`; two declarations that lower to
//! one name are a problem naming both.
//!
//! | Teal | shape |
//! |---|---|
//! | `record R` | `T.shape({ ... }, { open = false })` |
//! | `record R` marked `---@extensible` | `{ open = true }` |
//! | field `f: X` | `f = <X>` |
//! | field marked `---@optional` | `<X>:is_optional()` |
//! | `record R where self.f == "v"` | the shape of `R` as above, with `f = T.literal("v")` in place of the type `f` is declared with (`f` stays a field; its type is `string` or an enum with the member `v`) |
//! | `string` `number` `integer` `boolean` `any` | `T.string` `T.number` `T.integer` `T.boolean` `T.any` |
//! | `nil`, as the whole type of a field | `T.any` |
//! | `thread` `userdata` | `T.any --[[ thread ]]` (lshape has no primitive; the comment names the type) |
//! | a function, a method | `T.fn` |
//! | `{X}` `{K : V}` `{A, B}` | `T.array_of(<X>)` `T.map_of(<K>, <V>)` `T.tuple({ <A>, <B> })` |
//! | `A \| B`, at most one of them a record | `T.any_of({ <A>, <B> })` |
//! | `A \| nil`, `A \| B \| nil` | the union of the members but nil, as above, and the field `:is_optional()` (inside a container, the element): an optional `A`, not "anything" |
//! | `enum E` | `T.one_of({ "a", "b" })`, the members sorted bytewise |
//! | a record or enum of this file | the sibling shape `M.<Name>` |
//! | a record or enum of another file the run lowers | `require("shapes.<module>").<Name>` |
//! | an alias of this file | what it names, by this table |
//!
//! A shape is written after the shapes it names (dependency order, ties by position). What
//! has no shape is a **problem** at the field, and a file with any problem is written in
//! neither of its two files, the way [`contract::publish`](crate::contract::publish) skips
//! a declaration it cannot produce — a placeholder `T.any` written for it would be a check
//! that silently passes:
//!
//! | | problem |
//! |---|---|
//! | `A \| B` with two or more records | a tag is what tells them apart (#512) |
//! | a type declared outside the project's sources (a dependency's record, a type htl's library declares, `local type M = require(..)`) | it has no shape |
//! | the record a module returns, an interface | a module is not a type a value has, and a value is checked against a record: each is said so, not called outside the project ([`RefEntry::NoShape`]) |
//! | a generic (`Box<integer>`), a type variable, an empty table, ... | it has no shape |
//! | `record R is I` | not lowered yet |
//! | a `where` clause that is not `self.f == "v"` (or `"v" == self.f`), a hand-written `metamethod __is` | it has no tag to read: the clause is quoted |
//! | `where self.f == "v"` and the body declares no `f` | the tag needs a field |
//! | `where self.f == "v"` and `f` is neither a `string` nor an enum with the member `v` | a tag is one of those |
//! | `where self.f == "v"` and `f` is optional (`---@optional`, `X \| nil`) | a tag is always there |
//! | a name two declarations lower to | rename one |
//! | an alias that refers back to itself | |
//! | a cycle of files, `shapes.a` needing `shapes.b` and back | a Lua `require` loop: [`module_cycles`] |
//!
//! # Known limitations
//!
//! - **In-file recursion is `T.table`.** A record whose field names the record itself (a
//!   linked list, a tree), or two records naming each other, would need a shape that refers
//!   to one not yet built. The field that closes the cycle is written `T.table --[[
//!   recursive: Name ]]` and a note is printed: the check there is fail-open (any table
//!   passes). lshape's `T.ref` is the later answer.
//! - **`is I` is refused.** The fields of `record R is I` are written in `I`, with their own
//!   markers, and the index does not list them under `R`. It is a problem at the record.
//! - **A `where` record is lowered alone, not as a member of a union.** Its tag field is the
//!   one value, but `A | B` of two records is still a problem (#512): nothing yet says which
//!   record a value is. The tag of an enum declared in another file is taken as written:
//!   its members are not known here, and Teal refuses a value that is not one of them.
//! - **A file that fails its check, a module named `shapes`, a `.d.tl`** are the caller's:
//!   see the subcommand.
//!
//! # Records in one module, the boundary in another
//!
//! The declaration of a module requires the module (`local type main = require("main")`),
//! so a file cannot name its own records through `shapes.main` while it is the file being
//! checked: Teal hands the requirer of a circular require a placeholder, and `main.Request`
//! is then another nominal than the `M.Request` the file declares. Measured, with
//! `src/main.tl` declaring `M.Request` and requiring `shapes.main` to assert:
//!
//! ```text
//! error: src/main.tl:10:28: argument 2: type parameter <T>: main.Request is not a M.Request
//! ```
//!
//! From another file the same assert checks clean. No form of the generated declaration
//! changes this, since naming the type means requiring the module that is still being
//! checked, and `htl shapes` adds no detection for it: the checker reports the real error at
//! the `require`. The rule is stated where it is read — the second line of every generated
//! `.d.tl` — and is this: **the records live in one module, and the boundary that asserts
//! them in another.** A `types/*.d.tl` or a `defs.tl` module is that shape already.
//!
//! The error shows where the file names its own type for the result (`local r: M.Request =
//! lshape.check.assert(v, S.Request)`); left to infer it, the result is typed
//! `main.Request`, which has the same fields and is not the type the file declares. The
//! escape hatch for a file that declares a record and must assert it itself is one cast
//! per record, taking the schema as the type the file declares:
//!
//! ```text
//! local r: M.Request = lshape.check.assert(v, S.Request as lshape.Schema<M.Request>)
//! ```
//!
//! `as` checks nothing, so the cast is the one place that says the shape and the record
//! are the same thing, and it stays true because both come from the one record.
//!
//! # Output text
//!
//! Deterministic: three-space indent, `\n`, one trailing newline, no timestamp, and no path
//! but the project-relative source path in the header. The first line of every generated
//! file starts with [`HEADER`]; a caller that is about to overwrite a file checks it, so
//! that a hand-written file of the same name is never touched.

use crate::CheckInfo;
use crate::Htl;
use crate::MarkerSite;
#[cfg(all(feature = "pkg", feature = "dts"))]
use crate::ModuleKind;
use crate::diagnostic::{Diagnostic, Severity};
#[cfg(all(feature = "pkg", feature = "dts"))]
use crate::resolve::{Candidate, Resolution};
use crate::typeindex::{Decl, DeclKind, Place, Prim, TypeExpr, TypeInfo, Where};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// What starts the first line of every file `htl shapes` writes. A caller that is about to
/// overwrite a file checks that its first line starts with this, and leaves it alone
/// otherwise.
pub const HEADER: &str = "-- Generated by htl shapes";

/// The declaration of lshape the generated shapes are written against: what
/// `<types>/lshape.d.tl` holds. Its first line starts with [`HEADER`].
pub const LSHAPE_DTL: &str = include_str!("../lua/lshape.d.tl");

/// The rule a note is said under.
const RULE: &str = "shapes";

/// The rule the findings about the lshape version are said under.
const VERSION_RULE: &str = "lshape-version";

/// What a declaration's `-- lshape:` line can get wrong: the text after the colon is not a
/// Cargo version requirement.
///
/// Kept apart from the line number, which [`declared_requirement`] returns beside the
/// requirement and cannot return beside an error without making the error a pair; the
/// caller that reports it ([`lshape_requirement_finding`]) already has both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReqError {
    /// What followed `lshape:` on the line, trimmed: the text that is not a requirement.
    pub text: String,
    /// Why `semver` refused it, in its words.
    pub reason: String,
}

impl std::fmt::Display for ReqError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is not a version requirement: {}",
            self.text, self.reason
        )
    }
}

impl std::error::Error for ReqError {}

/// The lshape requirement a declaration states, and the line it states it on.
///
/// The requirement is written in the declaration itself, `-- lshape: 0.3` among the
/// comment lines that open the file, in Cargo's grammar (`0.3` is `>=0.3.0, <0.4.0`; the
/// grammar of `[toolchain] htl`). It is read from the text the project has, not from
/// [`LSHAPE_DTL`], so a copy of the declaration, or one an older htl wrote, says which
/// lshape it was written for and not which one this binary would write today.
///
/// - `None`: the file is not one `htl shapes` writes (its first line does not start with
///   [`HEADER`]; a hand-written declaration of lshape is not this module's to judge), or it
///   is one and has no `-- lshape:` line among its leading comment lines (written before
///   the line existed). Neither is checked.
/// - `Some(Err(_))`: the line is there and what follows it is not a requirement.
/// - `Some(Ok((req, line)))`: the requirement, and its 1-based line, which is where a
///   finding about it points.
///
/// "Among the leading comment lines" is the run of lines that start with `--`, from the
/// first line to the first line that does not; the first `lshape:` line in it wins. Text
/// only, so that this is a function of what the caller read and the caller decides whether
/// and when to read.
pub fn declared_requirement(text: &str) -> Option<Result<(semver::VersionReq, usize), ReqError>> {
    let mut lines = text.lines();
    if !lines.next()?.starts_with(HEADER) {
        return None;
    }
    for (i, line) in lines.enumerate() {
        let comment = line.strip_prefix("--")?;
        let Some(value) = comment.trim_start().strip_prefix("lshape:") else {
            continue;
        };
        let value = value.trim();
        return Some(match semver::VersionReq::parse(value) {
            Ok(req) => Ok((req, i + 2)),
            Err(e) => Err(ReqError {
                text: value.to_string(),
                reason: e.to_string(),
            }),
        });
    }
    None
}

/// The candidate of a resolution of `lshape` that is the library itself: the first, in the
/// order the searchers consult them, that is not a declaration.
///
/// A declaration (`types/lshape.d.tl`, which `htl shapes` writes) is what the checker
/// reads for `require("lshape")` once it exists, so the resolution's winner is usually the
/// declaration, and says nothing about whether a module is there to load. The library is
/// the first candidate behind it that is an implementation, Teal or Lua. `None` is a
/// project with no lshape on its path: `htl shapes` refuses to write shapes against it,
/// and a version check has nothing to read.
///
/// One definition, because both `htl shapes` and the version check ask the question, and
/// two answers could disagree.
///
/// On the project layer's features (`pkg` and `dts`) like [`crate::resolve`], whose
/// answer this reads; the rest of this module does not need them.
#[cfg(all(feature = "pkg", feature = "dts"))]
pub fn lshape_runtime(res: &Resolution) -> Option<&Candidate> {
    res.candidates
        .iter()
        .find(|c| c.kind != ModuleKind::Declaration)
}

/// What a module's source says its version is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeVersion {
    /// A `_VERSION` field assigned a string literal that is a version.
    Found(semver::Version),
    /// The source parses and assigns no `_VERSION` field.
    Missing,
    /// The source does not parse (as Teal for a `.tl` chunk, as Lua otherwise); the
    /// parser's first error, `line:col: message`. Kept apart from [`Missing`](Self::Missing)
    /// because the fix is in a different place: the file, not its version field.
    Unparsable(String),
    /// The field is there and is not a string literal (`_VERSION = make()`): the version is
    /// only known by running the module, which a check does not do.
    NotLiteral,
    /// The field is a string literal that is not a `major.minor.patch` version; the text.
    NotSemver(String),
}

/// The version a module states about itself, read from its source without running it.
///
/// `src` is the module's text and `chunk` names it: a name ending in `.tl` is parsed as
/// Teal, any other as Lua. `h` is the checker whose Teal parser reads it, which is why
/// this is not a text search: the parser is in the Lua state, and it is what tells a
/// `_VERSION` that is assigned from one in a comment or a string. `runtime_version.lua`
/// says which two forms count (`M._VERSION = "0.3.0"` and a `_VERSION = "0.3.0"` field of
/// a table constructor) and which wins when there are several.
///
/// `Err` is the Lua state failing, not the module saying something unreadable; that is a
/// [`RuntimeVersion`] other than `Found`.
pub fn runtime_version(h: &Htl, src: &str, chunk: &str) -> anyhow::Result<RuntimeVersion> {
    let (kind, text) = h.version_field(src, chunk)?;
    Ok(match (kind.as_str(), text) {
        ("found", Some(text)) => match semver::Version::parse(&text) {
            Ok(v) => RuntimeVersion::Found(v),
            Err(_) => RuntimeVersion::NotSemver(text),
        },
        ("not_literal", _) => RuntimeVersion::NotLiteral,
        ("unparsable", text) => RuntimeVersion::Unparsable(
            text.unwrap_or_else(|| "the parser gave no error".to_string()),
        ),
        ("missing", _) => RuntimeVersion::Missing,
        (other, _) => anyhow::bail!("runtime_version.lua answered an unknown kind `{other}`"),
    })
}

/// The verdict on a declaration's requirement against the runtime it is meant for: `None`
/// when they agree, the lint `lshape-version` when they do not or when the runtime does
/// not say.
///
/// The finding is at `decl_path:req_line`, the requirement's own line, because that is the
/// line a person edits or regenerates. The paths are spelled as the caller passes them and
/// are not canonicalized here.
///
/// - `Found(v)`: compared by Cargo's rule on `req`, after dropping `v`'s pre-release and
///   build parts. `0.3.1-dev` is not in `^0.3` by Cargo's rule (a pre-release matches only
///   a requirement that names one), but a runtime in development of 0.3.1 is a 0.3
///   runtime, and the finding is about which line of releases the two share. On 0.x a
///   minor is the unit of compatibility, so a runtime newer by a minor is reported as well
///   as an older one. The message names both versions and the two ways out: move the
///   runtime to match the declaration, or regenerate the declaration with an htl that
///   declares the runtime's version.
/// - anything else: the runtime's version could not be read, and the message names
///   `runtime_path` and says why. A runtime that does not say is not a pass; the check
///   would otherwise stop checking the day lshape changes how it writes its version.
pub fn lshape_version_finding(
    req: &semver::VersionReq,
    req_line: usize,
    decl_path: &Path,
    runtime_path: &Path,
    rv: &RuntimeVersion,
) -> Option<Diagnostic> {
    let message = match rv {
        RuntimeVersion::Found(v) => {
            let release = semver::Version::new(v.major, v.minor, v.patch);
            if req.matches(&release) {
                return None;
            }
            format!(
                "this declaration is written for lshape {req}, but the lshape at {} is version {v}. \
                 Either move that lshape to a version that satisfies {req}, or regenerate this \
                 declaration with an htl that declares lshape {v}",
                runtime_path.display()
            )
        }
        RuntimeVersion::Missing => unreadable(
            req,
            runtime_path,
            "it assigns no `_VERSION` field (`M._VERSION = \"x.y.z\"`) that can be read",
        ),
        RuntimeVersion::Unparsable(err) => unreadable(
            req,
            runtime_path,
            &format!("it does not parse ({err}), so no `_VERSION` can be read from it"),
        ),
        RuntimeVersion::NotLiteral => unreadable(
            req,
            runtime_path,
            "its `_VERSION` is not a string literal, so it is only known by running it",
        ),
        RuntimeVersion::NotSemver(text) => unreadable(
            req,
            runtime_path,
            &format!("its `_VERSION` is \"{text}\", which is not a version (major.minor.patch)"),
        ),
    };
    Some(Diagnostic::new(
        Severity::Lint,
        decl_path.display().to_string(),
        req_line,
        1,
        message,
        Some(VERSION_RULE),
    ))
}

/// The sentence of [`lshape_version_finding`] for a runtime whose version cannot be read.
fn unreadable(req: &semver::VersionReq, runtime_path: &Path, why: &str) -> String {
    format!(
        "could not read the version of lshape at {}: {why}, so it cannot be compared with the \
         lshape {req} this declaration is written for",
        runtime_path.display()
    )
}

/// The finding for a `-- lshape:` line whose text is not a requirement
/// ([`declared_requirement`]'s `Some(Err(_))`): the same lint as
/// [`lshape_version_finding`], at the same line, because a requirement that cannot be read
/// leaves the declaration unchecked the way an unreadable runtime does, and the person who
/// sees one wants the other's switch (`[lint]`). Separate from it because it has no runtime
/// to name and no version to compare.
pub fn lshape_requirement_finding(decl_path: &Path, line: usize, err: &ReqError) -> Diagnostic {
    Diagnostic::new(
        Severity::Lint,
        decl_path.display().to_string(),
        line,
        1,
        format!(
            "{err}; the `-- lshape:` line says which lshape this declaration is written for, \
             as a Cargo requirement (`-- lshape: 0.3`), and nothing can be checked against it"
        ),
        Some(VERSION_RULE),
    )
}

/// One file to lower.
#[derive(Debug, Clone, Copy)]
pub struct Source<'a> {
    /// The module name the file answers to (`Project::locate(file).name`): `main`,
    /// `knl.foo`.
    pub module: &'a str,
    /// The file, relative to the project root, for the generated header and the
    /// diagnostics: `src/main.tl`.
    pub rel_path: &'a Path,
    /// The check of the file with the index kept (`Htl::check_indexed`).
    pub info: &'a CheckInfo,
}

/// Where a nominal type's declaration is, to the shape that stands for it: built by the
/// caller from the declaration trees of every file it lowers, before it lowers any (two
/// passes), so the lowering of one file does not read another.
///
/// Keyed on the typedecl's own place (`TypeInfo::Record::at` / `Enum::at`), which is what a
/// nominal's `declared` carries — not on [`Decl::line`] / [`Decl::col`], which for a
/// top-level record is its `local` token. The file of a place is spelled as the checker
/// spelled it, which differs between the file checked and the same file reached through a
/// `require`, so a table is made with the function that canonicalises it (the module reads
/// no file, so the caller brings that), applied before inserting and before looking up.
#[derive(Clone)]
pub struct RefTable {
    canon: fn(&Path) -> PathBuf,
    map: HashMap<(PathBuf, usize, usize), RefEntry>,
}

/// What the table says of a declaration of the project: the shape it lowers to, or why it
/// has none. A declaration the table has no entry for is outside the project's sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefEntry {
    /// A record or enum the run lowers.
    Shape(ShapeRef),
    /// A declaration of the project that is never lowered.
    NoShape(NoShapeReason),
}

/// Why a declaration of the project has no shape: what a problem at a field that names it
/// says, in place of "declared outside the project's sources", which would not be true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoShapeReason {
    /// The record a module returns (`return M`), whose module is the one named: it is the
    /// module and not a type a value can have, so it is never a candidate.
    Returned(String),
    /// An interface: a value is checked against a record, and an interface is never lowered.
    Interface,
}

/// The shape a declaration lowers to, as another file refers to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeRef {
    /// The module the declaration is in: `shapes.<module>` is where its shape is.
    pub module: String,
    /// The shape's name in that module (`Request`, `Outer_Inner`).
    pub name: String,
    /// Whether it is a record (and not an enum): what decides that `A | B` is a union of
    /// records, which has no shape, and not a union of values.
    pub record: bool,
}

impl Default for RefTable {
    /// A table that compares file names as they are spelled.
    fn default() -> Self {
        Self::new(|p| p.to_path_buf())
    }
}

impl std::fmt::Debug for RefTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.map.iter()).finish()
    }
}

impl RefTable {
    /// An empty table whose keys are made with `canon` (the file of a place, canonicalised).
    pub fn new(canon: fn(&Path) -> PathBuf) -> Self {
        Self {
            canon,
            map: HashMap::new(),
        }
    }

    fn key(&self, p: &Place) -> (PathBuf, usize, usize) {
        ((self.canon)(&p.file), p.line, p.col)
    }

    /// Add what is known of the declaration at `place`: its shape, or why it has none.
    pub fn insert(&mut self, place: &Place, entry: RefEntry) {
        let key = self.key(place);
        self.map.insert(key, entry);
    }

    /// Add every entry [`declared_shapes`] made.
    pub fn extend(&mut self, entries: impl IntoIterator<Item = (Place, RefEntry)>) {
        for (place, entry) in entries {
            self.insert(&place, entry);
        }
    }

    /// What is known of the declaration at `place`, when an entry was inserted.
    pub fn get(&self, place: &Place) -> Option<&RefEntry> {
        self.map.get(&self.key(place))
    }
}

/// The two files one module lowers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// `<source>/shapes/<module>.lua`.
    pub lua: String,
    /// `<types>/shapes/<module>.d.tl`.
    pub dtl: String,
}

/// One of three outcomes, so "written with problems" is not a state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lowered {
    /// Both files, and what is printed but does not fail: the recursive field lowered as
    /// `T.table`, the record that has a shape but no declaration entry.
    Written {
        /// The text of the two files.
        output: Output,
        /// What is said and does not fail.
        notes: Vec<Diagnostic>,
        /// The other modules whose shapes this one requires, sorted: the edges
        /// [`module_cycles`] reads.
        requires: Vec<String>,
    },
    /// Nothing written: one problem per site, and the notes gathered before the first.
    Refused {
        /// What has no shape, one per site, in position order.
        problems: Vec<Diagnostic>,
        /// What is said and does not fail.
        notes: Vec<Diagnostic>,
    },
    /// The file declares nothing to lower.
    Nothing,
}

/// What kind of declaration a [`Cand`] is.
enum CandKind<'a> {
    /// A record; its fields are `Cand::decl`'s children.
    Record,
    /// An enum, with its members (sorted bytewise).
    Enum { members: &'a [String] },
}

/// A declaration the lowering writes a shape for.
struct Cand<'a> {
    decl: &'a Decl,
    /// The declaration's path as the markers spell it: `M.Outer.Inner`.
    qual: String,
    /// The shape's name: `Outer_Inner`.
    name: String,
    /// Whether the returned record contains it, so its type is nameable from outside.
    reachable: bool,
    at: &'a Place,
    kind: CandKind<'a>,
}

impl Cand<'_> {
    fn is_record(&self) -> bool {
        matches!(self.kind, CandKind::Record)
    }
}

/// The declarations to lower, in position order, with the shape name of each.
fn candidates(decls: &[Decl]) -> Vec<Cand<'_>> {
    fn walk<'a>(
        decls: &'a [Decl],
        qual: &mut Vec<&'a str>,
        shape: &mut Vec<&'a str>,
        reachable: bool,
        out: &mut Vec<Cand<'a>>,
    ) {
        for d in decls {
            match (&d.kind, &d.info) {
                (DeclKind::Record, Some(TypeInfo::Record { returned, at, .. })) => {
                    qual.push(&d.name);
                    if *returned {
                        walk(&d.children, qual, shape, true, out);
                    } else {
                        shape.push(&d.name);
                        out.push(Cand {
                            decl: d,
                            qual: qual.join("."),
                            name: shape.join("_"),
                            reachable,
                            at,
                            kind: CandKind::Record,
                        });
                        walk(&d.children, qual, shape, reachable, out);
                        shape.pop();
                    }
                    qual.pop();
                }
                (DeclKind::Enum, Some(TypeInfo::Enum { members, at })) => {
                    qual.push(&d.name);
                    shape.push(&d.name);
                    out.push(Cand {
                        decl: d,
                        qual: qual.join("."),
                        name: shape.join("_"),
                        reachable,
                        at,
                        kind: CandKind::Enum { members },
                    });
                    shape.pop();
                    qual.pop();
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(decls, &mut Vec::new(), &mut Vec::new(), false, &mut out);
    out
}

/// The entries one file contributes to the [`RefTable`]: the place of each declaration it
/// lowers and the shape it lowers to, and the place of each declaration it has that is never
/// lowered — the record the module returns and every interface, top level or nested — with
/// the reason, so that a field naming one is told so and not told it is outside the project.
pub fn declared_shapes(module: &str, decls: &[Decl]) -> Vec<(Place, RefEntry)> {
    fn no_shape(module: &str, decls: &[Decl], out: &mut Vec<(Place, RefEntry)>) {
        for d in decls {
            match &d.info {
                Some(TypeInfo::Record {
                    returned: true, at, ..
                }) => out.push((
                    at.clone(),
                    RefEntry::NoShape(NoShapeReason::Returned(module.to_string())),
                )),
                Some(TypeInfo::Interface { at }) => {
                    out.push((at.clone(), RefEntry::NoShape(NoShapeReason::Interface)));
                }
                _ => {}
            }
            no_shape(module, &d.children, out);
        }
    }
    let mut out: Vec<(Place, RefEntry)> = candidates(decls)
        .into_iter()
        .map(|c| {
            (
                c.at.clone(),
                RefEntry::Shape(ShapeRef {
                    module: module.to_string(),
                    name: c.name.clone(),
                    record: c.is_record(),
                }),
            )
        })
        .collect();
    no_shape(module, decls, &mut out);
    out
}

/// A type with the shapes it is made of, before the order the shapes are written in is
/// known (a reference to a sibling is an index until then).
enum Ex {
    Raw(String),
    Sibling(usize),
    Other {
        module: String,
        name: String,
    },
    Array(Box<Ex>),
    Map(Box<Ex>, Box<Ex>),
    Tuple(Vec<Ex>),
    AnyOf(Vec<Ex>),
    /// `T | nil`: the shape of `T`, and nil allowed. At the top of a field the field is
    /// optional instead; this is what stays when the union is inside a container.
    Optional(Box<Ex>),
    /// The one value a tag field can have (`where self.status == "ok"`): `T.literal(..)`, the
    /// member text as an enum keeps it ([`lua_member`]).
    Literal(String),
}

impl Ex {
    fn siblings(&self, out: &mut Vec<usize>) {
        match self {
            Ex::Sibling(i) => out.push(*i),
            Ex::Array(e) | Ex::Optional(e) => e.siblings(out),
            Ex::Map(k, v) => {
                k.siblings(out);
                v.siblings(out);
            }
            Ex::Tuple(es) | Ex::AnyOf(es) => es.iter().for_each(|e| e.siblings(out)),
            Ex::Raw(_) | Ex::Other { .. } | Ex::Literal(_) => {}
        }
    }
}

struct Field {
    name: String,
    ex: Ex,
    optional: bool,
    line: usize,
    col: usize,
    site: String,
}

enum Body {
    Record { open: bool, fields: Vec<Field> },
    Enum(Vec<String>),
}

type Key = (PathBuf, usize, usize);

struct Lowering<'a> {
    src: &'a Source<'a>,
    refs: &'a RefTable,
    rel: String,
    cands: Vec<Cand<'a>>,
    own: HashMap<Key, usize>,
    returned: Option<Key>,
    /// The aliases of the file, by name.
    aliases: Vec<&'a Decl>,
    problems: Vec<Diagnostic>,
    notes: Vec<Diagnostic>,
}

/// Lower the records and enums of one checked file.
///
/// `refs` holds the declarations of every file the run lowers, this one included (a
/// reference to a sibling is found by the place of its declaration, whichever table it is
/// found in). The result is [`Lowered::Nothing`] for a file that declares nothing to lower
/// or whose check kept no index.
pub fn lower(src: &Source, refs: &RefTable) -> Lowered {
    let Some(decls) = src.info.types.declarations() else {
        return Lowered::Nothing;
    };
    let cands = candidates(decls);
    if cands.is_empty() {
        return Lowered::Nothing;
    }
    let mut aliases = Vec::new();
    let mut returned = None;
    collect_aliases(decls, &mut aliases, &mut returned, refs);
    let own = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (refs.key(c.at), i))
        .collect();
    let mut l = Lowering {
        src,
        refs,
        rel: rel_display(src.rel_path),
        cands,
        own,
        returned,
        aliases,
        problems: Vec::new(),
        notes: Vec::new(),
    };
    l.run()
}

fn collect_aliases<'a>(
    decls: &'a [Decl],
    aliases: &mut Vec<&'a Decl>,
    returned: &mut Option<Key>,
    refs: &RefTable,
) {
    for d in decls {
        match (&d.kind, &d.info) {
            (DeclKind::Alias, Some(TypeInfo::Alias { .. })) => aliases.push(d),
            (
                DeclKind::Record,
                Some(TypeInfo::Record {
                    returned: true, at, ..
                }),
            ) => {
                *returned = Some(refs.key(at));
            }
            _ => {}
        }
        collect_aliases(&d.children, aliases, returned, refs);
    }
}

fn rel_display(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `s` is a Lua name that can stand before `=` in a table constructor.
fn is_lua_name(s: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if",
        "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
    ];
    let mut cs = s.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !RESERVED.contains(&s)
}

/// An enum member as a Lua string literal. Teal keeps a member as the text between the
/// quotes of the token, escapes unprocessed (`"say \"hi\""` is the member `say \"hi\"`), so
/// the text already is the body of a Lua literal and is written as it stands, with only a
/// bare `"` (from a member written in single quotes) escaped.
fn lua_member(raw: &str) -> String {
    let mut out = String::from("\"");
    let mut cs = raw.chars();
    while let Some(c) = cs.next() {
        match c {
            '\\' => {
                out.push(c);
                out.extend(cs.next());
            }
            '"' => out.push_str("\\\""),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A field name as a Lua string literal, for the key that is not a name.
fn lua_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The identifier a module is named by in a generated declaration: `knl.foo` is `knl_foo`.
fn ident(module: &str) -> String {
    let id: String = module
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if id.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{id}")
    } else {
        id
    }
}

impl<'a> Lowering<'a> {
    fn problem(&mut self, line: usize, col: usize, message: String) {
        self.problems.push(Diagnostic::new(
            Severity::Error,
            self.rel.clone(),
            line,
            col,
            message,
            None,
        ));
    }

    fn note(&mut self, line: usize, col: usize, message: String) {
        self.notes.push(Diagnostic::new(
            Severity::Warning,
            self.rel.clone(),
            line,
            col,
            message,
            Some(RULE),
        ));
    }

    fn run(&mut self) -> Lowered {
        // Two declarations to one shape name.
        let mut by_name: HashMap<&str, usize> = HashMap::new();
        let mut collisions = Vec::new();
        for (i, c) in self.cands.iter().enumerate() {
            if let Some(&first) = by_name.get(c.name.as_str()) {
                collisions.push((first, i));
            } else {
                by_name.insert(&c.name, i);
            }
        }
        for (first, second) in collisions {
            let (f, s) = (&self.cands[first], &self.cands[second]);
            let message = format!(
                "`{}` (line {}) and `{}` (line {}) both lower to the shape `{}`; rename one",
                f.qual, f.decl.name_line, s.qual, s.decl.name_line, s.name
            );
            let (line, col) = (s.decl.name_line, s.decl.name_col);
            self.problem(line, col, message);
        }

        let mut bodies = Vec::with_capacity(self.cands.len());
        for i in 0..self.cands.len() {
            let body = self.body(i);
            bodies.push(body);
        }

        // Declarations (records and enums) that have a shape and cannot be named from outside.
        for i in 0..self.cands.len() {
            let c = &self.cands[i];
            if c.reachable {
                continue;
            }
            let message = format!(
                "`{}` has a shape but no declaration entry: it is not reachable from what `{}` returns",
                c.qual, self.rel
            );
            let (line, col) = (c.decl.name_line, c.decl.name_col);
            self.note(line, col, message);
        }

        if !self.problems.is_empty() {
            let mut problems = std::mem::take(&mut self.problems);
            problems.sort_by_key(|d| (d.line, d.col));
            return Lowered::Refused {
                problems,
                notes: std::mem::take(&mut self.notes),
            };
        }
        let (output, requires) = self.render(&bodies);
        Lowered::Written {
            output,
            notes: std::mem::take(&mut self.notes),
            requires,
        }
    }

    /// The marker `marker` on the declaration `name` of kind `kind`, when the census has
    /// one — and says so at a line the declaration is on.
    fn marked(&mut self, marker: &str, kind: &str, d: &Decl, name: &str) -> bool {
        let sites: &[MarkerSite] = &self.src.info.markers;
        let Some(site) = sites
            .iter()
            .find(|s| s.marker == marker && s.kind == kind && s.name == name)
        else {
            return false;
        };
        if site.line == d.line || site.line == d.name_line {
            return true;
        }
        let message = format!(
            "`{name}`: the marker census puts `---@{marker}` on line {} and the declaration is on line {}",
            site.line, d.name_line
        );
        self.problem(d.name_line, d.name_col, message);
        false
    }

    /// The lowering of candidate `i`.
    fn body(&mut self, i: usize) -> Body {
        let (d, qual) = (self.cands[i].decl, self.cands[i].qual.clone());
        if let CandKind::Enum { members } = self.cands[i].kind {
            return Body::Enum(members.to_vec());
        }
        // What the header says is the checker's: the index flags `is I` and `where`.
        let mut tag: Option<(&str, &str)> = None;
        if let Some(TypeInfo::Record {
            interfaces,
            where_clause,
            ..
        }) = &d.info
        {
            if *interfaces {
                let message =
                    format!("`{qual}` takes fields from an interface (`is`); not lowered yet");
                self.problem(d.name_line, d.name_col, message);
            }
            match where_clause {
                Some(Where::Tag { field, value }) => tag = Some((field, value)),
                Some(Where::Other(text)) => {
                    let message = format!(
                        "`{qual}` has a `where` clause that is not `self.<field> == \"<tag>\"`, so it has no tag to read: `{text}`; not lowered"
                    );
                    self.problem(d.name_line, d.name_col, message);
                }
                None => {}
            }
        }
        let open = self.marked("extensible", "record", d, &qual);
        // In the order the body declares them: a function the file defines below its record
        // stands where the definition is, and `body` is where the record wrote its type.
        let mut children: Vec<&Decl> = d.children.iter().collect();
        children.sort_by_key(|f| f.body.unwrap_or((f.line, f.col)));
        let mut fields = Vec::new();
        // Where the tag field is among `fields`, and whether its type is a record.
        let mut tag_at: Option<(usize, bool)> = None;
        for f in children {
            let Some(TypeInfo::Field { ty }) = &f.info else {
                continue;
            };
            if !matches!(
                f.kind,
                DeclKind::Field | DeclKind::Function | DeclKind::Method
            ) {
                continue;
            }
            let site = format!("{qual}.{}", f.name);
            let optional = self.marked("optional", "field", f, &site);
            let mut trail = Vec::new();
            let Some((ex, is_record)) = self.lower_ty(ty, f, &site, &mut trail) else {
                continue;
            };
            // A field whose type is `T | nil` is an optional field of `T`.
            let (ex, optional) = match ex {
                Ex::Optional(inner) => (*inner, true),
                ex => (ex, optional),
            };
            if tag.is_some_and(|(field, _)| field == f.name) {
                tag_at = Some((fields.len(), is_record));
            }
            fields.push(Field {
                name: f.name.clone(),
                ex,
                optional,
                line: f.line,
                col: f.col,
                site,
            });
        }
        if let Some((field, value)) = tag {
            self.tag(d, &qual, field, value, tag_at, &mut fields);
        }
        Body::Record { open, fields }
    }

    /// The tag field of a record with `where self.<field> == "<value>"`, whose shape is the
    /// one value (`T.literal`) in place of the type it was declared with. A problem, said at
    /// the record, when the clause cannot be read as a tag of this record: the body declares
    /// no such field, its type is not a `string` or an enum with the member, or it is
    /// optional (a value with no tag is not the record the clause selects).
    ///
    /// `at` is the field's place in `fields` and whether its type is a record; `None` when it
    /// was not lowered: the body does not declare it, it is a method or a function with no
    /// field type to read, or a problem was already said at the field.
    fn tag(
        &mut self,
        d: &Decl,
        qual: &str,
        field: &str,
        value: &str,
        at: Option<(usize, bool)>,
        fields: &mut [Field],
    ) {
        let child = d.children.iter().find(|c| {
            c.name == field
                && matches!(
                    c.kind,
                    DeclKind::Field | DeclKind::Function | DeclKind::Method
                )
        });
        let Some((i, is_record)) = at else {
            let message = match child {
                // Lowered with a problem already said at the field.
                Some(c) if matches!(c.info, Some(TypeInfo::Field { .. })) => return,
                // A method, or a function written outside the body: no type to read.
                Some(_) => format!(
                    "`{qual}` has a `where` clause on `{field}`, which has no field type to read; a tag is a `string` or an enum with the member \"{value}\""
                ),
                None => format!(
                    "`{qual}` has a `where` clause on `{field}`, which the record's body does not declare; the tag needs a field"
                ),
            };
            self.problem(d.name_line, d.name_col, message);
            return;
        };
        let detail = child.and_then(|c| c.detail.as_deref()).unwrap_or("?");
        let f = &mut fields[i];
        let not_a_tag = || {
            format!(
                "`{qual}` has a `where` clause on `{field}`, which is `{detail}`; a tag is a `string` or an enum with the member \"{value}\""
            )
        };
        let message = if f.optional {
            Some(format!(
                "`{qual}` has a `where` clause on `{field}`, which is optional; a tag is always there"
            ))
        } else {
            match &f.ex {
                Ex::Raw(t) if t == "T.string" => None,
                // An enum of another file: its members are not known here, and Teal has
                // already refused a value that is not one of them (`is not a member of`).
                Ex::Other { .. } if !is_record => None,
                Ex::Sibling(k) => match self.cands[*k].kind {
                    CandKind::Enum { members } if members.iter().any(|m| m == value) => None,
                    CandKind::Enum { .. } => Some(format!(
                        "`{qual}` has a `where` clause on `{field}`, which is `{detail}`, an enum without the member \"{value}\""
                    )),
                    CandKind::Record => Some(not_a_tag()),
                },
                _ => Some(not_a_tag()),
            }
        };
        match message {
            Some(message) => self.problem(d.name_line, d.name_col, message),
            None => f.ex = Ex::Literal(value.to_string()),
        }
    }

    /// The shape of `ty`, and whether it is a record (what a union counts). `None` once a
    /// problem has been said for it.
    fn lower_ty(
        &mut self,
        ty: &TypeExpr,
        at: &Decl,
        site: &str,
        aliases: &mut Vec<String>,
    ) -> Option<(Ex, bool)> {
        let raw = |s: &str| Some((Ex::Raw(s.to_string()), false));
        match ty {
            TypeExpr::Prim(p) => match p {
                Prim::String => raw("T.string"),
                Prim::Number => raw("T.number"),
                Prim::Integer => raw("T.integer"),
                Prim::Boolean => raw("T.boolean"),
                Prim::Any | Prim::Nil => raw("T.any"),
                Prim::Thread => raw("T.any --[[ thread ]]"),
                Prim::Userdata => raw("T.any --[[ userdata ]]"),
            },
            TypeExpr::Fn => raw("T.fn"),
            TypeExpr::Array(e) => {
                let (e, _) = self.lower_ty(e, at, site, aliases)?;
                Some((Ex::Array(Box::new(e)), false))
            }
            TypeExpr::Map(k, v) => {
                let (k, _) = self.lower_ty(k, at, site, aliases)?;
                let (v, _) = self.lower_ty(v, at, site, aliases)?;
                Some((Ex::Map(Box::new(k), Box::new(v)), false))
            }
            TypeExpr::Tuple(es) => {
                let mut out = Vec::new();
                for e in es {
                    out.push(self.lower_ty(e, at, site, aliases)?.0);
                }
                Some((Ex::Tuple(out), false))
            }
            TypeExpr::Union(ms) => {
                let mut out = Vec::new();
                let mut records = Vec::new();
                // `T | nil` is an optional `T`: nil is not a shape of its own here.
                let mut nilable = false;
                for m in ms {
                    if matches!(m, TypeExpr::Prim(Prim::Nil)) {
                        nilable = true;
                        continue;
                    }
                    let (e, is_record) = self.lower_ty(m, at, site, aliases)?;
                    if is_record && let TypeExpr::Nominal { names, .. } = m {
                        records.push(names.join("."));
                    }
                    out.push(e);
                }
                if records.len() >= 2 {
                    let message = format!(
                        "`{site}`: `{}` is a union of records; a tag is what tells them apart (#512)",
                        records.join(" | ")
                    );
                    self.problem(at.line, at.col, message);
                    return None;
                }
                let ex = match out.len() {
                    // Nothing but nil: as a type of its own, `T.any`.
                    0 => return raw("T.any"),
                    1 => out.pop().expect("one member"),
                    _ => Ex::AnyOf(out),
                };
                Some((
                    if nilable {
                        Ex::Optional(Box::new(ex))
                    } else {
                        ex
                    },
                    false,
                ))
            }
            TypeExpr::Nominal { names, declared } => {
                self.nominal(names, declared.as_ref(), at, site, aliases)
            }
            TypeExpr::Other(text) => {
                let message = format!("`{site}`: `{text}` has no shape");
                self.problem(at.line, at.col, message);
                None
            }
        }
    }

    fn nominal(
        &mut self,
        names: &[String],
        declared: Option<&Place>,
        at: &Decl,
        site: &str,
        aliases: &mut Vec<String>,
    ) -> Option<(Ex, bool)> {
        let written = names.join(".");
        if let Some(p) = declared {
            let key = self.refs.key(p);
            if let Some(&i) = self.own.get(&key) {
                return Some((Ex::Sibling(i), self.cands[i].is_record()));
            }
            if self.returned.as_ref() == Some(&key) {
                let message = format!(
                    "`{site}`: `{written}` is the record this file returns; it has no shape"
                );
                self.problem(at.line, at.col, message);
                return None;
            }
            match self.refs.get(p) {
                Some(RefEntry::Shape(r)) => {
                    return Some((
                        Ex::Other {
                            module: r.module.clone(),
                            name: r.name.clone(),
                        },
                        r.record,
                    ));
                }
                Some(RefEntry::NoShape(reason)) => {
                    let message = match reason {
                        NoShapeReason::Returned(module) => format!(
                            "`{site}`: `{written}` is the record module `{module}` returns; a module has no shape"
                        ),
                        NoShapeReason::Interface => format!(
                            "`{site}`: `{written}` is an interface; a value is checked against a record, not an interface"
                        ),
                    };
                    self.problem(at.line, at.col, message);
                    return None;
                }
                None => {}
            }
            if let Some(alias) = self.alias_for(names, p) {
                if aliases.contains(&alias.name) {
                    let message = format!("`{site}`: the alias `{written}` refers back to itself");
                    self.problem(at.line, at.col, message);
                    return None;
                }
                let Some(TypeInfo::Alias { ty }) = &alias.info else {
                    return None;
                };
                aliases.push(alias.name.clone());
                let out = self.lower_ty(ty, at, site, aliases);
                aliases.pop();
                return out;
            }
        }
        let message = format!(
            "`{site}`: `{written}` is declared outside the project's sources and has no shape"
        );
        self.problem(at.line, at.col, message);
        None
    }

    /// The alias of this file a nominal named `names` resolved to at `declared`.
    fn alias_for(&self, names: &[String], declared: &Place) -> Option<&'a Decl> {
        let key = self.refs.key(declared);
        let this_file = self.cands.first().map(|c| self.refs.key(c.at).0);
        let in_file = this_file.as_ref() == Some(&key.0);
        self.aliases.iter().copied().find(|a| {
            let by_place = [(a.line, a.col), (a.name_line, a.name_col)]
                .iter()
                .any(|&(l, c)| in_file && (l, c) == (key.1, key.2));
            by_place || (in_file && names.len() == 1 && names[0] == a.name)
        })
    }

    /// The text of both files, and the modules the Lua requires.
    fn render(&mut self, bodies: &[Body]) -> (Output, Vec<String>) {
        // Dependency order: a shape after the shapes it names, ties by position.
        let n = self.cands.len();
        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, b) in bodies.iter().enumerate() {
            if let Body::Record { fields, .. } = b {
                for f in fields {
                    f.ex.siblings(&mut deps[i]);
                }
            }
        }
        let mut state = vec![0u8; n];
        let mut order = Vec::with_capacity(n);
        fn visit(i: usize, deps: &[Vec<usize>], state: &mut [u8], order: &mut Vec<usize>) {
            if state[i] != 0 {
                return;
            }
            state[i] = 1;
            for &d in &deps[i] {
                visit(d, deps, state, order);
            }
            state[i] = 2;
            order.push(i);
        }
        for i in 0..n {
            visit(i, &deps, &mut state, &mut order);
        }
        let mut pos = vec![0usize; n];
        for (p, &i) in order.iter().enumerate() {
            pos[i] = p;
        }

        let mut requires = BTreeSet::new();
        let mut lua = format!(
            "{HEADER} from {}. The record is the source; do not edit.\nlocal T = require(\"lshape\").t\nlocal M = {{}}\n",
            self.rel
        );
        for &i in &order {
            let name = self.cands[i].name.clone();
            lua.push('\n');
            match &bodies[i] {
                Body::Enum(members) => {
                    let list: Vec<String> = members.iter().map(|m| lua_member(m)).collect();
                    lua.push_str(&format!("M.{name} = T.one_of({{ {} }})\n", list.join(", ")));
                }
                Body::Record { open, fields } => {
                    if fields.is_empty() {
                        lua.push_str(&format!("M.{name} = T.shape({{}}, {{ open = {open} }})\n"));
                        continue;
                    }
                    lua.push_str(&format!("M.{name} = T.shape({{\n"));
                    for f in fields {
                        let mut text = self.expr(&f.ex, i, &pos, f, &mut requires);
                        if f.optional {
                            text.push_str(":is_optional()");
                        }
                        let key = if is_lua_name(&f.name) {
                            f.name.clone()
                        } else {
                            format!("[{}]", lua_string(&f.name))
                        };
                        lua.push_str(&format!("   {key} = {text},\n"));
                    }
                    lua.push_str(&format!("}}, {{ open = {open} }})\n"));
                }
            }
        }
        lua.push_str("\nreturn M\n");

        let dtl = self.declaration();
        (
            Output { lua, dtl },
            requires.into_iter().collect::<Vec<_>>(),
        )
    }

    /// The Lua text of `ex`, in the shape `cur`: a sibling written later is a cycle, cut
    /// there.
    fn expr(
        &mut self,
        ex: &Ex,
        cur: usize,
        pos: &[usize],
        field: &Field,
        requires: &mut BTreeSet<String>,
    ) -> String {
        match ex {
            Ex::Raw(s) => s.clone(),
            Ex::Sibling(i) => {
                if pos[*i] < pos[cur] {
                    format!("M.{}", self.cands[*i].name)
                } else {
                    let name = self.cands[*i].name.clone();
                    let message = format!(
                        "`{}`: `{name}` refers back to a shape it is part of; written as T.table (any table passes)",
                        field.site
                    );
                    self.note(field.line, field.col, message);
                    format!("T.table --[[ recursive: {name} ]]")
                }
            }
            Ex::Other { module, name } => {
                requires.insert(module.clone());
                format!("require(\"shapes.{module}\").{name}")
            }
            Ex::Literal(raw) => format!("T.literal({})", lua_member(raw)),
            Ex::Optional(e) => format!("{}:is_optional()", self.expr(e, cur, pos, field, requires)),
            Ex::Array(e) => format!("T.array_of({})", self.expr(e, cur, pos, field, requires)),
            Ex::Map(k, v) => format!(
                "T.map_of({}, {})",
                self.expr(k, cur, pos, field, requires),
                self.expr(v, cur, pos, field, requires)
            ),
            Ex::Tuple(es) => {
                let parts: Vec<String> = es
                    .iter()
                    .map(|e| self.expr(e, cur, pos, field, requires))
                    .collect();
                format!("T.tuple({{ {} }})", parts.join(", "))
            }
            Ex::AnyOf(es) => {
                let parts: Vec<String> = es
                    .iter()
                    .map(|e| self.expr(e, cur, pos, field, requires))
                    .collect();
                format!("T.any_of({{ {} }})", parts.join(", "))
            }
        }
    }

    /// The declaration of the module: an entry per shape of a record the returned record
    /// contains, in position order.
    fn declaration(&self) -> String {
        let module = self.src.module;
        let id = ident(module);
        let mut out = format!(
            "{HEADER} from {}. The record is the source; do not edit.\n-- Assert from another module: this file requires \"{module}\", so \"{module}\" cannot require it.\n",
            self.rel
        );
        let entries: Vec<&Cand> = self.cands.iter().filter(|c| c.reachable).collect();
        if !entries.is_empty() {
            out.push_str(&format!(
                "local lshape = require(\"lshape\")\nlocal type {id} = require(\"{module}\")\n"
            ));
        }
        out.push_str(&format!("local record shapes_{id}\n"));
        for c in entries {
            // The path from the returned record: `M.Outer.Inner` is `<module>.Outer.Inner`.
            let path = c.qual.split_once('.').map_or(c.qual.as_str(), |(_, p)| p);
            // Named as the Lua module names the shape (`Outer_Inner`), so `S.<name>` is the
            // value the module returns and not a nil the declaration promised.
            out.push_str(&format!("   {}: lshape.Schema<{id}.{path}>\n", c.name));
        }
        out.push_str(&format!("end\nreturn shapes_{id}\n"));
        out
    }
}

/// The groups of modules that require each other, directly or through others: each is a
/// loop of Lua `require`s at run time (`shapes.a` needs `shapes.b` and back), which the
/// generated files cannot be loaded through. `requires` is every written module and the
/// modules its shapes require ([`Lowered::Written::requires`]); a group is sorted, and so
/// is the list. A module that requires itself is not one (it never does).
pub fn module_cycles(requires: &[(String, Vec<String>)]) -> Vec<Vec<String>> {
    let index: HashMap<&str, usize> = requires
        .iter()
        .enumerate()
        .map(|(i, (m, _))| (m.as_str(), i))
        .collect();
    let edges: Vec<Vec<usize>> = requires
        .iter()
        .map(|(_, to)| {
            to.iter()
                .filter_map(|m| index.get(m.as_str()).copied())
                .collect()
        })
        .collect();
    // Tarjan's strongly connected components.
    struct Walk<'a> {
        edges: &'a [Vec<usize>],
        next: usize,
        number: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        groups: Vec<Vec<usize>>,
    }
    fn connect(w: &mut Walk, v: usize) {
        w.number[v] = Some(w.next);
        w.low[v] = w.next;
        w.next += 1;
        w.stack.push(v);
        w.on_stack[v] = true;
        for k in 0..w.edges[v].len() {
            let to = w.edges[v][k];
            match w.number[to] {
                None => {
                    connect(w, to);
                    w.low[v] = w.low[v].min(w.low[to]);
                }
                Some(n) if w.on_stack[to] => w.low[v] = w.low[v].min(n),
                Some(_) => {}
            }
        }
        if Some(w.low[v]) == w.number[v] {
            let mut group = Vec::new();
            while let Some(m) = w.stack.pop() {
                w.on_stack[m] = false;
                group.push(m);
                if m == v {
                    break;
                }
            }
            w.groups.push(group);
        }
    }
    let n = requires.len();
    let mut w = Walk {
        edges: &edges,
        next: 0,
        number: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        groups: Vec::new(),
    };
    for v in 0..n {
        if w.number[v].is_none() {
            connect(&mut w, v);
        }
    }
    let mut out: Vec<Vec<String>> = w
        .groups
        .into_iter()
        .filter(|g| g.len() > 1)
        .map(|g| {
            let mut names: Vec<String> = g.into_iter().map(|i| requires[i].0.clone()).collect();
            names.sort();
            names
        })
        .collect();
    out.sort();
    out
}
