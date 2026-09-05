//! htl: Teal, hidden.
//!
//! Embeds the Teal compiler (`tl.lua`) into an mlua state so `.tl` sources can be
//! type-checked, generated and executed without any external toolchain.
//!
//! - [`Htl::check`] / [`Htl::gen`]: type-check and generate Lua from a `.tl` file
//! - [`Htl::install_searcher`]: strict `require` for `.tl` (type errors abort the require)
//! - [`Htl::preload`]: register generated Lua (e.g. from `include_tl!`) under a module name
//! - [`bundle`]: stripped-bytecode bundles produced by `htl build`

pub use mlua;

use anyhow::{Context, Result, anyhow, bail};
use mlua::chunk::ChunkMode;
use mlua::{Function, Lua, Table, Value, Variadic};
use std::path::{Path, PathBuf};

pub mod bundle;
#[cfg(feature = "dts")]
pub mod dts;
#[cfg(feature = "pkg")]
pub mod pkg;
pub mod teal;
pub mod testing;

/// Registry key under which the prelude table is stored (lets `pkg::TealResolver`
/// reach the compiler from a bare `&Lua`).
pub(crate) const PRELUDE_REGISTRY_KEY: &str = "htl.prelude";

const TL_SRC: &str = include_str!("../vendor/tl.lua");
const LINT_SRC: &str = include_str!("lint.lua");
const FMT_SRC: &str = include_str!("fmt.lua");
const PRELUDE: &str = include_str!("prelude.lua");

/// Teal version vendored into this crate.
pub const TEAL_VERSION: &str = "0.24.8";

/// Result of type-checking one `.tl` file.
#[derive(Debug, Clone, Default)]
pub struct CheckInfo {
    /// `file:line:col: message` for syntax and type errors.
    pub errors: Vec<String>,
    /// `file:line:col: message` for warnings (non-fatal).
    pub warnings: Vec<String>,
    /// Files pulled in via `require` during checking (`.tl` / `.d.tl` / `.lua`).
    pub deps: Vec<PathBuf>,
    /// htl lint findings (`nil-index`, `enum-exhaustive`). Advisory unless the caller
    /// promotes them (`htl check --strict`, `include_tl!`).
    pub lints: Vec<String>,
}

impl CheckInfo {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }

    /// `true` when there are no errors, warnings or lints.
    pub fn clean(&self) -> bool {
        self.errors.is_empty() && self.warnings.is_empty() && self.lints.is_empty()
    }
}

/// An mlua state with the Teal compiler loaded.
pub struct Htl {
    lua: Lua,
    h: Table,
}

impl Htl {
    /// New state. Uses `Lua::unsafe_new` so stripped bytecode bundles can be loaded.
    pub fn new() -> Result<Self> {
        // SAFETY: we accept binary chunks only from bundles we produced ourselves.
        let lua = unsafe { Lua::unsafe_new() };
        Self::from_lua(lua)
    }

    /// Attach the Teal compiler to an existing Lua state (the host's own `Lua`).
    pub fn from_lua(lua: Lua) -> Result<Self> {
        let tl_loader: Function = lua
            .load(TL_SRC)
            .set_name("=tl.lua")
            .into_function()
            .context("compiling vendored tl.lua")?;
        let lint_loader: Function = lua
            .load(LINT_SRC)
            .set_name("=htl-lint")
            .into_function()
            .context("compiling htl lint.lua")?;
        let package: Table = lua.globals().get("package")?;
        let preload: Table = package.get("preload")?;
        let fmt_loader: Function = lua
            .load(FMT_SRC)
            .set_name("=htl-fmt")
            .into_function()
            .context("compiling htl fmt.lua")?;
        preload.set("tl", tl_loader)?;
        preload.set("htl.lint", lint_loader)?;
        preload.set("htl.fmt", fmt_loader)?;
        let h: Table = lua
            .load(PRELUDE)
            .set_name("=htl-prelude")
            .eval()
            .context("loading htl prelude")?;
        lua.set_named_registry_value(PRELUDE_REGISTRY_KEY, h.clone())?;
        Ok(Self { lua, h })
    }

    pub fn lua(&self) -> &Lua {
        &self.lua
    }

    /// Type-check one file.
    pub fn check(&self, file: &Path) -> Result<CheckInfo> {
        let f: Function = self.h.get("check")?;
        let t: Table = f.call(path_str(file))?;
        read_checkinfo(&t)
    }

    /// Type-check and generate Lua source. `None` code means errors (see `CheckInfo`).
    pub fn gen_lua(&self, file: &Path) -> Result<(Option<String>, CheckInfo)> {
        let f: Function = self.h.get("gen")?;
        let (code, t): (Option<String>, Table) = f.call(path_str(file))?;
        Ok((code, read_checkinfo(&t)?))
    }

