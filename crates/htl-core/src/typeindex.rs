//! What the checker knows about each position of a file, kept for an editor's hover and
//! go-to-definition ([`TypeIndex`], [`CheckInfo::types`](crate::CheckInfo::types)).
//!
//! Teal's checker can report, for a file it has checked, the type of every expression by
//! the position the expression starts at, and the symbols in scope at each point (`tl.get_types`:
//! `by_pos`, `symbols_by_file`). The lints already read that report to ask "what type is
//! this"; this module is the other reader, the one that answers a person pointing at a
//! name. It is a copy of the part of the report an editor asks about, made when the check
//! ran ([`Htl::check_indexed`](crate::Htl::check_indexed)), so a question about a position is
//! answered from memory and no check is repeated for it.
//!
//! # What a question is
//!
//! [`TypeIndex::at`] takes the text the check read, a line and a byte column — where a
//! cursor is, anywhere in a name — and answers about the name the position is on:
//!
//! - **The type**, as Teal writes it (`Greeting`, `function(string): Greeting`): the
//!   `str` of the report's type, the same text an error message uses.
//! - **Where it is declared**, when the report says. Two sources, tried in this order for a
//!   name that is not a field (`a.b`, `a:b`) — a field is not in scope, so a local of the
//!   same name must not answer for it:
//!   1. the symbols in scope. The innermost declaration of the name that encloses the
//!      position is the one a reader means: a `local`, a parameter, a `for` variable, a
//!      local function or type. Position is where Teal records it: the name for a variable
//!      or parameter, the start of the statement for a function or a record.
//!   2. the type itself. A function's type carries where the function is written, so
//!      `m.hello` goes to `function M.hello` in the file `m` comes from. A name that *is* the
//!      name of its type (`Cfg` in `local c: m.Cfg`) goes to the declaration the type
//!      resolves to. A value of a record type is not answered this way: its type's position
//!      is where the type is declared, and that is not where the value is.
//!
//!   Everything else — a field of a primitive type, a global of the standard library (its
//!   declaration is in a file that is not on disk), a name the report has no entry for —
//!   has no declaration here, and the answer says so.
//!
//! # Where a position is
//!
//! The report is keyed by the position an expression starts at, which for most names is the
//! name and for `a.b` or `a:b` is the `.` or `:` (the index expression starts there: the
//! field's type is stored under the dot). So a cursor on `b` looks at the character before
//! it. A cursor in the middle of a name, or just after its last character, is on the name:
//! the name is found in the text and the question is asked of where it starts.
//!
//! A position in a comment or a string, on a keyword or on a number is not on a name. (A
//! string is not a name even when its text is one; the `require` string is the one place
//! an editor asks about a string, and [`RequireSite`](crate::RequireSite) answers it.)
//!
//! # What it cannot say
//!
//! - **`local x = x`.** Scope is found by position, as `tl.symbols_in_scope` finds it, and
//!   the new `x` is declared at a position before the `x` on the right of the `=`. The
//!   right-hand `x` is taken for the new one. (Teal's own tool has the same shortcoming.)
//! - **Another file's locals.** The report keeps symbols for the file checked; a name from a
//!   required module is reached through the type of the field (above).
//! - **A field of a primitive type** has no position of its own in the report: Teal shares
//!   one type for every `string`, so a `name: string` field has nowhere to point.
//!
//! # Declarations
//!
//! The index also keeps what the file declares, as a tree ([`TypeIndex::declarations`]): what
//! an editor's outline lists and a symbol search matches. It is read off the checker's syntax
//! tree for the structure and the positions, and off the report for the type strings, in one
//! walk over the file's top-level statements (their bodies are not entered), so it is the
//! file's own declarations and not the locals of a function.
//!
//! - **Top level**: every `local` / `global` variable (a [`DeclKind::Function`] if what it
//!   holds is a function), `local function` and `global function`, `local macroexp`, and
//!   every `record`, `interface`, `enum` and `type` alias (`local type M = require("m")`
//!   included), in the order written.
//! - **A record's children**: its fields, the records, enums and aliases nested in it, and
//!   the functions declared on it anywhere in the file — `function R.f` is a
//!   [`DeclKind::Function`] and `function R:m` a [`DeclKind::Method`], whatever the
//!   position, so a module record's functions, which are written after its `end`, are its
//!   children. They are in position order. A `function R.f` whose record is not declared
//!   in this file (a field of a record a `require` brought in) stands at the top level, named
//!   `R.f`, and has no type string. A field written in the body whose type is a function is a
//!   function too — and when the file also defines it (`f: function(integer): string` in
//!   the body, `function R.f(n: integer): string` below, the way a module declares its API
//!   in the record and defines it after), the two are **one** child: the definition's, named
//!   `f`, with the definition's range (`function` to `end`) and name, and the type. A body
//!   declaration with no definition in this file stays as the body wrote it, and so does a
//!   definition with no body declaration. A second definition of the name (an overload)
//!   is one more entry.
//!   A nested `record` is one more level of the same. The fields a `record R is I` takes
//!   from `I` are not listed under `R`: they are written in `I`.
//!
//! What is approximate:
//!
//! - **A range is the declaration's extent where the checker records one**: a function, and
//!   a `record`/`interface`/`enum` of the top level, run from their first token (`local`,
//!   `global`, `function`) to their `end`. A nested `record` / `enum` runs to its `end`, found
//!   by lexing the file once. A variable runs to the end of the last token of its initialiser
//!   that the tree positions (a table or a function's `end` included); a field, an alias,
//!   and anything the tree gives no end to, run from the name to the end of the line it is on.
//!   A multi-line field type or alias is therefore cut at its first line.
//! - **A field's name** is not in the report (a `name: string` has the position of
//!   `string`, if any), so it is found by looking for the name before the type on the line
//!   (or the two above it). A name written in a way that has no such text (a `["quoted key"]`)
//!   is given the position of its type.
//! - **An enum's members** are not listed: Teal keeps them as a set, without an order or a
//!   position.
//! - **A record function's range is not inside its record's.** The record ends where its
//!   body does, and `function R.f` is written after it; the function is still its child.
//!
//! The tree is `None` — not an empty one — when the checker kept no tree for the file: it did
//! not parse. A file that parses and declares nothing has `Some` of an empty list.
//!
//! # What it costs
//!
//! One pass over the report's positions at check time, with each distinct type copied once,
//! and the declaration walk above, which visits the top-level statements and the initialisers
//! of the variables among them; it makes a few dozen to a few hundred small tables for a
//! source file and reads the type string of each. `HTL_PROFILE=1` prints them as `type_index`
//! (the whole, the declaration walk included) and `decl_index` (the part that is the walk); it
//! is only done by [`Htl::check_indexed`](crate::Htl::check_indexed), which `htl check` does
//! not call.

