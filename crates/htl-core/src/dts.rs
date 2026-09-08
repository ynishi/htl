//! Teal declaration (`.d.tl`) generation from Rust source.
//!
//! Shared by the proc macros (`#[host_module]` / `#[derive(TealRecord)]` at expansion
//! time) and by `htl dts`, which scans `.rs` files and writes the same declarations
//! *before* any `cargo build`, so `htl check` works on a fresh checkout.
//!
//! Type mapping is syntactic: `f64 -> number`, integers -> `integer`, `String`/`&str`
//! -> `string`, `bool -> boolean`, `Vec<T> -> {T}`, `HashMap<K, V> -> {K:V}`,
//! `Option<T> -> T`, `Result<T, _> -> T`, other identifiers pass through as record names.
//! An `Option<T>` *parameter* is declared `name?: T` where Teal accepts the mark (a
//! trailing run of them); a field and a return value stay `T`.
//!
//! # Why `#[derive(TealRecord)]` lowers the way it does
//!
//! What each Rust shape becomes on the Teal side is tabled in README "Embedding in
//! Rust"; this is the reasoning behind the choices there.
//!
//! - **A data-carrying enum is a union of `where`-discriminated records**, not one
//!   record with every variant's fields optional. Teal refuses a plain union of two
//!   table types (it cannot tell them apart at run time), and a `where` clause on each
//!   record is its own answer to that; with it, `is N_A` narrows and `union-exhaustive`
//!   counts the variants, which is the whole point of declaring a closed set.
//! - **It can only be declared nested** (`records = [N]` in the host module). A caller
//!   narrows with `is module.N_A`, so it needs the variant records by name, and a
//!   `.d.tl` module exports one name — the union. `#[teal(dts = ..)]` on one is refused
//!   with that advice rather than writing a declaration nobody can narrow against.
//! - **`uses` imports with `local type X = require("X")`**. A module whose value is only
//!   a type (an alias, an enum) is "abstract" to Teal when required as a value, and the
//!   `type` form imports a record just the same, so one form serves every kind.
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
//!   Record fields are declared under their Rust names; renaming those is a separate
//!   decision nobody has asked for, so `#[teal(..)]` on a field is refused.

use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, GenericArgument, ImplItem, Item, ItemEnum, ItemImpl, ItemStruct, Lit,
    Meta, Pat, PathArguments, ReturnType, Token, Type,
};

// ---------------------------------------------------------------- type mapping

/// Map a Rust type to a Teal type name. `self_name` replaces `Self`.
pub fn teal_type(ty: &Type, self_name: &str) -> Result<String, String> {
    match ty {
        Type::Reference(r) => teal_type(&r.elem, self_name),
        Type::Paren(p) => teal_type(&p.elem, self_name),
        Type::Tuple(t) if t.elems.is_empty() => Ok(String::new()),
        Type::Tuple(t) => {
            let parts: Result<Vec<_>, _> =
                t.elems.iter().map(|e| teal_type(e, self_name)).collect();
            Ok(parts?.join(", "))
        }
        Type::Slice(s) => Ok(format!("{{{}}}", teal_type(&s.elem, self_name)?)),
        Type::Array(a) => Ok(format!("{{{}}}", teal_type(&a.elem, self_name)?)),
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
                    .and_then(|t| teal_type(t, self_name))
            };
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
                // mlua's handles to a userdata value: the Teal side sees the host type
                // itself, which is what `open(..) -> Session` declared on the way out.
                "UserDataRef" | "UserDataRefMut" | "UserDataOwned" => arg(0)?,
                "Value" => "any".into(),
                "Table" => "{any:any}".into(),
                "Function" => "function".into(),
                "LuaString" => "string".into(),
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

/// How an enum variant's Rust name is spelled on the Teal side: `#[teal(rename_all =
/// "..")]`, serde's set and serde's spellings of it, so a type that is also `Serialize`
/// can say the same thing twice and the two agree.
///
/// The rules are serde's, applied to a variant name (`InReview`): `lowercase` lowers the
/// whole word, `snake_case` breaks it at each capital, and the rest follow from those
/// two.
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
}

