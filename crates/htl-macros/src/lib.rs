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

// ------------------------------------------------------------------ include_bundle!

/// `include_bundle!("src/main.tl", host = ["host"], extra = ["modkit"], payload = "source", debug = true)`
///
/// Links the entry's `require` closure at `cargo build` (see `htl::link`) and embeds
/// the encoded bundle as `&'static [u8]`; run it with
/// `Htl::run_bundle(&Bundle::decode(BUNDLE)?, &args)` after registering host modules.
/// Every linked file and every declaration the checker read is `include_bytes!`-tracked,
/// so an edit rebuilds and a Teal type error is a compile error, like `include_tl!`.
/// `payload` is `"bytecode"` (default, stripped; `debug = true` keeps line info) or
/// `"source"` (generated Lua: what to use when cross-compiling, since bytecode is
/// produced by the build machine's Lua).
#[proc_macro]
pub fn include_bundle(input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(input as BundleArgs);
    match expand_bundle(&args) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

struct BundleArgs {
    entry: String,
    opts: htl_core::link::LinkOptions,
}

impl syn::parse::Parse for BundleArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let entry: LitStr = input.parse()?;
        let mut opts = htl_core::link::LinkOptions::default();
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let key: syn::Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "host" => opts.host = parse_str_list(input)?,
                "extra" => opts.extra = parse_str_list(input)?,
                "payload" => {
                    let v: LitStr = input.parse()?;
                    match v.value().as_str() {
                        "bytecode" => opts.source = false,
                        "source" => opts.source = true,
                        other => return Err(syn::Error::new(v.span(), format!("payload must be \"bytecode\" or \"source\", got {other:?}"))),
                    }
                }
                "debug" => {
                    let v: syn::LitBool = input.parse()?;
                    opts.debug = v.value();
                }
                other => return Err(syn::Error::new(key.span(), format!("unknown include_bundle! option `{other}` (host, extra, payload, debug)"))),
            }
        }
        Ok(Self { entry: entry.value(), opts })
    }
}

fn parse_str_list(input: syn::parse::ParseStream) -> syn::Result<Vec<String>> {
    let content;
    syn::bracketed!(content in input);
    let items: Punctuated<LitStr, Token![,]> = content.parse_terminated(<LitStr as syn::parse::Parse>::parse, Token![,])?;
    Ok(items.iter().map(|l| l.value()).collect())
}

#[derive(Debug)]
struct BundleOut {
    bytes: Vec<u8>,
    inputs: Vec<String>,
}

/// Link `rel` (relative to `manifest_dir`) with the CLI's search paths and `[build]`
/// settings from `htl.toml` merged into `opts`.
fn resolve_bundle(manifest_dir: &Path, rel: &str, opts: &htl_core::link::LinkOptions) -> Result<BundleOut, String> {
    let path = manifest_dir.join(rel);
    if !path.is_file() {
        return Err(format!("include_bundle!: no such file: {}", path.display()));
    }
    let (h, cfg) = checker_for("include_bundle!", manifest_dir, &path)?;
    let mut opts = opts.clone();
    if let Some(c) = &cfg {
        opts.extra.extend(c.build.extra.iter().cloned());
        opts.host.extend(c.build.host.iter().cloned());
    }
    let linked = htl_core::link::link(&h, &path, &opts).map_err(|e| format!("include_bundle!: {e:#}"))?;
    for (_, ci) in &linked.checks {
        for w in &ci.warnings {
            eprintln!("include_bundle! warning: {w}");
        }
    }
    if !linked.lints.is_empty() {
        if lenient(&cfg) {
            for l in &linked.lints {
                eprintln!("include_bundle! lint: {l}");
            }
        } else {
            return Err(format!("htl lint failed (set HTL_LINT=warn to downgrade):\n{}", linked.lints.join("\n")));
        }
    }
    let inputs: Vec<String> = linked
        .inputs()
        .into_iter()
        .map(|p| if p.is_absolute() { p } else { manifest_dir.join(p) })
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let bundle = linked.into_bundle().map_err(|e| format!("include_bundle!: {e:#}"))?;
    Ok(BundleOut { bytes: bundle.encode(), inputs })
}

