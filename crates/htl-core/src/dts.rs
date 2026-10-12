//! Teal declaration (`.d.tl`) generation from Rust source.
//!
//! Shared by the proc macros (`#[host_module]` / `#[derive(TealRecord)]` at expansion
//! time) and by `htl dts`, which scans `.rs` files and writes the same declarations
//! *before* any `cargo build`, so `htl check` works on a fresh checkout.
//!
//! Type mapping is syntactic: `f64 -> number`, integers -> `integer`, `String`/`&str`
//! -> `string`, `bool -> boolean`, `Vec<T>` / `&[T]` / `[T; N]` / `VecDeque<T>` /
//! `HashSet<T>` -> `{T}`, `HashMap<K, V>` / `BTreeMap<K, V>` -> `{K:V}`, `mlua::Value` /
//! `serde_json::Value` -> `any` (the deliberate escape hatch), `Option<T> -> T`,
//! `Result<T, _> -> T`, `Strict<T> -> T`, `htl::task`'s `RecvChannel<T>` / `SendChannel<T>` /
//! `Request<A, B>` -> `task.RecvChannel<T>` / `task.SendChannel<T>` / `task.Request<A, B>`
//! (the declaration then starts with `local type task = require("htl.task")`, itself a
//! default `uses` entry — see below), other identifiers pass through as record names. A
//! qualified Rust path whose *first* segment is a `uses` entry's local name, for a module
//! other than `htl.task`, passes through dotted the same way (`types::Event` under
//! `uses = [types = ".."]` becomes `types.Event`, ahead of every other rule, so
//! `types::Value` is `types.Value` and not `any`) — any other qualified path
//! (`crate::geom::Point`, a type with no matching `uses` entry, or one pointed at
//! `htl.task` itself) keeps only its last segment, the same as a bare identifier.
//! There is no reflection on types in either direction: a Rust field of type `Foo` is
//! declared as `Foo`, and it is on the host that a Teal `Foo` exists; the module a
//! `---@contract` type is declared in goes the other way, Teal to `.d.tl`, with each
//! signature carried across as it was written ([`crate::contract::publish`]).
//! An `Option<T>` *parameter* is declared `name?: T` where Teal accepts the mark (a
//! trailing run of them); a field and a return value stay `T`, because a Teal record
//! field is nilable already and a return position has no `?`, while the mark on a
//! parameter is what lets a caller leave the argument out.
//!
//! # What each Rust shape becomes
//!
//! | Rust | Teal declaration | crosses as |
//! |---|---|---|
//! | `struct Point { x: f64, y: f64 }` | `record Point` | a table |
//! | `enum Mode { Fast, Careful }` | `enum Mode "Fast" "Careful" end` | the variant name, a string; any other string is refused: `Mode: expected one of "Fast", "Careful", got "fst"` |
//! | `enum Shape { Dot, Circle(f64), Rect { w: f64, h: f64 } }` | `record Shape_Dot`, `record Shape_Circle`, `record Shape_Rect`, each `where self.kind == "…"`, and `type Shape = Shape_Dot \| Shape_Circle \| Shape_Rect` | a table with `kind`; a newtype payload under `value`, struct fields under their names; `union-exhaustive` counts the variants, and a missing field reads `Shape.Rect.h: expected number, got nil` |
//! | `#[teal(rename_all = "snake_case")] enum State { Open, InReview }` | `enum State "open" "in_review" end` | the renamed word: `"open"` is accepted, `"Open"` is refused (`State: expected one of "open", "in_review", got "Open"`) |
//! | `#[serde(rename_all = "camelCase")] struct Call { #[serde(rename = "callId")] call_id: String, retry_count: u32, #[serde(alias = "msg")] message: String }` | `record Call` with `callId: string`, `retryCount: integer`, `message: string`; a name that is not a Teal identifier is declared `["call-id"]: string` | a table keyed as `serde_json` keys the object; `msg` is also read for `message` on the way in, and a missing field reads `Call.callId: expected string, got nil` |
//! | `struct Label(String)` | `type Label = string` | whatever the inner type crosses as |
//! | `Option<T>` | `T` as a field and as a return, `name?: T` as a method parameter | nil where the Rust side has `None`; the mark on a parameter is what lets a caller write `api:find("x")` |
//! | `Vec<T>` / `&[T]` / `[T; N]` / `VecDeque<T>` / `HashSet<T>` | `{T}` | a table used as a sequence |
//! | `HashMap<K, V>` / `BTreeMap<K, V>` | `{K:V}` | a table keyed by `K` |
//! | `mlua::Value` / `serde_json::Value` | `any` | unchanged: the deliberate escape hatch |
//! | `mlua::Function` as a `#[host_module]` parameter | `f: function` (`Option<Function>` as any `Option` parameter), and a sync fn's ones named in a trailing `---@noyield(f)`; see the `host_module` macro doc for the rule and the overrides | a Lua function the host calls |
//! | `&mlua::Lua` as a `#[host_module]` parameter | nothing: left out, as `&self` is; see the `host_module` macro doc | the state the method runs on |
//! | `#[teal(as = "string")] stamp: Stamp` as a record field (or a struct variant's), `Stamp` a host type with its own `IntoLua` / `FromLua` | `stamp: string`: the text, verbatim; the Rust type is not read | by `Stamp`'s own `IntoLua` / `FromLua`; a refused read is `Rec.stamp: expected string, got integer` |
//! | `#[teal(noyield)] update: Function` as a record field | `update: function ---@noyield` (`Option<Function>` as any field); see the `host_module` macro doc, *A Lua function as a parameter*, for when a field says it | a Lua function the host reads off the table and calls |
//! | `htl::task::RecvChannel<T>` / `SendChannel<T>` (feature `async`) as a `#[host_module]` return or parameter | `task.RecvChannel<T>` / `task.SendChannel<T>`, with `local type task = require("htl.task")` as the file's first line | the `htl.task` channel object: Teal receives from the first and sends into the second, and the other direction is a check error |
//! | `htl::task::Request<Req, Resp>` as a channel's element | `task.Request<Req, Resp>` | a value Teal answers once, `req:reply(v)` or `req:reject(reason)`; a third type argument (mlua-isle's rejection type) is an error, the reason being a `string` |
//!
//! A data-carrying enum is declared nested in the host module (`records = [Shape]`),
//! where its variant records are reachable as `host.Shape_Circle` for `is`; a record
//! declared in another module nests the same way, named by that module's path one level
//! deep, `records = [geom::Point]` for a `mod geom;` declared in the `#[host_module]`'s
//! own file ([`RecordRef`]) — `uses = [Name]` imports a type from another module instead,
//! a bare entry's local name and module path being the same word. `uses = [name =
//! "module.path"]` (an entry written `ident = "string"`) lets them differ: `local type
//! name = require("module.path")`, which is what a type whose module is not its own name
//! needs — `htl.task`'s types used as `task.RecvChannel<T>`, or a library record from
//! `somelib.types` used as `types.Event`. `htl.task` is itself written this way: the
//! generator adds a default `task = "htl.task"` entry when the declaration needs one of
//! `htl.task`'s own types and no entry already names `task` — a `uses` entry that does
//! name `task` for `htl.task` itself is accepted in the default's place (no second line);
//! one that names `task` for a different module, where the declaration also needs
//! `htl.task`, is refused, naming both; see [`TealAttrs::uses`].
//! `#[teal(rename_all = "..")]` takes serde's set — `lowercase`, `UPPERCASE`,
//! `PascalCase`, `camelCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`,
//! `SCREAMING-KEBAB-CASE` — applied serde's way ([`RenameRule`]): on an enum it spells the
//! variants, on a struct the fields, and `#[teal(name = "..")]` on one variant overrides
//! it. A table coming back that does not fit says which record, which field (as the Teal
//! side names it), what was declared and what arrived ([`crate::teal::FieldError`]):
//! `Outcome.cause: expected string, got nil`.
//!
//! **What a type's `#[serde(..)]` says is read**, so a type deriving `Serialize` /
//! `Deserialize` beside `TealRecord` is declared and crosses under the names serde gives
//! it, and the keys of the Lua table are the keys `serde_json` writes:
//!
//! - read: `rename_all` (a struct's fields, an enum's variants), `rename_all_fields` (on an
//!   enum, the fields of its struct variants), `rename` (a field, a variant) and `alias` (a
//!   field: one more key `from_lua` reads when the field's own is nil, never declared and
//!   never written). A name that is not a Teal identifier (`call-id`, `end`) is declared
//!   with Teal's key syntax, `["call-id"]: string`; one holding a `"`, a `\` or a control
//!   character is refused.
//! - precedence: the word on the field or variant beats the rule on the type, and at each
//!   level `teal` beats `serde` — `#[teal(name)]` over `#[serde(rename)]` on a variant,
//!   `#[teal(rename_all)]` over `#[serde(rename_all)]` on the type.
//! - refused, saying why: a name that differs by direction (`rename(serialize = "a",
//!   deserialize = "b")`, or one side alone; `rename_all` the same) — one type cannot carry
//!   two names; and every word that changes the shape and is not read yet: `skip`,
//!   `skip_serializing`, `skip_deserializing`, `skip_serializing_if`, `default`, `flatten`,
//!   `tag`, `content`, `untagged`, `transparent`, `with`, `serialize_with`,
//!   `deserialize_with`, `from`, `into`, `try_from`, `remote`, `other`, `borrow`, `getter`,
//!   `variant_identifier`, `field_identifier`. A declaration that ignored one would state a
//!   shape the value does not have. `alias` / `rename_all` on a variant and an attribute
//!   the reader cannot parse are refused too, the latter quoting it.
//! - passed through to serde, as they keep the shape: `crate`, `bound`,
//!   `deny_unknown_fields`, `expecting`, and `rename` on the type itself (the Teal name is
//!   `#[teal(name)]`).
//!
//! # Why `#[derive(TealRecord)]` lowers the way it does
//!
//! This is the reasoning behind the table above.
//!
//! - **A data-carrying enum is a union of `where`-discriminated records**, not one
//!   record with every variant's fields optional. Teal refuses a plain union of two
//!   table types (`cannot discriminate a union between multiple table types`): `is`
//!   narrows with a `type()` check, and two records are both `table`. A `where` clause
//!   on each record is its own answer to that; with it, `is N_A` narrows and
//!   `union-exhaustive` counts the variants, which is the whole point of declaring a
//!   closed set. The derive covers enums and aliases as well as structs for the same
//!   reason: a host's closed sets reach Teal as declarations rather than as `any`.
//! - **It can only be declared nested** (`records = [N]` in the host module). A caller
//!   narrows with `is module.N_A`, so it needs the variant records by name, and a
//!   `.d.tl` module exports one name — the union. `#[teal(dts = ..)]` on one is refused
//!   with that advice rather than writing a declaration nobody can narrow against.
//! - **`uses` imports with `local type name = require("module")`**. A module whose value
//!   is only a type (an alias, an enum) is "abstract" to Teal when required as a value,
//!   and the `type` form imports a record just the same, so one form serves every kind.
//! - **The tag is `kind`, the newtype payload is `value`**, and neither is configurable:
//!   a name that differs per host is a name the reader has to look up, and these are
//!   what Teal's own `where` examples use. A struct variant may not carry a field named
//!   `kind`.
//! - **A variant's word is the Rust name unless the enum says otherwise.**
//!   `#[teal(rename_all = "snake_case")]` on the enum and `#[teal(name = "..")]` on one
//!   variant spell what crosses — the `enum` entries, the `kind` tag, the run-time match
//!   and the message that lists what is accepted — because a Teal project that already
//!   holds `"open"` in its store should not have to become `"Open"` to move its enum to
//!   the host. What the rule does *not* touch is the union's record names
//!   (`Shape_InReview` stays): those are Teal identifiers, and `kebab-case` is not one.
//!   A record field is declared under its Rust name unless serde names it otherwise
//!   (`#[serde(rename)]`, the type's `rename_all`; see above). A field's `#[teal(..)]`
//!   takes two words, neither of which spells it: `as = "<teal type>"`, the type it is
//!   declared as when its Rust type does not say (below), and `noyield`, on a `Function` /
//!   `Option<Function>` field of a struct, which says how the host calls it. Anything else
//!   in a field's `#[teal(..)]` is refused. Renaming a field is not `as`'s job: `as` says
//!   the type, and the name is serde's when serde says one, the Rust one otherwise; the
//!   two compose, a renamed field carrying `as` too.
//! - **`#[teal(as = "..")]` declares; it does not convert.** The field is written as the
//!   given Teal text, verbatim (`"string"`, `"{string}"`, `"geom.Point"`), and the Rust type
//!   is not mapped; the value still crosses by the field type's own `IntoLua` / `FromLua`,
//!   so that type must implement both, and the attribute is the host's statement of the
//!   shape they produce — a type with a hand-written `IntoLua` that crosses as a string,
//!   say. Nothing checks the statement against the impl: a field that says `string` and
//!   converts to a table is a declaration that lies. The text is also what a refused read
//!   reports as expected, when the type's `FromLua` refuses with mlua's
//!   `FromLuaConversionError` (`Rec.stamp: expected string, got integer`); any other error
//!   it returns keeps its own message under the field's path. It applies to a field
//!   of a struct record or of a data enum's struct variant; on a type, a variant or a
//!   `#[host_module]` it is refused.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, GenericArgument, ImplItem, Item, ItemEnum, ItemImpl, ItemStruct, Lit,
    Meta, Pat, PathArguments, ReturnType, Token, Type,
};

// ---------------------------------------------------------------- type mapping

/// Map a Rust type to a Teal type name. `self_name` replaces `Self`.
pub fn teal_type(ty: &Type, self_name: &str) -> Result<String, String> {
    teal_type_in(ty, self_name, &[])
}

/// [`teal_type`], with `uses` in scope: a qualified Rust path (`types::Event`) whose
/// *first* segment is one of `uses`'s local names, for a module other than `htl.task`,
/// crosses dotted (`types.Event`) — the word that entry's import resolves under — ahead
/// of every other rule here, so a name that would otherwise be special (`types::Value`,
/// `mytask::RecvChannel<T>`) still dots rather than being read as `any` or mistaken for
/// `htl.task`'s own channel. A qualified path whose first segment is not a `uses` name —
/// `crate::geom::Point`, `self::Point`, simply `geom::Point` with no matching entry —
/// keeps only its last segment (`Point`), as every other mapping here does; a `.d.tl`
/// would otherwise be asked to resolve a dotted name nothing imports. A `uses` entry
/// whose module *is* `htl.task` is not treated specially here at all: `task::RecvChannel`
/// still reaches the arity-matched `RecvChannel` arm below, which writes the `task.`
/// prefix itself.
fn teal_type_in(ty: &Type, self_name: &str, uses: &[Use]) -> Result<String, String> {
    match ty {
        Type::Reference(r) => teal_type_in(&r.elem, self_name, uses),
        Type::Paren(p) => teal_type_in(&p.elem, self_name, uses),
        Type::Tuple(t) if t.elems.is_empty() => Ok(String::new()),
        Type::Tuple(t) => {
            let parts: Result<Vec<_>, _> = t
                .elems
                .iter()
                .map(|e| teal_type_in(e, self_name, uses))
                .collect();
            Ok(parts?.join(", "))
        }
        Type::Slice(s) => Ok(format!("{{{}}}", teal_type_in(&s.elem, self_name, uses)?)),
        Type::Array(a) => Ok(format!("{{{}}}", teal_type_in(&a.elem, self_name, uses)?)),
        Type::Path(p) => {
            let seg = p.path.segments.last().ok_or("empty type path")?;
            let ident = seg.ident.to_string();
            let args: Vec<&Type> = match &seg.arguments {
                PathArguments::AngleBracketed(ab) => ab
                    .args
                    .iter()
                    .filter_map(|a| match a {
                        GenericArgument::Type(t) => Some(t),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let arg = |i: usize| -> Result<String, String> {
                args.get(i)
                    .ok_or_else(|| format!("{ident}: missing type argument {i}"))
                    .and_then(|t| teal_type_in(t, self_name, uses))
            };
            // A path qualified by one of `uses`'s local names, for a module other than
            // `htl.task`, crosses dotted ahead of every special case below — `Vec`,
            // `Value`, `RecvChannel` included — so `uses = [mytask = "my.task"]`'s
            // `mytask::RecvChannel<i64>` is `mytask.RecvChannel` and is not mistaken for
            // `htl.task`'s own `RecvChannel`, and `types::Value` under a `uses = [types =
            // ".."]` is `types.Value`, not `any`. Generic arguments are dropped here, the
            // same as the bare fallback below (`other => other.to_string()`): a `uses`
            // name's own `Container<T>` crosses as `name.Container`, not
            // `name.Container<T>`. A `uses` entry whose module *is* `htl.task` is left to
            // the arity-matched `RecvChannel` / `SendChannel` / `Request` arms below
            // instead (an explicit `task = "htl.task"` entry's `task::RecvChannel<T>`
            // must still reach them).
            if p.path.segments.len() > 1
                && let Some(u) = uses.iter().find(|u| p.path.segments[0].ident == u.name)
                && u.module != "htl.task"
            {
                return Ok(p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("."));
            }
            Ok(match ident.as_str() {
                "f32" | "f64" => "number".into(),
                "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
                | "u128" | "usize" => "integer".into(),
                "bool" => "boolean".into(),
                "String" | "str" => "string".into(),
                "Self" => self_name.into(),
                "Vec" | "VecDeque" | "HashSet" | "BTreeSet" => format!("{{{}}}", arg(0)?),
                "HashMap" | "BTreeMap" => format!("{{{}:{}}}", arg(0)?, arg(1)?),
                "Option" | "Result" | "Box" | "Rc" | "Arc" => arg(0)?,
                // `Strict<T>` holds the runtime to the declaration `T` already makes; the
                // declaration itself is `T`'s.
                "Strict" => arg(0)?,
                // mlua's handles to a userdata value: the Teal side sees the host type
                // itself, which is what `open(..) -> Session` declared on the way out.
                "UserDataRef" | "UserDataRefMut" | "UserDataOwned" => arg(0)?,
                "Value" => "any".into(),
                "Table" => "{any:any}".into(),
                "Function" => "function".into(),
                "LuaString" => "string".into(),
                // `htl::task`'s typed host channels and requests, declared in `htl.task`:
                // the generated declaration imports it as `task` (see `with_task_default`).
                // Matched by name and arity, so a type of the host's own called `Request`
                // with fewer than two type arguments stays a name of its own (three is
                // refused below).
                "RecvChannel" | "SendChannel" if args.len() == 1 => {
                    format!("{TASK_ALIAS}.{ident}<{}>", arg(0)?)
                }
                "Request" if args.len() == 2 => {
                    format!("{TASK_ALIAS}.Request<{}, {}>", arg(0)?, arg(1)?)
                }
                // mlua-isle's `Request<Req, Resp, E>` with the rejection reason's type
                // spelled out. `task.Request` declares `req:reject(reason: string)`, the
                // `E = String` htl's `Ask` uses; a third argument has no Teal spelling, and
                // falling through to the bare name would declare a `Request` no `.d.tl`
                // has. Refused here rather than guessed.
                "Request" if args.len() == 3 => {
                    return Err("`Request` with three type arguments: htl's `task.Request` \
                                takes two, `Request<Q, A>`; the rejection reason is a string \
                                (`req:reject(reason: string)`), so drop the third"
                        .into());
                }
                other => other.to_string(),
            })
        }
        _ => Err(
            "unsupported type for Teal mapping (use a path, reference, tuple, slice or array type)"
                .into(),
        ),
    }
}

/// `true` if the outermost type is `Option<..>`. A parameter of that shape is declared
/// `name?: T`, the Teal spelling for an argument the caller may leave out.
pub fn is_option(ty: &Type) -> bool {
    match ty {
        Type::Reference(r) => is_option(&r.elem),
        Type::Paren(p) => is_option(&p.elem),
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident == "Option")
            .unwrap_or(false),
        _ => false,
    }
}

/// `true` if the type is mlua's `Function` — a Lua function handed to the host — bare,
/// behind a reference, or as `Option<Function>`. Matched by the path's last segment, as
/// every mapping here is except a `uses` name's own qualified types ([`teal_type`]):
/// `mlua::Function`, `htl::mlua::Function` and an imported `Function` are the same
/// parameter to a source that is read, not resolved.
pub fn is_function(ty: &Type) -> bool {
    match ty {
        Type::Reference(r) => is_function(&r.elem),
        Type::Paren(p) => is_function(&p.elem),
        Type::Path(p) => {
            let Some(seg) = p.path.segments.last() else {
                return false;
            };
            if seg.ident == "Function" {
                return true;
            }
            if seg.ident != "Option" {
                return false;
            }
            match &seg.arguments {
                PathArguments::AngleBracketed(ab) => ab.args.iter().any(|a| match a {
                    GenericArgument::Type(t) => is_function(t),
                    _ => false,
                }),
                _ => false,
            }
        }
        _ => false,
    }
}

/// `true` if `ty` is a reference to mlua's `Lua` — `&Lua`, `&mlua::Lua`, `&htl::mlua::Lua`,
/// `&::htl::mlua::Lua` — matched by the last path segment, as [`is_function`] is: a
/// source is read, not resolved, so another crate's type called `Lua` behind a reference
/// is taken for mlua's. `&mut Lua` does not match (mlua hands out `&Lua`, never `&mut`),
/// nor does the bare `Lua` with no reference. Such a parameter is filled from the
/// closure's own Lua handle rather than from the Lua arguments, and is left out of the
/// `.d.tl`, the way `&self` is ([`host_decl`]).
pub fn is_lua_ref(ty: &Type) -> bool {
    match ty {
        Type::Reference(r) if r.mutability.is_none() => match &*r.elem {
            Type::Path(p) => p.path.segments.last().is_some_and(|s| s.ident == "Lua"),
            _ => false,
        },
        _ => false,
    }
}

/// `true` if `ty` is mlua's `Lua` *by value* — no reference at all. Matched the same way
/// [`is_lua_ref`] matches the reference: a parameter spelled this way is refused in
/// [`host_decl`] rather than left to fall through to the ordinary parameter path, where it
/// would be declared `lua: Lua` in the `.d.tl` and then fail at `cargo build` with mlua's
/// `the trait bound (htl::mlua::Lua,): FromLuaMulti is not satisfied` — the same error
/// `&Lua` used to produce before this type was given its own path (#427).
fn is_lua_owned(ty: &Type) -> bool {
    match ty {
        Type::Path(p) => p.path.segments.last().is_some_and(|s| s.ident == "Lua"),
        _ => false,
    }
}

/// `true` if the outermost type is `Result<..>` (the wrapper must propagate the error).
pub fn is_result(ty: &Type) -> bool {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident == "Result")
            .unwrap_or(false),
        _ => false,
    }
}

// ---------------------------------------------------------------- attributes

/// How a Rust name is spelled on the Teal side: `#[teal(rename_all = "..")]` /
/// `#[serde(rename_all = "..")]`, serde's set and serde's spellings of it, so a type that
/// is also `Serialize` says the same thing once and the two agree.
///
/// The rules are serde's, and serde applies them two ways. To a variant name (`InReview`,
/// [`apply`](Self::apply)): `lowercase` lowers the whole word, `snake_case` breaks it at
/// each capital, and the rest follow from those two. To a field name (`call_id`,
/// [`apply_to_field`](Self::apply_to_field)), which Rust already writes in `snake_case`:
/// `lowercase` and `snake_case` leave it, `camelCase` / `PascalCase` join it at each `_`,
/// and the rest change the case or the separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameRule {
    /// `InReview` -> `inreview`.
    Lower,
    /// `InReview` -> `INREVIEW`.
    Upper,
    /// `InReview` -> `InReview` (the Rust spelling; accepted so the attribute can be
    /// written out where a `Serialize` impl already says it).
    Pascal,
    /// `InReview` -> `inReview`.
    Camel,
    /// `InReview` -> `in_review`.
    Snake,
    /// `InReview` -> `IN_REVIEW`.
    ScreamingSnake,
    /// `InReview` -> `in-review`.
    Kebab,
    /// `InReview` -> `IN-REVIEW`.
    ScreamingKebab,
}

