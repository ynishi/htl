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
pub mod config;
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
    /// Every `require("<literal>")` in the file and where the checker resolved it.
    /// Input to [`require_cycles`].
    pub requires: Vec<RequireSite>,
}

/// One literal `require` call in a checked file.
#[derive(Debug, Clone)]
pub struct RequireSite {
    pub module: String,
    /// Resolved file, `None` when the checker could not find it.
    pub path: Option<PathBuf>,
    pub line: usize,
    pub col: usize,
}

/// Result of a static contract check (see [`Htl::contract_check`]).
#[derive(Debug, Clone, Default)]
pub struct ContractResult {
    /// Type errors from `local m: <T> = require("<mod>")`.
    pub errors: Vec<String>,
    /// Declared fields absent from the module's returned table literal; `None` when the
    /// return value is not a literal (not decidable statically).
    pub missing: Option<Vec<String>>,
    pub missing_at: (usize, usize),
}

impl Htl {
    /// Make an `htl.toml` project's dirs visible to the checker: `root`, `root/src` and
    /// `[check] paths`. `root` is the directory holding `htl.toml`.
    pub fn apply_config(&self, root: &Path, cfg: &config::HtlConfig) -> Result<()> {
        for p in cfg.search_paths(root) {
            self.add_path(&p)?;
        }
        Ok(())
    }

    /// Static form of `TealResolver::expect_type` / `require_fields` for one module file:
    /// `modname` is what a `require` would say (its stem), `type_path` is `"defs.Mod"`.
    pub fn contract_check(&self, file: &Path, modname: &str, type_path: &str, require_fields: bool) -> Result<ContractResult> {
        let f: Function = self.h.get("contract_check")?;
        let t: Table = f.call((path_str(file), modname, type_path, require_fields))?;
        let errors: Table = t.get("errors")?;
        let errors = errors.sequence_values::<String>().collect::<mlua::Result<_>>()?;
        let missing = match t.get::<Option<Table>>("missing")? {
            Some(m) => Some(m.sequence_values::<String>().collect::<mlua::Result<Vec<_>>>()?),
            None => None,
        };
        let missing_at = (
            t.get::<Option<usize>>("missing_y")?.unwrap_or(1),
            t.get::<Option<usize>>("missing_x")?.unwrap_or(1),
        );
        Ok(ContractResult { errors, missing, missing_at })
    }
}

/// `contract` lint for one file: when `file` sits directly under a `[[contract]]` dir
/// of `cfg` (relative to `root`, the directory holding `htl.toml`), check it against
/// that contract statically. Returns lint lines (empty when no contract applies).
pub fn contract_lints(h: &Htl, root: &Path, cfg: &config::HtlConfig, file: &Path) -> Result<Vec<String>> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let file_abs = canon(file);
    let mut out = Vec::new();
    if !is_tl_source(&file_abs) {
        return Ok(out);
    }
    let modname = file_abs.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    for c in &cfg.contract {
        let Some(dir) = c.dirs(root).into_iter().map(|d| canon(&d)).find(|d| file_abs.parent() == Some(d.as_path()))
        else {
            continue;
        };
        if !c.applies_to(&modname) {
            continue;
        }
        // Same visibility as `TealResolver::for_contract`: the contract dir, plus the
        // project root, its `src/` and `[check] paths`.
        h.add_path(&dir)?;
        h.apply_config(root, cfg)?;
        let r = h.contract_check(&file_abs, &modname, &c.type_path, c.require_fields)?;
        for e in &r.errors {
            // The stub's own "<contract ...>:L:C: " prefix says nothing useful; keep the message.
            let msg = e.splitn(4, ':').last().unwrap_or(e).trim();
            out.push(format!(
                "{}:1:1: does not satisfy contract {} ({}): {msg} [htl contract]",
                file.display(),
                c.type_path,
                c.dir
            ));
        }
        if let Some(missing) = &r.missing
            && !missing.is_empty()
        {
            out.push(format!(
                "{}:{}:{}: returned table lacks declared field(s) of {}: {} [htl contract]",
                file.display(),
                r.missing_at.0,
                r.missing_at.1,
                c.type_path,
                missing.join(", ")
            ));
        }
    }
    Ok(out)
}

