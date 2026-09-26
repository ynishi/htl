//! `htl.toml`: project-level settings shared by the CLI and `include_tl!`.
//!
//! ```toml
//! [toolchain]
//! htl = "0.8"               # the htl command this project expects; a mismatch is refused
//!
//! [lint]
//! strict = true             # every warn counts as deny: fails htl check, htl fix,
//!                           # htl build and include_tl! (not htl test / run / gen)
//!
//! [lint.rules]              # allow = not reported, warn = reported, deny = fails the run
//! nil-index = "deny"
//! class-record = "warn"     # allow by default: seen without failing the run
//! shadow-local = "allow"
//! "tl:hint" = "allow"       # a warning kind of the Teal compiler; quote the `:`
//!
//! [fmt]
//! indent = 3
//!
//! [layout]                  # where this project's own files live
//! source = "src"            # its .tl; "." for a flat project
//! types = "types"           # hand-written .d.tl for modules something else provides
//! tests = "tests"           # tests, and the helpers only tests may require
//!
//! [check]
//! paths = ["mods", "~/.cache/tsk/sdk"]   # extra dirs require() resolves from while checking
//!
//! [imports]
//! mathx = "dep:mathx"       # a name the project and a dependency share: which one it means
//! mathx_local = "own:mathx" # the project's own mathx, under a name of its choosing
//!
//! [build]
//! target = "bin"            # what runs this project's output: hb (the default when absent),
//!                           # bin, cdylib, window
//! # extra = ["modkit"]      # modules only a dynamic require reaches, for htl build
//! # host = ["engine"]       # a module the host provides that the project cannot see
//!
//! [[contract]]              # where this project accepts modules written outside it
//! dir = "mods"              # relative to htl.toml; "sites/*" = every subdirectory of sites/
//! # module = "Site"         # optional: only this module name (in each dir) is held to it
//! # exclude = ["defs"]      # optional: modules in dir not held to it (a helper, an SDK)
//! # enforced_by = "mods/_validate.lua"   # where the host enforces it, when the scan cannot see
//!
//! [fix]
//! # unsafe = ["no-global"]  # rules whose fix htl fix applies without --unsafe
//! # disable = ["contract"]  # rules whose fix is never applied
//!
//! [async]                   # the executor htl run / htl test run a program on
//! grace_ms = 1000           # how long a cancelled program may keep running its cleanup
//! # preempt = 1             # yield every N cancel checks, so a sibling task can run
//!
//! [lang]
//! async = true              # async / await are keywords (local async function f, async local
//!                           # x = e, await x, await f()); off, they are ordinary names
//! ```
//!
//! Found by walking up from a file or directory, like `mlua-pkg.toml`. Command-line
//! flags and the `HTL_LINTS` / `HTL_LINT` environment variables take precedence over it.

use crate::BuildTarget;
use crate::lint;
use anyhow::{Context, Result};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

/// The file name walked up for, and written by `htl new`. One name in one place, so that
/// the search, the scaffold and the error that names it cannot disagree.
pub const CONFIG_NAME: &str = "htl.toml";

/// A project's `htl.toml`, parsed.
///
/// Every section defaults, so a project may write only the one it has an opinion about and
/// a project with no file at all is this struct's [`Default`]. `deny_unknown_fields`
/// throughout: a key nobody reads is a key the writer believed in, and reporting it is the
/// only way they find out it did nothing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtlConfig {
    /// Which `htl` command the project expects. Checked once where the config is loaded,
    /// before the command reads anything else.
    #[serde(default)]
    pub toolchain: ToolchainConfig,
    /// `[lint]` — which rules this project has an opinion about, and whether what they
    /// report stops a run.
    #[serde(default)]
    pub lint: LintConfig,
    /// `[fmt]` — what `htl fmt` writes where the formatter has a choice.
    #[serde(default)]
    pub fmt: FmtConfig,
    /// `[layout]` — where this project's own files live.
    #[serde(default)]
    pub layout: LayoutConfig,
    /// `[check]` — where `require` may resolve from besides the project's own tree.
    #[serde(default)]
    pub check: CheckConfig,
    /// `[build]` — what `htl build` cannot learn from the sources alone.
    #[serde(default)]
    pub build: BuildConfig,
    /// `[fix]` — per-rule control over what `htl fix` applies.
    #[serde(default)]
    pub fix: FixConfig,
    /// `[cache]` — how `htl check` reuses what it already worked out.
    #[serde(default)]
    pub cache: CacheConfig,
    /// `[async]` — the executor `htl run` and `htl test` run a program on: how long a
    /// cancelled program may keep cleaning up, and whether a task in a CPU loop is
    /// interrupted for its siblings.
    #[serde(default, rename = "async")]
    pub async_: AsyncConfig,
    /// `[lang]` — which words of this project's Teal are htl's keywords: `async` and
    /// `await`, off unless the project says so.
    #[serde(default)]
    pub lang: LangConfig,
    /// Static counterpart of `TealResolver::expect_type` / `require_fields`: files
    /// directly under `dir` must return `type`; checked by the `contract` lint.
    #[serde(default)]
    pub contract: Vec<Contract>,
    /// `[imports]` — which module a name in the project's own `require`s means, where two
    /// modules answer to it. See [`ImportTarget`].
    #[serde(default)]
    pub imports: std::collections::BTreeMap<String, String>,
}

