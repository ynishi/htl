//! mlua-pkg integration: a [`TealResolver`] that serves `.tl` modules through
//! mlua-pkg's `Registry`, so Teal sources sit in the same resolution chain as
//! Rust-native modules, embedded Lua, vendored git deps and assets.
//!
//! ```text
//! require("name")
//!   Registry
//!    ├─ NativeResolver   host_module userdata / Rust tables
//!    ├─ TealResolver     name -> name.tl | name/init.tl  (check + gen + load)
//!    │                   name -> name.d.tl              (type-only: empty table)
//!    ├─ VendoredResolver mlua-pkg.toml git deps
//!    └─ FsResolver       plain .lua
//! ```
//!
//! The resolver must run on a `Lua` that an [`Htl`](crate::Htl) was attached to
//! (`Htl::new` / `Htl::from_lua`); it finds the compiler through the Lua registry.
//! Type errors are returned as `Some(Err)` so, per mlua-pkg's contract, a broken
//! `.tl` never silently falls through to a later resolver.

use crate::PRELUDE_REGISTRY_KEY;
use mlua::{Function, Lua, Table, Value};
use mlua_pkg::Resolver;
use mlua_pkg::sandbox::{FsSandbox, InitError, ReadError, SandboxedFs, SymlinkAwareSandbox};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use mlua_pkg;

/// Resolves `require("a.b")` to `a/b.tl`, `a/b/init.tl`, or `a/b.d.tl` under a
/// sandboxed root, type-checking and generating on the fly.
pub struct TealResolver {
    sandbox: Box<dyn SandboxedFs>,
    root: Option<PathBuf>,
    path_added: AtomicBool,
    module_separator: char,
    /// `"defs.Mod"`: every module served by this resolver must be assignable to that type.
    expect_type: Option<String>,
    /// With `expect_type`: every declared field of the record must be non-nil at run time.
    require_fields: bool,
    /// Extra dirs the checker may search for `require`s (e.g. where `defs.tl` lives).
    checker_paths: Vec<PathBuf>,
    /// Module names served here that `expect_type` / `require_fields` skip.
    exclude: Vec<String>,
    /// When set, `expect_type` / `require_fields` apply to this module name only.
    only_module: Option<String>,
}

impl TealResolver {
    /// Strict sandbox (no symlinks out of `root`).
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, InitError> {
        let root = root.into();
        Ok(Self {
            sandbox: Box::new(FsSandbox::new(&root)?),
            root: Some(root),
            path_added: AtomicBool::new(false),
            module_separator: '.',
            expect_type: None,
            require_fields: false,
            checker_paths: Vec::new(),
            exclude: Vec::new(),
            only_module: None,
        })
    }

    /// Sandbox that follows symlinks directly under `root` (linked package roots).
    pub fn new_symlink_aware(root: impl Into<PathBuf>) -> Result<Self, InitError> {
        let root = root.into();
        Ok(Self {
            sandbox: Box::new(SymlinkAwareSandbox::new(&root)?),
            root: Some(root),
            path_added: AtomicBool::new(false),
            module_separator: '.',
            expect_type: None,
            require_fields: false,
            checker_paths: Vec::new(),
            exclude: Vec::new(),
            only_module: None,
        })
    }

    /// Custom sandbox. Pass `root` so the Teal checker can also see the tree when
    /// resolving `require`s inside `.tl` files (it searches `package.path`).
    pub fn with_sandbox(sandbox: impl SandboxedFs + 'static, root: Option<PathBuf>) -> Self {
        Self {
            sandbox: Box::new(sandbox),
            root,
            path_added: AtomicBool::new(false),
            module_separator: '.',
            expect_type: None,
            require_fields: false,
            checker_paths: Vec::new(),
            exclude: Vec::new(),
            only_module: None,
        }
    }

    pub fn with_module_separator(mut self, sep: char) -> Self {
        self.module_separator = sep;
        self
    }

    /// Require every `.tl` module served here to be assignable to `type_path`, written
    /// as `"<module>.<Type>"` (e.g. `"defs.Mod"`, where `defs.tl` / `defs.d.tl` declares
    /// `Mod`). A module that does not satisfy it fails at `require` time even if it never
    /// annotates its own return value.
    ///
    /// What this catches is what Teal's record assignability catches: a field of the
    /// **wrong type** (`hp = "lots"` for `hp: integer`). On its own it does **not** catch
    /// a **missing** field: every Teal record field is nilable, so `{ name = "x" }`
    /// satisfies `Mod` with `monsters` absent. Add [`require_fields`](Self::require_fields)
    /// to reject that at run time, or nil-guard optional data on the host side.
    pub fn expect_type(mut self, type_path: impl Into<String>) -> Self {
        self.expect_type = Some(type_path.into());
        self
    }

