//! htl proc macros.
//!
//! - `include_tl!("path.tl")`        -> `&'static str` generated Lua, Teal-checked at build time
//! - `include_tl_bytes!("path.tl")`  -> `&'static [u8]` stripped Lua 5.4 bytecode, same check
//! - `#[derive(TealRecord)]`         -> Teal `record` decl + `IntoLua` / `FromLua` (plain table)
//! - `#[host_module(name = "...")]`  -> `UserData` impl + Teal `.d.tl` from a plain `impl` block
//!
//! Paths are relative to `CARGO_MANIFEST_DIR`. Teal type errors (and htl lints, unless
//! `HTL_LINT=warn`) become `compile_error!`s. Every `.tl` consulted is registered with
//! `include_str!` so edits trigger a rebuild. Generated code refers to `::htl::...`, so
//! use these through the `htl` umbrella crate.
//!
//! Declaration text (`.d.tl`) comes from `htl_core::dts`, the same code `htl dts` runs
//! from the CLI, so the files can also be produced before any `cargo build`.

mod ty;

use htl_core::dts;
use proc_macro::TokenStream;
use proc_macro2::Literal;
use quote::{format_ident, quote};
use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::{ItemImpl, ItemStruct, LitStr, Meta, Token, parse::Parser, parse_macro_input};

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
    // Only explicit directories: the process cwd (cargo's) must not leak into module
    // resolution, or a same-named file there gets picked up and tracked by a path that
    // is relative to nothing in particular.
    h.reset_search_path().map_err(|e| format!("include_tl!: {e:#}"))?;
    h.add_path(&htl_core::parent_dir(&path))
        .map_err(|e| format!("include_tl!: {e:#}"))?;
    // The crate's `src/` (where scaffolded `.tl` modules and `.d.tl` live), so scripts kept
    // elsewhere (e.g. `scripts/`) still resolve them.
    let crate_src = manifest_dir.join("src");
    if crate_src.is_dir() {
        h.add_path(&crate_src).map_err(|e| format!("include_tl!: {e:#}"))?;
    }
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
    // `include_str!` resolves relative to the *Rust* source file, so every tracked dep
    // must be absolute. Drop anything that does not exist as a file (nothing to track).
    let deps: Vec<String> = ci
        .deps
        .iter()
        .filter_map(|d| {
            let p = if d.is_absolute() { d.clone() } else { manifest_dir.join(d) };
            let p = std::fs::canonicalize(&p).unwrap_or(p);
            p.is_file().then(|| p.to_string_lossy().into_owned())
        })
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

// ------------------------------------------------------------------ shared

fn write_dts(rel: &str, text: &str) -> Result<(), String> {
    let path = manifest_dir()?.join(rel);
    htl_core::write_if_changed(&path, text).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(())
}

// ------------------------------------------------------------------ #[derive(TealRecord)]

#[proc_macro_derive(TealRecord, attributes(teal))]
pub fn derive_teal_record(input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemStruct);
    match expand_record(&item) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

fn expand_record(item: &ItemStruct) -> Result<TokenStream, String> {
    let rd = dts::record_decl(item)?;
    if let Some(d) = &rd.attrs.dts {
        write_dts(d, &rd.decl)?;
    }
    let ident = &item.ident;
    let name = &rd.name;
    let decl = &rd.decl;
    let field_idents: Vec<_> = rd.fields.iter().map(|(f, _)| format_ident!("{}", f)).collect();
    let field_names: Vec<&str> = rd.fields.iter().map(|(f, _)| f.as_str()).collect();

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

/// Items of the file this macro is expanding in (needed for `records = [...]`).
fn current_file_items() -> Option<Vec<syn::Item>> {
    let file = proc_macro::Span::call_site().local_file()?;
    let src = std::fs::read_to_string(&file).ok()?;
    syn::parse_file(&src).ok().map(|f| f.items)
}

fn expand_host_module(metas: Punctuated<Meta, Token![,]>, imp: &ItemImpl) -> Result<TokenStream, String> {
    let attrs = dts::parse_attr_metas(metas)?;
    let file_items = if attrs.records.is_empty() { None } else { current_file_items() };
    let hd = dts::host_decl(imp, attrs, file_items.as_deref())?;
    if let Some(d) = &hd.attrs.dts {
        write_dts(d, &hd.decl)?;
    }

    let self_ty = &imp.self_ty;
    let module = &hd.module;
    let decl = &hd.decl;
    let mut registrations = Vec::new();
    for m in &hd.methods {
        let fname = format_ident!("{}", m.name);
        let fname_s = &m.name;
        let arg_pats: Vec<_> = m.params.iter().map(|p| format_ident!("{}", p.name)).collect();
        let arg_tys: Vec<&syn::Type> = m.params.iter().map(|p| &p.owned_ty).collect();
        let call_exprs: Vec<_> = m
            .params
            .iter()
            .map(|p| {
                let id = format_ident!("{}", p.name);
                if p.by_ref { quote! { &#id } } else { quote! { #id } }
            })
            .collect();
        let call_args = quote! { #( #call_exprs ),* };
        let body = match (m.receiver, m.ret_is_result) {
            (Some(_), false) => quote! { ::htl::mlua::Result::Ok(this.#fname(#call_args)) },
            (Some(_), true) => quote! { this.#fname(#call_args).map_err(::htl::mlua::Error::external) },
            (None, false) => quote! { ::htl::mlua::Result::Ok(<#self_ty>::#fname(#call_args)) },
            (None, true) => quote! { <#self_ty>::#fname(#call_args).map_err(::htl::mlua::Error::external) },
        };
        let pat = quote! { (#( #arg_pats, )*): (#( #arg_tys, )*) };
        registrations.push(match m.receiver {
            Some(false) => quote! { m.add_method(#fname_s, |_lua, this, #pat| #body); },
            Some(true) => quote! { m.add_method_mut(#fname_s, |_lua, this, #pat| #body); },
            None => quote! { m.add_function(#fname_s, |_lua, #pat| #body); },
        });
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
        assert!(
            inc.deps.iter().all(|d| Path::new(d).is_absolute()),
            "tracked deps must be absolute for include_str!: {:?}",
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

    /// A flat package exposes its top-level module as `<name>/<name>.tl`.
    #[test]
    fn include_resolves_flat_package_module() {
        let root = scratch("flat");
        write(&root.join("mlua-pkg.toml"), "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[deps]\n");
        write(
            &root.join(".mlua-pkgs/vendored/mathx/mathx.tl"),
            "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
        );
        write(&root.join("src/main.tl"), "local mathx = require(\"mathx\")\nprint(mathx.twice(1))\n");
        resolve_include(&root, "src/main.tl", false).expect("flat package must resolve");
    }

    /// The process cwd (cargo's, during a build) must not take part in resolution: a
    /// same-named module sitting there is neither picked up nor tracked by a relative path.
    #[test]
    fn include_ignores_modules_in_the_process_cwd() {
        let decoy = scratch("cwd-decoy");
        write(&decoy.join("Tasks.tl"), "local record Tasks\nend\nreturn Tasks\n");
        let root = scratch("cwd-crate");
        write(&root.join("src/main.tl"), "local ok, t = pcall(require, \"Tasks\")\nprint(ok, t)\n");

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&decoy).unwrap();
        let out = resolve_include(&root, "src/main.tl", false);
        std::env::set_current_dir(prev).unwrap();

        let err = out.unwrap_err();
        assert!(err.contains("module not found: 'Tasks'"), "cwd decoy must not resolve: {err}");
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
