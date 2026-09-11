//! `htl.toml`: project-level settings shared by the CLI and `include_tl!`.
//!
//! ```toml
//! [toolchain]
//! htl = "0.4"               # the htl command this project expects; a mismatch is refused
//!
//! [lint]
//! strict = true             # for this run, every `warn` counts as `deny` (htl check
//!                           # only); lints also fail include_tl!
//!
//! [lint.rules]              # a level per rule: allow (not reported) / warn (reported,
//! nil-index = "deny"        # advisory) / deny (reported, fails htl check)
//! class-record = "warn"
//! shadow-local = "allow"
//! "tl:hint" = "allow"       # Teal's warning kinds: quote the key, `:` is not a bare one
//!
//! [fmt]
//! indent = 3
//!
//! [check]
//! paths = ["mods", "~/.cache/tsk/sdk"]   # extra dirs the checker resolves require() from
//!
//! [[contract]]
//! dir = "mods"                 # or "sites/*" for one level of subdirectories
//! type = "defs.Mod"
//! require_fields = ["name", "monsters"]  # or `true` for every declared field
//! exclude = ["defs", "modkit"] # modules in `dir` that are not held to the contract
//! # module = "Site"            # only this module name (in each dir) is held to it
//! ```
//!
//! Found by walking up from a file or directory, like `mlua-pkg.toml`. Command-line
//! flags and the `HTL_LINTS` / `HTL_LINT` environment variables take precedence over it.

use crate::BuildTarget;
use crate::lint;
use anyhow::{Context, Result};
use semver::{Version, VersionReq};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "htl.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtlConfig {
    /// Which `htl` command the project expects. Checked once where the config is loaded,
    /// before the command reads anything else.
    #[serde(default)]
    pub toolchain: ToolchainConfig,
    #[serde(default)]
    pub lint: LintConfig,
    #[serde(default)]
    pub fmt: FmtConfig,
    #[serde(default)]
    pub check: CheckConfig,
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub fix: FixConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    /// Static counterpart of `TealResolver::expect_type` / `require_fields`: files
    /// directly under `dir` must return `type`; checked by the `contract` lint.
    #[serde(default)]
    pub contract: Vec<Contract>,
}

/// `[toolchain]` — the `htl` command a project expects to be checked by.
///
/// `Cargo.toml` already pins the `htl` *crate* a Rust host builds against, and nothing
/// pinned the command. The command is what decides whether the project checks: a default
/// lint added in a release turns a green project red on unchanged sources, and without
/// this key the first place that shows up is a teammate's terminal rather than the line
/// in this file that says which release the project moved to.
///
/// htl does not install anything — it is one binary, not a toolchain manager — so a
/// mismatch is reported and the message names `cargo install htl-cli`.
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
/// itself with `---@contract` (see [`crate::contract`]).
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// `[lint.rules]` — the level of each rule this project has an opinion about.
    ///
    /// A key is any entry of [`lint::RULES`](crate::lint::RULES) — one of htl's own rules,
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
    /// warnings and htl's lints fail `htl check`; lints also fail `include_tl!` (the macro
    /// reports Teal's warnings and builds anyway). `false`: advisory everywhere (including
    /// the macro, whose built-in default is strict), except for a rule the project set to
    /// `deny`, which fails `htl check` with or without this key.
    ///
    /// A run-wide promotion rather than a concept of its own: `strict` and a `[lint.rules]`
    /// level are the same question asked at two grains.
    ///
    /// `htl test` does not read it, by design: a test run's verdict is its tests, plus
    /// the type errors that stop a file from running at all. Warnings and lints are
    /// still reported there; `htl check` is where they are judged.
    pub strict: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FmtConfig {
    pub indent: Option<usize>,
}

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
    /// plain `htl build` produces and what every project without Rust in it is. Read from
    /// the file; the scaffold does not write it yet (see `t_htl_toml` in `htl-cli` for why).
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
        Ok(cfg)
    }

    /// Nearest `htl.toml` at or above `start` (a file or directory). `Ok(None)` when
    /// there is none; `Err` when one exists but does not parse.
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
            if path.is_file() {
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

    /// Directories the checker should search, in the order it consults them: `root`,
    /// `root/src`, `root/types` (hand-written `.d.tl` for modules the host provides, the
    /// DefinitelyTyped shape), then `[check] paths` (resolved against `root`, `~`
    /// expanded). Only existing dirs. The project's own code comes before declarations
    /// it keeps for other people's, and both come before anything supplied from outside.
    ///
    /// Put them on the path with [`Htl::add_search_paths`](crate::Htl::add_search_paths),
    /// which preserves this order; `add_path` alone prepends, so adding the list front to
    /// back reverses it.
    ///
    /// A `.tl` source anywhere on the path beats a `.d.tl`, so a declaration under
    /// `types/` never shadows an implementation, and the order only decides between two
    /// declarations of one module — which `duplicate-declaration` reports.
    pub fn search_paths(&self, root: &Path) -> Vec<PathBuf> {
        let types = root.join("types");
        let mut out = vec![root.to_path_buf(), root.join("src"), types.clone()];
        // `types/<crate>/` holding declarations materialised from that crate: on the path
        // itself, so the module keeps the name it was declared under whatever the crate
        // shipping it is called (`crate::materialised_types_dirs`). After `types/`, so a
        // declaration the project wrote by hand is the one read and the shipped one is
        // what `duplicate-declaration` reports as shadowed.
        out.extend(crate::materialised_types_dirs(&types));
        for p in &self.check.paths {
            out.push(resolve_path(root, p));
        }
        out.retain(|p| p.is_dir());
        out.dedup();
        out
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