/// What an `[imports]` entry points a name at.
///
/// ```toml
/// [imports]
/// mathx = "dep:mathx"          # `require("mathx")` in the project means the dependency
/// mathx_local = "own:mathx"    # and the project's own `mathx` goes by another name
/// ```
///
/// A name belongs to one module, and two modules that both answer to one are an error
/// in `htl check`. Renaming one of them ends it; an entry here ends it without a rename,
/// by saying which of the two the project means and letting the other be reached under a
/// name of the project's choosing. The key covers the name and everything under it:
/// `mathx = "dep:mathx"` sends `require("mathx.vec")` to the dependency as well.
///
/// It is the project's say over its own `require`s only. A dependency's `require`s keep
/// meaning what they mean to the dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// `own:<name>`: the project's own module of that name. The key then answers to
    /// `@/<name>` in generated Lua, and only the project's own module may answer it — the
    /// dependency of the same name is what the entry was written to set aside.
    Own(String),
    /// `dep:<name>`: a module of a dependency — `<name>` starts with the dependency's name
    /// (`dep:mathx`, `dep:mathx.vec`).
    Dep(String),
}

impl ImportTarget {
    /// `own:<name>` or `dep:<name>`, a dotted module name after the colon.
    pub fn parse(text: &str) -> Result<Self> {
        let (kind, name) = text.split_once(':').unwrap_or(("", ""));
        let ok = !name.is_empty()
            && name.split('.').all(|seg| {
                !seg.is_empty()
                    && seg
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            });
        match (kind, ok) {
            ("own", true) => Ok(Self::Own(name.to_string())),
            ("dep", true) => Ok(Self::Dep(name.to_string())),
            _ => anyhow::bail!(
                "[imports] value \"{text}\" is not one: write \"own:<module>\" for the \
                 project's own module or \"dep:<module>\" for a dependency's"
            ),
        }
    }
}

impl HtlConfig {
    /// `[imports]`, each value parsed. The file was refused at parse if one is not.
    pub fn import_targets(&self) -> Vec<(String, ImportTarget)> {
        self.imports
            .iter()
            .filter_map(|(k, v)| ImportTarget::parse(v).ok().map(|t| (k.clone(), t)))
            .collect()
    }
}

/// `[toolchain]` — the `htl` command a project expects to be checked by.
///
/// `Cargo.toml` already pins the `htl` *crate* a Rust host builds against, and nothing
/// pinned the command. The command is what decides whether the project checks: a default
/// lint added in a release turns a green project red on unchanged sources — three lints
/// were added on one day and all three are reported by default, so a project quiet under
/// the release before them says three new things under the release after, fatally if it
/// runs `--strict` — and without this key the first place that shows up is a teammate's
/// terminal rather than the line in this file that says which release the project moved
/// to.
///
/// htl does not install anything — it is one binary, not a toolchain manager — so a
/// mismatch is reported and the message names `cargo install htl-cli`.
///
/// The crate and the CLI are released together, so `htl check` also prints one line when
/// the `htl = "0.8"` in the project's `Cargo.toml` does not admit the command running —
/// `htl 0.8.0; Cargo.toml asks for htl 0.7.1 — the crate and the CLI are meant to move
/// together (cargo install htl-cli --version 0.7.1, or bump the dependency)`. A warning
/// and nothing more; a `path` or `git` dependency states no version and is passed over.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainConfig {
    /// A Cargo-style requirement the running command must satisfy: `"0.4"` for 0.4.x,
    /// `"1"` for 1.x, `">=0.4.2, <0.6"` when a project needs to say more. Absent, any
    /// command runs the project, which is what every project did before the key existed.
    pub htl: Option<String>,
}

impl ToolchainConfig {
    /// The requirement, parsed. `Ok(None)` when the key is absent; `Err` when it is there
    /// and is not a requirement — which [`HtlConfig::parse`] raises with the rest of the
    /// config errors, so a typo here is found where a typo in `[lint]` is.
    pub fn req(&self) -> Result<Option<VersionReq>> {
        let Some(text) = &self.htl else {
            return Ok(None);
        };
        match VersionReq::parse(text) {
            Ok(req) => Ok(Some(req)),
            Err(e) => Err(anyhow::anyhow!(
                "[toolchain] htl = \"{text}\" is not a version requirement: {e}"
            )),
        }
    }
}