use crate::CheckInfo;
use anyhow::Result;
use mlua::Table;
use std::collections::HashMap;
use std::path::PathBuf;

/// A place in a file: `line` counted from 1, `col` the byte column counted from 1 — the
/// convention of [`Diagnostic`](crate::Diagnostic).
///
/// `file` is spelled as the checker spelled it: the name the file was checked under for a
/// position in the checked file, a path from the search for one in another (relative to the
/// working directory, or absolute — and `./m.tl` for the file `m.tl` of the directory the
/// checker ran in). A file of Teal's standard library (`stdlib.d.tl`) is not on disk at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The file, spelled as above.
    pub file: PathBuf,
    /// Line, from 1.
    pub line: usize,
    /// Byte column, from 1.
    pub col: usize,
}

/// What [`TypeIndex::at`] says about the name a position is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAt {
    /// Line of the name, from 1.
    pub line: usize,
    /// Byte column the name starts at, from 1.
    pub col: usize,
    /// Byte column just past the name's last character, from 1.
    pub end: usize,
    /// Its type as Teal writes it; `None` when the report has none for this name.
    pub ty: Option<String>,
    /// Where it is declared, when the report says (see the module doc).
    pub declared: Option<Place>,
}

/// What a [`Decl`] is. Which Teal construct each is, and which positions they have, is in
/// the module doc (Declarations).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclKind {
    /// A `local` or `global` variable that does not hold a function.
    Local,
    /// A function: `local function`, `global function`, `function R.f`, a record field typed
    /// as a function, or a variable that holds one.
    Function,
    /// `function R:m`, or a record field typed as a method.
    Method,
    /// A field of a record that is not a function.
    Field,
    /// A `record` (`local`, `global` or nested).
    Record,
    /// An `interface`.
    Interface,
    /// An `enum`.
    Enum,
    /// A `type X = ...` that names another type.
    Alias,
}

