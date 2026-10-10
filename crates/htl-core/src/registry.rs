//! The seam a binary adds its own `require`-able libraries through, wherever this crate
//! builds an `Htl` for its own use rather than the binary's: [`crate::testing`]'s
//! per-test state and [`crate::project::checker`]. Both already call
//! [`Htl::install_std`](crate::Htl::install_std) and
//! [`Htl::install_task_lib`](crate::Htl::install_task_lib) for the libraries this crate
//! can name itself; this module is for the ones it cannot.
//!
//! # Why this exists
//!
//! A library beyond `std.*` and `htl.task` is a crate that depends on `htl`, the
//! umbrella crate, the way any `#[host_module]` consumer does. `htl-core` and
//! `htl-macros` sit *under* `htl` in the dependency graph (`htl` is `pub use
//! htl_core::*` over both plus the macros), so neither can depend on such a crate back
//! without a cycle: `<library> -> htl -> htl-core -> <library>`. Only a binary that
//! links both `htl-core` and that library at once sits where both ends of the call are
//! visible at the same time.
//!
//! [`register_installer`] is such a binary's side of that call, made once, early,
//! before it builds any `Htl` of its own: it hands `htl-core` a function pointer and
//! the names that function makes `require`-able, without `htl-core` ever naming the
//! crate the function comes from. [`Htl::install_registered`] is `htl-core`'s side:
//! every site inside this crate that already calls `install_std` for its own internal
//! state calls this immediately after, so a state `htl-core` builds for itself — a
//! test file's, the checker's — carries what the binary registered, the same as a
//! state the binary builds directly, where it would call the registered function
//! itself and has no need of this at all.
//!
//! # What this is not
//!
//! The proc-macro checker (`htl-macros`'s `#[host_module]` and `include_tl!`
//! expansion) builds its own `Htl` to type a file the way `htl check` does —
//! `h.install_std()` included — but it runs inside `rustc`, expanding one crate's
//! macros: a process with no `main` that ever called `register_installer`, so nothing
//! is registered there no matter what a binary that later links the crate does. A
//! crate's own module still checks clean there, through a different route: `dep_dts`
//! materialises a dependency's declarations under the project's search path for any
//! crate that depends on it ([`crate::dep_dts`]), so the checker finds the
//! declaration the way it finds any dependency's and never needs the module preloaded
//! to type it. A binary's own `Htl::new` sites — wherever it builds a state directly
//! rather than asking this crate to build one for it — call whatever they registered
//! themselves, there, the same as they would with this module not existing at all, and
//! have no need of [`Htl::install_registered`] either.
//!
//! # Order and duplicates
//!
//! [`Htl::install_registered`] runs every registered installer in the order
//! [`register_installer`] was called in — registration order, not sorted, so a binary
//! that registers two libraries whose declarations disagree about one name gets to
//! decide which wins by registering the one that should last. Registering the same
//! `name` twice is refused: [`register_installer`] returns an error rather than
//! silently keeping the first registration or replacing it with the second, because the
//! two calls disagree about something — a crate linked twice under different paths, two
//! crates that chose the same name — that is worth seeing rather than papering over.
//!
//! # Why a directory of its own
//!
//! [`lib_dir`](crate::lib_dir)'s name is a hash over exactly what this crate's own
//! `bundled_declarations` would write — `htl.test`, and under their features, `std.*`
//! and `htl.task` — so two builds that carry different sets, or the same set with
//! different source, land in different directories and can never read each other's
//! (that function's own doc, and #220: a binary that found another's declarations on
//! its search path type-checked a project against modules it then could not load). A
//! file [`Htl::install_declarations`] writes is not part of that hash: `owner`'s crate
//! is not `htl-core`'s own, and a build of the binary with a newer `owner` would write
//! different text under the same key `lib_dir` already settled on without it ever
//! changing — the #220 bug again, one level up, with `owner`'s own version in place of
//! `htl-core`'s. So `install_declarations` hashes only the files it is given, under a
//! directory named for `owner` as well as that hash, and two binaries whose `owner`
//! differs — in version, in which crate it is — get directories that differ too.
//!
//! # Which copy wins when a project has one too
//!
//! A project that depends on the crate behind a registered installer may carry its own
//! copy of that crate's declarations as well, under `types/<crate>/` — `dep_dts`
//! materialises one there for any dependency a project's `Cargo.lock` names (`htl-std`'s
//! `std/fs.d.tl` under `types/htl-std/`, for example), the same way it would for any
//! other dependency that ships a `.d.tl`. That is the same name declared twice: once by
//! `install_declarations`, under the binary's own temp directory, and once by the
//! project's own `types/`. Which one the checker reads is [`Htl::add_path`]'s own rule —
//! prepend order, so whichever directory was put on the path last is searched first —
//! and every site that calls [`Htl::install_registered`] calls it before the project's
//! own directories go on the path (`apply_model`), so the project's copy ends up in
//! front. That is the order a project's own pin should win by: `types/<crate>/` is
//! written from the version the project's `Cargo.lock` resolved, which a project author
//! can see and change, while the registered installer's copy is only ever the version
//! the binary doing the checking happened to be built with — a fact about that binary,
//! not a choice the project made. A project's own files, including the ones a dependency
//! manager wrote for it, read before whatever the binary carries beyond them.
use crate::{Htl, declarations_key, write_if_changed};
use anyhow::{Context, Result, bail};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// One registered library: the name it was registered under (for the error on a second
/// registration and for [`Htl::install_registered`]'s own error context), the function
/// that call runs, and the names it makes `require`-able, for [`registered_provides`].
///
/// `Copy`, deliberately: every field already is (a `&'static str`, a function pointer, a
/// `&'static [&'static str]`), and [`Htl::install_registered`] needs to copy entries out
/// of the registry's lock rather than hold it while an installer runs — see that
/// method's own doc.
#[derive(Clone, Copy)]
struct Registered {
    name: &'static str,
    install: fn(&Htl) -> Result<()>,
    // Only read by `registered_provides`, which only `model::Project::load` calls —
    // a build without `pkg` and `dts` stores it and never reads it back.
    #[cfg_attr(not(all(feature = "pkg", feature = "dts")), allow(dead_code))]
    provides: &'static [&'static str],
}