/// Refuse the run when the config names a toolchain this command is not.
///
/// `running` is the command's own `CARGO_PKG_VERSION`, passed in rather than read here so
/// that the version answered for is the binary the person invoked, not whichever crate
/// this code was compiled into.
///
/// Refusing rather than warning is the point of a pin: a warning is ignorable, and a pin
/// that can be ignored stops being one. The cost is bounded — the fix is the one line
/// this message quotes.
///
/// Matching is cargo's, pre-release rule included: `0.4.0-rc.1` does not satisfy `"0.4"`,
/// the same way it does not satisfy the `htl = "0.4"` beside it in `Cargo.toml`.
pub fn check_toolchain(cfg: &HtlConfig, path: &Path, running: &str) -> Result<()> {
    let Some(req) = cfg.toolchain.req()? else {
        return Ok(());
    };
    let version = Version::parse(running)
        .with_context(|| format!("this htl reports its version as {running}, which is not one"))?;
    if req.matches(&version) {
        return Ok(());
    }
    let text = cfg.toolchain.htl.as_deref().unwrap_or_default();
    anyhow::bail!(
        "htl {running} does not satisfy the toolchain this project asks for\n  \
         {}: [toolchain] htl = \"{text}\"\n  \
         htl installs nothing: cargo install htl-cli --version \"{text}\"",
        path.display()
    )
}

/// `[cache]` — how `htl check` reuses what it already worked out.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    /// `"per-module"` (the default) or `"whole-run"`. Which one is faster depends on where
    /// edits land in the dependency graph; the CLI's `--cache-mode` overrides this, and
    /// `--no-cache` turns the cache off entirely, which is a separate question from how it
    /// is grained.
    pub mode: Option<String>,
}

/// `require_fields` of a `[[contract]]`: which fields of the contract type a module's
/// returned table has to carry.
///
/// ```toml
/// require_fields = true                            # every declared field
/// require_fields = ["name", "monsters", "items"]   # these, so the type can grow
/// ```
///
/// The list exists because every Teal record field is nilable and Teal has no `?` for
/// record fields, so a type cannot say which of its own fields are mandatory. Without
/// it, adding a field to a contract type makes every module already written against it
/// fail, and the only way out is to stop checking.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum RequireFields {
    /// `true`: every field the type declares. `false`: no field check at all.
    All(bool),
    /// Exactly these. A name the type does not declare is an error, not a no-op.
    Named(Vec<String>),
}

impl Default for RequireFields {
    fn default() -> Self {
        Self::All(false)
    }
}

impl RequireFields {
    /// Is any field required at all?
    pub fn is_on(&self) -> bool {
        match self {
            Self::All(b) => *b,
            Self::Named(names) => !names.is_empty(),
        }
    }

    /// The names asked for, or `None` when the answer is "whatever the type declares".
    pub fn named(&self) -> Option<&[String]> {
        match self {
            Self::Named(names) => Some(names),
            Self::All(_) => None,
        }
    }
}

/// `[[contract]]` — where this project accepts modules from outside it. One line, in the
/// file a reader opens first; the shape those modules must have is declared on the record
/// itself with `---@contract` (see [`crate::contract`]). Marking the record is what makes
/// the contract discoverable: a directory carries no evidence of which of a project's
/// records is the one its modules must satisfy.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    /// Directory relative to `htl.toml`, e.g. `"mods"`. One path segment may be `*`
    /// (`"sites/*"`): every subdirectory at that level is a contract directory.
    pub dir: String,
    /// When set, only this module name (in each matched dir) is held to the contract.
    /// `---@contract(module = "…")` says the same thing on the record.
    pub module: Option<String>,
    /// Module names (file stems) inside `dir` that are not held to the contract: a
    /// helper, or an SDK the host writes there. A declaration (`.d.tl`) is never held to
    /// a contract and does not need listing; a `.tl` beside the modules does.
    /// `---@contract(exclude = "a b")` says the same thing on the record.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Where this contract is enforced at run time, when it is somewhere `htl check`
    /// cannot see: a Lua-side validator, a resolver in a sibling crate, generated code,
    /// or a resolver built by hand. Relative to `htl.toml` (`~` and absolute paths
    /// resolve as `[check] paths` does). Turns `contract-unenforced` off for this
    /// contract and no other.
    ///
    /// A path rather than a flag on purpose: the file has to exist, so the claim is one
    /// the check can hold to something, and a missing one is reported under the same
    /// rule. This is not a per-contract off switch.
    pub enforced_by: Option<String>,
}

