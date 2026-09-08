//! What `#[c_export]` writes: the plan for one `extern "C"` wrapper per method, and the
//! C header that declares them.
//!
//! This is [`dts`](crate::dts) pointed the other way. `dts` takes an `impl` block and
//! says what Teal sees; this takes the same breakdown ([`dts::host_decl`]) and says what
//! C sees. Both run in the proc macro at `cargo build` and in the CLI (`htl dts`), so a
//! rename on the Rust side moves the `.d.tl` and the `.h` together, which is the drift a
//! hand-written FFI layer keeps reintroducing.
//!
//! # The allowed surface, and why it is small
//!
//! A C header can express a great deal that no two host languages agree on. What crosses
//! here is the intersection that every one of them gets right:
//!
//! | C | Rust |
//! | --- | --- |
//! | `const char *` | `&str` / `String`, or any `Serialize` / `Deserialize` type as JSON |
//! | `int` | `i32`, and the [`Status`](crate::ffi::Status) an `int` return carries |
//! | `int *` | the out-parameter an integer result is written through |
//! | `char *` | a string this library allocated, freed with `<prefix>_free` |
//! | `<prefix>_handle *` | the opaque handle |
//!
//! Anything else is refused at compile time, naming the type and this set: `bool` (C#,
//! Swift and Rust do not agree on its width), a bare enum (its underlying type is
//! implementation-defined), a float (the ABI differs by platform in ways a JSON number
//! does not), an integer of another width, a struct by value, and variadics.
//!
//! # The four wrapper shapes
//!
//! Every generated function is one of these, decided by the Rust return type:
//!
//! | Rust return | C |
//! | --- | --- |
//! | `()` | `void <p>_m(<p>_handle *h, ..)` |
//! | `String` | `char *<p>_m(..)` — the text, or `NULL` on failure |
//! | any `Serialize` type | `char *<p>_m(..)` — JSON, or `NULL` on failure |
//! | `Result<(), E>` | `int <p>_m(..)` — a status |
//! | `i32` / `Result<i32, E>` | `int <p>_m(.., int *out)` — a status, value through `out` |
//!
//! A `Result<String, E>` and a `String` are the same shape: the failure of a `char *`
//! function is `NULL`, and *why* is `<p>_last_status()` and `<p>_last_error()`. The
//! integer case is the one a hand-written layer usually gets wrong by returning `1 / 0 /
//! -1` with three meanings; here the value has its own out-parameter and the return is
//! only ever a status.

use crate::dts::{HostDecl, HostMethod, HostParam, is_result};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, ImplItem, ItemImpl, Lit, Meta, ReturnType, Token, Type};

/// The status codes as the header spells them, in code order.
///
/// A copy of [`crate::ffi::Status`], which lives behind the `ffi` feature while this
/// lives behind `dts`: the generator has to name them without the runtime being
/// compiled. `crates/htl/tests/c_export.rs` fails if the two ever disagree.
pub const STATUSES: &[(&str, i32)] = &[
    ("OK", 0),
    ("ERR", 1),
    ("BAD_HANDLE", 2),
    ("NOT_FOUND", 3),
    ("LUA", 4),
    ("PANIC", 5),
    ("WRONG_THREAD", 6),
    ("INTERRUPTED", 7),
];

/// `ABI_VERSION` as the header states it. Mirrors [`crate::ffi::ABI_VERSION`] for the
/// same reason [`STATUSES`] does.
pub const ABI_VERSION: i32 = 1;

/// What the runtime already exports under the prefix, and therefore what a method may
/// not be called.
pub const RESERVED: &[&str] = &[
    "open",
    "close",
    "free",
    "interrupt",
    "version",
    "abi_version",
    "threadsafe",
    "last_error",
    "last_error_into",
    "last_status",
];

/// The message every refusal ends with. One sentence, so that a compile error naming a
/// type also says what would have worked.
pub const ALLOWED: &str = "what can cross is `&str` / `String` (const char *), \
     `i32` (int), and any serde type as JSON (const char *); \
     not bool, a bare enum, a float, an integer of another width, a struct by value or variadics";