/// The process-wide, append-only list [`register_installer`] pushes onto and
/// [`Htl::install_registered`] / [`registered_provides`] read. A `Mutex` rather than
/// something lock-free because both sides of it run rarely — registration once at
/// startup, installation once per `Htl` this crate builds for itself — and never on a
/// path `htl check` or `htl test` runs per file.
fn registry() -> &'static Mutex<Vec<Registered>> {
    static REGISTRY: OnceLock<Mutex<Vec<Registered>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Registers `install` to run on every `Htl` an internal site of this crate builds for
/// itself ([`Htl::install_registered`]), under `name` — a crate name, named in the error
/// this gives on a second call with the same one — and `provides`: the names `install`
/// makes `require`-able, which `registered_provides` (crate-private: see that
/// function's own doc) hands to the project model so it can say the binary provides
/// them ([`crate::model::Provider::Std`], where the `pkg` and
/// `dts` features build that far). `provides` is meant to come from the registering
/// crate's own `#[host_module]` constants (`Fs::MODULE`, not a hand-written `"std.fs"`)
/// the way [`crate::batteries::module_names`] is built from `mlua_batteries::dts::entries`
/// rather than a second list — a name written twice is the name that drifts.
///
/// Meant to be called once, early, by the binary that links both `htl-core` and the
/// crate `install` comes from — see the module doc's "Why this exists" for why
/// `htl-core` cannot make this call itself. Refuses a second registration under the same
/// `name` rather than running both or keeping only one; see the module doc's "Order and
/// duplicates".
pub fn register_installer(
    name: &'static str,
    install: fn(&Htl) -> Result<()>,
    provides: &'static [&'static str],
) -> Result<()> {
    let mut g = registry().lock().expect("installer registry lock");
    if g.iter().any(|r| r.name == name) {
        bail!("register_installer: \"{name}\" is already registered");
    }
    g.push(Registered {
        name,
        install,
        provides,
    });
    Ok(())
}

/// Every name a registered installer makes `require`-able, in registration order, for
/// the project model's `providers` (`model.rs`) to fold into what the project says the
/// binary provides.
#[cfg_attr(not(all(feature = "pkg", feature = "dts")), allow(dead_code))]
pub(crate) fn registered_provides() -> Vec<String> {
    registry()
        .lock()
        .expect("installer registry lock")
        .iter()
        .flat_map(|r| r.provides.iter().map(|n| n.to_string()))
        .collect()
}

