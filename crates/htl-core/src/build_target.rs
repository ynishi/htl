//! [`BuildTarget`]: what runs the output of an htl project.
//!
//! One enum, three entries, and everything else about a target derived from it: the crate
//! types cargo is told to build ([`BuildTarget::crate_types`]), whether the project has an
//! entry script ([`BuildTarget::entry`]), and the sentence that names the thing on the far
//! end ([`BuildTarget::runs_it`]). `htl.toml` records the choice as `[build] target`, and
//! the scaffolder in `htl-cli` is a consumer of this type rather than the place it is
//! defined.

use serde::{Deserialize, Deserializer};
use std::fmt;
use std::str::FromStr;

/// **A build target is what runs htl's output.** The `htl` binary runs a `.hb` bundle; the
/// OS runs a native binary; a caller written in C, Python or C# loads a `cdylib`. That is
/// the axis, and it is the only thing the entries below differ about — README
/// "[Build targets](https://github.com/ynishi/htl#build-targets---target-name)" has the
/// table of what each one produces.
///
/// # What is in the enum, and what is derived from it
///
/// The enum is over *what runs the output*, and nothing else. Cargo's `[lib] crate-type`
/// and the entry-script rule are answers derived from an entry ([`crate_types`],
/// [`entry`]), not fields stored beside it, so adding a target is adding one arm and the
/// answers it gives rather than a row of parallel data that can disagree with itself. A
/// platform or a target triple is deliberately not in here: `bin` on Linux and `bin` on
/// Windows are the same build target, and if a platform ever has to be named it is an
/// attribute of one target, not a fourth entry.
///
/// [`crate_types`]: BuildTarget::crate_types
/// [`entry`]: BuildTarget::entry
///
/// # Why it is not called a host
///
/// Two of the three entries happen to be Rust crates, which is why this used to be called
/// a host; an output nothing Rust runs — a `.love` bundle, say — is the entry that makes
/// that word plainly wrong.
///
/// *Host* keeps its own meaning throughout htl and is not this: it is the Rust side that
/// embeds the Lua state — `#[host_module]`, the `src/host.d.tl` generated from it, and
/// `[build] host` / `htl build --host x,y`, which name the modules that side provides at
/// run time. A project can have a host and the default target (`htl build` alone), and two
/// targets can share one host, so they are two axes rather than two words for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildTarget {
    /// A `.hb` bundle, run by the `htl` binary. What plain `htl build` produces, and what
    /// every project without Rust in it is.
    Hb,
    /// A native binary, run by the OS: a library crate holding the host module and the
    /// embedded scripts, with a thin binary on top.
    Bin,
    /// A C ABI library, loaded by a caller that is not written in Rust.
    Cdylib,
}

impl BuildTarget {
    /// Every target there is, in the order they are offered and reported.
    pub const ALL: &'static [BuildTarget] =
        &[BuildTarget::Hb, BuildTarget::Bin, BuildTarget::Cdylib];

    /// How it is spelled on the command line (`--target <name>`) and in `htl.toml`.
    pub fn name(&self) -> &'static str {
        match self {
            BuildTarget::Hb => "hb",
            BuildTarget::Bin => "bin",
            BuildTarget::Cdylib => "cdylib",
        }
    }

    /// Every name, in [`ALL`](BuildTarget::ALL) order: what a flag offers and what a typo
    /// is answered with.
    pub fn names() -> Vec<&'static str> {
        BuildTarget::ALL.iter().map(BuildTarget::name).collect()
    }

    /// `[lib] crate-type = [...]` beyond the default `rlib`, which needs no section at all.
    /// Derived rather than stored: the crate shape is a consequence of what loads the
    /// output, so it is answered here and `Cargo.toml` has nothing else to know about it.
    pub fn crate_types(&self) -> &'static [&'static str] {
        match self {
            // No Rust crate at all; `htl build` writes the bundle.
            BuildTarget::Hb => &[],
            // The default `rlib`, plus the binary cargo builds from `src/main.rs`.
            BuildTarget::Bin => &[],
            // The shared object a caller loads, and the static library Unity on iOS links;
            // the second costs one more artefact and nothing else.
            BuildTarget::Cdylib => &["rlib", "cdylib", "staticlib"],
        }
    }

    /// What this target has to say about `src/main.tl`.
    pub fn entry(&self) -> Script {
        match self {
            BuildTarget::Hb => Script::Either,
            BuildTarget::Bin => Script::Either,
            // A `cdylib` has no entry point of its own, and the caller that loads it brings
            // its own `main`.
            BuildTarget::Cdylib => Script::Forbids,
        }
    }

    /// Who is on the far end, in the words the README's table and `htl new`'s refusals use.
    pub fn runs_it(&self) -> &'static str {
        match self {
            BuildTarget::Hb => "the htl binary",
            BuildTarget::Bin => "the OS, as a binary",
            BuildTarget::Cdylib => "a C / Python / Unity caller",
        }
    }
}