/// `[lint]` — which rules run at what level, and whether what they report stops the run.
///
/// The two keys are the same question at two grains: [`rules`](Self::rules) names one rule,
/// [`strict`](Self::strict) promotes every `warn` of a run at once.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// `[lint.rules]` — the level of each rule this project has an opinion about.
    ///
    /// A key is any entry of [`crate::lint::RULES`] — one of htl's own rules,
    /// or one of the vendored Teal compiler's warning kinds under its `tl:` prefix
    /// (`"tl:hint"`, `"tl:unused"`, ..., which have to be quoted because `:` is not a bare
    /// TOML key). `htl check --list-lints` prints them all with their defaults. A value is
    /// `"allow"`, `"warn"` or `"deny"`. An unknown name or level is refused rather than
    /// ignored: a typo that turned nothing on would read exactly like a rule that found
    /// nothing, and a misspelt `"deny"` would read like a run that passed.
    ///
    /// One place per rule says everything about that rule. The `enable` / `disable` lists
    /// this replaced said it in two places that had to be read together, and neither could
    /// say what a rule was worth. A rule this table does not name keeps its default level.
    #[serde(default)]
    pub rules: std::collections::BTreeMap<String, lint::Level>,
    /// `true`: every finding this run reports at `warn` counts as `deny`, so Teal's
    /// warnings and htl's lints fail `htl check`, `htl fix`, `htl build` and the macros.
    /// `false`, or
    /// absent: advisory everywhere, except for a rule the project set to `deny`, which
    /// fails all of them with or without this key. One default for the command and the
    /// build ([`crate::verdict`]); `HTL_LINT` overrides it for a build.
    ///
    /// A run-wide promotion rather than a concept of its own: `strict` and a `[lint.rules]`
    /// level are the same question asked at two grains.
    ///
    /// `htl test` does not read it, by design: a test run's verdict is its tests, plus
    /// the type errors that stop a file from running at all. `htl run` and `htl gen` are
    /// the same with their program ([`crate::verdict::Policy::ERRORS_ONLY`]). Warnings and
    /// lints are still reported there; `htl check` is where they are judged.
    pub strict: Option<bool>,
}

/// `[fmt]` — what `htl fmt` writes where the formatter has a choice.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmtConfig {
    /// Spaces per level of indentation. `None` leaves the formatter's own default, which
    /// is 3 — what `tl` itself writes, and what the scaffold puts in a new project's
    /// `htl.toml` so that the number is visible rather than assumed. `--indent` overrides
    /// it for one run.
    pub indent: Option<usize>,
}

/// `[layout]` — where this project's own files live.
///
/// These directories were constants until now: `src/` and `types/` were written into
/// [`search_paths`](HtlConfig::search_paths) beside the project root, and a project that
/// kept its code somewhere else had no way to say so. A constant is not a default — the
/// reader cannot see it, and nobody can disagree with it — and every layout question htl
/// answers starts here, so this is the section that answers them.
///
/// Each value is **one directory, not a list**. A module name resolves to exactly one
/// file, so a root that answers a name has to be the only root that could; a list would
/// put htl back in the business of deciding which of two files a name means.
/// [`CheckConfig::paths`] is a list because it is the other layer — directories holding
/// modules this project did not write, reached through a contract.
///
/// Relative to `htl.toml`. `"."` is a flat project, where the sources sit beside the
/// config rather than under a directory of their own.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayoutConfig {
    /// The project's own `.tl`. Default `src`.
    #[serde(default = "default_source")]
    pub source: String,
    /// Hand-written `.d.tl` for modules something else provides at run time, the
    /// DefinitelyTyped shape. Default `types`.
    ///
    /// Four kinds of declaration arrive here, wherever this key puts the directory: the
    /// ones written by hand; the ones a Rust dependency ships (`types/<crate>/`,
    /// [`crate::dep_dts`]); the ones a Lua dependency published (`htl pkg install` copies
    /// them in); and the ones for a library that published none of its own (`htl types
    /// add`). Only the first are anyone's to edit — the rest are copies, and a change to
    /// one belongs in the crate or package it came from.
    #[serde(default = "default_types")]
    pub types: String,
    /// The project's tests and the helpers only tests may `require`. Default `tests`.
    ///
    /// A file here is not a test by being here: a test is a file that loads the test
    /// library ([`crate::testing::discover_tests_for`]), and one that does not is a
    /// helper. What the directory decides is who may read it — a test sees the project's
    /// sources and this directory, the sources do not see this directory.
    #[serde(default = "default_tests")]
    pub tests: String,
}

fn default_source() -> String {
    "src".to_string()
}

fn default_types() -> String {
    "types".to_string()
}

fn default_tests() -> String {
    "tests".to_string()
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            source: default_source(),
            types: default_types(),
            tests: default_tests(),
        }
    }
}

