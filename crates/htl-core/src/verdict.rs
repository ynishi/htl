//! Whether a run fails: the findings of the run, and the policy that judges them.
//!
//! A run says things — errors, Teal's warnings, htl's lints — and each thing it says at
//! `warn` or `deny` carries a rule whose level the project chose ([`crate::lint::Level`]).
//! Whether all of that adds up to a failure is one question, and this module is the one
//! place it is answered: [`verdict`] over [`Findings`] and a [`Policy`].
//!
//! The answer, in full: **an error fails the run; so does a finding at `deny`; and under
//! `strict` every finding the run reported counts as `deny`.** An error is htl being unable
//! to stand behind the code, so no level reaches it. `strict` is the run-wide form of a
//! level rather than a concept of its own — everything a run reports is at `warn` or
//! `deny` (`allow` is not reported), so promoting `warn` leaves nothing advisory.
//!
//! The policy is built once per run, from `htl.toml` and the command line
//! ([`Policy::resolve`]), so that the default lives here and not in each command. A command
//! that judges differently on purpose says so as a field of [`Policy`], not with a
//! predicate of its own.
//!
//! One default for every command and for the macros: a finding is advice until a rule is
//! at `deny` or the run is `strict`. The macros used to fail a build on any lint, levels
//! unread; a build path stricter than the checker is the shape that broke builds
//! elsewhere (Rust's `#![deny(warnings)]` under a new compiler, a bundler that failed on
//! lints the editor called warnings), and the answer that held there is this one — one
//! severity model, and strictness a knob the invoker turns ([`Policy::with_env`]).

use crate::Diagnostic;
use crate::config::HtlConfig;
use crate::lint::{Level, Selection};

/// What a run said, counted: the input to [`verdict`].
///
/// The counts are of what reached the output — a dependency's error said once for the
/// thirty files that require it is one error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Findings {
    /// Errors: the file's own, and the modules it required (Teal errors, htl's
    /// project-level errors such as a name with two owners).
    pub errors: usize,
    /// Teal's warnings, under the rule names htl gives them.
    pub warnings: usize,
    /// htl's lints, the project-level ones included.
    pub lints: usize,
    /// How many of `warnings` and `lints` were said under a rule at `deny`. It overlaps the
    /// two counts rather than adding to them: it is the part of what was said that fails.
    pub denied: usize,
}

/// How a run judges its findings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Policy {
    /// Every finding the run reports counts as `deny` (`[lint] strict`, `--strict`,
    /// `HTL_LINT=deny`).
    pub strict: bool,
    /// No finding fails the run, whatever its level: only an error does. `HTL_LINT=warn`,
    /// the environment's way to let a build through that a project's `deny` would stop —
    /// a cap on levels, as `--cap-lints` is for rustc, rather than a second default.
    pub capped: bool,
}

impl Policy {
    /// The policy of a command whose verdict is its program: `htl run` and `htl gen`. They
    /// fail on an error, which stops the program from being generated at all, and report
    /// every warning and lint without judging it — `htl check` is where those are judged,
    /// as `htl test`'s verdict is its tests. Named here so the difference is one value in
    /// one place rather than a command that never asks.
    pub const ERRORS_ONLY: Self = Self {
        strict: false,
        capped: true,
    };

    /// The policy `htl.toml` and the command line give: `strict` when either says so, and
    /// advisory otherwise. The flag can only raise it — a `--strict` run of a project that
    /// wrote `strict = false` is strict — because the flag is the question asked now and
    /// the file is the default for when nobody asks.
    pub fn resolve(config: Option<&HtlConfig>, strict_flag: bool) -> Self {
        Self {
            strict: strict_flag || config.and_then(|c| c.lint.strict).unwrap_or(false),
            capped: false,
        }
    }

    /// This policy with `HTL_LINT` applied, given its value (`None` when unset): `deny`
    /// makes the run strict, `warn` caps it. Anything else is refused rather than read as
    /// one of the two — a misspelt `deny` would otherwise be a build that passed.
    pub fn with_env(self, htl_lint: Option<&str>) -> Result<Self, String> {
        match htl_lint {
            None => Ok(self),
            Some("deny") => Ok(Self {
                strict: true,
                capped: false,
            }),
            Some("warn") => Ok(Self {
                strict: false,
                capped: true,
            }),
            Some(other) => Err(format!(
                "HTL_LINT={other:?}: expected \"warn\" (no finding fails) or \"deny\" (every \
                 finding fails)"
            )),
        }
    }
}