/// One declaration of a file ([`TypeIndex::declarations`]). Lines are counted from 1 and
/// columns are byte columns from 1, as in [`Place`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    /// The name as written (`greet`); `R.f` for a record function whose record is declared
    /// elsewhere.
    pub name: String,
    /// What it is.
    pub kind: DeclKind,
    /// Its type as Teal writes it, when the report has one: a variable's, a function's, a
    /// field's, the type an alias names. Not set for a record, interface or enum, whose
    /// name is its type.
    pub detail: Option<String>,
    /// Line the declaration starts on: its first token (`local`, `global`, `function`), or
    /// the name for a field.
    pub line: usize,
    /// Byte column of that first token.
    pub col: usize,
    /// Line it ends on.
    pub end_line: usize,
    /// The byte column just past its last character, or `None` for "the end of `end_line`"
    /// (where the checker records no end, see the module doc).
    pub end_col: Option<usize>,
    /// Line of the name.
    pub name_line: usize,
    /// Byte column the name starts at; the name is `name.len()` bytes long (for a
    /// `R.f` that stands at the top level, the `f` is the part this points at).
    pub name_col: usize,
    /// Fields, nested types and functions, in position order; empty but for a record or
    /// interface.
    pub children: Vec<Decl>,
}

/// The positions of a checked file that an editor asks about. Empty ([`Default`]) for a
/// check that was not asked to keep it, or one whose file the checker could not read.
#[derive(Debug, Clone, Default)]
pub struct TypeIndex {
    /// The file the positions are in, as it was named to the checker.
    file: PathBuf,
    /// `(line, col, type id)` of every expression the report has a type for, sorted by
    /// position.
    at: Vec<(u32, u32, u32)>,
    types: HashMap<u32, Ty>,
    symbols: Vec<Symbol>,
    /// The file's declarations; `None` when the checker kept no tree for it.
    decls: Option<Vec<Decl>>,
}

#[derive(Debug, Clone)]
struct Ty {
    name: String,
    /// Where the type is written; for a function, where the function is declared.
    own: Option<Place>,
    /// Where the type's `ref` chain ends: the declaration a name of the type stands for.
    target: Option<Place>,
    function: bool,
}

/// One entry of Teal's flattened scope list (`symbols_by_file`): a declaration, or the
/// start (`@{`) or end (`@}`) of a scope.
#[derive(Debug, Clone)]
struct Symbol {
    line: u32,
    col: u32,
    name: String,
    /// A declaration's type id; for `@{` the 1-based index of its `@}`, and for `@}` the
    /// index of the entry before its `@{` — where a walk outward from inside continues.
    link: i64,
}

const OPEN: &str = "@{";
const CLOSE: &str = "@}";

impl TypeIndex {
    /// Whether there is nothing in it: the check was not asked to keep one, or the file did
    /// not parse.
    pub fn is_empty(&self) -> bool {
        self.at.is_empty() && self.symbols.is_empty() && self.decls.is_none()
    }