    /// With [`expect_type`](Self::expect_type): after the type check, every field the
    /// record declares must be present (non-nil) in the loaded module, or the `require`
    /// fails naming the missing fields. Use for contracts where every field is mandatory;
    /// contracts with optional fields should keep the default and nil-guard instead.
    pub fn require_fields(mut self) -> Self {
        self.require_fields = true;
        self
    }

    /// Let the Teal checker also search `dir` when resolving `require`s inside served
    /// modules (and the module named by `expect_type`). The sandbox root is always
    /// searched; add the project `src/` here when `defs.tl` lives there.
    pub fn with_checker_path(mut self, dir: impl Into<PathBuf>) -> Self {
        self.checker_paths.push(dir.into());
        self
    }

    /// Modules (by `require` name) served here that are *not* held to `expect_type` /
    /// `require_fields`: an SDK the host writes into the same dir, for instance. The
    /// module that declares the expected type is always exempt.
    pub fn exclude_modules(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.exclude.extend(names.into_iter().map(Into::into));
        self
    }

    /// Hold only this module name to `expect_type` / `require_fields`; everything else
    /// served here is type-checked as usual but not against the contract.
    pub fn only_module(mut self, name: impl Into<String>) -> Self {
        self.only_module = Some(name.into());
        self
    }

    /// Does the contract (`expect_type` / `require_fields`) apply to `name`?
    fn held(&self, name: &str) -> bool {
        if self.expect_type.is_none() || self.exclude.iter().any(|e| e == name) {
            return false;
        }
        self.only_module.as_deref().is_none_or(|m| m == name)
    }

    /// Resolvers for one `[[contract]]` of `htl.toml`: one per concrete contract dir
    /// (a `dir` with `*` expands to every subdirectory), each with `expect_type(type)`
    /// (and `require_fields()` when set), the contract's `exclude` / `module`, and
    /// `root` + `root/src` visible to the checker. `root` is the directory holding
    /// `htl.toml`. The `contract-unenforced` lint of `htl check` recognises this call.
    pub fn for_contract(root: &Path, c: &crate::config::Contract) -> Result<Vec<Self>, InitError> {
        c.dirs(root).into_iter().map(|d| Self::for_contract_dir(root, &d, c)).collect()
    }

    /// One resolver for the concrete contract directory `dir` (see [`for_contract`](Self::for_contract)).
    pub fn for_contract_dir(root: &Path, dir: &Path, c: &crate::config::Contract) -> Result<Self, InitError> {
        let mut r = Self::new_symlink_aware(dir)?
            .expect_type(c.type_path.clone())
            .exclude_modules(c.exclude.iter().cloned())
            .with_checker_path(root)
            .with_checker_path(root.join("src"));
        if let Some(m) = &c.module {
            r = r.only_module(m.clone());
        }
        if c.require_fields {
            r = r.require_fields();
        }
        Ok(r)
    }

    /// Declared fields of the expected record that are nil in `value`.
    fn missing_fields(&self, h: &Table, value: &Value) -> mlua::Result<Vec<String>> {
        let (Some(tp), true) = (&self.expect_type, self.require_fields) else { return Ok(Vec::new()) };
        let f: Function = h.get("record_fields")?;
        let names: Option<Vec<String>> = f
            .call::<Option<Table>>(tp.as_str())?
            .map(|t| t.sequence_values::<String>().collect::<mlua::Result<_>>())
            .transpose()?;
        let Some(names) = names else {
            return Err(mlua::Error::external(format!(
                "TealResolver::require_fields: record type {tp:?} not found by the checker"
            )));
        };
        let Value::Table(t) = value else {
            return Ok(names); // not a table at all: everything is missing
        };
        let mut missing = Vec::new();
        for n in names {
            if matches!(t.get::<Value>(n.as_str())?, Value::Nil) {
                missing.push(n);
            }
        }
        Ok(missing)
    }

    /// Check `local m: <T> = require("<name>")` against the checker; `None` when it holds.
    fn expectation_errors(&self, h: &Table, name: &str) -> mlua::Result<Option<Vec<String>>> {
        let Some(tp) = &self.expect_type else { return Ok(None) };
        let (module, _) = tp.split_once('.').ok_or_else(|| {
            mlua::Error::external(format!(
                "TealResolver::expect_type: expected \"<module>.<Type>\", got {tp:?}"
            ))
        })?;
        // The module that declares the type is not itself held to it.
        if name == module {
            return Ok(None);
        }
        let stub = format!(
            "local {module} = require(\"{module}\")\nlocal m: {tp} = require(\"{name}\")\nreturn m\n"
        );
        // Fresh checker env per stub: several resolvers may serve a module of the same
        // name (one per contract dir) and must not share a cached type for it.
        let check: Function = h.get("check_stub")?;
        let errors: Table = check.call((stub.as_str(), format!("<expect {tp} for module '{name}'>")))?;
        let msgs: Vec<String> = errors.sequence_values::<String>().collect::<mlua::Result<_>>()?;
        Ok(if msgs.is_empty() { None } else { Some(msgs) })
    }