impl RenameRule {
    /// The accepted spellings, in the order the error message lists them.
    pub const NAMES: &'static [&'static str] = &[
        "lowercase",
        "UPPERCASE",
        "PascalCase",
        "camelCase",
        "snake_case",
        "SCREAMING_SNAKE_CASE",
        "kebab-case",
        "SCREAMING-KEBAB-CASE",
    ];

    /// Parse the attribute's string, naming the whole set when it is none of them.
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(match s {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            other => {
                return Err(format!(
                    "`rename_all` must be one of {}, got {other:?}",
                    Self::NAMES
                        .iter()
                        .map(|n| format!("{n:?}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        })
    }

    /// Apply the rule to a variant name written in Rust's `PascalCase`.
    pub fn apply(self, variant: &str) -> String {
        // snake_case is the one that reads the word's shape; the other separator forms
        // are it with a different separator or a different case.
        let snake = || {
            let mut s = String::new();
            for (i, ch) in variant.char_indices() {
                if i > 0 && ch.is_uppercase() {
                    s.push('_');
                }
                s.push(ch.to_ascii_lowercase());
            }
            s
        };
        match self {
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Pascal => variant.to_string(),
            Self::Camel => {
                let mut c = variant.chars();
                match c.next() {
                    Some(first) => first.to_ascii_lowercase().to_string() + c.as_str(),
                    None => String::new(),
                }
            }
            Self::Snake => snake(),
            Self::ScreamingSnake => snake().to_ascii_uppercase(),
            Self::Kebab => snake().replace('_', "-"),
            Self::ScreamingKebab => snake().to_ascii_uppercase().replace('_', "-"),
        }
    }

    /// Apply the rule to a field name written in Rust's `snake_case`, as serde does
    /// (`call_id` -> `callId` under `camelCase`).
    pub fn apply_to_field(self, field: &str) -> String {
        let pascal = || {
            let mut s = String::new();
            let mut capitalize = true;
            for ch in field.chars() {
                if ch == '_' {
                    capitalize = true;
                } else if capitalize {
                    s.push(ch.to_ascii_uppercase());
                    capitalize = false;
                } else {
                    s.push(ch);
                }
            }
            s
        };
        match self {
            Self::Lower | Self::Snake => field.to_string(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => pascal(),
            Self::Camel => {
                let p = pascal();
                let mut c = p.chars();
                match c.next() {
                    Some(first) => first.to_ascii_lowercase().to_string() + c.as_str(),
                    None => String::new(),
                }
            }
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.to_ascii_uppercase().replace('_', "-"),
        }
    }
}

/// One `uses` entry: the local name a `.d.tl` imports a module's type under, and the
/// module `require` resolves it from. A bare `uses = [X]` entry is `Use { name: "X",
/// module: "X" }` (the bare form); `uses = [name = "module.path"]` is
/// `Use { name: "name", module: "module.path" }`, for a module whose path is not the word
/// a declaration uses the type under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Use {
    /// The local name: what the `.d.tl` calls `require`'s result, and what a reference to
    /// one of the module's types is qualified with (`task.RecvChannel<T>`).
    pub name: String,
    /// What `require` is asked for: a bare entry's own name, or the string after `=`.
    pub module: String,
}

impl Use {
    /// `uses = [X]`: `X` is both the local name and the module `require` resolves.
    fn bare(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            module: name.clone(),
            name,
        }
    }
}

/// One `records = [..]` entry. A bare path (`records = [Point]`) is `RecordRef { module:
/// None, name: "Point" }`: the record is looked for among the `#[host_module]`'s own
/// file's items, as it always was. A two-segment path (`records = [geom::Point]`) is
/// `RecordRef { module: Some("geom"), name: "Point" }`: `geom` names a `mod geom;` (or
/// `pub mod geom;`) declared in that same file, and `Point` is looked for in its
/// contents — inline (`mod geom { .. }`) or, when the module has none of its own, in the
/// file Rust would compile `mod geom;` from: next to the `#[host_module]`'s own file when
/// that file is a crate root, or, for any other file, in a directory named after that
/// file's own stem (`src/hostio.rs`'s `mod geom;` is `src/hostio/geom.rs`, not
/// `src/geom.rs`; see `mod_base_dir` for exactly which files and directories count as a
/// crate root). This does not know about a `[lib] path` override or another custom crate
/// root in `Cargo.toml`, nor about a `#[cfg_attr(.., path = "..")]` (only a plain
/// `#[path = ".."]` is read, and even that is refused rather than followed); of two
/// `#[cfg]`-alternated `mod geom` declarations in the same file, the first is the one
/// read; in every one of these cases `mod geom;` could resolve — or already resolves —
/// somewhere this does not predict. A path longer than two segments is refused where
/// `records` is parsed (`record_list`): one module level is resolved, no deeper, and no
/// re-export is followed — the path names the module the record is declared in, not
/// wherever it is re-exported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRef {
    /// `Some("geom")` for `records = [geom::Point]`; `None` for a bare `records =
    /// [Point]`.
    pub module: Option<String>,
    /// The record's Rust name (`records = [geom::Point]` and `records = [Point]` both
    /// name it `Point`; nothing about the nested declaration's own name changes with
    /// where it was found).
    pub name: String,
}

/// `#[teal(...)]` / `#[host_module(...)]` arguments.
#[derive(Debug, Clone, Default)]
pub struct TealAttrs {
    /// `#[teal(name = "..")]` / `#[host_module(name = "..")]`: what the item is called on
    /// the Teal side, when that is not the Rust identifier. It names a module, a record,
    /// or one enum variant's word, depending on what it was parsed from — and is refused
    /// on a record reached through `records = [..]`, where the host's own signatures
    /// already spell the Rust name.
    pub name: Option<String>,
    /// `#[teal(rename_all = "..")]`: on an enum, how its variants are spelled on the Teal
    /// side; on a struct, how its fields are. It wins over the type's
    /// `#[serde(rename_all)]`. The Rust names are unchanged, and so are the union's variant
    /// record names (`Shape_InReview`) — a Teal identifier cannot be `kebab-case`; what the
    /// rule spells is the word that crosses.
    pub rename_all: Option<RenameRule>,
    /// `.d.tl` output path, relative to the crate's `CARGO_MANIFEST_DIR`.
    pub dts: Option<String>,
    /// Types declared in their own `.d.tl` module: `uses = [X]` emits
    /// `local type X = require("X")`. `uses = [name = "module.path"]` — an entry written
    /// `ident = "string"`, the one other shape a `uses` element may take, refused when
    /// the left side is not a single identifier, the right not a non-empty string of
    /// letters, digits, `_`, `.` or `-`, or a local name is given twice — emits
    /// `local type name = require("module.path")`, for a type whose module is not its own
    /// name (`types = "somelib.types"` for a type used as `types.Event`); the same name
    /// is what a qualified Rust field or return type (`name::Type`) crosses dotted under
    /// (`teal_type`). `htl::task`'s types (`task = "htl.task"`) are folded in this
    /// way too, as the default entry [`host_decl`] / [`record_decl`] add when the
    /// declaration needs one and no entry already names `task`; a `uses` entry that does
    /// name `task` for `htl.task` itself needs no second line, one that names it for a
    /// *different* module while the declaration also needs `htl.task` is refused, naming
    /// both. See [`Use`].
    pub uses: Vec<Use>,
    /// `#[derive(TealRecord)]` types nested inside the module record: a bare entry
    /// (`records = [Point]`) in the same source file, a module-qualified one
    /// (`records = [geom::Point]`) in a `mod geom;` declared in that file. See
    /// [`RecordRef`].
    pub records: Vec<RecordRef>,
    /// How `Result<T, E>` returns reach Lua: `"raise"` (default; `Err` becomes a Lua
    /// error) or `"return"` (`T, string` / `boolean, string` in the `io.open` style).
    pub errors: Option<String>,
}

/// How a `#[host_module]` maps `Result<T, E>` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrMode {
    /// `Err(e)` raises a Lua error; Teal sees `function(...): T`.
    Raise,
    /// `Ok(v)` -> `v` (or `true` for unit), `Err(e)` -> `nil, tostring(e)`;
    /// Teal sees `function(...): T, string` (`boolean, string` for unit).
    Return,
}

fn lit_str(l: &Lit) -> Result<String, String> {
    match l {
        Lit::Str(s) => Ok(s.value()),
        _ => Err("expected a string literal".into()),
    }
}

/// `records = [Point, geom::Light]`: a one-segment path is a bare [`RecordRef`] (the
/// record lives in the `#[host_module]`'s own file, as it always did); a two-segment
/// path names a `mod` declared in that file and the record inside it
/// (`resolve_module_items`). Anything longer is refused: `records` resolves one module
/// level, no deeper. Every leading `self::` is stripped first (`self::Point` is `Point`,
/// `self::self::Point` is still `Point` — each just repeats "this module", which is what
/// a bare entry already means) before anything else is read off the path. A leading `::`
/// (`::geom::Point` — in the 2018+ path grammar, a crate rather than a local item) and
/// `crate::` / `super::` are refused outright, naming the two forms `records` does
/// accept, rather than either read as a module literally named `crate` or `super` (there
/// is no such `mod`, so that read would only fail later with a worse message) or ignored
/// silently (a leading `::` changes nothing about which segments remain, so without this
/// check it would resolve exactly like the same path without it).
fn record_list(arr: &syn::ExprArray) -> Result<Vec<RecordRef>, String> {
    let expected = "`records` names a record directly (`Record`), or through one module level (`module::Record`)";
    let mut out = Vec::new();
    for e in &arr.elems {
        match e {
            Expr::Path(p) => {
                let full: Vec<String> = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                if p.path.leading_colon.is_some() {
                    return Err(format!(
                        "{expected}; `::{}` names an external crate (a leading `::`), not a \
                         module declared in this file",
                        full.join("::")
                    ));
                }
                let mut segs = full.clone();
                while segs.first().map(String::as_str) == Some("self") {
                    segs.remove(0);
                }
                if matches!(
                    segs.first().map(String::as_str),
                    Some("crate") | Some("super")
                ) {
                    return Err(format!("{expected}; `{}` is neither", full.join("::")));
                }
                match segs.len() {
                    0 => return Err(format!("{expected}; `{}` is neither", full.join("::"))),
                    1 => out.push(RecordRef {
                        module: None,
                        name: segs.into_iter().next().unwrap_or_default(),
                    }),
                    2 => {
                        let mut segs = segs.into_iter();
                        let module = segs.next().unwrap_or_default();
                        let name = segs.next().unwrap_or_default();
                        out.push(RecordRef {
                            module: Some(module),
                            name,
                        })
                    }
                    n => {
                        return Err(format!(
                            "{expected}; `{}` is {} levels deep",
                            full.join("::"),
                            n - 1
                        ));
                    }
                }
            }
            _ => return Err("`records` expects a list of type names".to_string()),
        }
    }
    Ok(out)
}

/// `true` for a non-empty string of letters, digits, `_`, `.` or `-` — the characters a
/// dotted Teal module path (and `require`'s argument) is made of; anything else would
/// land unescaped inside `require("...")` ([`uses_header`]) and either break the string
/// literal or `require` nothing a file could ever be named.
fn valid_module_path(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// `uses = [Mode, task = "htl.task"]`: a bare path is [`Use::bare`], and `ident = "str"`
/// (parsed as an assignment expression — `=` inside an array literal) is `Use { name:
/// ident, module: str }`. A repeated local name, in either form, is refused naming it;
/// `=`'s left side must be a single identifier and its right a string of the characters
/// [`valid_module_path`] accepts.
fn use_list(arr: &syn::ExprArray) -> Result<Vec<Use>, String> {
    let expected = "`uses` expects a list of type names, or `name = \"module.path\"`";
    let mut out: Vec<Use> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut push = |u: Use| -> Result<(), String> {
        if !seen_names.insert(u.name.clone()) {
            return Err(format!(
                "`uses` names `{}` twice; each local name may appear once",
                u.name
            ));
        }
        out.push(u);
        Ok(())
    };
    for e in &arr.elems {
        match e {
            Expr::Path(p) => {
                let name = p
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                push(Use::bare(name))?;
            }
            Expr::Assign(a) => {
                let Expr::Path(name_path) = &*a.left else {
                    return Err(format!(
                        "{expected} (the left side of `=` must be a single identifier, got an expression)"
                    ));
                };
                let Some(name) = name_path.path.get_ident() else {
                    return Err(format!(
                        "{expected} (the left side of `=` must be a single identifier, got a path)"
                    ));
                };
                let Expr::Lit(syn::ExprLit {
                    lit: Lit::Str(module),
                    ..
                }) = &*a.right
                else {
                    return Err(format!(
                        "{expected} (the right side of `=` must be a string literal)"
                    ));
                };
                let module = module.value();
                if !valid_module_path(&module) {
                    return Err(format!(
                        "`uses`'s module path must be non-empty and made only of letters, \
                         digits, `_`, `.` or `-`, got {module:?}"
                    ));
                }
                push(Use {
                    name: name.to_string(),
                    module,
                })?;
            }
            _ => return Err(expected.into()),
        }
    }
    Ok(out)
}

/// The refusal of `as` written on a type, a variant or a `#[host_module]`.
const AS_OFF_FIELD: &str = "`as` is a field attribute: `#[teal(as = \"<teal type>\")]` goes on \
     a field of a `#[derive(TealRecord)]` struct or of a data enum's struct variant, not on a \
     type, an enum variant or a `#[host_module]`";

/// Parse `name = "..", rename_all = "..", dts = "..", uses = [A, b = "mod.b"], records =
/// [C]`.
///
/// Every key is parsed here whatever it sits on; where a key is meaningful is decided by
/// the caller (`rename_all` on an enum, `name` alone on a variant).
pub fn parse_attr_metas(metas: impl IntoIterator<Item = Meta>) -> Result<TealAttrs, String> {
    let mut out = TealAttrs::default();
    for meta in metas {
        // `as` is a word of a record field's `#[teal(..)]`, and a field is the only place it
        // means anything; here it would be read as nothing, so it is refused by name.
        if meta.path().is_ident("as") {
            return Err(AS_OFF_FIELD.into());
        }
        let Meta::NameValue(nv) = meta else {
            return Err("expected `key = value` pairs".into());
        };
        let key = nv
            .path
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        match (key.as_str(), &nv.value) {
            ("name", Expr::Lit(l)) => out.name = Some(lit_str(&l.lit)?),
            ("rename_all", Expr::Lit(l)) => {
                out.rename_all = Some(RenameRule::parse(&lit_str(&l.lit)?)?)
            }
            ("dts", Expr::Lit(l)) => out.dts = Some(lit_str(&l.lit)?),
            ("uses", Expr::Array(arr)) => out.uses = use_list(arr)?,
            ("records", Expr::Array(arr)) => out.records = record_list(arr)?,
            ("errors", Expr::Lit(l)) => {
                let v = lit_str(&l.lit)?;
                if v != "raise" && v != "return" {
                    return Err(format!(
                        "`errors` must be \"raise\" or \"return\", got {v:?}"
                    ));
                }
                out.errors = Some(v);
            }
            (k, _) => return Err(format!("unknown or malformed attribute `{k}`")),
        }
    }
    Ok(out)
}

/// Every `#[<name>(..)]` in `attrs`, merged. `is` says which attribute paths are `name`.
fn parse_named_attr(
    attrs: &[Attribute],
    is: impl Fn(&syn::Path) -> bool,
) -> Result<Option<TealAttrs>, String> {
    let mut metas = Vec::new();
    let mut found = false;
    for a in attrs {
        if is(a.path()) {
            found = true;
            if let Meta::List(_) = &a.meta {
                let list = a
                    .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                    .map_err(|e| {
                        // A `Meta` cannot be named by a keyword, so `as` fails the parse
                        // before `parse_attr_metas` could name the place it belongs.
                        if names_as(a) {
                            AS_OFF_FIELD.to_string()
                        } else {
                            e.to_string()
                        }
                    })?;
                metas.extend(list);
            }
        }
    }
    if !found {
        return Ok(None);
    }
    parse_attr_metas(metas).map(Some)
}

/// `true` when `a`'s list names `as` as one of its words, read with the parser that takes a
/// keyword as a word's name (the one a list of `Meta` refuses).
fn names_as(a: &Attribute) -> bool {
    let mut found = false;
    let _ = a.parse_nested_meta(|m| {
        if m.path.is_ident("as") {
            found = true;
            return Err(m.error("as"));
        }
        if m.input.peek(Token![=]) {
            m.value()?.parse::<Expr>()?;
        }
        Ok(())
    });
    found
}

/// `#[teal(...)]` on a struct or enum (absent -> defaults).
///
/// Only the bare name: `teal` is a helper attribute of `#[derive(TealRecord)]`, and Rust
/// does not accept a helper written with a path, so `#[htl::teal(..)]` never compiles.
pub fn parse_teal_attrs(attrs: &[Attribute]) -> Result<TealAttrs, String> {
    Ok(parse_named_attr(attrs, |p| p.is_ident("teal"))?.unwrap_or_default())
}

/// `#[host_module(...)]` on an impl block, or `None` when the attribute is absent.
///
/// Matched by the path's last segment, as `derive(TealRecord)` is: the attribute macro
/// compiles as `#[host_module]` after `use htl::host_module` and as `#[htl::host_module]`
/// without it, and the host registers the module either way. Which crate the path names
/// is not checked — a source is read, not resolved — so another crate's attribute called
/// `host_module` is taken for htl's.
pub fn parse_host_module_attr(attrs: &[Attribute]) -> Result<Option<TealAttrs>, String> {
    parse_named_attr(attrs, |p| {
        p.segments.last().is_some_and(|s| s.ident == "host_module")
    })
}

/// `true` if `#[derive(..., TealRecord, ...)]` is present.
pub fn derives_teal_record(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("derive") {
            return false;
        }
        a.parse_args_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
            .map(|paths| {
                paths.iter().any(|p| {
                    p.segments
                        .last()
                        .map(|s| s.ident == "TealRecord")
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    })
}

/// The name a generated declaration imports `htl.task` under, and the prefix
/// [`teal_type`] writes for `RecvChannel<T>` / `SendChannel<T>` / `Request<A, B>`.
const TASK_ALIAS: &str = "task";

/// `true` when `body` contains a type [`teal_type_in`] wrote from `htl.task` — the
/// generated forms `task.RecvChannel<`, `task.SendChannel<` or `task.Request<` — as
/// opposed to any other text ending in `task.`, which may be an unrelated `uses` entry's
/// own local name (a host module called `task`, say, with a field `j: task::Job`).
fn mentions_htl_task_type(body: &str) -> bool {
    ["task.RecvChannel<", "task.SendChannel<", "task.Request<"]
        .iter()
        .any(|needle| body.contains(needle))
}

/// `uses`, with a default `task = "htl.task"` entry prepended when `body` contains a type
/// `htl.task` declares (see [`mentions_htl_task_type`]) and no entry already names
/// `task`. Read off the text rather than tracked through each signature, so a nested
/// record's field and a method's return are covered by the one test. `what` names the
/// item an error is about (`` host_module `d` `` / `` TealRecord `Run` ``) — `htl dts`
/// reports an error with no file or source span of its own, so the message is the only
/// place a reader finds which declaration it was.
///
/// A `uses` entry already naming `task` is left as written when it points at `htl.task`
/// itself (no second line) or when the declaration does not need `htl.task` at all; one
/// that names `task` for a *different* module while the declaration does need `htl.task`
/// is refused — a `.d.tl` cannot import two modules under the one local name, and nothing
/// says which of `task.RecvChannel<..>` and the user's own `task.*` the file meant.
fn with_task_default(what: &str, body: &str, uses: &[Use]) -> Result<Vec<Use>, String> {
    let needs_htl_task = mentions_htl_task_type(body);
    if let Some(existing) = uses.iter().find(|u| u.name == TASK_ALIAS) {
        return if needs_htl_task && existing.module != "htl.task" {
            Err(format!(
                "{what}: `uses` names `task` for \"{}\", but the declaration also needs \
                 `htl.task` (a channel or request of `htl::task`'s); name the `uses` \
                 entry something other than `task`, or point it at \"htl.task\"",
                existing.module
            ))
        } else {
            Ok(uses.to_vec())
        };
    }
    if !needs_htl_task {
        return Ok(uses.to_vec());
    }
    let mut out = Vec::with_capacity(uses.len() + 1);
    out.push(Use {
        name: TASK_ALIAS.to_string(),
        module: "htl.task".to_string(),
    });
    out.extend_from_slice(uses);
    Ok(out)
}

/// `local type name = require("module")` per `uses` entry, in the order written (the
/// default `task` entry [`with_task_default`] adds, when it applies, is first), followed
/// by a blank line; empty when there are none. The `type` form is the one that imports an
/// alias or an enum — a module whose value is only a type is "abstract" to Teal when
/// required as a value — and it imports a record just the same, so one form serves every
/// kind a `.d.tl` can return.
fn uses_header(uses: &[Use]) -> String {
    let mut s = String::new();
    for u in uses {
        s.push_str(&format!(
            "local type {} = require(\"{}\")\n",
            u.name, u.module
        ));
    }
    if !uses.is_empty() {
        s.push('\n');
    }
    s
}

// ---------------------------------------------------------------- records

/// The Teal shape a `#[derive(TealRecord)]` item lowers to (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordKind {
    /// A struct with named fields: `record N` with `(field, teal type)` in declaration
    /// order, crossing as a plain table.
    Record {
        /// The field's name on the Teal side and the Teal type it maps to — or, for a field
        /// marked `#[teal(as = "..")]`, the text it gives. The name is serde's: the field's
        /// `#[serde(rename)]`, else the type's `#[teal(rename_all)]` / `#[serde(rename_all)]`
        /// applied to the Rust name, else the Rust name (`r#type` read as `type`). It is the
        /// key the table carries and what a `FieldError` names; the declaration writes it
        /// bare when it is a Teal identifier and as `["name"]` when it is not. `as` says
        /// what a field is declared as, never what it is called. A field's `#[teal(..)]`
        /// takes `as` and the one word below; anything else is refused rather than ignored.
        fields: Vec<(String, String)>,
        /// The fields marked `#[teal(noyield)]`, by the name in `fields`, in declaration
        /// order: each is declared
        /// with a trailing bare `---@noyield` (the host reads the function off the table
        /// and calls it from C; the `host_module` macro doc states the rule).
        noyield: Vec<String>,
    },
    /// `struct N(T)`: `type N = T`, crossing as `T` does.
    Alias {
        /// The Teal type of the one field — already mapped, so `struct Id(String)` holds
        /// `string` rather than `String`.
        inner: String,
    },
    /// An enum of unit variants: `enum N "A" "B" end`, crossing as the variant name —
    /// as `#[teal(rename_all)]` / `#[teal(name)]` spell it, in declaration order.
    Enum {
        /// The words themselves, not the Rust identifiers: this is what the `enum` body
        /// lists and what a value crossing the boundary must be one of.
        variants: Vec<String>,
    },
    /// An enum with a data variant: one `where`-discriminated record per variant and
    /// `type N = N_A | N_B`, crossing as a table whose `kind` names the variant.
    Union {
        /// One per variant, in declaration order, which is the order the generated
        /// `type N = N_A | N_B` lists them in.
        variants: Vec<UnionVariant>,
    },
}

/// One variant of a data-carrying enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionVariant {
    /// The Rust name, which is also the record's (`Shape_InReview`) and the segment an
    /// error inside the variant is reported under (`Shape.InReview.h`): a Teal
    /// identifier cannot hold every `rename_all` spelling, and the record is what a
    /// caller narrows with by name.
    pub name: String,
    /// The word the `kind` tag carries: `where self.kind == "in_review"`, and what the
    /// value crossing the boundary must say. Equals `name` unless renamed.
    pub word: String,
    /// What it carries, and so which fields its record has beyond `kind` — the one thing
    /// that differs between the variants of a union whose records are otherwise alike.
    pub shape: VariantShape,
}

/// What a variant carries, and so which fields its record has beyond `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantShape {
    /// `A`: only `kind`.
    Unit,
    /// `B(T)`: `value: T`.
    Newtype(String),
    /// `C { x: U, .. }`: the fields as declared.
    Struct(Vec<(String, String)>),
}

/// One `#[derive(TealRecord)]` item, lowered: the shape it takes, the module text that
/// declares it, and the attributes that decided both.
#[derive(Debug, Clone)]
pub struct RecordDecl {
    /// The Teal name — `#[teal(name = "..")]` when given, the Rust identifier otherwise.
    /// What the declaration declares and what a caller writes.
    pub name: String,
    /// Which of the four shapes it lowered to, kept beside the text because a caller that
    /// nests this record needs the parts rather than the finished module.
    pub kind: RecordKind,
    /// Full module text: `local record NAME ... end  return NAME` (or the `enum` /
    /// `type` form; a union's is the top-level form, which only `DECL` ever holds).
    pub decl: String,
    /// What was parsed off the item: where to write it (`dts`), what to import (`uses`),
    /// how variants are spelled. Carried so the caller can act on them without parsing
    /// the attribute a second time.
    pub attrs: TealAttrs,
}

impl RecordDecl {
    /// `record N` / `enum N` / `type N`: how the declaration is named in reports, by the
    /// Teal keyword that declares `N` (a data-carrying enum is a `type`, the union).
    pub fn what(&self) -> String {
        let kw = match self.kind {
            RecordKind::Record { .. } => "record",
            RecordKind::Alias { .. } | RecordKind::Union { .. } => "type",
            RecordKind::Enum { .. } => "enum",
        };
        format!("{kw} {}", self.name)
    }
}

/// `(field, teal type)` in declaration order, and the fields marked `#[teal(noyield)]`.
type Fields = (Vec<(String, String)>, Vec<String>);

// ---------------------------------------------------------------- serde attributes
//
// What `#[derive(TealRecord)]` reads of an item's `#[serde(..)]`, and *every* refusal it
// makes of one. The refusals are kept together here on purpose: each later step of the
// serde work widens the read set by moving a word out of `SERDE_NOT_READ`, and the
// step that adds the remaining refusals (`with` and friends, one-way `skip`) gathers them
// in this same spot, so one place says what a serde type may and may not carry.

/// The serde words read, as a refusal lists them.
const SERDE_READ: &str = "rename_all, rename, alias, rename_all_fields";

/// serde words that do not change the shape of what crosses, accepted and left to serde.
const SERDE_PASS: &[&str] = &["crate", "bound", "deny_unknown_fields", "expecting"];

/// serde words that change the shape of what crosses and are not read yet. A type that
/// carries one is refused: a declaration that ignored it would say a shape the value does
/// not have.
const SERDE_NOT_READ: &[&str] = &[
    "skip",
    "skip_serializing",
    "skip_deserializing",
    "skip_serializing_if",
    "default",
    "flatten",
    "tag",
    "content",
    "untagged",
    "transparent",
    "with",
    "serialize_with",
    "deserialize_with",
    "from",
    "into",
    "try_from",
    "remote",
    "other",
    "borrow",
    "getter",
    "variant_identifier",
    "field_identifier",
];

/// Where a `#[serde(..)]` sits, which decides which of the read words it may carry.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SerdePlace {
    Container,
    Field,
    Variant,
}

/// The names one `#[serde(..)]` (or several, merged) says.
#[derive(Default)]
struct SerdeNames {
    /// `rename = ".."` on a field or a variant.
    rename: Option<String>,
    /// `rename_all = ".."` on a struct (its fields) or an enum (its variants).
    rename_all: Option<RenameRule>,
    /// `rename_all_fields = ".."` on an enum: the fields of its struct variants.
    rename_all_fields: Option<RenameRule>,
    /// `alias = ".."` on a field, in the order written: extra keys accepted on the way in.
    aliases: Vec<String>,
}

/// `#[serde(..)]` word `word` naming the type one way and another the other way.
fn serde_two_names(word: &str, ser: Option<&str>, de: Option<&str>) -> String {
    let said = |v: Option<&str>| match v {
        Some(v) => format!("{v:?}"),
        None => "nothing (so the Rust name)".to_string(),
    };
    format!(
        "`{word}` says serialize = {} and deserialize = {}: one type cannot carry two names \
         (the `.d.tl` declares one); write two types, or one name: `{word} = \"..\"`",
        said(ser),
        said(de)
    )
}

/// A serde word that changes the shape and is not read yet.
fn serde_not_read(word: &str) -> String {
    format!(
        "`#[serde({word})]` is not read by TealRecord; the attributes it reads are {SERDE_READ} \
         (and it leaves {} to serde)",
        SERDE_PASS.join(", ")
    )
}

/// A read serde word written where TealRecord does not read it.
fn serde_misplaced(word: &str, place: SerdePlace) -> String {
    let (here, goes) = match (word, place) {
        ("alias", SerdePlace::Variant) => ("a variant", "a field"),
        ("alias", _) => ("a type", "a field"),
        ("rename_all", SerdePlace::Variant) => (
            "a variant",
            "the struct or the enum (`rename_all_fields` on the enum renames the fields of its struct variants)",
        ),
        ("rename_all" | "rename_all_fields", _) => ("a field", "the struct or the enum"),
        _ => ("here", "elsewhere"),
    };
    format!("`#[serde({word})]` on {here} is not read by TealRecord; it reads it on {goes}")
}

/// A renamed field whose name a Teal record cannot hold as a key.
fn serde_unrepresentable(name: &str) -> String {
    format!(
        "the field crosses as {name:?}, which a Teal record cannot declare: a key that is not a \
         Teal identifier is written `[\"..\"]`, and that form holds no `\"`, `\\` or control \
         character and is not empty; rename it to a name without them"
    )
}

/// `rename = ".."` / `rename_all = ".."`, or the `(serialize = .., deserialize = ..)` form
/// when both sides say the same word; anything else is refused.
fn serde_name_value(meta: &syn::meta::ParseNestedMeta, word: &str) -> syn::Result<String> {
    if meta.input.peek(Token![=]) {
        let s: syn::LitStr = meta.value()?.parse()?;
        return Ok(s.value());
    }
    if meta.input.peek(syn::token::Paren) {
        let (mut ser, mut de) = (None, None);
        meta.parse_nested_meta(|m| {
            let s: syn::LitStr = m.value()?.parse()?;
            if m.path.is_ident("serialize") {
                ser = Some(s.value());
            } else if m.path.is_ident("deserialize") {
                de = Some(s.value());
            } else {
                return Err(m.error("expected `serialize` or `deserialize`"));
            }
            Ok(())
        })?;
        return match (ser, de) {
            (Some(a), Some(b)) if a == b => Ok(a),
            (a, b) => Err(meta.error(serde_two_names(word, a.as_deref(), b.as_deref()))),
        };
    }
    Err(meta.error(format!("`{word}` takes a string: `{word} = \"..\"`")))
}

/// Consume a passed-through word's value, whatever form serde gives it.
fn serde_skip_value(meta: &syn::meta::ParseNestedMeta) -> syn::Result<()> {
    if meta.input.peek(Token![=]) {
        meta.value()?.parse::<Expr>()?;
    } else if meta.input.peek(syn::token::Paren) {
        meta.parse_nested_meta(|m| serde_skip_value(&m))?;
    }
    Ok(())
}

/// Every `#[serde(..)]` in `attrs`, read for names at `place`; `at` names the item in a
/// refusal (`Call`, `Call.call_id`, `State::InReview`). See the section comment above for
/// which words are read, passed through and refused.
fn serde_names(attrs: &[Attribute], place: SerdePlace, at: &str) -> Result<SerdeNames, String> {
    let mut out = SerdeNames::default();
    for a in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        let Meta::List(list) = &a.meta else {
            return Err(format!(
                "TealRecord: {at}: cannot read `{}`: serde's attribute takes a list, `#[serde(..)]`",
                if matches!(a.meta, Meta::Path(_)) {
                    "#[serde]".to_string()
                } else {
                    "#[serde = ..]".to_string()
                }
            ));
        };
        // A refusal of ours is carried out of the parser here, so it is told apart from a
        // syntax error, which is reported quoting the attribute instead.
        let mut refused: Option<String> = None;
        let parsed = a.parse_nested_meta(|meta| {
            let word = meta
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            let mut refuse = |msg: String| {
                refused = Some(msg.clone());
                Err(meta.error(msg))
            };
            match (word.as_str(), place) {
                ("rename", SerdePlace::Container) => serde_skip_value(&meta),
                ("rename", _) => match serde_name_value(&meta, "rename") {
                    Ok(n) => {
                        out.rename = Some(n);
                        Ok(())
                    }
                    Err(e) => refuse(e.to_string()),
                },
                ("rename_all", SerdePlace::Container)
                | ("rename_all_fields", SerdePlace::Container) => {
                    let rule = match serde_name_value(&meta, &word) {
                        Ok(n) => n,
                        Err(e) => return refuse(e.to_string()),
                    };
                    match RenameRule::parse(&rule) {
                        Ok(r) if word == "rename_all" => out.rename_all = Some(r),
                        Ok(r) => out.rename_all_fields = Some(r),
                        Err(e) => return refuse(e.replace("`rename_all`", &format!("`{word}`"))),
                    }
                    Ok(())
                }
                ("alias", SerdePlace::Field) => {
                    if !meta.input.peek(Token![=]) {
                        return refuse("`alias` takes a string: `alias = \"..\"`".into());
                    }
                    let s: syn::LitStr = meta.value()?.parse()?;
                    out.aliases.push(s.value());
                    Ok(())
                }
                ("alias" | "rename_all" | "rename_all_fields", _) => {
                    refuse(serde_misplaced(&word, place))
                }
                (w, _) if SERDE_PASS.contains(&w) => serde_skip_value(&meta),
                (w, _) if SERDE_NOT_READ.contains(&w) => refuse(serde_not_read(w)),
                (w, _) => refuse(format!(
                    "`#[serde({w})]` is not a serde word TealRecord knows; the attributes it reads \
                     are {SERDE_READ} (and it leaves {} to serde)",
                    SERDE_PASS.join(", ")
                )),
            }
        });
        if let Err(e) = parsed {
            return Err(match refused {
                Some(msg) => format!("TealRecord: {at}: {msg}"),
                None => format!(
                    "TealRecord: {at}: cannot read `#[serde({})]`: {e}",
                    list.tokens
                ),
            });
        }
    }
    Ok(out)
}