    /// What the file declares, as a tree (the module doc's Declarations says of what, and
    /// how approximate its ranges are), in the order written. `None` is "not known" — the
    /// check kept no index, or the file did not parse — and is not the same answer as `Some`
    /// of an empty slice, a file that declares nothing.
    pub fn declarations(&self) -> Option<&[Decl]> {
        self.decls.as_deref()
    }

    /// Read the table `type_index` (`prelude.lua`) builds.
    pub(crate) fn read(t: &Table) -> Result<Self> {
        let ints = |key: &str| -> Result<Vec<i64>> {
            Ok(t.get::<Table>(key)?
                .sequence_values::<i64>()
                .collect::<mlua::Result<_>>()?)
        };
        let at_ints = ints("at")?;
        let mut at: Vec<(u32, u32, u32)> = at_ints
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| (c[0] as u32, c[1] as u32, c[2] as u32))
            .collect();
        at.sort_unstable();
        let place =
            |file: Option<String>, line: Option<i64>, col: Option<i64>| match (file, line, col) {
                (Some(file), Some(line), Some(col)) if line > 0 => Some(Place {
                    file: PathBuf::from(file),
                    line: line as usize,
                    col: col.max(1) as usize,
                }),
                _ => None,
            };
        let mut types = HashMap::new();
        for row in t.get::<Table>("types")?.sequence_values::<Table>() {
            let row = row?;
            types.insert(
                row.get::<i64>("id")? as u32,
                Ty {
                    name: row.get::<Option<String>>("str")?.unwrap_or_default(),
                    own: place(row.get("file")?, row.get("y")?, row.get("x")?),
                    target: place(row.get("tfile")?, row.get("ty")?, row.get("tx")?),
                    function: row.get::<Option<bool>>("fn")?.unwrap_or(false),
                },
            );
        }
        let names: Vec<String> = t
            .get::<Table>("names")?
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()?;
        let sym_ints = ints("syms")?;
        let symbols = sym_ints
            .as_chunks::<3>()
            .0
            .iter()
            .zip(names)
            .map(|(c, name)| Symbol {
                line: c[0] as u32,
                col: c[1] as u32,
                name,
                link: c[2],
            })
            .collect();
        // The tree is an extra of an extra: one that cannot be read is no tree, and leaves
        // the rest of the index as it was.
        let decls = t
            .get::<Option<Table>>("decls")?
            .and_then(|list| read_decls(&list).ok());
        Ok(Self {
            file: PathBuf::from(t.get::<String>("file")?),
            at,
            types,
            symbols,
            decls,
        })
    }

    /// The name `text` has at (`line`, `col`) — a byte column, 1-based, which may be
    /// anywhere in the name or just after it — and what the report says of it. `text` is the
    /// text the check read; positions in any other text are not the report's. `None` when
    /// the position is not on a name or the report knows nothing about it.
    pub fn at(&self, text: &str, line: usize, col: usize) -> Option<TypeAt> {
        let lines: Vec<&str> = text.split('\n').collect();
        let row = lines.get(line.checked_sub(1)?)?;
        let word = Word::find(row, col.saturating_sub(1))?;
        if word.is_keyword() || word.is_number() {
            return None;
        }
        // Not in a comment or a string: the line's offset in the text, and the cursor's in it.
        let before: usize = lines[..line - 1].iter().map(|l| l.len() + 1).sum();
        if inert(text, before + word.start) {
            return None;
        }
        let field = word.field_dot(row);
        // The entry under the position the report keys the name by.
        let entry = |c: usize| {
            self.at
                .binary_search_by_key(&(line as u32, c as u32 + 1), |&(y, x, _)| (y, x))
                .ok()
                .and_then(|i| self.types.get(&self.at[i].2))
        };
        let keyed = match field {
            // A type written `m.Cfg` is stored whole, under the `m`: the name of a type is
            // found there, and only when it is the name the type has, so that `b` of `a.b`
            // is never answered with the type of `a`.
            Some(dot) => entry(dot)
                .or_else(|| entry(word.start))
                .or_else(|| entry(word.head(row)).filter(|t| t.base() == word.text)),
            None => entry(word.start),
        };
        let symbol = match field {
            Some(_) => None,
            None => self.symbol(word.text, line as u32, word.start as u32 + 1),
        };
        // `as` and `is` are Teal's operator words, not reserved ones: the cast or the test
        // they make is keyed under them, so they are a name only where one is declared so.
        if field.is_none() && symbol.is_none() && matches!(word.text, "as" | "is") {
            return None;
        }
        let symbol_ty = symbol.and_then(|s| self.types.get(&(s.link as u32)));
        // The head of a dotted name (`m` of `m.Cfg`): what is keyed under it may be the whole
        // of a type written there, or of a cast to one, and not `m`; the declaration in
        // scope says what `m` is.
        let ty = if field.is_none() && word.is_head(row) {
            symbol_ty.or(keyed)
        } else {
            keyed.or(symbol_ty)
        }
        .map(|t| t.name.clone());
        let declared = symbol
            .map(|s| Place {
                file: self.file.clone(),
                line: s.line as usize,
                col: s.col as usize,
            })
            .or_else(|| keyed.and_then(|t| t.declaration(word.text)));
        if ty.is_none() && declared.is_none() {
            return None;
        }
        Some(TypeAt {
            line,
            col: word.start + 1,
            end: word.start + word.text.len() + 1,
            ty,
            declared,
        })
    }

    /// The innermost declaration of `name` in scope at (`line`, `col`): Teal's
    /// `symbols_in_scope`, for one name. From the last entry at or before the position,
    /// walking back; a scope that has ended is jumped over whole.
    fn symbol(&self, name: &str, line: u32, col: u32) -> Option<&Symbol> {
        // The list is in the order Teal met the declarations, which is not always the order
        // of their positions (`self` is declared after the parameters that follow it), so
        // it is not searched by bisection.
        let mut n = self
            .symbols
            .iter()
            .rposition(|s| (s.line, s.col) <= (line, col))?
            + 1;
        while n >= 1 {
            let s = &self.symbols[n - 1];
            match s.name.as_str() {
                OPEN => n -= 1,
                CLOSE => n = usize::try_from(s.link).ok()?,
                _ if s.name == name => return Some(s),
                _ => n -= 1,
            }
        }
        None
    }
}

