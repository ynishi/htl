//! The rules a finding can be reported under, and which of them a run has on.
//!
//! Every rule name htl prints — the ` [htl <rule>]` suffix a finding's message ends with,
//! and the `rule` field of `--format json` — is one entry of [`RULES`]. Thirteen of them are
//! implemented in `lint.lua`, five in the project layer and seven by the vendored Teal
//! compiler, and that difference used to decide what a project could say about them: the
//! registry was `L.DEFAULT` in `lint.lua`, so `--lint` and `[lint]` knew the thirteen and
//! answered `unknown lint rule: contract` to a name htl had just printed.
//!
//! The list lives on this side because both sides can read it here and only one of them
//! could read it there. The Lua side keeps no defaults of its own any more: it is handed
//! the resolved selection ([`Htl::select_lints`](crate::Htl::select_lints)), so the names a
//! project may write and the names that run cannot drift apart. What `lint.lua` still owns
//! is the *implementation* of its thirteen — `tests/lint_registry.rs` holds that list to this
//! one, so a rule renamed on one side fails a test rather than going quietly silent.
//!
//! Whether a rule is on and how much it matters are one question here, answered by a
//! [`Level`]: `allow` is not reported, `warn` is reported, `deny` is reported and fails the
//! run. A rule's default is a level ([`Rule::default`]) and a project overrides it by name
//! (`[lint.rules]`, `--lint`), which is what lets one rule be advice while another stops
//! the run. `strict` is not a fourth thing: it promotes every `warn` of the run to `deny`.
//!
//! Not every entry of [`RULES`] is a name a run can turn off. Two of them —
//! `forward-ref` and `tl:error` — are the classes `htl fix` gives to a Teal error that has
//! no rule of its own: read by `htl fix --rule` and `[fix] disable`, consulted by nothing
//! in a check. So each entry says which surfaces it appears on ([`Surfaces`]), and the two
//! sides ask different questions of the list. What the listing prints and a spec takes
//! ([`rule_defaults`], [`rule_names`], [`Selection::parse`]) is the rules a check reports
//! under; what a fix filter takes ([`fix_rule_names`], [`check_fix_rules`]) is all of them,
//! since a fix travels with the diagnostic of whatever named it. Without the split,
//! `--list-lints` would print a name no level applies to and `--lint -forward-ref` would
//! parse into an off switch nothing reads.
//!
//! A rule can also be renamed, and [`RENAMED`] is where the old spelling is kept so that a
//! project which wrote it is told what to write instead. There is one entry: bare `error`
//! became `tl:error` when it joined the identity space, because `error` as a name collides
//! with everything and Teal's errors belong in the same namespace as its warnings. The
//! fix filters match by string, so without the entry `--rule error` would select nothing
//! and report that it fixed nothing — which reads exactly like a project with nothing to
//! fix.

use crate::{Diagnostic, Severity};
use anyhow::{Result, bail};
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Which part of htl produces a finding under a rule. It decides nothing a user can see —
/// all three are configured by the same names and silenced by the same comment — and
/// exists so that a selection can be handed to a producer without the rules it does not
/// produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// A rule of `lint.lua`, run over one file's syntax tree.
    Lua,
    /// A rule of the project layer, run over what a check resolved.
    Rust,
    /// A warning kind of the vendored Teal compiler, reported under the name Teal gives
    /// it. htl decides whether it is said; it does not decide what it says.
    Tl,
}

/// How much a finding under a rule matters: whether it is said, and whether the run fails
/// on it.
///
/// The three words are selene's `[lints]` and Cargo's, in that spelling, because a reader
/// arriving from either already knows them. Nothing defaults to [`Deny`](Self::Deny) — the
/// levels a project gets without writing anything are `warn` for every rule htl reports
/// and `allow` for the three that are opinions — so `deny` is the thing a project asks
/// for, and `strict` is asking for it run-wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Not reported, and so not counted: a run is judged on what it said.
    Allow,
    /// Reported; the run does not fail on it. What every rule htl reported was, before
    /// levels: `--strict` was the only way to make one matter.
    Warn,
    /// Reported, and the run fails. `htl check` exits 1 on a finding at this level with no
    /// flag needed.
    Deny,
}