/// `#[c_export(prefix = "..", header = "..")]`.
#[derive(Debug, Clone, Default)]
pub struct CAttrs {
    /// Every generated symbol starts with this. Defaults to the type name, lowercased,
    /// the way `#[host_module]` defaults its module name.
    pub prefix: Option<String>,
    /// Where to write the header, relative to `CARGO_MANIFEST_DIR`. With no `header`,
    /// the text is still available as `<Type>::HEADER`; nothing is written.
    pub header: Option<String>,
}

/// Parse the attribute's `key = "value"` pairs.
pub fn parse_c_export_metas(metas: impl IntoIterator<Item = Meta>) -> Result<CAttrs, String> {
    let mut out = CAttrs::default();
    for meta in metas {
        let Meta::NameValue(nv) = meta else {
            return Err("c_export: expected `key = value` pairs".into());
        };
        let key = nv
            .path
            .get_ident()
            .map(|i| i.to_string())
            .unwrap_or_default();
        let value = match &nv.value {
            Expr::Lit(l) => match &l.lit {
                Lit::Str(s) => s.value(),
                _ => return Err(format!("c_export: `{key}` expects a string literal")),
            },
            _ => return Err(format!("c_export: `{key}` expects a string literal")),
        };
        match key.as_str() {
            "prefix" => out.prefix = Some(value),
            "header" => out.header = Some(value),
            other => {
                return Err(format!(
                    "c_export: unknown attribute `{other}` (prefix, header)"
                ));
            }
        }
    }
    Ok(out)
}

/// The `#[c_export(..)]` on an item, for the file scan `htl dts` runs.
pub fn parse_c_export_attr(attrs: &[Attribute]) -> Result<Option<CAttrs>, String> {
    for a in attrs {
        if a.path().is_ident("c_export") {
            let metas: Punctuated<Meta, Token![,]> = match &a.meta {
                Meta::List(_) => a
                    .parse_args_with(Punctuated::parse_terminated)
                    .map_err(|e| format!("c_export: {e}"))?,
                _ => Punctuated::new(),
            };
            return parse_c_export_metas(metas).map(Some);
        }
    }
    Ok(None)
}

/// How one parameter crosses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// `const char *name` — UTF-8 text, borrowed for the call.
    Text,
    /// `const char *name_json` — parsed with serde.
    Json,
    /// `int name`.
    Int,
    /// Not a C parameter at all: the handle's [`Interrupt`](crate::ffi::Interrupt),
    /// handed to the opener so it can install the hook.
    Interrupt,
}

/// One parameter of a generated wrapper.
#[derive(Clone)]
pub struct CParam {
    /// The Rust parameter name.
    pub name: String,
    /// What it is called in C (`_json` appended for a JSON payload).
    pub c_name: String,
    pub kind: ParamKind,
    /// The owned Rust type the wrapper builds (`&str` arrives as `String`).
    pub ty: Type,
    /// The Rust fn takes it by reference.
    pub by_ref: bool,
}

/// What a wrapper returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// `void`. The Rust fn returns nothing and cannot fail.
    Void,
    /// `char *`: the `String` as it is, or `NULL`.
    Text,
    /// `char *`: the value as JSON, or `NULL`.
    Json,
    /// `int`: a status, nothing else to hand back.
    Status,
    /// `int`: a status, with the `i32` written through a trailing `int *out`.
    IntOut,
}

impl Shape {
    /// The C return type.
    pub fn c_return(self) -> &'static str {
        match self {
            Shape::Void => "void",
            Shape::Text | Shape::Json => "char *",
            Shape::Status | Shape::IntOut => "int",
        }
    }
}

/// One generated `extern "C"` function.
#[derive(Clone)]
pub struct CFn {
    /// The `pub fn` it wraps.
    pub rust_name: String,
    /// The exported symbol.
    pub c_name: String,
    pub params: Vec<CParam>,
    pub shape: Shape,
    /// The Rust fn returns a `Result` the wrapper has to unwrap.
    pub is_result: bool,
    /// The receiver is `&mut self`.
    pub takes_mut: bool,
}

/// The whole generated ABI for one `impl` block.
#[derive(Clone)]
pub struct CPlan {
    pub prefix: String,
    /// The Rust type the handle owns.
    pub type_name: String,
    /// `<prefix>_open`: the associated fn returning `Self`.
    pub open: CFn,
    /// One per other `pub fn`.
    pub methods: Vec<CFn>,
    /// Where to write [`CPlan::header`], relative to `CARGO_MANIFEST_DIR`.
    pub header_path: Option<String>,
}

