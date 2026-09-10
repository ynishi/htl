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
pub mod cache;
#[cfg(feature = "dts")]
pub mod cexport;
pub mod config;
pub mod contract;
#[cfg(feature = "dts")]
pub mod dep_dts;
pub mod diagnostic;
#[cfg(feature = "dts")]
pub mod dts;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod fix;
pub mod link;
#[cfg(feature = "pkg")]
pub mod pkg;
// The project layer: a walk over many files, the run cache under it, and the decisions
// both `htl check` and a macro expansion make about that store. It reaches the mlua-pkg
// project a file belongs to and the Cargo package around it, so it asks for the two
// features that provide them; every consumer that has a project to check has both.
#[cfg(all(feature = "pkg", feature = "dts"))]
pub mod project;
// What one module name resolves to, and what that hides. Reads the project the same way
// the project layer does — the installed deps, the config's search paths, the notes `htl
// dts` leaves under `types/<crate>/` — so it carries the same features.
#[cfg(all(feature = "pkg", feature = "dts"))]
pub mod resolve;
pub mod teal;
pub mod testing;
// The complement of the require closure: what no entry reaches. On the project layer,
// whose check hands it the graph, so it carries that layer's features.
#[cfg(all(feature = "pkg", feature = "dts"))]
pub mod unused;

pub use diagnostic::{Diagnostic, Severity};

/// Registry key under which the prelude table is stored (lets `pkg::TealResolver`
/// reach the compiler from a bare `&Lua`).
pub(crate) const PRELUDE_REGISTRY_KEY: &str = "htl.prelude";

const TL_SRC: &str = include_str!("../vendor/tl.lua");
const LINT_SRC: &str = include_str!("lint.lua");
const FMT_SRC: &str = include_str!("fmt.lua");
const PRELUDE: &str = include_str!("prelude.lua");

/// A hash of the Lua the checker is made of: the vendored `tl`, the lints, the formatter
/// and the prelude. Two builds with the same value generate the same Lua for the same
/// input, whatever else differs about them.
///
/// The run cache stamps its entries with this ([`cache`]). The CLI also stamps them with
/// its own binary, which moves on every rebuild; inside a proc macro the binary is
/// `rustc`, which does not move when htl does, and this is what tells those entries apart
/// from a checker that no longer exists.
pub fn checker_identity() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        let mut h = blake3::Hasher::new();
        for src in [TL_SRC, LINT_SRC, FMT_SRC, PRELUDE] {
            h.update(src.as_bytes());
            h.update(b"\0");
        }
        h.finalize().to_hex().to_string()
    })
}

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
    /// `error_fixes[i]` is the fix for `errors[i]`, when the error has one.
    pub error_fixes: Vec<Option<Fix>>,
    /// `lint_fixes[i]` is the fix for `lints[i]`, when the lint has one.
    pub lint_fixes: Vec<Option<Fix>>,
    /// Type errors in the modules this check pulled in through `require`, transitively,
    /// each dependency once. Not in `errors`, and not what [`ok`](Self::ok) answers: the
    /// file itself checked, and generates; it is the `require` of that module that will
    /// raise at run time ([`Htl::install_searcher`]), which is why a caller reporting on a
    /// project treats these as errors too (`htl check`, `include_tl!`).
    pub dependency_errors: Vec<DependencyError>,
}

/// A type error in a module a check reached through `require` (see
/// [`CheckInfo::dependency_errors`]).
///
/// The checker checks a required module into the same environment and hands the
/// requirer its *type*; the module's own errors stay with the module's result. This is
/// that result's error, said against the file that required it, so a report can name
/// both — a dependency is only ever checked through a `require`, since its sources are
/// not the project's to walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyError {
    /// The file the error is in, as the checker found it on the search path.
    pub file: PathBuf,
    /// The file whose `require` (direct or through another dependency) pulled it in:
    /// the first one on this check's walk.
    pub required_by: PathBuf,
    /// `file:line:col: message`, formatted as the file's own errors are.
    pub text: String,
}

/// How safely a [`Fix`] can be applied without a human looking at it.
///
/// Serializes as its [`as_str`](Applicability::as_str) name, which is what a stored fix
/// and `--format json` both carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Applicability {
    /// The rewrite does not change what the program does at run time.
    Safe,
    /// It may; applied only when asked (`htl fix --unsafe`).
    Unsafe,
    /// Shown, never applied (a placeholder to fill, a choice to make).
    Suggest,
}

impl Applicability {
    pub fn as_str(self) -> &'static str {
        match self {
            Applicability::Safe => "safe",
            Applicability::Unsafe => "unsafe",
            Applicability::Suggest => "suggest",
        }
    }
}

/// One text replacement: `[start, end)` in 1-based line / byte-column coordinates;
/// an insertion has `end == start`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Edit {
    pub line: usize,
    pub col: usize,
    pub end_line: usize,
    pub end_col: usize,
    pub text: String,
}

/// A mechanical rewrite attached to a diagnostic (see [`fix`]).
///
/// Serializes as [`cache::FixJson`] does, since the two describe the same thing and the
/// store reads back what `--format json` prints.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Fix {
    pub applicability: Applicability,
    pub edits: Vec<Edit>,
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

/// A named function of a `.tl` file, for coverage (see [`Htl::coverage_spans`]).
///
/// The body is `line + 1 ..= last - 1`, strictly between the two: defining a function
/// runs both ends of it, so neither says whether the function was ever entered. A
/// never-called `function m.f()` spanning lines 12..15 comes back from the line hook
/// with 12 and 15 executed and 13, 14 not. Functions with nothing in between (one
/// line, or an empty body) have no such span and are not reported at all.
#[derive(Debug, Clone)]
pub struct FunctionSpan {
    /// As the source writes it: `f`, `M.f`, `M:f`.
    pub name: String,
    /// The line the function is declared on.
    pub line: usize,
    /// The line its `end` is on. Always at least `line + 2`.
    pub last: usize,
}

