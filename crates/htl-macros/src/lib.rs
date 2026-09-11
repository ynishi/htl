//! htl proc macros.
//!
//! - `include_tl!("path.tl")`        -> `&'static str` generated Lua, Teal-checked at build time
//! - `include_tl_bytes!("path.tl")`  -> `&'static [u8]` stripped Lua 5.4 bytecode, same check
//! - `#[derive(TealRecord)]`         -> Teal `record` / `enum` / `type` decl + `IntoLua` / `FromLua`
//! - `#[host_module(name = "...")]`  -> `UserData` impl + Teal `.d.tl` from a plain `impl` block
//! - `#[c_export(prefix = "...")]`   -> `extern "C"` wrappers + C header from the same `impl` block
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
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::{
    Item, ItemEnum, ItemImpl, ItemStruct, LitStr, Meta, Token, parse::Parser, parse_macro_input,
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

// ------------------------------------------------------------------ include_bundle!

/// `include_bundle!("src/main.tl", host = ["host"], extra = ["modkit"], payload = "source", debug = true)`
///
/// Links the entry's `require` closure at `cargo build` (see `htl::link`) and embeds
/// the encoded bundle as `&'static [u8]`; run it with
/// `Htl::run_bundle(&Bundle::decode(BUNDLE)?, &args)` after registering host modules.
/// Every linked file and every declaration the checker read is `include_bytes!`-tracked,
/// so an edit rebuilds and a Teal type error is a compile error, like `include_tl!`.
/// `payload` is `"bytecode"` (default, stripped; `debug = true` keeps line info) or
/// `"source"` (generated Lua: for a big-endian target, a Lua with non-default number
/// types, or a bundle that must outlive a Lua upgrade; bytecode already loads on every
/// 64-bit little-endian host, see `htl::bundle`).
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
                        other => {
                            return Err(syn::Error::new(
                                v.span(),
                                format!(
                                    "payload must be \"bytecode\" or \"source\", got {other:?}"
                                ),
                            ));
                        }
                    }
                }
                "debug" => {
                    let v: syn::LitBool = input.parse()?;
                    opts.debug = v.value();
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown include_bundle! option `{other}` (host, extra, payload, debug)"
                        ),
                    ));
                }
            }
        }
        Ok(Self {
            entry: entry.value(),
            opts,
        })
    }
}

fn parse_str_list(input: syn::parse::ParseStream) -> syn::Result<Vec<String>> {
    let content;
    syn::bracketed!(content in input);
    let items: Punctuated<LitStr, Token![,]> =
        content.parse_terminated(<LitStr as syn::parse::Parse>::parse, Token![,])?;
    Ok(items.iter().map(|l| l.value()).collect())
}

#[derive(Debug)]
struct BundleOut {
    bytes: Vec<u8>,
    inputs: Vec<String>,
    /// Typed modules replayed from the run cache, out of how many.
    cached: (usize, usize),
}

/// Link `rel` (relative to `manifest_dir`) with the CLI's search paths and `[build]`
/// settings from `htl.toml` merged into `opts`.
fn resolve_bundle(
    manifest_dir: &Path,
    rel: &str,
    opts: &htl_core::link::LinkOptions,
) -> Result<BundleOut, String> {
    let path = manifest_dir.join(rel);
    if !path.is_file() {
        return Err(format!("include_bundle!: no such file: {}", path.display()));
    }
    let ck = checker_for("include_bundle!", manifest_dir, &path)?;
    let (h, cfg) = (&ck.h, &ck.cfg);
    let mut opts = opts.clone();
    if let Some(c) = cfg {
        opts.extra.extend(c.build.extra.iter().cloned());
        opts.host.extend(c.build.host.iter().cloned());
    }
    let store = ck.store();
    let linked = htl_core::link::link_with(h, &path, &opts, ck.link_store(store.as_ref()))
        .map_err(|e| format!("include_bundle!: {e:#}"))?;
    let typed = linked.modules.iter().filter(|m| m.typed).count();
    let cached = (linked.cached, typed);
    for (_, ci) in &linked.checks {
        for w in &ci.warnings {
            eprintln!("include_bundle! warning: {w}");
        }
    }
    if !linked.lints.is_empty() {
        if lenient(cfg) {
            for l in &linked.lints {
                eprintln!("include_bundle! lint: {l}");
            }
        } else {
            return Err(format!(
                "htl lint failed (set HTL_LINT=warn to downgrade):\n{}",
                linked.lints.join("\n")
            ));
        }
    }
    let inputs: Vec<String> = linked
        .inputs()
        .into_iter()
        .map(|p| {
            if p.is_absolute() {
                p
            } else {
                manifest_dir.join(p)
            }
        })
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let bundle = linked
        .into_bundle()
        .map_err(|e| format!("include_bundle!: {e:#}"))?;
    Ok(BundleOut {
        bytes: bundle.encode(),
        inputs,
        cached,
    })
}

/// `HTL_CACHE_DEBUG`: say what the store did for this expansion, the way the CLI's
/// `--explain-cache` does at the end of a run. Build output is the only place a macro can
/// say anything, and only when asked.
fn explain_cache(tag: &str, cached: usize, of: usize) {
    if std::env::var_os("HTL_CACHE_DEBUG").is_some() {
        eprintln!("{tag}: {cached} of {of} module(s) replayed from the run cache");
    }
}

fn expand_bundle(args: &BundleArgs) -> Result<TokenStream, String> {
    let out = resolve_bundle(&manifest_dir()?, &args.entry, &args.opts)?;
    explain_cache("include_bundle!", out.cached.0, out.cached.1);
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
    /// Whether the check and the Lua came from the run cache.
    cached: bool,
}

#[derive(Debug)]
enum Payload {
    Source(String),
    Bytes(Vec<u8>),
}

/// Check + generate `rel` (relative to `manifest_dir`). Search paths match the CLI:
/// the file's own directory, the nearest `mlua-pkg.toml` project's installed deps at
/// their entries (and `target_dir` copies), and the bundled `htl.test` declarations.
/// A checker set up the way the CLI would be for `path`: `htl.toml` lints, the file's
/// own dir, the crate's `src/`, the mlua-pkg project, `[check] paths`, the test lib.
/// Never the process cwd (cargo's), which has nothing to do with the script.
/// A checker set up for one macro expansion, with what the run cache needs to key and
/// validate what the checker produces.
struct Checker {
    h: htl_core::Htl,
    cfg: Option<htl_core::config::HtlConfig>,
    /// Where `htl.toml` was found, if it was.
    cfg_path: Option<PathBuf>,
    /// The project root: beside `htl.toml`, else the crate's manifest directory. Where the
    /// store lives (`.htl/cache`), the same place the CLI keeps it.
    root: PathBuf,
    /// The lint selection in force: `[lint]` from `htl.toml`, then `HTL_LINTS`.
    spec: String,
}