impl Level {
    /// The word a config and a spec write it as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Warn => "warn",
            Self::Deny => "deny",
        }
    }

    /// A level from the word, or `None` for anything else.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" => Some(Self::Allow),
            "warn" => Some(Self::Warn),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    /// Whether a finding at this level is reported at all.
    pub fn is_on(self) -> bool {
        self != Self::Allow
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which of htl's two rule-name surfaces a rule appears on.
///
/// Two things in htl take a rule by name and they do not take the same set. One is the
/// lint surface — `htl check --list-lints`, `[lint.rules]`, `--lint`, `HTL_LINTS`, and the
/// `-- htl: allow(...)` comment — which answers "is this said, and does it stop the run".
/// The other is the fix surface — `htl fix --rule`, `[fix] disable`, `[fix] unsafe` —
/// which answers "may a tool rewrite this". The two questions are independent (a level
/// never decides applicability and applicability never decides a level), and `[lint]` and
/// `[fix]` stay two tables; what this says is only *which names each table may contain*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surfaces {
    /// Both. A rule some check reports under: the listing prints it with its default
    /// level, a spec sets that level, an allow comment silences one occurrence — and a
    /// fix filter names it too, because a finding under it may carry a fix.
    LintAndFix,
    /// The fix surface alone. A class `htl fix` gives to a Teal error that has no rule of
    /// its own: nothing in a check reports under the name, so there is no level for a
    /// project to set, nothing for the listing to print and nothing an allow comment
    /// could silence. `htl fix --rule` and `[fix] disable` still have to name it, which is
    /// why it is a registered rule rather than a bare string in `fix.rs`.
    FixOnly,
}

/// One rule: the name it is reported and configured under, the level a project that says
/// nothing about it gets, which half implements it, and where the name is taken.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub name: &'static str,
    /// What a check says under this name for a project that configures nothing. A
    /// [`Surfaces::FixOnly`] rule is [`Level::Allow`], which is not a placeholder: no
    /// check reports under it, so "nothing is said under this name" is the true answer,
    /// and it is the answer nothing can change — the listing does not print it and a spec
    /// refuses to set it.
    pub default: Level,
    pub side: Side,
    pub surfaces: Surfaces,
}

impl Rule {
    /// Reported, and advisory until a project says `deny` or the run says `strict`.
    const fn warn(name: &'static str, side: Side) -> Self {
        Self {
            name,
            default: Level::Warn,
            side,
            surfaces: Surfaces::LintAndFix,
        }
    }

    /// Not reported until a project asks for it. An opinion htl has, rather than a state
    /// a project is in by accident.
    const fn allow(name: &'static str, side: Side) -> Self {
        Self {
            name,
            default: Level::Allow,
            side,
            surfaces: Surfaces::LintAndFix,
        }
    }

    /// A name only `htl fix` uses: the class it files an error's fix under when the error
    /// has no rule to file it under.
    const fn fix_class(name: &'static str, side: Side) -> Self {
        Self {
            name,
            default: Level::Allow,
            side,
            surfaces: Surfaces::FixOnly,
        }
    }

    /// Whether the listing prints this rule and a spec may set its level.
    pub fn is_lint(&self) -> bool {
        matches!(self.surfaces, Surfaces::LintAndFix)
    }
}