/// `contract-unenforced` lint: a `[[contract]]` in `htl.toml` only becomes a run-time
/// guarantee when the host builds its resolver with it. Scan the host crate's Rust
/// sources (under `cargo_root`) for `expect_type("<type>")` (plus `require_fields()` when
/// required) or for the config-driven `contract_resolvers(` / `for_contract(` helpers.
/// No host crate (`cargo_root` = None) means a script-only project: nothing to enforce.
pub fn contract_enforcement_lints(cfg: &config::HtlConfig, cfg_path: &Path, cargo_root: Option<&Path>) -> Vec<String> {
    let mut out = Vec::new();
    if cfg.contract.is_empty() {
        return out;
    }
    let Some(root) = cargo_root else { return out };
    let mut sources = String::new();
    for sub in ["src", "examples", "tests", "benches"] {
        let dir = root.join(sub);
        if !dir.is_dir() {
            continue;
        }
        for e in walkdir::WalkDir::new(&dir).into_iter().flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()) == Some("rs")
                && let Ok(t) = std::fs::read_to_string(p)
            {
                sources.push_str(&t);
                sources.push('\n');
            }
        }
    }
    // `for_contract_dir(` does not contain `for_contract(` as a substring: list it.
    let by_config = ["contract_resolvers(", "for_contract(", "for_contract_dir("]
        .iter()
        .any(|api| sources.contains(api));
    for c in &cfg.contract {
        // A contract with nothing under it is not enforced by anyone; the dir may be
        // populated later (glob dirs especially), so say nothing about the host.
        if c.dirs(root_of(cfg_path)).is_empty() {
            continue;
        }
        let by_hand = sources.contains(&format!("expect_type(\"{}\")", c.type_path));
        let want_fields = if c.require_fields { ".require_fields()" } else { "" };
        if !(by_config || by_hand) {
            let by_hand_hint = if c.dir.contains('*') {
                String::new() // one resolver per matched dir: not a one-liner by hand
            } else {
                format!("add TealResolver::new(\"{}\").expect_type(\"{}\"){} in the Rust host, or ", c.dir, c.type_path, want_fields)
            };
            out.push(format!(
                "{}:1:1: contract `{}` -> {} is declared but the host does not enforce it: {}build resolvers with \
                 htl::pkg::contract_resolvers(root, &config) [htl contract-unenforced]",
                cfg_path.display(),
                c.dir,
                c.type_path,
                by_hand_hint
            ));
        } else if c.require_fields && !by_config && !sources.contains("require_fields()") {
            out.push(format!(
                "{}:1:1: contract `{}` -> {} has require_fields = true but the host never calls .require_fields(): \
                 missing fields will pass at run time [htl contract-unenforced]",
                cfg_path.display(),
                c.dir,
                c.type_path
            ));
        }
    }
    out
}

fn root_of(cfg_path: &Path) -> &Path {
    cfg_path.parent().unwrap_or(Path::new("."))
}