impl Checker {
    /// The lint selection as a cache key takes it.
    fn lint(&self) -> Option<&str> {
        (!self.spec.is_empty()).then_some(self.spec.as_str())
    }

    /// The run cache, when there is a project to keep one in: an `htl.toml` was found
    /// (that is the opt-in; `htl init` / `htl new` write it and gitignore `.htl/`), and
    /// its directory is not build scratch — the copy `cargo publish` verifies under
    /// `target/package/`, where a new file aborts the publish, or a registry checkout.
    /// Otherwise `None`, silently: everything is generated, which is what happened before
    /// there was a store. `HTL_CACHE_DEBUG` says which of the two it was.
    ///
    /// Both rules and the opening itself are `htl_core::project`'s, which is where
    /// `htl check` opens the same store. An expansion and a check reading one store used
    /// to answer this question with two pieces of code, and a fix to one was not a fix to
    /// the other (#107).
    ///
    /// Per-module always, like `htl test`: an expansion is one closure, and an edit
    /// anywhere in it should cost that module and its dependents rather than the closure.
    /// Nothing is swept from here — an expansion sees one closure, and only `htl check`,
    /// which sees the project, bounds the store.
    fn store(&self) -> Option<htl_core::cache::Cache> {
        htl_core::project::store(
            &self.root,
            htl_core::cache::Options::from_env(),
            htl_core::project::store_refusal(&self.root, self.cfg_path.is_some()).as_deref(),
            "the macro expansion",
        )
    }

    fn link_store<'a>(
        &'a self,
        cache: Option<&'a htl_core::cache::Cache>,
    ) -> Option<htl_core::link::LinkStore<'a>> {
        cache.map(|c| htl_core::link::LinkStore {
            cache: c,
            lint: self.lint(),
            root: &self.root,
            config: match (&self.cfg_path, &self.cfg) {
                (Some(p), Some(c)) => Some((p.as_path(), c)),
                _ => None,
            },
        })
    }
}

fn checker_for(tag: &str, manifest_dir: &Path, path: &Path) -> Result<Checker, String> {
    let h = htl_core::Htl::new().map_err(|e| format!("{tag}: {e:#}"))?;
    // htl.toml `[lint]` first, then HTL_LINTS, so the env var wins.
    let cfg = htl_core::config::HtlConfig::find(path).map_err(|e| format!("{tag}: {e:#}"))?;
    let cfg_root = cfg.as_ref().map(|(p, _)| htl_core::parent_dir(p));
    let cfg_path = cfg.as_ref().map(|(p, _)| p.clone());
    let cfg = cfg.map(|(_, c)| c);
    let file_spec = cfg.as_ref().map(|c| c.lint_spec()).unwrap_or_default();
    let env_spec = std::env::var("HTL_LINTS").unwrap_or_default();
    let spec = htl_core::config::join_specs([file_spec.as_str(), env_spec.as_str()]);
    if !spec.is_empty() {
        h.configure_lints(&spec)
            .map_err(|e| format!("{tag}: lint spec {spec:?}: {e:#}"))?;
    }
    h.reset_search_path().map_err(|e| format!("{tag}: {e:#}"))?;
    h.add_path(&htl_core::parent_dir(path))
        .map_err(|e| format!("{tag}: {e:#}"))?;
    let crate_src = manifest_dir.join("src");
    if crate_src.is_dir() {
        h.add_path(&crate_src)
            .map_err(|e| format!("{tag}: {e:#}"))?;
    }
    if let Some(p) = htl_core::pkg::Project::find(path) {
        h.apply_project(&p).map_err(|e| format!("{tag}: {e:#}"))?;
    }
    if let (Some(root), Some(c)) = (&cfg_root, &cfg) {
        h.apply_config(root, c)
            .map_err(|e| format!("{tag}: {e:#}"))?;
    }
    h.install_test_lib().map_err(|e| format!("{tag}: {e:#}"))?;
    let root = cfg_root.unwrap_or_else(|| manifest_dir.to_path_buf());
    Ok(Checker {
        h,
        cfg,
        cfg_path,
        root,
        spec,
    })
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
    let ck = checker_for("include_tl!", manifest_dir, &path)?;
    let (h, cfg) = (&ck.h, &ck.cfg);
    // One module, through the same store the linker uses: its `gen` entry, if it still
    // holds, is the check and the Lua.
    let store = ck.store();
    let htl_core::link::Generated {
        code,
        check: ci,
        cached,
    } = htl_core::link::generate(h, &path, ck.link_store(store.as_ref()))
        .map_err(|e| format!("include_tl!: {e:#}"))?;

    for w in &ci.warnings {
        eprintln!("include_tl! warning: {w}");
    }
    let Some(code) = code else {
        return Err(format!("Teal type check failed:\n{}", ci.errors.join("\n")));
    };
    // The file checked, but something it required did not. The generated code would
    // build, and the module's first `require` would raise at run time; the build is where
    // that belongs, the same as for the file's own errors.
    if !ci.dependency_errors.is_empty() {
        let lines: Vec<String> = ci
            .dependency_errors
            .iter()
            .map(|e| format!("{}\n  (required by {})", e.text, e.required_by.display()))
            .collect();
        return Err(format!(
            "Teal type check failed in a required module:\n{}",
            lines.join("\n")
        ));
    }
    if !ci.lints.is_empty() {
        if lenient(cfg) {
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
            let p = if d.is_absolute() {
                d.clone()
            } else {
                manifest_dir.join(d)
            };
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

    Ok(Included {
        main_abs,
        deps,
        payload,
        cached,
    })
}

fn expand_include(rel: &str, bytes: bool) -> Result<TokenStream, String> {
    let inc = resolve_include(&manifest_dir()?, rel, bytes)?;
    explain_cache("include_tl!", usize::from(inc.cached), 1);
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
    htl_core::write_if_changed(&path, text)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(())
}

// ------------------------------------------------------------------ #[derive(TealRecord)]

#[proc_macro_derive(TealRecord, attributes(teal))]
pub fn derive_teal_record(input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as Item);
    match expand_record(&item) {
        Ok(ts) => ts,
        Err(msg) => quote! { compile_error!(#msg) }.into(),
    }
}

/// What every kind gets: the `TealRecord` constants, `IntoLua` / `FromLua` around the
/// per-kind bodies, and `htl_preload`. The declaration text and the kind come from
/// `htl_core::dts`, the same code `htl dts` runs, so the `.d.tl` a build writes and the
/// one the CLI writes cannot disagree.
fn expand_record(item: &Item) -> Result<TokenStream, String> {
    let rd = dts::record_decl(item)?;
    if let Some(d) = &rd.attrs.dts {
        write_dts(d, &rd.decl)?;
    }
    let name = rd.name.as_str();
    let (ident, into, from) = match (item, &rd.kind) {
        (Item::Struct(st), dts::RecordKind::Record { fields }) => {
            (&st.ident, record_into(st), record_from(st, name, fields))
        }
        (Item::Struct(st), dts::RecordKind::Alias { inner }) => {
            (&st.ident, alias_into(), alias_from(st, name, inner))
        }
        (Item::Enum(en), dts::RecordKind::Enum { variants }) => (
            &en.ident,
            enum_into(en, variants),
            enum_from(en, name, variants),
        ),
        (Item::Enum(en), dts::RecordKind::Union { variants }) => (
            &en.ident,
            union_into(en, variants),
            union_from(en, name, variants),
        ),
        _ => return Err("TealRecord: only structs and enums are supported".into()),
    };
    let decl = &rd.decl;
    // A data-carrying enum is never a `.d.tl` module of its own (see `dts`), so there
    // is no `require("NAME")` for it to satisfy.
    let preload = if matches!(rd.kind, dts::RecordKind::Union { .. }) {
        quote! {}
    } else {
        quote! {
            impl #ident {
                /// Make `require("NAME")` resolve at runtime (type-only module -> empty table).
                pub fn htl_preload(h: &::htl::Htl) -> ::htl::mlua::Result<()> {
                    let t = h.lua().create_table()?;
                    h.preload_value(#name, t).map_err(::htl::mlua::Error::external)
                }
            }
        }
    };
    Ok(quote! {
        impl ::htl::teal::TealRecord for #ident {
            const NAME: &'static str = #name;
            const DECL: &'static str = #decl;
        }
        impl ::htl::mlua::IntoLua for #ident {
            fn into_lua(self, lua: &::htl::mlua::Lua) -> ::htl::mlua::Result<::htl::mlua::Value> {
                #into
            }
        }
        impl ::htl::mlua::FromLua for #ident {
            fn from_lua(value: ::htl::mlua::Value, lua: &::htl::mlua::Lua) -> ::htl::mlua::Result<Self> {
                #from
            }
        }
        #preload
    }
    .into())
}