/// Every rule there is. The first twenty-four are the lint surface, in the order
/// `htl check --list-lints` prints them: the file-level rules first, in the order
/// `lint.lua` runs them, then the ones the project layer asks once the files have been
/// checked, then the warning kinds the vendored Teal compiler reports for itself. The last
/// two are `htl fix`'s error classes, which the listing does not print — see [`Surfaces`].
pub const RULES: &[Rule] = &[
    Rule::warn("nil-index", Side::Lua),
    Rule::warn("nil-return", Side::Lua),
    Rule::warn("struct-fields", Side::Lua),
    Rule::warn("sealed-record", Side::Lua),
    Rule::warn("enum-exhaustive", Side::Lua),
    Rule::warn("enum-cast", Side::Lua),
    Rule::warn("enum-table", Side::Lua),
    Rule::warn("union-exhaustive", Side::Lua),
    Rule::warn("shadow-local", Side::Lua),
    Rule::warn("no-global", Side::Lua),
    Rule::allow("no-any", Side::Lua),
    Rule::allow("explicit-number", Side::Lua),
    Rule::allow("class-record", Side::Lua),
    // The project layer. All `warn`: each describes a state a project is in by accident
    // rather than on purpose, so it is worth saying, and none of them is worth failing a
    // run over unless the project says so — which is what a level is for.
    Rule::warn("duplicate-declaration", Side::Rust),
    Rule::warn("host-module-shadowed", Side::Rust),
    Rule::warn("contract", Side::Rust),
    Rule::warn("contract-unenforced", Side::Rust),
    Rule::warn("require-cycle", Side::Rust),
    // Teal's warning kinds, kept in the compiler's own vocabulary behind a `tl:` prefix.
    // The prefix is not decoration. `unused` already means something else here — `htl
    // unused` reports modules nothing requires, not locals nothing reads — and these seven
    // words are Teal's to rename, not htl's; keeping them in a namespace of their own says
    // where they came from and leaves htl's thirteen free of them.
    //
    // All `warn`, which is what they have always been: the compiler raises them as
    // warnings and htl forwarded them as warnings long before it could name them. There is
    // one severity to inherit, so the level says the same thing with nothing added — see
    // the withdrawn fourth level in the umbrella issue.
    Rule::warn("tl:unknown", Side::Tl),
    Rule::warn("tl:unused", Side::Tl),
    Rule::warn("tl:unread", Side::Tl),
    Rule::warn("tl:redeclaration", Side::Tl),
    Rule::warn("tl:branch", Side::Tl),
    Rule::warn("tl:hint", Side::Tl),
    Rule::warn("tl:debug", Side::Tl),
    // The classes `htl fix` files an error's fix under. A Teal error carries no rule of
    // its own — the compiler does not name its errors the way it names its warning kinds —
    // so `--rule` and `[fix] disable` would have nothing to say about one. These two are
    // that name. `forward-ref` is the class htl recognises by its message (a record key
    // used before the function that defines it); `tl:error` is every other error, in the
    // same namespace as the compiler's warnings because it is the same compiler speaking.
    //
    // Both are `Side::Tl` — the compiler is what produced the diagnostic — and neither is
    // a lint: no check reports under either name, and `--list-lints` and `[lint.rules]`
    // say so by not having them.
    Rule::fix_class("forward-ref", Side::Tl),
    Rule::fix_class("tl:error", Side::Tl),
];

/// Old spellings and what they are called now: consulted only to answer a project that
/// wrote one, never to accept it.
///
/// `error` was the class of every fixable Teal error that was not a forward reference,
/// and it was a name a project could write into `--rule` and `[fix] disable`. Renaming it
/// is a breaking change made on purpose, so what it must not be is a *silent* one: the
/// filters match by string, and an unrecognised name would simply match nothing and
/// report that nothing was fixed.
pub const RENAMED: &[(&str, &str)] = &[("error", "tl:error")];

/// The rules of the lint surface, with their position in a [`Selection`].
fn lint_rules() -> impl Iterator<Item = (usize, &'static Rule)> {
    RULES.iter().filter(|r| r.is_lint()).enumerate()
}

/// Where `name` sits in a [`Selection`], or `None` when it is not a rule of the lint
/// surface (a fix class, or not a rule at all).
fn index_of(name: &str) -> Option<usize> {
    lint_rules().find(|(_, r)| r.name == name).map(|(i, _)| i)
}

/// The name of every rule of the lint surface, in [`RULES`] order.
pub fn rule_names() -> Vec<&'static str> {
    lint_rules().map(|(_, r)| r.name).collect()
}

/// Every rule of the lint surface with the level a project that says nothing gets, in
/// [`RULES`] order. What `htl check --list-lints` prints, which is the one place a reader
/// sees which rules are `allow` without having to fail to provoke one.
pub fn rule_defaults() -> Vec<(&'static str, Level)> {
    lint_rules().map(|(_, r)| (r.name, r.default)).collect()
}