    /// Configure lint rules: `"+no-any,-shadow-local"` on top of the defaults.
    pub fn configure_lints(&self, spec: &str) -> Result<()> {
        let f: Function = self.h.get("set_lints")?;
        let (ok, err): (Option<bool>, Option<String>) = f.call(spec)?;
        if ok.unwrap_or(false) {
            Ok(())
        } else {
            bail!("{}", err.unwrap_or_else(|| "invalid lint spec".into()))
        }
    }

    /// Names of all lint rules (enabled or not).
    pub fn lint_rules(&self) -> Result<Vec<String>> {
        let f: Function = self.h.get("lint_rules")?;
        let t: Table = f.call(())?;
        Ok(t.sequence_values::<String>().collect::<mlua::Result<_>>()?)
    }

    /// Format a `.tl` file (whitespace-only formatter). Returns the formatted text.
    pub fn format_file(&self, file: &Path, indent: usize) -> Result<String> {
        let f: Function = self.h.get("format")?;
        let (out, err): (Option<String>, Option<String>) = f.call((path_str(file), indent))?;
        out.ok_or_else(|| anyhow!("{}", err.unwrap_or_else(|| "format failed".into())))
    }

    /// Drop Lua's default search path (cwd-relative `./?.lua` etc.) so only directories
    /// passed to [`add_path`](Self::add_path) are consulted by the checker and `require`.
    pub fn reset_search_path(&self) -> Result<()> {
        let f: Function = self.h.get("reset_path")?;
        f.call::<()>(())?;
        Ok(())
    }

    /// Prepend `dir/?.tl;dir/?/init.tl` to `package.path` (Teal resolves requires through it).
    pub fn add_path(&self, dir: &Path) -> Result<()> {
        let f: Function = self.h.get("add_path")?;
        f.call::<()>(path_str(dir))?;
        Ok(())
    }

    /// Install the strict `.tl` searcher: `require` of a `.tl` with type errors fails.
    pub fn install_searcher(&self) -> Result<()> {
        let f: Function = self.h.get("install_searcher")?;
        f.call::<()>(())?;
        Ok(())
    }

    /// Register generated Lua source under a module name (`package.preload`).
    pub fn preload(&self, name: &str, lua_src: &str) -> Result<()> {
        let loader = self
            .lua
            .load(lua_src)
            .set_name(format!("={name}"))
            .into_function()
            .with_context(|| format!("compiling preloaded module {name}"))?;
        self.preload_table()?.set(name, loader)?;
        Ok(())
    }

    /// Register stripped bytecode (e.g. from `include_tl_bytes!`) under a module name.
    pub fn preload_bytes(&self, name: &str, bytecode: &[u8]) -> Result<()> {
        let loader = self
            .lua
            .load(bytecode)
            .set_name(format!("={name}"))
            .set_mode(ChunkMode::Binary)
            .into_function()
            .with_context(|| format!("loading bytecode for module {name}"))?;
        self.preload_table()?.set(name, loader)?;
        Ok(())
    }

    /// Execute stripped bytecode with `...` = args.
    pub fn exec_bytes(&self, bytecode: &[u8], chunk_name: &str, args: &[String]) -> Result<()> {
        let f = self
            .lua
            .load(bytecode)
            .set_name(chunk_name)
            .set_mode(ChunkMode::Binary)
            .into_function()?;
        let va: Variadic<String> = args.iter().cloned().collect();
        f.call::<()>(va)?;
        Ok(())
    }

    /// Register a ready-made value (typically a Rust-built table) as a module.
    pub fn preload_value(&self, name: &str, value: impl mlua::IntoLua) -> Result<()> {
        let value = value.into_lua(&self.lua)?;
        let loader = self
            .lua
            .create_function(move |_, ()| Ok(value.clone()))?;
        self.preload_table()?.set(name, loader)?;
        Ok(())
    }

    fn preload_table(&self) -> Result<Table> {
        let package: Table = self.lua.globals().get("package")?;
        Ok(package.get("preload")?)
    }

    /// Set the global `arg` table like the `lua` CLI does.
    pub fn set_arg(&self, script: &str, args: &[String]) -> Result<()> {
        let t = self.lua.create_table()?;
        t.set(0, script)?;
        for (i, a) in args.iter().enumerate() {
            t.set(i + 1, a.as_str())?;
        }
        self.lua.globals().set("arg", t)?;
        Ok(())
    }

    /// Execute Lua source with `...` = args.
    pub fn exec(&self, lua_src: &str, chunk_name: &str, args: &[String]) -> Result<()> {
        let f = self
            .lua
            .load(lua_src)
            .set_name(chunk_name)
            .into_function()?;
        let va: Variadic<String> = args.iter().cloned().collect();
        f.call::<()>(va)?;
        Ok(())
    }

