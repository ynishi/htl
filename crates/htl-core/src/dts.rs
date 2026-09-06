//! Teal declaration (`.d.tl`) generation from Rust source.
//!
//! Shared by the proc macros (`#[host_module]` / `#[derive(TealRecord)]` at expansion
//! time) and by `htl dts`, which scans `.rs` files and writes the same declarations
//! *before* any `cargo build`, so `htl check` works on a fresh checkout.
//!
//! Type mapping is syntactic: `f64 -> number`, integers -> `integer`, `String`/`&str`
//! -> `string`, `bool -> boolean`, `Vec<T> -> {T}`, `HashMap<K, V> -> {K:V}`,
//! `Option<T> -> T`, `Result<T, _> -> T`, other identifiers pass through as record names.

use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, FnArg, GenericArgument, ImplItem, Item, ItemImpl, ItemStruct, Lit, Meta, Pat,
    PathArguments, ReturnType, Token, Type,
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

/// `#[teal(...)]` / `#[host_module(...)]` arguments.
#[derive(Debug, Clone, Default)]
pub struct TealAttrs {
    pub name: Option<String>,
    /// `.d.tl` output path, relative to the crate's `CARGO_MANIFEST_DIR`.
    pub dts: Option<String>,
    /// Record types declared in their own `.d.tl` module: emits `local X = require("X")`.
    pub uses: Vec<String>,
    /// Record types (structs in the same source file) nested inside the module record.
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

/// Parse `name = "..", dts = "..", uses = [A, B], records = [C]`.
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

/// `#[teal(...)]` on a struct (absent -> defaults).
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

fn uses_header(uses: &[String]) -> String {
    let mut s = String::new();
    for u in uses {
        s.push_str(&format!("local {u} = require(\"{u}\")\n"));
    }
    if !uses.is_empty() {
        s.push('\n');
    }
    s
}

// ---------------------------------------------------------------- records

#[derive(Debug, Clone)]
pub struct RecordDecl {
    pub name: String,
    /// `(field, teal type)` in declaration order.
    pub fields: Vec<(String, String)>,
    /// Full module text: `local record NAME ... end  return NAME`.
    pub decl: String,
    pub attrs: TealAttrs,
}

fn record_fields(
    fields: &syn::FieldsNamed,
    self_name: &str,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for f in &fields.named {
        let fi = f.ident.as_ref().unwrap().to_string();
        let tt = teal_type(&f.ty, self_name)?;
        out.push((fi, tt));
    }
    Ok(out)
}

/// Declaration for a `#[derive(TealRecord)]` struct.
pub fn record_decl(item: &ItemStruct) -> Result<RecordDecl, String> {
    let attrs = parse_teal_attrs(&item.attrs)?;
    let name = attrs.name.clone().unwrap_or_else(|| item.ident.to_string());
    let syn::Fields::Named(fields) = &item.fields else {
        return Err("TealRecord: only structs with named fields are supported".into());
    };
    let fields = record_fields(fields, &name)?;
    let mut decl = uses_header(&attrs.uses);
    decl.push_str(&format!("local record {name}\n"));
    for (f, t) in &fields {
        decl.push_str(&format!("   {f}: {t}\n"));
    }
    decl.push_str(&format!("end\n\nreturn {name}\n"));
    Ok(RecordDecl {
        name,
        fields,
        decl,
        attrs,
    })
}

/// Find a struct by name in a file's items (recursing into inline modules).
pub fn find_struct<'a>(items: &'a [Item], name: &str) -> Option<&'a ItemStruct> {
    for it in items {
        match it {
            Item::Struct(s) if s.ident == name => return Some(s),
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content
                    && let Some(s) = find_struct(inner, name)
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
        let st = find_struct(items, name).ok_or_else(|| {
            format!(
                "host_module: struct `{name}` not found in this file (records must live in the same file; \
                 use `uses` for records from other modules)"
            )
        })?;
        let syn::Fields::Named(fields) = &st.fields else {
            return Err(format!(
                "host_module: `{name}` must be a struct with named fields"
            ));
        };
        let mut s = format!("   record {name}\n");
        for (f, t) in record_fields(fields, name)? {
            s.push_str(&format!("      {f}: {t}\n"));
        }
        s.push_str("   end\n");
        out.push(s);
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
                    teal_params.push(format!("{pname}: {teal}"));
                    params.push(HostParam {
                        name: pname,
                        owned_ty,
                        by_ref,
                        teal,
                    });
                }
            }
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
    /// `host_module <module>` / `record <Name>` for reporting.
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
            Item::Struct(st) if derives_teal_record(&st.attrs) => {
                let rd = record_decl(st)?;
                if let Some(dts) = &rd.attrs.dts {
                    out.push(Generated {
                        target: manifest_dir.join(dts),
                        text: rd.decl.clone(),
                        source: path.to_path_buf(),
                        what: format!("record {}", rd.name),
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
