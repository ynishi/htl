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
use anyhow::Context;
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
    /// With `expect_type`: which fields must be non-nil at run time. `All(false)` is off.
    require_fields: crate::config::RequireFields,
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
            require_fields: Default::default(),
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
            require_fields: Default::default(),
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
            require_fields: Default::default(),
            checker_paths: Vec::new(),
            exclude: Vec::new(),
            only_module: None,
        }
    }

    /// The character in a module name that stands for a directory boundary. `.` by
    /// default, as `require("a.b")` writes it.
    ///
    /// It is a setting rather than a constant because the name a host registers a module
    /// under is the host's to choose, and one that uses `/` or `::` still has to reach
    /// `a/b.tl` on disk. Only the separator moves: the candidate list built from it
    /// (`.tl`, `/init.tl`, `/<last>.tl`, `.d.tl`) is the same whatever it is.
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

    /// With [`expect_type`](Self::expect_type): after the type check, these fields must
    /// be present (non-nil) in the loaded module, or the `require` fails naming the ones
    /// that are absent.
    ///
    /// Naming them rather than taking all of them is what lets the type grow: the fields
    /// listed here are the contract, and a field added to the record later is optional
    /// until it is added here too. A name the record does not declare is an error at the
    /// first `require`, not a line that quietly does nothing.
    ///
    /// [`require_all_fields`](Self::require_all_fields) is the every-field form, and the
    /// static counterpart of both is `require_fields` in `[[contract]]`.
    pub fn require_fields(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.require_fields =
            crate::config::RequireFields::Named(names.into_iter().map(Into::into).collect());
        self
    }

    /// With [`expect_type`](Self::expect_type): every field the record declares must be
    /// present (non-nil) in the loaded module. Adding a field to the record makes every
    /// module that predates it fail, which is what
    /// [`require_fields`](Self::require_fields) exists to avoid; use this where the type
    /// is settled, or where every field really is mandatory.
    pub fn require_all_fields(mut self) -> Self {
        self.require_fields = crate::config::RequireFields::All(true);
        self
    }

    /// Let the Teal checker also search `dir` when resolving `require`s inside served
    /// modules (and the module named by `expect_type`). The sandbox root is always
    /// searched; add the project `src/` here when `defs.tl` lives there.
    ///
    /// A directory already on the checker's search path is not moved — neither `dir` nor
    /// the root. A host that chains a resolver per directory tier therefore states the
    /// order once, with [`Htl::add_search_paths`](crate::Htl::add_search_paths), before
    /// the first `require`; without that the order is whichever resolver answered first.
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
    /// and the contract's `require_fields` as written, its `exclude` / `module`, and
    /// the project's search paths visible to the checker
    /// ([`search_paths`](crate::config::HtlConfig::search_paths): `root`, its `src/` and
    /// `types/`, then `[check] paths`). `root` is the directory holding `htl.toml`. The
    /// `contract-unenforced` lint of `htl check` recognises this call.
    pub fn for_contract(
        root: &Path,
        cfg: &crate::config::HtlConfig,
        c: &crate::contract::Resolved,
    ) -> Result<Vec<Self>, InitError> {
        c.dirs(root)
            .into_iter()
            .map(|d| Self::for_contract_dir(root, &d, cfg, c))
            .collect()
    }

    /// One resolver for the concrete contract directory `dir` (see [`for_contract`](Self::for_contract)).
    pub fn for_contract_dir(
        root: &Path,
        dir: &Path,
        cfg: &crate::config::HtlConfig,
        c: &crate::contract::Resolved,
    ) -> Result<Self, InitError> {
        let mut r = Self::new_symlink_aware(dir)?
            .expect_type(c.type_path.clone())
            .exclude_modules(c.exclude.iter().cloned());
        // The same paths the `contract` lint checks through (`Htl::apply_config`), so a
        // contract type declared in `types/` resolves in the run as well as in the check.
        for p in cfg.search_paths(root) {
            r = r.with_checker_path(p);
        }
        if let Some(m) = &c.module {
            r = r.only_module(m.clone());
        }
        r.require_fields = c.require_fields.clone();
        Ok(r)
    }

    /// Required fields of the expected record that are nil in `value`.
    fn missing_fields(&self, h: &Table, value: &Value) -> mlua::Result<Vec<String>> {
        let Some(tp) = &self.expect_type else {
            return Ok(Vec::new());
        };
        if !self.require_fields.is_on() {
            return Ok(Vec::new());
        }
        let f: Function = h.get("record_fields")?;
        let declared: Option<Vec<String>> = f
            .call::<Option<Table>>(tp.as_str())?
            .map(|t| t.sequence_values::<String>().collect::<mlua::Result<_>>())
            .transpose()?;
        let Some(declared) = declared else {
            return Err(mlua::Error::external(format!(
                "TealResolver::require_fields: record type {tp:?} not found by the checker"
            )));
        };
        // A listed name the record does not declare is a mistake in the host's own
        // wiring; saying so beats holding modules to a field that cannot exist.
        let names = match self.require_fields.named() {
            None => declared,
            Some(wanted) => {
                let unknown: Vec<&str> = wanted
                    .iter()
                    .filter(|w| !declared.iter().any(|d| d == *w))
                    .map(|w| w.as_str())
                    .collect();
                if !unknown.is_empty() {
                    return Err(mlua::Error::external(format!(
                        "TealResolver::require_fields names field(s) that {tp} does not declare: {}",
                        unknown.join(", ")
                    )));
                }
                wanted.to_vec()
            }
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
        let Some(tp) = &self.expect_type else {
            return Ok(None);
        };
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
        //
        // And the root goes in front of the search path for the length of the check, for
        // the same reason: the stub says `require("<name>")`, so what it types is whatever
        // directory on the path answers first, while what is being served — and what the
        // contract is about — is this resolver's file. With a contract over `sites/*` both
        // dirs are on the path and the one in front would decide, whichever of the two is
        // serving. The path is put back afterwards, so this states nothing about the
        // search order the host chose.
        let check: Function = h.get("check_stub")?;
        let saved: Option<String> = match &self.root {
            Some(root) => {
                let f: Function = h.get("push_path_front")?;
                Some(f.call::<String>(root.to_string_lossy().as_ref())?)
            }
            None => None,
        };
        let errors =
            check.call::<Table>((stub.as_str(), format!("<expect {tp} for module '{name}'>")));
        if let Some(saved) = saved {
            let f: Function = h.get("set_path")?;
            f.call::<()>(saved)?;
        }
        let msgs: Vec<String> = errors?
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()?;
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
        // Back to front: `add_path` prepends, so this leaves the sandbox root consulted
        // first (a module resolving its siblings) and the project's paths behind it, in
        // the order `search_paths` states. Adding them front to back reversed both.
        //
        // A directory already on the path is left where it is (`add_path` is idempotent),
        // so none of this displaces an order the host stated with `add_search_paths` —
        // including the root's place in it. The root goes first only where nobody else
        // put it, which is the single-resolver case the paragraph above describes.
        for p in self.checker_paths.iter().rev() {
            if p.is_dir() {
                f.call::<()>(p.to_string_lossy().as_ref())?;
            }
        }
        if let Some(root) = &self.root {
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

    fn load_teal(
        &self,
        lua: &Lua,
        h: &Table,
        src: &str,
        resolved: &Path,
        name: &str,
    ) -> mlua::Result<Value> {
        let gen_fn: Function = h.get("gen_string")?;
        let (code, info): (Option<String>, Table) =
            gen_fn.call((src, resolved.to_string_lossy().as_ref()))?;
        let Some(code) = code else {
            let errors: Table = info.get("errors")?;
            let msgs: Vec<String> = errors
                .sequence_values::<String>()
                .collect::<mlua::Result<_>>()?;
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

/// An `mlua-pkg.toml` project: where the manifest, lockfile and installed deps live.
///
/// Installed deps go under [`pkgs_dir`] — `<root>/.htl/modules`, beside the check cache
/// and regenerated the same way: from the manifest and the lockfile rather than from the
/// project's own sources. Deps that are *committed* are the other thing, and they are
/// declared: `target_dirs`.
#[derive(Debug, Clone)]
pub struct Project {
    /// The directory holding `mlua-pkg.toml`, and what every other path here is derived
    /// from. Canonicalised when [`Project::find`] walked up to it, so two starting points
    /// under the same project produce the same paths.
    pub root: PathBuf,
    /// `<root>/mlua-pkg.toml`. Recorded even when it does not parse: [`Project::at`] takes
    /// what it can from a broken manifest and leaves the reporting of it to mlua-pkg, so a
    /// project with a syntax error still has a root and a cache directory to name.
    pub manifest: PathBuf,
    /// `<root>/mlua-pkg.lock`. Its presence is the whole of [`installed`](Self::installed):
    /// a lockfile is what `mlua-pkg install` writes last, so a project that has one has
    /// deps to resolve and a project that does not has nothing under [`entries`](Self::entries)
    /// to find.
    pub lockfile: PathBuf,
    /// `<root>/.htl/modules`: everything installed, under the same `.htl` the check cache
    /// lives in, because both are regenerated from the manifest rather than written by
    /// hand and both are what a `.gitignore` excludes in one line.
    pub pkgs_dir: PathBuf,
    /// `pkgs_dir/vendored`: one link per installed dep, pointing at the **package root**
    /// mlua-pkg fetched (or at the patched copy standing in for it). The name is
    /// mlua-pkg's own and describes its layout, not htl's — what is in there is installed
    /// and regenerated, while a copy that is committed to the repo is a `target_dir` dep
    /// below. This is the root a dep publishes beside its code from (`types/`), and not
    /// where `require` looks: that is [`entries`](Self::entries).
    pub vendored: PathBuf,
    /// `pkgs_dir/entries`: one link per installed dep, pointing at the dep's `entry`
    /// directory below the root — `../vendored/<name>/<entry>` — so `require("<name>.x")`
    /// finds `<entry>/x.tl` under it. htl writes these from the lockfile
    /// ([`Project::link_entries`]); mlua-pkg places the root and records the entry, and
    /// applies it by rewriting the module name in its own `.lua` resolver, which a checker
    /// resolving through `package.path` cannot do. A directory of links is the same fact in
    /// the form a path can express, and the one directory the `.tl` side searches.
    pub entries: PathBuf,
    /// Parent directories of `target_dir` deps (physically vendored copies declared in
    /// the manifest, e.g. `target_dir = "lua/lshape"` -> `<root>/lua`), so
    /// `require("lshape")` resolves to `<root>/lua/lshape/init.*` like a vendored dep.
    pub target_dirs: Vec<PathBuf>,
    /// The `target_dir` copies themselves (`<root>/lua/lshape`), as against the parents
    /// above.
    ///
    /// A copy is a dependency's source that happens to sit in the repo, and `mlua-pkg
    /// install` rewrites it every time it runs — so it is not the project's to check,
    /// format or take tests from, and editing one there does not survive the next install.
    /// What that means for the walkers is in [`crate::project_skip_dirs`].
    pub vendored_copies: Vec<PathBuf>,
    /// The `patch_dir` deps: a dependency's source taken into the tree, and what the
    /// manifest calls it. Unlike a `target_dir` copy, which install rewrites, this one is
    /// the project's own code — [`Project::patch`] wrote it once and the project edits it
    /// from then on. What that means for the walkers is in [`crate::patched_dirs`].
    pub patches: Vec<Patched>,
}

/// A dependency the project took into its tree: the name the manifest declares it under,
/// the directory `patch_dir` points at, and the directory inside it that `require` reads.
/// Both paths absolute.
///
/// The name is carried beside the directory because it is what a report says. htl's own
/// layout puts mathx in `patches/mathx`, but the manifest may name any directory, and a
/// type error in there is the dependency's name to report either way.
#[derive(Debug, Clone)]
pub struct Patched {
    /// The `[deps]` key, which is also the module name `require` reaches the dependency
    /// by — and so the name a diagnostic in the copy is reported under.
    pub name: String,
    /// Where `patch_dir` points, made absolute against the project root. The manifest
    /// writes it relative; a walker asked whether it may enter a directory needs the
    /// absolute form.
    pub dir: PathBuf,
    /// `<dir>/<entry>`: the dependency's own require root inside the copy, the same
    /// directory `entries/<name>` is a link to. `require("<name>.x")` reads `x.tl` from
    /// here. How it is arrived at is `patch_entry`'s to say, below; what goes on the
    /// search path is [`search_dir`](Self::search_dir).
    pub entry: PathBuf,
}

impl Patched {
    /// The directory to put on the search path so that the copy answers to the
    /// dependency's name.
    ///
    /// A directory on the path is consulted as `<dir>/<module>`, `<dir>/<module>/init`
    /// and `<dir>/<module>/<module>` (the three templates `add_path` writes), with the
    /// dots of the module name as separators. So the directory that resolves a
    /// dependency exactly as `.htl/modules/entries` does is the one holding
    /// [`entry`](Self::entry) *as a child named after the dependency* — which is what
    /// the link `entries/<name>` is, made out of a name and a directory rather than
    /// found as one.
    ///
    /// There is such a directory whenever the entry is named after the dependency:
    /// `src/<name>` and `lua/<name>`, the layout most packages have, and the copy root
    /// itself for a package whose entry is `.`, since `htl pkg patch` writes
    /// `patches/<dep>`. Its parent is the answer, and every name then resolves to the
    /// file an install would have resolved it to.
    ///
    /// A flat package — `entry = "src"` holding `<name>.tl` beside its other modules —
    /// has no such directory anywhere, because nothing in the copy is named after the
    /// dependency. The entry itself is the answer there: `require("<name>")` reads
    /// `<entry>/<name>.tl`, which is the file the link resolves it to as well, and
    /// `require("<name>.sub")` does not resolve, since the link reaches that at
    /// `<entry>/sub.tl` and no directory on a path reaches it as `<name>/sub`. Such a
    /// dependency needs its link, and an install is what writes one.
    ///
    /// Both go on the path before everything else, which puts them *last* in it
    /// ([`Htl::apply_project`](crate::Htl::apply_project)): a name the copy answers is
    /// one the project's own sources, the entry links and every other dependency have
    /// already declined, so the flat case's extra names cannot shadow anything.
    pub fn search_dir(&self) -> PathBuf {
        match (self.entry.file_name(), self.entry.parent()) {
            (Some(f), Some(up)) if f == std::ffi::OsStr::new(&self.name) => up.to_path_buf(),
            _ => self.entry.clone(),
        }
    }
}

/// Which directory inside a patched copy `require` reads it from.
///
/// `over` is the entry somebody recorded for this dependency — the lockfile's when an
/// install has run, else the `entry` the project's own `[deps.<name>]` overrides it with;
/// mlua-pkg gives the dependency's manifest precedence to the consumer, and the lockfile
/// is that decision already made. It is joined without asking whether the directory
/// exists, because a search path lists what a name *would* resolve through, and a tarball
/// is read before anything in it is built.
///
/// With nothing recorded — a fresh clone whose `mlua-pkg.lock` is not committed, or the
/// copy `cargo package` verifies when it is not — the copy answers for itself: its own
/// `mlua-pkg.toml` `[package].entry`, which `htl pkg patch` copied along with the sources,
/// and failing that mlua-pkg's own fallback chain through [`mlua_pkg::resolve_entry`]
/// (`src/`, then `lua/`, then the root). The chain is mlua-pkg's rule, called rather than
/// restated, so the directory htl searches and the directory an install would have linked
/// cannot drift apart. `resolve_entry` picks the first candidate that exists and errors
/// when none do; a `patch_dir` naming a directory nobody wrote is that error, and the
/// answer is the directory itself — a path on the search path that resolves nothing, which
/// is what the situation is.
fn patch_entry(dir: &Path, over: Option<&Path>) -> PathBuf {
    if let Some(e) = over {
        return mlua_pkg::lockfile::join_entry(dir, e);
    }
    if let Ok(m) = mlua_pkg::manifest::Manifest::from_path(dir.join(MANIFEST_NAME))
        && let Some(e) = m.package.entry
    {
        return mlua_pkg::lockfile::join_entry(dir, &e);
    }
    mlua_pkg::resolve_entry(dir, None).unwrap_or_else(|_| dir.to_path_buf())
}

/// What [`Project::add`] did: mlua-pkg's own report, and what htl carried across it.
///
/// `add` rewrites the whole `[deps.<name>]` entry, so a patch the entry declared would be
/// dropped by it. `kept_patch_dir` is that key, put back — named here so the report can say
/// it happened rather than leaving the manifest quietly different from what `add` wrote.
#[derive(Debug, Clone)]
pub struct AddDone {
    /// What mlua-pkg's own `add` returned, passed through unchanged so a caller reads the
    /// same report it would have got without htl in the way.
    pub report: mlua_pkg::ops::AddReport,
    /// The `patch_dir` the entry had before `add` rewrote it, when there was one. `None`
    /// means nothing was carried across — either the entry declared no patch, or the
    /// dependency is new.
    pub kept_patch_dir: Option<PathBuf>,
}

/// What [`Project::patch`] did: mlua-pkg's own report, and what htl took back out of the
/// copy.
///
/// The copy is made from a checkout rather than from a published archive, so it arrives
/// with the repository around the package. What was removed is named here rather than
/// happening quietly: the directory is about to be committed, and a file the author of the
/// dependency can see upstream and the patcher cannot find in `patches/<dep>` is a
/// difference worth one line of output.
///
/// `Project::patch` returned mlua-pkg's `PatchReport` bare until 0.6.3, which shipped
/// this type in its place as a patch release; 0.7.0 is the version that says an API
/// moved, and the one a caller of the old shape should read `0.6` as stopping before.
/// The same holds for [`Patched::entry`], which arrived in the same release.
#[derive(Debug, Clone)]
pub struct PatchDone {
    /// What mlua-pkg's own `patch` returned, passed through unchanged: where the copy is,
    /// whether the directory was created or rebuilt, and the revision it came from.
    pub report: mlua_pkg::ops::PatchReport,
    /// The dot-entries removed from the copy's root, by name and sorted. Empty when the
    /// repository had nothing of its own beside the package — which is most of the time,
    /// and why the report says this only when there is something to say.
    pub dropped: Vec<String>,
}

/// Where a patched dependency stands after an install: whether the copy is what the
/// dependency resolves from, and the two revisions the answer rests on.
///
/// `in_use` is false when the directory is gone, when the lockfile records no base for it,
/// or when the pin has moved on from that base — the dependency then resolves to the
/// upstream revision, and the copy sits in the tree unused until it is refreshed or
/// removed. See [`Project::patch_status`].
#[derive(Debug, Clone)]
pub struct PatchStatus {
    /// The `[deps]` key the patched dependency is declared under.
    pub name: String,
    /// The copy in the tree, absolute — reported whether or not it is [`in_use`](Self::in_use),
    /// since "the directory is there and nothing reads it" is the finding worth printing.
    pub dir: PathBuf,
    /// The revision the copy was taken from (`patch_base`), when the lockfile has one.
    pub base: Option<String>,
    /// The revision the pin resolves to, as the last install recorded it.
    pub locked: Option<String>,
    /// Whether the copy is what the dependency resolves from. False when the directory is
    /// gone, when there is no recorded `base`, or when `base` and `locked` have diverged —
    /// the three ways a patch stops being the thing in use, told apart by the two fields
    /// above rather than by a second enum.
    pub in_use: bool,
}

/// The manifest's file name, taken from mlua-pkg rather than spelled here, so htl and the
/// tool that writes the file cannot disagree about what it is called.
pub const MANIFEST_NAME: &str = mlua_pkg::project::MANIFEST_FILE_NAME;
/// The lockfile's file name, from mlua-pkg for the same reason as [`MANIFEST_NAME`].
pub const LOCKFILE_NAME: &str = mlua_pkg::project::LOCKFILE_FILE_NAME;

/// Where [`Project::patch`] puts a dependency it takes into the tree: `patches/<dep>`,
/// beside the project's own sources rather than under [`pkgs_dir`]. One directory per
/// dependency, named after it, so the path a diagnostic carries names the dependency it
/// is in.
pub const PATCHES_DIR: &str = "patches";

/// Where a project's installed deps go: `<root>/.htl/modules`, always.
///
/// One directory, named in one place. htl does not read the location out of the
/// environment and does not infer it from whether `target/` happens to exist — it decides
/// it here and hands it to mlua-pkg when it runs one (`htl pkg`), so the installer and the
/// checker cannot name different directories.
///
/// What goes on *inside* is mlua-pkg's: [`mlua_pkg::PkgDir`] derives `cache/` and
/// `vendored/` from the base, and this returns one so htl does not spell that layout out a
/// second time. The one directory htl adds beside them is [`ENTRIES_DIR`].
pub fn pkgs_dir(root: &Path) -> mlua_pkg::PkgDir {
    mlua_pkg::PkgDir::new(root.join(".htl").join("modules"))
}

/// The directory under [`pkgs_dir`] that holds one link per installed dep at that dep's
/// `entry` — where `require` looks. See [`Project::entries`].
pub const ENTRIES_DIR: &str = "entries";

/// The project that owns `dir`, when `dir` is a dependency's directory rather than a
/// project of its own.
///
/// Walks up from `dir` for a manifest that declares it — a `patch_dir` the project edits,
/// or a `target_dir` copy install rewrites — and answers with that project's root, or
/// with its owner in turn when the copy is itself inside another copy. `None` when
/// nothing above claims it.
///
/// One question, asked by [`Project::find`] and by
/// [`HtlConfig::find`](crate::config::HtlConfig::find), because a directory that is not a
/// project must not become one for either of them: the enclosing manifest decides, and a
/// manifest that came along in the copy does not.
pub(crate) fn owning_project(dir: &Path) -> Option<PathBuf> {
    let mut up = dir.to_path_buf();
    while up.pop() {
        if up.join(MANIFEST_NAME).is_file() && Project::at(&up).declares(dir) {
            return Some(owning_project(&up).unwrap_or(up));
        }
    }
    None
}

impl Project {
    /// Walk up from `start` (a file or directory) looking for `mlua-pkg.toml`.
    ///
    /// **The nearest manifest is not always the project.** `htl pkg patch` copies a
    /// dependency's whole package root into `patches/<dep>/`, its own `mlua-pkg.toml`
    /// among the files, so a file inside a patched dependency has a manifest above it
    /// belonging to the dependency and another above that belonging to the project doing
    /// the patching. The one that declared the copy is the project: the walk stops at the
    /// first manifest but then asks `owning_project` whether anything above claims that
    /// directory, and takes the answer. Nothing claims it and the first hit stands — a
    /// dependency checked out on its own is its own project.
    ///
    /// Whoever the root is gets the `.htl/`: the store, the installed deps, the entry
    /// links. A patch directory that was a root of its own collected a second one inside
    /// the project's tree, which is the nested `.htl/` of #267.
    pub fn find(start: &Path) -> Option<Self> {
        let mut dir = if start.is_dir() {
            start.to_path_buf()
        } else {
            crate::parent_dir(start)
        };
        if let Ok(abs) = std::fs::canonicalize(&dir) {
            dir = abs;
        }
        loop {
            let manifest = dir.join(MANIFEST_NAME);
            if manifest.is_file() {
                let root = owning_project(&dir).unwrap_or(dir);
                return Some(Self::at(&root));
            }
            if !dir.pop() {
                return None;
            }
        }
    }

    /// Is `dir` inside one of the dependency directories this project declares — a
    /// `patch_dir` it owns, or a `target_dir` copy install writes? Canonical paths on both
    /// sides: one comes from a manifest, the other from a walk.
    fn declares(&self, dir: &Path) -> bool {
        self.patches
            .iter()
            .map(|p| p.dir.clone())
            .chain(self.vendored_copies.iter().cloned())
            .any(|d| dir.starts_with(std::fs::canonicalize(&d).unwrap_or(d)))
    }

    /// Project rooted at `root` (must contain `mlua-pkg.toml`; not checked here).
    pub fn at(root: &Path) -> Self {
        let inner = mlua_pkg::Project::in_dir(root, pkgs_dir(root));
        let manifest = inner.manifest_path().to_path_buf();
        // `target_dir` deps: the copy itself, and the parent `require` searches. `patch_dir`
        // deps: the directory itself, which is what a walker is asked about. A manifest
        // that fails to parse contributes nothing here (mlua-pkg itself reports it).
        let mut target_dirs: Vec<PathBuf> = Vec::new();
        let mut vendored_copies: Vec<PathBuf> = Vec::new();
        let mut patches: Vec<Patched> = Vec::new();
        if let Ok(m) = mlua_pkg::manifest::Manifest::from_path(&manifest) {
            // The lockfile, and only for a manifest that patches something: it is the one
            // place an `entry` is recorded once an install has run, and every other
            // project would be paying a file read for an answer it has no question for.
            let locked = m
                .deps
                .values()
                .any(|d| d.patch_dir.is_some())
                .then(|| mlua_pkg::lockfile::Lockfile::read(inner.lock_path()).ok())
                .flatten();
            for (name, dep) in &m.deps {
                if let Some(td) = &dep.target_dir {
                    let abs = root.join(td);
                    let parent = abs
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| root.to_path_buf());
                    if !target_dirs.contains(&parent) {
                        target_dirs.push(parent);
                    }
                    if !vendored_copies.contains(&abs) {
                        vendored_copies.push(abs);
                    }
                }
                if let Some(pd) = &dep.patch_dir {
                    let dir = root.join(pd);
                    let over = locked
                        .as_ref()
                        .and_then(|l| l.pkg.iter().find(|p| &p.name == name))
                        .map(|p| p.entry.clone())
                        .or_else(|| dep.entry.clone());
                    let entry = patch_entry(&dir, over.as_deref());
                    patches.push(Patched {
                        name: name.clone(),
                        dir,
                        entry,
                    });
                }
            }
        }
        Self {
            root: root.to_path_buf(),
            manifest,
            lockfile: inner.lock_path().to_path_buf(),
            vendored: inner.pkg_dir().vendored(),
            entries: inner.pkg_dir().base().join(ENTRIES_DIR),
            pkgs_dir: inner.pkg_dir().base().to_path_buf(),
            target_dirs,
            vendored_copies,
            patches,
        }
    }

    /// Where the patched deps are, for a walker that only asks whether it may enter.
    pub fn patch_dirs(&self) -> Vec<PathBuf> {
        self.patches.iter().map(|p| p.dir.clone()).collect()
    }

    /// Where a `require` searches the patched deps: one directory per patch, at its
    /// [`search_dir`](Patched::search_dir).
    ///
    /// The search path and the cache's probe list are the same list, and this is it
    /// ([`Htl::apply_project`](crate::Htl::apply_project),
    /// [`crate::dependency_dirs`]). Nothing here is asked to exist: a `patch_dir` the
    /// manifest names and nobody has written yet is a directory a name resolves nothing
    /// through, and the probe over it is what notices when it arrives.
    pub fn patch_search_dirs(&self) -> Vec<PathBuf> {
        self.patches.iter().map(Patched::search_dir).collect()
    }

    /// `true` once `mlua-pkg install` has produced the lockfile.
    pub fn installed(&self) -> bool {
        self.lockfile.is_file()
    }

    /// Resolver for `.tl` / `.d.tl` inside installed deps (symlink-aware, like
    /// `VendoredResolver`), rooted at [`entries`](Self::entries) so a dep's `entry` is
    /// applied. Writes any link the lockfile calls for that is not there yet, and creates
    /// the directory if it does not exist.
    ///
    /// Under build scratch ([`crate::cache::scratch_root`]) it writes neither: a copy that
    /// carries no `.htl/` has no directory to root a resolver at, and this is the
    /// `InitError::RootNotFound` of a missing root rather than a write that would make
    /// cargo refuse the tarball. Nothing on the checking path comes through here — the
    /// checker and the macros take [`crate::Htl::apply_project`], which puts the same
    /// directory on the search path and does not mind that it is absent.
    pub fn teal_resolver(&self) -> Result<TealResolver, InitError> {
        let _ = self.link_entries();
        if crate::cache::scratch_root(&self.root).is_none() {
            let _ = std::fs::create_dir_all(&self.entries);
        }
        TealResolver::new_symlink_aware(&self.entries)
    }

    /// Write `entries/<name>` → `../vendored/<name>/<entry>` for every package the lockfile
    /// records, and remove a link there the lockfile no longer names.
    ///
    /// Idempotent and cheap: a link that already points where it should is left alone. It
    /// runs after every install, and again from [`teal_resolver`](Self::teal_resolver) and
    /// [`crate::Htl::apply_project`], so a project installed by an htl that did not write
    /// these works after upgrading without a reinstall. Returns the names linked, in
    /// lockfile order; no lockfile is no packages, not an error.
    ///
    /// The link is relative so that it follows `vendored/<name>` wherever install points
    /// that — at the cache, or at a `patch_dir` copy — rather than pinning a revision of
    /// its own. An entry of `"."` gets a link too, to the root: one layout, not two.
    ///
    /// **Under build scratch it repairs nothing**, and the reason it must not is
    /// [`crate::cache::scratch_root`]'s to state. It still reads the lockfile and still
    /// answers with the names, which is all the caller wanted from it there: the tarball
    /// carries `mlua-pkg.lock` and no `.htl/`, so the names are known and the links are
    /// not htl's to write into a tree cargo is verifying byte for byte (#267).
    pub fn link_entries(&self) -> anyhow::Result<Vec<String>> {
        if !self.installed() {
            return Ok(Vec::new());
        }
        let lock = mlua_pkg::lockfile::Lockfile::read(&self.lockfile)?;
        if crate::cache::scratch_root(&self.root).is_some() {
            return Ok(lock.pkg.iter().map(|p| p.name.clone()).collect());
        }
        std::fs::create_dir_all(&self.entries)
            .with_context(|| format!("creating {}", self.entries.display()))?;
        let mut names = Vec::new();
        for p in &lock.pkg {
            let mut target = PathBuf::from("..").join("vendored").join(&p.name);
            if !(p.entry.as_os_str().is_empty() || p.entry == Path::new(".")) {
                target.push(&p.entry);
            }
            let link = self.entries.join(&p.name);
            match std::fs::symlink_metadata(&link) {
                Ok(m) if m.file_type().is_symlink() => {
                    if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
                        names.push(p.name.clone());
                        continue;
                    }
                    remove_link(&link)?;
                }
                Ok(_) => anyhow::bail!(
                    "{} is not a link: htl writes that directory from the lockfile, and \
                     something else put a file there",
                    link.display()
                ),
                Err(_) => {}
            }
            make_link(&target, &link)?;
            names.push(p.name.clone());
        }
        // A dependency dropped from the manifest leaves its link behind otherwise, and a
        // `require` of it would then keep working until the cache was cleaned.
        if let Ok(rd) = std::fs::read_dir(&self.entries) {
            for e in rd.flatten() {
                let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
                let name = e.file_name().to_string_lossy().into_owned();
                if is_link && !names.contains(&name) {
                    remove_link(&e.path())?;
                }
            }
        }
        Ok(names)
    }

    /// mlua-pkg's own resolver for plain `.lua` inside vendored deps.
    pub fn vendored_resolver(&self) -> anyhow::Result<mlua_pkg::resolvers::VendoredResolver> {
        if self.installed() {
            Ok(mlua_pkg::resolvers::VendoredResolver::from_lockfile(
                &self.lockfile,
                &self.vendored,
            )?)
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

    /// Bring the declarations a dep publishes into the project's own `types/`.
    ///
    /// A dep that follows htl's own convention keeps its `.d.tl` under `types/` at its
    /// package root, and that is outside the entry directory `require` looks in
    /// (`entries/<name>`) — so the checker never sees it, and the depending project writes
    /// the declaration again by hand. Copying rather than widening the search path is what makes the
    /// result survive a fresh clone: [`pkgs_dir`] is machine-local and empty until someone
    /// installs, while `types/` is committed.
    ///
    /// A name `types/` already has is left alone and reported. Two libraries publishing a
    /// module of the same name is a real situation, and there is no registry to arbitrate
    /// it with, so the project decides rather than the last install winning.
    pub fn sync_types(&self) -> anyhow::Result<TypesSync> {
        let mut out = TypesSync::default();
        if !self.installed() {
            return Ok(out);
        }
        let lock = mlua_pkg::lockfile::Lockfile::read(&self.lockfile)?;
        let dest = self.root.join("types");
        for p in &lock.pkg {
            let Some(root) = self.package_root(p) else {
                continue;
            };
            copy_declarations(
                &root.join("types"),
                &dest,
                &Origin {
                    name: p.name.clone(),
                    sha: p.sha.clone(),
                    under: PathBuf::from("types"),
                },
                false,
                &mut out,
            )?;
        }
        Ok(out)
    }

    /// Copy one library's declarations out of teal-types into `types/`.
    ///
    /// teal-types is where the Teal ecosystem collects declarations for libraries that
    /// ship none of their own, laid out as `types/<library>/<module>.d.tl`. Nothing there
    /// ties a declaration to a version of the library it describes: the rocks are
    /// versioned on their own count, declare no dependency on the library, and name no
    /// revision of it. So the `.src` note beside each file is the whole of the record —
    /// what was taken, and from which commit of the collection.
    pub fn add_types(&self, library: &str, force: bool) -> anyhow::Result<TypesSync> {
        let cache = pkgs_dir(&self.root).cache();
        std::fs::create_dir_all(&cache)?;
        let fetcher = mlua_pkg::fetcher::GitFetcher::new(cache);
        let got = mlua_pkg::fetcher::Fetcher::fetch(
            &fetcher,
            &mlua_pkg::manifest::Dep {
                git: TEAL_TYPES_GIT.to_string(),
                tag: None,
                rev: None,
                branch: None,
                entry: None,
                target_dir: None,
                patch_dir: None,
                patch_drift: None,
            },
        )?;
        self.add_types_from(&got.cache_path, library, &got.sha, force)
    }

    /// The same from a checkout already on disk, recording `sha` as the revision it is at.
    pub fn add_types_from(
        &self,
        checkout: &Path,
        library: &str,
        sha: &str,
        force: bool,
    ) -> anyhow::Result<TypesSync> {
        let under = Path::new("types").join(library);
        let published = checkout.join(&under);
        if !published.is_dir() {
            anyhow::bail!("{}", no_such_library(checkout, library));
        }
        let mut out = TypesSync::default();
        copy_declarations(
            &published,
            &self.root.join("types"),
            &Origin {
                name: TEAL_TYPES_NAME.to_string(),
                sha: sha.to_string(),
                under,
            },
            force,
            &mut out,
        )?;
        Ok(out)
    }

    /// What mlua-pkg is handed to act on this project: htl's own directories, and the
    /// manifest read from disk.
    ///
    /// The library reads neither the environment nor the working directory to decide where
    /// packages go — it takes the [`mlua_pkg::PkgDir`] it is given — so [`pkgs_dir`] is the
    /// only place that answer is written down, for the installer and the checker alike.
    fn config(&self) -> mlua_pkg::Config {
        mlua_pkg::Config::new(mlua_pkg::Project::in_dir(&self.root, pkgs_dir(&self.root)))
    }

    /// Fetch every dependency the manifest declares, and write the lockfile.
    ///
    /// The report says what each one resolved to and where it was placed, including
    /// whether it came from a `patch_dir`; nothing is printed here. Declarations a
    /// dependency publishes are a separate step ([`Project::sync_types`]) because they are
    /// copied into the project rather than installed.
    pub fn install(&self) -> anyhow::Result<mlua_pkg::ops::InstallReport> {
        let report = mlua_pkg::ops::install(&self.config())?;
        // The roots are placed and the lockfile written: now the links `require` reads.
        self.link_entries()?;
        Ok(report)
    }

    /// Write a dependency into the manifest. `install` is what fetches it.
    ///
    /// mlua-pkg replaces the whole `[deps.<name>]` entry and `AddSpec` carries no
    /// `patch_dir`, so adding a dependency that is already patched would drop the key that
    /// binds `patches/<dep>` to it — the project would keep building, against upstream,
    /// with the copy sitting unread in the tree. What the entry declared about its patch is
    /// carried across and reported.
    pub fn add(&self, spec: mlua_pkg::ops::AddSpec) -> anyhow::Result<AddDone> {
        let name = spec.name.clone();
        let previous = mlua_pkg::manifest::Manifest::from_path(&self.manifest)
            .ok()
            .and_then(|m| m.deps.get(&name).cloned());
        let report = mlua_pkg::ops::add(&self.config(), spec)?;
        let Some(dep) = previous else {
            return Ok(AddDone {
                report,
                kept_patch_dir: None,
            });
        };
        let Some(dir) = dep.patch_dir.clone() else {
            return Ok(AddDone {
                report,
                kept_patch_dir: None,
            });
        };
        set_dep_key(&self.manifest, &name, "patch_dir", &to_toml_path(&dir))?;
        if let Some(drift) = dep.patch_drift {
            let value = match drift {
                mlua_pkg::manifest::PatchDrift::Warn => "warn",
                mlua_pkg::manifest::PatchDrift::Error => "error",
            };
            set_dep_key(&self.manifest, &name, "patch_drift", value)?;
        }
        Ok(AddDone {
            report,
            kept_patch_dir: Some(dir),
        })
    }

    /// Refresh dependencies, bump the pins that follow releases, and install what changed.
    pub fn update(
        &self,
        opts: mlua_pkg::ops::UpdateOpts,
    ) -> anyhow::Result<mlua_pkg::ops::UpdateReport> {
        let mut report = mlua_pkg::ops::update(&self.config(), opts)?;
        self.link_entries()?;
        // mlua-pkg walks a map, so the same project reports its dependencies in a
        // different order on every run. A report that is read by a person, and diffed
        // against the last one, is sorted.
        report.entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(report)
    }

    /// Remove cached packages the lockfile no longer refers to (`all`: the whole cache).
    ///
    /// Never touches what install placed under `vendored/`: a dangling link there is
    /// repaired by the next install.
    pub fn clean(&self, all: bool) -> anyhow::Result<mlua_pkg::ops::CleanReport> {
        Ok(mlua_pkg::ops::clean(&self.config(), all)?)
    }

    /// Take a dependency's source into `patches/<dep>/`, where the project owns it.
    ///
    /// The whole package root is copied, so the dep's `types/` comes with it, minus the
    /// dot-entries at its root, which are the repository the package was checked out of
    /// rather than the package — [`PatchDone::dropped`] names the ones that were there.
    /// `patch_dir` on that dependency in the manifest says which dependency the directory
    /// stands in for. There is no patch file and nothing is applied: from here the
    /// directory is the project's code, edited and committed with git like the rest of the
    /// tree, and install resolves the dependency from it for as long as the pin still
    /// resolves to the revision the copy was taken from (`patch_base` in the lockfile).
    /// When the pin moves on, install uses the new revision, leaves the copy alone and
    /// says so on every install until the patch is refreshed or removed.
    ///
    /// On a dependency that is already patched this refreshes the copy from the revision
    /// the pin now resolves to and records that as the new base. The copy is overwritten
    /// rather than merged — carrying the project's own change forward onto it is a merge
    /// git performs, and it can only do that if the change is committed — so a directory
    /// with uncommitted changes is refused unless `force`.
    pub fn patch(&self, name: &str, force: bool) -> anyhow::Result<PatchDone> {
        let manifest = mlua_pkg::manifest::Manifest::from_path(&self.manifest)?;
        let dep = manifest.deps.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "no dependency '{name}' in {}: `htl pkg patch` takes a name the manifest declares",
                self.manifest.display()
            )
        })?;
        // Where the copy goes. htl's own layout is `patches/<dep>`; a manifest that
        // already names a directory keeps the one it names.
        let declared = dep.patch_dir.is_some();
        let rel = match &dep.patch_dir {
            Some(p) => p.clone(),
            None => PathBuf::from(format!("{PATCHES_DIR}/{name}")),
        };
        let dir = self.root.join(&rel);
        if dir.exists() && !force {
            refuse_if_uncommitted(&self.root, &rel)?;
        }

        let before = std::fs::read_to_string(&self.manifest)?;
        if !declared {
            set_dep_key(&self.manifest, name, "patch_dir", &to_toml_path(&rel))?;
        }

        // mlua-pkg does the copy and the bookkeeping: it fetches the pin, copies the
        // package root into `patch_dir`, and records the commit it came from as
        // `patch_base`. `force` there is the "directory already exists" refusal, which is
        // the question already answered above against git rather than against the
        // directory's existence.
        let opts = mlua_pkg::ops::PatchOpts {
            name: name.to_string(),
            force: true,
        };
        match mlua_pkg::ops::patch(&self.config(), opts) {
            Ok(report) => {
                let dropped = drop_dot_entries(&report.patch_dir)?;
                Ok(PatchDone { report, dropped })
            }
            Err(e) => {
                // A `patch_dir` naming a directory that was never written turns every
                // later install into a drift report, so the manifest goes back as it was.
                if !declared {
                    let _ = std::fs::write(&self.manifest, &before);
                }
                Err(e.into())
            }
        }
    }

    /// Where each patched dependency stands, read back from the manifest and the lockfile.
    ///
    /// A patch is bound to the revision it was taken from. Install compares the two itself
    /// and falls back to upstream when they differ; this reads the same two values
    /// afterwards so htl can say what happened in its own verbs — mlua-pkg's warning names
    /// `mlua-pkg patch --force`, which skips the question htl asks git and leaves the
    /// upstream repository's dot-entries in the copy.
    pub fn patch_status(&self) -> Vec<PatchStatus> {
        let lock = mlua_pkg::lockfile::Lockfile::read(&self.lockfile).ok();
        self.patches
            .iter()
            .map(|p| {
                let locked = lock
                    .as_ref()
                    .and_then(|l| l.pkg.iter().find(|e| e.name == p.name));
                let base = locked.and_then(|e| e.patch_base.clone());
                let sha = locked.map(|e| e.sha.clone());
                let in_use = p.dir.is_dir() && base.is_some() && base == sha;
                PatchStatus {
                    name: p.name.clone(),
                    dir: p.dir.clone(),
                    base,
                    locked: sha,
                    in_use,
                }
            })
            .collect()
    }

    /// The package root behind `vendored/<name>`.
    ///
    /// That symlink points at the package root itself, and the lockfile's `entry` says
    /// where below it `require` looks — so what a dep publishes beside its entry, `types/`
    /// among it, is reached from here without subtracting the entry again. mlua-pkg moved
    /// the symlink from the entry directory to the root in 0.11; a dep whose entry is
    /// `src/` used to need the difference popped off and now must not.
    fn package_root(&self, p: &mlua_pkg::lockfile::LockedPkg) -> Option<PathBuf> {
        std::fs::canonicalize(self.vendored.join(&p.name)).ok()
    }
}

/// A directory link at `link` pointing at `target`, as written (relative stays relative).
fn make_link(target: &Path, link: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    let r = std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    let r = std::os::windows::fs::symlink_dir(target, link);
    r.with_context(|| format!("linking {} -> {}", link.display(), target.display()))
}

/// Remove a link, and only a link: the caller has checked what is there.
fn remove_link(link: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    let r = std::fs::remove_file(link);
    #[cfg(windows)]
    let r = std::fs::remove_dir(link).or_else(|_| std::fs::remove_file(link));
    r.with_context(|| format!("removing the link {}", link.display()))
}

/// Write one key onto `[deps.<name>]`, leaving the rest of the file as it was.
///
/// The manifest is a file a person wrote: its comments say why a dependency is pinned
/// where it is, and its order is the order they put things in. `toml_edit` keeps both,
/// where re-serialising the parsed manifest would not.
fn set_dep_key(manifest: &Path, name: &str, key: &str, value: &str) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(manifest)?;
    let mut doc = text.parse::<toml_edit::DocumentMut>()?;
    let deps = doc
        .get_mut("deps")
        .and_then(|i| i.as_table_like_mut())
        .with_context(|| format!("no [deps] table in {}", manifest.display()))?;
    let entry = deps
        .get_mut(name)
        .and_then(|i| i.as_table_like_mut())
        .with_context(|| format!("[deps.{name}] is not a table"))?;
    entry.insert(key, toml_edit::value(value));
    std::fs::write(manifest, doc.to_string())?;
    Ok(())
}

/// A manifest-relative path as the manifest spells it: `/` on every platform, because the
/// file is read on all of them.
fn to_toml_path(p: &Path) -> String {
    p.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Take the upstream repository's own dot-entries out of the copy's root, and name them.
///
/// The copy is made from a checkout rather than from a published archive, so it arrives
/// with the repository around the package: `.git`, the CI workflows under `.github`, the
/// ignore rules, whatever tool state (`.htl`, `.mlua-pkgs`) and OS litter (`.DS_Store`) the
/// checkout happened to hold. None of it is the dependency's source, and each kind of it
/// costs the project that is about to commit the directory something:
///
/// - `.git` — git reads `patches/<dep>` as an embedded repository and records it as a
///   gitlink, a commit id pointing at a repository nobody else has, with none of the files
///   in this project's history.
/// - `.github` — a workflow under `patches/` is inert (GitHub only runs the ones at the
///   repository root) but it is still a workflow file, and the gates that watch
///   `.github/workflows/*` — review rules, secret scanners, branch protection — fire on it.
/// - `.gitignore` — a second ignore file inside the tree, whose rules were written for a
///   different repository, silently drops files from this project's own commits. That is
///   not a hypothesis: it is what cargo's vendored copies do to their consumers
///   (rust-lang/cargo#13607), and the lesson cargo draws is that a copy landing inside
///   somebody else's repository must not carry ignore rules with it.
/// - the rest — [`crate::is_skipped_dir`] already refuses to descend into a dot-directory,
///   so anything else here would be committed, reviewed and never read.
///
/// Root level only. Below the copy's root a dot-entry belongs to the package the way any
/// other file there does, and htl does not know which ones the dependency needs. The same
/// reasoning keeps `htl.toml` and `mlua-pkg.toml`: they are the package's, they are what
/// says where its entry is, and install reads them from the copy.
///
/// This is a denylist of one shape rather than an allowlist of names, which is the
/// narrowest rule that closes the whole class — npm's named denylist has to grow a name
/// every time an ecosystem invents a dotfile, and `.github` is still not on it. There is no
/// flag to keep them: somebody who wants the repository clones the repository.
fn drop_dot_entries(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut dropped = Vec::new();
    let entries = std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // `file_type` here is the directory entry's, so a symlink is a symlink rather than
        // what it points at — and a link is unlinked, never followed and emptied. (A
        // worktree checkout's `.git` is a plain file pointing elsewhere; it goes the same
        // way.)
        let ft = entry
            .file_type()
            .with_context(|| format!("reading {}", path.display()))?;
        if ft.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        }
        .with_context(|| format!("removing {}", path.display()))?;
        dropped.push(name);
    }
    // read_dir's order is the filesystem's. What a report prints, and what a test asserts,
    // is sorted.
    dropped.sort();
    Ok(dropped)
}