/// What one parse gives a coverage report: the statement ranges, and the functions
/// those ranges sit in. See [`Htl::coverage_spans`].
pub type CoverageSpans = (Vec<(usize, usize)>, Vec<FunctionSpan>);

/// What a file on the search path is, for [`Htl::module_candidates`]. The three the
/// searchers try, in the order they try them: a `.tl` source beats a `.d.tl` declaration
/// wherever the two sit, and a plain `.lua` is what is left when neither is reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleKind {
    Source,
    Declaration,
    Lua,
}

impl ModuleKind {
    fn of(s: &str) -> Self {
        match s {
            "source" => Self::Source,
            "declaration" => Self::Declaration,
            _ => Self::Lua,
        }
    }

    /// As a report says it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Declaration => "declaration",
            Self::Lua => "lua",
        }
    }
}

impl std::fmt::Display for ModuleKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One file `require(name)` could have resolved to. See [`Htl::module_candidates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleCandidate {
    pub path: PathBuf,
    pub kind: ModuleKind,
    /// The search-path directory it was found under.
    pub dir: PathBuf,
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
    /// Names `require_fields` asked for that the contract type does not declare. The
    /// config is wrong about the type, which is a different finding from a module that
    /// fails the contract, and no module can fix it.
    pub bad_require_fields: Vec<String>,
}

impl Htl {
    /// Make an `htl.toml` project's dirs visible to the checker: `root`, `root/src` and
    /// `[check] paths`. `root` is the directory holding `htl.toml`.
    pub fn apply_config(&self, root: &Path, cfg: &config::HtlConfig) -> Result<()> {
        self.add_search_paths(&cfg.search_paths(root))
    }

    /// Put `dirs` on the search path so they are consulted **in the order given** — the
    /// order [`search_paths`](config::HtlConfig::search_paths) documents, and the one a
    /// reader assumes from a list. [`add_path`](Self::add_path) prepends, so adding the
    /// list front to back would leave its last entry first; this adds it back to front.
    ///
    /// It decides one thing: which of two declarations of the same module is read. A
    /// `.tl` source beats a `.d.tl` wherever the two sit, so until neither is a source
    /// the order is invisible.
    pub fn add_search_paths(&self, dirs: &[PathBuf]) -> Result<()> {
        for p in dirs.iter().rev() {
            self.add_path(p)?;
        }
        Ok(())
    }

    /// Static form of `TealResolver::expect_type` / `require_fields` for one module file:
    /// `modname` is what a `require` would say (its stem), `type_path` is `"defs.Mod"`.
    pub fn contract_check(
        &self,
        file: &Path,
        modname: &str,
        type_path: &str,
        require_fields: &config::RequireFields,
    ) -> Result<ContractResult> {
        let f: Function = self.h.get("contract_check")?;
        // `true` for "everything the type declares", the list itself when it names them.
        let wanted = match require_fields.named() {
            Some(names) => mlua::Value::Table(self.lua().create_sequence_from(names.to_vec())?),
            None => mlua::Value::Boolean(require_fields.is_on()),
        };
        let t: Table = f.call((path_str(file), modname, type_path, wanted))?;
        let errors: Table = t.get("errors")?;
        let errors = errors
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()?;
        let missing = match t.get::<Option<Table>>("missing")? {
            Some(m) => Some(
                m.sequence_values::<String>()
                    .collect::<mlua::Result<Vec<_>>>()?,
            ),
            None => None,
        };
        let missing_at = (
            t.get::<Option<usize>>("missing_y")?.unwrap_or(1),
            t.get::<Option<usize>>("missing_x")?.unwrap_or(1),
        );
        let bad_require_fields = match t.get::<Option<Table>>("bad_require_fields")? {
            Some(b) => b
                .sequence_values::<String>()
                .collect::<mlua::Result<Vec<_>>>()?,
            None => Vec::new(),
        };
        Ok(ContractResult {
            errors,
            missing,
            missing_at,
            bad_require_fields,
        })
    }
}

/// `contract` lint for one file: when `file` sits directly under the directory a
/// contract holds (relative to `root`, the directory holding `htl.toml`), check it
/// against that contract statically. Returns lint lines (empty when none applies).
///
/// `contracts` comes from [`contract::resolve`], which reads the `---@contract` markers;
/// resolving once per run rather than once per file is the caller's job.
pub fn contract_lints(
    h: &Htl,
    root: &Path,
    cfg: &config::HtlConfig,
    contracts: &[contract::Resolved],
    file: &Path,
) -> Result<Vec<String>> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let file_abs = canon(file);
    let mut out = Vec::new();
    if !is_tl_source(&file_abs) {
        return Ok(out);
    }
    let modname = file_abs
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    for c in contracts {
        let Some(dir) = c
            .dirs(root)
            .into_iter()
            .map(|d| canon(&d))
            .find(|d| file_abs.parent() == Some(d.as_path()))
        else {
            continue;
        };
        if !c.applies_to(&modname) {
            continue;
        }
        // Same visibility as `TealResolver::for_contract`: the contract dir, plus what
        // `HtlConfig::search_paths` gives (the project root, its `src/` and `types/`,
        // then `[check] paths`). Both sides go through that one function.
        h.add_path(&dir)?;
        h.apply_config(root, cfg)?;
        let r = h.contract_check(&file_abs, &modname, &c.type_path, &c.require_fields)?;
        if !r.bad_require_fields.is_empty() {
            // A `---@required` the checker cannot see as a field of the record: the
            // marker is on something else, and no module under the dir can satisfy it.
            out.push(format!(
                "{}:{}:1: ---@required on field(s) {} does not declare: {} [htl contract]",
                c.declared_in.display(),
                c.declared_at,
                c.type_path,
                r.bad_require_fields.join(", ")
            ));
            continue;
        }
        for e in &r.errors {
            // The stub's own "<contract ...>:L:C: " prefix says nothing useful; keep the
            // message. The same reading of a diagnostic's text every other caller makes.
            let msg = diagnostic::position(e)
                .map_or(e.as_str(), |(_, _, _, msg)| msg)
                .trim();
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

/// `duplicate-declaration` lint: a module `file` requires resolved to a `.d.tl` while
/// another `.d.tl` for the same module was reachable further along the search path. One
/// was read and the other was not, decided by position, and until now nothing said so —
/// the case this catches is a host publishing a declaration into a project that also
/// keeps a hand-written one for the same module.
///
/// Only declarations collide. A `.tl` source beats every `.d.tl` wherever the two sit
/// (`prelude.lua` searches sources across the whole path first), so a require that
/// landed on a source is not reported, and neither is a module declared once.
///
/// A require that landed on a source is where the other lint here lives.
/// `host-module-shadowed`: `host_modules` are the names the surrounding crate registers
/// in `package.preload` (from `#[host_module]`, scanned without a build), and Lua
/// consults preload before any path searcher. So when a require of one of those names
/// resolved to a file, the check read the file and the run will load the host: what was
/// checked is not what runs, and the program fails at the first call of anything the two
/// do not share. Both halves of that are already in hand at this point — the name the
/// host registers, and the path the checker read — which is why it is asked here.
///
/// A require of a host module name that landed on a `.d.tl` is not reported: a
/// declaration is how a host module is given types at all, and `htl dts` writes exactly
/// that file, so the two agree by construction.
///
/// Call it with the search path the file was checked under: the answer depends on it.
pub fn declaration_conflict_lints(
    h: &Htl,
    file: &Path,
    info: &CheckInfo,
    host_modules: &[String],
) -> Result<Vec<String>> {
    let f: Function = h.h.get("declaration_sites")?;
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for site in &info.requires {
        let Some(read) = site.path.as_ref() else {
            continue;
        };
        // One report per module, not one per `require` of it.
        if seen.contains(&site.module.as_str()) {
            continue;
        }
        if !is_declaration(read) {
            if host_modules.contains(&site.module) {
                seen.push(&site.module);
                out.push(format!(
                    "{}:{}:{}: {} is a host module of this crate and also {}: the check \
                     reads the file, the run loads the host — package.preload is consulted \
                     before any path searcher, so what is checked here is not what runs \
                     [htl host-module-shadowed]",
                    file.display(),
                    site.line,
                    site.col,
                    site.module,
                    read.display(),
                ));
            }
            continue;
        }
        let sites: Vec<String> = f
            .call::<Table>(site.module.as_str())?
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()?;
        let shadowed: Vec<&str> = sites
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !same_file(Path::new(s), read))
            .collect();
        if shadowed.is_empty() {
            continue;
        }
        seen.push(&site.module);
        out.push(format!(
            "{}:{}:{}: {} is declared more than once on the search path: {} is read, {} {} not [htl duplicate-declaration]",
            file.display(),
            site.line,
            site.col,
            site.module,
            read.display(),
            shadowed.join(" and "),
            if shadowed.len() == 1 { "is" } else { "are" },
        ));
    }
    Ok(out)
}