    fn prelude(lua: &Lua) -> mlua::Result<Table> {
        if let Ok(t) = lua.named_registry_value::<Table>(PRELUDE_REGISTRY_KEY) {
            return Ok(t);
        }
        // A runtime state whose checker lives in another Lua (`Htl::with_checker`).
        if let Some(c) = lua.app_data_ref::<crate::CheckerHandle>() {
            return Ok(c.0.clone());
        }
        Err(mlua::Error::external(
            "htl::pkg::TealResolver: this Lua has no htl prelude (create it with Htl::new / Htl::from_lua)",
        ))
    }

    /// The checker resolves `require`s inside `.tl` via `package.path`; make sure the
    /// root is visible there (once).
    fn ensure_checker_path(&self, lua: &Lua, h: &Table) -> mlua::Result<()> {
        if self.path_added.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        let f: Function = h.get("add_path")?;
        if let Some(root) = &self.root {
            f.call::<()>(root.to_string_lossy().as_ref())?;
        }
        for p in &self.checker_paths {
            if p.is_dir() {
                f.call::<()>(p.to_string_lossy().as_ref())?;
            }
        }
        let _ = lua;
        Ok(())
    }

    fn has_lua_sibling(&self, relative: &str) -> bool {
        for cand in [format!("{relative}.lua"), format!("{relative}/init.lua")] {
            if let Ok(Some(_)) = self.sandbox.read(Path::new(&cand)) {
                return true;
            }
        }
        false
    }

    fn load_teal(&self, lua: &Lua, h: &Table, src: &str, resolved: &Path, name: &str) -> mlua::Result<Value> {
        let gen_fn: Function = h.get("gen_string")?;
        let (code, info): (Option<String>, Table) = gen_fn.call((src, resolved.to_string_lossy().as_ref()))?;
        let Some(code) = code else {
            let errors: Table = info.get("errors")?;
            let msgs: Vec<String> = errors.sequence_values::<String>().collect::<mlua::Result<_>>()?;
            return Err(mlua::Error::external(TealResolveError::TypeCheck {
                module: name.to_string(),
                errors: msgs,
            }));
        };
        if self.held(name)
            && let Some(errs) = self.expectation_errors(h, name)?
        {
            return Err(mlua::Error::external(TealResolveError::Expectation {
                module: name.to_string(),
                expected: self.expect_type.clone().unwrap_or_default(),
                errors: errs,
            }));
        }
        let chunk = lua
            .load(code)
            .set_name(format!("@{}", resolved.display()))
            .into_function()?;
        chunk.call::<Value>((name, resolved.to_string_lossy().as_ref()))
    }
}

// ---------------------------------------------------------------- Project (mlua-pkg.toml)

/// An `mlua-pkg.toml` project: where the manifest, lockfile and vendored deps live.
///
/// The pkgs dir follows mlua-pkg's own rule, evaluated against the manifest's
/// directory: `MLUA_PKG_DIR` env > `<root>/target/mlua-pkgs` when `<root>/target`
/// exists > `<root>/.mlua-pkgs`.
#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub lockfile: PathBuf,
    pub pkgs_dir: PathBuf,
    pub vendored: PathBuf,
    /// Parent directories of `target_dir` deps (physically vendored copies declared in
    /// the manifest, e.g. `target_dir = "lua/lshape"` -> `<root>/lua`), so
    /// `require("lshape")` resolves to `<root>/lua/lshape/init.*` like a vendored dep.
    pub target_dirs: Vec<PathBuf>,
}

pub const MANIFEST_NAME: &str = "mlua-pkg.toml";
pub const LOCKFILE_NAME: &str = "mlua-pkg.lock";