/// Cycles in the require graph of a set of checked files, one message per cycle,
/// anchored at the first edge's call site. Teal types a circular require as an opaque
/// `circular_require`, so a cycle shows up elsewhere as "cannot index" errors; naming
/// the loop is the useful part. Files outside `infos` are treated as leaves.
pub fn require_cycles(infos: &[(PathBuf, CheckInfo)]) -> Vec<String> {
    use std::collections::{HashMap, HashSet};
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let mut edges: HashMap<PathBuf, Vec<(PathBuf, &RequireSite)>> = HashMap::new();
    let mut display: HashMap<PathBuf, PathBuf> = HashMap::new();
    for (file, ci) in infos {
        let from = canon(file);
        display.insert(from.clone(), file.clone());
        let list = edges.entry(from).or_default();
        for r in &ci.requires {
            if let Some(p) = &r.path {
                list.push((canon(p), r));
            }
        }
    }
    let nodes: Vec<PathBuf> = {
        let mut v: Vec<PathBuf> = edges.keys().cloned().collect();
        v.sort();
        v
    };
    let mut out = Vec::new();
    let mut reported: HashSet<Vec<PathBuf>> = HashSet::new();
    let mut state: HashMap<PathBuf, u8> = HashMap::new(); // 1 = on stack, 2 = done
    let mut stack: Vec<(PathBuf, Option<&RequireSite>)> = Vec::new();

    fn dfs<'a>(
        node: PathBuf,
        edges: &HashMap<PathBuf, Vec<(PathBuf, &'a RequireSite)>>,
        state: &mut HashMap<PathBuf, u8>,
        stack: &mut Vec<(PathBuf, Option<&'a RequireSite>)>,
        reported: &mut HashSet<Vec<PathBuf>>,
        display: &HashMap<PathBuf, PathBuf>,
        out: &mut Vec<String>,
    ) {
        state.insert(node.clone(), 1);
        if let Some(list) = edges.get(&node) {
            for (to, site) in list {
                match state.get(to).copied() {
                    Some(1) => {
                        // back edge: cycle = stack from `to` .. node, then back to `to`
                        let start = stack.iter().position(|(n, _)| n == to).unwrap_or(0);
                        let mut members: Vec<PathBuf> =
                            stack[start..].iter().map(|(n, _)| n.clone()).chain(std::iter::once(node.clone())).collect();
                        members.dedup();
                        let mut key = members.clone();
                        key.sort();
                        if reported.insert(key) {
                            let name = |p: &PathBuf| {
                                display
                                    .get(p)
                                    .unwrap_or(p)
                                    .file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| p.display().to_string())
                            };
                            let chain: Vec<String> = members.iter().map(name).chain(std::iter::once(name(to))).collect();
                            let first_file = display.get(&members[0]).cloned().unwrap_or_else(|| members[0].clone());
                            // anchor: the edge leaving the cycle's first member
                            let anchor = stack.get(start + 1).and_then(|(_, s)| *s).unwrap_or(site);
                            out.push(format!(
                                "{}:{}:{}: require cycle: {} (Teal types the back edge as an opaque circular require; \
                                 break it by moving shared types into a module both sides require) [htl require-cycle]",
                                first_file.display(),
                                anchor.line,
                                anchor.col,
                                chain.join(" -> ")
                            ));
                        }
                    }
                    Some(2) => {}
                    _ => {
                        stack.push((to.clone(), Some(site)));
                        dfs(to.clone(), edges, state, stack, reported, display, out);
                        stack.pop();
                    }
                }
            }
        }
        state.insert(node, 2);
    }

    for n in nodes {
        if !state.contains_key(&n) {
            stack.push((n.clone(), None));
            dfs(n, &edges, &mut state, &mut stack, &mut reported, &display, &mut out);
            stack.pop();
        }
    }
    out.sort();
    out
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
    /// The program's state: `require`, preloads, `exec`, bundles.
    lua: Lua,
    /// The prelude table (checker API). Lives in `lua` unless this is a split state
    /// made by [`with_checker`](Self::with_checker), where it belongs to the checker.
    h: Table,
    /// `true` when the checker is another Lua state (`with_checker`).
    split: bool,
}

/// Checker prelude of another state, kept in a runtime state's app data so the
/// mlua-pkg resolvers find their checker (`Htl::with_checker`).
pub(crate) struct CheckerHandle(pub(crate) Table);

const RUNTIME_REGISTRY_KEY: &str = "htl.runtime";

/// The part of the prelude a runtime state needs when its checker lives elsewhere:
/// the strict searcher (asking the checker through `gen`), the declaration-only
/// module, and `package.path` bookkeeping.
const RUNTIME_PRELUDE: &str = r#"
local R = {}

function R.type_only_module(module_name, decl_path)
   return setmetatable({}, {
      __index = function(_, key)
         error(string.format(
            "module '%s' is declaration-only here (%s): '%s' has no implementation on this path. " ..
            "It must be provided by the host program (e.g. a Rust #[host_module] via cargo run) " ..
            "or by a .tl/.lua module with that name.",
            module_name, decl_path, tostring(key)), 2)
      end,
   })
end

-- gen(name) -> kind, a, b  (see resolve_for_require in the checker prelude)
function R.install_searcher(gen)
   table.insert(package.searchers, 2, function(module_name)
      local kind, a, b = gen(module_name)
      if kind == "code" then
         local chunk, lerr = load(a, "@" .. b, "t")
         if not chunk then
            error("htl: generated Lua failed to load: " .. tostring(lerr), 0)
         end
         return function(modname) return chunk(modname, b) end, b
      elseif kind == "type_only" then
         return function() return R.type_only_module(module_name, a) end, a
      end
      return a
   end)
