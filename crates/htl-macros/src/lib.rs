//! htl proc macros.
//!
//! - `include_tl!("path.tl")`        -> `&'static str` generated Lua, Teal-checked at build time
//! - `include_tl_bytes!("path.tl")`  -> `&'static [u8]` stripped Lua 5.4 bytecode, same check
//! - `#[derive(TealRecord)]`         -> Teal `record` decl + `IntoLua` / `FromLua` (plain table)
//! - `#[host_module(name = "...")]`  -> `UserData` impl + Teal `.d.tl` from a plain `impl` block
//!
//! Paths are relative to `CARGO_MANIFEST_DIR`. Teal type errors (and htl lints, unless
//! `HTL_LINT=warn`) become `compile_error!`s. Every `.tl` consulted is registered with
//! `include_str!` so edits trigger a rebuild.

mod ty;

use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::{format_ident, quote};
use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::{
    Data, DeriveInput, Expr, FnArg, ImplItem, ItemImpl, Lit, LitStr, Meta, Pat, ReturnType, Token,
    parse_macro_input,
};

// ------------------------------------------------------------------ include_tl!

#[proc_macro]
pub fn include_tl(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    match expand_include(&lit.value(), false) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

#[proc_macro]
pub fn include_tl_bytes(input: TokenStream) -> TokenStream {
    let lit = parse_macro_input!(input as LitStr);
    match expand_include(&lit.value(), true) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

fn manifest_dir() -> Result<PathBuf, String> {
    std::env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map_err(|_| "CARGO_MANIFEST_DIR is not set".to_string())
}

/// What `include_tl!` / `include_tl_bytes!` embed, computed without proc-macro types so
/// it can be unit-tested.
#[derive(Debug)]
struct Included {
    main_abs: String,
    deps: Vec<String>,
    payload: Payload,
}

#[derive(Debug)]
enum Payload {
    Source(String),
    Bytes(Vec<u8>),
}

/// Check + generate `rel` (relative to `manifest_dir`). Search paths match the CLI:
/// the file's own directory, the nearest `mlua-pkg.toml` project's vendored deps
/// (and `target_dir` copies), and the bundled `htl.test` declarations.
fn resolve_include(manifest_dir: &Path, rel: &str, bytes: bool) -> Result<Included, String> {
    let path = manifest_dir.join(rel);
    if !path.is_file() {
        return Err(format!("include_tl!: no such file: {}", path.display()));
    }

    let h = htl_core::Htl::new().map_err(|e| format!("include_tl!: {e:#}"))?;
    if let Ok(spec) = std::env::var("HTL_LINTS") {
        h.configure_lints(&spec).map_err(|e| format!("include_tl!: HTL_LINTS: {e:#}"))?;
    }
    h.add_path(&htl_core::parent_dir(&path))
        .map_err(|e| format!("include_tl!: {e:#}"))?;
    if let Some(p) = htl_core::pkg::Project::find(&path) {
        h.apply_project(&p).map_err(|e| format!("include_tl!: {e:#}"))?;
    }
    h.install_test_lib().map_err(|e| format!("include_tl!: {e:#}"))?;
    let (code, ci) = h.gen_lua(&path).map_err(|e| format!("include_tl!: {e:#}"))?;

    for w in &ci.warnings {
        eprintln!("include_tl! warning: {w}");
    }
    let Some(code) = code else {
        return Err(format!("Teal type check failed:\n{}", ci.errors.join("\n")));
    };
    if !ci.lints.is_empty() {
        let lenient = std::env::var("HTL_LINT").map(|v| v == "warn").unwrap_or(false);
        if lenient {
            for l in &ci.lints {
                eprintln!("include_tl! lint: {l}");
            }
        } else {
            return Err(format!(
                "htl lint failed (set HTL_LINT=warn to downgrade):\n{}",
                ci.lints.join("\n")
            ));
        }
    }

    let main_abs = path.to_string_lossy().into_owned();
    let deps: Vec<String> = ci
        .deps
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();

    let payload = if bytes {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("chunk")
            .to_string();
        let bc = h
            .compile(&name, &code)
            .map_err(|e| format!("include_tl_bytes!: {e:#}"))?;
        Payload::Bytes(bc)
    } else {
        Payload::Source(code)
    };

    Ok(Included { main_abs, deps, payload })
}

fn expand_include(rel: &str, bytes: bool) -> Result<TokenStream, String> {
    let inc = resolve_include(&manifest_dir()?, rel, bytes)?;
    let main_abs = inc.main_abs;
    let deps = inc.deps;
    let payload = match inc.payload {
        Payload::Bytes(bc) => {
            let lit = Literal::byte_string(&bc);
            quote! { #lit as &[u8] }
        }
        Payload::Source(code) => quote! { #code },
    };

    Ok(quote! {{
        const _: &str = include_str!(#main_abs);
        #( const _: &str = include_str!(#deps); )*
        #payload
    }}
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "htl-macros-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// The macro must see the same tree as the CLI: a module vendored by mlua-pkg
    /// (`.mlua-pkgs/vendored/<name>/init.tl`) resolves from a script under the project.
    #[test]
    fn include_resolves_vendored_dep_from_mlua_pkg_project() {
        let root = scratch("vendored");
        write(&root.join("mlua-pkg.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[deps]\n");
        write(
            &root.join(".mlua-pkgs/vendored/mathx/init.tl"),
            "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
        );
        write(
            &root.join("scripts/main.tl"),
            "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
        );

        let inc = resolve_include(&root, "scripts/main.tl", false).expect("vendored dep must resolve");
        assert!(inc.main_abs.ends_with("scripts/main.tl"));
        assert!(
            inc.deps.iter().any(|d| d.ends_with("vendored/mathx/init.tl")),
            "dep must be tracked for rebuilds: {:?}",
            inc.deps
        );
        match inc.payload {
            Payload::Source(code) => assert!(code.contains("require(\"mathx\")")),
            Payload::Bytes(_) => panic!("expected source"),
        }
    }

    /// `target_dir` deps (physically vendored under the manifest) resolve too.
    #[test]
    fn include_resolves_target_dir_dep() {
        let root = scratch("targetdir");
        write(
            &root.join("mlua-pkg.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[deps]\nmathx = { git = \"https://example.invalid/mathx\", tag = \"v1\", target_dir = \"lua/mathx\" }\n",
        );
        write(
            &root.join("lua/mathx/init.tl"),
            "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
        );
        write(&root.join("src/main.tl"), "local mathx = require(\"mathx\")\nprint(mathx.twice(1))\n");

        let inc = resolve_include(&root, "src/main.tl", true).expect("target_dir dep must resolve");
        assert!(matches!(inc.payload, Payload::Bytes(ref b) if !b.is_empty()));
    }

    /// Without a project the same script fails: the dep is genuinely not on the path.
    #[test]
    fn include_without_project_does_not_see_vendored_dir() {
        let root = scratch("noproject");
        write(&root.join(".mlua-pkgs/vendored/mathx/init.tl"), "return {}\n");
        write(&root.join("scripts/main.tl"), "local mathx = require(\"mathx\")\nprint(mathx)\n");
        let err = resolve_include(&root, "scripts/main.tl", false).unwrap_err();
        assert!(err.contains("module not found: 'mathx'"), "{err}");
    }
}

// ------------------------------------------------------------------ shared attr parsing

#[derive(Default)]
struct TealAttrs {
    name: Option<String>,
    dts: Option<String>,
    /// Record types declared in their own `.d.tl` module: emits `local X = require("X")`.
    uses: Vec<String>,
    /// Record types (structs in the same source file) nested inside the module record.
    records: Vec<String>,
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

fn parse_attr_metas(metas: impl IntoIterator<Item = Meta>) -> Result<TealAttrs, String> {
    let mut out = TealAttrs::default();
    for meta in metas {
        let Meta::NameValue(nv) = meta else {
            return Err("expected `key = value` pairs".into());
        };
        let key = nv.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
        match (key.as_str(), &nv.value) {
            ("name", Expr::Lit(l)) => out.name = Some(lit_str(&l.lit)?),
            ("dts", Expr::Lit(l)) => out.dts = Some(lit_str(&l.lit)?),
            ("uses", Expr::Array(arr)) => out.uses = type_list(arr, "uses")?,
            ("records", Expr::Array(arr)) => out.records = type_list(arr, "records")?,
            (k, _) => return Err(format!("unknown or malformed attribute `{k}`")),
        }
    }
    Ok(out)
}

fn lit_str(l: &Lit) -> Result<String, String> {
    match l {
        Lit::Str(s) => Ok(s.value()),
        _ => Err("expected a string literal".into()),
    }
}

/// `#[teal(...)]` on a derive input.
fn parse_teal_attrs(attrs: &[syn::Attribute]) -> Result<TealAttrs, String> {
    let mut metas = Vec::new();
    for a in attrs {
        if a.path().is_ident("teal") {
            let list = a
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .map_err(|e| e.to_string())?;
            metas.extend(list);
        }
    }
    parse_attr_metas(metas)
}

fn write_dts(rel: &str, text: &str) -> Result<(), String> {
    let path = manifest_dir()?.join(rel);
    htl_core::write_if_changed(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(())
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

/// `(field name, Teal type)` for each named field.
fn record_fields(fields: &syn::FieldsNamed, self_name: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for f in &fields.named {
        let fi = f.ident.as_ref().unwrap().to_string();
        let tt = ty::teal_type(&f.ty, self_name)?;
        out.push((fi, tt));
    }
    Ok(out)
}

/// Nested `record X ... end` blocks (indented one level) for structs found in the
/// current source file. Uses `Span::local_file()`, so it only sees this file.
fn nested_record_decls(names: &[String]) -> Result<Vec<String>, String> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let file = proc_macro::Span::call_site().local_file().ok_or_else(|| {
        "host_module: `records` needs the source file path (unavailable in this expansion); \
         declare the record in its own module and use `uses` instead"
            .to_string()
    })?;
    let src = std::fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
    let ast = syn::parse_file(&src).map_err(|e| format!("parsing {}: {e}", file.display()))?;
    let mut out = Vec::new();
    for name in names {
        let st = find_struct(&ast.items, name).ok_or_else(|| {
            format!(
                "host_module: struct `{name}` not found in {} (records must live in the same file; \
                 use `uses` for records from other modules)",
                file.display()
            )
        })?;
        let syn::Fields::Named(fields) = &st.fields else {
            return Err(format!("host_module: `{name}` must be a struct with named fields"));
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

fn find_struct<'a>(items: &'a [syn::Item], name: &str) -> Option<&'a syn::ItemStruct> {
    for it in items {
        match it {
            syn::Item::Struct(s) if s.ident == name => return Some(s),
            syn::Item::Mod(m) => {
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

// ------------------------------------------------------------------ #[derive(TealRecord)]

#[proc_macro_derive(TealRecord, attributes(teal))]
pub fn derive_teal_record(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_record(&input) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

fn expand_record(input: &DeriveInput) -> Result<TokenStream, String> {
    let attrs = parse_teal_attrs(&input.attrs)?;
    let ident = &input.ident;
    let name = attrs.name.clone().unwrap_or_else(|| ident.to_string());

    let Data::Struct(ds) = &input.data else {
        return Err("TealRecord: only structs with named fields are supported".into());
    };
    let syn::Fields::Named(fields) = &ds.fields else {
        return Err("TealRecord: only structs with named fields are supported".into());
    };

    let mut decl = uses_header(&attrs.uses);
    decl.push_str(&format!("local record {name}\n"));
    let mut field_idents = Vec::new();
    let mut field_names = Vec::new();
    for (fname, tt) in record_fields(fields, &name)? {
        decl.push_str(&format!("   {fname}: {tt}\n"));
        field_idents.push(format_ident!("{}", fname));
        field_names.push(fname);
    }
    decl.push_str(&format!("end\n\nreturn {name}\n"));

    if let Some(dts) = &attrs.dts {
        write_dts(dts, &decl)?;
    }

    Ok(quote! {
        impl ::htl::teal::TealRecord for #ident {
            const NAME: &'static str = #name;
            const DECL: &'static str = #decl;
        }
        impl ::htl::mlua::IntoLua for #ident {
            fn into_lua(self, lua: &::htl::mlua::Lua) -> ::htl::mlua::Result<::htl::mlua::Value> {
                let t = lua.create_table()?;
                #( t.set(#field_names, self.#field_idents)?; )*
                Ok(::htl::mlua::Value::Table(t))
            }
        }
        impl ::htl::mlua::FromLua for #ident {
            fn from_lua(value: ::htl::mlua::Value, lua: &::htl::mlua::Lua) -> ::htl::mlua::Result<Self> {
                let t = <::htl::mlua::Table as ::htl::mlua::FromLua>::from_lua(value, lua)?;
                Ok(Self { #( #field_idents: t.get(#field_names)?, )* })
            }
        }
        impl #ident {
            /// Make `require("NAME")` resolve at runtime (type-only module -> empty table).
            pub fn htl_preload(h: &::htl::Htl) -> ::htl::mlua::Result<()> {
                let t = h.lua().create_table()?;
                h.preload_value(#name, t).map_err(::htl::mlua::Error::external)
            }
        }
    }
    .into())
}

// ------------------------------------------------------------------ #[host_module]

#[proc_macro_attribute]
pub fn host_module(attr: TokenStream, item: TokenStream) -> TokenStream {
    let metas = match Punctuated::<Meta, Token![,]>::parse_terminated.parse(attr) {
        Ok(m) => m,
        Err(e) => {
            let msg = e.to_string();
            return quote! { compile_error!(#msg) }.into();
        }
    };
    let imp = parse_macro_input!(item as ItemImpl);
    match expand_host_module(metas, &imp) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

use syn::parse::Parser;

fn expand_host_module(metas: Punctuated<Meta, Token![,]>, imp: &ItemImpl) -> Result<TokenStream, String> {
    let attrs = parse_attr_metas(metas)?;
    let self_ty = &imp.self_ty;
    let type_name = match &**self_ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default(),
        _ => return Err("host_module: impl target must be a plain type".into()),
    };
    let module = attrs.name.clone().unwrap_or_else(|| type_name.to_lowercase());

    let mut decl = uses_header(&attrs.uses);
    decl.push_str(&format!("local record {module}\n"));
    for r in nested_record_decls(&attrs.records)? {
        decl.push_str(&r);
    }
    let mut registrations = Vec::new();

    for it in &imp.items {
        let ImplItem::Fn(f) = it else { continue };
        if !matches!(f.vis, syn::Visibility::Public(_)) {
            continue;
        }
        let fname = &f.sig.ident;
        let fname_s = fname.to_string();

        let mut receiver: Option<bool> = None; // Some(is_mut)
        let mut arg_pats = Vec::new();
        let mut arg_tys = Vec::new();
        let mut call_exprs = Vec::new();
        let mut teal_params = Vec::new();
        for a in &f.sig.inputs {
            match a {
                FnArg::Receiver(r) => receiver = Some(r.mutability.is_some()),
                FnArg::Typed(pt) => {
                    // `&str` -> String, `&[T]` -> Vec<T>, `&T` -> T on the Lua side; the
                    // wrapper passes `&value` back to the Rust fn. `&mut` is not supported.
                    let (owned_ty, by_ref): (syn::Type, bool) = match &*pt.ty {
                        syn::Type::Reference(r) => {
                            if r.mutability.is_some() {
                                return Err(format!(
                                    "host_module: `{fname_s}`: `&mut` parameters are not supported"
                                ));
                            }
                            let owned: syn::Type = match &*r.elem {
                                syn::Type::Path(p) if p.path.is_ident("str") => {
                                    syn::parse_quote!(::std::string::String)
                                }
                                syn::Type::Slice(s) => {
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
                        _ => format!("a{}", arg_pats.len()),
                    };
                    let tt = ty::teal_type(&owned_ty, &module)?;
                    teal_params.push(format!("{pname}: {tt}"));
                    let id = format_ident!("{}", pname);
                    call_exprs.push(if by_ref { quote! { &#id } } else { quote! { #id } });
                    arg_pats.push(id);
                    arg_tys.push(owned_ty);
                }
            }
        }
        if receiver.is_some() {
            teal_params.insert(0, format!("self: {module}"));
        }

        let (ret_teal, is_result) = match &f.sig.output {
            ReturnType::Default => (String::new(), false),
            ReturnType::Type(_, t) => (ty::teal_type(t.as_ref(), &module)?, ty::is_result(t.as_ref())),
        };
        let ret_suffix = if ret_teal.is_empty() { String::new() } else { format!(": {ret_teal}") };
        decl.push_str(&format!("   {fname_s}: function({}){ret_suffix}\n", teal_params.join(", ")));

        let call_args = quote! { #( #call_exprs ),* };
        let body = match (receiver, is_result) {
            (Some(_), false) => quote! { ::htl::mlua::Result::Ok(this.#fname(#call_args)) },
            (Some(_), true) => quote! { this.#fname(#call_args).map_err(::htl::mlua::Error::external) },
            (None, false) => quote! { ::htl::mlua::Result::Ok(<#self_ty>::#fname(#call_args)) },
            (None, true) => quote! { <#self_ty>::#fname(#call_args).map_err(::htl::mlua::Error::external) },
        };
        let pat = quote! { (#( #arg_pats, )*): (#( #arg_tys, )*) };
        registrations.push(match receiver {
            Some(false) => quote! { m.add_method(#fname_s, |_lua, this, #pat| #body); },
            Some(true) => quote! { m.add_method_mut(#fname_s, |_lua, this, #pat| #body); },
            None => quote! { m.add_function(#fname_s, |_lua, #pat| #body); },
        });
    }
    decl.push_str(&format!("end\n\nreturn {module}\n"));

    if let Some(dts) = &attrs.dts {
        write_dts(dts, &decl)?;
    }

    Ok(quote! {
        #imp
        impl ::htl::mlua::UserData for #self_ty {
            fn add_methods<M: ::htl::mlua::UserDataMethods<Self>>(m: &mut M) {
                #( #registrations )*
            }
        }
        impl ::htl::teal::HostModule for #self_ty {
            const MODULE: &'static str = #module;
            const DECL: &'static str = #decl;
        }
        impl #self_ty {
            /// Register this instance as the `require("MODULE")` value.
            pub fn htl_preload(self, h: &::htl::Htl) -> ::htl::mlua::Result<()> {
                h.preload_value(#module, self).map_err(::htl::mlua::Error::external)
            }
        }
    }
    .into())
}