impl Project {
    /// Walk up from `start` (a file or directory) looking for `mlua-pkg.toml`.
    pub fn find(start: &Path) -> Option<Self> {
        let mut dir = if start.is_dir() { start.to_path_buf() } else { crate::parent_dir(start) };
        if let Ok(abs) = std::fs::canonicalize(&dir) {
            dir = abs;
        }
        loop {
            let manifest = dir.join(MANIFEST_NAME);
            if manifest.is_file() {
                return Some(Self::at(&dir));
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    /// Project rooted at `root` (must contain `mlua-pkg.toml`; not checked here).
    pub fn at(root: &Path) -> Self {
        let pkgs_dir = match std::env::var("MLUA_PKG_DIR") {
            Ok(p) if !p.is_empty() => PathBuf::from(p),
            _ if root.join("target").is_dir() => root.join("target").join("mlua-pkgs"),
            _ => root.join(".mlua-pkgs"),
        };
        let manifest = root.join(MANIFEST_NAME);
        // `target_dir` deps: collect the parent of each declared copy target. A manifest
        // that fails to parse contributes nothing here (mlua-pkg itself reports it).
        let mut target_dirs: Vec<PathBuf> = Vec::new();
        if let Ok(m) = mlua_pkg::manifest::Manifest::from_path(&manifest) {
            for dep in m.deps.values() {
                if let Some(td) = &dep.target_dir {
                    let abs = root.join(td);
                    let parent = abs.parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
                    if !target_dirs.contains(&parent) {
                        target_dirs.push(parent);
                    }
                }
            }
        }
        Self {
            root: root.to_path_buf(),
            manifest,
            lockfile: root.join(LOCKFILE_NAME),
            vendored: pkgs_dir.join("vendored"),
            pkgs_dir,
            target_dirs,
        }
    }

    /// `true` once `mlua-pkg install` has produced the lockfile.
    pub fn installed(&self) -> bool {
        self.lockfile.is_file()
    }

    /// Resolver for `.tl` / `.d.tl` inside vendored deps (symlink-aware, like
    /// `VendoredResolver`). Creates the vendored dir if it does not exist yet.
    pub fn teal_resolver(&self) -> Result<TealResolver, InitError> {
        let _ = std::fs::create_dir_all(&self.vendored);
        TealResolver::new_symlink_aware(&self.vendored)
    }

    /// mlua-pkg's own resolver for plain `.lua` inside vendored deps.
    pub fn vendored_resolver(&self) -> anyhow::Result<mlua_pkg::resolvers::VendoredResolver> {
        if self.installed() {
            Ok(mlua_pkg::resolvers::VendoredResolver::from_lockfile(&self.lockfile, &self.vendored)?)
        } else {
            let _ = std::fs::create_dir_all(&self.vendored);
            Ok(mlua_pkg::resolvers::VendoredResolver::new(&self.vendored)?)
        }
    }

    /// Registry with the project's deps: Teal first, then plain Lua. Add your
    /// `NativeResolver`s *before* calling `install` if Teal code declares them in `.d.tl`.
    pub fn registry(&self) -> anyhow::Result<mlua_pkg::Registry> {
        let mut reg = mlua_pkg::Registry::new();
        reg.add(self.teal_resolver()?);
        reg.add(self.vendored_resolver()?);
        for d in &self.target_dirs {
            if d.is_dir() {
                reg.add(TealResolver::new(d)?);
                reg.add(mlua_pkg::resolvers::FsResolver::new(d)?);
            }
        }
        Ok(reg)
    }
}

/// One [`TealResolver`] per `[[contract]]` in `htl.toml`, in declaration order, so the
/// host and `htl check` enforce the same contracts from the same source. `root` is the
/// directory holding `htl.toml` (the path [`HtlConfig::find`](crate::config::HtlConfig::find)
/// returns, minus the file name). Add them to a `Registry` before the plain resolvers.
pub fn contract_resolvers(root: &Path, cfg: &crate::config::HtlConfig) -> Result<Vec<TealResolver>, InitError> {
    let mut out = Vec::new();
    for c in &cfg.contract {
        for mut r in TealResolver::for_contract(root, c)? {
            for p in cfg.search_paths(root) {
                r = r.with_checker_path(p);
            }
            out.push(r);
        }
    }
    Ok(out)
}

impl crate::Htl {
    /// Make the project's vendored deps visible to the Teal checker and to the
    /// prelude's strict searcher (`htl run` / `htl test` without a Registry).
    pub fn apply_project(&self, p: &Project) -> anyhow::Result<()> {
        let _ = std::fs::create_dir_all(&p.vendored);
        self.add_path(&p.vendored)?;
        for d in &p.target_dirs {
            self.add_path(d)?;
        }
        // The project's own modules: `<root>/src` (the scaffold layout) so a script anywhere
        // in the project resolves them the same way `tests/` does.
        let src = p.root.join("src");
        if src.is_dir() {
            self.add_path(&src)?;
        }
        Ok(())
    }
}

/// Error raised when a `.tl` module fails the type check at `require` time.
#[derive(Debug)]
pub enum TealResolveError {
    TypeCheck { module: String, errors: Vec<String> },
    /// The module type-checks on its own but is not assignable to the resolver's
    /// [`expect_type`](TealResolver::expect_type).
    Expectation { module: String, expected: String, errors: Vec<String> },
    /// [`require_fields`](TealResolver::require_fields): declared fields absent at run time.
    MissingFields { module: String, expected: String, fields: Vec<String> },
    Read { module: String, source: ReadError },
}

impl std::fmt::Display for TealResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TypeCheck { module, errors } => {
                write!(f, "Teal type check failed for module '{module}':")?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                Ok(())
            }
            Self::Expectation { module, expected, errors } => {
                write!(f, "module '{module}' does not satisfy {expected}:")?;
                for e in errors {
                    write!(f, "\n  {e}")?;
                }
                write!(
                    f,
                    "\n  hint: annotate the returned table in the module (`local m: {expected} = {{ ... }}  return m`) \
                     to get field-level errors with line numbers"
                )
            }
            Self::MissingFields { module, expected, fields } => write!(
                f,
                "module '{module}' is missing required field(s) of {expected}: {} (every field of that record must be non-nil)",
                fields.join(", ")
            ),
            Self::Read { module, source } => write!(f, "reading module '{module}': {source}"),
        }
    }
}