impl Findings {
    /// The findings of a run that holds them as values: Teal's `warnings` and htl's
    /// `lints`, each counted as `denied` too when its rule is at `deny` in `levels`. What a
    /// caller with no [`Sink`](crate::project::Sink) counts, by the reading the sink uses.
    pub fn of(warnings: &[Diagnostic], lints: &[Diagnostic], levels: &Selection) -> Self {
        Self {
            errors: 0,
            warnings: warnings.len(),
            lints: lints.len(),
            denied: warnings
                .iter()
                .chain(lints)
                .filter(|d| is_denied(d, levels))
                .count(),
        }
    }
}

/// Whether `d` was said under a rule at `deny` in `levels`. An error is judged as an error
/// whatever its rule: an error's rule is the class its fix is filed under, not a level.
pub fn is_denied(d: &Diagnostic, levels: &Selection) -> bool {
    d.severity != crate::Severity::Error
        && d.rule
            .as_deref()
            .is_some_and(|rule| levels.level_of(rule) == Level::Deny)
}

/// Whether a run with `findings`, judged by `policy`, fails.
pub fn verdict(findings: &Findings, policy: &Policy) -> bool {
    findings.errors > 0
        || (!policy.capped
            && (findings.denied > 0
                || (policy.strict && (findings.warnings > 0 || findings.lints > 0))))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADVISORY: Policy = Policy {
        strict: false,
        capped: false,
    };
    const STRICT: Policy = Policy {
        strict: true,
        capped: false,
    };
    const CAPPED: Policy = Policy {
        strict: false,
        capped: true,
    };

    fn f(errors: usize, warnings: usize, lints: usize, denied: usize) -> Findings {
        Findings {
            errors,
            warnings,
            lints,
            denied,
        }
    }

    #[test]
    fn nothing_said_passes_under_either_policy() {
        assert!(!verdict(&Findings::default(), &ADVISORY));
        assert!(!verdict(&Findings::default(), &STRICT));
    }

    #[test]
    fn an_error_fails_whatever_the_policy() {
        assert!(verdict(&f(1, 0, 0, 0), &ADVISORY));
        assert!(verdict(&f(1, 0, 0, 0), &STRICT));
    }

    #[test]
    fn a_warn_finding_is_advice_unless_strict() {
        assert!(!verdict(&f(0, 1, 0, 0), &ADVISORY));
        assert!(!verdict(&f(0, 0, 1, 0), &ADVISORY));
        assert!(verdict(&f(0, 1, 0, 0), &STRICT));
        assert!(verdict(&f(0, 0, 1, 0), &STRICT));
    }

    #[test]
    fn a_deny_finding_fails_without_strict() {
        // One lint, said under a rule at `deny`: it is both a lint and the denied part.
        assert!(verdict(&f(0, 0, 1, 1), &ADVISORY));
        assert!(verdict(&f(0, 1, 0, 1), &ADVISORY));
    }

    #[test]
    fn strict_comes_from_the_flag_or_the_file_and_the_flag_only_raises_it() {
        let file = |strict: Option<bool>| {
            let mut c = HtlConfig::default();
            c.lint.strict = strict;
            c
        };
        assert_eq!(Policy::resolve(None, false), ADVISORY);
        assert_eq!(Policy::resolve(None, true), STRICT);
        assert_eq!(Policy::resolve(Some(&file(None)), false), ADVISORY);
        assert_eq!(Policy::resolve(Some(&file(Some(true))), false), STRICT);
        assert_eq!(Policy::resolve(Some(&file(Some(false))), true), STRICT);
        assert_eq!(Policy::resolve(Some(&file(Some(false))), false), ADVISORY);
    }

    #[test]
    fn htl_lint_raises_caps_or_is_refused() {
        assert_eq!(ADVISORY.with_env(None), Ok(ADVISORY));
        assert_eq!(ADVISORY.with_env(Some("deny")), Ok(STRICT));
        assert_eq!(STRICT.with_env(Some("warn")), Ok(CAPPED));
        assert!(ADVISORY.with_env(Some("error")).is_err());
    }

    #[test]
    fn a_cap_lets_findings_through_and_never_an_error() {
        assert!(!verdict(&f(0, 1, 1, 2), &CAPPED));
        assert!(verdict(&f(1, 0, 0, 0), &CAPPED));
    }

    #[test]
    fn findings_count_deny_by_each_ones_rule() {
        let levels = crate::lint::Lints::parse("require-cycle=deny")
            .unwrap()
            .selection()
            .clone();
        let lints = vec![
            Diagnostic::parse(crate::Severity::Lint, "a.tl:1:1: x [htl require-cycle]"),
            Diagnostic::parse(crate::Severity::Lint, "a.tl:2:1: y [htl nil-index]"),
        ];
        let found = Findings::of(&[], &lints, &levels);
        assert_eq!(found, f(0, 0, 2, 1));
        assert!(verdict(&found, &ADVISORY));
    }
}