/// The keys a field's `#[serde(alias = "..")]` adds on the way in, in the order written:
/// what the derived `from_lua` tries, after the field's own name, when that is nil. Not
/// declared and never written. Empty for a field without one (and for a `#[serde(..)]`
/// [`record_decl`] would refuse, which never reaches the code that asks).
pub fn serde_aliases(attrs: &[Attribute]) -> Vec<String> {
    serde_names(attrs, SerdePlace::Field, "")
        .map(|n| n.aliases)
        .unwrap_or_default()
}

/// A Rust identifier's name as serde reads it: `r#type` is `type`.
fn unraw(ident: &syn::Ident) -> String {
    let s = ident.to_string();
    s.strip_prefix("r#").map(str::to_string).unwrap_or(s)
}

/// `true` for a name a Teal record can hold as a key: a Teal identifier is written bare,
/// anything else as `["name"]`, which takes any text but a `"`, a `\` or a control
/// character (the declaration writes the text as it is, with no escapes).
fn representable_key(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|c| c == '"' || c == '\\' || c.is_control())
}

/// How a field is written in a record body: `name` when it is a Teal identifier,
/// `["name"]` otherwise (Teal's own syntax for a key that is not one, `tl.lua`'s record
/// body parser).
fn teal_field_key(name: &str) -> String {
    if is_teal_ident(name) {
        name.to_string()
    } else {
        format!("[\"{name}\"]")
    }
}

/// The fields of a struct record (`struct_record` = `true`) or of a data variant, and which
/// of them carry `#[teal(noyield)]` (always none for a variant, whose fields cannot take it).
/// `uses` is what a qualified field type crosses dotted against ([`teal_type_in`]): the
/// record's own at the top level, the host's for one nested through `records = [..]`.
///
/// Each field is named as serde names it: its `#[serde(rename = "..")]`, else `rule` (the
/// struct's `#[teal(rename_all)]` / `#[serde(rename_all)]`, or the enum's
/// `#[serde(rename_all_fields)]` for a variant) applied to the Rust name, else the Rust
/// name. Two fields under one name, or an alias that is another field's name, are refused.
///
/// A field's Teal type is the one [`teal_type_in`] maps its Rust type to, unless the field
/// says `#[teal(as = "..")]`: then it is that text, written as given, and the Rust type is
/// not mapped at all — the attribute is for the type whose `IntoLua` / `FromLua` the host
/// wrote itself, which the type table cannot see into.
fn record_fields(
    fields: &syn::FieldsNamed,
    self_name: &str,
    struct_record: bool,
    rule: Option<RenameRule>,
    uses: &[Use],
) -> Result<Fields, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut noyield = Vec::new();
    let mut aliases: Vec<(String, String)> = Vec::new();
    for f in &fields.named {
        let ident = f.ident.as_ref().unwrap();
        let fi = unraw(ident);
        let fa = field_attrs(&f.attrs, self_name, &fi)?;
        let sn = serde_names(&f.attrs, SerdePlace::Field, &format!("{self_name}.{fi}"))?;
        let name = match sn.rename {
            Some(n) => n,
            None => rule.map_or_else(|| fi.clone(), |r| r.apply_to_field(&fi)),
        };
        for n in std::iter::once(&name).chain(&sn.aliases) {
            if !representable_key(n) {
                return Err(format!(
                    "TealRecord: {self_name}.{fi}: {}",
                    serde_unrepresentable(n)
                ));
            }
        }
        if let Some((other, _)) = out.iter().find(|(n, _)| *n == name) {
            return Err(format!(
                "TealRecord: {self_name}.{fi}: crosses as {name:?}, which another field \
                 (`{other}`) already does; give one a `#[serde(rename = \"..\")]` of its own"
            ));
        }
        if fa.noyield {
            // A variant's field is never called through by the host: the table the host
            // reads a callback off is a record it was handed, not one variant of a union.
            if !struct_record {
                return Err(format!(
                    "TealRecord: {self_name}.{fi}: `#[teal(noyield)]` applies to a field of a struct \
                     record; a variant's fields take only `as`"
                ));
            }
            if !is_function(&f.ty) {
                return Err(format!(
                    "TealRecord: {self_name}.{fi}: `#[teal(noyield)]` on a field that is not a \
                     `Function`: the word says the host calls the Lua function it reads from the field from C"
                ));
            }
            noyield.push(name.clone());
        }
        let tt = match fa.as_type {
            Some(t) => t,
            None => teal_type_in(&f.ty, self_name, uses)?,
        };
        aliases.extend(sn.aliases.into_iter().map(|a| (a, fi.clone())));
        out.push((name, tt));
    }
    // An alias is read only when the field's own key is nil, so one equal to another
    // field's name would hand that field's value to this one.
    for (a, fi) in &aliases {
        if out.iter().any(|(n, _)| n == a) {
            return Err(format!(
                "TealRecord: {self_name}.{fi}: `#[serde(alias = {a:?})]` is the name another \
                 field crosses as; an alias has to be a key no field is declared under"
            ));
        }
    }
    Ok((out, noyield))
}

/// What a record field's `#[teal(..)]` says.
#[derive(Default)]
struct FieldAttrs {
    /// `#[teal(as = "..")]`: the Teal type the field is declared as, verbatim.
    as_type: Option<String>,
    /// `#[teal(noyield)]`: the host calls the function it reads from the field from C.
    noyield: bool,
}

/// The words a record field's `#[teal(..)]` takes, as a refusal names them.
const FIELD_WORDS: &str = "`as = \"<teal type>\"` or `noyield`";

/// `#[teal(..)]` on field `fname` of `rec`. A field takes two words: `as = ".."`, the Teal
/// type it is declared as when the Rust type does not say (see [`record_fields`]), and
/// `noyield`. Any other word, a word in the wrong form, or the attribute with no word is
/// refused: a word nobody reads is worse than a refusal. Names are not among them — a
/// field is named by `#[serde(rename)]` / `rename_all` ([`record_fields`]).
fn field_attrs(attrs: &[Attribute], rec: &str, fname: &str) -> Result<FieldAttrs, String> {
    let mut out = FieldAttrs::default();
    let mut found = false;
    let mut said = false;
    for a in attrs.iter().filter(|a| a.path().is_ident("teal")) {
        found = true;
        let Meta::List(_) = &a.meta else {
            return Err(format!(
                "TealRecord: {rec}.{fname}: `#[teal(..)]` on a record field takes {FIELD_WORDS}"
            ));
        };
        // `parse_nested_meta` rather than a list of `Meta`: `as` is a keyword, and only the
        // nested-meta parser takes a keyword as a word's name.
        a.parse_nested_meta(|meta| {
            let fail = |msg: String| Err(meta.error(msg));
            if meta.path.is_ident("noyield") {
                if !(meta.input.is_empty() || meta.input.peek(Token![,])) {
                    return fail("`noyield` takes no value".into());
                }
                out.noyield = true;
                said = true;
                return Ok(());
            }
            if meta.path.is_ident("as") {
                let text = if meta.input.peek(Token![=]) {
                    match meta.value()?.parse::<Expr>()? {
                        Expr::Lit(l) => lit_str(&l.lit).ok(),
                        _ => None,
                    }
                } else {
                    None
                };
                let Some(text) = text else {
                    return fail(
                        "`as` takes the Teal type as a string: `#[teal(as = \"<teal type>\")]`"
                            .into(),
                    );
                };
                if text.trim().is_empty() {
                    return fail(
                        "`#[teal(as = \"\")]` names no type; write the Teal type the field \
                         crosses as, `as = \"string\"` say"
                            .into(),
                    );
                }
                if out.as_type.is_some() {
                    return fail("`as` is given twice".into());
                }
                out.as_type = Some(text);
                said = true;
                return Ok(());
            }
            let key = meta
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            fail(format!(
                "`#[teal(..)]` on a record field takes {FIELD_WORDS}, got `{key}`"
            ))
        })
        .map_err(|e| format!("TealRecord: {rec}.{fname}: {e}"))?;
    }
    if found && !said {
        return Err(format!(
            "TealRecord: {rec}.{fname}: `#[teal(..)]` on a record field takes {FIELD_WORDS}"
        ));
    }
    Ok(out)
}

/// The kind a struct lowers to: named fields -> record, one unnamed field -> alias. `rule`
/// renames the fields ([`record_fields`]).
fn struct_kind(
    st: &ItemStruct,
    name: &str,
    rule: Option<RenameRule>,
    uses: &[Use],
) -> Result<RecordKind, String> {
    match &st.fields {
        syn::Fields::Named(fields) => {
            let (fields, noyield) = record_fields(fields, name, true, rule, uses)?;
            Ok(RecordKind::Record { fields, noyield })
        }
        syn::Fields::Unnamed(u) if u.unnamed.len() == 1 => Ok(RecordKind::Alias {
            inner: teal_type_in(&u.unnamed[0].ty, name, uses)?,
        }),
        syn::Fields::Unnamed(_) => Err(format!(
            "TealRecord: `{name}` is a tuple struct with more than one field; only a newtype (`struct {name}(T)`) lowers to a Teal type"
        )),
        syn::Fields::Unit => Err(format!(
            "TealRecord: `{name}` is a unit struct and has nothing to declare"
        )),
    }
}

/// The Teal spelling of one variant: `#[teal(name = "..")]` on it, else its
/// `#[serde(rename = "..")]`, else `rule` (the enum's `#[teal(rename_all)]`, else its
/// `#[serde(rename_all)]`), else the Rust name. The word on the variant beats the enum's
/// rule, and at each level `teal` beats `serde`. Nothing but `name` may be written in a
/// variant's `#[teal(..)]` (a variant has no `.d.tl` of its own to ask for, and
/// `rename_all` over one variant is what `name` already is).
fn variant_word(
    v: &syn::Variant,
    rule: Option<RenameRule>,
    enum_name: &str,
) -> Result<String, String> {
    let attrs = parse_teal_attrs(&v.attrs)
        .map_err(|e| format!("TealRecord: {enum_name}::{}: {e}", v.ident))?;
    let sn = serde_names(
        &v.attrs,
        SerdePlace::Variant,
        &format!("{enum_name}::{}", v.ident),
    )?;
    if attrs.rename_all.is_some()
        || attrs.dts.is_some()
        || !attrs.uses.is_empty()
        || !attrs.records.is_empty()
        || attrs.errors.is_some()
    {
        return Err(format!(
            "TealRecord: {enum_name}::{}: only `#[teal(name = \"..\")]` applies to a variant \
             (`rename_all` goes on the enum)",
            v.ident
        ));
    }
    Ok(match attrs.name.or(sn.rename) {
        Some(n) => n,
        None => match rule {
            Some(r) => r.apply(&unraw(&v.ident)),
            None => unraw(&v.ident),
        },
    })
}

/// Two variants that reach Teal as the same word are a declaration that cannot say which
/// one a value meant — an `enum` listing it twice, or two records with the same `where`
/// clause — so it is refused, naming both.
fn reject_duplicate_words(en: &ItemEnum, words: &[String], enum_name: &str) -> Result<(), String> {
    for (i, w) in words.iter().enumerate() {
        if let Some(j) = words[..i].iter().position(|p| p == w) {
            return Err(format!(
                "TealRecord: {enum_name}::{} and {enum_name}::{} both reach Teal as {w:?}; \
                 give one a `#[teal(name = \"..\")]` of its own",
                en.variants[j].ident, en.variants[i].ident
            ));
        }
    }
    Ok(())
}

/// The kind an enum lowers to: all unit variants -> `enum`, otherwise a union of
/// `where`-discriminated records. `rule` spells the variants ([`variant_word`]),
/// `fields_rule` (`#[serde(rename_all_fields)]`) the fields of its struct variants. `uses`
/// is what a variant's field types cross dotted against ([`teal_type_in`]).
fn enum_kind(
    en: &ItemEnum,
    name: &str,
    rule: Option<RenameRule>,
    fields_rule: Option<RenameRule>,
    uses: &[Use],
) -> Result<RecordKind, String> {
    if en.variants.is_empty() {
        return Err(format!(
            "TealRecord: `{name}` has no variants and has nothing to declare"
        ));
    }
    let words: Vec<String> = en
        .variants
        .iter()
        .map(|v| variant_word(v, rule, name))
        .collect::<Result<_, _>>()?;
    reject_duplicate_words(en, &words, name)?;
    if en
        .variants
        .iter()
        .all(|v| matches!(v.fields, syn::Fields::Unit))
    {
        return Ok(RecordKind::Enum { variants: words });
    }
    let mut variants = Vec::new();
    for (v, word) in en.variants.iter().zip(words) {
        let vname = v.ident.to_string();
        let shape = match &v.fields {
            syn::Fields::Unit => VariantShape::Unit,
            syn::Fields::Unnamed(u) if u.unnamed.len() == 1 => {
                VariantShape::Newtype(teal_type_in(&u.unnamed[0].ty, name, uses)?)
            }
            syn::Fields::Unnamed(_) => {
                return Err(
                    "TealRecord: tuple variants with more than one field are not supported"
                        .to_string(),
                );
            }
            syn::Fields::Named(fields) => {
                let fs = record_fields(fields, name, false, fields_rule, uses)?.0;
                // The tag is `kind`, on every variant record; a payload field crossing
                // under that name would be declared twice and could not survive a round
                // trip.
                if fs.iter().any(|(f, _)| f == "kind") {
                    return Err(format!(
                        "TealRecord: {name}::{vname}: a field named `kind` collides with the variant tag"
                    ));
                }
                VariantShape::Struct(fs)
            }
        };
        variants.push(UnionVariant {
            name: vname,
            word,
            shape,
        });
    }
    Ok(RecordKind::Union { variants })
}

/// Attributes and name of a `#[derive(TealRecord)]` item, its [`RecordKind`] not built yet
/// — that is [`record_kind`], which takes a `uses` list of the caller's choosing: the
/// item's own at the top level ([`record_parts`]), the host's for a record reached
/// through `records = [..]` ([`nested_record_decls`]).
fn record_attrs(item: &Item) -> Result<(TealAttrs, String), String> {
    match item {
        Item::Struct(st) => Ok((parse_teal_attrs(&st.attrs)?, st.ident.to_string())),
        Item::Enum(en) => Ok((parse_teal_attrs(&en.attrs)?, en.ident.to_string())),
        _ => Err("TealRecord: only structs and enums are supported".into()),
    }
}

