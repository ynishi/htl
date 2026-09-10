//! The rules a finding can be reported under, and which of them a run has on.
//!
//! Every rule name htl prints — the ` [htl <rule>]` suffix a lint's message ends with, and
//! the `rule` field of `--format json` — is one entry of [`RULES`]. Twelve of them are
//! implemented in `lint.lua` and five in the project layer, and that difference used to
//! decide what a project could say about them: the registry was `L.DEFAULT` in `lint.lua`,
//! so `--lint` and `[lint]` knew the twelve and answered `unknown lint rule: contract` to a
//! name htl had just printed.
//!
//! The list lives on this side because both sides can read it here and only one of them
//! could read it there. The Lua side keeps no defaults of its own any more: it is handed
//! the resolved selection ([`Htl::select_lints`](crate::Htl::select_lints)), so the names a
//! project may write and the names that run cannot drift apart. What `lint.lua` still owns
//! is the *implementation* of its twelve — `tests/lint_registry.rs` holds that list to this
//! one, so a rule renamed on one side fails a test rather than going quietly silent.
//!
//! Whether a rule is on is one question; how loud it is when it fires is another, and this
//! module answers only the first. `htl check --strict` still promotes everything at once.

use crate::{Diagnostic, Severity};
use anyhow::{Result, bail};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Which half of htl implements a rule. It decides nothing a user can see — both halves
/// are configured by the same names and silenced by the same comment — and exists so the
/// selection can be handed to `lint.lua` without the rules it does not implement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// A rule of `lint.lua`, run over one file's syntax tree.
    Lua,
    /// A rule of the project layer, run over what a check resolved.
    Rust,
}

/// One rule: the name it is reported and configured under, whether a project that says
/// nothing gets it, and which half implements it.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub name: &'static str,
    pub default_on: bool,
    pub side: Side,
}

impl Rule {
    const fn on(name: &'static str, side: Side) -> Self {
        Self {
            name,
            default_on: true,
            side,
        }
    }

    const fn off(name: &'static str, side: Side) -> Self {
        Self {
            name,
            default_on: false,
            side,
        }
    }
}

/// Every rule there is, in the order `htl check --list-lints` prints them: the file-level
/// rules first, in the order `lint.lua` runs them, then the ones the project layer asks
/// once the files have been checked.
pub const RULES: &[Rule] = &[
    Rule::on("nil-index", Side::Lua),
    Rule::on("struct-fields", Side::Lua),
    Rule::on("sealed-record", Side::Lua),
    Rule::on("enum-exhaustive", Side::Lua),
    Rule::on("enum-cast", Side::Lua),
    Rule::on("enum-table", Side::Lua),
    Rule::on("union-exhaustive", Side::Lua),
    Rule::on("shadow-local", Side::Lua),
    Rule::on("no-global", Side::Lua),
    Rule::off("no-any", Side::Lua),
    Rule::off("explicit-number", Side::Lua),
    Rule::off("class-record", Side::Lua),
    // The project layer. All on by default: each describes a state a project is in by
    // accident rather than on purpose, and a project that wants one off now has a way to
    // say so where before it had none.
    Rule::on("duplicate-declaration", Side::Rust),
    Rule::on("host-module-shadowed", Side::Rust),
    Rule::on("contract", Side::Rust),
    Rule::on("contract-unenforced", Side::Rust),
    Rule::on("require-cycle", Side::Rust),
];

fn index_of(name: &str) -> Option<usize> {
    RULES.iter().position(|r| r.name == name)
}

/// The name of every rule, in [`RULES`] order. What `htl check --list-lints` prints.
pub fn rule_names() -> Vec<&'static str> {
    RULES.iter().map(|r| r.name).collect()
}

/// Which rules a run has on: the defaults, with a `+rule,-rule` spec applied over them.
///
/// The spec is what `[lint] enable` / `disable` is turned into
/// ([`HtlConfig::lint_spec`](crate::config::HtlConfig::lint_spec)), what `--lint` takes and
/// what `HTL_LINTS` carries into `include_tl!`. Later entries win, so a flag can turn back
/// on what the file turned off.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Parallel to [`RULES`].
    on: Vec<bool>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            on: RULES.iter().map(|r| r.default_on).collect(),
        }
    }
}

impl Selection {
    /// `"+no-any,-shadow-local"` over the defaults. An unknown name is an error rather
    /// than a no-op: a typo in `htl.toml` that silently turned nothing on would read
    /// exactly like a rule that found nothing.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut sel = Self::default();
        for item in spec
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
        {
            let (on, name) = match item.strip_prefix('-') {
                Some(rest) => (false, rest),
                None => (true, item.strip_prefix('+').unwrap_or(item)),
            };
            let Some(i) = index_of(name) else {
                bail!("unknown lint rule: {item}");
            };
            sel.on[i] = on;
        }
        Ok(sel)
    }

    /// Whether this run reports `name`. An unknown name is off: nothing produces one, and
    /// a caller asking about a name that is not a rule is asking about nothing.
    pub fn is_on(&self, name: &str) -> bool {
        index_of(name).is_some_and(|i| self.on[i])
    }

    /// The rules of one side and whether each is on, for a consumer that has to be handed
    /// the selection rather than ask about it — `lint.lua`, which runs its twelve from a
    /// table.
    pub fn of_side(&self, side: Side) -> impl Iterator<Item = (&'static str, bool)> + '_ {
        RULES
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.side == side)
            .map(|(i, r)| (r.name, self.on[i]))
    }
}