/// `[check]` — where `require` may resolve from besides the project's own tree.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckConfig {
    /// Extra directories `require` resolves from during checking (CLI, `include_tl!`,
    /// and the checker behind `TealResolver::for_contract`). Relative to `htl.toml`;
    /// absolute and `~/` paths allowed. Use it for modules the host supplies at run time
    /// from somewhere else (an SDK cache, a mods dir).
    #[serde(default)]
    pub paths: Vec<String>,
}

/// `[build]`: what `htl build` cannot learn from literal `require`s alone.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
    /// Modules to bundle even though no literal `require` reaches them (targets of a
    /// dynamic `require(expr)`).
    #[serde(default)]
    pub extra: Vec<String>,
    /// Modules the host provides at run time, besides those declared only by a `.d.tl`.
    #[serde(default)]
    pub host: Vec<String>,
    /// What runs this project's output; absent means [`BuildTarget::Hb`], which is what
    /// plain `htl build` produces and what every project without Rust in it is. Written by
    /// `htl new --target <name>` when the project's htl pin reads this key (see
    /// `htl new --target <name>`), read by every command that loads the file.
    /// `htl build` refuses a project whose target is not `hb`.
    #[serde(default)]
    pub target: Option<BuildTarget>,
}

/// `[fix]`: per-rule control over what `htl fix` applies.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixConfig {
    /// Rules whose `unsafe` fix is applied as if it were safe (e.g. `["no-global"]`).
    #[serde(default, rename = "unsafe")]
    pub unsafe_: Vec<String>,
    /// Rules whose fix is never applied.
    #[serde(default)]
    pub disable: Vec<String>,
}

/// `[lang]`: what the project's Teal is written in, beyond Teal. One key today:
///
/// ```toml
/// [lang]
/// async = true
/// ```
///
/// makes `async` and `await` keywords for the project (`htl check`, `run`, `test`, `gen`,
/// `fmt`, `fix`, and `include_tl!`, which reads the same file): `local async function f`,
/// `async local x = e` (a child task, `x: Task<T>`), `await x` (its value), `await f(x)`
/// (the marker on a call of an async function). Off, the default, both words are the
/// ordinary names they are in Teal, and a file that uses them as names checks and runs as
/// it always did. On, they are still names where a name is the only reading — after `.`
/// or `:`, before `:` `=` `,` `)` `.` `(` — so `t.await`, `x:await()`, `{ await = 1 }` and a
/// record field `await:` stay what they were.
///
/// A project turns it on knowing that a Teal language server does not know the two words
/// and reports a syntax error on them; the checker is htl's, and it reads them.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LangConfig {
    /// `async` and `await` are keywords. Absent: no.
    #[serde(default, rename = "async")]
    pub async_: Option<bool>,
}

impl LangConfig {
    /// Whether `async` / `await` are keywords for this project.
    pub fn async_on(&self) -> bool {
        self.async_.unwrap_or(false)
    }
}

/// `[async]`: the executor a program runs on (`htl run`, `htl test`, and
/// [`Htl::run_async`](crate::Htl::run_async) in a host that applies it), which is
/// mlua-isle's [`Config`](mlua_isle::runtime::Config) with htl's defaults.
///
/// A program runs as a root coroutine under a cancel token, and a cancel is a Lua error
/// raised at the next hook check or at the next await. What happens then is the grace:
///
/// - `grace_ms = 0`: the coroutine is dropped at once. Its `__close` handlers run but
///   cannot await (a Lua 5.4 rule for a close run at drop time), and a host future the
///   program was awaiting is released at that moment.
/// - `grace_ms > 0` (the default, one second): the cancel arrives as an error first and
///   unwinds normally, so a `<close>` handler runs and may await — an unwrapped host
///   function waits out its work, one the macro wrapped in `cancellable` (every
///   `#[host_module]` `async fn`) returns the cancel error at once. When the grace ends
///   the coroutine is dropped as above. The grace is one deadline for the program and
///   every task under it: cleanup that starts tasks does not extend it.
///
/// `preempt` is off by default. On, the running task is yielded every `preempt` cancel
/// checks (a check is every 1000 Lua instructions), so a sibling task on the same runtime
/// — one that would cancel it, say — gets to run while it is in a CPU loop. The cost is
/// that tasks then interleave at points the program did not mark, which is what requiring
/// an explicit await is meant to rule out; off, a CPU-bound task cannot be cancelled by a
/// sibling on the same runtime, only from another thread (Ctrl-C in `htl run`, an
/// `ffi::Interrupt` in a host).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AsyncConfig {
    /// How long a cancelled program may keep running its cleanup before it is dropped,
    /// in milliseconds. Absent: 1000.
    #[serde(default)]
    pub grace_ms: Option<u64>,
    /// Yield the running task every this many cancel checks (a check is every 1000 Lua
    /// instructions — the unit is checks, not instructions), so that other tasks on the
    /// same thread get to run while it is in a CPU loop. Absent: never.
    #[serde(default)]
    pub preempt: Option<u32>,
}