/// The [`RecordKind`] `item` (named `name`) lowers to, its field types resolved against
/// `uses` ([`teal_type_in`]). `rename_all` is the item's `#[teal(rename_all)]`, which wins
/// over its `#[serde(rename_all)]` (read here): on a struct it spells the fields, on an
/// enum the variants.
fn record_kind(
    item: &Item,
    name: &str,
    rename_all: Option<RenameRule>,
    uses: &[Use],
) -> Result<RecordKind, String> {
    let attrs = match item {
        Item::Struct(st) => &st.attrs,
        Item::Enum(en) => &en.attrs,
        _ => unreachable!(),
    };
    let sn = serde_names(attrs, SerdePlace::Container, name)?;
    let rule = rename_all.or(sn.rename_all);
    match item {
        Item::Struct(st) => {
            if sn.rename_all_fields.is_some() {
                return Err(format!(
                    "TealRecord: {name}: `#[serde(rename_all_fields)]` is an enum's (the fields \
                     of its struct variants); on a struct, `rename_all` renames the fields"
                ));
            }
            struct_kind(st, name, rule, uses)
        }
        Item::Enum(en) => enum_kind(en, name, rule, sn.rename_all_fields, uses),
        _ => unreachable!(),
    }
}

/// Name, attributes and kind of a `#[derive(TealRecord)]` item, its own `uses` resolving
/// its field types.
fn record_parts(item: &Item) -> Result<(String, TealAttrs, RecordKind), String> {
    let (attrs, ident) = record_attrs(item)?;
    let name = attrs.name.clone().unwrap_or(ident);
    let kind = record_kind(item, &name, attrs.rename_all, &attrs.uses)?;
    Ok((name, attrs, kind))
}

/// Field lines of a variant record: `kind` first, then what the variant carries.
fn variant_fields(v: &UnionVariant) -> Vec<(String, String)> {
    let mut fields = vec![("kind".to_string(), "string".to_string())];
    match &v.shape {
        VariantShape::Unit => {}
        VariantShape::Newtype(t) => fields.push(("value".to_string(), t.clone())),
        VariantShape::Struct(fs) => fields.extend(fs.iter().cloned()),
    }
    fields
}

/// The declaration at `indent` (`""` for a module of its own, `"   "` nested in a host
/// module record). `local` prefixes top-level declarations only.
fn kind_decl(name: &str, kind: &RecordKind, indent: &str) -> String {
    let local = if indent.is_empty() { "local " } else { "" };
    let inner = format!("{indent}   ");
    let mut s = String::new();
    match kind {
        RecordKind::Record { fields, noyield } => {
            s.push_str(&format!("{indent}{local}record {name}\n"));
            for (f, t) in fields {
                let marker = if noyield.contains(f) {
                    " ---@noyield"
                } else {
                    ""
                };
                s.push_str(&format!("{inner}{}: {t}{marker}\n", teal_field_key(f)));
            }
            s.push_str(&format!("{indent}end\n"));
        }
        RecordKind::Alias { inner: t } => {
            s.push_str(&format!("{indent}{local}type {name} = {t}\n"));
        }
        RecordKind::Enum { variants } => {
            s.push_str(&format!("{indent}{local}enum {name}\n"));
            for v in variants {
                s.push_str(&format!("{inner}\"{v}\"\n"));
            }
            s.push_str(&format!("{indent}end\n"));
        }
        RecordKind::Union { variants } => {
            for v in variants {
                s.push_str(&format!("{indent}{local}record {name}_{}\n", v.name));
                s.push_str(&format!("{inner}where self.kind == \"{}\"\n", v.word));
                for (f, t) in variant_fields(v) {
                    s.push_str(&format!("{inner}{}: {t}\n", teal_field_key(&f)));
                }
                s.push_str(&format!("{indent}end\n"));
            }
            let members: Vec<String> = variants
                .iter()
                .map(|v| format!("{name}_{}", v.name))
                .collect();
            s.push_str(&format!(
                "{indent}{local}type {name} = {}\n",
                members.join(" | ")
            ));
        }
    }
    s
}

/// Declaration for a `#[derive(TealRecord)]` struct or enum.
///
/// The text is the module form (`local ... return NAME`). A data-carrying enum gets it
/// too — it is what its `DECL` constant holds — but asking for it as a file
/// (`#[teal(dts = ..)]`) is refused: see the module docs for why it has to be nested.
pub fn record_decl(item: &Item) -> Result<RecordDecl, String> {
    let (name, attrs, kind) = record_parts(item)?;
    if attrs.dts.is_some() && matches!(kind, RecordKind::Union { .. }) {
        return Err(format!(
            "TealRecord: `{name}` has data-carrying variants and cannot be a `.d.tl` module of its own \
             (a caller narrows it with `is {name}_<Variant>`, and a module exports one name); \
             drop `dts` and declare it nested in the host module with `records = [{name}]`"
        ));
    }
    let mut body = kind_decl(&name, &kind, "");
    body.push_str(&format!("\nreturn {name}\n"));
    let uses = with_task_default(&format!("TealRecord `{name}`"), &body, &attrs.uses)?;
    let decl = format!("{}{body}", uses_header(&uses));
    Ok(RecordDecl {
        name,
        kind,
        decl,
        attrs,
    })
}

/// Find a struct or enum by name in a file's items (recursing into inline modules).
pub fn find_item<'a>(items: &'a [Item], name: &str) -> Option<&'a Item> {
    for it in items {
        match it {
            Item::Struct(s) if s.ident == name => return Some(it),
            Item::Enum(e) if e.ident == name => return Some(it),
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content
                    && let Some(s) = find_item(inner, name)
                {
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

/// Find a struct or enum named `name` directly among `items`, without recursing into any
/// nested `mod`. Unlike [`find_item`] (which a bare `records = [Point]` entry still
/// uses, unchanged): a module-qualified entry (`records = [geom::Point]`) names `geom`
/// *and* `Point`, so it resolves to the record `geom` declares itself, not to one some
/// further nesting inside `geom` happens to reach by the same name
/// (`mod geom { mod deeper { struct Point; } }` is not `geom::Point`).
fn find_item_direct<'a>(items: &'a [Item], name: &str) -> Option<&'a Item> {
    items.iter().find(|it| match it {
        Item::Struct(s) => s.ident == name,
        Item::Enum(e) => e.ident == name,
        _ => false,
    })
}

/// A `records` entry's `Item`, already found — nested under its Rust name (`records =
/// [X]` names the item, and the host's signatures say `X` too), refusing a
/// `#[teal(name = ..)]` on it: that could satisfy neither (`Self` inside the item would
/// already have been mapped to the rename), so it is refused rather than half-applied.
/// `uses` is the host's — a nested record's field types resolve against the `uses` the
/// host module itself declared, not against anything the nested item might declare of
/// its own, so `task::Job` nested beside a host's `uses = [task = ".."]` crosses the same
/// way whichever `#[host_module]` method also returns it. A `#[teal(uses = ..)]` on the
/// nested item itself writes no import line of its own either way: only the host
/// module's `uses_header` is ever emitted, once, at the top of the file.
fn nested_item_decl(it: &Item, name: &str, uses: &[Use]) -> Result<String, String> {
    let (attrs, ident) = record_attrs(it)?;
    let renamed = attrs.name.clone().unwrap_or(ident);
    if attrs.name.is_some() {
        return Err(format!(
            "host_module: `{name}` is nested through `records` and cannot be renamed \
             (`#[teal(name = \"{renamed}\")]`); to declare it as `{renamed}`, give it a \
             `.d.tl` of its own (`#[teal(dts = ..)]`) and import it with `uses = [{renamed}]`"
        ));
    }
    let kind = record_kind(it, &renamed, attrs.rename_all, uses)?;
    Ok(kind_decl(name, &kind, "   "))
}

/// The directory `mod module;` in `file` resolves against. Rust treats four kinds of file
/// as a crate (or crate-like) root, each the place its own submodules live next to: a
/// file literally named `lib.rs`, `main.rs` or `mod.rs`; a file directly under
/// `src/bin/` (each a binary crate root of its own); or a file directly under a
/// crate-root `tests/`, `examples/` or `benches/` (each an integration test / example /
/// benchmark crate root of its own) — *crate-root* meaning the directory holding it is a
/// sibling of the package's `Cargo.toml`, which is what tells `src/tests/helpers.rs`'s
/// `tests` (nested under `src/`, nowhere near `Cargo.toml`) from a real `tests/` at the
/// package root: the first is a plain submodule of `src/hostio.rs`'s kind, not a crate
/// root, even though the directory is spelled the same way. Any other file — `src/
/// hostio.rs` say — is a non-root module, and *its* submodules live in a directory named
/// after its own stem next to it: `src/hostio.rs`'s `mod geom;` is `src/hostio/geom.rs`,
/// not `src/geom.rs`; `src/tests/helpers.rs`'s is `src/tests/helpers/geom.rs` for the
/// same reason. This knows nothing of a `[lib] path` override or another custom crate
/// root in `Cargo.toml`, either of which could move a crate root somewhere these rules do
/// not predict.
fn mod_base_dir(file: &Path) -> PathBuf {
    let dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let is_root_name = matches!(
        file.file_name().and_then(|s| s.to_str()),
        Some("mod.rs" | "lib.rs" | "main.rs")
    );
    let dir_name = dir.file_name().and_then(|s| s.to_str());
    // `src/bin/` is always under `src/` by convention; a nested `bin` dir elsewhere (a
    // non-root module's own `bin` submodule, say) is not a crate root.
    let in_src_bin = dir_name == Some("bin")
        && dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            == Some("src");
    // `tests` / `examples` / `benches` are crate roots only at the package root — a
    // directory of the same name nested deeper (`src/tests/`) is a plain submodule
    // directory, told apart by whether ITS parent holds the package's `Cargo.toml`.
    let in_package_root_dir = matches!(dir_name, Some("tests" | "examples" | "benches"))
        && dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file());
    if is_root_name || in_src_bin || in_package_root_dir {
        return dir.to_path_buf();
    }
    match file.file_stem().and_then(|s| s.to_str()) {
        Some(stem) if !stem.is_empty() => dir.join(stem),
        _ => dir.to_path_buf(),
    }
}

/// `records = [module::..]`: the items `module` declares — inline (`mod module { .. }`,
/// nothing further to read) or, for `mod module;` with no body of its own, read from the
/// module's file ([`mod_base_dir`]) — and, when a file was read, its path in both forms a
/// caller needs: as read (what a message names it by, through `display_path`, never
/// canonicalized — a message says what was tried, not a resolved form of it) and
/// canonicalized where possible, for the caller to track with an `include_str!`
/// ([`HostDecl::record_files`]). Refuses a `#[path = ".."]` on `mod module`: `records`
/// follows the module Rust would resolve `mod module;` to on its own, not wherever
/// `#[path]` points it instead, and reading the wrong file silently (or refusing to find
/// anything there) would be worse than refusing the attribute outright. `name` is only
/// for the messages — which record a caller was after when this module needed resolving
/// — the caller may resolve more than one record out of the same module, and each should
/// read this once (see `nested_record_decls`'s cache).
/// A module's items, resolved by [`resolve_module_items`], and — when a file was read
/// for it — that file's path in both forms a caller needs: as read (`.0`, for messages)
/// and canonicalized where possible (`.1`, for `record_files`'s `include_str!`).
type ModuleItems = (Vec<Item>, Option<(PathBuf, PathBuf)>);

fn resolve_module_items(
    items: &[Item],
    current_file: Option<&Path>,
    module: &str,
    name: &str,
) -> Result<ModuleItems, String> {
    let m = items
        .iter()
        .find_map(|it| match it {
            Item::Mod(m) if m.ident == module => Some(m),
            _ => None,
        })
        .ok_or_else(|| {
            format!(
                "host_module: no `mod {module}` in this file (`records = [{module}::{name}]` \
                 names a module declared in the same file as the host)"
            )
        })?;
    if m.attrs.iter().any(|a| a.path().is_ident("path")) {
        return Err(format!(
            "host_module: `#[path]` on `mod {module}` is not followed by `records`"
        ));
    }
    if let Some((_, inner)) = &m.content {
        return Ok((inner.clone(), None));
    }
    let current_file = current_file.ok_or_else(|| {
        format!(
            "host_module: `records = [{module}::{name}]` needs the current file's path to \
             find `mod {module}`'s file (unavailable in this expansion)"
        )
    })?;
    let base = mod_base_dir(current_file);
    let candidates = [
        base.join(format!("{module}.rs")),
        base.join(module).join("mod.rs"),
    ];
    let path = candidates
        .iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            format!(
                "host_module: no file for `mod {module};` (tried {} and {})",
                crate::diagnostic::display_path(&candidates[0]),
                crate::diagnostic::display_path(&candidates[1]),
            )
        })?
        .clone();
    let src = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "host_module: reading {}: {e}",
            crate::diagnostic::display_path(&path)
        )
    })?;
    let file = syn::parse_file(&src).map_err(|e| {
        format!(
            "host_module: parsing {}: {e}",
            crate::diagnostic::display_path(&path)
        )
    })?;
    // `canonicalize` resolves symlinks (and so matches whatever path a reader later
    // compares this one against, the #424 class), but needs the file to exist; the
    // `is_file()` check just above makes that likely, not certain (another process could
    // remove it in between), so a failure falls back to `std::path::absolute` — lexical,
    // not touching the filesystem again, so it cannot fail the same way — rather than
    // risking a path relative to some other process's cwd reaching `include_str!`. Either
    // way `path` itself — what was actually tried, not a resolved form of it — is kept
    // for messages.
    let tracked = std::fs::canonicalize(&path)
        .or_else(|_| std::path::absolute(&path))
        .unwrap_or_else(|_| path.clone());
    Ok((file.items, Some((path, tracked))))
}

/// Every `records = [..]` entry's nested declaration, in order, and every file a
/// module-qualified one read (for [`HostDecl::record_files`]) — empty when every entry is
/// bare, the same as before module-qualified entries existed. Two entries naming the same
/// module (`records = [geom::Point, geom::Size]`) read and parse it once: `modules`
/// caches [`resolve_module_items`]'s result by module name, and only a cache miss adds to
/// `record_files`.
fn nested_record_decls(
    records: &[RecordRef],
    file_items: Option<&[Item]>,
    file_path: Option<&Path>,
    uses: &[Use],
) -> Result<(Vec<String>, Vec<PathBuf>), String> {
    if records.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let items = file_items.ok_or_else(|| {
        "host_module: `records` needs the source file (unavailable in this expansion); \
         declare the record in its own module and use `uses` instead"
            .to_string()
    })?;
    let mut out = Vec::new();
    let mut record_files = Vec::new();
    let mut modules: HashMap<String, ModuleItems> = HashMap::new();
    for r in records {
        let it = match &r.module {
            None => find_item(items, &r.name).cloned().ok_or_else(|| {
                format!(
                    "host_module: `{}` not found in this file; a record in another module is \
                     named `records = [<module>::{}]` (a `mod <module>;` declared in this \
                     file), or `uses = [{}]` to import it on its own (`host.{}` becomes `{}` \
                     from its own module, so every `.tl` that wrote `host.{}` has to be \
                     rewritten)",
                    r.name, r.name, r.name, r.name, r.name, r.name
                )
            })?,
            Some(module) => {
                let resolved = match modules.entry(module.clone()) {
                    std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                    std::collections::hash_map::Entry::Vacant(e) => {
                        let got = resolve_module_items(items, file_path, module, &r.name)?;
                        if let Some((_, tracked)) = &got.1 {
                            record_files.push(tracked.clone());
                        }
                        e.insert(got)
                    }
                };
                find_item_direct(&resolved.0, &r.name)
                    .cloned()
                    .ok_or_else(|| match &resolved.1 {
                        Some((read, _tracked)) => format!(
                            "host_module: `{}` not found in {}",
                            r.name,
                            crate::diagnostic::display_path(read)
                        ),
                        None => format!(
                            "host_module: `{}` not found in `mod {module}` in this file",
                            r.name
                        ),
                    })?
            }
        };
        out.push(nested_item_decl(&it, &r.name, uses)?);
    }
    Ok((out, record_files))
}

// ---------------------------------------------------------------- host modules

/// One parameter of a `#[host_module]` method, as both sides need it: the Teal
/// declaration and the Rust wrapper that receives the value.
#[derive(Clone)]
pub struct HostParam {
    /// The Rust parameter name, which is also the one the declaration spells — a Teal
    /// caller passes positionally, but the name is what the signature reads as.
    pub name: String,
    /// Type the Lua side hands over (`&str` -> `String`, `&[T]` -> `Vec<T>`, `&T` -> `T`).
    /// A `&mut` parameter is refused: the wrapper owns the value it converted, and nothing
    /// on the Lua side would see a mutation of it.
    pub owned_ty: Type,
    /// The Rust fn takes a reference; the wrapper passes `&value`.
    pub by_ref: bool,
    /// The Teal type, already mapped — `string` for a `&str`, `{T}` for a `Vec<T>`. The
    /// declaration's half of [`owned_ty`](Self::owned_ty).
    pub teal: String,
    /// Declared `name?: T` — an `Option<T>` the Lua caller may leave out (or pass nil),
    /// and the method sees `None`. Only a *trailing* run of them can be marked (see
    /// `host_decl`): Teal parses `?` on the last parameters only, so an `Option` with a
    /// required parameter after it is declared as the plain `T` and has to be passed.
    pub optional: bool,
    /// Named in the line's `---@noyield(..)`, by the rule the `host_module` macro doc states
    /// (*A Lua function as a parameter*); always `false` for a parameter that is not a
    /// `Function`.
    pub noyield: bool,
}

/// A method's `&Lua` parameter ([`is_lua_ref`]), if it has one.
#[derive(Clone)]
pub struct LuaParam {
    /// The Rust parameter name: what the wrapper binds the closure's Lua handle to.
    pub name: String,
    /// Its position among the method's *typed* parameters (the receiver is not among
    /// them) in the original signature — where, among [`HostMethod::params`] (which does
    /// not hold it), the wrapper splices it back in when it builds the call.
    pub index: usize,
}

/// What `#[teal(..)]` on a `#[host_module]` parameter says about a Lua function the host is
/// handed. The only words a parameter takes; each states the fact outright, so it reads the
/// same on a sync fn and an `async fn` — only the default it replaces differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackAttr {
    /// `#[teal(yields)]`: a suspension inside the function can reach the host's executor —
    /// the host stores it and calls it later with `call_async` under an executor of its
    /// own. Not named in `---@noyield`, whatever the method is.
    Yields,
    /// `#[teal(noyield)]`: the host calls it with the sync `Function::call` (an `async fn`
    /// that does so on purpose says it here). Named in `---@noyield`.
    NoYield,
}

/// `#[teal(yields)]` / `#[teal(noyield)]` on parameter `pname` of `fname`, or `None` when
/// the parameter carries no `#[teal(..)]`. Anything else inside the attribute, both words at
/// once, or an attribute with no word is refused rather than ignored: the macro strips the
/// attribute before rustc sees it, so a word it did not read would be one nobody reads.
pub fn param_callback_attr(
    attrs: &[Attribute],
    fname: &str,
    pname: &str,
) -> Result<Option<CallbackAttr>, String> {
    let mut out: Option<CallbackAttr> = None;
    let mut found = false;
    for a in attrs.iter().filter(|a| a.path().is_ident("teal")) {
        found = true;
        let Meta::List(_) = &a.meta else {
            return Err(format!(
                "host_module: `{fname}`: `#[teal(..)]` on parameter `{pname}` takes `yields` or `noyield`"
            ));
        };
        let list = a
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .map_err(|e| format!("host_module: `{fname}`: parameter `{pname}`: {e}"))?;
        for meta in list {
            let word = match &meta {
                Meta::Path(p) => p.get_ident().map(|i| i.to_string()),
                _ => None,
            };
            let got = match word.as_deref() {
                Some("yields") => CallbackAttr::Yields,
                Some("noyield") => CallbackAttr::NoYield,
                _ => {
                    let key = meta
                        .path()
                        .segments
                        .iter()
                        .map(|s| s.ident.to_string())
                        .collect::<Vec<_>>()
                        .join("::");
                    return Err(format!(
                        "host_module: `{fname}`: `#[teal(..)]` on parameter `{pname}` takes `yields` or `noyield`, got `{key}`"
                    ));
                }
            };
            if out.is_some_and(|prev| prev != got) {
                return Err(format!(
                    "host_module: `{fname}`: parameter `{pname}` is marked both `yields` and `noyield`"
                ));
            }
            out = Some(got);
        }
    }
    if found && out.is_none() {
        return Err(format!(
            "host_module: `{fname}`: `#[teal(..)]` on parameter `{pname}` takes `yields` or `noyield`"
        ));
    }
    Ok(out)
}

/// One `pub fn` of a `#[host_module]` impl block, broken down for the declaration and for
/// the wrapper that registers it.
#[derive(Clone)]
pub struct HostMethod {
    /// The Rust fn name, which is the key it is registered under and the name Teal calls.
    pub name: String,
    /// `None` = associated fn (no `self`), `Some(false)` = `&self`, `Some(true)` = `&mut self`.
    pub receiver: Option<bool>,
    /// In declaration order, which is the order a Teal caller passes them. The receiver is
    /// not among them — it is [`receiver`](Self::receiver) — so a method's Teal arity is
    /// this length either way.
    pub params: Vec<HostParam>,
    /// Teal type of the success value (`T` of `Result<T, E>`, or the plain return); empty for unit.
    pub ret_teal: String,
    /// The Rust fn returns a `Result`. What that becomes on the Teal side is
    /// [`HostDecl::err_mode`]'s to say — a raise or a second return value — so this only
    /// records that there is an `Err` to decide about.
    pub ret_is_result: bool,
    /// The success value is `()` (nothing to hand back but "it worked").
    pub ret_is_unit: bool,
    /// `async fn`: registered through mlua's async variant, and callable only from inside
    /// a Lua coroutine. The executor is the caller's — mlua yields to whatever is polling
    /// and provides nothing of its own — so the method runs under `call_async`, which
    /// creates the coroutine, or an `AsyncThread` the host drives; called from a plain
    /// `load(..).eval()` there is nothing to suspend, and Lua raises rather than blocking.
    /// The Teal signature is the same either way — an async function yields internally
    /// and hands back the same values — so this changes the generated Rust and adds one
    /// thing to the `.d.tl`: the trailing `---@async` marker on the declaration line,
    /// which under `[lang] async` is how the checker knows a call of the method may
    /// suspend (`await-missing`, `await-non-async` in [`crate::lint`]).
    pub is_async: bool,
    /// The method's `&Lua` parameter, if it has one: at most one, anywhere among the
    /// parameters, filled from the closure's own Lua handle rather than from the Lua
    /// arguments (so not among [`params`](Self::params)) and left out of the `.d.tl`.
    pub lua_param: Option<LuaParam>,
}

/// A `#[host_module]` impl block, lowered: what Teal is told, and what the macro needs to
/// write the wrappers that make it true.
#[derive(Clone)]
pub struct HostDecl {
    /// The Rust type the block is `impl`ed on. What the wrappers are generated against,
    /// and what a `Self` in a signature was mapped to.
    pub type_name: String,
    /// The Teal module name — `#[host_module(name = "..")]`, or the type name — which is
    /// what `require` asks for and what the declaration's record is called.
    pub module: String,
    /// The finished `.d.tl` text, including any nested `records` and `uses` imports.
    pub decl: String,
    /// Every `pub fn` that crosses, in source order. The wrappers are generated from
    /// these, so a method missing here is one Teal cannot call however the declaration
    /// reads.
    pub methods: Vec<HostMethod>,
    /// What was parsed off `#[host_module(..)]`: where to write the declaration, what to
    /// import, which records to nest.
    pub attrs: TealAttrs,
    /// How an `Err` reaches Lua — raised, or returned beside the value. Decided once for
    /// the block rather than per method, so a module does not mix the two conventions.
    pub err_mode: ErrMode,
    /// Absolute paths of every file read to resolve a module-qualified `records =
    /// [module::Name]` entry — empty unless `records` named one. The macro tracks each
    /// with `const _: &str = include_str!(..)`, the same way it tracks `htl.toml` and a
    /// `.tl` dependency, so editing one of these files expands the macro again; `htl dts`
    /// (no build to track) leaves this unread.
    pub record_files: Vec<PathBuf>,
}

/// Teal's lexer keywords (`crates/htl-core/vendor/tl.lua`'s `keywords` table, copied
/// rather than read at build time since the vendored file is Lua, not Rust): the words a
/// Teal identifier may not be spelled, because the grammar reads them as something else
/// wherever one is expected.
const TEAL_KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

/// `true` for a Teal identifier: starts with a letter or `_`, continues with letters,
/// digits or `_`, and is not one of [`TEAL_KEYWORDS`].
fn is_teal_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_') && !TEAL_KEYWORDS.contains(&s)
}