/// The names `htl fix --rule`, `[fix] disable` and `[fix] unsafe` take: every rule there
/// is. A fix travels with a diagnostic, so any rule that can report can carry one, and the
/// two classes exist for the errors that report under no rule.
///
/// A rule with no fix to its name is accepted here and matches nothing — `--rule
/// require-cycle` is a run that fixes nothing rather than an error — because which rules
/// carry fixes is a fact about today's implementations, not about the vocabulary.
pub fn fix_rule_names() -> Vec<&'static str> {
    RULES.iter().map(|r| r.name).collect()
}

/// Refuse the names a fix filter was given that no fix could ever be filed under.
///
/// `surface` is what the message calls the filter (`htl fix --rule`, `[fix] disable`,
/// `[fix] unsafe`). A name that was renamed is answered with the name it has now: this is
/// the only place that knows the old spelling meant something, and a project reading
/// "unknown" about a word that used to work would be left guessing.
pub fn check_fix_rules(names: &[String], surface: &str) -> Result<()> {
    for name in names {
        if RULES.iter().any(|r| r.name == name.as_str()) {
            continue;
        }
        if let Some((_, now)) = RENAMED.iter().find(|(old, _)| *old == name.as_str()) {
            bail!(
                "{surface}: `{name}` is now `{now}` — Teal's errors are named in the `tl:` \
                 namespace, as its warnings are"
            );
        }
        bail!(
            "{surface}: unknown rule `{name}`. Takes any rule `htl check --list-lints` \
             names, or one of the classes an error's fix is filed under: {}",
            fix_classes().join(", ")
        );
    }
    Ok(())
}

/// The names that exist on the fix surface and nowhere else.
pub fn fix_classes() -> Vec<&'static str> {
    RULES
        .iter()
        .filter(|r| !r.is_lint())
        .map(|r| r.name)
        .collect()
}

/// What level each rule has for a run: the defaults, with a spec applied over them.
///
/// The spec is what `[lint.rules]` is turned into
/// ([`HtlConfig::lint_spec`](crate::config::HtlConfig::lint_spec)), what `--lint` takes and
/// what `HTL_LINTS` carries into `include_tl!`. Later entries win, so a flag can raise or
/// lower what the file set.
#[derive(Debug, Clone)]
pub struct Selection {
    /// Parallel to the lint surface of [`RULES`] — a fix class has no level to carry, so
    /// there is no slot for one here either.
    levels: Vec<Level>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            levels: lint_rules().map(|(_, r)| r.default).collect(),
        }
    }
}