end

function R.add_path(dir)
   local templates = dir .. "/?.lua;" .. dir .. "/?/init.lua;" .. dir .. "/?/?.lua"
   if package.path == nil or package.path == "" then
      package.path = templates
   else
      package.path = templates .. ";" .. package.path
   end
end

function R.reset_path()
   package.path = ""
end

return R
"#;

impl Htl {
    /// New state. Uses `Lua::unsafe_new` so stripped bytecode bundles can be loaded.
    pub fn new() -> Result<Self> {
        // SAFETY: we accept binary chunks only from bundles we produced ourselves.
        let lua = unsafe { Lua::unsafe_new() };
        Self::from_lua(lua)
    }

    /// A fresh program state that borrows `checker`'s compiler instead of loading its
    /// own: modules `checker` has already type-checked and generated are served from
    /// its store, so a run of many programs (the test runner: one state per file)
    /// checks each module once. The program state itself is as isolated as
    /// [`new`](Self::new): nothing but the checker is shared. The checker starts a new
    /// program env for this state (module-name resolution is per program).
    pub fn with_checker(checker: &Htl) -> Result<Self> {
        // SAFETY: as in `new`.
        let lua = unsafe { Lua::unsafe_new() };
        let r: Table = lua
            .load(RUNTIME_PRELUDE)
            .set_name("=htl-runtime")
            .eval()
            .context("loading htl runtime prelude")?;
        lua.set_named_registry_value(RUNTIME_REGISTRY_KEY, r)?;
        lua.set_app_data(CheckerHandle(checker.h.clone()));
        let begin: Function = checker.h.get("begin_program")?;
        begin.call::<()>(())?;
        Ok(Self { lua, h: checker.h.clone(), split: true })
    }

    fn runtime(&self) -> Result<Table> {
        Ok(self.lua.named_registry_value::<Table>(RUNTIME_REGISTRY_KEY)?)
    }

    /// The checker's `package.path` (what `require` inside `.tl` resolves through).
    pub fn search_path(&self) -> Result<String> {
        let f: Function = self.h.get("get_path")?;
        Ok(f.call(())?)
    }

    /// Restore a checker `package.path` taken with [`search_path`](Self::search_path).
    pub fn set_search_path(&self, path: &str) -> Result<()> {
        let f: Function = self.h.get("set_path")?;
        f.call::<()>(path)?;
        Ok(())
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
        Ok(Self { lua, h, split: false })
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
        if self.split {
            let f: Function = self.runtime()?.get("reset_path")?;
            f.call::<()>(())?;
        }
        Ok(())
    }

    /// Search paths implied by where `file` sits in the scaffold layout: its own
    /// directory, and for a file under `tests/` also the project root and `<root>/src`
    /// (the test runner's rule, so `htl check tests` sees what `htl test` sees).
    pub fn add_layout_paths(&self, file: &Path) -> Result<()> {
        let dir = parent_dir(file);
        self.add_path(&dir)?;
        if dir.file_name().is_some_and(|n| n == "tests")
            && let Some(root) = dir.parent()
        {
            self.add_path(root)?;
            let src = root.join("src");
            if src.is_dir() {
                self.add_path(&src)?;
            }
        }
        Ok(())
    }

    /// Prepend `dir/?.tl;dir/?/init.tl` to `package.path` (Teal resolves requires through it).
    pub fn add_path(&self, dir: &Path) -> Result<()> {
        let f: Function = self.h.get("add_path")?;
        f.call::<()>(path_str(dir))?;
        if self.split {
            // The program state resolves plain `.lua` (and `.d.tl` siblings) itself.
            let f: Function = self.runtime()?.get("add_path")?;
            f.call::<()>(path_str(dir))?;
        }
        Ok(())
    }