/// `true` for a `#[host_module(name = "..")]` this macro can declare: one or more
/// `.`-separated segments, each a [`is_teal_ident`]. Every segment, including the ones
/// before the last, becomes a directory name under the declaration root (`std.fs` is
/// `types/std/fs.d.tl`) and a component of the `require` key the dots join back into —
/// see the `#[host_module]` attribute doc (`htl_macros::host_module`) for the record
/// identifier the last segment alone becomes.
fn valid_host_module_name(name: &str) -> bool {
    !name.is_empty() && name.split('.').all(is_teal_ident)
}

/// [`host_decl`], for `#[c_export]`'s own breakdown of an `impl` block
/// (`htl_core::cexport::plan`): the same parse, but without the Teal-name check
/// `host_decl` applies, since a `#[c_export]`-only impl writes no `.d.tl` — `HostDecl::decl`
/// is built and then never read — and refusing it over an identifier it never declares
/// would refuse a C export of, say, `impl End` for a reason that only applies to Teal.
pub fn host_decl_for_c_export(
    imp: &ItemImpl,
    file_items: Option<&[Item]>,
    file_path: Option<&Path>,
) -> Result<HostDecl, String> {
    host_decl_checked(imp, TealAttrs::default(), file_items, file_path, false)
}

/// Declaration + wrapper plan for a `#[host_module]` impl block. `file_items` (the
/// enclosing file's items) and `file_path` (that file's own path) are only needed when
/// `records = [...]` is used — `file_path` only when one of its entries is
/// module-qualified (`records = [geom::Point]`), to find the file Rust compiles `mod
/// geom;` from. Both come from the same place: the macro's `current_file()` at expansion
/// time, `htl dts`'s `scan_rust_file` outside one.
pub fn host_decl(
    imp: &ItemImpl,
    attrs: TealAttrs,
    file_items: Option<&[Item]>,
    file_path: Option<&Path>,
) -> Result<HostDecl, String> {
    host_decl_checked(imp, attrs, file_items, file_path, true)
}

/// [`host_decl`] and [`host_decl_for_c_export`], the one difference between them being
/// `check_name`: `true` refuses a `module` [`valid_host_module_name`] rejects before any
/// of it is spliced into the declaration's grammar; `false` (the c-export path) never
/// asks the question, since nothing of `module` reaches a `.d.tl` there.
fn host_decl_checked(
    imp: &ItemImpl,
    attrs: TealAttrs,
    file_items: Option<&[Item]>,
    file_path: Option<&Path>,
    check_name: bool,
) -> Result<HostDecl, String> {
    let type_name = match &*imp.self_ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => return Err("host_module: impl target must be a plain type".into()),
    };
    let module = attrs
        .name
        .clone()
        .unwrap_or_else(|| type_name.to_lowercase());
    if check_name && !valid_host_module_name(&module) {
        return Err(match &attrs.name {
            Some(_) => format!(
                "host_module: `name` must be one or more `.`-separated Teal identifiers \
                 (a letter or `_`, then letters, digits or `_`; not a Teal keyword such \
                 as `end` or `local`), got {module:?}"
            ),
            // No `name` was written at all: `module` is the impl target's own name,
            // lowercased, so the message has to name that rather than talk about a
            // `name` nobody wrote.
            None => format!(
                "host_module: `{type_name}` lowercases to `{module}`, which is not a valid \
                 Teal identifier{}; give this `#[host_module]` a `name = \"..\"` that is",
                if TEAL_KEYWORDS.contains(&module.as_str()) {
                    " — it is a Teal keyword"
                } else {
                    ""
                }
            ),
        });
    }
    // `module` is `require`'s full key (`std.fs`) and what the macro registers in
    // `package.preload` under; `ident` is its last segment (`fs`), the identifier the
    // declaration's grammar actually has room for — `local record <ident>`, every
    // `self: <ident>`, `return <ident>` (see `htl_macros::host_module`'s doc for why).
    // The two coincide when `name` has no dot, so a plain `name = "host"` is unaffected.
    let ident = match module.rsplit_once('.') {
        Some((_, last)) => last.to_string(),
        None => module.clone(),
    };
    let err_mode = match attrs.errors.as_deref() {
        Some("return") => ErrMode::Return,
        _ => ErrMode::Raise,
    };

    let mut body = format!("local record {ident}\n");
    let (nested, record_files) =
        nested_record_decls(&attrs.records, file_items, file_path, &attrs.uses)?;
    for r in nested {
        body.push_str(&r);
    }

    let mut methods = Vec::new();
    for it in &imp.items {
        let ImplItem::Fn(f) = it else { continue };
        let fname = f.sig.ident.to_string();
        if !matches!(f.vis, syn::Visibility::Public(_)) {
            // A fn that does not cross has no declaration line for the attribute to shape,
            // and the macro strips it all the same: refused, not read as nothing.
            for a in &f.sig.inputs {
                if let FnArg::Typed(pt) = a
                    && pt.attrs.iter().any(|a| a.path().is_ident("teal"))
                {
                    let pname = match &*pt.pat {
                        Pat::Ident(pi) => pi.ident.to_string(),
                        _ => "_".to_string(),
                    };
                    return Err(format!(
                        "host_module: `{fname}` is not `pub` and does not cross, so `#[teal(..)]` on its parameter `{pname}` says nothing"
                    ));
                }
            }
            continue;
        }
        let is_async = f.sig.asyncness.is_some();
        let mut receiver: Option<bool> = None;
        let mut params = Vec::new();
        let mut teal_params = Vec::new();
        let mut lua_param: Option<LuaParam> = None;
        let mut typed_idx = 0usize;
        for a in &f.sig.inputs {
            match a {
                FnArg::Receiver(r) => receiver = Some(r.mutability.is_some()),
                FnArg::Typed(pt) => {
                    let index = typed_idx;
                    typed_idx += 1;
                    if is_lua_ref(&pt.ty) {
                        let pname = match &*pt.pat {
                            Pat::Ident(pi) => pi.ident.to_string(),
                            _ => format!("a{index}"),
                        };
                        if let Some(prev) = &lua_param {
                            return Err(format!(
                                "host_module: `{fname}`: only one `&Lua` parameter is allowed \
                                 (already have `{}`, found a second, `{pname}`)",
                                prev.name
                            ));
                        }
                        if pt.attrs.iter().any(|a| a.path().is_ident("teal")) {
                            return Err(format!(
                                "host_module: `{fname}`: `#[teal(..)]` on parameter `{pname}`, \
                                 whose type is `&Lua`: the Lua state takes no word"
                            ));
                        }
                        lua_param = Some(LuaParam { name: pname, index });
                        continue;
                    }
                    if is_lua_owned(&pt.ty) {
                        let pname = match &*pt.pat {
                            Pat::Ident(pi) => pi.ident.to_string(),
                            _ => format!("a{index}"),
                        };
                        return Err(format!(
                            "host_module: `{fname}`: parameter `{pname}` is `Lua` by value; \
                             take `&Lua`, not `Lua`"
                        ));
                    }
                    let (owned_ty, by_ref): (Type, bool) = match &*pt.ty {
                        Type::Reference(r) => {
                            if r.mutability.is_some() {
                                return Err(format!(
                                    "host_module: `{fname}`: `&mut` parameters are not supported"
                                ));
                            }
                            let owned: Type = match &*r.elem {
                                Type::Path(p) if p.path.is_ident("str") => {
                                    syn::parse_quote!(::std::string::String)
                                }
                                Type::Slice(s) => {
                                    let e = &s.elem;
                                    syn::parse_quote!(::std::vec::Vec<#e>)
                                }
                                other => other.clone(),
                            };
                            (owned, true)
                        }
                        other => (other.clone(), false),
                    };
                    let pname = match &*pt.pat {
                        Pat::Ident(pi) => pi.ident.to_string(),
                        _ => format!("a{}", params.len()),
                    };
                    let teal = teal_type_in(&owned_ty, &ident, &attrs.uses)?;
                    let optional = is_option(&owned_ty);
                    let callback = param_callback_attr(&pt.attrs, &fname, &pname)?;
                    let is_fn = is_function(&owned_ty);
                    if let Some(word) = callback
                        && !is_fn
                    {
                        let word = match word {
                            CallbackAttr::Yields => "yields",
                            CallbackAttr::NoYield => "noyield",
                        };
                        return Err(format!(
                            "host_module: `{fname}`: `#[teal({word})]` on parameter `{pname}`, which is not a `Function`: the word says how the host calls a Lua function it is handed"
                        ));
                    }
                    // A sync fn has only `Function::call`, which a suspension cannot yield
                    // through; an `async fn` is expected to `call_async`, which it can.
                    let noyield = is_fn
                        && match callback {
                            Some(CallbackAttr::Yields) => false,
                            Some(CallbackAttr::NoYield) => true,
                            None => !is_async,
                        };
                    params.push(HostParam {
                        name: pname,
                        owned_ty,
                        by_ref,
                        teal,
                        optional,
                        noyield,
                    });
                }
            }
        }
        // `Option<T>` is declared `name?: T` — the Rust side already takes nil for it, and
        // the declaration is what lets a Teal caller leave the argument out. Teal parses
        // `?` only on a trailing run ("non-optional arguments cannot follow optional
        // arguments"), so an `Option` with a required parameter after it stays required:
        // marking it would make the whole `.d.tl` unparseable, and refusing the signature
        // would reject a shape both Rust and the wrapper handle.
        let mut required_seen = false;
        for p in params.iter_mut().rev() {
            if !p.optional {
                required_seen = true;
            } else if required_seen {
                p.optional = false;
            }
        }
        for p in &params {
            let mark = if p.optional { "?" } else { "" };
            teal_params.push(format!("{}{mark}: {}", p.name, p.teal));
        }
        if receiver.is_some() {
            teal_params.insert(0, format!("self: {ident}"));
        }
        let (ret_teal, ret_is_result) = match &f.sig.output {
            ReturnType::Default => (String::new(), false),
            ReturnType::Type(_, t) => (teal_type_in(t, &ident, &attrs.uses)?, is_result(t)),
        };
        let ret_is_unit = ret_teal.is_empty();
        // Teal-side return: `Result` in return mode becomes `T, string` (`boolean, string`
        // for unit), the Lua `value, err` convention; otherwise just `T`.
        let teal_ret = if ret_is_result && err_mode == ErrMode::Return {
            if ret_is_unit {
                "boolean, string".to_string()
            } else {
                format!("{ret_teal}, string")
            }
        } else {
            ret_teal.clone()
        };
        let ret_suffix = if teal_ret.is_empty() {
            String::new()
        } else {
            format!(": {teal_ret}")
        };
        // `---@async` on the line: the Teal signature is the same either way, and the
        // marker is how the checker's `await-missing` / `await-non-async` learn that a call
        // of this method may suspend (read off the declaring line, like `---@nilable`).
        let async_marker = if is_async { " ---@async" } else { "" };
        // `---@noyield(f, g)`: the `Function` parameters the host calls from C, by the names
        // this line spells, which is how `async-as-sync-callback` finds them for either call
        // form (`api:each(cb)`, `api.each(api, cb)`). After `---@async` when both are there.
        let noyield: Vec<&str> = params
            .iter()
            .filter(|p| p.noyield)
            .map(|p| p.name.as_str())
            .collect();
        let noyield_marker = if noyield.is_empty() {
            String::new()
        } else {
            format!(" ---@noyield({})", noyield.join(", "))
        };
        body.push_str(&format!(
            "   {fname}: function({}){ret_suffix}{async_marker}{noyield_marker}\n",
            teal_params.join(", ")
        ));
        methods.push(HostMethod {
            name: fname,
            receiver,
            params,
            ret_teal,
            ret_is_result,
            ret_is_unit,
            is_async,
            lua_param,
        });
    }
    body.push_str(&format!("end\n\nreturn {ident}\n"));
    let uses = with_task_default(&format!("host_module `{module}`"), &body, &attrs.uses)?;
    let decl = format!("{}{body}", uses_header(&uses));

    Ok(HostDecl {
        type_name,
        module,
        decl,
        methods,
        attrs,
        err_mode,
        record_files,
    })
}

// ---------------------------------------------------------------- file scanning (`htl dts`)

/// Which Rust shape a [`Generated`] came from — the filter [`regenerate_host_decls`]
/// uses to skip everything except a `#[host_module]`'s own `.d.tl` (S1 of #429's review:
/// a `#[derive(TealRecord)]` sees `#[cfg]`-stripped input the same way the derive itself
/// does, so rewriting a record from the raw source without that stripping can write a
/// *different* declaration than the one the `#[cfg]`'d derive would — the #429 class,
/// moved to records, if the crate-wide scan touched records too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedKind {
    /// `#[host_module(dts = ..)]`.
    HostModule,
    /// `#[derive(TealRecord)]` / `#[teal(dts = ..)]`.
    Record,
    /// `#[c_export(header = ..)]`.
    CHeader,
}

/// One `.d.tl` (or C header) derived from a Rust source file.
#[derive(Debug, Clone)]
pub struct Generated {
    /// Absolute output path (`<manifest_dir>/<dts>`).
    pub target: PathBuf,
    /// The declaration itself. Returned rather than written, so the caller decides — the
    /// proc macro writes at expansion time, `htl dts` after its own scan.
    pub text: String,
    /// The `.rs` it was derived from. A scan reads many files into one list, and this is
    /// what says which of them a given declaration came from.
    pub source: PathBuf,
    /// `host_module <module>`, or `RecordDecl::what` (`record <Name>` / `enum <Name>` /
    /// `type <Name>`), for reporting.
    pub what: String,
    /// Which Rust shape this came from — see [`GeneratedKind`].
    pub kind: GeneratedKind,
}

fn walk_items<'a>(items: &'a [Item], out: &mut Vec<&'a Item>) {
    for it in items {
        out.push(it);
        if let Item::Mod(m) = it
            && let Some((_, inner)) = &m.content
        {
            walk_items(inner, out);
        }
    }
}