/// `field: { .. }` initializers reading each named field out of table `t`, in
/// declaration order, so `teal` (from `dts`, same order) pairs with the Rust types.
/// A failed field is reported under `path` (`Record` or `Enum.Variant`).
fn field_reads(
    path: &str,
    named: &syn::FieldsNamed,
    teal: &[(String, String)],
) -> Vec<TokenStream2> {
    named
        .named
        .iter()
        .zip(teal)
        .map(|(f, (fname, fteal))| {
            let id = f.ident.as_ref().unwrap();
            let read = field_read(path, fname, &f.ty, fteal);
            quote! { #id: #read }
        })
        .collect()
}

/// A block reading key `fname` out of table `t` as `ty`, reporting a failure as
/// `path.fname` with the Teal type `fteal` the declaration promised.
fn field_read(path: &str, fname: &str, ty: &syn::Type, fteal: &str) -> TokenStream2 {
    quote! {
        {
            // Take the field as a Value first: what arrived is half the message, and
            // the conversion consumes it.
            let v: ::htl::mlua::Value = t.get(#fname)?;
            let got = v.type_name();
            <#ty as ::htl::mlua::FromLua>::from_lua(v, lua).map_err(|e| {
                ::htl::teal::field_error(#path, #fname, #fteal, got, e)
            })?
        }
    }
}

fn named_fields(st: &ItemStruct) -> &syn::FieldsNamed {
    match &st.fields {
        syn::Fields::Named(n) => n,
        _ => unreachable!("dts::record_decl classified this struct as a record"),
    }
}

/// Record -> table, one key per field.
fn record_into(st: &ItemStruct) -> TokenStream2 {
    let idents: Vec<_> = named_fields(st)
        .named
        .iter()
        .map(|f| f.ident.as_ref().unwrap())
        .collect();
    let names: Vec<String> = idents.iter().map(|i| i.to_string()).collect();
    quote! {
        let t = lua.create_table()?;
        #( t.set(#names, self.#idents)?; )*
        Ok(::htl::mlua::Value::Table(t))
    }
}

/// Table -> record, every field checked and named on failure.
fn record_from(st: &ItemStruct, name: &str, fields: &[(String, String)]) -> TokenStream2 {
    let reads = field_reads(name, named_fields(st), fields);
    quote! {
        let t = <::htl::mlua::Table as ::htl::mlua::FromLua>::from_lua(value, lua)?;
        Ok(Self { #( #reads, )* })
    }
}

/// Newtype -> whatever the inner value converts to.
fn alias_into() -> TokenStream2 {
    quote! { ::htl::mlua::IntoLua::into_lua(self.0, lua) }
}

/// Value -> newtype, through the inner type's own conversion.
fn alias_from(st: &ItemStruct, name: &str, inner: &str) -> TokenStream2 {
    let ty = match &st.fields {
        syn::Fields::Unnamed(u) => &u.unnamed[0].ty,
        _ => unreachable!("dts::record_decl classified this struct as an alias"),
    };
    quote! {
        let got = value.type_name();
        let inner = <#ty as ::htl::mlua::FromLua>::from_lua(value, lua)
            .map_err(|e| ::htl::teal::value_error(#name, #inner, got, e))?;
        Ok(Self(inner))
    }
}

fn variant_idents(en: &ItemEnum) -> Vec<&syn::Ident> {
    en.variants.iter().map(|v| &v.ident).collect()
}

/// Unit enum -> the variant's word as a string: the one the declaration lists, which
/// `#[teal(rename_all)]` / `#[teal(name)]` may have spelled differently from Rust.
fn enum_into(en: &ItemEnum, words: &[String]) -> TokenStream2 {
    let idents = variant_idents(en);
    quote! {
        let s = match self { #( Self::#idents => #words, )* };
        Ok(::htl::mlua::Value::String(lua.create_string(s)?))
    }
}

/// String -> unit enum; anything else, or a string that is no variant, names the enum
/// and lists what it accepts.
fn enum_from(en: &ItemEnum, name: &str, variants: &[String]) -> TokenStream2 {
    let idents = variant_idents(en);
    quote! {
        const VARIANTS: &[&str] = &[#( #variants ),*];
        match &value {
            ::htl::mlua::Value::String(s) => {
                // A string that is not UTF-8 is no variant either; it is reported by
                // its type rather than quoted, as there is nothing readable to quote.
                let Ok(s) = s.to_str() else {
                    return Err(::htl::teal::enum_error(#name, VARIANTS, "string"));
                };
                match &*s {
                    #( #variants => Ok(Self::#idents), )*
                    other => Err(::htl::teal::enum_error(#name, VARIANTS, &format!("\"{other}\""))),
                }
            }
            other => Err(::htl::teal::enum_error(#name, VARIANTS, other.type_name())),
        }
    }
}

/// Data enum -> table: `kind` carries the variant's word (the one its `where` clause
/// tests), a newtype payload goes under `value`, struct-variant fields under their own
/// names.
fn union_into(en: &ItemEnum, variants: &[dts::UnionVariant]) -> TokenStream2 {
    let arms: Vec<TokenStream2> = en
        .variants
        .iter()
        .zip(variants)
        .map(|(v, uv)| {
            let id = &v.ident;
            let vname = uv.word.as_str();
            match &v.fields {
                syn::Fields::Unit => quote! {
                    Self::#id => { t.set("kind", #vname)?; }
                },
                syn::Fields::Unnamed(_) => quote! {
                    Self::#id(value) => { t.set("kind", #vname)?; t.set("value", value)?; }
                },
                syn::Fields::Named(n) => {
                    let ids: Vec<_> = n.named.iter().map(|f| f.ident.as_ref().unwrap()).collect();
                    let names: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
                    // Bound under prefixed names: a field called `t` or `lua` must not
                    // shadow the table or the state the arm writes through.
                    let binds: Vec<_> =
                        ids.iter().map(|i| format_ident!("__htl_f_{}", i)).collect();
                    quote! {
                        Self::#id { #( #ids: #binds ),* } => {
                            t.set("kind", #vname)?;
                            #( t.set(#names, #binds)?; )*
                        }
                    }
                }
            }
        })
        .collect();
    quote! {
        let t = lua.create_table()?;
        match self { #( #arms )* }
        Ok(::htl::mlua::Value::Table(t))
    }
}

/// Table -> data enum: `kind` picks the variant, then its fields are read as a record's
/// are, reported under `Enum.Variant`. No table, no `kind`, or an unknown one names the
/// enum and lists the variants.
fn union_from(en: &ItemEnum, name: &str, variants: &[dts::UnionVariant]) -> TokenStream2 {
    let vnames: Vec<&str> = variants.iter().map(|v| v.word.as_str()).collect();
    let arms: Vec<TokenStream2> = en
        .variants
        .iter()
        .zip(variants)
        .map(|(v, uv)| {
            let id = &v.ident;
            let vname = uv.word.as_str();
            // The path names the variant record the value failed to fill
            // (`Shape.Rect.h` for `record Shape_Rect`), which stays the Rust name.
            let path = format!("{name}.{}", uv.name);
            match (&v.fields, &uv.shape) {
                (syn::Fields::Unit, _) => quote! { #vname => Ok(Self::#id), },
                (syn::Fields::Unnamed(u), dts::VariantShape::Newtype(teal)) => {
                    let read = field_read(&path, "value", &u.unnamed[0].ty, teal);
                    quote! { #vname => Ok(Self::#id(#read)), }
                }
                (syn::Fields::Named(n), dts::VariantShape::Struct(fields)) => {
                    let reads = field_reads(&path, n, fields);
                    quote! { #vname => Ok(Self::#id { #( #reads, )* }), }
                }
                _ => unreachable!("dts::record_decl and this walk see the same variants"),
            }
        })
        .collect();
    quote! {
        const VARIANTS: &[&str] = &[#( #vnames ),*];
        let t = match value {
            ::htl::mlua::Value::Table(t) => t,
            other => return Err(::htl::teal::enum_error(#name, VARIANTS, other.type_name())),
        };
        let kind: ::htl::mlua::Value = t.get("kind")?;
        let kind = match &kind {
            ::htl::mlua::Value::String(s) => match s.to_str() {
                Ok(s) => s.to_string(),
                // Not UTF-8: no variant, reported by type (nothing readable to quote).
                Err(_) => return Err(::htl::teal::enum_error(#name, VARIANTS, "string")),
            },
            other => return Err(::htl::teal::enum_error(#name, VARIANTS, other.type_name())),
        };
        match kind.as_str() {
            #( #arms )*
            other => Err(::htl::teal::enum_error(#name, VARIANTS, &format!("\"{other}\""))),
        }
    }
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

fn expand_host_module(
    metas: Punctuated<Meta, Token![,]>,
    imp: &ItemImpl,
) -> Result<TokenStream, String> {
    let attrs = dts::parse_attr_metas(metas)?;
    let file_items = if attrs.records.is_empty() {
        None
    } else {
        current_file_items()
    };
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
        let arg_pats: Vec<_> = m
            .params
            .iter()
            .map(|p| format_ident!("{}", p.name))
            .collect();
        let arg_tys: Vec<&syn::Type> = m.params.iter().map(|p| &p.owned_ty).collect();
        let call_exprs: Vec<_> = m
            .params
            .iter()
            .map(|p| {
                let id = format_ident!("{}", p.name);
                if p.by_ref {
                    quote! { &#id }
                } else {
                    quote! { #id }
                }
            })
            .collect();
        let call_args = quote! { #( #call_exprs ),* };
        let call = match m.receiver {
            Some(_) => quote! { this.#fname(#call_args) },
            None => quote! { <#self_ty>::#fname(#call_args) },
        };
        // An `async fn` returns a future; everything downstream — the error handling, the
        // `value, err` convention — is about the value it resolves to.
        let call = if m.is_async {
            quote! { #call.await }
        } else {
            call
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
        // The async variants differ in more than the name: they take the Lua by value and
        // the receiver as a borrow guard (`UserDataRef`) that the future holds across
        // every await, and the future itself must be `'static`. `async move` is what
        // makes it one — the arguments are moved in rather than borrowed from the call.
        registrations.push(match (m.receiver, m.is_async) {
            (Some(false), false) => quote! { m.add_method(#fname_s, |_lua, this, #pat| #body); },
            (Some(true), false) => quote! { m.add_method_mut(#fname_s, |_lua, this, #pat| #body); },
            (None, false) => quote! { m.add_function(#fname_s, |_lua, #pat| #body); },
            (Some(false), true) => quote! {
                m.add_async_method(#fname_s, |_lua, this, #pat| async move { #body });
            },
            (Some(true), true) => quote! {
                m.add_async_method_mut(#fname_s, |_lua, mut this, #pat| async move { #body });
            },
            (None, true) => quote! {
                m.add_async_function(#fname_s, |_lua, #pat| async move { #body });
            },
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

// ------------------------------------------------------------------ #[c_export]

/// `#[c_export(prefix = "hello", header = "include/hello.h")]` on an `impl` block ->
/// one `extern "C"` wrapper per `pub fn`, the handle lifecycle, and the C header.
///
/// The mirror of [`macro@host_module`]: the same breakdown of the `impl` block, said to
/// a caller that is not written in Rust. What is generated, what each wrapper's shape
/// is, and which signatures are refused is in `htl::cexport`; the runtime the generated
/// code calls is `htl::ffi`, which needs the `ffi` feature of the `htl` crate.
///
/// `prefix` defaults to the type name lowercased. Without `header` the text is still
/// `<Type>::HEADER`; with it, the file is written at expansion time relative to
/// `CARGO_MANIFEST_DIR` and only when its contents changed, exactly as
/// `#[host_module(dts = "..")]` writes its `.d.tl`.
#[proc_macro_attribute]
pub fn c_export(attr: TokenStream, item: TokenStream) -> TokenStream {
    let metas = match Punctuated::<Meta, Token![,]>::parse_terminated.parse(attr) {
        Ok(m) => m,
        Err(e) => {
            let msg = e.to_string();
            return quote! { compile_error!(#msg) }.into();
        }
    };
    let imp = parse_macro_input!(item as ItemImpl);
    match expand_c_export(metas, &imp) {
        Ok(ts) => ts,
        // The impl block is emitted even when the attribute refuses it, so that the one
        // error the reader has to act on is not buried under "no method named .." for
        // every call site.
        Err(msg) => quote! { #imp compile_error!(#msg); }.into(),
    }
}

fn expand_c_export(
    metas: Punctuated<Meta, Token![,]>,
    imp: &ItemImpl,
) -> Result<TokenStream, String> {
    let attrs = htl_core::cexport::parse_c_export_metas(metas)?;
    if imp.trait_.is_some() {
        return Err("c_export: put it on the inherent `impl` block, not on a trait impl".into());
    }
    if !imp.generics.params.is_empty() {
        return Err(
            "c_export: the impl block is generic; there is one C symbol per method, and a \
             generic type has one per instantiation. Export a concrete type"
                .into(),
        );
    }
    let hd = dts::host_decl(imp, dts::TealAttrs::default(), None)?;
    let plan = htl_core::cexport::plan(&hd, imp, attrs)?;
    let header = plan.header();
    if let Some(path) = &plan.header_path {
        write_dts(path, &header)?;
    }

    let self_ty = &imp.self_ty;
    let prefix = plan.prefix.as_str();
    let mut fns = vec![c_runtime_fns(&plan, self_ty), c_open_fn(&plan, self_ty)];
    for m in &plan.methods {
        fns.push(c_method_fn(&plan, m, self_ty));
    }
    Ok(quote! {
        #imp
        impl ::htl::ffi::CExport for #self_ty {
            const PREFIX: &'static str = #prefix;
            const HEADER: &'static str = #header;
            const ABI_VERSION: ::core::ffi::c_int = ::htl::ffi::ABI_VERSION;
        }
        #( #fns )*
    }
    .into())
}

/// The `# Safety` clause every generated pointer-taking function carries: one sentence
/// per pointer it is handed, because clippy asks for it and because it is the contract.
fn c_safety(what: &str) -> String {
    format!("# Safety\n\n{what}")
}

/// The functions that do not come from a method: versions, the error slot, `free`, and
/// the two ends of the handle's life.
fn c_runtime_fns(plan: &htl_core::cexport::CPlan, self_ty: &syn::Type) -> TokenStream2 {
    let p = plan.prefix.as_str();
    let (abi, ver, ts) = (
        format_ident!("{p}_abi_version"),
        format_ident!("{p}_version"),
        format_ident!("{p}_threadsafe"),
    );
    let (le, lei, ls) = (
        format_ident!("{p}_last_error"),
        format_ident!("{p}_last_error_into"),
        format_ident!("{p}_last_status"),
    );
    let (free, close, int) = (
        format_ident!("{p}_free"),
        format_ident!("{p}_close"),
        format_ident!("{p}_interrupt"),
    );
    let d_le = "The last error on this thread, or NULL. Owned by this library and valid \
                until the next call on this thread; do not free it.";
    let d_lei = c_safety("`buf` must be writable for `len` bytes.");
    let d_free = c_safety(
        "`p` must be NULL or a pointer this library returned and has not taken back. \
         Freeing anything else, or the same pointer twice, is undefined behaviour.",
    );
    let d_close = c_safety(
        "`h` must be NULL or a handle from `_open` that has not been closed, and no call \
         may be in progress on it. Closing from another thread does nothing.",
    );
    let d_int = c_safety(
        "`h` must be NULL or a live handle. Callable from any thread; the caller keeps \
         the handle alive until it stops calling.",
    );
    quote! {
        #[doc = "The shape of this ABI. Compare it before calling anything else."]
        #[unsafe(no_mangle)]
        pub extern "C" fn #abi() -> ::core::ffi::c_int {
            ::htl::ffi::ABI_VERSION
        }

        #[doc = "The version of the crate that was built, as a static string."]
        #[unsafe(no_mangle)]
        pub extern "C" fn #ver() -> *const ::core::ffi::c_char {
            concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const ::core::ffi::c_char
        }

        #[doc = "0: one handle per thread (the convention of sqlite3_threadsafe)."]
        #[unsafe(no_mangle)]
        pub extern "C" fn #ts() -> ::core::ffi::c_int {
            0
        }

        #[doc = #d_le]
        #[unsafe(no_mangle)]
        pub extern "C" fn #le() -> *const ::core::ffi::c_char {
            ::htl::ffi::last_error_ptr()
        }

        #[doc = #d_lei]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #lei(
            buf: *mut ::core::ffi::c_char,
            len: ::core::ffi::c_int,
        ) -> ::core::ffi::c_int {
            unsafe { ::htl::ffi::last_error_into(buf, len) }
        }

        #[doc = "The status that went with the last error."]
        #[unsafe(no_mangle)]
        pub extern "C" fn #ls() -> ::core::ffi::c_int {
            ::htl::ffi::last_status()
        }

        #[doc = #d_free]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #free(p: *mut ::core::ffi::c_char) {
            unsafe { ::htl::ffi::free(p) }
        }

        #[doc = #d_close]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #close(__handle: *mut ::core::ffi::c_void) {
            const MAGIC: u64 = ::htl::ffi::magic(#p);
            unsafe { ::htl::ffi::Handle::<#self_ty>::close(__handle, MAGIC) }
        }

        #[doc = #d_int]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #int(__handle: *mut ::core::ffi::c_void) -> ::core::ffi::c_int {
            const MAGIC: u64 = ::htl::ffi::magic(#p);
            unsafe { ::htl::ffi::Handle::<#self_ty>::interrupt(__handle, MAGIC) }
        }
    }
}

/// The signature fragments, the decoding of each argument, and the call arguments.
///
/// Arguments are decoded *before* the handle is entered: a NULL or non-UTF-8 argument is
/// the caller's mistake, not the handle's, and must not count as a failed call on it.
fn c_params(
    f: &htl_core::cexport::CFn,
    fail: &TokenStream2,
) -> (Vec<TokenStream2>, Vec<TokenStream2>, Vec<TokenStream2>) {
    use htl_core::cexport::ParamKind;
    let (mut sig, mut dec, mut call) = (Vec::new(), Vec::new(), Vec::new());
    for p in &f.params {
        if p.kind == ParamKind::Interrupt {
            call.push(quote! { __interrupt });
            continue;
        }
        let id = format_ident!("{}", p.name);
        let what = p.c_name.as_str();
        let ty = &p.ty;
        match p.kind {
            ParamKind::Text => {
                sig.push(quote! { #id: *const ::core::ffi::c_char });
                dec.push(quote! {
                    let #id: ::std::string::String = match unsafe { ::htl::ffi::arg_str(#id, #what) } {
                        ::core::option::Option::Some(__s) => ::std::string::String::from(__s),
                        ::core::option::Option::None => return #fail,
                    };
                });
            }
            ParamKind::Int => {
                sig.push(quote! { #id: ::core::ffi::c_int });
                dec.push(quote! { let #id: i32 = #id; });
            }
            ParamKind::Json => {
                sig.push(quote! { #id: *const ::core::ffi::c_char });
                dec.push(quote! {
                    let #id: #ty = match unsafe { ::htl::ffi::arg_str(#id, #what) } {
                        ::core::option::Option::Some(__s) => {
                            match ::htl::ffi::from_json::<#ty>(__s, #what) {
                                ::core::result::Result::Ok(__v) => __v,
                                ::core::result::Result::Err(__e) => {
                                    ::htl::ffi::set_error(::htl::ffi::Status::Err, __e);
                                    return #fail;
                                }
                            }
                        }
                        ::core::option::Option::None => return #fail,
                    };
                });
            }
            ParamKind::Interrupt => unreachable!("handled above"),
        }
        call.push(if p.by_ref {
            quote! { &#id }
        } else {
            quote! { #id }
        });
    }
    (sig, dec, call)
}

/// `<prefix>_open`: decode the options, build the value, hand back the handle.
fn c_open_fn(plan: &htl_core::cexport::CPlan, self_ty: &syn::Type) -> TokenStream2 {
    use htl_core::cexport::ParamKind;
    let f = &plan.open;
    let p = plan.prefix.as_str();
    let name = format_ident!("{}", f.c_name);
    let rust = format_ident!("{}", f.rust_name);
    let fail = quote! { ::core::ptr::null_mut() };
    let (sig, dec, call) = c_params(f, &fail);
    // The flag exists whether or not the opener asked for it; naming it `_interrupt`
    // when it did not keeps the generated code warning-free in the caller's crate.
    let flag = if f.params.iter().any(|x| x.kind == ParamKind::Interrupt) {
        format_ident!("__interrupt")
    } else {
        format_ident!("_interrupt")
    };
    let make = if f.is_result {
        quote! {
            <#self_ty>::#rust(#( #call ),*)
                .map_err(|__e| ::std::string::ToString::to_string(&__e))
        }
    } else {
        quote! {
            ::core::result::Result::Ok::<_, ::std::string::String>(
                <#self_ty>::#rust(#( #call ),*)
            )
        }
    };
    let doc = c_safety(
        "Every `const char *` argument must be NULL or a NUL-terminated UTF-8 string \
         that stays alive for the call. The returned handle belongs to this thread and \
         is closed with `_close`; NULL means the call failed, and `_last_error` says why.",
    );
    quote! {
        #[doc = #doc]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #name(#( #sig ),*) -> *mut ::core::ffi::c_void {
            const MAGIC: u64 = ::htl::ffi::magic(#p);
            ::htl::ffi::clear_error();
            #( #dec )*
            ::htl::ffi::Handle::<#self_ty>::open(MAGIC, move |#flag| #make)
        }
    }
}

/// One method: the wrapper whose shape `cexport::Shape` chose.
fn c_method_fn(
    plan: &htl_core::cexport::CPlan,
    m: &htl_core::cexport::CFn,
    self_ty: &syn::Type,
) -> TokenStream2 {
    use htl_core::cexport::Shape;
    let p = plan.prefix.as_str();
    let name = format_ident!("{}", m.c_name);
    let rust = format_ident!("{}", m.rust_name);
    let (ret, fail) = match m.shape {
        Shape::Void => (quote! {}, quote! {}),
        Shape::Text | Shape::Json => (
            quote! { -> *mut ::core::ffi::c_char },
            quote! { ::core::ptr::null_mut() },
        ),
        Shape::Status | Shape::IntOut => (
            quote! { -> ::core::ffi::c_int },
            quote! { ::htl::ffi::Status::Err.code() },
        ),
    };
    let (sig, dec, call) = c_params(m, &fail);
    let invoke = quote! { __this.#rust(#( #call ),*) };
    let ok = quote! { ::htl::ffi::Status::Ok.code() };
    let body = match (m.shape, m.is_result) {
        (Shape::Void, _) => quote! { #invoke; },
        (Shape::Text, false) => quote! { ::htl::ffi::give(#invoke) },
        (Shape::Text, true) => quote! {
            match #invoke {
                ::core::result::Result::Ok(__v) => ::htl::ffi::give(__v),
                ::core::result::Result::Err(__e) => {
                    ::htl::ffi::fail(__e);
                    ::core::ptr::null_mut()
                }
            }
        },
        (Shape::Json, false) => quote! { ::htl::ffi::give_json(&#invoke) },
        (Shape::Json, true) => quote! {
            match #invoke {
                ::core::result::Result::Ok(__v) => ::htl::ffi::give_json(&__v),
                ::core::result::Result::Err(__e) => {
                    ::htl::ffi::fail(__e);
                    ::core::ptr::null_mut()
                }
            }
        },
        (Shape::Status, _) => quote! {
            match #invoke {
                ::core::result::Result::Ok(()) => #ok,
                ::core::result::Result::Err(__e) => ::htl::ffi::fail(__e).code(),
            }
        },
        (Shape::IntOut, false) => quote! {
            let __v = #invoke;
            unsafe { *__out = __v as ::core::ffi::c_int };
            #ok
        },
        (Shape::IntOut, true) => quote! {
            match #invoke {
                ::core::result::Result::Ok(__v) => {
                    unsafe { *__out = __v as ::core::ffi::c_int };
                    #ok
                }
                ::core::result::Result::Err(__e) => ::htl::ffi::fail(__e).code(),
            }
        },
    };
    let (out_sig, out_check) = if m.shape == Shape::IntOut {
        (
            quote! { __out: *mut ::core::ffi::c_int, },
            quote! {
                if __out.is_null() {
                    ::htl::ffi::set_error(::htl::ffi::Status::Err, "argument `out` is NULL");
                    return ::htl::ffi::Status::Err.code();
                }
            },
        )
    } else {
        (quote! {}, quote! {})
    };
    let enter = match m.shape {
        Shape::Status | Shape::IntOut => quote! {
            unsafe {
                ::htl::ffi::Handle::<#self_ty>::enter_status(__handle, MAGIC, move |__this| { #body })
            }
        },
        Shape::Void => quote! {
            unsafe {
                ::htl::ffi::Handle::<#self_ty>::enter(__handle, MAGIC, (), move |__this| { #body })
            }
        },
        Shape::Text | Shape::Json => quote! {
            unsafe {
                ::htl::ffi::Handle::<#self_ty>::enter(
                    __handle, MAGIC, ::core::ptr::null_mut(), move |__this| { #body },
                )
            }
        },
    };
    let doc = c_safety(
        "`h` must be NULL or a handle from `_open`, used on the thread that opened it; \
         any `const char *` argument must be NULL or a NUL-terminated UTF-8 string alive \
         for the call, and any out-parameter must be writable.",
    );
    quote! {
        #[doc = #doc]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #name(
            __handle: *mut ::core::ffi::c_void,
            #( #sig, )*
            #out_sig
        ) #ret {
            const MAGIC: u64 = ::htl::ffi::magic(#p);
            ::htl::ffi::clear_error();
            #( #dec )*
            #out_check
            #enter
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory under the system temp dir. Counted rather than timestamped: the
    /// clock advances in microsecond steps, so two calls close together get the same
    /// value and the same directory.
    fn scratch(name: &str) -> PathBuf {
        static NTH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "htl-macros-test-{name}-{}-{}",
            std::process::id(),
            NTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// The macro must see the same tree as the CLI: an installed dependency, reached at its
    /// entry (`.htl/modules/entries/<name>/init.tl`), resolves from a script under the
    /// project.
    #[test]
    fn include_resolves_vendored_dep_from_mlua_pkg_project() {
        let root = scratch("vendored");
        write(
            &root.join("mlua-pkg.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[deps]\n",
        );
        write(
            &root.join(".htl/modules/entries/mathx/init.tl"),
            "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
        );
        write(
            &root.join("scripts/main.tl"),
            "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
        );

        let inc =
            resolve_include(&root, "scripts/main.tl", false).expect("vendored dep must resolve");
        assert!(inc.main_abs.ends_with("scripts/main.tl"));
        assert!(
            inc.deps
                .iter()
                .any(|d| d.ends_with("entries/mathx/init.tl")),
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
        write(
            &root.join("src/main.tl"),
            "local mathx = require(\"mathx\")\nprint(mathx.twice(1))\n",
        );

        let inc = resolve_include(&root, "src/main.tl", true).expect("target_dir dep must resolve");
        assert!(matches!(inc.payload, Payload::Bytes(ref b) if !b.is_empty()));
    }

    /// A flat package exposes its top-level module as `<name>/<name>.tl`.
    #[test]
    fn include_resolves_flat_package_module() {
        let root = scratch("flat");
        write(
            &root.join("mlua-pkg.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[deps]\n",
        );
        write(
            &root.join(".htl/modules/entries/mathx/mathx.tl"),
            "local record mathx\nend\nfunction mathx.twice(n: number): number\n   return n * 2\nend\nreturn mathx\n",
        );
        write(
            &root.join("src/main.tl"),
            "local mathx = require(\"mathx\")\nprint(mathx.twice(1))\n",
        );
        resolve_include(&root, "src/main.tl", false).expect("flat package must resolve");
    }

    /// The process cwd (cargo's, during a build) must not take part in resolution: a
    /// same-named module sitting there is neither picked up nor tracked by a relative path.
    #[test]
    fn include_ignores_modules_in_the_process_cwd() {
        let decoy = scratch("cwd-decoy");
        write(
            &decoy.join("Tasks.tl"),
            "local record Tasks\nend\nreturn Tasks\n",
        );
        let root = scratch("cwd-crate");
        write(
            &root.join("src/main.tl"),
            "local ok, t = pcall(require, \"Tasks\")\nprint(ok, t)\n",
        );

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&decoy).unwrap();
        let out = resolve_include(&root, "src/main.tl", false);
        std::env::set_current_dir(prev).unwrap();

        let err = out.unwrap_err();
        assert!(
            err.contains("module not found: 'Tasks'"),
            "cwd decoy must not resolve: {err}"
        );
    }

    /// `htl.toml` drives the macro too: `[lint] enable` turns a rule on, and
    /// `strict = false` makes its findings advisory instead of a compile error.
    #[test]
    fn include_reads_htl_toml_lint_settings() {
        let root = scratch("htl-toml");
        // `no-any` is off by default; the script only trips when htl.toml enables it.
        write(&root.join("src/main.tl"), "local x: any = 1\nprint(x)\n");
        resolve_include(&root, "src/main.tl", false).expect("no-any is off by default");

        write(&root.join("htl.toml"), "[lint.rules]\nno-any = \"warn\"\n");
        let err = resolve_include(&root, "src/main.tl", false).unwrap_err();
        assert!(
            err.contains("htl lint failed") && err.contains("no-any"),
            "{err}"
        );

        write(
            &root.join("htl.toml"),
            "[lint]\nstrict = false\n\n[lint.rules]\nno-any = \"warn\"\n",
        );
        resolve_include(&root, "src/main.tl", false).expect("strict = false downgrades lints");
    }

    /// Both macros read the run cache under the project root: the second expansion
    /// replays what the first generated, an edit is a miss for the module and its
    /// dependents, and what comes out is the same either way.
    #[test]
    fn macros_replay_from_the_run_cache_on_the_second_expansion() {
        let root = scratch("cache");
        write(&root.join("htl.toml"), "[check]\n");
        write(
            &root.join("src/main.tl"),
            "local util = require(\"util\")\nprint(util.twice(21))\n",
        );
        write(
            &root.join("src/util.tl"),
            "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
        );
        let opts = htl_core::link::LinkOptions::default();
        let first = resolve_bundle(&root, "src/main.tl", &opts).expect("links");
        assert_eq!(first.cached, (0, 2), "cold");
        let second = resolve_bundle(&root, "src/main.tl", &opts).expect("links");
        assert_eq!(second.cached, (2, 2), "warm");
        assert_eq!(first.bytes, second.bytes, "same bundle either way");
        assert!(
            root.join(".htl/cache").is_dir(),
            "the store is beside htl.toml"
        );

        write(
            &root.join("src/main.tl"),
            "local util = require(\"util\")\nprint(util.twice(42))\n",
        );
        let edited = resolve_bundle(&root, "src/main.tl", &opts).expect("links");
        assert_eq!(edited.cached, (1, 2), "util replays, main does not");

        // `include_tl!` on the entry alone: generated on the first expansion (the bundle
        // above keyed it by the same path and the same lints, so it is warm already), and
        // replayed once it is stored.
        let inc = resolve_include(&root, "src/main.tl", false).expect("checks");
        assert!(
            inc.cached,
            "the bundle's entry for main serves include_tl! too"
        );
        write(
            &root.join("src/main.tl"),
            "local util = require(\"util\")\nprint(util.twice(7))\n",
        );
        let inc = resolve_include(&root, "src/main.tl", false).expect("checks");
        assert!(!inc.cached, "edited: generated");
        let inc = resolve_include(&root, "src/main.tl", false).expect("checks");
        assert!(inc.cached, "and stored");
    }

    /// No `htl.toml`: the crate has not opted into the layout, and the macro leaves no
    /// `.htl/` behind. Under `target/`: the copy `cargo publish` verifies, where a new
    /// file would abort the publish. Both generate everything and write nothing.
    #[test]
    fn macros_leave_no_store_without_a_project_or_under_target() {
        let opts = htl_core::link::LinkOptions::default();
        let sources = |root: &Path| {
            write(
                &root.join("src/main.tl"),
                "local util = require(\"util\")\nprint(util.twice(21))\n",
            );
            write(
                &root.join("src/util.tl"),
                "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
            );
        };

        let bare = scratch("nocfg");
        sources(&bare);
        for _ in 0..2 {
            let out = resolve_bundle(&bare, "src/main.tl", &opts).expect("links");
            assert_eq!(out.cached, (0, 2), "nothing replays without a project");
        }
        assert!(!bare.join(".htl").exists(), "no htl.toml, no store");

        let published = scratch("publish").join("target/package/host-0.1.0");
        sources(&published);
        write(&published.join("htl.toml"), "[check]\n");
        for _ in 0..2 {
            let out = resolve_bundle(&published, "src/main.tl", &opts).expect("links");
            assert_eq!(out.cached, (0, 2), "nothing replays under target/");
        }
        assert!(
            !published.join(".htl").exists(),
            "cargo publish's verify copy is left exactly as it was"
        );
    }

    /// `include_bundle!`: the closure is linked, host modules are recorded, every input
    /// is tracked, and a type error anywhere in the closure is a compile error.
    #[test]
    fn bundle_links_closure_tracks_inputs_and_fails_on_type_errors() {
        let root = scratch("bundle");
        write(
            &root.join("src/main.tl"),
            "local util = require(\"util\")\nlocal host = require(\"host\")\nprint(util.twice(host.base()))\n",
        );
        write(
            &root.join("src/util.tl"),
            "local record util\nend\nfunction util.twice(n: integer): integer\n   return n * 2\nend\nreturn util\n",
        );
        write(
            &root.join("src/host.d.tl"),
            "local record host\n   base: function(): integer\nend\nreturn host\n",
        );
        write(&root.join("htl.toml"), "[build]\nextra = [\"plugin\"]\n");
        write(&root.join("src/plugin.tl"), "return { plugged = true }\n");

        let out = resolve_bundle(
            &root,
            "src/main.tl",
            &htl_core::link::LinkOptions::default(),
        )
        .expect("links");
        let b = htl_core::bundle::Bundle::decode(&out.bytes).unwrap();
        let names: Vec<&str> = b.modules.iter().map(|m| m.name.as_str()).collect();
        assert!(
            names.contains(&"main") && names.contains(&"util") && names.contains(&"plugin"),
            "{names:?}"
        );
        assert_eq!(b.host_modules, vec!["host".to_string()]);
        let has = |s: &str| out.inputs.iter().any(|p| p.ends_with(s));
        assert!(
            has("src/main.tl")
                && has("src/util.tl")
                && has("src/host.d.tl")
                && has("src/plugin.tl"),
            "{:?}",
            out.inputs
        );

        let src_opts = htl_core::link::LinkOptions {
            source: true,
            ..Default::default()
        };
        let out = resolve_bundle(&root, "src/main.tl", &src_opts).expect("links as source");
        let b = htl_core::bundle::Bundle::decode(&out.bytes).unwrap();
        assert!(
            b.modules
                .iter()
                .all(|m| m.kind == htl_core::bundle::Kind::Source)
        );

        write(
            &root.join("src/util.tl"),
            "local record util\nend\nfunction util.twice(n: integer): integer\n   return \"no\"\nend\nreturn util\n",
        );
        let err = resolve_bundle(
            &root,
            "src/main.tl",
            &htl_core::link::LinkOptions::default(),
        )
        .unwrap_err();
        assert!(
            err.contains("util.tl") && err.contains("expected integer"),
            "{err}"
        );
    }

    /// `include_tl!`: the script checks, but a module it requires does not. The macro and
    /// the CLI share the checker, and the build is where the error belongs — the generated
    /// Lua would have raised at the module's first `require`.
    #[test]
    fn include_fails_on_a_dependency_type_error() {
        let root = scratch("deperr");
        write(
            &root.join("mlua-pkg.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\n\n[deps]\n",
        );
        write(
            &root.join(".htl/modules/entries/mathx/init.tl"),
            "local record mathx\nend\nfunction mathx.twice(n: number): number\n   local s: number = \"no\"\n   return n * 2 + s\nend\nreturn mathx\n",
        );
        write(
            &root.join("scripts/main.tl"),
            "local mathx = require(\"mathx\")\nprint(mathx.twice(21))\n",
        );
        let err = resolve_include(&root, "scripts/main.tl", false).unwrap_err();
        assert!(
            err.contains("required module") && err.contains("mathx/init.tl:4:"),
            "{err}"
        );
        assert!(err.contains("required by"), "{err}");
    }

    /// Without a project the same script fails: the dep is genuinely not on the path.
    #[test]
    fn include_without_project_does_not_see_vendored_dir() {
        let root = scratch("noproject");
        write(
            &root.join(".htl/modules/entries/mathx/init.tl"),
            "return {}\n",
        );
        write(
            &root.join("scripts/main.tl"),
            "local mathx = require(\"mathx\")\nprint(mathx)\n",
        );
        let err = resolve_include(&root, "scripts/main.tl", false).unwrap_err();
        assert!(err.contains("module not found: 'mathx'"), "{err}");
    }
}