impl Htl {
    /// Runs every installer [`register_installer`] registered, on `self`, in
    /// registration order. Called immediately after
    /// [`Htl::install_std`](crate::Htl::install_std) at every site inside this crate
    /// that builds an `Htl` for its own use ([`crate::testing`], [`crate::project`]'s
    /// checker) — see the module doc for why the binary's own `Htl::new` sites call the
    /// registered functions directly instead and have no need of this.
    ///
    /// The entries are copied out of the registry and its lock dropped before any of
    /// them runs — a `std::sync::Mutex` is not reentrant, so an installer that itself
    /// calls [`register_installer`] or reaches `registered_provides` (crate-private;
    /// both lock the same registry) would deadlock against a lock this method was
    /// still holding, and an installer that panics would poison it for every later
    /// caller. Copying is cheap: every field of `Registered` (crate-private) is
    /// `Copy`.
    pub fn install_registered(&self) -> Result<()> {
        let entries: Vec<Registered> = registry().lock().expect("installer registry lock").clone();
        for r in entries {
            (r.install)(self).with_context(|| format!("running the \"{}\" installer", r.name))?;
        }
        Ok(())
    }

    /// Writes `files` — `(path relative to the directory, source)` pairs — under a
    /// directory of their own, keyed by `owner` and their content the way
    /// [`lib_dir`](crate::lib_dir) is keyed by this crate's own bundled declarations,
    /// and puts that directory on the search path ([`Htl::add_path`](crate::Htl::add_path)).
    /// Returns the directory.
    ///
    /// For a crate outside this one that wants its declarations on the checker's search
    /// path without going through [`lib_dir`](crate::lib_dir) — the crate behind a
    /// [`register_installer`]'d library, typically, called from inside the function it
    /// registered, though nothing here requires that. See the module doc's "Why a
    /// directory of its own" for why this does not write into `lib_dir` itself.
    ///
    /// `owner` names the directory for a person reading it (a temp directory listing, an
    /// error) and is checked for it: a plain token of ASCII letters, digits, `_` and `-`,
    /// which refuses `/` and `..` on its own and so cannot walk the path anywhere this
    /// was not meant to write.
    pub fn install_declarations(&self, owner: &str, files: &[(&str, &str)]) -> Result<PathBuf> {
        if owner.is_empty()
            || !owner
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            bail!(
                "install_declarations: owner {owner:?} is not a plain name (letters, digits, _, - only)"
            );
        }
        let key = declarations_key(files.iter().copied());
        let dir = std::env::temp_dir().join(format!(
            "htl-lib-{}-{owner}-{key}",
            env!("CARGO_PKG_VERSION")
        ));
        for (path, source) in files {
            write_if_changed(&dir.join(path), source).with_context(|| {
                format!("writing {owner}'s declarations under {}", dir.display())
            })?;
        }
        self.add_path(&dir)?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`register_installer`] followed by [`Htl::install_registered`] on a fresh state
    /// runs the registered function: the flag it sets is read back from that state's
    /// own Lua globals.
    #[test]
    fn register_installer_runs_on_install_registered() -> Result<()> {
        register_installer(
            "htl-core-tests/registry-flag",
            |h| {
                h.lua().globals().set("HTL_CORE_TEST_REGISTRY_FLAG", true)?;
                Ok(())
            },
            &[],
        )?;
        let h = Htl::new()?;
        h.install_registered()?;
        let flag: bool = h.lua().globals().get("HTL_CORE_TEST_REGISTRY_FLAG")?;
        assert!(flag);
        Ok(())
    }

    /// The module doc's "Order and duplicates": a second [`register_installer`] under a
    /// name already registered is refused, not run alongside the first and not swapped
    /// in for it.
    #[test]
    fn registering_the_same_name_twice_is_refused() -> Result<()> {
        register_installer("htl-core-tests/dup", |_h| Ok(()), &[])?;
        let err = register_installer("htl-core-tests/dup", |_h| Ok(()), &[])
            .expect_err("a second registration under the same name");
        assert!(err.to_string().contains("htl-core-tests/dup"), "{err}");
        Ok(())
    }

    /// [`Htl::install_declarations`] writes under a directory named for `owner` and a
    /// key over the content it was given, and the directory it returns is on the
    /// search path: a `.d.tl` it wrote there makes `require` of the name it declares
    /// find a module rather than fail to resolve.
    #[test]
    fn install_declarations_writes_under_an_owner_keyed_directory_on_the_path() -> Result<()> {
        let h = Htl::new()?;
        let text = "local record M\n   ok: boolean\nend\nreturn M\n";
        let dir =
            h.install_declarations("htl-core-tests-decls", &[("registry/decltest.d.tl", text)])?;
        assert!(
            dir.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("htl-core-tests-decls")),
            "{}",
            dir.display()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("registry/decltest.d.tl"))?,
            text
        );

        // On the search path: a file that `require`s the declared name checks clean
        // rather than failing to find a module.
        let script_dir = crate::core_common::tempdir("htl-core-registry", "require");
        let script = script_dir.join("main.tl");
        std::fs::write(
            &script,
            "local m = require(\"registry.decltest\")\nprint(m)\n",
        )?;
        let (_code, info) = h.gen_lua(&script)?;
        assert!(info.ok(), "{info:?}");

        // A second call with different content gets a different directory: the key is
        // over the content, not only the owner.
        let other = h.install_declarations(
            "htl-core-tests-decls",
            &[("registry/decltest.d.tl", "local record M\nend\nreturn M\n")],
        )?;
        assert_ne!(dir, other);
        Ok(())
    }

    /// `owner` is checked before anything is written: a `/` or `..` in it is refused
    /// rather than followed.
    #[test]
    fn install_declarations_refuses_an_owner_that_is_not_a_plain_name() {
        let h = Htl::new().unwrap();
        assert!(h.install_declarations("../etc", &[]).is_err());
        assert!(h.install_declarations("a/b", &[]).is_err());
        assert!(h.install_declarations("", &[]).is_err());
    }

    /// The per-test state [`crate::testing::run_test_file`] builds runs registered
    /// installers too: a file that `require`s a name a registered installer both
    /// preloads and declares (through [`Htl::install_declarations`], called from
    /// inside the registered function) checks and runs.
    #[cfg(all(feature = "pkg", feature = "dts"))]
    #[test]
    fn the_per_test_state_runs_registered_installers() -> Result<()> {
        register_installer(
            "htl-core-tests/testing-flag",
            |h| {
                h.lua()
                    .load(
                        "package.preload['registry.testlib'] = function() return { ok = true } end",
                    )
                    .exec()?;
                h.install_declarations(
                    "htl-core-tests-testing",
                    &[(
                        "registry/testlib.d.tl",
                        "local record M\n   ok: boolean\nend\nreturn M\n",
                    )],
                )?;
                Ok(())
            },
            // `&[]`, not `&["registry.testlib"]`: `run_test_file` never asks the project
            // model, so nothing here needs the name on the model-provides fold, and
            // registering it there would leak into every later `Project::load` in this
            // same test binary (the registry is process-wide and append-only — see the
            // module doc's "Order and duplicates").
            &[],
        )?;
        let dir = crate::core_common::tempdir("htl-core-registry", "testing");
        let file = dir.join("main_test.tl");
        std::fs::write(
            &file,
            "local m = require(\"registry.testlib\")\nassert(m.ok == true)\n",
        )?;
        let rep = crate::testing::run_test_file(
            &file,
            None,
            crate::testing::DEFAULT_LIB,
            None,
            &crate::testing::RunOptions::default(),
        )?;
        assert!(rep.ok(), "{rep:?}");
        Ok(())
    }

    /// [`crate::project::checker`] — the state `htl check` builds — runs registered
    /// installers too.
    #[cfg(all(feature = "pkg", feature = "dts"))]
    #[test]
    fn the_checker_state_runs_registered_installers() -> Result<()> {
        register_installer(
            "htl-core-tests/checker-flag",
            |h| {
                h.lua().globals().set("HTL_CORE_TEST_CHECKER_FLAG", true)?;
                Ok(())
            },
            &[],
        )?;
        let h = crate::project::checker(
            None,
            &crate::lint::Selection::default(),
            &crate::config::LangConfig::default(),
        )?;
        let flag: bool = h.lua().globals().get("HTL_CORE_TEST_CHECKER_FLAG")?;
        assert!(flag);
        Ok(())
    }

    /// [`Htl::install_registered`]'s own doc: the registry's lock is dropped before any
    /// installer runs, so an installer that itself calls [`registered_provides`] (which
    /// locks the same, non-reentrant, `Mutex`) does not deadlock against a lock
    /// `install_registered` is still holding. If it did, this test would hang rather
    /// than return.
    #[test]
    fn an_installer_that_calls_registered_provides_does_not_deadlock() -> Result<()> {
        register_installer(
            "htl-core-tests/reentrant",
            |_h| {
                let _ = registered_provides();
                Ok(())
            },
            &[],
        )?;
        let h = Htl::new()?;
        h.install_registered()?;
        Ok(())
    }
}