impl fmt::Display for BuildTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for BuildTarget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        BuildTarget::ALL
            .iter()
            .copied()
            .find(|t| t.name() == s)
            .ok_or_else(|| {
                format!(
                    "unknown target `{s}`; registered targets: {}",
                    BuildTarget::names().join(", ")
                )
            })
    }
}

/// The string form is the only form: `target = "cdylib"` in `htl.toml` goes through
/// [`FromStr`], so a name that is not one is refused with the same sentence the command
/// line answers a typo with, rather than with serde's list of variant spellings.
impl<'de> Deserialize<'de> for BuildTarget {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// What a target has to say about `src/main.tl`. `--lib` is the user's side of the same
/// question, and the two are reconciled once, where the scaffold resolves a target, before
/// anything is written.
// `Requires` is the half no target has yet — #104's window loop is the one that will — so
// until then the code that reads it is exercised by this module's tests.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Script {
    /// The target runs an entry script and cannot be built without one (a window loop).
    Requires,
    /// The target is a library for someone else to call and has no entry point (a C ABI).
    Forbids,
    /// Either shape works; `--lib` decides.
    Either,
}

impl Script {
    /// Does this rule accept a project built with `--lib` (`lib = true`), or without one?
    pub fn accepts(self, lib: bool) -> bool {
        match self {
            Script::Requires => !lib,
            Script::Forbids => lib,
            Script::Either => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildTarget, Script};

    /// The name is the only spelling there is, and it round-trips: what `htl.toml` records
    /// is what `--target` takes, for every entry, with nothing spelled twice.
    #[test]
    fn every_target_round_trips_through_its_name() {
        for t in BuildTarget::ALL {
            assert_eq!(t.name().parse::<BuildTarget>().unwrap(), *t);
            assert_eq!(t.to_string(), t.name());
        }
        assert_eq!(BuildTarget::names(), vec!["hb", "bin", "cdylib"]);
    }

    /// A name that is not one is answered with the ones that are — the same sentence
    /// whether it arrived on the command line or in `htl.toml`.
    #[test]
    fn an_unknown_name_is_refused_with_all_three() {
        let e = "rust".parse::<BuildTarget>().unwrap_err();
        assert!(e.contains("unknown target `rust`"), "{e}");
        for n in BuildTarget::names() {
            assert!(e.contains(n), "{e}");
        }
    }

    /// The crate shape follows from what loads the output: only the C ABI target needs a
    /// `[lib]` section, and it needs all three of those types.
    #[test]
    fn crate_types_are_derived_from_the_target() {
        assert!(BuildTarget::Hb.crate_types().is_empty());
        assert!(BuildTarget::Bin.crate_types().is_empty());
        assert_eq!(
            BuildTarget::Cdylib.crate_types(),
            ["rlib", "cdylib", "staticlib"]
        );
    }

    /// So does the entry-script rule: a `cdylib` refuses one, and the other two leave the
    /// question to `--lib`.
    #[test]
    fn the_entry_rule_is_derived_from_the_target() {
        assert_eq!(BuildTarget::Hb.entry(), Script::Either);
        assert_eq!(BuildTarget::Bin.entry(), Script::Either);
        assert_eq!(BuildTarget::Cdylib.entry(), Script::Forbids);

        assert!(Script::Requires.accepts(false) && !Script::Requires.accepts(true));
        assert!(Script::Forbids.accepts(true) && !Script::Forbids.accepts(false));
        assert!(Script::Either.accepts(true) && Script::Either.accepts(false));
    }
}