/// The grace a state gets when no `[async]` says otherwise: what
/// [`AsyncConfig::runtime`] applies for an absent `grace_ms`, and what every state is
/// attached with, so a host that never touches the config gets the same cancel semantics
/// `htl run` has.
pub const DEFAULT_GRACE_MS: u64 = 1000;

impl AsyncConfig {
    /// The section as mlua-isle's [`Config`](mlua_isle::runtime::Config), defaults
    /// applied.
    pub fn runtime(&self) -> mlua_isle::runtime::Config {
        mlua_isle::runtime::Config {
            grace: std::time::Duration::from_millis(self.grace_ms.unwrap_or(DEFAULT_GRACE_MS)),
            preempt_every: self.preempt,
        }
    }
}

impl HtlConfig {
    /// Parse `htl.toml` text.
    pub fn parse(text: &str) -> Result<Self> {
        let cfg: Self = toml::from_str(text)
            .map_err(
                |e| match (moved_contract_key(text), removed_lint_lists(text)) {
                    // `type` / `require_fields` / `exclude` moved onto the record itself, and
                    // the serde message for an unknown key does not say where they went.
                    (Some(k), _) => anyhow::anyhow!(
                        "[[contract]] {k} moved onto the type: mark the record \
                     `---@contract` and its mandatory fields `---@required`, and leave \
                     `dir` (with `module` / `exclude` if you use them) here"
                    ),
                    // `enable` / `disable` became a level per rule. The message writes the
                    // replacement out of this file's own names, so the fix is a paste.
                    (_, Some(msg)) => anyhow::anyhow!("{msg}"),
                    _ => anyhow::Error::from(e),
                },
            )
            .context("parsing htl.toml")?;
        // Here rather than at the comparison: a requirement that is not one is a fact
        // about the file, so it is reported when the file is read and by every reader of
        // it, including the one that never compares versions.
        cfg.toolchain.req().context("parsing htl.toml")?;
        // Likewise: a directory claimed by two keys that mean different things is a
        // contradiction the file states, so no source has to be read to find it.
        cfg.source_dir_is_the_project_s_alone()
            .context("parsing htl.toml")?;
        // And an `[imports]` value that points nowhere is wrong in the file itself.
        for v in cfg.imports.values() {
            ImportTarget::parse(v).context("parsing htl.toml")?;
        }
        Ok(cfg)
    }

    /// The source directory claimed once, by the key that means "this project wrote it".
    ///
    /// `[layout] source` says a module under it is the project's own — checked with the
    /// project, generated, bundled into the artefact. `[layout] types` and each `[check]
    /// paths` entry say the opposite: a module found there is somebody else's, declared
    /// or supplied rather than built. One directory cannot be both, and nothing later
    /// could pick — which of two answers a name gets is the thing htl is trying to stop
    /// deciding by accident.
    ///
    /// `types` appearing in `[check] paths` is *not* refused. Those two make the same
    /// claim, so saying it twice says nothing new; a project that lists the directory it
    /// would have got anyway is redundant, not wrong. `[layout] tests` is compared with
    /// nothing: only `source` makes the claim the others contradict.
    ///
    /// Nothing here touches the filesystem. The contradiction is in the file, so it is
    /// reported when the file is parsed, before a single source is read.
    ///
    /// Spelling does not hide it: `lib`, `./lib` and `./lib/.` compare equal, the way
    /// [`search_paths`](Self::search_paths) resolves them. An absolute entry is compared
    /// only with other absolute ones — there is no root here to resolve a relative one
    /// against, and `search_paths` drops the duplicate entry it would otherwise make.
    fn source_dir_is_the_project_s_alone(&self) -> Result<()> {
        let source = without_cur_dir(Path::new(&self.layout.source));
        let claimed = |dir: &str, by: &str| -> Result<()> {
            if without_cur_dir(Path::new(dir)) != source {
                return Ok(());
            }
            anyhow::bail!(
                "[layout] source and {by} are the same directory (\"{dir}\"): the first \
                 says a module there is this project's own and the second says it is \
                 somebody else's, and nothing later could tell which. Give them different \
                 directories, or drop the key that should take its default"
            )
        };
        claimed(&self.layout.types, "[layout] types")?;
        for p in &self.check.paths {
            claimed(p, &format!("the [check] paths entry \"{p}\""))?;
        }
        Ok(())
    }