/// Refuse to overwrite a patched copy that git has not been told about.
///
/// The refresh replaces the directory with the pinned upstream, and the project's own
/// change survives that only through git: it is carried forward by merging the new copy
/// with the history of the old one. A change git cannot see is a change that cannot be
/// carried forward, so it is named here and the refresh does not happen.
fn refuse_if_uncommitted(root: &Path, rel: &Path) -> anyhow::Result<()> {
    match uncommitted(root, rel) {
        Ok(changes) if changes.is_empty() => Ok(()),
        Ok(changes) => {
            let mut msg = format!(
                "{} has uncommitted changes, and refreshing it from the pin overwrites \
                 them. Commit them first — git is what carries them onto the refreshed \
                 copy — or pass --force to discard them:",
                rel.display()
            );
            for c in changes.iter().take(10) {
                msg.push_str("\n  ");
                msg.push_str(c);
            }
            if changes.len() > 10 {
                msg.push_str(&format!("\n  and {} more", changes.len() - 10));
            }
            anyhow::bail!("{msg}")
        }
        Err(why) => anyhow::bail!(
            "cannot tell whether {} has uncommitted changes ({why}), and refreshing it \
             from the pin overwrites whatever is in it. Pass --force to refresh it anyway.",
            rel.display()
        ),
    }
}

