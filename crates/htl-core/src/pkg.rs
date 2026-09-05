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
        }
    }

    pub fn with_module_separator(mut self, sep: char) -> Self {
        self.module_separator = sep;
        self
    }

    fn prelude(lua: &Lua) -> mlua::Result<Table> {
        lua.named_registry_value::<Table>(PRELUDE_REGISTRY_KEY)
            .map_err(|_| mlua::Error::external(
                "htl::pkg::TealResolver: this Lua has no htl prelude (create it with Htl::new / Htl::from_lua)",
            ))
    }

    /// The checker resolves `require`s inside `.tl` via `package.path`; make sure the
    /// root is visible there (once).
    fn ensure_checker_path(&self, lua: &Lua, h: &Table) -> mlua::Result<()> {
        if self.path_added.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        if let Some(root) = &self.root {
            let f: Function = h.get("add_path")?;
            f.call::<()>(root.to_string_lossy().as_ref())?;
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

impl crate::Htl {
    /// Make the project's vendored deps visible to the Teal checker and to the
    /// prelude's strict searcher (`htl run` / `htl test` without a Registry).
    pub fn apply_project(&self, p: &Project) -> anyhow::Result<()> {
        let _ = std::fs::create_dir_all(&p.vendored);
        self.add_path(&p.vendored)?;
        for d in &p.target_dirs {
            self.add_path(d)?;
        }
        Ok(())
    }
}

/// Error raised when a `.tl` module fails the type check at `require` time.
#[derive(Debug)]
pub enum TealResolveError {
    TypeCheck { module: String, errors: Vec<String> },
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
            Self::Read { module, source } => write!(f, "reading module '{module}': {source}"),
        }
    }
}

impl std::error::Error for TealResolveError {}

impl Resolver for TealResolver {
    fn resolve(&self, lua: &Lua, name: &str) -> Option<mlua::Result<Value>> {
        let relative = name.replace(self.module_separator, "/");
        let candidates = [
            (format!("{relative}.tl"), false),
            (format!("{relative}/init.tl"), false),
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
                        // Declaration-only module: nothing to run, give require a table.
                        return Some(lua.create_table().map(Value::Table));
                    }
                    return Some(self.load_teal(lua, &h, &file.content, &file.resolved_path, name));
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
