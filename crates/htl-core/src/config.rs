//! `htl.toml`: project-level settings shared by the CLI and `include_tl!`.
//!
//! ```toml
//! [lint]
//! enable  = ["class-record", "explicit-number"]
//! disable = ["shadow-local"]
//! strict  = true            # lints are errors (htl check) / compile errors (include_tl!)
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
//! require_fields = true
//! exclude = ["defs", "modkit"] # modules in `dir` that are not held to the contract
//! # module = "Site"            # only this module name (in each dir) is held to it
//! ```
//!
//! Found by walking up from a file or directory, like `mlua-pkg.toml`. Command-line
//! flags and the `HTL_LINTS` / `HTL_LINT` environment variables take precedence over it.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONFIG_NAME: &str = "htl.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HtlConfig {
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    /// Directory relative to `htl.toml`, e.g. `"mods"`. One path segment may be `*`
    /// (`"sites/*"`): every subdirectory at that level is a contract directory.
    pub dir: String,
    /// `"<module>.<Type>"`, e.g. `"defs.Mod"`.
    #[serde(rename = "type")]
    pub type_path: String,
    /// Every declared field must appear in the module's returned table literal.
    #[serde(default)]
    pub require_fields: bool,
    /// Module names (file stems) inside `dir` that are not held to the contract, e.g.
    /// an SDK the host writes there (`defs`, `modkit`). The module that declares `type`
    /// is always exempt.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// When set, only this module name (in each matched dir) is held to the contract.
    pub module: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintConfig {
    /// Rules to turn on in addition to the defaults.
    #[serde(default)]
    pub enable: Vec<String>,
    /// Rules to turn off.
    #[serde(default)]
    pub disable: Vec<String>,
    /// `true`: lints fail `htl check` / `htl test` and `include_tl!`. `false`: advisory
    /// everywhere (including the macro, whose built-in default is strict).
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
        toml::from_str(text).context("parsing htl.toml")
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

    /// The `[lint]` section as a `+rule,-rule` spec for [`Htl::configure_lints`](crate::Htl::configure_lints).
    /// Append a command-line / env spec after it so later entries win.
    pub fn lint_spec(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for r in &self.lint.enable {
            parts.push(format!("+{r}"));
        }
        for r in &self.lint.disable {
            parts.push(format!("-{r}"));
        }
        parts.join(",")
    }

    /// Directories the checker should search, in order: `root`, `root/src`, `root/types`
    /// (hand-written `.d.tl` for modules the host provides, the DefinitelyTyped shape),
    /// then `[check] paths` (resolved against `root`, `~` expanded). Only existing dirs.
    /// A `.tl` source anywhere on the path beats a `.d.tl`, so a declaration under
    /// `types/` never shadows an implementation.
    pub fn search_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut out = vec![root.to_path_buf(), root.join("src"), root.join("types")];
        for p in &self.check.paths {
            out.push(resolve_path(root, p));
        }
        out.retain(|p| p.is_dir());
        out.dedup();
        out
    }
}

impl Contract {
    /// Concrete contract directories under `root` (expands one `*` segment). Missing
    /// directories are dropped; a literal `dir` that does not exist yields nothing.
    pub fn dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut acc = vec![root.to_path_buf()];
        for seg in self.dir.split('/').filter(|s| !s.is_empty() && *s != ".") {
            let mut next = Vec::new();
            for base in &acc {
                if seg == "*" {
                    if let Ok(rd) = std::fs::read_dir(base) {
                        let mut subs: Vec<PathBuf> = rd
                            .flatten()
                            .map(|e| e.path())
                            .filter(|p| p.is_dir() && !crate::is_skipped_dir(p, &[]))
                            .collect();
                        subs.sort();
                        next.extend(subs);
                    }
                } else {
                    let p = base.join(seg);
                    if p.is_dir() {
                        next.push(p);
                    }
                }
            }
            acc = next;
        }
        acc
    }

    /// Is a module with this name (file stem) held to the contract?
    pub fn applies_to(&self, module: &str) -> bool {
        if self
            .type_path
            .split_once('.')
            .is_some_and(|(m, _)| m == module)
        {
            return false;
        }
        if self.exclude.iter().any(|e| e == module) {
            return false;
        }
        match &self.module {
            Some(only) => only == module,
            None => true,
        }
    }
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