impl Selection {
    /// `"no-any=warn,nil-index=deny"` over the defaults.
    ///
    /// `+rule` and `-rule` are the shorthand the flag has always taken, and they say the
    /// same thing a level does: `+` is `=warn` (say it) and `-` is `=allow` (do not).
    /// A bare name is `+name`.
    ///
    /// An unknown name is an error rather than a no-op: a typo in `htl.toml` that
    /// silently turned nothing on would read exactly like a rule that found nothing. So
    /// is an unknown level, for the same reason — a misspelt `deny` that meant "fail the
    /// run" must not read as "everything passed".
    ///
    /// A name that is a rule but not one of this surface ([`Surfaces::FixOnly`]) is
    /// refused too, and says so rather than saying "unknown": the name exists, and what a
    /// reader needs to hear is that a level is not a thing it has.
    pub fn parse(spec: &str) -> Result<Self> {
        let mut sel = Self::default();
        for item in spec
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|s| !s.is_empty())
        {
            let (name, level) = match item.split_once('=') {
                Some((name, word)) => {
                    let Some(level) = Level::parse(word.trim()) else {
                        bail!("unknown lint level: {item} (allow, warn or deny)");
                    };
                    (name.trim(), level)
                }
                None => match item.strip_prefix('-') {
                    Some(rest) => (rest, Level::Allow),
                    None => (item.strip_prefix('+').unwrap_or(item), Level::Warn),
                },
            };
            let Some(i) = index_of(name) else {
                if RULES.iter().any(|r| r.name == name) {
                    bail!(
                        "not a lint rule: {name} is the class `htl fix` files an error's \
                         fix under, taken by `htl fix --rule` and `[fix] disable`. No \
                         check reports under it, so it has no level"
                    );
                }
                bail!("unknown lint rule: {item}");
            };
            sel.levels[i] = level;
        }
        Ok(sel)
    }

    /// The level `name` has for this run. An unknown name is [`Level::Allow`]: nothing
    /// produces one, and a caller asking about a name that is not a rule is asking about
    /// nothing.
    pub fn level_of(&self, name: &str) -> Level {
        index_of(name).map_or(Level::Allow, |i| self.levels[i])
    }

    /// Whether this run reports `name` at all — `allow` is the only level that does not.
    pub fn is_on(&self, name: &str) -> bool {
        self.level_of(name).is_on()
    }

    /// The rules of one side and whether each is on, for a consumer that has to be handed
    /// the selection rather than ask about it — `lint.lua`, which runs its thirteen from a
    /// table. Fix classes are not among them: no producer produces one, so there is
    /// nothing to tell a producer about them.
    ///
    /// A producer is told whether to produce and not how much it matters: the level of
    /// what it produced is read where the run is judged ([`Lints::level`]), so a rule
    /// moving between `warn` and `deny` changes no producer's work.
    pub fn of_side(&self, side: Side) -> impl Iterator<Item = (&'static str, bool)> + '_ {
        lint_rules()
            .filter(move |(_, r)| r.side == side)
            .map(|(i, r)| (r.name, self.levels[i].is_on()))
    }
}