impl CPlan {
    /// The header text, deterministic: the same `impl` block produces the same bytes on
    /// any machine, so it can sit in the repository and be diffed.
    pub fn header(&self) -> String {
        header(self)
    }
}

/// Work out the ABI for a `#[c_export]` `impl` block, or say why it cannot have one.
///
/// `hd` is the same breakdown `#[host_module]` uses; `imp` is consulted for the one
/// thing a `HostDecl` does not carry, the Rust return type, which is what decides the
/// wrapper shape.
pub fn plan(hd: &HostDecl, imp: &ItemImpl, attrs: CAttrs) -> Result<CPlan, String> {
    let type_name = hd.type_name.clone();
    let prefix = attrs.prefix.unwrap_or_else(|| type_name.to_lowercase());
    if prefix.is_empty()
        || !prefix
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        || prefix.starts_with(|c: char| c.is_ascii_digit())
    {
        return Err(format!(
            "c_export: `prefix = {prefix:?}` is not a C identifier prefix \
             (lowercase ASCII letters, digits and underscore, not starting with a digit)"
        ));
    }

    let mut open: Option<CFn> = None;
    let mut methods = Vec::new();
    for m in &hd.methods {
        let sig = signature(imp, &m.name)?;
        check_signature(&type_name, m, sig)?;
        let ret = success_type(sig);
        let is_self = ret.map(|t| is_self_type(t, &type_name)).unwrap_or(false);
        if m.receiver.is_none() {
            if !is_self {
                return Err(format!(
                    "c_export: `{type_name}::{}` has no `self` and does not return `Self`. \
                     Only the opener may be an associated function; \
                     every other entry point takes the handle `<prefix>_open` returned",
                    m.name
                ));
            }
            if open.is_some() {
                return Err(format!(
                    "c_export: `{type_name}` has more than one associated function returning \
                     `Self`; exactly one is the opener that becomes `{prefix}_open`"
                ));
            }
            open = Some(open_fn(&type_name, &prefix, m)?);
            continue;
        }
        if is_self {
            return Err(format!(
                "c_export: `{type_name}::{}` returns `Self` from a method; a handle is made \
                 only by the opener, so that closing it is `{prefix}_close` and nothing else",
                m.name
            ));
        }
        if RESERVED.contains(&m.name.as_str()) {
            return Err(format!(
                "c_export: `{type_name}::{}` would export `{prefix}_{}`, which the runtime \
                 already exports; rename the method",
                m.name, m.name
            ));
        }
        let mut params = Vec::new();
        for p in &m.params {
            let cp = param(&type_name, &m.name, p)?;
            if cp.kind == ParamKind::Interrupt {
                return Err(format!(
                    "c_export: `{type_name}::{}`: an `Interrupt` is handed to the opener, \
                     not to a method",
                    m.name
                ));
            }
            params.push(cp);
        }
        let shape = shape(&type_name, m, ret)?;
        methods.push(CFn {
            rust_name: m.name.clone(),
            c_name: format!("{prefix}_{}", m.name),
            params,
            shape,
            is_result: m.ret_is_result,
            takes_mut: m.receiver == Some(true),
        });
    }

    let open = open.ok_or_else(|| {
        format!(
            "c_export: `{type_name}` has no opener: add a `pub fn` with no `self` returning \
             `Self` or `Result<Self, E>`, taking the options as one `&str` of JSON. \
             It becomes `{prefix}_open(const char *options_json)`"
        )
    })?;

    Ok(CPlan {
        prefix,
        type_name,
        open,
        methods,
        header_path: attrs.header,
    })
}