    /// Check + gen + run a `.tl` script. If the check fails the script is not run and the
    /// returned `CheckInfo` carries the errors. Runtime errors come back as `Err`.
    pub fn run_file(&self, file: &Path, args: &[String]) -> Result<CheckInfo> {
        self.add_path(&parent_dir(file))?;
        self.install_searcher()?;
        self.set_arg(&file.to_string_lossy(), args)?;
        let (code, ci) = self.gen_lua(file)?;
        let Some(code) = code else { return Ok(ci) };
        self.exec(&code, &format!("@{}", file.display()), args)?;
        Ok(ci)
    }

    /// Compile Lua source to stripped bytecode (Lua 5.4 format of this build).
    pub fn compile(&self, name: &str, lua_src: &str) -> Result<Vec<u8>> {
        let f = self
            .lua
            .load(lua_src)
            .set_name(format!("={name}"))
            .into_function()
            .with_context(|| format!("compiling generated Lua for {name}"))?;
        Ok(f.dump(true))
    }

    /// Install a searcher serving modules from a bundle.
    pub fn install_bundle(&self, b: &bundle::Bundle) -> Result<()> {
        let modules = b.modules.clone();
        let searcher = self.lua.create_function(move |lua, name: String| {
            match modules.iter().find(|(n, _)| *n == name) {
                Some((_, bc)) => {
                    let f = lua
                        .load(bc.as_slice())
                        .set_name(format!("={name}"))
                        .set_mode(ChunkMode::Binary)
                        .into_function()?;
                    Ok(Value::Function(f))
                }
                None => Ok(Value::String(
                    lua.create_string(format!("\n\tno bundled module '{name}'"))?,
                )),
            }
        })?;
        let package: Table = self.lua.globals().get("package")?;
        let searchers: Table = package.get("searchers")?;
        searchers.raw_insert(2, searcher)?;
        Ok(())
    }

    /// Install the bundle and run its entry module with `...` = args.
    pub fn run_bundle(&self, b: &bundle::Bundle, args: &[String]) -> Result<()> {
        let entry_bc = b
            .modules
            .iter()
            .find(|(n, _)| *n == b.entry)
            .map(|(_, bc)| bc.clone())
            .ok_or_else(|| anyhow!("entry module '{}' not in bundle", b.entry))?;
        self.install_bundle(b)?;
        self.set_arg(&b.entry, args)?;
        let main: Function = self
            .lua
            .load(entry_bc.as_slice())
            .set_name(format!("={}", b.entry))
            .set_mode(ChunkMode::Binary)
            .into_function()?;
        let va: Variadic<String> = args.iter().cloned().collect();
        main.call::<()>(va)?;
        Ok(())
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Write `text` to `path` only if the content differs. Returns `true` when written.
/// Used by the derive macros to emit `.d.tl` files without churning cargo's fingerprints.
pub fn write_if_changed(path: &Path, text: &str) -> std::io::Result<bool> {
    if let Ok(cur) = std::fs::read_to_string(path)
        && cur == text
    {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)?;
    Ok(true)
}

/// Parent directory of a file, `.` when the path has none.
pub fn parent_dir(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("."));
    if dir.as_os_str().is_empty() { PathBuf::from(".") } else { dir.to_path_buf() }
}

fn read_checkinfo(t: &Table) -> Result<CheckInfo> {
    let seq = |key: &str| -> Result<Vec<String>> {
        let inner: Table = t.get(key)?;
        Ok(inner.sequence_values::<String>().collect::<mlua::Result<_>>()?)
    };
    Ok(CheckInfo {
        errors: seq("errors")?,
        warnings: seq("warnings")?,
        deps: seq("deps")?.into_iter().map(PathBuf::from).collect(),
        lints: seq("lints")?,
    })
}

/// `true` for `foo.tl` but not `foo.d.tl`.
pub fn is_tl_source(p: &Path) -> bool {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    p.is_file() && name.ends_with(".tl") && !name.ends_with(".d.tl")
}

/// Collect `.tl` sources from files and directories (sorted, recursive).
pub fn collect_tl(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            for e in walkdir::WalkDir::new(p).sort_by_file_name() {
                let e = e?;
                if is_tl_source(e.path()) {
                    out.push(e.path().to_path_buf());
                }
            }
        } else if p.is_file() {
            out.push(p.clone());
        } else {
            bail!("no such file or directory: {}", p.display());
        }
    }
    Ok(out)
}

/// `root/foo/bar.tl` -> `foo.bar`, `root/foo/init.tl` -> `foo`.
pub fn module_name(root: &Path, file: &Path) -> Result<String> {
    let rel = file.strip_prefix(root)?.with_extension("");
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.last().map(|s| s == "init").unwrap_or(false) {
        parts.pop();
    }
    if parts.is_empty() {
        bail!("cannot derive module name for {}", file.display());
    }
    Ok(parts.join("."))
}