/// `contract-unenforced` lint: a contract only becomes a run-time guarantee when the
/// host builds its resolver from it. Scan the host crate's Rust sources (under
/// `cargo_root`) for `contract_resolvers(`. No host crate (`cargo_root` = None) means a
/// script-only project: nothing to enforce.
///
/// One call to look for, not four. `contract_resolvers(root, &config)` is what the README
/// documents and what keeps the host and `htl check` reading the same markers; a resolver
/// assembled by hand from `expect_type` / `require_fields` now has to restate what the
/// record already says, so recognising it would be recognising the drift this lint
/// exists to prevent. Enforcement the scan cannot see at all — a Lua-side validator, a
/// resolver in a sibling crate, generated code, or a resolver built by hand — is what
/// `[[contract]] enforced_by` is for: it names the file the enforcement lives in, and
/// that contract is then not held to the scan. The file has to exist, which is what
/// separates the key from a per-contract off switch, and a name that points at nothing is
/// reported under this same rule whether or not the call was found.
pub fn contract_enforcement_lints(
    cfg_path: &Path,
    contracts: &[contract::Resolved],
    cargo_root: Option<&Path>,
) -> Vec<String> {
    let mut out = Vec::new();
    if contracts.is_empty() {
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
    let by_config = sources.contains("contract_resolvers(");
    for c in contracts {
        // A contract with nothing under it is not enforced by anyone; the dir may be
        // populated later (glob dirs especially), so say nothing about the host.
        if c.dirs(root_of(cfg_path)).is_empty() {
            continue;
        }
        match &c.enforced_by {
            // The path is the whole of what makes `enforced_by` a claim rather than an
            // off switch, so it is checked whether or not the scan found the call: a name
            // that points at nothing is a broken statement either way.
            Some(p) => {
                let at = config::resolve_path(root_of(cfg_path), p);
                if !at.exists() {
                    out.push(format!(
                        "{}:1:1: contract {} -> {} says it is enforced by {:?}, and there \
                         is no such file: name where the enforcement lives, or drop the \
                         key and let the scan look for \
                         htl::pkg::contract_resolvers(root, &config) \
                         [htl contract-unenforced]",
                        cfg_path.display(),
                        c.dir,
                        c.type_path,
                        p,
                    ));
                }
            }
            None if !by_config => out.push(format!(
                "{}:{}:1: contract {} -> {} is declared but the host does not enforce it: \
                 build resolvers with htl::pkg::contract_resolvers(root, &config), or say \
                 where it is enforced with [[contract]] enforced_by \
                 [htl contract-unenforced]",
                c.declared_in.display(),
                c.declared_at,
                c.dir,
                c.type_path,
            )),
            None => {}
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
                        let mut members: Vec<PathBuf> = stack[start..]
                            .iter()
                            .map(|(n, _)| n.clone())
                            .chain(std::iter::once(node.clone()))
                            .collect();
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
                            let chain: Vec<String> = members
                                .iter()
                                .map(name)
                                .chain(std::iter::once(name(to)))
                                .collect();
                            let first_file = display
                                .get(&members[0])
                                .cloned()
                                .unwrap_or_else(|| members[0].clone());
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
            dfs(
                n,
                &edges,
                &mut state,
                &mut stack,
                &mut reported,
                &display,
                &mut out,
            );
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

    /// Everything this check found about the file itself, structured, in the order the
    /// text output says it: warnings, then lints, then errors.
    ///
    /// Errors in what the file *required* are not here — they belong to the module they
    /// are in, and it is the reporting caller that decides how to say them
    /// ([`dependency_errors`](Self::dependency_errors)).
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut out = self.warning_diagnostics();
        out.extend(self.lint_diagnostics());
        out.extend(self.error_diagnostics());
        out
    }

    /// [`errors`](Self::errors) with their positions and their fixes.
    pub fn error_diagnostics(&self) -> Vec<Diagnostic> {
        parsed(Severity::Error, &self.errors, &self.error_fixes)
    }

    /// [`warnings`](Self::warnings) with their positions. Warnings carry no fix.
    pub fn warning_diagnostics(&self) -> Vec<Diagnostic> {
        parsed(Severity::Warning, &self.warnings, &[])
    }

    /// [`lints`](Self::lints) with their positions, their rule names and their fixes.
    pub fn lint_diagnostics(&self) -> Vec<Diagnostic> {
        parsed(Severity::Lint, &self.lints, &self.lint_fixes)
    }
}

/// `texts[i]` parsed, with `fixes[i]` attached when there is one.
fn parsed(severity: Severity, texts: &[String], fixes: &[Option<Fix>]) -> Vec<Diagnostic> {
    texts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let mut d = Diagnostic::parse(severity, text);
            d.fix = fixes.get(i).and_then(|f| f.clone());
            d
        })
        .collect()
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

-- Put already-generated Lua in front of the searcher for one module name.
--
-- `package.preload` is searcher position 1 and R.install_searcher puts htl's at 2, so a
-- preloaded module is never asked of the searcher — which is the point: asking would check
-- and generate it again. Loaded the same way the searcher would have loaded it, so the
-- module sees the same chunk name and the same arguments.
function R.preload_generated(module_name, code, filename)
   -- Never displace what is already there. The test library and anything a host preloads are
   -- put in package.preload by whoever owns them, and Lua generated from a `.tl` of the same
   -- name is not the same module: preloading over `htl.test` gives the file a stand-in whose
   -- `run()` reports nothing, and every test silently stops counting.
   if package.preload[module_name] ~= nil then return end
   local chunk, lerr = load(code, "@" .. filename, "t")
   if not chunk then
      error("htl: cached Lua failed to load: " .. tostring(lerr), 0)
   end
   package.preload[module_name] = function(modname) return chunk(modname, filename) end
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

-- Line coverage: which lines of which chunk ran. Lua's line hook is per thread, so
-- code that runs inside a coroutine the test creates is not seen.
local cov = nil
function R.coverage_start()
   cov = {}
   -- The line event is the hot path. One "S" lookup per function (cached by the
   -- function object) instead of per line; a call/return-event stack was measured
   -- slower on a call-heavy suite, since calls are almost as frequent as lines there.
   local srcs = setmetatable({}, { __mode = "k" })
   local getinfo = debug.getinfo
   debug.sethook(function(_, line)
      local fi = getinfo(2, "f")
      local func = fi and fi.func
      if func == nil then return end
      local t = srcs[func]
      if t == nil then
         local si = getinfo(2, "S")
         local src = si and si.source
         t = false
         if src then
            t = cov[src]
            if not t then
               t = {}
               cov[src] = t
            end
         end
         srcs[func] = t
      end
      if t then t[line] = true end
   end, "l")
end

function R.coverage_stop()
   debug.sethook()
   local out = {}
   for src, lines in pairs(cov or {}) do
      local list = {}
      for l in pairs(lines) do list[#list + 1] = l end
      table.sort(list)
      out[#out + 1] = { source = src, lines = list }
   end
   cov = nil
   return out
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
        Ok(Self {
            lua,
            h: checker.h.clone(),
            split: true,
        })
    }

    fn runtime(&self) -> Result<Table> {
        Ok(self
            .lua
            .named_registry_value::<Table>(RUNTIME_REGISTRY_KEY)?)
    }

    /// Put Lua this checker generated for a `.tl` module in front of the searcher, in a
    /// program state.
    ///
    /// Distinct from [`preload`](Self::preload), which registers a source string as a module:
    /// this loads the way the searcher would have, so the module sees the same chunk name and
    /// the same arguments as if it had been generated during the run.
    ///
    /// Without it, a `require` in running code asks the searcher, which checks and generates
    /// the module then and there. With it, the module is already present. The two are the
    /// same thing only if `code` is what this checker would generate now — the caller's
    /// promise, and the reason anything serving this has to invalidate on the module's own
    /// content.
    pub fn preload_generated(&self, name: &str, code: &str, file: &Path) -> Result<()> {
        let f: Function = self.runtime()?.get("preload_generated")?;
        f.call::<()>((name, code, path_str(file)))?;
        Ok(())
    }

    /// Start recording which lines of which chunk run in the program state (a state
    /// made by [`with_checker`](Self::with_checker)). Lua's line hook is per thread:
    /// code inside coroutines the program creates is not seen.
    pub fn coverage_start(&self) -> Result<()> {
        let f: Function = self.runtime()?.get("coverage_start")?;
        f.call::<()>(())?;
        Ok(())
    }

    /// Stop recording; `(chunk source, sorted executed lines)` per chunk. Sources are as
    /// Lua names them: `@<path>` for files loaded by the searcher and the entry.
    pub fn coverage_stop(&self) -> Result<Vec<(String, Vec<usize>)>> {
        let f: Function = self.runtime()?.get("coverage_stop")?;
        let t: Table = f.call(())?;
        let mut out = Vec::new();
        for e in t.sequence_values::<Table>() {
            let e = e?;
            let source: String = e.get("source")?;
            let lines: Table = e.get("lines")?;
            out.push((
                source,
                lines
                    .sequence_values::<usize>()
                    .collect::<mlua::Result<_>>()?,
            ));
        }
        Ok(out)
    }

    /// Statements of a `.tl` file as `(first line, last line)` ranges: what a coverage
    /// report counts as executable. A statement counts as executed when any line of its
    /// range ran (Lua attributes a multi-line statement's instructions to several lines).
    pub fn executable_ranges(&self, file: &Path) -> Result<Vec<(usize, usize)>> {
        Ok(self.coverage_spans(file)?.0)
    }

    /// The statement ranges of [`executable_ranges`](Self::executable_ranges) and the
    /// file's named functions, from one parse: a coverage report wants both, and the
    /// second is what lets it say *which function* the missed statements belong to.
    pub fn coverage_spans(&self, file: &Path) -> Result<CoverageSpans> {
        let f: Function = self.h.get("executable_ranges")?;
        let (ranges, funcs): (Option<Table>, Option<Table>) = f.call(path_str(file))?;
        let Some(ranges) = ranges else {
            return Ok((Vec::new(), Vec::new()));
        };
        let mut out = Vec::new();
        for r in ranges.sequence_values::<Table>() {
            let r = r?;
            out.push((r.get::<usize>(1)?, r.get::<usize>(2)?));
        }
        let mut fns = Vec::new();
        if let Some(funcs) = funcs {
            for f in funcs.sequence_values::<Table>() {
                let f = f?;
                fns.push(FunctionSpan {
                    name: f.get("name")?,
                    line: f.get("y")?,
                    last: f.get("last")?,
                });
            }
        }
        Ok((out, fns))
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
        Ok(Self {
            lua,
            h,
            split: false,
        })
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

    /// Check what is on disk right now, ignoring the store and not adding to it.
    ///
    /// [`check`](Self::check) serves a module the checker already knows from its store, and
    /// the underlying `tl.check_file` returns early when the environment has the file
    /// loaded. That is what makes checking a project fast, and it is wrong for a caller that
    /// has just written the file: the answer describes the version from before the write.
    /// `htl fix` writes and then measures, and was reverting correct fixes because of it.
    ///
    /// Nothing is stored either, because the caller may be about to put the file back —
    /// leaving the result behind would have the store describing a file that no longer says
    /// that.
    ///
    /// Slower than `check`: a cold environment re-checks the modules this file requires.
    pub fn check_written(&self, file: &Path) -> Result<CheckInfo> {
        let f: Function = self.h.get("check")?;
        let opts = self.lua.create_table()?;
        opts.set("seed", false)?;
        opts.set("store", false)?;
        // `H.check(filename, env, opts)`: a nil env is a fresh one.
        let t: Table = f.call((path_str(file), mlua::Value::Nil, opts))?;
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

    /// Search paths implied by where `file` sits in the scaffold layout, in the order
    /// they are consulted: its own directory first, and for a file under `tests/` then
    /// the project root and `<root>/src` (the test runner's rule, so `htl check tests`
    /// sees what `htl test` sees).
    pub fn add_layout_paths(&self, file: &Path) -> Result<()> {
        let dir = parent_dir(file);
        let mut dirs = vec![dir.clone()];
        if dir.file_name().is_some_and(|n| n == "tests")
            && let Some(root) = dir.parent()
        {
            dirs.push(root.to_path_buf());
            let src = root.join("src");
            if src.is_dir() {
                dirs.push(src);
            }
        }
        self.add_search_paths(&dirs)
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
    ///
    /// The chunk is named after the `.tl` a `require` of this name would have found —
    /// `foo.bar` becomes `@foo/bar.tl` — because that name is what a run-time failure
    /// shows, and a reader who has only the output needs something to open. Use
    /// [`Htl::preload_at`] when the source sits somewhere else (`@scripts/util.tl`), or
    /// when there is no file at all and a bare label is the honest answer (`=htl.test`).
    pub fn preload(&self, name: &str, lua_src: &str) -> Result<()> {
        self.preload_at(name, &module_chunk_name(name), lua_src)
    }

    /// [`Htl::preload`] with the chunk name spelled out, the way [`Htl::exec`] takes one.
    /// `@<path>` is a source location and is what a host with a file should pass;
    /// `=<label>` is a literal label, for a module no file backs.
    pub fn preload_at(&self, name: &str, chunk_name: &str, lua_src: &str) -> Result<()> {
        let loader = self
            .lua
            .load(lua_src)
            .set_name(chunk_name)
            .into_function()
            .with_context(|| format!("compiling preloaded module {name}"))?;
        self.preload_table()?.set(name, loader)?;
        Ok(())
    }

    /// Register stripped bytecode (e.g. from `include_tl_bytes!`) under a module name.
    ///
    /// A chunk name is worth less here than it is to [`Htl::preload`], and the reason is
    /// worth knowing before reading a failure from an embedded module: a compiled chunk
    /// carries its own name, given when it was compiled, and `lua_load`'s name is used
    /// only for the messages loading itself produces. Stripping drops the carried name
    /// along with the line numbers, so every frame from a stripped payload reads `?` —
    /// `?: in function 'sample.greet'`. Running the `.tl` under `htl run` or `htl test`
    /// is where those frames are; a bundle keeps them with `htl build --debug`.
    pub fn preload_bytes(&self, name: &str, bytecode: &[u8]) -> Result<()> {
        let loader = self
            .lua
            .load(bytecode)
            .set_name(module_chunk_name(name))
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
        let loader = self.lua.create_function(move |_, ()| Ok(value.clone()))?;
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
        self.compile_with(name, lua_src, true)
    }

    /// Compile to bytecode; `strip` drops debug info (line numbers, local and upvalue
    /// names, and the chunk name: tracebacks then show the name given at load).
    pub fn compile_with(&self, name: &str, lua_src: &str, strip: bool) -> Result<Vec<u8>> {
        let f = self
            .lua
            .load(lua_src)
            .set_name(format!("={name}"))
            .into_function()
            .with_context(|| format!("compiling generated Lua for {name}"))?;
        Ok(f.dump(strip))
    }

    /// The Lua bytecode header this state produces (signature, version, format,
    /// `LUAC_DATA`, sizes of Instruction / Integer / Number, endianness probes): what
    /// another state must match to load this state's bytecode. Lua's own version byte
    /// is the same for every 5.4.x, so bundles carry this instead.
    pub fn fingerprint(&self) -> Result<Vec<u8>> {
        let bc = self.compile_with("fp", "return 0", true)?;
        // 4 signature + 1 version + 1 format + 6 LUAC_DATA + 3 sizes + 8 LUAC_INT + 8 LUAC_NUM
        Ok(bc.iter().take(31).copied().collect())
    }

    /// Literal `require`s of a plain Lua source, resolved through the checker's path.
    pub fn lua_requires(&self, src: &str, file: &Path) -> Result<Vec<RequireSite>> {
        let f: Function = self.h.get("lua_requires")?;
        let t: Table = f.call((src, path_str(file)))?;
        read_requires(&t)
    }

    /// Where `require(name)` resolves for the checker (`.tl`, `.d.tl` or `.lua`), and
    /// where a plain `.lua` implementation sits on the path (a `.d.tl` may only be
    /// typing it). Either may be `None`.
    pub fn resolve_module(&self, name: &str) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
        let f: Function = self.h.get("resolve_module")?;
        let (found, lua): (Option<String>, Option<String>) = f.call(name)?;
        Ok((found.map(PathBuf::from), lua.map(PathBuf::from)))
    }

    /// Every file on the search path that could answer `require(name)`, in the order the
    /// searchers consult them — so the first is the one [`resolve_module`](Self::resolve_module)
    /// answers with, and the rest are what it hides.
    ///
    /// The same walk `declaration_sites` does for the `duplicate-declaration` lint, over
    /// all three kinds rather than declarations alone: a searcher answers with the first
    /// hit and says nothing about the others, and which of two files is read is decided by
    /// a position nobody wrote down. [`resolve`] is what turns this into a report.
    pub fn module_candidates(&self, name: &str) -> Result<Vec<ModuleCandidate>> {
        let f: Function = self.h.get("module_candidates")?;
        let t: Table = f.call(name)?;
        let mut out = Vec::new();
        for c in t.sequence_values::<Table>() {
            let c = c?;
            out.push(ModuleCandidate {
                path: PathBuf::from(c.get::<String>("path")?),
                kind: ModuleKind::of(&c.get::<String>("kind")?),
                dir: PathBuf::from(c.get::<String>("dir")?),
            });
        }
        Ok(out)
    }

    /// The directories the search path consults, in order. One entry per directory,
    /// however many `package.path` templates it contributes.
    pub fn search_path_dirs(&self) -> Result<Vec<PathBuf>> {
        let f: Function = self.h.get("search_dirs")?;
        let t: Table = f.call(())?;
        Ok(t.sequence_values::<String>()
            .collect::<mlua::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    /// Install a searcher serving modules from a bundle.
    pub fn install_bundle(&self, b: &bundle::Bundle) -> Result<()> {
        // Bytecode from a Lua that disagrees with ours would fail with "bad binary
        // format" somewhere inside the first require; say what differs instead.
        // The header cannot tell one 5.4.x from another, so the htl versions go in the
        // message too: they are the only record of which Lua produced each side.
        if b.modules.iter().any(|m| m.kind == bundle::Kind::Bytecode) && !b.fingerprint.is_empty() {
            let mine = self.fingerprint()?;
            if mine != b.fingerprint {
                let built_by = if b.htl_version.is_empty() {
                    "an htl that did not record its version".to_string()
                } else {
                    format!("htl {}", b.htl_version)
                };
                bail!(
                    "bundle bytecode was compiled for {} by {built_by}, but this host runs {} on htl {}; \
                     rebuild the bundle here, or build it with --source",
                    bundle::describe_fingerprint(&b.fingerprint),
                    bundle::describe_fingerprint(&mine),
                    env!("CARGO_PKG_VERSION")
                );
            }
        }
        // Host-provided modules must already be registered, or the program's first
        // require of them fails with a message that points at the wrong place.
        let package: Table = self.lua.globals().get("package")?;
        let preload: Table = package.get("preload")?;
        let loaded: Table = package.get("loaded")?;
        let missing: Vec<&String> = b
            .host_modules
            .iter()
            .filter(|n| {
                matches!(preload.get::<Value>(n.as_str()), Ok(Value::Nil))
                    && matches!(loaded.get::<Value>(n.as_str()), Ok(Value::Nil))
            })
            .collect();
        if !missing.is_empty() {
            bail!(
                "bundle expects host-provided module(s) {} (declared only by a .d.tl or [build] host at link \
                 time): register them with preload / preload_value / htl_preload before running",
                missing
                    .iter()
                    .map(|m| format!("'{m}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        // Bundled modules become `package.preload` entries: the same place a host puts
        // its own modules, so everything that already defers to preload (a `.d.tl`
        // stepping aside for the implementation, mlua-pkg resolvers ahead of Lua's
        // searchers) sees them without knowing about bundles. A name the host preloaded
        // first is left alone: the host wins. Loaders get (modname, ":preload:") as
        // Lua's preload searcher passes them.
        for m in &b.modules {
            if !matches!(preload.get::<Value>(m.name.as_str())?, Value::Nil) {
                continue;
            }
            let payload = m.payload.clone();
            let kind = m.kind;
            let name = m.name.clone();
            let loader =
                self.lua
                    .create_function(move |lua, (modname, origin): (String, Value)| {
                        let chunk = lua.load(payload.as_slice()).set_name(format!("={name}"));
                        let f = match kind {
                            bundle::Kind::Bytecode => {
                                chunk.set_mode(ChunkMode::Binary).into_function()?
                            }
                            bundle::Kind::Source => {
                                chunk.set_mode(ChunkMode::Text).into_function()?
                            }
                        };
                        f.call::<Value>((modname, origin))
                    })?;
            preload.set(m.name.as_str(), loader)?;
        }
        Ok(())
    }

    /// Install the bundle and run its entry module with `...` = args.
    pub fn run_bundle(&self, b: &bundle::Bundle, args: &[String]) -> Result<()> {
        let entry = b
            .module(&b.entry)
            .cloned()
            .ok_or_else(|| anyhow!("entry module '{}' not in bundle", b.entry))?;
        self.install_bundle(b)?;
        self.set_arg(&b.entry, args)?;
        let chunk = self
            .lua
            .load(entry.payload.as_slice())
            .set_name(format!("={}", b.entry));
        let main: Function = match entry.kind {
            bundle::Kind::Bytecode => chunk.set_mode(ChunkMode::Binary).into_function()?,
            bundle::Kind::Source => chunk.set_mode(ChunkMode::Text).into_function()?,
        };
        let va: Variadic<String> = args.iter().cloned().collect();
        main.call::<()>(va)?;
        Ok(())
    }
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// The chunk name for a module registered without one: the `.tl` `require` would have
/// looked for, as a `@` source location. `htl.test` becomes `@htl/test.tl`, which is why
/// the test library asks for `=htl.test` instead — it ships inside the binary.
fn module_chunk_name(name: &str) -> String {
    format!("@{}.tl", name.replace('.', "/"))
}

/// A message for the people an embedding host serves: the innermost cause without Lua's
/// `stack traceback:` block. A host function's `Err(e)` surfaces as `e`'s own text; a Lua
/// `error("msg")` surfaces as `file:line: msg`.
///
/// ```text
/// sgen: content/no-date.md: front matter: 'date' is required
/// ```
/// instead of that line followed by `stack traceback: [C]: in method 'pages' ...`.
///
/// This is the answer for a program whose users did not write the Teal and cannot act on
/// its frames — a static site generator telling an author which file is missing a date.
/// It is not the answer for whoever is developing the program: see
/// [`developer_message`], which is what `htl run` and `htl test` print.
pub fn user_message(err: &anyhow::Error) -> String {
    if let Some(e) = err.downcast_ref::<mlua::Error>() {
        return user_message_lua(e);
    }
    strip_traceback(&format!("{err:#}"))
}

/// [`user_message`] for an error already held as mlua's own type, which is how a caller
/// that catches `mlua::Result` (the C ABI in `ffi`, say) has it.
pub fn user_message_lua(e: &mlua::Error) -> String {
    match e {
        mlua::Error::CallbackError { cause, .. } => user_message_lua(cause),
        mlua::Error::ExternalError(ext) => ext.to_string(),
        mlua::Error::WithContext { cause, .. } => user_message_lua(cause),
        other => strip_traceback(&other.to_string()),
    }
}

/// A message for whoever is developing the program: [`user_message`]'s innermost cause,
/// followed by Lua's `stack traceback:` block when the error carries one.
///
/// ```text
/// depth.tl:8: attempt to index a nil value (local 'c')
/// stack traceback:
///     depth.tl:8: in function 'depth.field'
///     depth.tl:12: in function 'depth.describe'
///     boom.tl:3: in main chunk
/// ```
///
/// The innermost line says a value was nil; the frames say which caller passed it, and
/// they name Teal files and Teal lines because a generated chunk is loaded under its
/// source's own name. This is what `htl run` and `htl test` print. The frames are absent
/// only where the debug information is: stripped bytecode, which is what a bundle without
/// `--debug` and `include_tl_bytes!` both hold.
pub fn developer_message(err: &anyhow::Error) -> String {
    let head = user_message(err);
    let full = match err.downcast_ref::<mlua::Error>() {
        Some(e) => e.to_string(),
        None => format!("{err:#}"),
    };
    match traceback_block(&full) {
        Some(tb) => format!("{head}\n{tb}"),
        None => head,
    }
}

/// The `stack traceback:` block of an error text, trimmed, without the newline before it.
fn traceback_block(text: &str) -> Option<&str> {
    let at = text.find("\nstack traceback:")?;
    Some(text[at + 1..].trim_end())
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
    if dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        dir.to_path_buf()
    }
}

fn read_checkinfo(t: &Table) -> Result<CheckInfo> {
    let seq = |key: &str| -> Result<Vec<String>> {
        let inner: Table = t.get(key)?;
        Ok(inner
            .sequence_values::<String>()
            .collect::<mlua::Result<_>>()?)
    };
    let requires = match t.get::<Table>("requires") {
        Ok(list) => read_requires(&list)?,
        Err(_) => Vec::new(),
    };
    let errors = seq("errors")?;
    let lints = seq("lints")?;
    let error_fixes = read_fixes(t, "error_fixes", errors.len())?;
    let lint_fixes = read_fixes(t, "lint_fixes", lints.len())?;
    let dependency_errors = match t.get::<Table>("dependency_errors") {
        Ok(list) => read_dependency_errors(&list)?,
        Err(_) => Vec::new(),
    };
    Ok(CheckInfo {
        errors,
        warnings: seq("warnings")?,
        deps: seq("deps")?.into_iter().map(PathBuf::from).collect(),
        lints,
        requires,
        error_fixes,
        lint_fixes,
        dependency_errors,
    })
}

fn read_dependency_errors(list: &Table) -> Result<Vec<DependencyError>> {
    let mut out = Vec::new();
    for e in list.sequence_values::<Table>() {
        let e = e?;
        out.push(DependencyError {
            file: PathBuf::from(e.get::<String>("file")?),
            required_by: PathBuf::from(e.get::<String>("required_by")?),
            text: e.get::<String>("text")?,
        });
    }
    Ok(out)
}

/// `fixes[i]` is a fix table or `false`; missing entries are `None`.
fn read_fixes(t: &Table, key: &str, len: usize) -> Result<Vec<Option<Fix>>> {
    let mut out = vec![None; len];
    let Ok(list) = t.get::<Table>(key) else {
        return Ok(out);
    };
    for (i, slot) in out.iter_mut().enumerate() {
        let v: Value = list.get(i + 1)?;
        if let Value::Table(f) = v {
            let applicability = match f.get::<Option<String>>("applicability")?.as_deref() {
                Some("unsafe") => Applicability::Unsafe,
                Some("suggest") => Applicability::Suggest,
                _ => Applicability::Safe,
            };
            let mut edits = Vec::new();
            if let Ok(es) = f.get::<Table>("edits") {
                for e in es.sequence_values::<Table>() {
                    let e = e?;
                    edits.push(Edit {
                        line: e.get("line")?,
                        col: e.get("col")?,
                        end_line: e.get("end_line")?,
                        end_col: e.get("end_col")?,
                        text: e.get::<Option<String>>("text")?.unwrap_or_default(),
                    });
                }
            }
            *slot = Some(Fix {
                applicability,
                edits,
            });
        }
    }
    Ok(out)
}

fn read_requires(list: &Table) -> Result<Vec<RequireSite>> {
    let mut requires = Vec::new();
    for r in list.sequence_values::<Table>() {
        let r = r?;
        requires.push(RequireSite {
            module: r.get::<String>("name")?,
            path: r.get::<Option<String>>("path")?.map(PathBuf::from),
            line: r.get::<Option<usize>>("y")?.unwrap_or(0),
            col: r.get::<Option<usize>>("x")?.unwrap_or(0),
        });
    }
    Ok(requires)
}

/// `true` for `foo.tl` but not `foo.d.tl`.
pub fn is_tl_source(p: &Path) -> bool {
    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    p.is_file() && name.ends_with(".tl") && !name.ends_with(".d.tl")
}

/// The note `htl dts` writes beside the declarations it materialises from a dependency
/// crate, in `types/<crate>/`. See [`dep_dts`].
pub const DEP_TYPES_NOTE: &str = ".htl-dts";

/// The immediate subdirectories of `types/` holding declarations materialised from a
/// dependency, in name order.
///
/// They go on the search path in their own right, so that a declaration keeps the module
/// name it was written under whatever the crate shipping it is called: `htl-mq`'s
/// `mq.d.tl` is `require("mq")`, not `require("htl-mq.mq")`. A directory a person laid out
/// under `types/` carries no note and goes on meaning what it has always meant — the path
/// below `types/` is the module name, as `socket/http.d.tl` is `require("socket.http")`.
pub fn materialised_types_dirs(types: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(types) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join(DEP_TYPES_NOTE).is_file())
        .collect();
    out.sort();
    out
}

/// `true` for `foo.d.tl`: a declaration, with the implementation somewhere else.
pub fn is_declaration(p: &Path) -> bool {
    p.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.ends_with(".d.tl"))
}

/// Directories never descended into when collecting sources under a root: build output,
/// installed packages, VCS and tool state. A root passed explicitly is always walked.
pub const SKIP_DIRS: &[&str] = &["target", "node_modules", ".mlua-pkgs", ".git"];

/// `true` for a directory entry that source collection should not enter: a name in
/// [`SKIP_DIRS`], any dot-directory, or one of `extra` — named by path rather than by
/// name, for what the caller knows and a name cannot say.
pub fn is_skipped_dir(path: &Path, extra: &[PathBuf]) -> bool {
    if !path.is_dir() {
        return false;
    }
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if SKIP_DIRS.contains(&name) || (name.starts_with('.') && name.len() > 1) {
        return true;
    }
    extra.iter().any(|e| same_file(path, e))
}

/// The two paths name the same thing on disk, `..` and symlinks resolved. Falls back to
/// comparing them as written when either cannot be canonicalised (it does not exist).
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Extra directories to skip below `root`, when `root` is inside an `mlua-pkg.toml`
/// project: where it installed its deps, and each copy a `target_dir` dep put in the tree.
///
/// Both hold a dependency's own sources and tests rather than the project's. The copies
/// need saying because they are *in* the repo and committed — nothing about the path tells
/// one apart from the project's own code beside it, and only the manifest knows. `mlua-pkg
/// install` rewrites them every time it runs, so checking one reports someone else's
/// errors, formatting it writes a diff against upstream that the next install undoes, and
/// running its tests runs a dependency's suite. Go settled the same question the same way:
/// `./...` has excluded `vendor/` since 1.9.
///
/// A `patch_dir` dep is the other case and is not here: the project owns that copy, so
/// whether to walk it depends on what the walk is for ([`patched_dirs`]).
#[cfg(feature = "pkg")]
pub fn project_skip_dirs(root: &Path) -> Vec<PathBuf> {
    match pkg::Project::find(root) {
        Some(p) => {
            let mut out = vec![p.pkgs_dir];
            out.extend(p.vendored_copies);
            out
        }
        None => Vec::new(),
    }
}

#[cfg(not(feature = "pkg"))]
pub fn project_skip_dirs(_root: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// The `patch_dir` deps below `root`: a dependency's source taken into the tree, which the
/// project edits and commits (`htl pkg patch`).
///
/// Not in [`project_skip_dirs`], because whether to walk one depends on what the walk is
/// for. Its errors are the project's to fix, so `htl check` reports them; but the change
/// it holds is a diff against the revision it was taken from, so `htl fmt` would bury that
/// change under a reformatting of every file, and its `*_test.tl` are the dependency's
/// suite rather than the project's. Those two skip it, and pass this to
/// [`collect_tl_skipping`] / [`testing::discover_tests_skipping`] to say so.
#[cfg(feature = "pkg")]
pub fn patched_dirs(root: &Path) -> Vec<PathBuf> {
    match pkg::Project::find(root) {
        Some(p) => p.patch_dirs(),
        None => Vec::new(),
    }
}

#[cfg(not(feature = "pkg"))]
pub fn patched_dirs(_root: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// Collect `.tl` sources from files and directories (sorted, recursive). Directories in
/// [`SKIP_DIRS`], dot-directories and the project's package dir are not entered unless
/// given as a root themselves.
pub fn collect_tl(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    collect_tl_skipping(paths, &[])
}

/// [`collect_tl`], not entering `skip` either — directories named by path rather than by
/// name, for what the caller knows and a name cannot say ([`patched_dirs`]).
pub fn collect_tl_skipping(paths: &[PathBuf], skip: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for p in paths {
        if p.is_dir() {
            let mut extra = project_skip_dirs(p);
            extra.extend(skip.iter().cloned());
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