/// The opener: one options parameter, plus optionally the interrupt flag.
fn open_fn(type_name: &str, prefix: &str, m: &HostMethod) -> Result<CFn, String> {
    let mut params = Vec::new();
    let mut options = 0;
    for p in &m.params {
        let mut cp = param(type_name, &m.name, p)?;
        // Whatever the Rust parameter is called, the header says `options_json`: the
        // convention is one JSON object, and the header is where a caller reads it.
        if matches!(cp.kind, ParamKind::Text | ParamKind::Json) {
            cp.c_name = "options_json".to_string();
        }
        match cp.kind {
            ParamKind::Interrupt => {}
            ParamKind::Text | ParamKind::Json => options += 1,
            ParamKind::Int => {
                return Err(format!(
                    "c_export: `{type_name}::{}`: the opener takes its options as one JSON \
                     string, not as separate arguments — a host that cannot read the \
                     environment or the working directory passes seeds, names and \
                     absolute paths in there",
                    m.name
                ));
            }
        }
        params.push(cp);
    }
    if options != 1 {
        return Err(format!(
            "c_export: `{type_name}::{}` takes {options} options parameters; the opener takes \
             exactly one, a `&str` of JSON (or a serde type parsed from it), which becomes \
             `{prefix}_open(const char *options_json)`",
            m.name
        ));
    }
    Ok(CFn {
        rust_name: m.name.clone(),
        // Whatever it is called in Rust, it crosses as `<prefix>_open`: a caller reading
        // the header should not have to learn a second word for "make me one".
        c_name: format!("{prefix}_open"),
        params,
        shape: Shape::Void, // unused: the opener returns the handle
        is_result: m.ret_is_result,
        takes_mut: false,
    })
}

/// The `syn` signature of a `pub fn` the `HostDecl` listed.
fn signature<'a>(imp: &'a ItemImpl, name: &str) -> Result<&'a syn::Signature, String> {
    imp.items
        .iter()
        .find_map(|it| match it {
            ImplItem::Fn(f) if f.sig.ident == name => Some(&f.sig),
            _ => None,
        })
        .ok_or_else(|| format!("c_export: `{name}` is not in this impl block"))
}

/// Things no wrapper shape can express, whatever the types are.
fn check_signature(type_name: &str, m: &HostMethod, sig: &syn::Signature) -> Result<(), String> {
    if m.is_async {
        return Err(format!(
            "c_export: `{type_name}::{}` is `async`; a C caller has no executor to poll it \
             with. Expose a blocking method, or keep the async one for the Rust host only",
            m.name
        ));
    }
    if !sig.generics.params.is_empty() {
        return Err(format!(
            "c_export: `{type_name}::{}` is generic; there is one symbol per method and a \
             generic has no single one. Take a concrete type: {ALLOWED}",
            m.name
        ));
    }
    if sig.variadic.is_some() {
        return Err(format!(
            "c_export: `{type_name}::{}` is variadic; {ALLOWED}",
            m.name
        ));
    }
    Ok(())
}

/// The success type: `T` of `Result<T, E>`, or the plain return. `None` for `()`.
fn success_type(sig: &syn::Signature) -> Option<&Type> {
    let ReturnType::Type(_, t) = &sig.output else {
        return None;
    };
    let inner = if is_result(t) { first_arg(t)? } else { t };
    if matches!(inner, Type::Tuple(tu) if tu.elems.is_empty()) {
        return None;
    }
    Some(inner)
}