    /// Nearest `htl.toml` at or above `start` (a file or directory). `Ok(None)` when
    /// there is none; `Err` when one exists but does not parse.
    ///
    /// A `htl.toml` inside a directory an enclosing project declares as a dependency's —
    /// a `patch_dir`, a `target_dir` — is passed over, and the walk goes on to the
    /// project's own. `htl pkg patch` copies a dependency's package root whole, config
    /// file included, and the copy is code the project owns rather than a project of its
    /// own: one root, one store, one lint selection over the whole tree, the patched
    /// directories with it. The question is `pkg::owning_project`'s, asked here and by
    /// [`Project::find`](crate::pkg::MluaProject::find) so that the manifest and the config
    /// cannot disagree about where the root is.
    pub fn find(start: &Path) -> Result<Option<(PathBuf, Self)>> {
        let mut dir = if start.is_dir() {
            start.to_path_buf()
        } else {
            crate::parent_dir(start)
        };
        if let Ok(abs) = std::fs::canonicalize(&dir) {
            dir = abs;
        }
        loop {
            let path = dir.join(CONFIG_NAME);
            // Without `pkg` there is no manifest to read and no `patch_dir` to respect,
            // so the nearest `htl.toml` is the whole answer — what this was before #275,
            // for a build that has no projects (#288).
            #[cfg(feature = "pkg")]
            let owned_by_a_project = crate::pkg::owning_project(&dir).is_some();
            #[cfg(not(feature = "pkg"))]
            let owned_by_a_project = false;
            if path.is_file() && !owned_by_a_project {
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let cfg = Self::parse(&text).with_context(|| path.display().to_string())?;
                return Ok(Some((path, cfg)));
            }
            if !dir.pop() {
                return Ok(None);
            }
        }
    }

    /// The `[lint.rules]` table as a `rule=level` spec for
    /// [`Htl::configure_lints`](crate::Htl::configure_lints). Append a command-line / env
    /// spec after it so later entries win.
    ///
    /// The spec is also part of a cache key, so the rendering is ordered (the table is a
    /// `BTreeMap`): two runs that say the same thing have to produce the same string.
    pub fn lint_spec(&self) -> String {
        self.lint
            .rules
            .iter()
            .map(|(rule, level)| format!("{rule}={level}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Directories the checker should search, in the order it consults them: `root`, the
    /// source directory, the types directory (hand-written `.d.tl` for modules the host
    /// provides, the DefinitelyTyped shape), the `types/<crate>/` directories materialised
    /// under it, then `[check] paths` (resolved against `root`, `~` expanded). Only
    /// existing dirs. The project's own code comes before declarations it keeps for other
    /// people's, and both come before anything supplied from outside. Between two
    /// declarations of one module that order is the whole rule — the project's own, then
    /// the ones crates ship, then `[check] paths` — and the first is read: a hand-written
    /// `types/mq.d.tl` beside a shipped `types/htl-mq/mq.d.tl` is the one in effect, and
    /// the shipped one is what `duplicate-declaration` reports as shadowed. (The model,
    /// which every command resolves through, puts a dependency's declarations between
    /// the project's own and the crates'; see `model::Project::load`.)
    ///
    /// `root` is here for the legacy path-based callers (`Htl::apply_config`) and for
    /// `pkg::contract_resolvers`. The model does not search it: a `.tl` beside `htl.toml`
    /// is the project's only when `[layout] source = "."` says the sources are there
    /// ([`marker_roots`](Self::marker_roots) applies the same rule).
    ///
    /// The two middle entries are [`LayoutConfig`]'s, `src` and `types` unless the
    /// project says otherwise. They were constants here until that section existed.
    ///
    /// Put them on the path with [`Htl::add_search_paths`](crate::Htl::add_search_paths),
    /// which preserves this order; `add_path` alone prepends, so adding the list front to
    /// back reverses it.
    ///
    /// A `.tl` source anywhere on the path beats a `.d.tl`, so a declaration under
    /// `types/` never shadows an implementation, and the order only decides between two
    /// declarations of one module — which `duplicate-declaration` reports.
    pub fn search_paths(&self, root: &Path) -> Vec<PathBuf> {
        let types = resolve_path(root, &self.layout.types);
        let mut out = vec![
            root.to_path_buf(),
            resolve_path(root, &self.layout.source),
            types.clone(),
        ];
        // `types/<crate>/` holding declarations materialised from that crate: on the path
        // itself, so the module keeps the name it was declared under whatever the crate
        // shipping it is called (`crate::materialised_types_dirs`). After `types/`, so a
        // declaration the project wrote by hand is the one read and the shipped one is
        // what `duplicate-declaration` reports as shadowed.
        out.extend(crate::materialised_types_dirs(&types));
        for p in &self.check.paths {
            out.push(resolve_path(root, p));
        }
        // `source = "."` is a flat project, and `root.join(".")` is a second spelling of
        // `root` that `dedup` would keep and every later string comparison would read as
        // a different directory. Dropping the `.` components makes one directory one
        // entry however the config spelled it (`lib`, `./lib`, `./lib/.`).
        for p in &mut out {
            *p = without_cur_dir(p);
        }
        out.retain(|p| p.is_dir());
        out.dedup();
        out
    }

    /// Where the project declares its contracts: the roots of the modules the project
    /// model makes of this config — the source root, the declaration root and the
    /// directories materialised under it, each `[check] paths` entry — in
    /// [`search_paths`](Self::search_paths)' order. The project root is among them only
    /// when it is the source root (`[layout] source = "."`): otherwise it is no module's
    /// root, and a file there is not one of the project's modules.
    ///
    /// A function of the config rather than of a loaded [`crate::model::Project`] so that
    /// a host built with `pkg` alone, which has no model, looks for markers in the same
    /// places the model does (`pkg::contract_resolvers`).
    pub fn marker_roots(&self, root: &Path) -> Vec<PathBuf> {
        let source = without_cur_dir(&resolve_path(root, &self.layout.source));
        let top = without_cur_dir(root);
        self.search_paths(root)
            .into_iter()
            .filter(|p| *p != top || *p == source)
            .collect()
    }
}

/// The first `[[contract]]` key that used to live in `htl.toml` and now lives on the
/// record, if the text still carries one. A scan of the lines after a `[[contract]]`
/// header, which is enough to tell a stale config from an unrelated typo.
fn moved_contract_key(text: &str) -> Option<&'static str> {
    let mut in_contract = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_contract = line.starts_with("[[contract]]");
            continue;
        }
        if !in_contract {
            continue;
        }
        for k in ["type", "require_fields"] {
            if line
                .strip_prefix(k)
                .is_some_and(|r| r.trim_start().starts_with('='))
            {
                return Some(k);
            }
        }
    }
    None
}