/// `#[teal(...)]` / `#[host_module(...)]` arguments.
#[derive(Debug, Clone, Default)]
pub struct TealAttrs {
    pub name: Option<String>,
    /// `#[teal(rename_all = "..")]` on an enum: how its variants are spelled on the Teal
    /// side. The Rust names are unchanged, and so are the union's variant record names
    /// (`Shape_InReview`) — a Teal identifier cannot be `kebab-case`; what the rule
    /// spells is the word that crosses.
    pub rename_all: Option<RenameRule>,
    /// `.d.tl` output path, relative to the crate's `CARGO_MANIFEST_DIR`.
    pub dts: Option<String>,
    /// Types declared in their own `.d.tl` module: emits `local type X = require("X")`.
    pub uses: Vec<String>,
    /// `#[derive(TealRecord)]` types (structs and enums in the same source file) nested
    /// inside the module record.
    pub records: Vec<String>,
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

fn type_list(arr: &syn::ExprArray, key: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for e in &arr.elems {
        match e {
            Expr::Path(p) => out.push(
                p.path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default(),
            ),
            _ => return Err(format!("`{key}` expects a list of type names")),
        }
    }
    Ok(out)
}

/// Parse `name = "..", rename_all = "..", dts = "..", uses = [A, B], records = [C]`.
///
/// Every key is parsed here whatever it sits on; where a key is meaningful is decided by
/// the caller (`rename_all` on an enum, `name` alone on a variant).
pub fn parse_attr_metas(metas: impl IntoIterator<Item = Meta>) -> Result<TealAttrs, String> {
    let mut out = TealAttrs::default();
    for meta in metas {
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
            ("uses", Expr::Array(arr)) => out.uses = type_list(arr, "uses")?,
            ("records", Expr::Array(arr)) => out.records = type_list(arr, "records")?,
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

fn parse_named_attr(attrs: &[Attribute], name: &str) -> Result<Option<TealAttrs>, String> {
    let mut metas = Vec::new();
    let mut found = false;
    for a in attrs {
        if a.path().is_ident(name) {
            found = true;
            if let Meta::List(_) = &a.meta {
                let list = a
                    .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                    .map_err(|e| e.to_string())?;
                metas.extend(list);
            }
        }
    }
    if !found {
        return Ok(None);
    }
    parse_attr_metas(metas).map(Some)
}

/// `#[teal(...)]` on a struct or enum (absent -> defaults).
pub fn parse_teal_attrs(attrs: &[Attribute]) -> Result<TealAttrs, String> {
    Ok(parse_named_attr(attrs, "teal")?.unwrap_or_default())
}

/// `#[host_module(...)]` on an impl block, or `None` when the attribute is absent.
pub fn parse_host_module_attr(attrs: &[Attribute]) -> Result<Option<TealAttrs>, String> {
    parse_named_attr(attrs, "host_module")
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

/// `local type X = require("X")` per `uses` entry. The `type` form is the one that
/// imports an alias or an enum — a module whose value is only a type is "abstract" to
/// Teal when required as a value — and it imports a record just the same, so one form
/// serves every kind a `.d.tl` can return.
fn uses_header(uses: &[String]) -> String {
    let mut s = String::new();
    for u in uses {
        s.push_str(&format!("local type {u} = require(\"{u}\")\n"));
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
    Record { fields: Vec<(String, String)> },
    /// `struct N(T)`: `type N = T`, crossing as `T` does.
    Alias { inner: String },
    /// An enum of unit variants: `enum N "A" "B" end`, crossing as the variant name —
    /// as `#[teal(rename_all)]` / `#[teal(name)]` spell it, in declaration order.
    Enum { variants: Vec<String> },
    /// An enum with a data variant: one `where`-discriminated record per variant and
    /// `type N = N_A | N_B`, crossing as a table whose `kind` names the variant.
    Union { variants: Vec<UnionVariant> },
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

#[derive(Debug, Clone)]
pub struct RecordDecl {
    pub name: String,
    pub kind: RecordKind,
    /// Full module text: `local record NAME ... end  return NAME` (or the `enum` /
    /// `type` form; a union's is the top-level form, which only `DECL` ever holds).
    pub decl: String,
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

fn record_fields(
    fields: &syn::FieldsNamed,
    self_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for f in &fields.named {
        let fi = f.ident.as_ref().unwrap().to_string();
        // A field carries no `#[teal(..)]`: renaming one is a decision nobody has taken,
        // and an attribute that is quietly ignored is worse than one that is refused.
        if f.attrs.iter().any(|a| a.path().is_ident("teal")) {
            return Err(format!(
                "TealRecord: {self_name}.{fi}: `#[teal(..)]` on a record field is not supported; \
                 fields are declared under their Rust names"
            ));
        }
        let tt = teal_type(&f.ty, self_name)?;
        out.push((fi, tt));
    }
    Ok(out)
}

/// The kind a struct lowers to: named fields -> record, one unnamed field -> alias.
fn struct_kind(st: &ItemStruct, name: &str) -> Result<RecordKind, String> {
    match &st.fields {
        syn::Fields::Named(fields) => Ok(RecordKind::Record {
            fields: record_fields(fields, name)?,
        }),
        syn::Fields::Unnamed(u) if u.unnamed.len() == 1 => Ok(RecordKind::Alias {
            inner: teal_type(&u.unnamed[0].ty, name)?,
        }),
        syn::Fields::Unnamed(_) => Err(format!(
            "TealRecord: `{name}` is a tuple struct with more than one field; only a newtype (`struct {name}(T)`) lowers to a Teal type"
        )),
        syn::Fields::Unit => Err(format!(
            "TealRecord: `{name}` is a unit struct and has nothing to declare"
        )),
    }
}

/// The Teal spelling of one variant: `#[teal(name = "..")]` on it, else the enum's
/// `#[teal(rename_all = "..")]`, else the Rust name. `name` on a variant is the override,
/// so it wins; nothing else may be written there (a variant has no `.d.tl` of its own to
/// ask for, and `rename_all` over one variant is what `name` already is).
fn variant_word(
    v: &syn::Variant,
    rule: Option<RenameRule>,
    enum_name: &str,
) -> Result<String, String> {
    let attrs = parse_teal_attrs(&v.attrs)
        .map_err(|e| format!("TealRecord: {enum_name}::{}: {e}", v.ident))?;
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
    Ok(match attrs.name {
        Some(n) => n,
        None => match rule {
            Some(r) => r.apply(&v.ident.to_string()),
            None => v.ident.to_string(),
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
/// `where`-discriminated records.
fn enum_kind(en: &ItemEnum, name: &str, rule: Option<RenameRule>) -> Result<RecordKind, String> {
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
                VariantShape::Newtype(teal_type(&u.unnamed[0].ty, name)?)
            }
            syn::Fields::Unnamed(_) => {
                return Err(
                    "TealRecord: tuple variants with more than one field are not supported"
                        .to_string(),
                );
            }
            syn::Fields::Named(fields) => {
                // The tag is `kind`, on every variant record; a payload field of that
                // name would be declared twice and could not survive a round trip.
                if fields
                    .named
                    .iter()
                    .any(|f| f.ident.as_ref().is_some_and(|i| i == "kind"))
                {
                    return Err(format!(
                        "TealRecord: {name}::{vname}: a field named `kind` collides with the variant tag"
                    ));
                }
                VariantShape::Struct(record_fields(fields, name)?)
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

/// Name, attributes and kind of a `#[derive(TealRecord)]` item.
fn record_parts(item: &Item) -> Result<(String, TealAttrs, RecordKind), String> {
    let (attrs, ident) = match item {
        Item::Struct(st) => (parse_teal_attrs(&st.attrs)?, st.ident.to_string()),
        Item::Enum(en) => (parse_teal_attrs(&en.attrs)?, en.ident.to_string()),
        _ => return Err("TealRecord: only structs and enums are supported".into()),
    };
    let name = attrs.name.clone().unwrap_or(ident);
    let kind = match item {
        Item::Struct(st) => {
            // `rename_all` spells variants, and a struct has none. Field renaming is a
            // separate decision, so this is refused rather than silently ignored.
            if attrs.rename_all.is_some() {
                return Err(format!(
                    "TealRecord: `{name}`: `rename_all` applies to enum variants; \
                     record fields are declared under their Rust names"
                ));
            }
            struct_kind(st, &name)?
        }
        Item::Enum(en) => enum_kind(en, &name, attrs.rename_all)?,
        _ => unreachable!(),
    };
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
        RecordKind::Record { fields } => {
            s.push_str(&format!("{indent}{local}record {name}\n"));
            for (f, t) in fields {
                s.push_str(&format!("{inner}{f}: {t}\n"));
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
                    s.push_str(&format!("{inner}{f}: {t}\n"));
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
    let mut decl = uses_header(&attrs.uses);
    decl.push_str(&kind_decl(&name, &kind, ""));
    decl.push_str(&format!("\nreturn {name}\n"));
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

fn nested_record_decls(
    names: &[String],
    file_items: Option<&[Item]>,
) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let items = file_items.ok_or_else(|| {
        "host_module: `records` needs the source file (unavailable in this expansion); \
         declare the record in its own module and use `uses` instead"
            .to_string()
    })?;
    let mut out = Vec::new();
    for name in names {
        let it = find_item(items, name).ok_or_else(|| {
            format!(
                "host_module: `{name}` not found in this file (records must live in the same file; \
                 use `uses` for records from other modules)"
            )
        })?;
        // The nested name is the Rust one: `records = [X]` names the item, and the
        // host's signatures say `X` too. A `#[teal(name = ..)]` rename could satisfy
        // neither — and `Self` inside the item would already have been mapped to the
        // rename — so it is refused rather than half-applied.
        let (renamed, attrs, kind) = record_parts(it)?;
        if attrs.name.is_some() {
            return Err(format!(
                "host_module: `{name}` is nested through `records` and cannot be renamed \
                 (`#[teal(name = \"{renamed}\")]`); to declare it as `{renamed}`, give it a \
                 `.d.tl` of its own (`#[teal(dts = ..)]`) and import it with `uses = [{renamed}]`"
            ));
        }
        out.push(kind_decl(name, &kind, "   "));
    }
    Ok(out)
}

// ---------------------------------------------------------------- host modules

#[derive(Clone)]
pub struct HostParam {
    pub name: String,
    /// Type the Lua side hands over (`&str` -> `String`, `&[T]` -> `Vec<T>`, `&T` -> `T`).
    pub owned_ty: Type,
    /// The Rust fn takes a reference; the wrapper passes `&value`.
    pub by_ref: bool,
    pub teal: String,
    /// Declared `name?: T` — an `Option<T>` the Lua caller may leave out. Only a
    /// *trailing* run of them can be marked (see `host_decl`).
    pub optional: bool,
}

#[derive(Clone)]
pub struct HostMethod {
    pub name: String,
    /// `None` = associated fn (no `self`), `Some(false)` = `&self`, `Some(true)` = `&mut self`.
    pub receiver: Option<bool>,
    pub params: Vec<HostParam>,
    /// Teal type of the success value (`T` of `Result<T, E>`, or the plain return); empty for unit.
    pub ret_teal: String,
    pub ret_is_result: bool,
    /// The success value is `()` (nothing to hand back but "it worked").
    pub ret_is_unit: bool,
    /// `async fn`: registered through mlua's async variant, and callable only from inside
    /// a Lua coroutine. The Teal declaration is the same either way — an async function
    /// yields internally and hands back the same values — so this changes the generated
    /// Rust, not the `.d.tl`.
    pub is_async: bool,
}

#[derive(Clone)]
pub struct HostDecl {
    pub type_name: String,
    pub module: String,
    pub decl: String,
    pub methods: Vec<HostMethod>,
    pub attrs: TealAttrs,
    pub err_mode: ErrMode,
}

/// Declaration + wrapper plan for a `#[host_module]` impl block. `file_items` (the
/// enclosing file's items) is only needed when `records = [...]` is used.
pub fn host_decl(
    imp: &ItemImpl,
    attrs: TealAttrs,
    file_items: Option<&[Item]>,
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
    let err_mode = match attrs.errors.as_deref() {
        Some("return") => ErrMode::Return,
        _ => ErrMode::Raise,
    };

    let mut decl = uses_header(&attrs.uses);
    decl.push_str(&format!("local record {module}\n"));
    for r in nested_record_decls(&attrs.records, file_items)? {
        decl.push_str(&r);
    }

    let mut methods = Vec::new();
    for it in &imp.items {
        let ImplItem::Fn(f) = it else { continue };
        if !matches!(f.vis, syn::Visibility::Public(_)) {
            continue;
        }
        let fname = f.sig.ident.to_string();
        let mut receiver: Option<bool> = None;
        let mut params = Vec::new();
        let mut teal_params = Vec::new();
        for a in &f.sig.inputs {
            match a {
                FnArg::Receiver(r) => receiver = Some(r.mutability.is_some()),
                FnArg::Typed(pt) => {
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
                    let teal = teal_type(&owned_ty, &module)?;
                    let optional = is_option(&owned_ty);
                    params.push(HostParam {
                        name: pname,
                        owned_ty,
                        by_ref,
                        teal,
                        optional,
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
            teal_params.insert(0, format!("self: {module}"));
        }
        let (ret_teal, ret_is_result) = match &f.sig.output {
            ReturnType::Default => (String::new(), false),
            ReturnType::Type(_, t) => (teal_type(t, &module)?, is_result(t)),
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
        decl.push_str(&format!(
            "   {fname}: function({}){ret_suffix}\n",
            teal_params.join(", ")
        ));
        methods.push(HostMethod {
            name: fname,
            receiver,
            params,
            ret_teal,
            ret_is_result,
            ret_is_unit,
            is_async: f.sig.asyncness.is_some(),
        });
    }
    decl.push_str(&format!("end\n\nreturn {module}\n"));

    Ok(HostDecl {
        type_name,
        module,
        decl,
        methods,
        attrs,
        err_mode,
    })
}

// ---------------------------------------------------------------- file scanning (`htl dts`)

/// One `.d.tl` derived from a Rust source file.
#[derive(Debug, Clone)]
pub struct Generated {
    /// Absolute output path (`<manifest_dir>/<dts>`).
    pub target: PathBuf,
    pub text: String,
    pub source: PathBuf,
    /// `host_module <module>`, or `RecordDecl::what` (`record <Name>` / `enum <Name>` /
    /// `type <Name>`), for reporting.
    pub what: String,
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
                    let hd = host_decl(imp, attrs, Some(&file.items))?;
                    if let Some(dts) = &hd.attrs.dts {
                        out.push(Generated {
                            target: manifest_dir.join(dts),
                            text: hd.decl.clone(),
                            source: path.to_path_buf(),
                            what: format!("host_module {}", hd.module),
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
                    });
                }
            }
            _ => {}
        }
    }
    Ok(out)
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
    let mut results = Vec::new();
    for sub in ["src", "examples", "tests", "benches"] {
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
                let written = crate::write_if_changed(&g.target, &g.text)
                    .map_err(|err| format!("writing {}: {err}", g.target.display()))?;
                results.push((g.target, written));
            }
        }
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Fields are declared under their Rust names: `rename_all` on a struct and
    /// `#[teal(..)]` on a field are refused rather than quietly ignored.
    #[test]
    fn renaming_record_fields_is_refused_not_ignored() {
        let e = record_decl(&item(
            "#[derive(TealRecord)] #[teal(rename_all = \"snake_case\")] pub struct P { pub x: f64 }",
        ))
        .unwrap_err();
        assert!(e.contains("`rename_all` applies to enum variants"), "{e}");
        let e = record_decl(&item(
            "#[derive(TealRecord)] pub struct P { #[teal(name = \"ex\")] pub x: f64 }",
        ))
        .unwrap_err();
        assert!(e.contains("P.x") && e.contains("not supported"), "{e}");
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
        let hd = host_decl(imp, attrs, Some(&file.items)).unwrap();
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
        host_decl(imp, attrs, Some(&file.items)).unwrap()
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
        let e = match host_decl(imp, attrs, Some(&file.items)) {
            Ok(hd) => panic!("rename accepted: {}", hd.decl),
            Err(e) => e,
        };
        assert!(
            e.contains("`Point` is nested through `records` and cannot be renamed")
                && e.contains("uses = [Pt]"),
            "{e}"
        );
    }
}