impl std::error::Error for TealResolveError {}

/// Is `name` registered in `package.preload` (host-provided implementation)?
fn preloaded(lua: &Lua, name: &str) -> mlua::Result<bool> {
    let package: Table = lua.globals().get("package")?;
    let preload: Table = package.get("preload")?;
    Ok(!matches!(preload.get::<Value>(name)?, Value::Nil))
}

impl Resolver for TealResolver {
    fn resolve(&self, lua: &Lua, name: &str) -> Option<mlua::Result<Value>> {
        let relative = name.replace(self.module_separator, "/");
        // Flat packages: `<name>/<name>.tl` stands in for `<name>/init.tl`.
        let last = relative.rsplit('/').next().unwrap_or(&relative).to_string();
        let candidates = [
            (format!("{relative}.tl"), false),
            (format!("{relative}/init.tl"), false),
            (format!("{relative}/{last}.tl"), false),
            (format!("{relative}.d.tl"), true),
        ];
        let h = match Self::prelude(lua) {
            Ok(h) => h,
            Err(e) => return Some(Err(e)),
        };
        if let Err(e) = self.ensure_checker_path(lua, &h) {
            return Some(Err(e));
        }
        for (candidate, type_only) in &candidates {
            match self.sandbox.read(Path::new(candidate)) {
                Ok(Some(file)) => {
                    if *type_only {
                        // A `.d.tl` may describe a plain `.lua` served by a later resolver
                        // (FsResolver / VendoredResolver): step aside if one is present.
                        // Native modules must be registered *before* this resolver.
                        if self.has_lua_sibling(&relative) {
                            return None;
                        }
                        // ... or that the host registered in `package.preload` (a Rust
                        // `#[host_module]`, `Htl::preload_value`). The Registry's searcher
                        // runs *before* Lua's preload searcher, so this is the only chance.
                        match preloaded(lua, name) {
                            Ok(true) => return None,
                            Ok(false) => {}
                            Err(e) => return Some(Err(e)),
                        }
                        // Declaration-only module: nothing to run. Hand require a table whose
                        // lookups explain that the implementation lives elsewhere.
                        return Some(
                            h.get::<Function>("type_only_module")
                                .and_then(|f| f.call::<Value>((name, file.resolved_path.to_string_lossy().as_ref()))),
                        );
                    }
                    let loaded = match self.load_teal(lua, &h, &file.content, &file.resolved_path, name) {
                        Ok(v) => v,
                        Err(e) => return Some(Err(e)),
                    };
                    if !self.held(name) {
                        return Some(Ok(loaded));
                    }
                    match self.missing_fields(&h, &loaded) {
                        Ok(m) if m.is_empty() => return Some(Ok(loaded)),
                        Ok(missing) => {
                            return Some(Err(mlua::Error::external(TealResolveError::MissingFields {
                                module: name.to_string(),
                                expected: self.expect_type.clone().unwrap_or_default(),
                                fields: missing,
                            })));
                        }
                        Err(e) => return Some(Err(e)),
                    }
                }
                Ok(None) => continue,
                Err(source) => {
                    return Some(Err(mlua::Error::external(TealResolveError::Read {
                        module: name.to_string(),
                        source,
                    })));
                }
            }
        }
        None
    }
}