/// Declarations requested by `#[host_module(dts = ..)]` / `#[teal(dts = ..)]` in one file.
pub fn scan_rust_file(path: &Path, manifest_dir: &Path) -> Result<Vec<Generated>, String> {
    let src =
        std::fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let file = syn::parse_file(&src).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    let mut flat = Vec::new();
    walk_items(&file.items, &mut flat);
    let mut out = Vec::new();
    for it in flat {
        match it {
            Item::Impl(imp) => {
                if let Some(attrs) = parse_host_module_attr(&imp.attrs)? {
                    let hd = host_decl(imp, attrs, Some(&file.items), Some(path))?;
                    if let Some(dts) = &hd.attrs.dts {
                        out.push(Generated {
                            target: manifest_dir.join(dts),
                            text: hd.decl.clone(),
                            source: path.to_path_buf(),
                            what: format!("host_module {}", hd.module),
                            kind: GeneratedKind::HostModule,
                        });
                    }
                }
                // The C header is the same scan pointed at the other output: `htl dts`
                // writes both, so a header can be regenerated without a `cargo build`
                // (and reviewed in a diff, which is what `#[c_export(header = ..)]`
                // wrote it to a file for).
                if let Some(attrs) = crate::cexport::parse_c_export_attr(&imp.attrs)?
                    && attrs.header.is_some()
                {
                    let hd = host_decl_for_c_export(imp, Some(&file.items), Some(path))?;
                    let plan = crate::cexport::plan(&hd, imp, attrs)?;
                    if let Some(header) = &plan.header_path {
                        out.push(Generated {
                            target: manifest_dir.join(header),
                            text: plan.header(),
                            source: path.to_path_buf(),
                            what: format!("c_export {}", plan.prefix),
                            kind: GeneratedKind::CHeader,
                        });
                    }
                }
            }
            Item::Struct(ItemStruct { attrs, .. }) | Item::Enum(ItemEnum { attrs, .. })
                if derives_teal_record(attrs) =>
            {
                let rd = record_decl(it)?;
                if let Some(dts) = &rd.attrs.dts {
                    out.push(Generated {
                        target: manifest_dir.join(dts),
                        text: rd.decl.clone(),
                        source: path.to_path_buf(),
                        what: rd.what(),
                        kind: GeneratedKind::Record,
                    });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Where [`host_module_names`], [`generate_crate_to`], [`scan_fingerprint`] and
/// [`regenerate_host_decls`] all look, so the four cannot drift into scanning different
/// trees of the same crate.
const SCAN_SUBDIRS: [&str; 4] = ["src", "examples", "tests", "benches"];

/// Every module name a `#[host_module]` under this crate root registers, whether or not
/// it also asks for a `.d.tl`. The name is the attribute's `name = ".."`, or the impl
/// target lowercased — the same rule [`host_decl`] applies, which is what makes this
/// answer the run-time set of `package.preload` keys without a build.
///
/// The walk of [`generate_crate`] minus the writing, and with a substring test before the
/// parse: a crate with no `#[host_module]` anywhere pays one read per `.rs` file and no
/// `syn`, and a project with no crate at all is never asked (the caller has no root to
/// pass). A file that will not parse, or an impl whose target is not a plain type,
/// contributes nothing — `htl dts` is where either of those is an error worth reporting.
pub fn host_module_names(manifest_dir: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for sub in SCAN_SUBDIRS {
        let dir = manifest_dir.join(sub);
        if !dir.is_dir() {
            continue;
        }
        for e in walkdir::WalkDir::new(&dir)
            .sort_by_file_name()
            .into_iter()
            .flatten()
        {
            let p = e.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(p) else {
                continue;
            };
            if !src.contains("host_module") {
                continue;
            }
            let Ok(file) = syn::parse_file(&src) else {
                continue;
            };
            let mut flat = Vec::new();
            walk_items(&file.items, &mut flat);
            for it in flat {
                let Item::Impl(imp) = it else { continue };
                let Ok(Some(attrs)) = parse_host_module_attr(&imp.attrs) else {
                    continue;
                };
                let name = match (attrs.name, &*imp.self_ty) {
                    (Some(n), _) => Some(n),
                    (None, Type::Path(p)) => p
                        .path
                        .segments
                        .last()
                        .map(|s| s.ident.to_string().to_lowercase()),
                    (None, _) => None,
                };
                if let Some(n) = name
                    && !out.contains(&n)
                {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// Nearest ancestor of `start` holding a `Cargo.toml` with a `[package]` section
/// (a workspace root without a package does not count).
pub fn find_cargo_package_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start.to_path_buf()
    } else {
        crate::parent_dir(start)
    };
    if let Ok(abs) = std::fs::canonicalize(&dir) {
        dir = abs;
    }
    loop {
        let manifest = dir.join("Cargo.toml");
        if manifest.is_file()
            && let Ok(text) = std::fs::read_to_string(&manifest)
            && text.contains("[package]")
        {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Scan `src/`, `examples/`, `tests/`, `benches/` under a crate root and write every
/// requested `.d.tl` (only when content changed). Returns `(target, written)` pairs.
pub fn generate_crate(manifest_dir: &Path) -> Result<Vec<(PathBuf, bool)>, String> {
    generate_crate_to(manifest_dir, true)
}

/// [`generate_crate`], writing only when `write` is set; without it each pair says whether
/// the file would change.
///
/// The walk is `src/`, `examples/`, `tests/`, `benches/` under `manifest_dir`, not under
/// whatever crate a file inside one of them happens to belong to: a fixture crate nested
/// under `tests/<fixture>/` with its own `Cargo.toml` (a trybuild-style project, say) is
/// still scanned relative to the *outer* manifest dir, the same as `htl dts` does today —
/// a `dts = ".."` inside that fixture resolves against the outer root, not the fixture's
/// own, because this walk never looks for a nested `Cargo.toml` to retarget against.
pub fn generate_crate_to(manifest_dir: &Path, write: bool) -> Result<Vec<(PathBuf, bool)>, String> {
    let mut results = Vec::new();
    for sub in SCAN_SUBDIRS {
        let dir = manifest_dir.join(sub);
        if !dir.is_dir() {
            continue;
        }
        for e in walkdir::WalkDir::new(&dir).sort_by_file_name() {
            let e = e.map_err(|e| e.to_string())?;
            let p = e.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            for g in scan_rust_file(p, manifest_dir)? {
                let written = crate::write_if_changed_when(&g.target, &g.text, write)
                    .map_err(|err| format!("writing {}: {err}", g.target.display()))?;
                results.push((g.target, written));
            }
        }
    }
    Ok(results)
}

/// How many `.rs` files `SCAN_SUBDIRS` holds and the newest modification time among
/// them, as of one `fs::metadata` per file — no `read_to_string`, no `syn`. Two calls
/// that answer the same pair have seen the same set of files with the same content as
/// far as an editor saving one of them can tell apart (an mtime granularity collision
/// aside, which only means a cache might refresh one write-cycle later than it had to,
/// never earlier): a file added, removed or rewritten moves the count or the newest time,
/// which is what [`regenerate_host_decls`]'s caller re-runs the scan on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanFingerprint {
    files: usize,
    newest: Option<SystemTime>,
}

/// [`ScanFingerprint`] of `manifest_dir`'s `SCAN_SUBDIRS`, a stat walk rather than a
/// read-and-parse one: cheap enough to run on every `include_tl!` / `include_bundle!`
/// expansion so that the caller — `regenerate_crate` in `htl-macros` — knows whether to
/// redo the write pass without needing a result from a previous run in the same process.
/// That matters because "one process compiles one crate" is not true of every caller: an
/// IDE's proc-macro server (rust-analyzer's `proc-macro-srv`, say) loads this dylib once
/// and serves every crate of the workspace, for the whole editing session, from inside
/// it — a cached "already regenerated" flag would never refresh there, and a crate's
/// `.d.tl`s would go stale after the first edit to a `#[host_module]` and stay stale
/// until the IDE restarts.
pub fn scan_fingerprint(manifest_dir: &Path) -> ScanFingerprint {
    let mut files = 0usize;
    let mut newest: Option<SystemTime> = None;
    for sub in SCAN_SUBDIRS {
        let dir = manifest_dir.join(sub);
        if !dir.is_dir() {
            continue;
        }
        for e in walkdir::WalkDir::new(&dir).into_iter().flatten() {
            let p = e.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            files += 1;
            if let Ok(meta) = e.metadata()
                && let Ok(modified) = meta.modified()
            {
                newest = Some(match newest {
                    Some(prev) if prev >= modified => prev,
                    _ => modified,
                });
            }
        }
    }
    ScanFingerprint { files, newest }
}

/// Writes every `#[host_module]`'s `.d.tl` it can read, parse and write, from the Rust
/// source — [`scan_rust_file`] filtered to [`GeneratedKind::HostModule`] (S1: not
/// records, whose own derive sees `#[cfg]`-stripped input in a way this raw-source scan
/// cannot reproduce, and not C headers, which `#[host_module]`'s own expansion does not
/// also write on every expansion the way it does its `.d.tl`).
///
/// Best-effort and silent (S2): a file this cannot read, cannot parse (an unparsable
/// fixture under `tests/` — a trybuild case deliberately testing invalid syntax, say, or
/// one written for a newer Rust edition than this `syn` parses), or whose attributes
/// `scan_rust_file` refuses, is skipped rather than failing every `include_tl!` of a
/// crate that never compiles that file; so is a `.d.tl` this cannot write (a read-only
/// directory, a path a sibling process is also writing to — made safe to race with by
/// [`crate::write_if_changed`]'s atomic rename, not by skipping, but a failure past that
/// is still skipped here). Nothing it skips is unreported: the file's own compilation —
/// that `#[host_module]`'s real expansion, or rustc parsing a file this cannot — reports
/// the same error at its own site, which is the error a developer is looking at code to
/// fix, not one surfaced from behind an unrelated `include_tl!` of some other file.
///
/// A `#[cfg]`-disabled `mod` holding a `#[host_module]` is still a file under one of
/// `SCAN_SUBDIRS`, and this scan — like `htl dts` — does not evaluate `#[cfg]` on a
/// `mod` item any more than on anything else inside the file it parses, so that module's
/// `.d.tl` is written (and an `include_tl!` elsewhere checks cleanly against it) even
/// though the module is not compiled into this build. That `require` then fails at run
/// time naming the missing module, the same as a module no `.d.tl` was ever written for;
/// this is not a correctness hole the scan introduces, since `#[host_module]`'s own
/// expansion would refuse to run under the same `#[cfg]` either way.
///
/// The prefilter is `host_module_names`'s: a file read but not handed to `syn` unless its
/// text contains the literal substring `host_module`, so a crate with no host modules at
/// all — most files of most crates — pays one `read_to_string` and nothing else.
pub fn regenerate_host_decls(manifest_dir: &Path) {
    for sub in SCAN_SUBDIRS {
        let dir = manifest_dir.join(sub);
        if !dir.is_dir() {
            continue;
        }
        for e in walkdir::WalkDir::new(&dir)
            .sort_by_file_name()
            .into_iter()
            .flatten()
        {
            let p = e.path();
            if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(p) else {
                continue;
            };
            if !src.contains("host_module") {
                continue;
            }
            let Ok(entries) = scan_rust_file(p, manifest_dir) else {
                continue;
            };
            for g in entries {
                if g.kind != GeneratedKind::HostModule {
                    continue;
                }
                let _ = crate::write_if_changed(&g.target, &g.text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_common;

    fn item(src: &str) -> Item {
        syn::parse_str(src).unwrap()
    }

    #[test]
    fn a_struct_with_named_fields_is_a_record_as_before() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] pub struct Point { pub x: f64, pub y: f64 }",
        ))
        .unwrap();
        assert_eq!(rd.what(), "record Point");
        assert_eq!(
            rd.decl,
            "local record Point\n   x: number\n   y: number\nend\n\nreturn Point\n"
        );
    }

    #[test]
    fn a_newtype_is_a_type_alias_of_its_inner_type() {
        let rd = record_decl(&item("#[derive(TealRecord)] pub struct Sql(pub String);")).unwrap();
        assert_eq!(
            rd.kind,
            RecordKind::Alias {
                inner: "string".into()
            }
        );
        assert_eq!(rd.what(), "type Sql");
        assert_eq!(rd.decl, "local type Sql = string\n\nreturn Sql\n");
    }

    #[test]
    fn a_unit_enum_is_a_teal_enum_of_its_variant_names() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] pub enum Mode { Fast, Careful }",
        ))
        .unwrap();
        assert_eq!(rd.what(), "enum Mode");
        assert_eq!(
            rd.decl,
            "local enum Mode\n   \"Fast\"\n   \"Careful\"\nend\n\nreturn Mode\n"
        );
    }

    /// `rename_all` spells the entries; the Rust names are untouched, so the enum still
    /// reads as Rust and the Teal side keeps the words its store already holds.
    #[test]
    fn rename_all_spells_the_enum_entries() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"snake_case\")] pub enum State { Open, InReview }",
        ))
        .unwrap();
        assert_eq!(
            rd.kind,
            RecordKind::Enum {
                variants: vec!["open".into(), "in_review".into()]
            }
        );
        assert_eq!(
            rd.decl,
            "local enum State\n   \"open\"\n   \"in_review\"\nend\n\nreturn State\n"
        );
    }

    /// serde's set, applied to a variant name, so a type that is also `Serialize` can say
    /// the same thing twice and the two agree.
    #[test]
    fn every_rename_rule_spells_a_variant_serde_s_way() {
        let spellings: Vec<String> = RenameRule::NAMES
            .iter()
            .map(|n| RenameRule::parse(n).unwrap().apply("InReview"))
            .collect();
        assert_eq!(
            spellings,
            vec![
                "inreview",
                "INREVIEW",
                "InReview",
                "inReview",
                "in_review",
                "IN_REVIEW",
                "in-review",
                "IN-REVIEW",
            ]
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"Title Case\")] pub enum S { A }",
        ))
        .unwrap_err();
        assert!(
            e.contains("`rename_all` must be one of") && e.contains("\"snake_case\""),
            "{e}"
        );
    }

    /// `#[teal(name = ..)]` on a variant is the override, and nothing else may be
    /// written there.
    #[test]
    fn a_variant_name_wins_over_rename_all() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"snake_case\")] pub enum State { Open, #[teal(name = \"REVIEW\")] InReview }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local enum State\n   \"open\"\n   \"REVIEW\"\nend\n\nreturn State\n"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub enum State { #[teal(rename_all = \"lowercase\")] Open }",
        ))
        .unwrap_err();
        assert!(
            e.contains("State::Open") && e.contains("`rename_all` goes on the enum"),
            "{e}"
        );
    }

    /// Two variants under one word is a declaration that cannot say which one a value
    /// meant, so it is refused, naming both.
    #[test]
    fn two_variants_reaching_the_same_word_are_refused() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"lowercase\")] pub enum State { Open, OPEN }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: State::Open and State::OPEN both reach Teal as \"open\"; \
             give one a `#[teal(name = \"..\")]` of its own"
        );
        // The override collides the same way, and against a plain Rust name too.
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub enum State { Open, #[teal(name = \"Open\")] Closed }",
        ))
        .unwrap_err();
        assert!(e.contains("State::Open and State::Closed"), "{e}");
    }

    /// Fields are named by `#[serde(rename)]` and `rename_all` — `#[teal(rename_all)]` on
    /// a struct included — and a naming word in a field's `#[teal(..)]` is refused rather
    /// than quietly ignored.
    #[test]
    fn record_fields_are_named_by_rename_all_and_serde_rename() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"camelCase\")] pub struct P { pub call_id: f64 }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local record P\n   callId: number\nend\n\nreturn P\n"
        );
        // `teal` wins over `serde` on the struct; a field's own `rename` wins over both.
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[serde(rename_all = \"kebab-case\")] #[teal(rename_all = \"PascalCase\")] \
             pub struct P { pub call_id: f64, #[serde(rename = \"t\")] pub retry_count: u8 }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local record P\n   CallId: number\n   t: integer\nend\n\nreturn P\n"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { #[teal(name = \"ex\")] pub x: f64 }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: P.x: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`, got `name`"
        );
    }

    /// A rename that differs by direction is refused, naming both words: the `.d.tl`
    /// declares one name. The same word both ways is one name, and is read.
    #[test]
    fn an_asymmetric_rename_is_refused_naming_both() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { #[serde(rename(serialize = \"a\", deserialize = \"b\"))] pub x: f64 }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: P.x: `rename` says serialize = \"a\" and deserialize = \"b\": one type \
             cannot carry two names (the `.d.tl` declares one); write two types, or one name: \
             `rename = \"..\"`"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[serde(rename_all(serialize = \"camelCase\"))] pub struct P { pub x: f64 }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: P: `rename_all` says serialize = \"camelCase\" and deserialize = \
             nothing (so the Rust name): one type cannot carry two names (the `.d.tl` declares \
             one); write two types, or one name: `rename_all = \"..\"`"
        );
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[serde(rename_all(serialize = \"camelCase\", deserialize = \"camelCase\"))] \
             pub struct P { pub call_id: f64 }",
        ))
        .unwrap();
        assert!(rd.decl.contains("   callId: number\n"), "{}", rd.decl);
    }

    /// A serde word that changes the shape and is not read yet is refused, listing what is
    /// read; a word that keeps the shape is passed through to serde.
    #[test]
    fn a_serde_word_not_read_is_refused_and_one_keeping_the_shape_passes() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { #[serde(skip)] pub x: f64 }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: P.x: `#[serde(skip)]` is not read by TealRecord; the attributes it reads \
             are rename_all, rename, alias, rename_all_fields (and it leaves crate, bound, \
             deny_unknown_fields, expecting to serde)"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[serde(tag = \"type\")] pub enum S { A { x: f64 } }",
        ))
        .unwrap_err();
        assert!(
            e.starts_with("TealRecord: S: `#[serde(tag)]` is not read"),
            "{e}"
        );
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[serde(deny_unknown_fields, crate = \"htl::serde\", \
             bound(serialize = \"\"), expecting = \"a P\")] pub struct P { #[serde(bound = \"\")] pub x: f64 }",
        ))
        .unwrap();
        assert_eq!(rd.decl, "local record P\n   x: number\nend\n\nreturn P\n");
    }

    /// What the reader cannot parse is quoted; a read word in the wrong place says where it
    /// goes; a name a Teal record cannot hold is refused; two fields under one name are.
    #[test]
    fn serde_names_the_reader_cannot_use_are_refused() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[serde(rename_all = \"camelCase\" deny_unknown_fields)] pub struct P { pub x: f64 }",
        ))
        .unwrap_err();
        assert!(
            e.starts_with(
                "TealRecord: P: cannot read `#[serde(rename_all = \"camelCase\" deny_unknown_fields)]`: "
            ),
            "{e}"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub enum S { #[serde(alias = \"a\")] A, B }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: S::A: `#[serde(alias)]` on a variant is not read by TealRecord; it reads it on a field"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { #[serde(rename = \"a\\\"b\")] pub x: f64 }",
        ))
        .unwrap_err();
        assert!(
            e.starts_with("TealRecord: P.x: the field crosses as \"a\\\"b\", which a Teal record cannot declare"),
            "{e}"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { pub x: f64, #[serde(rename = \"x\")] pub y: f64 }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: P.y: crosses as \"x\", which another field (`x`) already does; give one \
             a `#[serde(rename = \"..\")]` of its own"
        );
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub enum S { A { #[serde(rename = \"kind\")] k: f64 } }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: S::A: a field named `kind` collides with the variant tag"
        );
    }

    /// serde's rename on a variant spells its word; `#[teal(name)]` wins over it.
    #[test]
    fn a_variant_is_spelled_by_serde_rename_under_teal_name() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[serde(rename_all = \"snake_case\")] pub enum S { \
             #[serde(rename = \"o\")] Open, #[serde(rename = \"x\")] #[teal(name = \"R\")] InReview, Done }",
        ))
        .unwrap();
        assert_eq!(
            rd.kind,
            RecordKind::Enum {
                variants: vec!["o".into(), "R".into(), "done".into()]
            }
        );
    }

    /// `#[teal(noyield)]` on a `Function` / `Option<Function>` field of a struct writes a
    /// bare `---@noyield` at the end of the field's line, nested (`records = [..]`) as well
    /// as in a module of its own; an unmarked `Function` field is a plain `function`.
    #[test]
    fn a_function_field_marked_noyield_is_declared_with_the_bare_marker() {
        let src = "#[derive(TealRecord)] pub struct Game {\n\
                   \x20   #[teal(noyield)] load: Option<Function>,\n\
                   \x20   #[teal(noyield)] update: mlua::Function,\n\
                   \x20   #[teal(noyield)] draw: Function,\n\
                   \x20   later: Function,\n\
                   \x20   n: i64,\n\
                   }";
        let rd = record_decl(&item(src)).unwrap();
        assert_eq!(
            rd.decl,
            "local record Game\n   load: function ---@noyield\n   update: function ---@noyield\n   \
             draw: function ---@noyield\n   later: function\n   n: integer\nend\n\nreturn Game\n"
        );
        let hd = host_impl(&format!(
            "{src}\npub struct Host;\n#[host_module(name = \"host\", records = [Game])]\n\
             impl Host {{\n    pub fn run(&self, g: Game) {{}}\n}}\n"
        ));
        assert!(
            hd.decl.contains(
                "   record Game\n      load: function ---@noyield\n      update: function ---@noyield\n      \
                 draw: function ---@noyield\n      later: function\n      n: integer\n   end\n"
            ),
            "{}",
            hd.decl
        );
    }

    /// A field takes `noyield` and nothing else, on a `Function`, of a struct; the rest is
    /// refused rather than read as nothing.
    #[test]
    fn a_field_attribute_other_than_noyield_on_a_function_is_refused() {
        for (src, want) in [
            (
                "pub struct G { #[teal(yields)] f: Function }",
                "TealRecord: G.f: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`, got `yields`",
            ),
            (
                "pub struct G { #[teal(noyield, name = \"g\")] f: Function }",
                "TealRecord: G.f: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`, got `name`",
            ),
            (
                "pub struct G { #[teal] f: Function }",
                "TealRecord: G.f: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`",
            ),
            (
                "pub struct G { #[teal()] f: Function }",
                "TealRecord: G.f: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`",
            ),
            (
                "pub struct G { #[teal(noyield)] n: i64 }",
                "TealRecord: G.n: `#[teal(noyield)]` on a field that is not a `Function`: the word says the host calls the Lua function it reads from the field from C",
            ),
            (
                "pub struct G { #[teal(noyield)] t: Option<Table> }",
                "TealRecord: G.t: `#[teal(noyield)]` on a field that is not a `Function`: the word says the host calls the Lua function it reads from the field from C",
            ),
            (
                "pub struct G { #[teal(noyield = true)] f: Function }",
                "TealRecord: G.f: `noyield` takes no value",
            ),
            (
                "pub struct G { #[teal(noyield(x))] f: Function }",
                "TealRecord: G.f: `noyield` takes no value",
            ),
            (
                "pub enum G { A, B { #[teal(noyield)] f: Function } }",
                "TealRecord: G.f: `#[teal(noyield)]` applies to a field of a struct record; a variant's fields take only `as`",
            ),
            (
                "pub enum G { A, Y { #[teal(name = \"z\")] f: i64 } }",
                "TealRecord: G.f: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`, got `name`",
            ),
        ] {
            let e = record_decl(&item(&format!("#[derive(TealRecord)] {src}"))).unwrap_err();
            assert_eq!(e, want, "{src}");
        }
    }

    /// `#[teal(as = "..")]` on a field declares it as the text given, verbatim, whatever
    /// the Rust type — one the type table would map, one it would not — on a struct's
    /// field, a struct variant's, and beside `noyield`; the Rust type is not read, so one
    /// the table refuses is declared too.
    #[test]
    fn a_field_marked_as_is_declared_as_the_text_it_gives() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] pub struct Entry {\n\
             \x20   #[teal(as = \"string\")] stamp: Stamp,\n\
             \x20   #[teal(as = \"{string}\")] tags: Tags,\n\
             \x20   #[teal(as = \"geom.Point\")] at: (f64, f64),\n\
             \x20   #[teal(as = \"function(integer)\", noyield)] on: Function,\n\
             \x20   n: i64,\n\
             }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local record Entry\n   stamp: string\n   tags: {string}\n   at: geom.Point\n   \
             on: function(integer) ---@noyield\n   n: integer\nend\n\nreturn Entry\n"
        );
        let RecordKind::Record { fields, noyield } = &rd.kind else {
            panic!("{:?}", rd.kind)
        };
        assert_eq!(fields[0], ("stamp".to_string(), "string".to_string()));
        assert_eq!(noyield, &["on".to_string()]);
        let rd = record_decl(&item(
            "#[derive(TealRecord)] pub enum Ev { Tick { #[teal(as = \"string\")] at: Stamp } }",
        ))
        .unwrap();
        assert!(
            rd.decl.contains("local record Ev_Tick\n   where self.kind == \"Tick\"\n   kind: string\n   at: string\nend\n"),
            "{}",
            rd.decl
        );
    }

    /// `as` is refused when it names nothing, when it is not a string, and anywhere but on
    /// a field — the type, a variant, a `#[host_module]` — with the place it does go named.
    #[test]
    fn as_that_names_no_type_or_sits_off_a_field_is_refused() {
        let off_field = "`as` is a field attribute: `#[teal(as = \"<teal type>\")]` goes on a \
                         field of a `#[derive(TealRecord)]` struct or of a data enum's struct \
                         variant, not on a type, an enum variant or a `#[host_module]`";
        for (src, want) in [
            (
                "pub struct S { #[teal(as = \"\")] stamp: Stamp }",
                "TealRecord: S.stamp: `#[teal(as = \"\")]` names no type; write the Teal type the field crosses as, `as = \"string\"` say".to_string(),
            ),
            (
                "pub struct S { #[teal(as = \"  \")] stamp: Stamp }",
                "TealRecord: S.stamp: `#[teal(as = \"\")]` names no type; write the Teal type the field crosses as, `as = \"string\"` say".to_string(),
            ),
            (
                "pub struct S { #[teal(as)] stamp: Stamp }",
                "TealRecord: S.stamp: `as` takes the Teal type as a string: `#[teal(as = \"<teal type>\")]`".to_string(),
            ),
            (
                "pub struct S { #[teal(as = string)] stamp: Stamp }",
                "TealRecord: S.stamp: `as` takes the Teal type as a string: `#[teal(as = \"<teal type>\")]`".to_string(),
            ),
            (
                "pub struct S { #[teal(as = \"string\", as = \"number\")] stamp: Stamp }",
                "TealRecord: S.stamp: `as` is given twice".to_string(),
            ),
            (
                "#[teal(as = \"string\")] pub struct S { stamp: Stamp }",
                off_field.to_string(),
            ),
            (
                "#[teal(as = \"string\")] pub struct S(Stamp);",
                off_field.to_string(),
            ),
            (
                "pub enum E { #[teal(as = \"string\")] A, B(i64) }",
                off_field.to_string(),
            ),
        ] {
            let e = record_decl(&item(&format!("#[derive(TealRecord)] {src}"))).unwrap_err();
            assert!(e.ends_with(&want), "{src}\n got: {e}\nwant: {want}");
        }
        let imp: syn::ItemImpl =
            syn::parse_str("#[host_module(name = \"h\", as = \"string\")] impl H {}").unwrap();
        assert_eq!(parse_host_module_attr(&imp.attrs).unwrap_err(), off_field);
    }

    /// A naming word on a field is refused, the message naming the two a field takes:
    /// `as` declares a type and nothing else, so there is no `rename` beside it.
    #[test]
    fn a_rename_on_a_field_is_refused_naming_the_words_a_field_takes() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { #[teal(rename = \"x\")] pub ex: f64 }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: P.ex: `#[teal(..)]` on a record field takes `as = \"<teal type>\"` or `noyield`, got `rename`"
        );
    }

    #[test]
    fn a_data_enum_is_a_union_of_where_records() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] pub enum Shape { Dot, Circle(f64), Rect { w: f64, h: f64 } }",
        ))
        .unwrap();
        assert_eq!(rd.what(), "type Shape");
        assert_eq!(
            rd.decl,
            "local record Shape_Dot\n   where self.kind == \"Dot\"\n   kind: string\nend\n\
             local record Shape_Circle\n   where self.kind == \"Circle\"\n   kind: string\n   value: number\nend\n\
             local record Shape_Rect\n   where self.kind == \"Rect\"\n   kind: string\n   w: number\n   h: number\nend\n\
             local type Shape = Shape_Dot | Shape_Circle | Shape_Rect\n\nreturn Shape\n"
        );
    }

    /// A data enum renames its tag, not its records: `where self.kind == "in_review"` is
    /// the word that crosses, while `State_InReview` stays a Teal identifier (which
    /// `kebab-case` is not) and stays the name a caller narrows with.
    #[test]
    fn rename_all_spells_the_kind_tag_and_leaves_the_record_names() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"kebab-case\")] pub enum State { Open, InReview(f64) }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local record State_Open\n   where self.kind == \"open\"\n   kind: string\nend\n\
             local record State_InReview\n   where self.kind == \"in-review\"\n   kind: string\n   value: number\nend\n\
             local type State = State_Open | State_InReview\n\nreturn State\n"
        );
    }

    #[test]
    fn a_data_enum_refuses_to_be_a_module_of_its_own() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(dts = \"types/Shape.d.tl\")] pub enum Shape { Dot, Circle(f64) }",
        ))
        .unwrap_err();
        assert!(e.contains("records = [Shape]"), "{e}");
        assert!(e.contains("is Shape_<Variant>"), "{e}");
    }

    #[test]
    fn a_tuple_variant_with_two_fields_is_refused() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub enum Pair { Two(f64, f64) }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: tuple variants with more than one field are not supported"
        );
    }

    #[test]
    fn a_tuple_struct_with_two_fields_and_a_unit_struct_are_refused() {
        let e = record_decl(&item("#[derive(TealRecord)] pub struct P(f64, f64);")).unwrap_err();
        assert!(e.contains("newtype"), "{e}");
        let e = record_decl(&item("#[derive(TealRecord)] pub struct U;")).unwrap_err();
        assert!(e.contains("unit struct"), "{e}");
    }

    #[test]
    fn uses_imports_with_local_type() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [Mode])] pub struct Run { pub mode: Mode }",
        ))
        .unwrap();
        assert!(
            rd.decl
                .starts_with("local type Mode = require(\"Mode\")\n\nlocal record Run\n"),
            "{}",
            rd.decl
        );
    }

    /// `uses = [name = "module.path"]`: the local name and the module `require` resolves
    /// differ, and a field whose Rust path is qualified by that name crosses dotted the
    /// same way — the acceptance case of #426 (a record from `somelib.types`, used as
    /// `types.Event`).
    #[test]
    fn uses_imports_a_module_path_under_a_different_local_name() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [types = \"somelib.types\"])] \
             pub struct Run { pub ev: types::Event }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local type types = require(\"somelib.types\")\n\n\
             local record Run\n   ev: types.Event\nend\n\nreturn Run\n"
        );
    }

    /// A qualified Rust path keeps only its last segment — the same as a bare identifier
    /// — unless a `uses` entry names its *first* segment: `crate::geom::Point`,
    /// `self::Point` and `geom::Point` all cross as plain `Point` with no `uses` at all,
    /// and so does `UserDataRef<crate::Point>` (the qualifier is on the inner type, which
    /// `UserDataRef` passes through unwrapped). `htl::task::Request` with the wrong
    /// arity (one type argument, not two) falls out of the `Request` special case and
    /// through the same rule, to `Request` — not `htl.task.Request`, which no `.d.tl`
    /// would declare (#426).
    #[test]
    fn a_qualified_path_keeps_its_last_segment_unless_uses_names_its_first_segment() {
        let ty = |s: &str| syn::parse_str::<Type>(s).unwrap();
        for src in [
            "crate::geom::Point",
            "self::Point",
            "geom::Point",
            "UserDataRef<crate::Point>",
        ] {
            assert_eq!(teal_type(&ty(src), "Self").unwrap(), "Point", "{src}");
        }
        assert_eq!(
            teal_type(&ty("htl::task::Request<i64>"), "Self").unwrap(),
            "Request"
        );
    }

    /// `Request` with a third type argument — mlua-isle's rejection reason type — is
    /// refused, not passed through as the bare name: `task.Request<Q, A>` fixes the reason
    /// to `string`, so the message names the two-argument form to write instead. The
    /// refusal reaches a `#[host_module]` signature (the macro and `htl dts` both declare
    /// through `host_decl`).
    #[test]
    fn a_request_with_a_rejection_type_argument_is_refused() {
        let ty = |s: &str| syn::parse_str::<Type>(s).unwrap();
        let err = teal_type(&ty("Request<String, i64, String>"), "Self").unwrap_err();
        assert!(
            err.contains("`task.Request` takes two, `Request<Q, A>`"),
            "{err}"
        );
        assert!(err.contains("the rejection reason is a string"), "{err}");
        assert!(err.contains("drop the third"), "{err}");
        let err = match host_of_result(
            "pub struct D;\n\
             #[host_module(name = \"d\")]\n\
             impl D {\n    pub fn calls(&self) -> RecvChannel<Request<String, i64, String>> { todo!() }\n}\n",
        ) {
            Ok(hd) => panic!("expected an error, got {}", hd.decl),
            Err(e) => e,
        };
        assert!(
            err.contains("`task.Request` takes two, `Request<Q, A>`"),
            "{err}"
        );
    }

    /// A `uses` name whose module is not `htl.task` dots ahead of every special case,
    /// including `RecvChannel`/`SendChannel`/`Request`'s own: `mytask::RecvChannel<i64>`
    /// under `uses = [mytask = "my.task"]` is `mytask.RecvChannel` (its generic argument
    /// dropped, the same as a bare unknown identifier would be), not `task.RecvChannel`,
    /// and does not trigger the `htl.task` default (#426).
    #[test]
    fn a_uses_qualified_path_dots_ahead_of_the_special_case_names() {
        let hd = host_of(
            "pub struct D;\n\
             #[host_module(name = \"d\", uses = [mytask = \"my.task\"])]\n\
             impl D {\n    pub fn job(&self) -> mytask::RecvChannel<i64> { todo!() }\n}\n",
        );
        assert!(
            hd.decl
                .contains("job: function(self: d): mytask.RecvChannel\n"),
            "{}",
            hd.decl
        );
        assert!(!hd.decl.contains("htl.task"), "{}", hd.decl);
    }

    /// `uses` threaded all the way through a `#[host_module]` declaration: a parameter
    /// (`ev: types::Event`), an `Option<..>` return and a `Result<.., _>` return (both
    /// cross as the bare `types.Event`, the way `Option` / `Result` always unwrap), and a
    /// field of a record nested through `records = [..]`, which resolves against the
    /// host's `uses` rather than anything of its own (#426).
    #[test]
    fn uses_is_threaded_through_host_decl_params_returns_and_nested_records() {
        let hd = host_of(
            "#[derive(TealRecord)] pub struct Nested { pub ev: types::Event }\n\
             pub struct D;\n\
             #[host_module(name = \"d\", uses = [types = \"somelib.types\"], records = [Nested])]\n\
             impl D {\n\
             \x20   pub fn take(&self, ev: types::Event) -> i64 { 0 }\n\
             \x20   pub fn maybe(&self) -> Option<types::Event> { None }\n\
             \x20   pub fn try_get(&self) -> Result<types::Event, String> { todo!() }\n\
             }\n",
        );
        assert_eq!(
            hd.decl,
            "local type types = require(\"somelib.types\")\n\n\
             local record d\n\
             \x20  record Nested\n      ev: types.Event\n   end\n\
             \x20  take: function(self: d, ev: types.Event): integer\n\
             \x20  maybe: function(self: d): types.Event\n\
             \x20  try_get: function(self: d): types.Event\n\
             end\n\nreturn d\n"
        );
    }

    /// A field whose Rust type happens to be qualified by a module also called `task`,
    /// with no `uses` entry naming it, crosses as a plain `Job` and does not trip the
    /// `htl.task` default: nothing here is one of `htl.task`'s own types (#426).
    #[test]
    fn a_task_qualified_field_with_no_uses_entry_does_not_add_the_htl_task_default() {
        let hd = host_of(
            "pub struct D;\n\
             #[host_module(name = \"d\")]\n\
             impl D {\n    pub fn job(&self) -> task::Job { todo!() }\n}\n",
        );
        assert!(
            hd.decl.contains("job: function(self: d): Job\n"),
            "{}",
            hd.decl
        );
        assert!(!hd.decl.contains("htl.task"), "{}", hd.decl);
    }

    /// `use_list`'s refusals: a right side that is not a string literal, a left side
    /// that is not a single identifier, a module path with a character `require("...")`
    /// could not carry unescaped, and a local name written
    /// twice — bare or `= ".."`, against each other or against itself.
    #[test]
    fn uses_refuses_a_malformed_module_path_entry() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [t = 5])] pub struct R { pub x: t::T }",
        ))
        .unwrap_err();
        assert!(e.contains("string literal"), "{e}");

        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [a::b = \"m\"])] pub struct R { pub x: i64 }",
        ))
        .unwrap_err();
        assert!(e.contains("single identifier"), "{e}");

        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [t = \"a b\"])] pub struct R { pub x: i64 }",
        ))
        .unwrap_err();
        assert!(e.contains("letters, digits"), "{e}");
        assert!(e.contains("\"a b\""), "{e}");

        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [t = \"\"])] pub struct R { pub x: i64 }",
        ))
        .unwrap_err();
        assert!(e.contains("letters, digits"), "{e}");

        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [x = \"a\", x = \"b\"])] pub struct R { pub y: i64 }",
        ))
        .unwrap_err();
        assert!(e.contains("`x`") && e.contains("twice"), "{e}");

        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(uses = [Mode, Mode = \"m\"])] pub struct R { pub y: i64 }",
        ))
        .unwrap_err();
        assert!(e.contains("`Mode`") && e.contains("twice"), "{e}");
    }

    fn host_of(src: &str) -> HostDecl {
        host_of_result(src).unwrap()
    }

    fn host_of_result(src: &str) -> Result<HostDecl, String> {
        let file: syn::File = syn::parse_str(src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        host_decl(imp, attrs, Some(&file.items), None)
    }

    /// `htl::task`'s channels and requests are declared as `htl.task`'s types, and the
    /// file imports that module as `task` on its first line.
    #[test]
    fn task_channels_are_declared_from_htl_task_which_the_file_imports() {
        let hd = host_of(
            "#[derive(TealRecord)] pub struct Event { pub n: i64 }\n\
             #[derive(TealRecord)] pub struct Report { pub text: String }\n\
             pub struct D;\n\
             #[host_module(name = \"d\", records = [Event, Report])]\n\
             impl D {\n\
             \x20   pub fn events(&self) -> htl::task::RecvChannel<Event> { todo!() }\n\
             \x20   pub fn reports(&self) -> SendChannel<Report> { todo!() }\n\
             \x20   pub fn give(&self, out: SendChannel<Report>) {}\n\
             \x20   pub fn calls(&self) -> RecvChannel<Request<String, i64>> { todo!() }\n\
             }\n",
        );
        assert_eq!(
            hd.decl,
            "local type task = require(\"htl.task\")\n\n\
             local record d\n\
             \x20  record Event\n      n: integer\n   end\n\
             \x20  record Report\n      text: string\n   end\n\
             \x20  events: function(self: d): task.RecvChannel<Event>\n\
             \x20  reports: function(self: d): task.SendChannel<Report>\n\
             \x20  give: function(self: d, out: task.SendChannel<Report>)\n\
             \x20  calls: function(self: d): task.RecvChannel<task.Request<string, integer>>\n\
             end\n\nreturn d\n"
        );
    }

    /// The import goes in front of `uses` lines, and only where `task.` is a type of
    /// `htl.task`: a declaration that does not mention one, or mentions a name that only
    /// ends in `task`, gets none; a `Request` of the host's own (no type arguments) stays
    /// its own name.
    #[test]
    fn the_task_import_is_written_only_for_a_task_type() {
        let hd = host_of(
            "pub struct D;\n\
             #[host_module(name = \"d\", uses = [Mode])]\n\
             impl D {\n    pub fn events(&self) -> RecvChannel<Mode> { todo!() }\n}\n",
        );
        assert!(
            hd.decl.starts_with(
                "local type task = require(\"htl.task\")\nlocal type Mode = require(\"Mode\")\n\nlocal record d\n"
            ),
            "{}",
            hd.decl
        );
        let plain = host_of(
            "pub struct D;\n\
             #[host_module(name = \"subtask\", uses = [Request])]\n\
             impl D {\n    pub fn me(&self) -> Self { todo!() }\n    pub fn r(&self, q: Request) -> Request { q }\n}\n",
        );
        assert!(!plain.decl.contains("htl.task"), "{}", plain.decl);
        assert!(
            plain
                .decl
                .contains("r: function(self: subtask, q: Request): Request"),
            "{}",
            plain.decl
        );
    }

    /// A user `uses` entry naming `task` for `htl.task` itself takes the default entry's
    /// place (one line, no error); naming `task` for a *different* module, where the
    /// declaration also needs `htl.task`, is refused rather than silently preferred: a
    /// `.d.tl` cannot import two modules under one local name (#426).
    #[test]
    fn a_user_uses_entry_named_task_for_htl_task_is_accepted_any_other_module_is_refused() {
        // A user `uses` entry naming `task` for `htl.task` itself is the same import the
        // default would have written: accepted, one line, no error.
        let hd = host_of(
            "pub struct D;\n\
             #[host_module(name = \"d\", uses = [task = \"htl.task\"])]\n\
             impl D {\n    pub fn events(&self) -> RecvChannel<Mode> { todo!() }\n}\n",
        );
        assert!(
            hd.decl
                .starts_with("local type task = require(\"htl.task\")\n\nlocal record d\n"),
            "{}",
            hd.decl
        );

        // A `task` entry for a *different* module, where the declaration also needs
        // `htl.task`, cannot be resolved into one `.d.tl`: refused, naming both.
        let err = match host_of_result(
            "pub struct D;\n\
             #[host_module(name = \"d\", uses = [task = \"something.else\"])]\n\
             impl D {\n    pub fn events(&self) -> RecvChannel<Mode> { todo!() }\n}\n",
        ) {
            Ok(hd) => panic!("expected an error, got {}", hd.decl),
            Err(e) => e,
        };
        assert!(err.contains("something.else"), "{err}");
        assert!(err.contains("htl.task"), "{err}");
        assert!(err.starts_with("host_module `d`:"), "{err}");
    }

    /// Every kind nested in a host module: what `records = [..]` writes, indented, with
    /// the variant records reachable as `host.Shape_Circle`.
    #[test]
    fn every_kind_nests_in_a_host_module() {
        let file: syn::File = syn::parse_str(
            "#[derive(TealRecord)] pub enum Mode { Fast, Careful }\n\
             #[derive(TealRecord)] pub struct Label(pub String);\n\
             #[derive(TealRecord)] pub enum Shape { Dot, Circle(f64) }\n\
             #[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [Mode, Label, Shape, Point])]\n\
             impl Host {\n    pub fn area(&self, s: Shape, m: Mode) -> Label { todo!() }\n}\n",
        )
        .unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let hd = host_decl(imp, attrs, Some(&file.items), None).unwrap();
        assert_eq!(
            hd.decl,
            "local record host\n\
             \x20  enum Mode\n      \"Fast\"\n      \"Careful\"\n   end\n\
             \x20  type Label = string\n\
             \x20  record Shape_Dot\n      where self.kind == \"Dot\"\n      kind: string\n   end\n\
             \x20  record Shape_Circle\n      where self.kind == \"Circle\"\n      kind: string\n      value: number\n   end\n\
             \x20  type Shape = Shape_Dot | Shape_Circle\n\
             \x20  record Point\n      x: number\n   end\n\
             \x20  area: function(self: host, s: Shape, m: Mode): Label\n\
             end\n\nreturn host\n"
        );
    }

    /// `kind` is the tag on every variant record; a payload field of that name would be
    /// declared twice and could not come back as the value that went in.
    #[test]
    fn a_struct_variant_with_a_field_named_kind_is_refused() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub enum Op { Get, Set { kind: String, n: i64 } }",
        ))
        .unwrap_err();
        assert_eq!(
            e,
            "TealRecord: Op::Set: a field named `kind` collides with the variant tag"
        );
    }

    fn host_impl(src: &str) -> HostDecl {
        let file: syn::File = syn::parse_str(src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        host_decl(imp, attrs, Some(&file.items), None).unwrap()
    }

    /// `name = "std.fs"` splits `HostDecl::module` (the whole string) from the
    /// declaration's record (`fs`, its last segment) — see `htl_macros::host_module`'s
    /// doc for why.
    #[test]
    fn a_dotted_name_declares_the_record_under_its_last_segment() {
        let hd = host_impl(
            "pub struct Fs;\n\
             #[host_module(name = \"std.fs\")]\n\
             impl Fs {\n\
             \x20   pub async fn read(&self, path: String) -> String { todo!() }\n\
             }\n",
        );
        assert_eq!(hd.module, "std.fs");
        assert_eq!(
            hd.decl,
            "local record fs\n\
             \x20  read: function(self: fs, path: string): string ---@async\n\
             end\n\
             \n\
             return fs\n"
        );
    }

    /// An undotted `name` is one segment, so it is its own last segment and the
    /// declaration is byte-identical to before this split existed.
    #[test]
    fn an_undotted_name_is_unaffected() {
        let hd = host_impl(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn ping(&self) -> bool { true }\n\
             }\n",
        );
        assert_eq!(hd.module, "host");
        assert_eq!(
            hd.decl,
            "local record host\n\
             \x20  ping: function(self: host): boolean\n\
             end\n\
             \n\
             return host\n"
        );
    }

    /// Every segment of a dotted `name` has to be a Teal identifier, not only the last
    /// one: a leading digit, an empty segment (`std..fs`) and a Teal keyword (`end`) are
    /// all refused, with a message that says what a valid name is.
    #[test]
    fn an_invalid_dotted_name_is_refused() {
        for bad in ["std.1fs", "std..fs", "std.end"] {
            let e = host_impl_err(&format!(
                "pub struct Fs;\n\
                 #[host_module(name = \"{bad}\")]\n\
                 impl Fs {{\n    pub fn ping(&self) -> bool {{ true }}\n}}\n"
            ));
            assert!(
                e.contains(
                    "host_module: `name` must be one or more `.`-separated Teal \
                             identifiers"
                ) && e.contains(&format!("{bad:?}")),
                "{bad}: {e}"
            );
        }
    }

    /// No `name` was written at all, and the impl target's own name lowercases to a
    /// Teal keyword (`End` -> `end`): the message names the type that collided, not a
    /// `name` nobody wrote, and says what to add.
    #[test]
    fn an_omitted_name_that_lowercases_to_a_keyword_names_the_type() {
        let e = host_impl_err(
            "pub struct End;\n\
             #[host_module]\n\
             impl End {\n    pub fn ping(&self) -> bool { true }\n}\n",
        );
        assert!(
            e.contains("host_module: `End` lowercases to `end`")
                && e.contains("Teal keyword")
                && e.contains("give this `#[host_module]` a `name"),
            "{e}"
        );
    }

    /// An `Option<T>` parameter is what the caller may leave out, and `name?: T` is how
    /// Teal says so; the field and the return keep the plain type.
    #[test]
    fn an_option_parameter_is_declared_optional() {
        let hd = host_impl(
            "pub struct Api;\n\
             #[host_module(name = \"api\")]\n\
             impl Api {\n\
             \x20   pub fn find(&self, name: &str, scope: Option<String>) -> Option<String> { todo!() }\n\
             }\n",
        );
        assert!(
            hd.decl
                .contains("find: function(self: api, name: string, scope?: string): string"),
            "{}",
            hd.decl
        );
    }

    /// Several trailing `Option`s are all marked, and so is a lone one.
    #[test]
    fn every_trailing_option_is_marked() {
        let hd = host_impl(
            "pub struct Api;\n\
             #[host_module(name = \"api\")]\n\
             impl Api {\n\
             \x20   pub fn page(&self, n: i64, size: Option<i64>, cursor: Option<String>) {}\n\
             }\n",
        );
        assert!(
            hd.decl
                .contains("page: function(self: api, n: integer, size?: integer, cursor?: string)"),
            "{}",
            hd.decl
        );
    }

    /// Teal parses `?` only on a trailing run ("non-optional arguments cannot follow
    /// optional arguments"), so an `Option` with a required parameter after it is declared
    /// as the plain type — the declaration stays parseable and says what the caller must
    /// pass.
    #[test]
    fn an_option_followed_by_a_required_parameter_stays_required() {
        let hd = host_impl(
            "pub struct Api;\n\
             #[host_module(name = \"api\")]\n\
             impl Api {\n\
             \x20   pub fn at(&self, scope: Option<String>, n: i64, tail: Option<i64>) {}\n\
             }\n",
        );
        assert!(
            hd.decl
                .contains("at: function(self: api, scope: string, n: integer, tail?: integer)"),
            "{}",
            hd.decl
        );
    }

    /// The mark is a parameter's; a record field and a return value keep the plain type
    /// (a Teal record field is nilable by definition, and a return position has no `?`).
    #[test]
    fn a_field_and_a_return_are_not_marked() {
        let rd = record_decl(&item(
            "#[derive(TealRecord)] pub struct Outcome { pub did: String, pub blocked: Option<String> }",
        ))
        .unwrap();
        assert_eq!(
            rd.decl,
            "local record Outcome\n   did: string\n   blocked: string\nend\n\nreturn Outcome\n"
        );
        let hd = host_impl(
            "pub struct Api;\n\
             #[host_module(name = \"api\")]\n\
             impl Api {\n\
             \x20   pub fn last(&self) -> Option<String> { todo!() }\n\
             }\n",
        );
        assert!(
            hd.decl.contains("last: function(self: api): string"),
            "{}",
            hd.decl
        );
    }

    /// Every callback shape of a `#[host_module]`, written as `htl dts` and the macro read it.
    const CALLBACKS: &str = "pub struct Api;\n\
         #[host_module(name = \"api\", dts = \"types/api.d.tl\")]\n\
         impl Api {\n\
         \x20   pub fn each(&self, f: Function) -> String { todo!() }\n\
         \x20   pub fn walk(&self, f: Option<Function>) {}\n\
         \x20   pub fn split(&self, on_ok: mlua::Function, on_err: &Function) {}\n\
         \x20   pub async fn each_async(&self, f: Function) -> String { todo!() }\n\
         \x20   pub fn on(&self, #[teal(yields)] f: Function) {}\n\
         \x20   pub async fn each_sync_call(&self, #[teal(noyield)] f: Function) -> String { todo!() }\n\
         \x20   pub fn map(&self, xs: Vec<i64>, f: Function) -> Vec<i64> { todo!() }\n\
         \x20   pub fn plain(&self, n: i64) -> i64 { n }\n\
         }\n";

    const CALLBACK_LINES: [&str; 8] = [
        "   each: function(self: api, f: function): string ---@noyield(f)\n",
        "   walk: function(self: api, f?: function) ---@noyield(f)\n",
        "   split: function(self: api, on_ok: function, on_err: function) ---@noyield(on_ok, on_err)\n",
        "   each_async: function(self: api, f: function): string ---@async\n",
        "   on: function(self: api, f: function)\n",
        "   each_sync_call: function(self: api, f: function): string ---@async ---@noyield(f)\n",
        "   map: function(self: api, xs: {integer}, f: function): {integer} ---@noyield(f)\n",
        "   plain: function(self: api, n: integer): integer\n",
    ];

    /// A sync fn names its `Function` parameters in `---@noyield(..)`, an `async fn` does
    /// not, and `#[teal(yields)]` / `#[teal(noyield)]` turn either default around.
    #[test]
    fn a_function_parameter_of_a_sync_fn_is_marked_noyield() {
        let hd = host_impl(CALLBACKS);
        for line in CALLBACK_LINES {
            assert!(hd.decl.contains(line), "{line:?} not in\n{}", hd.decl);
        }
        let marked: Vec<(&str, Vec<&str>)> = hd
            .methods
            .iter()
            .map(|m| {
                let names = m
                    .params
                    .iter()
                    .filter(|p| p.noyield)
                    .map(|p| p.name.as_str())
                    .collect();
                (m.name.as_str(), names)
            })
            .collect();
        assert_eq!(
            marked,
            vec![
                ("each", vec!["f"]),
                ("walk", vec!["f"]),
                ("split", vec!["on_ok", "on_err"]),
                ("each_async", vec![]),
                ("on", vec![]),
                ("each_sync_call", vec!["f"]),
                ("map", vec!["f"]),
                ("plain", vec![]),
            ]
        );
    }

    /// `htl dts` scans the file and goes through the same `host_decl`: the text it would
    /// write is the macro's, parameter attributes and all.
    #[test]
    fn htl_dts_writes_the_same_noyield_lines_as_the_macro() {
        let dir = core_common::tempdir("htl-dts-noyield", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let lib = dir.join("src/lib.rs");
        std::fs::write(&lib, CALLBACKS).unwrap();
        let generated = scan_rust_file(&lib, &dir).unwrap();
        assert_eq!(generated.len(), 1);
        assert_eq!(generated[0].target, dir.join("types/api.d.tl"));
        assert_eq!(generated[0].text, host_impl(CALLBACKS).decl);
        for line in CALLBACK_LINES {
            assert!(
                generated[0].text.contains(line),
                "{line:?} not in\n{}",
                generated[0].text
            );
        }
    }

    fn host_impl_err(src: &str) -> String {
        let file: syn::File = syn::parse_str(src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        match host_decl(imp, attrs, Some(&file.items), None) {
            Ok(hd) => panic!("accepted: {}", hd.decl),
            Err(e) => e,
        }
    }

    /// A parameter's `#[teal(..)]` takes the two words and nothing else, on a `Function`,
    /// of a fn that crosses; the rest is refused rather than read as nothing.
    #[test]
    fn a_parameter_attribute_other_than_the_two_words_is_refused() {
        let wrap = |f: &str| {
            format!("pub struct Api;\n#[host_module(name = \"api\")]\nimpl Api {{\n    {f}\n}}\n")
        };
        for (f, want) in [
            (
                "pub fn a(&self, #[teal(name = \"g\")] f: Function) {}",
                "host_module: `a`: `#[teal(..)]` on parameter `f` takes `yields` or `noyield`, got `name`",
            ),
            (
                "pub fn a(&self, #[teal(maybe)] f: Function) {}",
                "host_module: `a`: `#[teal(..)]` on parameter `f` takes `yields` or `noyield`, got `maybe`",
            ),
            (
                "pub fn a(&self, #[teal] f: Function) {}",
                "host_module: `a`: `#[teal(..)]` on parameter `f` takes `yields` or `noyield`",
            ),
            (
                "pub fn a(&self, #[teal()] f: Function) {}",
                "host_module: `a`: `#[teal(..)]` on parameter `f` takes `yields` or `noyield`",
            ),
            (
                "pub fn a(&self, #[teal(yields, noyield)] f: Function) {}",
                "host_module: `a`: parameter `f` is marked both `yields` and `noyield`",
            ),
            (
                "pub fn a(&self, #[teal(yields)] n: i64) {}",
                "host_module: `a`: `#[teal(yields)]` on parameter `n`, which is not a `Function`: the word says how the host calls a Lua function it is handed",
            ),
            (
                "pub async fn a(&self, #[teal(noyield)] t: Table) {}",
                "host_module: `a`: `#[teal(noyield)]` on parameter `t`, which is not a `Function`: the word says how the host calls a Lua function it is handed",
            ),
            (
                "fn a(&self, #[teal(yields)] f: Function) {}",
                "host_module: `a` is not `pub` and does not cross, so `#[teal(..)]` on its parameter `f` says nothing",
            ),
        ] {
            assert_eq!(host_impl_err(&wrap(f)), want, "{f}");
        }
    }

    /// Either word states the fact, so it may repeat the default: the line is the one the
    /// bare parameter gives.
    #[test]
    fn a_word_that_repeats_the_default_changes_nothing() {
        let hd = host_impl(
            "pub struct Api;\n\
             #[host_module(name = \"api\")]\n\
             impl Api {\n\
             \x20   pub fn each(&self, #[teal(noyield)] f: Function) {}\n\
             \x20   pub async fn later(&self, #[teal(yields)] f: Function) {}\n\
             }\n",
        );
        assert!(
            hd.decl
                .contains("   each: function(self: api, f: function) ---@noyield(f)\n"),
            "{}",
            hd.decl
        );
        assert!(
            hd.decl
                .contains("   later: function(self: api, f: function) ---@async\n"),
            "{}",
            hd.decl
        );
    }

    /// A nested item is declared under its Rust name (the host's signatures say that
    /// name), so a `#[teal(name = ..)]` on it can only be refused, with the standalone
    /// route named.
    #[test]
    fn a_renamed_item_cannot_be_nested() {
        let file: syn::File = syn::parse_str(
            "#[derive(TealRecord)] #[teal(name = \"Pt\")] pub struct Point { pub x: f64, pub next: Option<Box<Self>> }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        )
        .unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let e = match host_decl(imp, attrs, Some(&file.items), None) {
            Ok(hd) => panic!("rename accepted: {}", hd.decl),
            Err(e) => e,
        };
        assert!(
            e.contains("`Point` is nested through `records` and cannot be renamed")
                && e.contains("uses = [Pt]"),
            "{e}"
        );
    }

    /// `records` resolves a record directly, or through one module level
    /// (`module::Record`); a path one level deeper is refused where it is parsed, not
    /// wherever it would have been resolved (#428).
    #[test]
    fn records_resolves_one_module_level_a_deeper_path_is_refused() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(records = [a::b::C])] pub struct R { pub x: i64 }",
        ))
        .unwrap_err();
        assert!(e.contains("one module level"), "{e}");
        assert!(e.contains("`a::b::C`"), "{e}");
    }

    /// `records = [{name}]` with nothing by that name in the file: the message no longer
    /// says a record must live in the same file (#428) — it says how to name one from a
    /// `mod` declared there (`records = [<module>::{name}]`), and what the `uses` route
    /// changes on the Teal side if the record is imported as a module of its own instead.
    #[test]
    fn a_bare_record_not_found_names_the_module_route_and_what_uses_changes() {
        let e = host_impl_err(
            "pub struct Host;\n\
             #[host_module(name = \"host\", records = [Point])]\n\
             impl Host {\n    pub fn origin(&self) -> i64 { 0 }\n}\n",
        );
        assert!(e.contains("`Point` not found in this file"), "{e}");
        assert!(e.contains("records = [<module>::Point]"), "{e}");
        assert!(e.contains("mod <module>;"), "{e}");
        assert!(e.contains("uses = [Point]"), "{e}");
        assert!(
            e.contains("`host.Point` becomes `Point` from its own module"),
            "{e}"
        );
    }

    /// `records = [nosuch::Point]` with no `mod nosuch` anywhere in the file: refused
    /// naming the module, not the record (#428 acceptance: "no such `mod` in the file").
    #[test]
    fn a_records_entry_naming_a_module_that_is_not_declared_is_refused() {
        let e = host_impl_err(
            "pub struct Host;\n\
             #[host_module(name = \"host\", records = [nosuch::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> i64 { 0 }\n}\n",
        );
        assert!(e.contains("no `mod nosuch`"), "{e}");
        assert!(e.contains("records = [nosuch::Point]"), "{e}");
    }

    /// `records = [geom::Point]`, `geom` declared inline (`mod geom { .. }`, no sibling
    /// file): resolved directly from the file already in hand, byte-identical to the
    /// same-file form, and nothing is added to `record_files` — there is no second file
    /// to track.
    #[test]
    fn a_module_qualified_record_in_an_inline_mod_needs_no_sibling_file() {
        let file: syn::File = syn::parse_str(
            "mod geom {\n    #[derive(TealRecord)] pub struct Point { pub x: f64 }\n}\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> geom::Point { todo!() }\n}\n",
        )
        .unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let hd = host_decl(imp, attrs, Some(&file.items), None).unwrap();
        assert!(hd.record_files.is_empty(), "{:?}", hd.record_files);
        let same_file = host_impl(
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        );
        assert_eq!(hd.decl, same_file.decl);
    }

    /// `records = [geom::Point]`, `geom` a `mod geom;` with no body of its own: read from
    /// the sibling `geom.rs` next to the `#[host_module]`'s own file, the nested
    /// declaration byte-identical to the same-file form (#428 acceptance 1), and the
    /// sibling's absolute path collected in `record_files` for the macro to track with
    /// `include_str!` (acceptance 3).
    #[test]
    fn a_module_qualified_record_resolves_to_the_sibling_file_byte_identically() {
        let dir = core_common::tempdir("htl-dts-geom", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let lib = dir.join("src/lib.rs");
        std::fs::write(
            &lib,
            "mod geom;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> geom::Point { todo!() }\n}\n",
        )
        .unwrap();
        let geom = dir.join("src/geom.rs");
        std::fs::write(
            &geom,
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n",
        )
        .unwrap();

        let src = std::fs::read_to_string(&lib).unwrap();
        let file: syn::File = syn::parse_str(&src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let hd = host_decl(imp, attrs, Some(&file.items), Some(&lib)).unwrap();

        let same_file = host_impl(
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        );
        assert_eq!(
            hd.decl, same_file.decl,
            "\n{}\n--\n{}",
            hd.decl, same_file.decl
        );
        assert_eq!(
            hd.record_files,
            vec![std::fs::canonicalize(&geom).unwrap()],
            "{:?}",
            hd.record_files
        );
    }

    /// `records = [geom::Nosuch]`, `geom.rs` exists and is read, but has nothing named
    /// `Nosuch`: refused naming the file that was looked in (#428 acceptance: "no such
    /// record in that file").
    #[test]
    fn a_record_missing_from_the_modules_sibling_file_names_the_file() {
        let dir = core_common::tempdir("htl-dts-geom-missing", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let lib = dir.join("src/lib.rs");
        std::fs::write(
            &lib,
            "mod geom;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Nosuch])]\n\
             impl Host {\n    pub fn origin(&self) -> i64 { 0 }\n}\n",
        )
        .unwrap();
        let geom = dir.join("src/geom.rs");
        std::fs::write(
            &geom,
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n",
        )
        .unwrap();

        let src = std::fs::read_to_string(&lib).unwrap();
        let file: syn::File = syn::parse_str(&src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let e = match host_decl(imp, attrs, Some(&file.items), Some(&lib)) {
            Ok(hd) => panic!("accepted: {}", hd.decl),
            Err(e) => e,
        };
        assert!(e.contains("`Nosuch` not found in"), "{e}");
        // Compared to `display_path` of `geom` as this test constructed it (the same
        // join `resolve_module_items` itself does, never canonicalized) — not to
        // `geom.canonicalize()`. The message names the path as read; canonicalizing it
        // is what `record_files` does, for a different reader (`include_str!`), and
        // asserting the message against that form instead would desync the moment the
        // two legitimately differ (a symlinked temp dir, the #424 class) — a `contains`
        // check could still pass by coincidence there (one of the two paths a substring
        // of the other) without the comparison being the right one.
        assert!(
            e.contains(crate::diagnostic::display_path(&geom).as_str()),
            "{e}"
        );
    }

    /// #428: `#[path]` redirects where `mod geom;` reads from; `records` does not
    /// follow it (reading the wrong file silently, or refusing to find anything there,
    /// would both be worse than refusing the attribute outright).
    #[test]
    fn a_path_attribute_on_the_named_mod_is_refused() {
        let e = host_impl_err(
            "#[path = \"other.rs\"]\n\
             mod geom;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> i64 { 0 }\n}\n",
        );
        assert!(e.contains("`#[path]` on `mod geom`"), "{e}");
        assert!(e.contains("not followed by `records`"), "{e}");
    }

    /// #428: `self::Point` names the same record a bare `records = [Point]` does —
    /// `self` just says "this module" — so the two resolve byte-identically.
    #[test]
    fn a_self_prefixed_bare_path_resolves_like_a_bare_one() {
        let self_prefixed = host_impl(
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [self::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        );
        let bare = host_impl(
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        );
        assert_eq!(self_prefixed.decl, bare.decl);
    }

    /// #428: `self::geom::Point` names the same record `geom::Point` does.
    #[test]
    fn a_self_prefixed_module_path_resolves_the_module() {
        let file: syn::File = syn::parse_str(
            "mod geom {\n    #[derive(TealRecord)] pub struct Point { pub x: f64 }\n}\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [self::geom::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> geom::Point { todo!() }\n}\n",
        )
        .unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let hd = host_decl(imp, attrs, Some(&file.items), None).unwrap();
        assert!(
            hd.decl
                .contains("   record Point\n      x: number\n   end\n"),
            "{}",
            hd.decl
        );
    }

    /// #428: `crate::Point` and `super::Point` are refused by name (there is no `mod
    /// crate` or `mod super` to find — refusing early, naming the two forms `records`
    /// does accept, reads better than failing later as "no `mod crate`").
    #[test]
    fn records_refuses_crate_and_super_prefixes() {
        for bad in ["crate::Point", "super::Point"] {
            let e = record_decl(&item(&format!(
                "#[derive(TealRecord)] #[teal(records = [{bad}])] pub struct R {{ pub x: i64 }}"
            )))
            .unwrap_err();
            assert!(
                e.contains("directly") && e.contains("module::Record"),
                "{bad}: {e}"
            );
            assert!(e.contains(bad), "{bad}: {e}");
        }
    }

    /// #428: every leading `self::` is stripped, not just the first — `self::self::Point`
    /// is `Point`, the same as a single `self::Point` or a bare `Point`, rather than
    /// `RecordRef { module: Some("self"), name: "Point" }` (which would fail later as "no
    /// `mod self`", a worse message for a path that named nothing but repeats of "this
    /// module").
    #[test]
    fn every_leading_self_is_stripped() {
        let doubled = host_impl(
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [self::self::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        );
        let bare = host_impl(
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [Point])]\n\
             impl Host {\n    pub fn origin(&self) -> Point { todo!() }\n}\n",
        );
        assert_eq!(doubled.decl, bare.decl);
    }

    /// #428: a leading `::` (`::geom::Point`) is refused rather than silently ignored — in
    /// the 2018+ path grammar it names an external crate, not a module declared in this
    /// file, and `records` only ever reads the segments after it, which look identical to
    /// the same path without the `::` and so would otherwise resolve exactly the same way.
    #[test]
    fn records_refuses_a_leading_double_colon() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(records = [::geom::Point])] pub struct R { pub x: i64 }",
        ))
        .unwrap_err();
        assert!(
            e.contains("directly") && e.contains("module::Record"),
            "{e}"
        );
        assert!(
            e.contains("::geom::Point") && e.contains("external crate"),
            "{e}"
        );
    }

    /// #428: `geom::Point` resolves to the record `mod geom` declares directly, not to
    /// one reached through further nesting inside it — `find_item` (which a bare
    /// `records = [Point]` entry still uses) recurses into inline modules, so without
    /// `find_item_direct` this would have matched `deeper`'s `Point` too.
    #[test]
    fn a_module_qualified_record_does_not_match_one_nested_deeper_inside_the_module() {
        let e = host_impl_err(
            "mod geom {\n    \
             mod deeper {\n        #[derive(TealRecord)] pub struct Point { pub x: f64 }\n    }\n\
             }\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> i64 { 0 }\n}\n",
        );
        assert!(e.contains("`Point` not found in `mod geom`"), "{e}");
    }

    /// #428: a non-root file's `mod geom;` is `<its own stem>/geom.rs`, not
    /// `geom.rs` beside it — `src/hostio.rs` reads `src/hostio/geom.rs`, and a decoy
    /// `src/geom.rs` (what a same-directory-always rule would have read instead) is
    /// never touched, confirmed by its field not showing up.
    #[test]
    fn a_mod_in_a_non_root_file_resolves_beneath_its_own_stem_not_beside_it() {
        let dir = core_common::tempdir("htl-dts-hostio", "default");
        std::fs::create_dir_all(dir.join("src/hostio")).unwrap();
        let hostio = dir.join("src/hostio.rs");
        std::fs::write(
            &hostio,
            "mod geom;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Point])]\n\
             impl Host {\n    pub fn origin(&self) -> geom::Point { todo!() }\n}\n",
        )
        .unwrap();
        // The decoy a same-directory-always rule would have read instead.
        std::fs::write(
            dir.join("src/geom.rs"),
            "#[derive(TealRecord)] pub struct Point { pub decoy: bool }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/hostio/geom.rs"),
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n",
        )
        .unwrap();

        let src = std::fs::read_to_string(&hostio).unwrap();
        let file: syn::File = syn::parse_str(&src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let hd = host_decl(imp, attrs, Some(&file.items), Some(&hostio)).unwrap();
        assert!(
            hd.decl.contains("record Point\n      x: number\n   end\n"),
            "{}",
            hd.decl
        );
        assert!(!hd.decl.contains("decoy"), "{}", hd.decl);
        assert_eq!(
            hd.record_files,
            vec![
                std::fs::canonicalize(dir.join("src/hostio/geom.rs")).unwrap_or_else(|_| {
                    std::path::absolute(dir.join("src/hostio/geom.rs")).unwrap()
                })
            ],
            "{:?}",
            hd.record_files
        );
    }

    /// #428: `tests` / `examples` / `benches` are crate-root directories only when their
    /// own parent holds the package's `Cargo.toml` — `src/tests/helpers.rs` (`tests`
    /// nested under `src/`, nowhere near `Cargo.toml`) is a plain non-root module like
    /// any other, not a `tests/` integration-test root, so its `mod geom;` is beneath its
    /// own stem (`src/tests/helpers/geom.rs`), not beside it (`src/tests/geom.rs`).
    #[test]
    fn mod_base_dir_treats_tests_as_a_root_only_beside_cargo_toml() {
        let dir = core_common::tempdir("htl-dts-tests-dir", "default");
        std::fs::create_dir_all(dir.join("src/tests")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"p\"\n").unwrap();

        // Nested under `src/`: `src` itself holds no `Cargo.toml`, so `src/tests/` is a
        // plain module directory, not an integration-test crate root.
        assert_eq!(
            mod_base_dir(&dir.join("src/tests/helpers.rs")),
            dir.join("src/tests/helpers")
        );
        // At the package root: `tests`'s own parent is `dir`, which holds `Cargo.toml` —
        // a real integration-test root, so its `mod geom;` sits beside it.
        assert_eq!(mod_base_dir(&dir.join("tests/other.rs")), dir.join("tests"));
    }

    /// #428: two entries naming the same module read and parse its file once — the
    /// second entry is a cache hit, so `record_files` holds the path once, not twice.
    #[test]
    fn two_entries_naming_the_same_module_read_its_file_once() {
        let dir = core_common::tempdir("htl-dts-geom-dedup", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let lib = dir.join("src/lib.rs");
        std::fs::write(
            &lib,
            "mod geom;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", records = [geom::Point, geom::Size])]\n\
             impl Host {\n    \
             pub fn origin(&self) -> geom::Point { todo!() }\n    \
             pub fn unit(&self) -> geom::Size { todo!() }\n\
             }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src/geom.rs"),
            "#[derive(TealRecord)] pub struct Point { pub x: f64 }\n\
             #[derive(TealRecord)] pub struct Size { pub w: f64 }\n",
        )
        .unwrap();

        let src = std::fs::read_to_string(&lib).unwrap();
        let file: syn::File = syn::parse_str(&src).unwrap();
        let imp = file
            .items
            .iter()
            .find_map(|i| match i {
                Item::Impl(imp) => Some(imp),
                _ => None,
            })
            .unwrap();
        let attrs = parse_host_module_attr(&imp.attrs).unwrap().unwrap();
        let hd = host_decl(imp, attrs, Some(&file.items), Some(&lib)).unwrap();
        assert_eq!(hd.record_files.len(), 1, "{:?}", hd.record_files);
    }

    /// `#[htl::host_module(..)]` is the same attribute as `#[host_module(..)]` after
    /// `use htl::host_module`: the model's scan names the module, and `htl dts` writes its
    /// declaration, for either spelling (#327).
    #[test]
    fn a_host_module_is_read_with_or_without_its_crate_path() {
        for (tag, attr) in [
            (
                "bare",
                "#[host_module(name = \"host\", dts = \"src/host.d.tl\")]",
            ),
            (
                "path",
                "#[htl::host_module(name = \"host\", dts = \"src/host.d.tl\")]",
            ),
        ] {
            let dir = core_common::tempdir("htl-dts-attr-path", tag);
            std::fs::create_dir_all(dir.join("src")).unwrap();
            let lib = dir.join("src/lib.rs");
            std::fs::write(
                &lib,
                format!(
                    "pub struct Host;\n{attr}\nimpl Host {{\n    pub fn ping(&self) -> i64 {{ 1 }}\n}}\n"
                ),
            )
            .unwrap();
            assert_eq!(host_module_names(&dir), vec!["host".to_string()], "{tag}");
            let generated = scan_rust_file(&lib, &dir).unwrap();
            assert_eq!(generated.len(), 1, "{tag}");
            assert_eq!(generated[0].target, dir.join("src/host.d.tl"), "{tag}");
            assert!(
                generated[0].text.contains("ping"),
                "{tag}: {}",
                generated[0].text
            );
        }
    }

    // ---------------------------------------------------------------- &Lua parameter (#427)

    /// A `&Lua` parameter is filled from the closure's own Lua handle rather than from the
    /// Lua arguments: left out of the declaration entirely, the way `&self` is — the
    /// method's Teal arity is unaffected by its presence or its position among the others.
    #[test]
    fn a_lua_parameter_is_left_out_of_the_declaration() {
        let hd = host_of(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn count(&self, lua: &htl::mlua::Lua) -> i64 { lua.used_memory() as i64 }\n\
             \x20   pub fn mixed(&self, lua: &::htl::mlua::Lua, n: i64) -> i64 { n }\n\
             \x20   pub fn before(&self, lua: &mlua::Lua, name: &str) -> String { name.to_string() }\n\
             }\n",
        );
        assert!(
            hd.decl.contains("count: function(self: host): integer\n"),
            "{}",
            hd.decl
        );
        assert!(
            hd.decl
                .contains("mixed: function(self: host, n: integer): integer\n"),
            "{}",
            hd.decl
        );
        assert!(
            hd.decl
                .contains("before: function(self: host, name: string): string\n"),
            "{}",
            hd.decl
        );
        let m = hd.methods.iter().find(|m| m.name == "count").unwrap();
        assert!(m.params.is_empty(), "{}", hd.decl);
        let lp = m.lua_param.as_ref().unwrap();
        assert_eq!(lp.name, "lua");
        assert_eq!(lp.index, 0);

        let m = hd.methods.iter().find(|m| m.name == "mixed").unwrap();
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "n");
        let lp = m.lua_param.as_ref().unwrap();
        assert_eq!(lp.index, 0, "lua came first in the signature");

        let m = hd.methods.iter().find(|m| m.name == "before").unwrap();
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "name");
        let lp = m.lua_param.as_ref().unwrap();
        assert_eq!(lp.index, 0, "lua still came first; `name` is index 1");
    }

    /// A second `&Lua` parameter is refused.
    #[test]
    fn a_second_lua_parameter_is_refused() {
        let e = match host_of_result(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn f(&self, a: &htl::mlua::Lua, b: &htl::mlua::Lua) -> i64 { 0 }\n\
             }\n",
        ) {
            Ok(hd) => panic!("second &Lua parameter accepted: {}", hd.decl),
            Err(e) => e,
        };
        assert!(
            e.contains("only one `&Lua` parameter is allowed") && e.contains('b'),
            "{e}"
        );
    }

    /// `#[teal(..)]` on a `&Lua` parameter is refused: the word says how the host calls a
    /// Lua function it is handed, and a `&Lua` is not one.
    #[test]
    fn teal_attribute_on_a_lua_parameter_is_refused() {
        let e = match host_of_result(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn f(&self, #[teal(noyield)] lua: &htl::mlua::Lua) -> i64 { 0 }\n\
             }\n",
        ) {
            Ok(hd) => panic!("#[teal(..)] on a &Lua parameter accepted: {}", hd.decl),
            Err(e) => e,
        };
        assert!(
            e.contains("whose type is `&Lua`") && e.contains("takes no word"),
            "{e}"
        );
    }

    /// `lua: Lua` (no reference) is refused by name here, instead of being declared
    /// `lua: Lua` in the `.d.tl` and failing at `cargo build` with mlua's own
    /// `FromLuaMulti` error — the same failure `&Lua` produced before #427.
    #[test]
    fn an_owned_lua_parameter_is_refused() {
        let e = match host_of_result(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn f(&self, lua: htl::mlua::Lua) -> i64 { 0 }\n\
             }\n",
        ) {
            Ok(hd) => panic!("owned Lua accepted: {}", hd.decl),
            Err(e) => e,
        };
        assert!(
            e.contains('`') && e.contains("lua") && e.contains("take `&Lua`, not `Lua`"),
            "{e}"
        );
    }

    /// A `&Lua` parameter after another: its position among the typed parameters is where
    /// it sat in the signature (1, not 0), and the regular parameter before it keeps its
    /// own place in the declaration.
    #[test]
    fn a_lua_parameter_after_another_parameter_keeps_its_signature_position() {
        let hd = host_of(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn after(&self, n: i64, lua: &htl::mlua::Lua) {}\n\
             }\n",
        );
        assert!(
            hd.decl
                .contains("after: function(self: host, n: integer)\n"),
            "{}",
            hd.decl
        );
        let m = hd.methods.iter().find(|m| m.name == "after").unwrap();
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "n");
        assert_eq!(m.lua_param.as_ref().unwrap().index, 1);
    }

    /// `&mut self` with a `&Lua` parameter: the receiver kind and the Lua parameter are
    /// independent (`add_method_mut`'s closure is `Fn(&Lua, &mut T, A)`, `&Lua` same as
    /// `add_method`'s).
    #[test]
    fn a_mut_self_method_may_also_take_lua() {
        let hd = host_of(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn bump(&mut self, lua: &htl::mlua::Lua) -> i64 { 0 }\n\
             }\n",
        );
        assert!(
            hd.decl.contains("bump: function(self: host): integer\n"),
            "{}",
            hd.decl
        );
        let m = hd.methods.iter().find(|m| m.name == "bump").unwrap();
        assert_eq!(m.receiver, Some(true));
        assert!(m.lua_param.is_some());
    }

    /// No receiver (an associated fn, `add_function`'s closure is also `Fn(&Lua, A)`): the
    /// `&Lua` parameter works the same way, at its own index.
    #[test]
    fn a_lua_parameter_with_no_receiver() {
        let hd = host_of(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn f(lua: &htl::mlua::Lua, n: i64) -> i64 { n }\n\
             }\n",
        );
        assert!(
            hd.decl.contains("f: function(n: integer): integer\n"),
            "{}",
            hd.decl
        );
        let m = hd.methods.iter().find(|m| m.name == "f").unwrap();
        assert_eq!(m.receiver, None);
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "n");
        assert_eq!(m.lua_param.as_ref().unwrap().index, 0);
    }

    /// A `&Lua` parameter written as a bare `_`: its generated name ([`LuaParam::name`]'s
    /// fallback, `a0`) is never read back out of the pattern, so an unnameable parameter
    /// works exactly like a named one.
    #[test]
    fn a_lua_parameter_written_as_a_bare_underscore() {
        let hd = host_of(
            "pub struct Host;\n\
             #[host_module(name = \"host\")]\n\
             impl Host {\n\
             \x20   pub fn f(&self, _: &htl::mlua::Lua, n: i64) -> i64 { n }\n\
             }\n",
        );
        assert!(
            hd.decl
                .contains("f: function(self: host, n: integer): integer\n"),
            "{}",
            hd.decl
        );
        let m = hd.methods.iter().find(|m| m.name == "f").unwrap();
        assert_eq!(m.params.len(), 1);
        assert_eq!(m.params[0].name, "n");
        assert!(m.lua_param.is_some());
    }

    /// S1 of #429's review: `regenerate_host_decls` writes a `#[host_module]`'s `.d.tl`
    /// but leaves a `#[derive(TealRecord)]`'s alone, even though `scan_rust_file` can see
    /// both in the same file. Seeded with a record `.d.tl` that does not match what a
    /// raw-source rescan of the file would produce (standing in for the derive's own
    /// `#[cfg]`-aware write, which this scan cannot reproduce from text alone) — if the
    /// scan touched records too, this would come back overwritten.
    #[test]
    fn regenerate_host_decls_writes_host_modules_but_leaves_records_alone() {
        let dir = core_common::tempdir("htl-dts-regen-kind", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "use htl::host_module;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", dts = \"src/host.d.tl\")]\n\
             impl Host {\n\
             \x20   pub fn ping(&self) -> i64 { 1 }\n\
             }\n\
             #[derive(htl::TealRecord)]\n\
             #[teal(dts = \"src/point.d.tl\")]\n\
             pub struct Point { pub x: f64 }\n",
        )
        .unwrap();
        // Stands in for whatever the derive itself last wrote under some `#[cfg]`; a
        // plain rescan of the source above would not produce this text.
        let seeded =
            "local record Point\n   x: number\n   from_cfg: boolean\nend\n\nreturn Point\n";
        std::fs::write(dir.join("src/point.d.tl"), seeded).unwrap();

        regenerate_host_decls(&dir);

        assert!(
            std::fs::read_to_string(dir.join("src/host.d.tl"))
                .unwrap()
                .contains("ping"),
            "the host module's own .d.tl should have been written"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("src/point.d.tl")).unwrap(),
            seeded,
            "a record's .d.tl must not be touched by this scan"
        );
    }

    /// S2 of #429's review: a file this cannot parse — a fixture deliberately holding
    /// invalid syntax, the way a trybuild case might — does not stop the rest of the
    /// crate's host modules from being written, and `regenerate_host_decls` reports
    /// nothing about it (it has no error channel to report through).
    #[test]
    fn regenerate_host_decls_skips_an_unparsable_file_and_still_writes_the_rest() {
        let dir = core_common::tempdir("htl-dts-regen-bad", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("tests")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "use htl::host_module;\n\
             pub struct Host;\n\
             #[host_module(name = \"host\", dts = \"src/host.d.tl\")]\n\
             impl Host {\n\
             \x20   pub fn ping(&self) -> i64 { 1 }\n\
             }\n",
        )
        .unwrap();
        // Unparsable, but contains the `host_module` substring so the prefilter still
        // hands it to `syn` — which is exactly the file this is meant to survive.
        std::fs::write(
            dir.join("tests/broken.rs"),
            "#[host_module此 not even close to valid Rust (",
        )
        .unwrap();

        regenerate_host_decls(&dir);

        assert!(
            std::fs::read_to_string(dir.join("src/host.d.tl"))
                .unwrap()
                .contains("ping"),
            "the valid file's host module should still have been written"
        );
    }

    /// S3: two scans of an unchanged tree agree; adding a file (even with no content
    /// relevant to any `#[host_module]`) changes the count half of the fingerprint, which
    /// is what tells `regenerate_crate`'s cache to redo the write pass.
    #[test]
    fn scan_fingerprint_changes_when_a_file_is_added() {
        let dir = core_common::tempdir("htl-dts-fingerprint", "default");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();

        let first = scan_fingerprint(&dir);
        assert_eq!(first, scan_fingerprint(&dir), "unchanged tree, same answer");

        std::fs::write(dir.join("src/extra.rs"), "pub fn g() {}\n").unwrap();
        let second = scan_fingerprint(&dir);
        assert_ne!(first, second, "a file was added");
    }
}