    /// Install the strict `.tl` searcher: `require` of a `.tl` with type errors fails.
    pub fn install_searcher(&self) -> Result<()> {
        if self.split {
            // The searcher runs in the program state and asks the checker for code.
            let gen_fn: Function = self.h.get("gen_for_require")?;
            let bridge = self.lua.create_function(move |_, name: String| {
                let (kind, a, b): (String, Option<String>, Option<String>) = gen_fn.call(name)?;
                Ok((kind, a, b))
            })?;
            let f: Function = self.runtime()?.get("install_searcher")?;
            f.call::<()>(bridge)?;
            return Ok(());
        }
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

/// A user-facing message for an error that came out of running Lua: the innermost
/// cause without Lua's `stack traceback:` block. A host function's `Err(e)` surfaces
/// as `e`'s own text; a Lua `error("msg")` surfaces as `file:line: msg`.
///
/// ```text
/// sgen: content/no-date.md: front matter: 'date' is required
/// ```
/// instead of that line followed by `stack traceback: [C]: in method 'pages' ...`.
pub fn user_message(err: &anyhow::Error) -> String {
    fn from_mlua(e: &mlua::Error) -> String {
        match e {
            mlua::Error::CallbackError { cause, .. } => from_mlua(cause),
            mlua::Error::ExternalError(ext) => ext.to_string(),
            mlua::Error::WithContext { cause, .. } => from_mlua(cause),
            other => strip_traceback(&other.to_string()),
        }
    }
    if let Some(e) = err.downcast_ref::<mlua::Error>() {
        return from_mlua(e);
    }
    strip_traceback(&format!("{err:#}"))
}

/// Remove a trailing Lua `stack traceback:` section from an error text.
pub fn strip_traceback(text: &str) -> String {
    let cut = text.find("\nstack traceback:").unwrap_or(text.len());
    text[..cut].trim_end().to_string()
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
    let mut requires = Vec::new();
    if let Ok(list) = t.get::<Table>("requires") {
        for r in list.sequence_values::<Table>() {
            let r = r?;
            requires.push(RequireSite {
                module: r.get::<String>("name")?,
                path: r.get::<Option<String>>("path")?.map(PathBuf::from),
                line: r.get::<Option<usize>>("y")?.unwrap_or(0),
                col: r.get::<Option<usize>>("x")?.unwrap_or(0),
            });
        }
    }
    Ok(CheckInfo {
        errors: seq("errors")?,
        warnings: seq("warnings")?,
        deps: seq("deps")?.into_iter().map(PathBuf::from).collect(),
        lints: seq("lints")?,
        requires,
    })
}

/// `true` for `foo.tl` but not `foo.d.tl`.
pub fn is_tl_source(p: &Path) -> bool {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    p.is_file() && name.ends_with(".tl") && !name.ends_with(".d.tl")
}

/// Directories never descended into when collecting sources under a root: build output,
/// installed packages, VCS and tool state. A root passed explicitly is always walked.
pub const SKIP_DIRS: &[&str] = &["target", "node_modules", ".mlua-pkgs", ".git"];

/// `true` for a directory entry that source collection should not enter: a name in
/// [`SKIP_DIRS`], any dot-directory, or the project's mlua-pkg directory (`pkgs_dir`,
/// which `MLUA_PKG_DIR` can move somewhere unremarkable).
pub fn is_skipped_dir(path: &Path, extra: &[PathBuf]) -> bool {
    if !path.is_dir() {
        return false;
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if SKIP_DIRS.contains(&name) || (name.starts_with('.') && name.len() > 1) {
        return true;
    }
    extra.iter().any(|e| same_dir(path, e))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Extra directories to skip below `root`: the mlua-pkg package dir when `root` is
/// inside an `mlua-pkg.toml` project (its vendored / cached sources are dependencies,
/// not the project's own files).
#[cfg(feature = "pkg")]
pub fn project_skip_dirs(root: &Path) -> Vec<PathBuf> {
    match pkg::Project::find(root) {
        Some(p) => vec![p.pkgs_dir],
        None => Vec::new(),
    }
}

#[cfg(not(feature = "pkg"))]
pub fn project_skip_dirs(_root: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// Collect `.tl` sources from files and directories (sorted, recursive). Directories in
/// [`SKIP_DIRS`], dot-directories and the project's package dir are not entered unless
/// given as a root themselves.
pub fn collect_tl(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let extra = project_skip_dirs(p);
            let root = p.clone();
            let walker = walkdir::WalkDir::new(p)
                .sort_by_file_name()
                .into_iter()
                .filter_entry(move |e| e.path() == root || !is_skipped_dir(e.path(), &extra));
            for e in walker {
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