/// What `git status` reports under `rel`, one entry per line as it prints them.
///
/// Untracked files count: the question is what would be lost, and a file git was never
/// told about is lost the same way an edited one is. `Err` is what could not be asked
/// rather than what came back dirty — no `git` on PATH, or a tree that is not a
/// repository. The pathspec is the manifest-relative one and the command runs at the
/// project root, so git reads it the way it reads any path a person types there.
fn uncommitted(root: &Path, rel: &Path) -> Result<Vec<String>, String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain", "--"])
        .arg(rel)
        .output()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => "no `git` on PATH".to_string(),
            _ => e.to_string(),
        })?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if why.is_empty() {
            format!("git exited {}", out.status)
        } else {
            why
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end().to_string())
        .collect())
}

/// Where the Teal ecosystem collects declarations for libraries that ship none of their
/// own: `types/<library>/<module>.d.tl`, published to LuaRocks one library at a time as
/// `<library>-tl-type`.
pub const TEAL_TYPES_GIT: &str = "https://github.com/teal-language/teal-types";

/// What the `.src` notes call it.
const TEAL_TYPES_NAME: &str = "teal-types";

/// What [`Project::sync_types`] and [`Project::add_types`] did: one entry per declaration
/// they were offered.
#[derive(Debug, Default)]
pub struct TypesSync {
    /// Written into `types/`, with what published it.
    pub written: Vec<(PathBuf, String)>,
    /// Left as it was, because `types/` already had that name — with what offered one too.
    pub taken: Vec<(PathBuf, String)>,
}