impl Decl {
    /// The part of the name that [`name_col`](Decl::name_col) points at: `f` for the `R.f` of
    /// a record function whose record is not declared in the file, the whole name otherwise.
    pub fn short_name(&self) -> &str {
        self.name.rsplit(['.', ':']).next().unwrap_or(&self.name)
    }
}

/// The rows `decl_index` (`prelude.lua`) builds, each with its `children`.
fn read_decls(list: &Table) -> Result<Vec<Decl>> {
    let mut out = Vec::new();
    for row in list.sequence_values::<Table>() {
        let row = row?;
        let int = |key: &str| -> Result<usize> {
            Ok(row.get::<Option<i64>>(key)?.unwrap_or(1).max(1) as usize)
        };
        let kind = match row.get::<String>("kind")?.as_str() {
            "local" => DeclKind::Local,
            "function" => DeclKind::Function,
            "method" => DeclKind::Method,
            "field" => DeclKind::Field,
            "record" => DeclKind::Record,
            "interface" => DeclKind::Interface,
            "enum" => DeclKind::Enum,
            _ => DeclKind::Alias,
        };
        let (name_line, name_col) = (int("ny")?, int("nx")?);
        let children = match row.get::<Option<Table>>("children")? {
            Some(children) => read_decls(&children)?,
            None => Vec::new(),
        };
        out.push(Decl {
            name: row.get("name")?,
            kind,
            detail: row.get("detail")?,
            line: int("y")?,
            col: int("x")?,
            end_line: row
                .get::<Option<i64>>("ey")?
                .map_or(name_line, |n| n.max(1) as usize),
            end_col: row.get::<Option<i64>>("ex")?.map(|n| n.max(1) as usize),
            name_line,
            name_col,
            children,
        });
    }
    Ok(out)
}

