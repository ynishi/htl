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
    /// Static counterpart of `TealResolver::expect_type` / `require_fields`: files
    /// directly under `dir` must return `type`; checked by the `contract` lint.
    #[serde(default)]
    pub contract: Vec<Contract>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Contract {
    /// Directory relative to `htl.toml`, e.g. `"mods"`.
    pub dir: String,
    /// `"<module>.<Type>"`, e.g. `"defs.Mod"`.
    #[serde(rename = "type")]
    pub type_path: String,
    /// Every declared field must appear in the module's returned table literal.
    #[serde(default)]
    pub require_fields: bool,
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

impl HtlConfig {
    /// Parse `htl.toml` text.
    pub fn parse(text: &str) -> Result<Self> {
        toml::from_str(text).context("parsing htl.toml")
    }

    /// Nearest `htl.toml` at or above `start` (a file or directory). `Ok(None)` when
    /// there is none; `Err` when one exists but does not parse.
    pub fn find(start: &Path) -> Result<Option<(PathBuf, Self)>> {
        let mut dir = if start.is_dir() { start.to_path_buf() } else { crate::parent_dir(start) };
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
}

/// Combine specs in precedence order (later wins): `"+a,-b"` + `"+b"` -> `"+a,-b,+b"`.
pub fn join_specs<'a>(specs: impl IntoIterator<Item = &'a str>) -> String {
    specs
        .into_iter()
        .filter(|s| !s.trim().is_empty())
        .collect::<Vec<_>>()
        .join(",")
}