/// Where a declaration came from, as the `.src` note beside it records it: what published
/// it, at which revision, and the path it had there.
struct Origin {
    name: String,
    sha: String,
    under: PathBuf,
}

/// Copy every `.d.tl` under `from` into `to`, keeping the path below `from`.
///
/// Keeping it is what keeps the module name: `socket/http.d.tl` is
/// `require("socket.http")`, and flattening it into `types/http.d.tl` would rename the
/// module to one the library never had.
fn copy_declarations(
    from: &Path,
    to: &Path,
    origin: &Origin,
    force: bool,
    out: &mut TypesSync,
) -> anyhow::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    let mut found: Vec<PathBuf> = walkdir::WalkDir::new(from)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|p| crate::is_declaration(p))
        .collect();
    found.sort();
    for src in found {
        let rel = src.strip_prefix(from).unwrap_or(&src).to_path_buf();
        let target = to.join(&rel);
        if target.exists() && !force {
            out.taken.push((target, origin.name.clone()));
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &target)?;
        // Beside it, the one thing the Lua ecosystem records nowhere: which revision of
        // what this declaration was taken from. Without it, staleness is not a question
        // anyone can ask.
        let mut note = target.clone().into_os_string();
        note.push(".src");
        std::fs::write(
            PathBuf::from(note),
            format!(
                "{} {} {}\n",
                origin.name,
                origin.sha,
                origin.under.join(&rel).display()
            ),
        )?;
        out.written.push((target, origin.name.clone()));
    }
    Ok(())
}