fn first_arg(ty: &Type) -> Option<&Type> {
    let Type::Path(p) = ty else { return None };
    let seg = p.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else {
        return None;
    };
    ab.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// The last segment of a path type, `""` for anything else.
fn ident_of(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => ident_of(&r.elem),
        Type::Paren(p) => ident_of(&p.elem),
        Type::Path(p) => p
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn is_self_type(ty: &Type, type_name: &str) -> bool {
    let id = ident_of(ty);
    id == "Self" || id == type_name
}

/// Rust primitives that have no honest C spelling here, and what to say about each.
fn refuse_primitive(ident: &str) -> Option<&'static str> {
    match ident {
        "bool" => Some(
            "`bool` is one byte in Rust, four in a Win32 `BOOL`, and its own thing in C# and \
             Swift; return an `i32` status or a JSON payload instead",
        ),
        "f32" | "f64" => Some(
            "a float's calling convention differs by platform and its text form differs by \
             locale; put it in a JSON payload instead",
        ),
        "char" => Some("`char` is a Unicode scalar in Rust and a byte in C; use a `String`"),
        "i8" | "i16" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => Some(
            "only `i32` crosses as `int`; a wider or unsigned integer changes width between \
             hosts, so put it in a JSON payload",
        ),
        _ => None,
    }
}

/// How one parameter crosses, or why it cannot.
fn param(type_name: &str, fname: &str, p: &HostParam) -> Result<CParam, String> {
    let refuse = |why: &str| {
        Err(format!(
            "c_export: `{type_name}::{fname}`: parameter `{}` {why}. Here {ALLOWED}",
            p.name
        ))
    };
    let ident = ident_of(&p.owned_ty);
    if !matches!(strip(&p.owned_ty), Type::Path(_)) {
        return refuse("is not a named type");
    }
    if let Some(why) = refuse_primitive(&ident) {
        return refuse(&format!("is `{ident}`, and {why}"));
    }
    let kind = match ident.as_str() {
        "String" | "str" => ParamKind::Text,
        "i32" => ParamKind::Int,
        "Interrupt" => ParamKind::Interrupt,
        // Everything else — a record, a `Vec`, an `Option<T>` (JSON `null`, since C has
        // no "argument left out") — crosses as JSON.
        _ => ParamKind::Json,
    };
    let c_name = if kind == ParamKind::Json && !p.name.ends_with("_json") {
        format!("{}_json", p.name)
    } else {
        p.name.clone()
    };
    Ok(CParam {
        name: p.name.clone(),
        c_name,
        kind,
        ty: p.owned_ty.clone(),
        by_ref: p.by_ref,
    })
}

fn strip(ty: &Type) -> &Type {
    match ty {
        Type::Reference(r) => strip(&r.elem),
        Type::Paren(p) => strip(&p.elem),
        other => other,
    }
}

/// Which of the four shapes a method's return type is.
fn shape(type_name: &str, m: &HostMethod, ret: Option<&Type>) -> Result<Shape, String> {
    let Some(ty) = ret else {
        return Ok(if m.ret_is_result {
            Shape::Status
        } else {
            Shape::Void
        });
    };
    let ident = ident_of(ty);
    let refuse = |why: String| {
        Err(format!(
            "c_export: `{type_name}::{}` returns {why}. Here {ALLOWED}",
            m.name
        ))
    };
    if !matches!(strip(ty), Type::Path(_)) {
        return refuse("a type that is not a named type".to_string());
    }
    if let Some(why) = refuse_primitive(&ident) {
        return refuse(format!("`{ident}`: {why}"));
    }
    Ok(match ident.as_str() {
        "String" | "str" => Shape::Text,
        "i32" => Shape::IntOut,
        _ => Shape::Json,
    })
}

// ---------------------------------------------------------------- header text

/// The C declaration of one generated function, without the trailing semicolon.
fn declare(plan: &CPlan, f: &CFn, is_open: bool) -> String {
    let mut args: Vec<String> = Vec::new();
    if is_open {
        for p in &f.params {
            if p.kind != ParamKind::Interrupt {
                args.push(format!("const char *{}", p.c_name));
            }
        }
        return format!("{p}_handle *{p}_open({})", args.join(", "), p = plan.prefix);
    }
    args.push(format!("{p}_handle *h", p = plan.prefix));
    for p in &f.params {
        args.push(match p.kind {
            ParamKind::Text | ParamKind::Json => format!("const char *{}", p.c_name),
            ParamKind::Int => format!("int {}", p.c_name),
            ParamKind::Interrupt => unreachable!("refused in `plan`"),
        });
    }
    if f.shape == Shape::IntOut {
        args.push("int *out".to_string());
    }
    let ret = f.shape.c_return();
    let sep = if ret.ends_with('*') { "" } else { " " };
    format!("{ret}{sep}{}({})", f.c_name, args.join(", "))
}

/// The header, in full. Deterministic and self-contained: no timestamps, no paths, no
/// includes beyond what the declarations need.
pub fn header(plan: &CPlan) -> String {
    let p = &plan.prefix;
    let up = p.to_uppercase();
    let mut s = String::new();
    s.push_str(&format!(
        "/* {p}.h — generated by htl's #[c_export] from `impl {}`. Do not edit.\n\
         \x20*\n\
         \x20* Ownership: every `char *` this library returns was allocated by it and must be\n\
         \x20* freed with {p}_free(); a `const char *` argument is borrowed for the call and the\n\
         \x20* caller may free it on return. An `int` return is a status ({up}_OK and friends),\n\
         \x20* never a value: a call with both writes the value through an out-parameter.\n\
         \x20*\n\
         \x20* Threads: a handle belongs to the thread that opened it and answers\n\
         \x20* {up}_WRONG_THREAD anywhere else ({p}_threadsafe() reports this). {p}_interrupt()\n\
         \x20* is the exception and may be called from another thread.\n\
         \x20*\n\
         \x20* Errors: a failed call returns NULL or a status; {p}_last_error() is the message\n\
         \x20* on this thread, valid until the next call, and {p}_last_error_into() copies it\n\
         \x20* into a buffer of your own.\n\
         \x20*/\n",
        plan.type_name
    ));
    s.push_str(&format!("#ifndef {up}_H\n#define {up}_H\n\n"));
    s.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");
    s.push_str(&format!(
        "/* The shape of these functions. Compare it with {p}_abi_version() before calling\n\
         \x20  anything else: a plugin host that never unloads a library cannot tell a stale\n\
         \x20  one apart any other way. */\n#define {up}_ABI_VERSION {ABI_VERSION}\n\n"
    ));
    s.push_str("/* What an int return means. */\n");
    for (name, code) in STATUSES {
        s.push_str(&format!("#define {up}_{name} {code}\n"));
    }
    s.push('\n');
    s.push_str(&format!(
        "/* One game, one thread. Opaque: only ever held as a pointer. */\ntypedef struct {p}_handle {p}_handle;\n\n"
    ));
    s.push_str("/* Library, not tied to a handle. */\n");
    for line in [
        format!("int {p}_abi_version(void)"),
        format!("const char *{p}_version(void)"),
        format!("int {p}_threadsafe(void)"),
        format!("const char *{p}_last_error(void)"),
        format!("int {p}_last_error_into(char *buf, int len)"),
        format!("int {p}_last_status(void)"),
        format!("void {p}_free(char *s)"),
    ] {
        s.push_str(&format!("{line};\n"));
    }
    s.push_str(&format!(
        "\n/* Lifecycle. The options are one JSON object: pass absolute paths, seeds and names\n\
         \x20  in it rather than expecting the library to read the environment or the working\n\
         \x20  directory. {p}_close() on a NULL handle is a no-op. */\n"
    ));
    s.push_str(&format!("{};\n", declare(plan, &plan.open, true)));
    s.push_str(&format!("void {p}_close({p}_handle *h);\n"));
    s.push_str(&format!(
        "/* Callable from another thread: stops the script running on this handle. */\n\
         int {p}_interrupt({p}_handle *h);\n"
    ));
    if !plan.methods.is_empty() {
        s.push_str(&format!(
            "\n/* Methods of `{}`. Payloads are JSON with a \"v\" schema version, except where\n\
             \x20  the method's own return type is a string. */\n",
            plan.type_name
        ));
        for m in &plan.methods {
            s.push_str(&format!("{};\n", declare(plan, m, false)));
        }
    }
    s.push_str("\n#ifdef __cplusplus\n}\n#endif\n");
    s.push_str(&format!("#endif /* {up}_H */\n"));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dts::{TealAttrs, host_decl};

    /// Build the plan for one `impl` block written as source.
    fn planned(src: &str) -> Result<CPlan, String> {
        let imp: ItemImpl = syn::parse_str(src).expect("parses as an impl block");
        let hd = host_decl(&imp, TealAttrs::default(), None)?;
        plan(&hd, &imp, CAttrs::default())
    }

    /// The message a refused `impl` block produces. `CPlan` holds `syn::Type`s and so is
    /// not `Debug`; this drops the success value so `unwrap_err` has nothing to print.
    fn refused(src: &str) -> String {
        planned(src).map(|_| ()).expect_err("must be refused")
    }

    const GAME: &str = r#"
        impl Game {
            pub fn open(options: &str) -> Result<Self, String> { todo!() }
            pub fn frame(&self) -> String { todo!() }
            pub fn state(&self) -> Frame { todo!() }
            pub fn key(&mut self, k: &str) -> Result<(), String> { todo!() }
            pub fn depth(&self) -> i32 { todo!() }
            pub fn save(&self) -> Result<i32, String> { todo!() }
            pub fn tick(&mut self) {}
            fn private(&self) -> bool { true }
        }
    "#;

    #[test]
    fn each_return_type_picks_its_shape() {
        let p = planned(GAME).expect("plans");
        let by = |n: &str| p.methods.iter().find(|m| m.rust_name == n).unwrap().shape;
        assert_eq!(by("frame"), Shape::Text, "a String is the text itself");
        assert_eq!(by("state"), Shape::Json, "anything else is JSON");
        assert_eq!(by("key"), Shape::Status, "Result<(), E> is a status");
        assert_eq!(by("depth"), Shape::IntOut, "an i32 goes through out");
        assert_eq!(by("save"), Shape::IntOut);
        assert_eq!(by("tick"), Shape::Void);
        assert_eq!(p.open.c_name, "game_open");
        assert!(
            !p.methods.iter().any(|m| m.rust_name == "private"),
            "a private fn is not exported"
        );
    }

    #[test]
    fn the_header_declares_every_wrapper() {
        let h = planned(GAME).expect("plans").header();
        for want in [
            "game_handle *game_open(const char *options_json);",
            "void game_close(game_handle *h);",
            "int game_interrupt(game_handle *h);",
            "char *game_frame(game_handle *h);",
            "char *game_state(game_handle *h);",
            "int game_key(game_handle *h, const char *k);",
            "int game_depth(game_handle *h, int *out);",
            "void game_tick(game_handle *h);",
            "void game_free(char *s);",
            "#define GAME_WRONG_THREAD 6",
            "#define GAME_ABI_VERSION 1",
        ] {
            assert!(h.contains(want), "header is missing `{want}`:\n{h}");
        }
    }

    #[test]
    fn the_header_is_the_same_bytes_every_time() {
        assert_eq!(
            planned(GAME).unwrap().header(),
            planned(GAME).unwrap().header()
        );
    }

    /// The refusals. Each names the offending type and ends with the allowed set, so a
    /// compile error says what would have worked instead of only what did not.
    #[test]
    fn a_type_that_cannot_cross_is_refused_by_name() {
        for (src, needle) in [
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn f(&self, b: bool) {} }",
                "`bool`",
            ),
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn f(&self) -> bool { true } }",
                "`bool`",
            ),
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn f(&self) -> f64 { 0.0 } }",
                "`f64`",
            ),
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn f(&self, n: u64) {} }",
                "`u64`",
            ),
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn f(&self, n: i64) {} }",
                "`i64`",
            ),
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn f<T>(&self, n: T) {} }",
                "is generic",
            ),
            (
                "impl G { pub fn open(o: &str) -> Self { todo!() } pub async fn f(&self) {} }",
                "is `async`",
            ),
        ] {
            let err = refused(src);
            assert!(err.contains(needle), "expected {needle} in: {err}");
            assert!(
                err.contains("what can cross is") || err.contains("Expose a blocking method"),
                "a refusal says what would have worked: {err}"
            );
        }
    }

    #[test]
    fn an_impl_without_an_opener_is_refused() {
        let err = refused("impl G { pub fn f(&self) -> String { todo!() } }");
        assert!(err.contains("has no opener"), "{err}");
    }

    #[test]
    fn the_opener_takes_exactly_one_options_argument() {
        let err = refused("impl G { pub fn open(a: &str, b: &str) -> Self { todo!() } }");
        assert!(err.contains("options parameters"), "{err}");
        let err = refused("impl G { pub fn open() -> Self { todo!() } }");
        assert!(err.contains("options parameters"), "{err}");
    }

    #[test]
    fn an_associated_fn_that_is_not_the_opener_is_refused() {
        let err = refused(
            "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn helper() -> String { todo!() } }",
        );
        assert!(err.contains("Only the opener"), "{err}");
    }

    #[test]
    fn a_method_may_not_take_a_name_the_runtime_exports() {
        let err = refused(
            "impl G { pub fn open(o: &str) -> Self { todo!() } pub fn free(&self) -> String { todo!() } }",
        );
        assert!(err.contains("already exports"), "{err}");
    }

    #[test]
    fn the_opener_may_take_the_interrupt_flag() {
        let p =
            planned("impl G { pub fn open(o: &str, i: htl::ffi::Interrupt) -> Self { todo!() } }")
                .expect("plans");
        assert_eq!(p.open.params.len(), 2);
        assert_eq!(p.open.params[1].kind, ParamKind::Interrupt);
        assert!(
            p.header()
                .contains("g_handle *g_open(const char *options_json);"),
            "the flag is not a C argument:\n{}",
            p.header()
        );
    }
}