/// A run's rule selection together with the `-- htl: allow(...)` comments of the sources it
/// reports on: everything needed to decide whether a finding of the project layer is said.
///
/// `lint.lua` answers the same two questions for its own twelve, inside `report`. This is
/// the other half — the same allow syntax, read from the file a finding points into. The
/// mechanism was never specific to Lua rules: an allow comment needs a line number and a
/// rule name, and a project-layer finding has both.
///
/// A file is read at most once per run, and only when something was reported in it.
pub struct Lints {
    sel: Selection,
    /// file -> line -> the rules that line allows. `None` for a file that could not be
    /// read (a diagnostic anchored at a path relative to somewhere else, or at `htl.toml`).
    allows: RefCell<HashMap<PathBuf, Option<AllowedLines>>>,
}

/// The `-- htl: allow(...)` lines of one source: line number -> the rules it names.
type AllowedLines = HashMap<usize, Vec<String>>;

impl Lints {
    pub fn new(sel: Selection) -> Self {
        Self {
            sel,
            allows: RefCell::new(HashMap::new()),
        }
    }

    /// The run's selection from a `+rule,-rule` spec.
    pub fn parse(spec: &str) -> Result<Self> {
        Ok(Self::new(Selection::parse(spec)?))
    }

    pub fn selection(&self) -> &Selection {
        &self.sel
    }

    /// Whether this run reports `rule` at all. Ask before doing the work a rule needs:
    /// the contract rules type-check a module and scan a crate's Rust sources, and a run
    /// that turned them off should pay for neither.
    pub fn on(&self, rule: &str) -> bool {
        self.sel.is_on(rule)
    }

    /// The findings of `lines` this run says: the rest are a rule the run has off, or a
    /// site whose line allows the rule by name.
    ///
    /// Text with no ` [htl <rule>]` suffix is kept. Nothing the project layer produces is
    /// in that shape, and dropping a finding because its name could not be read would be
    /// the wrong way round.
    pub fn keep(&self, lines: Vec<String>) -> Vec<String> {
        lines
            .into_iter()
            .filter(|l| {
                let d = Diagnostic::parse(Severity::Lint, l);
                let Some(rule) = d.rule.as_deref() else {
                    return true;
                };
                self.on(rule) && !self.allowed(Path::new(&d.file), d.line, rule)
            })
            .collect()
    }

    /// Whether the source line the finding points at carries `-- htl: allow(<rule>)`.
    fn allowed(&self, file: &Path, line: usize, rule: &str) -> bool {
        if line == 0 || file.as_os_str().is_empty() {
            return false;
        }
        let mut cache = self.allows.borrow_mut();
        let entry = cache.entry(file.to_path_buf()).or_insert_with(|| {
            std::fs::read_to_string(file)
                .ok()
                .map(|s| collect_allows(&s))
        });
        entry
            .as_ref()
            .and_then(|m| m.get(&line))
            .is_some_and(|names| names.iter().any(|n| n == rule))
    }
}

/// The `-- htl: allow(a, b)` comments of a source, by line number.
///
/// The sibling of `collect_allows` in `lint.lua`, and it has to accept what that accepts:
/// the comment anywhere on the line, any spacing around the `htl:`, names separated by
/// commas or spaces.
fn collect_allows(src: &str) -> AllowedLines {
    let mut out: AllowedLines = HashMap::new();
    for (i, line) in src.lines().enumerate() {
        for (at, _) in line.match_indices("--") {
            let rest = line[at + 2..].trim_start();
            let Some(rest) = rest.strip_prefix("htl:") else {
                continue;
            };
            let Some(rest) = rest.trim_start().strip_prefix("allow(") else {
                continue;
            };
            let Some(end) = rest.find(')') else { continue };
            let names = rest[..end]
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            out.entry(i + 1).or_default().extend(names);
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_registered_once() {
        let mut names: Vec<&str> = rule_names();
        let n = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n, "a rule name appears twice in RULES");
    }

    #[test]
    fn a_spec_turns_rules_on_and_off_over_the_defaults() {
        let sel = Selection::parse("+no-any,-nil-index").unwrap();
        assert!(sel.is_on("no-any"));
        assert!(!sel.is_on("nil-index"));
        // Untouched rules keep their default.
        assert!(sel.is_on("shadow-local"));
        assert!(!sel.is_on("class-record"));
    }

    #[test]
    fn the_names_the_project_layer_prints_are_names_a_spec_takes() {
        for rule in [
            "require-cycle",
            "duplicate-declaration",
            "host-module-shadowed",
            "contract",
            "contract-unenforced",
        ] {
            let sel = Selection::parse(&format!("-{rule}")).unwrap();
            assert!(!sel.is_on(rule), "{rule} stayed on");
        }
    }

    #[test]
    fn an_unknown_name_is_refused_as_written() {
        let err = Selection::parse("+nil-idex").unwrap_err().to_string();
        assert_eq!(err, "unknown lint rule: +nil-idex");
    }

    #[test]
    fn an_allow_comment_names_rules_for_its_own_line() {
        let src = "local t = {}\nlocal x = t[1].y  -- htl: allow(nil-index, shadow-local)\n";
        let allows = collect_allows(src);
        assert_eq!(allows.get(&1), None);
        assert_eq!(
            allows.get(&2).unwrap(),
            &vec!["nil-index".to_string(), "shadow-local".to_string()]
        );
    }

    #[test]
    fn a_finding_is_dropped_by_the_rule_being_off() {
        let lints = Lints::parse("-require-cycle").unwrap();
        let kept = lints.keep(vec![
            "a.tl:1:1: a -> b -> a [htl require-cycle]".to_string(),
            "a.tl:2:1: x is declared more than once [htl duplicate-declaration]".to_string(),
        ]);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].contains("duplicate-declaration"));
    }
}