/// What to say when the collection has no such library: the names it does have that look
/// related, or how many it holds at all — a list of every one of them is not an error
/// message.
fn no_such_library(checkout: &Path, library: &str) -> String {
    let mut names: Vec<String> = std::fs::read_dir(checkout.join("types"))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let near: Vec<&str> = names
        .iter()
        .filter(|n| n.contains(library) || library.contains(n.as_str()))
        .map(String::as_str)
        .collect();
    if near.is_empty() {
        format!(
            "teal-types has no declarations for `{library}` ({} libraries there)",
            names.len()
        )
    } else {
        format!(
            "teal-types has no declarations for `{library}` — it has {}",
            near.join(", ")
        )
    }
}

/// One [`TealResolver`] per `[[contract]]` in `htl.toml`, in declaration order, so the
/// host and `htl check` enforce the same contracts from the same source. `root` is the
/// directory holding `htl.toml` (the path [`HtlConfig::find`](crate::config::HtlConfig::find)
/// returns, minus the file name). Add them to a `Registry` before the plain resolvers.
pub fn contract_resolvers(
    root: &Path,
    cfg: &crate::config::HtlConfig,
) -> Result<Vec<TealResolver>, InitError> {
    let (contracts, _) = crate::contract::resolve(root, cfg);
    let mut out = Vec::new();
    for c in &contracts {
        out.extend(TealResolver::for_contract(root, cfg, c)?);
    }
    Ok(out)
}