/// A run's rule selection together with the `-- htl: allow(...)` comments of the sources it
/// reports on: everything needed to decide whether a finding of the project layer is said.
///
/// `lint.lua` answers the same two questions for its own thirteen, inside `report`. This is
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

    /// The level `rule` has for this run: what decides whether a finding under it fails
    /// the run, once it has been decided that the finding is said at all.
    pub fn level(&self, rule: &str) -> Level {
        self.sel.level_of(rule)
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

    /// The defaults are the levels a project that writes nothing gets, and they are what
    /// the run before levels behaved as: everything htl reported was advisory, and the
    /// three opinions were not reported. Nothing is `deny`, so no project's verdict
    /// changed when levels arrived.
    #[test]
    fn the_defaults_are_warn_except_the_three_opinions() {
        for (name, level) in rule_defaults() {
            let want = match name {
                "no-any" | "explicit-number" | "class-record" => Level::Allow,
                _ => Level::Warn,
            };
            assert_eq!(level, want, "{name}");
        }
    }

    #[test]
    fn a_spec_sets_a_level_by_name() {
        let sel = Selection::parse("nil-index=deny,tl:hint=allow,no-any=warn").unwrap();
        assert_eq!(sel.level_of("nil-index"), Level::Deny);
        assert_eq!(sel.level_of("tl:hint"), Level::Allow);
        assert_eq!(sel.level_of("no-any"), Level::Warn);
        // Untouched rules keep their default level.
        assert_eq!(sel.level_of("shadow-local"), Level::Warn);
        assert_eq!(sel.level_of("class-record"), Level::Allow);
    }

    /// `+`/`-` are the older spelling and say the same thing, so a `HTL_LINTS` already in
    /// someone's CI keeps its meaning.
    #[test]
    fn the_plus_and_minus_spelling_is_the_same_as_a_level() {
        let short = Selection::parse("+no-any,-nil-index").unwrap();
        let long = Selection::parse("no-any=warn,nil-index=allow").unwrap();
        for (name, _) in rule_defaults() {
            assert_eq!(short.level_of(name), long.level_of(name), "{name}");
        }
    }

    #[test]
    fn later_entries_win_so_a_flag_can_raise_what_a_file_set() {
        let sel = Selection::parse("nil-index=allow,nil-index=deny").unwrap();
        assert_eq!(sel.level_of("nil-index"), Level::Deny);
    }

    /// A misspelt level must not read as "nothing to report": it is refused as written,
    /// the way an unknown name is.
    #[test]
    fn an_unknown_level_is_refused_as_written() {
        let err = Selection::parse("nil-index=error").unwrap_err().to_string();
        assert_eq!(
            err,
            "unknown lint level: nil-index=error (allow, warn or deny)"
        );
        assert!(Selection::parse("no-such-rule=deny").is_err());
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
    fn a_teal_warning_kind_is_a_name_a_spec_takes() {
        // The `:` is inside the name, not a separator: a spec splits on commas and
        // whitespace only, so `-tl:hint` is one item naming one rule.
        let sel = Selection::parse("-tl:hint,-tl:unused").unwrap();
        assert!(!sel.is_on("tl:hint"));
        assert!(!sel.is_on("tl:unused"));
        assert!(sel.is_on("tl:redeclaration"), "the rest keep their default");
        // And the prefix is load-bearing: htl has no rule called `hint`.
        assert!(Selection::parse("-hint").is_err());
    }

    #[test]
    fn an_unknown_name_is_refused_as_written() {
        let err = Selection::parse("+nil-idex").unwrap_err().to_string();
        assert_eq!(err, "unknown lint rule: +nil-idex");
    }

    /// The two surfaces do not take the same names any more. The listing is the smaller
    /// set, and every name in it round-trips through a spec — which is the property the
    /// registry exists for, held here for whatever the listing prints rather than for a
    /// list written out by hand.
    #[test]
    fn the_listing_prints_fewer_names_than_a_fix_filter_takes() {
        let listed = rule_names();
        let fixable = fix_rule_names();
        for name in &listed {
            assert!(fixable.contains(name), "{name} is not a name a fix takes");
            Selection::parse(&format!("{name}=deny"))
                .unwrap_or_else(|e| panic!("the listing prints {name} and a spec refuses it: {e}"));
        }
        let only_fix: Vec<&str> = fixable
            .iter()
            .copied()
            .filter(|n| !listed.contains(n))
            .collect();
        assert_eq!(only_fix, fix_classes());
        assert_eq!(only_fix, ["forward-ref", "tl:error"], "{only_fix:?}");
    }

    /// A fix class is refused by a spec, and told apart from a typo: the name exists, and
    /// what a reader needs to hear is that a level is not something it has.
    #[test]
    fn a_fix_class_has_no_level_to_set() {
        for name in fix_classes() {
            let err = Selection::parse(&format!("-{name}"))
                .unwrap_err()
                .to_string();
            assert!(err.starts_with("not a lint rule: "), "{err}");
            assert!(
                err.contains(name) && err.contains("htl fix --rule"),
                "{err}"
            );
            // And it is not silently on: nothing produces one, so nothing reports one.
            assert!(!Selection::default().is_on(name), "{name}");
        }
    }

    #[test]
    fn a_fix_filter_takes_a_class_and_a_rule_and_refuses_a_typo() {
        let names = |s: &[&str]| s.iter().map(|n| (*n).to_string()).collect::<Vec<_>>();
        check_fix_rules(
            &names(&["tl:error", "forward-ref", "no-global"]),
            "[fix] disable",
        )
        .unwrap();
        let err = check_fix_rules(&names(&["forwardref"]), "htl fix --rule")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("htl fix --rule: unknown rule `forwardref`"),
            "{err}"
        );
        // The message says where to look, and names the classes the listing will not.
        assert!(
            err.contains("--list-lints") && err.contains("tl:error"),
            "{err}"
        );
    }

    /// The rename is breaking and says so. A project that wrote `error` is told the name
    /// it has now, rather than being told it is unknown — or, worse, being told nothing
    /// and shown a run that fixed nothing.
    #[test]
    fn the_old_error_spelling_names_what_replaced_it() {
        for surface in ["htl fix --rule", "[fix] disable", "[fix] unsafe"] {
            let err = check_fix_rules(&["error".to_string()], surface)
                .unwrap_err()
                .to_string();
            assert_eq!(
                err,
                format!(
                    "{surface}: `error` is now `tl:error` — Teal's errors are named in the \
                     `tl:` namespace, as its warnings are"
                )
            );
        }
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