impl Ty {
    /// The name of the type without where it is reached from or what it is applied to:
    /// `Cfg` for `m.Cfg`, `Box` for `Box<integer>`, and for `{string}` or `function(..)`
    /// something no word is.
    fn base(&self) -> &str {
        self.name
            .split(['<', ' ', '(', '{'])
            .next()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or_default()
    }

    /// Where a name that has this type is declared, for a name the symbols did not find.
    fn declaration(&self, word: &str) -> Option<Place> {
        // The word being the name of the type makes this a use of the type, which resolves
        // to the declaration the type is of.
        if self.base() == word {
            return self.target.clone().or_else(|| self.own.clone());
        }
        if self.function {
            return self.own.clone();
        }
        None
    }
}

/// A run of identifier characters in a line.
struct Word<'a> {
    text: &'a str,
    /// Byte offset in the line.
    start: usize,
}

impl<'a> Word<'a> {
    /// The word `at` (a byte offset) is in or just after, `None` between two non-word
    /// characters.
    fn find(row: &'a str, at: usize) -> Option<Self> {
        let b = row.as_bytes();
        let word = |i: usize| {
            b.get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
        };
        let at = at.min(b.len());
        let anchor = if word(at) {
            at
        } else if at > 0 && word(at - 1) {
            at - 1
        } else {
            return None;
        };
        let mut start = anchor;
        while start > 0 && word(start - 1) {
            start -= 1;
        }
        let mut end = anchor + 1;
        while word(end) {
            end += 1;
        }
        Some(Self {
            text: &row[start..end],
            start,
        })
    }

    /// Where the dotted name the word ends starts (`a.b.c`: the `a`).
    fn head(&self, row: &str) -> usize {
        let b = row.as_bytes();
        let mut at = self.start;
        while at >= 2
            && b[at - 1] == b'.'
            && (b[at - 2].is_ascii_alphanumeric() || b[at - 2] == b'_')
        {
            at -= 2;
            while at > 0 && (b[at - 1].is_ascii_alphanumeric() || b[at - 1] == b'_') {
                at -= 1;
            }
        }
        at
    }

    /// Whether a `.` or a `:` follows the word (`m` of `m.Cfg`, `a` of `a:b`), not a `..`.
    fn is_head(&self, row: &str) -> bool {
        let b = row.as_bytes();
        let end = self.start + self.text.len();
        matches!(b.get(end), Some(b'.' | b':')) && !matches!(b.get(end + 1), Some(b'.' | b':'))
    }

    fn is_keyword(&self) -> bool {
        matches!(
            self.text,
            "and"
                | "break"
                | "do"
                | "else"
                | "elseif"
                | "end"
                | "false"
                | "for"
                | "function"
                | "goto"
                | "if"
                | "in"
                | "local"
                | "nil"
                | "not"
                | "or"
                | "repeat"
                | "return"
                | "then"
                | "true"
                | "until"
                | "while"
        )
    }

    fn is_number(&self) -> bool {
        self.text.as_bytes()[0].is_ascii_digit()
    }

    /// The offset of the `.` or `:` the word is a field after (`a.b`, `a:b`), not a `..`, a
    /// `...` or a `::label::`.
    fn field_dot(&self, row: &str) -> Option<usize> {
        let b = row.as_bytes();
        let dot = self.start.checked_sub(1)?;
        if !matches!(b[dot], b'.' | b':') {
            return None;
        }
        match dot.checked_sub(1).map(|i| b[i]) {
            Some(b'.') | Some(b':') => None,
            _ => Some(dot),
        }
    }
}

