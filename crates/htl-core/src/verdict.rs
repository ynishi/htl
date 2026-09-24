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

use crate::config::HtlConfig;

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
    /// Every finding the run reports counts as `deny` (`[lint] strict`, `--strict`).
    pub strict: bool,
}

impl Policy {
    /// The policy `htl.toml` and the command line give: `strict` when either says so, and
    /// advisory otherwise. The flag can only raise it — a `--strict` run of a project that
    /// wrote `strict = false` is strict — because the flag is the question asked now and
    /// the file is the default for when nobody asks.
    pub fn resolve(config: Option<&HtlConfig>, strict_flag: bool) -> Self {
        Self {
            strict: strict_flag || config.and_then(|c| c.lint.strict).unwrap_or(false),
        }
    }
}

/// Whether a run with `findings`, judged by `policy`, fails.
pub fn verdict(findings: &Findings, policy: &Policy) -> bool {
    findings.errors > 0
        || findings.denied > 0
        || (policy.strict && (findings.warnings > 0 || findings.lints > 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADVISORY: Policy = Policy { strict: false };
    const STRICT: Policy = Policy { strict: true };

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
}