fn expand_bundle(args: &BundleArgs) -> Result<TokenStream, String> {
    let out = resolve_bundle(&manifest_dir()?, &args.entry, &args.opts)?;
    let lit = Literal::byte_string(&out.bytes);
    let inputs = out.inputs;
    Ok(quote! {{
        #( const _: &[u8] = include_bytes!(#inputs); )*
        #lit as &[u8]
    }}
    .into())
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
/// A checker set up the way the CLI would be for `path`: `htl.toml` lints, the file's
/// own dir, the crate's `src/`, the mlua-pkg project, `[check] paths`, the test lib.
/// Never the process cwd (cargo's), which has nothing to do with the script.
fn checker_for(tag: &str, manifest_dir: &Path, path: &Path) -> Result<(htl_core::Htl, Option<htl_core::config::HtlConfig>), String> {
    let h = htl_core::Htl::new().map_err(|e| format!("{tag}: {e:#}"))?;
    // htl.toml `[lint]` first, then HTL_LINTS, so the env var wins.
    let cfg = htl_core::config::HtlConfig::find(path).map_err(|e| format!("{tag}: {e:#}"))?;
    let cfg_root = cfg.as_ref().map(|(p, _)| htl_core::parent_dir(p));
    let cfg = cfg.map(|(_, c)| c);
    let file_spec = cfg.as_ref().map(|c| c.lint_spec()).unwrap_or_default();
    let env_spec = std::env::var("HTL_LINTS").unwrap_or_default();
    let spec = htl_core::config::join_specs([file_spec.as_str(), env_spec.as_str()]);
    if !spec.is_empty() {
        h.configure_lints(&spec).map_err(|e| format!("{tag}: lint spec {spec:?}: {e:#}"))?;
    }
    h.reset_search_path().map_err(|e| format!("{tag}: {e:#}"))?;
    h.add_path(&htl_core::parent_dir(path)).map_err(|e| format!("{tag}: {e:#}"))?;
    let crate_src = manifest_dir.join("src");
    if crate_src.is_dir() {
        h.add_path(&crate_src).map_err(|e| format!("{tag}: {e:#}"))?;
    }
    if let Some(p) = htl_core::pkg::Project::find(path) {
        h.apply_project(&p).map_err(|e| format!("{tag}: {e:#}"))?;
    }
    if let (Some(root), Some(c)) = (&cfg_root, &cfg) {
        h.apply_config(root, c).map_err(|e| format!("{tag}: {e:#}"))?;
    }
    h.install_test_lib().map_err(|e| format!("{tag}: {e:#}"))?;
    Ok((h, cfg))
}

/// Lints fail the build unless `HTL_LINT=warn`, else `htl.toml` `strict = false`.
fn lenient(cfg: &Option<htl_core::config::HtlConfig>) -> bool {
    match std::env::var("HTL_LINT") {
        Ok(v) => v == "warn",
        Err(_) => cfg.as_ref().and_then(|c| c.lint.strict) == Some(false),
    }
}

fn resolve_include(manifest_dir: &Path, rel: &str, bytes: bool) -> Result<Included, String> {
    let path = manifest_dir.join(rel);
    if !path.is_file() {
        return Err(format!("include_tl!: no such file: {}", path.display()));
    }
    let (h, cfg) = checker_for("include_tl!", manifest_dir, &path)?;
    let (code, ci) = h.gen_lua(&path).map_err(|e| format!("include_tl!: {e:#}"))?;

    for w in &ci.warnings {
        eprintln!("include_tl! warning: {w}");
    }
    let Some(code) = code else {
        return Err(format!("Teal type check failed:\n{}", ci.errors.join("\n")));
    };
    if !ci.lints.is_empty() {
        if lenient(&cfg) {
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
        let call = match m.receiver {
            Some(_) => quote! { this.#fname(#call_args) },
            None => quote! { <#self_ty>::#fname(#call_args) },
        };
        let body = match (m.ret_is_result, hd.err_mode) {
            (false, _) => quote! { ::htl::mlua::Result::Ok(#call) },
            (true, dts::ErrMode::Raise) => quote! { #call.map_err(::htl::mlua::Error::external) },
            // `value, err` convention: Ok(v) -> (v, nil); Ok(()) -> (true, nil); Err(e) -> (nil, e).
            (true, dts::ErrMode::Return) if m.ret_is_unit => quote! {
                match #call {
                    Ok(()) => ::htl::mlua::Result::Ok((Some(true), None::<String>)),
                    Err(e) => ::htl::mlua::Result::Ok((None::<bool>, Some(e.to_string()))),
                }
            },
            (true, dts::ErrMode::Return) => quote! {
                match #call {
                    Ok(v) => ::htl::mlua::Result::Ok((Some(v), None::<String>)),
                    Err(e) => ::htl::mlua::Result::Ok((None, Some(e.to_string()))),
                }
            },
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

    /// `htl.toml` drives the macro too: `[lint] enable` turns a rule on, and
    /// `strict = false` makes its findings advisory instead of a compile error.
    #[test]
    fn include_reads_htl_toml_lint_settings() {
        let root = scratch("htl-toml");
        // `no-any` is off by default; the script only trips when htl.toml enables it.
        write(&root.join("src/main.tl"), "local x: any = 1\nprint(x)\n");
        resolve_include(&root, "src/main.tl", false).expect("no-any is off by default");

        write(&root.join("htl.toml"), "[lint]\nenable = [\"no-any\"]\n");
        let err = resolve_include(&root, "src/main.tl", false).unwrap_err();
        assert!(err.contains("htl lint failed") && err.contains("no-any"), "{err}");

        write(&root.join("htl.toml"), "[lint]\nenable = [\"no-any\"]\nstrict = false\n");
        resolve_include(&root, "src/main.tl", false).expect("strict = false downgrades lints");
    }

    /// `include_bundle!`: the closure is linked, host modules are recorded, every input
    /// is tracked, and a type error anywhere in the closure is a compile error.
    #[test]
    fn bundle_links_closure_tracks_inputs_and_fails_on_type_errors() {
        let root = scratch("bundle");
        write(&root.join("src/main.tl"), "local util = require(\"util\")\nlocal host = require(\"host\")\nprint(util.twice(host.base()))\n");
        write(
            &root.join("src/util.tl"),
            "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
        );
        write(&root.join("src/host.d.tl"), "local record host\n   base: function(): integer\nend\nreturn host\n");
        write(&root.join("htl.toml"), "[build]\nextra = [\"plugin\"]\n");
        write(&root.join("src/plugin.tl"), "return { plugged = true }\n");

        let out = resolve_bundle(&root, "src/main.tl", &htl_core::link::LinkOptions::default()).expect("links");
        let b = htl_core::bundle::Bundle::decode(&out.bytes).unwrap();
        let names: Vec<&str> = b.modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"util") && names.contains(&"plugin"), "{names:?}");
        assert_eq!(b.host_modules, vec!["host".to_string()]);
        let has = |s: &str| out.inputs.iter().any(|p| p.ends_with(s));
        assert!(has("src/main.tl") && has("src/util.tl") && has("src/host.d.tl") && has("src/plugin.tl"), "{:?}", out.inputs);

        let src_opts = htl_core::link::LinkOptions { source: true, ..Default::default() };
        let out = resolve_bundle(&root, "src/main.tl", &src_opts).expect("links as source");
        let b = htl_core::bundle::Bundle::decode(&out.bytes).unwrap();
        assert!(b.modules.iter().all(|m| m.kind == htl_core::bundle::Kind::Source));

        write(&root.join("src/util.tl"), "local record util\nend\nfunction util.twice(n: integer): integer\n   return \"no\"\nend\nreturn util\n");
        let err = resolve_bundle(&root, "src/main.tl", &htl_core::link::LinkOptions::default()).unwrap_err();
        assert!(err.contains("util.tl") && err.contains("expected integer"), "{err}");
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