/// The message for a config that still writes `[lint] enable` / `disable`, or `None` when
/// it does not.
///
/// The keys are gone rather than deprecated: `HtlConfig` is `deny_unknown_fields`, so a
/// removed key fails loudly instead of being read as "no rules configured", which is the
/// behaviour to want for a key that used to decide what a run reports. What serde says on
/// its own — `unknown field \`enable\`` — is true and not actionable, so this writes the
/// replacement table out of the file's own names: `enable` said "report it", which is
/// `warn`, and `disable` said "do not", which is `allow`.
fn removed_lint_lists(text: &str) -> Option<String> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let lint = table.get("lint")?.as_table()?;
    let names = |key: &str| -> Vec<String> {
        lint.get(key)
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (enabled, disabled) = (names("enable"), names("disable"));
    let present: Vec<&str> = ["enable", "disable"]
        .into_iter()
        .filter(|k| lint.contains_key(*k))
        .collect();
    if present.is_empty() {
        return None;
    }
    let mut lines = vec![format!(
        "[lint] {} replaced by a level per rule. Write instead:\n\n  [lint.rules]",
        present.join(" and ")
    )];
    // The `#` in one column, so the block pastes as it reads.
    let width = enabled
        .iter()
        .chain(&disabled)
        .map(|r| r.len())
        .max()
        .unwrap_or(0);
    for (rules, level, was) in [
        (&enabled, lint::Level::Warn, "enable"),
        (&disabled, lint::Level::Allow, "disable"),
    ] {
        for rule in rules {
            // Every name is quoted: `tl:*` has to be, and one spelling reads better than
            // two in the same block.
            let assign = format!(
                "\"{rule}\"{:pad$} = \"{level}\"",
                "",
                pad = width - rule.len()
            );
            // The longest assignment is the widest name at the longest level word
            // (`"allow"`, seven characters with its quotes and three for the ` = `).
            lines.push(format!("  {assign:<w$}  # was in {was}", w = width + 12));
        }
    }
    if enabled.is_empty() && disabled.is_empty() {
        lines.push("  \"nil-index\" = \"deny\"".to_string());
    }
    lines.push(String::new());
    lines.push(
        "allow = not reported, warn = reported and advisory, deny = reported and fails \
         the run (htl check --list-lints lists every rule with its default)"
            .to_string(),
    );
    Some(lines.join("\n"))
}

/// Combine specs in precedence order (later wins): `"+a,-b"` + `"+b"` -> `"+a,-b,+b"`.
pub fn join_specs<'a>(specs: impl IntoIterator<Item = &'a str>) -> String {
    specs
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// `p` with its `.` components dropped, so that one directory has one spelling: `lib`,
/// `./lib` and `./lib/.` all come back as `lib`, and `.` comes back empty.
///
/// [`Path::components`] already drops a `.` anywhere but the front, and the front is
/// where `htl.toml` most often has one.
pub(crate) fn without_cur_dir(p: &Path) -> PathBuf {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect()
}

/// `~/x` -> `$HOME/x`; relative -> under `root`; absolute as is.
pub fn resolve_path(root: &Path, p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    let pb = PathBuf::from(p);
    if pb.is_absolute() { pb } else { root.join(pb) }
}