impl crate::Htl {
    /// Make the project's installed deps visible to the Teal checker and to the
    /// prelude's strict searcher (`htl run` / `htl test` without a Registry).
    ///
    /// The directory on the path is [`Project::entries`], where each dep is reached at its
    /// `entry`; the links are written first if the lockfile calls for any that are missing.
    ///
    /// **A `patch_dir` dependency is on the path in its own right**, at
    /// [`Project::patch_search_dirs`]. The copy is committed and the manifest names it,
    /// so the two together are the whole of what a `require` of that dependency needs:
    /// no install, no link, no network, and nothing that has to exist outside what a
    /// clone or a tarball carries. That is the arrangement `cargo vendor` and Go's
    /// `vendor/` settled on — the copy plus the manifest naming it is the source of
    /// truth, and its presence is what turns the network off — and htl's reason for it
    /// is the one #266 found: `.htl/` is gitignored, so the copy `cargo package`
    /// verifies has the patch and the manifest and no links at all, and every `require`
    /// of the dependency failed there with `module not found`.
    ///
    /// Those directories go on first and are therefore consulted last, after the links,
    /// the `target_dir` copies and the project's own `src/`. A checkout that has
    /// installed resolves exactly what it resolved before — the link and the copy are
    /// the same files, and the link still answers first — so what this adds is an answer
    /// where there was none.
    ///
    /// Except under build scratch, where this writes nothing at all — the rule and the
    /// reason are [`crate::cache::scratch_root`]'s. What it does there it does read-only:
    /// the directories go on the search path whether or not they exist (a path that
    /// resolves nothing is what a tarball with no `.htl/` means, and the project's own
    /// `src/` is still there), and the dependency names still come from the lockfile.
    pub fn apply_project(&self, p: &Project) -> anyhow::Result<()> {
        let installed = p.link_entries()?;
        // The names, for the rules that are about a library the project has rather than
        // about its own code (`htlx-available`). The lockfile's rather than the manifest's:
        // a dependency nothing installed is one `require` cannot reach, and advice to use
        // it would be advice to fail a check.
        self.set_deps(&installed)?;
        // First, which is to say last: `add_path` prepends, so what goes on here is what
        // the path consults after everything below. A checkout that has installed
        // resolves through its links exactly as it did before, and the copy answers where
        // there are none — a tarball, a clone nobody has installed in yet.
        for d in p.patch_search_dirs() {
            self.add_path(&d)?;
        }
        if crate::cache::scratch_root(&p.root).is_none() {
            let _ = std::fs::create_dir_all(&p.entries);
        }
        self.add_path(&p.entries)?;
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
///
/// Every variant carries `module` — the name that was required, not the path it resolved
/// to — because that is the name the `require` in the caller's source spells, and the
/// caller is where the mistake is read from. All four are returned as `Some(Err)` so the
/// `Registry` stops rather than falling through to a later resolver: a `.tl` that does not
/// check must not be quietly replaced by a `.lua` of the same name.
#[derive(Debug)]
pub enum TealResolveError {
    /// The module does not type-check on its own terms.
    TypeCheck {
        /// The name that was required.
        module: String,
        /// The checker's errors, one per line as it reported them.
        errors: Vec<String>,
    },
    /// The module type-checks on its own but is not assignable to the resolver's
    /// [`expect_type`](TealResolver::expect_type).
    Expectation {
        /// The name that was required.
        module: String,
        /// The type path the resolver demands, as `expect_type` was given it.
        expected: String,
        /// Why the assignment failed. The message adds a hint to annotate the returned
        /// table, because these errors are about the whole value and carry no line of
        /// their own until the module names its type.
        errors: Vec<String>,
    },
    /// [`require_fields`](TealResolver::require_fields): required fields absent at run time.
    MissingFields {
        /// The name that was required.
        module: String,
        /// The type whose fields were demanded — the same `expect_type`, since
        /// `require_fields` only applies alongside it.
        expected: String,
        /// The fields that were nil. Named rather than counted: every Teal record field is
        /// nilable, so which ones are missing is the whole of what the type check could
        /// not say.
        fields: Vec<String>,
    },
    /// The file could not be read through the sandbox — outside the root, or gone between
    /// the resolver finding it and opening it.
    Read {
        /// The name that was required.
        module: String,
        /// What the sandbox refused or failed on.
        source: ReadError,
    },
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
            Self::Expectation {
                module,
                expected,
                errors,
            } => {
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
            Self::MissingFields {
                module,
                expected,
                fields,
            } => write!(
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
                        return Some(h.get::<Function>("type_only_module").and_then(|f| {
                            f.call::<Value>((name, file.resolved_path.to_string_lossy().as_ref()))
                        }));
                    }
                    let loaded =
                        match self.load_teal(lua, &h, &file.content, &file.resolved_path, name) {
                            Ok(v) => v,
                            Err(e) => return Some(Err(e)),
                        };
                    if !self.held(name) {
                        return Some(Ok(loaded));
                    }
                    match self.missing_fields(&h, &loaded) {
                        Ok(m) if m.is_empty() => return Some(Ok(loaded)),
                        Ok(missing) => {
                            return Some(Err(mlua::Error::external(
                                TealResolveError::MissingFields {
                                    module: name.to_string(),
                                    expected: self.expect_type.clone().unwrap_or_default(),
                                    fields: missing,
                                },
                            )));
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
