//! `htl.trait`: default methods for a record that implements an interface, typed via
//! `trait.d.tl`.
//!
//! Teal has `interface`, `record R is I` and bounded generics `<T is I>`, which cover the
//! signatures and the bound. It has no default bodies (an interface is abstract, and a
//! function defined on it is refused) and no way to share one body between implementers.
//! `htl.trait` is the smallest piece that fills the second gap with plain Teal: the
//! defaults sit in a table typed with the interface, and `trait.impl(defaults, Record)`
//! copies into the record every key it does not already hold.
//!
//! ```teal
//! local trait = require("htl.trait")
//! local Shape_default: Shape = { describe = function(self: Shape): string ... end }
//! trait.impl(Shape_default, Circle)
//! ```
//!
//! # Why the defaults are copied
//!
//! A default ends up on the record's table as one of the record's own fields, beside the
//! methods the record defines. A value reaches it the way it reaches those methods, through
//! its own `__index` or a constructor. The alternative, a second `__index` chain from the
//! record to the defaults table, would put a hop on every method call that the record's own
//! methods do not pay, and would give a value two places to be looked up in that no Teal
//! type describes. Copying keeps one lookup rule, and `impl` never overwrites: a method the
//! record defines is the record's.
//!
//! # Where it is available
//!
//! It is a pure Lua module with no host dependency, so it is available wherever a program
//! runs or is checked, with no feature to turn on:
//!
//! - **Every state [`Htl::new`] creates** has it in `package.preload`, and so does a
//!   program state made from a checker ([`Htl::with_checker`], [`Htl::with_checker_lua`]).
//!   `htl run`, `htl test` and a host that embeds htl each get a state through one of those,
//!   so none of them installs it.
//! - **The checker** reads its declaration, `htl/trait.d.tl`, from [`crate::lib_dir`]:
//!   [`Htl::install_trait_lib`] writes it there and puts the directory on the search path,
//!   and `project::checker` and the test runner call it for every project.
//! - **`htl build`** puts its source into the bundle as a module, not as a host module (the
//!   linker's `Target::Builtin`). A bundle that requires it runs on a host that installed
//!   nothing. On a host that also preloaded it through [`Htl::new`], the bundle's copy is the
//!   one that runs, so a bundle keeps the module it was built and checked with: see
//!   [`Htl::install_bundle`].
//!
//! # What comes later
//!
//! This is the module and where it is available. The markers (`---@trait` on an interface,
//! `---@defaults` on its defaults table) and the lint that reads them (`trait-impl`: every
//! method of the trait is defined by the record or supplied by a `trait.impl` call) are a
//! later change; until then `impl` is a helper and nothing checks that a record is complete.
use crate::Htl;
use anyhow::Result;
use std::path::{Path, PathBuf};

const TRAIT_LUA: &str = include_str!("../lua/trait.lua");
const TRAIT_DTL: &str = include_str!("../lua/trait.d.tl");

/// Module name of the bundled trait library.
pub const TRAIT_LIB: &str = "htl.trait";

/// The Lua source of a module that ships inside the binary and has no file of the
/// project's, by the name a `require` asks for; `None` for any other name.
///
/// What [`crate::link`] bundles for it: the linker finds only the declaration on the
/// search path and would otherwise file the name under the host's, which is not the host's
/// to provide when the module is pure Lua.
pub(crate) fn bundled_source(name: &str) -> Option<&'static str> {
    (name == TRAIT_LIB).then_some(TRAIT_LUA)
}

/// Whether `decl` is htl's own library's declaration of `name` for a module whose source
/// ships in the binary: `name` has a [`bundled_source`], and `decl` is a file under
/// [`crate::lib_dir`], the directory this library writes its declaration to.
///
/// The one test for "this declaration is the built-in's", asked by the model
/// ([`crate::model::Provider::Builtin`]) and by the linker when it has no model to ask. A
/// `.d.tl` of a project's own under the same name is not under that directory, so it is
/// not the built-in's and its source is never bundled in the project's place. Both paths
/// are canonical, because the temp directory `lib_dir` sits in may be spelled through a
/// symlink (the model's `canon` does the same).
pub(crate) fn declares(name: &str, decl: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    bundled_source(name).is_some() && canon(decl).starts_with(canon(&crate::lib_dir()))
}

/// What this library writes under [`crate::lib_dir`]: its declaration, and the path it
/// takes there. A list rather than a constant for the reason `testing::declarations` is:
/// [`crate::lib_dir`] hashes it into the directory's name.
pub(crate) fn declarations() -> Vec<(String, String)> {
    vec![("htl/trait.d.tl".to_string(), TRAIT_DTL.to_string())]
}

/// [`crate::lib_dir`] with this library's declaration in it: `htl/trait.d.tl`, written on
/// demand, only when its content changes.
pub fn lib_dir() -> Result<PathBuf> {
    crate::write_bundled_declarations(declarations())
}

impl Htl {
    /// Make the types of `require("htl.trait")` visible to the checker: the declaration is
    /// written under [`crate::lib_dir`] and the directory goes on the search path.
    ///
    /// Only the checker's half. The runtime half, the `package.preload` entry, is already
    /// in every state [`Htl::new`], [`Htl::with_checker`] and [`Htl::with_checker_lua`]
    /// make, so this is not what makes `htl.trait` run; a host that builds a checker and
    /// checks files that require it calls this as it calls
    /// [`install_test_lib`](Htl::install_test_lib).
    pub fn install_trait_lib(&self) -> Result<()> {
        self.add_path(&lib_dir()?)?;
        Ok(())
    }

    /// Put `htl.trait` in `package.preload`, as one of the state's own modules
    /// ([`Htl::preload_builtin`]): a bundle that carries a copy replaces it.
    pub(crate) fn preload_trait_lib(&self) -> Result<()> {
        // A label, not a path, as for `htl.test`: the library ships inside the binary.
        self.preload_builtin(TRAIT_LIB, &format!("={TRAIT_LIB}"), TRAIT_LUA)
    }
}