/// Whether the byte at `target` of `text` is inside a comment or a string literal, by
/// Lua's lexical rules: `--` to the end of the line, `--[==[ .. ]==]`, `"` and `'` strings
/// (with escapes, ending at the line end if unterminated) and `[==[ .. ]==]`.
fn inert(text: &str, target: usize) -> bool {
    let b = text.as_bytes();
    // `[`, any number of `=`, `[` at `i`: the number of `=`.
    let long_open = |i: usize| -> Option<usize> {
        if b.get(i) != Some(&b'[') {
            return None;
        }
        let eq = b[i + 1..].iter().take_while(|c| **c == b'=').count();
        (b.get(i + 1 + eq) == Some(&b'[')).then_some(eq)
    };
    // The offset just past the `]==]` that closes a long bracket whose body starts at `from`.
    let long_end = |from: usize, eq: usize| -> usize {
        let mut close = vec![b']'];
        close.extend(std::iter::repeat_n(b'=', eq));
        close.push(b']');
        b[from..]
            .windows(close.len())
            .position(|w| w == close.as_slice())
            .map_or(b.len(), |p| from + p + close.len())
    };
    let mut i = 0;
    while i < b.len() && i <= target {
        let start = i;
        if b[i] == b'-' && b.get(i + 1) == Some(&b'-') {
            i = match long_open(i + 2) {
                Some(eq) => long_end(i + 4 + eq, eq),
                None => b[i..]
                    .iter()
                    .position(|c| *c == b'\n')
                    .map_or(b.len(), |p| i + p),
            };
        } else if let Some(eq) = long_open(i) {
            i = long_end(i + 2 + eq, eq);
        } else if b[i] == b'"' || b[i] == b'\'' {
            let quote = b[i];
            i += 1;
            while i < b.len() && b[i] != quote && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
        } else {
            i += 1;
            continue;
        }
        if (start..i).contains(&target) {
            return true;
        }
    }
    false
}

impl CheckInfo {
    /// Shorthand for [`TypeIndex::at`] on the index this check kept.
    pub fn type_at(&self, text: &str, line: usize, col: usize) -> Option<TypeAt> {
        self.types.at(text, line, col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_strings_are_inert() {
        let t = "local a = 1 -- note\nlocal s = \"x y\" .. 'z'\n--[[ multi\nline ]] local b\n--[==[ a ]] b ]==] c\nlocal l = [[ q\nr ]] d";
        let at = |needle: &str| inert(t, t.find(needle).unwrap());
        assert!(!at("local a"));
        assert!(at("note"));
        assert!(at("x y"));
        assert!(at("z"));
        assert!(at("line"));
        assert!(!at("local b"));
        assert!(at("b ]==]"));
        assert!(!at("c\n"));
        assert!(at("q\n"));
        assert!(at("r ]]"));
        assert!(!at("d"));
        // An unterminated string ends with its line.
        assert!(!inert("\"abc\nnext", 6));
        assert!(inert("\"a\\\"b\" c", 3));
        assert!(!inert("\"a\\\"b\" c", 7));
    }

    #[test]
    fn a_word_is_found_from_anywhere_in_it_and_from_just_after() {
        let row = "print(g.text, 12)";
        let w = |at| Word::find(row, at).map(|w| (w.text, w.start));
        assert_eq!(w(0), Some(("print", 0)));
        assert_eq!(w(2), Some(("print", 0)));
        assert_eq!(w(5), Some(("print", 0))); // just after
        assert_eq!(w(6), Some(("g", 6)));
        assert_eq!(w(8), Some(("text", 8)));
        assert_eq!(w(12), Some(("text", 8)));
        assert_eq!(w(13), None);
        assert_eq!(Word::find("   ", 1).map(|w| w.start), None);
        // The dot a field is after; not a concatenation.
        let f = |row: &str, at| Word::find(row, at).and_then(|w| w.field_dot(row));
        assert_eq!(f("g.text", 3), Some(1));
        assert_eq!(f("g:text()", 3), Some(1));
        assert_eq!(f("a..b", 3), None);
        assert_eq!(f("g.text", 0), None);
    }
}
