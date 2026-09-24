//! The project model: which modules a project is made of, where each keeps its files, and
//! what name a file answers to.
//!
//! # Why a model
//!
//! Lua has one answer to "which file does `require("a.b")` mean": walk `package.path`
//! and take the first template that matches. Teal inherits it, and so did htl. That answer
//! has no owner. A directory goes on the path because some part of htl wanted it there —
//! the project's sources, a dependency's entry link, a crate's declarations, a contract
//! directory — and nothing records *whose* directory it is, so nothing can say that a
//! name found in it belongs to anybody. What a name means then depends on the order the
//! directories happen to be added in, and different commands add them in different
//! orders.
//!
//! This module gives the answer an owner. A [`Project`] is a set of [`Module`]s; every
//! directory a name can be found in is a root of exactly one of them; and a file's name is
//! computed from the module that holds it, not from a search. `package.path` stays what
//! the Teal checker and the Lua runtime consume, but it becomes something derived *from*
//! the model rather than the place the model is spread across.
//!
//! # Terms
//!
//! - **Project** — what one `htl.toml` (and the `mlua-pkg.toml` beside it, when there is
//!   one) describes: a root directory, its configuration, and its modules.
//! - **Module** — a unit that owns a namespace and the directories that namespace is
//!   read from. The project's own code is one; so is every dependency, every crate that
//!   ships declarations, every directory a contract accepts modules from, and htl's own
//!   bundled library. They are the same type because they answer the same question —
//!   "which name does this file have" — and a type per kind would be a rule per kind.
//! - **Owner** — how a module came to be in the project ([`Owner`]). It decides what may
//!   be done to the module's files (checked, formatted, rewritten by an install), not how
//!   they are named.
//! - **Mount** — the prefix a module's names are read under. The project's own module is
//!   mounted at the top (`src/util.tl` is `util`); a dependency is mounted at its name
//!   (`<entry>/sub.tl` of dependency `x` is `x.sub`), the way a Go module path prefixes
//!   every package in the module.
//! - **Roots** — the directories a module keeps its files in, by role ([`Roots`]):
//!   `source` for code, `test` for tests, `decl` for declarations (`.d.tl`). A module may
//!   have any of them; a crate that ships declarations has only `decl`.
//!
//! # The naming rule
//!
//! One rule for every module ([`Module::name_of`]): the path of the file relative to the
//! root that holds it, extension dropped, separators turned into dots, under the module's
//! mount. Two spellings of a module directory are recognised, both as the name of the
//! directory itself: `<dir>/init.tl`, everywhere; and `<mount>/<mount>.tl`, only at the
//! top of a mounted module, which is where a flat package keeps its entry.
//!
//! # Which module a file belongs to
//!
//! The one whose root holds it most specifically ([`Project::locate`]). Roots may nest —
//! declarations a crate ships sit in their own directory inside the project's
//! declaration root, and a contract directory may sit inside the project's source root —
//! and the inner root is the one that owns what is under it. So a file has one owner and
//! one name, and the outer root's module does not also claim it under a longer name.
//!
//! # Which files a walk visits
//!
//! A module's roots are where its names are read from; its home ([`Module::home`]) is the
//! directory it owns as a whole. A walk over the tree — the files `htl check` checks,
//! `htl fmt` formats, `htl test` runs — decides whose files it enters by owner
//! ([`Purpose`], [`Project::not_walked`]): never a dependency an install writes, a
//! patched dependency only to check it.
//!
//! # What this module does not do yet
//!
//! A name still resolves through `package.path`: the model decides which directories go
//! on it ([`Project::search_dirs`]), but every name the model has is answered by its
//! [`Resolver`] first, and a file the search finds under a name the model does not give it
//! is not found. Every command
//! that checks, runs or bundles a file sets its checker up from the model — `htl check`,
//! `test`, `fix`, `gen`, `run`, `build`, `resolve` and the `include_tl!` /
//! `include_bundle!` macros — and a file that belongs to no project reads its own
//! directory instead ([`project::file_view`](crate::project::file_view)). The run cache
//! records, for each entry, what the resolver answers for every name the module required
//! ([`with_model`](crate::project::with_model)), and replays the entry only while those
//! answers stand. Host modules — the `.d.tl` a Rust host generates from
//! `#[host_module]` — are not loaded here: they are known to whoever compiled the host,
//! and enter a project through the file they are written to.

pub mod resolver;
pub use resolver::{Found, Resolution, Resolver};

use crate::config::{CONFIG_NAME, HtlConfig, resolve_path};
use crate::pkg;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// A project: its root, its configuration, and the modules it is made of.
///
/// Built by [`Project::load`] from a root that has already been found, or by
/// [`Project::discover`] from anywhere inside one. The modules are in a fixed order — the
/// project's own first, then what the project took on (patched, vendored, installed
/// dependencies), then declarations, then what it accepts from outside, then htl's own
/// library — and that order is only for reading: which module a file belongs to is
/// decided by [`locate`](Self::locate), not by position.
#[derive(Debug, Clone)]
pub struct Project {
    /// The directory holding `htl.toml`, or `mlua-pkg.toml` when there is no `htl.toml`.
    /// Every relative path in either file is relative to it.
    pub root: PathBuf,
    /// The parsed `htl.toml`; its default when the project has none.
    pub config: HtlConfig,
    /// Every module the project consists of. Exactly one has [`Owner::Own`].
    pub modules: Vec<Module>,
    /// The directory of entry links an install writes (`.htl/modules/entries`), one per
    /// installed dependency, each at the dependency's require root. `None` without an
    /// `mlua-pkg.toml`. On the search path whenever the project has one
    /// ([`search_dirs`](Self::search_dirs)): it is the one directory through which every
    /// installed dependency — linked under `vendored/` or copied to a `target_dir` —
    /// answers to its name.
    pub links: Option<PathBuf>,
    /// What reading the project could not make sense of, as messages. The model is built
    /// from the rest; nothing here stops a load. Today these come from `---@contract`
    /// markers that do not parse.
    pub problems: Vec<String>,
}

/// A unit that owns a namespace and the directories it is read from.
///
/// See the [module documentation](self) for why the project's own code, its dependencies,
/// shipped declarations and contract directories are all this one type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Module {
    /// What a report calls the module: the package name for the project's own and for a
    /// dependency, the crate for shipped declarations, the directory as `htl.toml` wrote
    /// it for a contract or a `[check] paths` entry.
    pub name: String,
    /// How the module came to be in the project.
    pub owner: Owner,
    /// The prefix its names are read under, dot-separated; empty for a module mounted at
    /// the top. See [`Module::name_of`].
    pub mount: String,
    /// Where its files are, by role.
    pub roots: Roots,
    /// The directory the module owns as a whole, when it has one: everything below it is
    /// the module's, whether or not a root reaches it.
    ///
    /// Not the same as its roots. A patched dependency's names are read from its entry
    /// (`patches/mathx/src/mathx`), but the copy is `patches/mathx/`, its tests and its
    /// manifest included; a walk deciding whether a file is the project's to format asks
    /// about the copy. For the project's own module it is the project root, which holds
    /// every other module's home that sits inside the tree — the most specific home
    /// answers ([`Project::not_walked`]).
    pub home: Option<PathBuf>,
}

/// How a module came to be in the project, which decides what may be done to its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// The project's own code: checked, formatted, tested and built as the project.
    Own,
    /// A dependency taken into the tree with `htl pkg patch`. The project edits it from
    /// then on, so it is the project's code under the dependency's name.
    Patched,
    /// A dependency copied into the tree at a manifest's `target_dir`. `mlua-pkg install`
    /// rewrites it, so an edit there does not survive.
    Vendored,
    /// A dependency installed under `.htl/modules`, regenerated from the lockfile.
    Installed,
    /// Declarations a Rust crate ships in `[package.metadata.htl] dts`, materialised by
    /// `htl dts` into a directory of their own under the declaration root.
    Crate {
        /// The version the note beside them records, when there is a readable note.
        version: Option<String>,
    },
    /// A directory a `[[contract]]` or a `---@contract` marker accepts modules from.
    /// Somebody else writes these; the contract is what the project holds them to.
    Contract,
    /// A `[check] paths` entry: modules the project did not write and reaches anyway.
    External,
    /// htl's own library — `htl.test`, and `std.*` when the binary has it — whose
    /// declarations htl writes to [`lib_dir`](crate::lib_dir) for the checker.
    Lib,
}

/// The directories a module keeps its files in, by role. Absolute.
///
/// A role a module does not have is `None`. Two roles may name the same directory: a
/// project that keeps its `.d.tl` beside its `.tl` has one directory as both `source`
/// and `decl`, and a file there takes the role its extension says ([`Role::of`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    /// Code: `.tl` the module is made of.
    pub source: Option<PathBuf>,
    /// Tests: `.tl` run by `htl test`, which may `require` the module's sources.
    pub test: Option<PathBuf>,
    /// Declarations: `.d.tl` describing modules whose implementation is elsewhere.
    pub decl: Option<PathBuf>,
}

/// The role a root plays in its module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// [`Roots::source`].
    Source,
    /// [`Roots::test`].
    Test,
    /// [`Roots::decl`].
    Decl,
}

impl Role {
    /// The role a file takes by its extension, where a directory serves more than one:
    /// a `.d.tl` is a declaration, anything else is source.
    pub fn of(file: &Path) -> Self {
        if crate::is_declaration(file) {
            Role::Decl
        } else {
            Role::Source
        }
    }
}

impl Roots {
    /// The roots with their roles, in role order, skipping the ones the module lacks.
    pub fn iter(&self) -> impl Iterator<Item = (Role, &Path)> {
        [
            (Role::Source, &self.source),
            (Role::Test, &self.test),
            (Role::Decl, &self.decl),
        ]
        .into_iter()
        .filter_map(|(r, p)| p.as_deref().map(|p| (r, p)))
    }

    /// The root playing `role`, when the module has one.
    pub fn get(&self, role: Role) -> Option<&Path> {
        match role {
            Role::Source => self.source.as_deref(),
            Role::Test => self.test.as_deref(),
            Role::Decl => self.decl.as_deref(),
        }
    }
}

/// Where a file sits in the model: the module that owns it, the root it is under, and
/// the name it answers to. What [`Project::locate`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place<'a> {
    /// The module whose root holds the file most specifically.
    pub module: &'a Module,
    /// Which of that module's roots.
    pub role: Role,
    /// The module name the file answers to.
    pub name: String,
}

impl Module {
    /// The module name `file` answers to, read from `root` (one of this module's roots)
    /// under this module's mount. `None` when `file` is not under `root`, or is `root`'s
    /// own `init.tl` in a module mounted at the top — that file would have to be named
    /// by the empty string.
    ///
    /// The rule, for a file at `<root>/<a>/<b>.tl`: its name is `<mount>.<a>.<b>`, with
    /// `.tl`, `.d.tl` or `.lua` dropped and an empty mount contributing nothing. Two
    /// spellings name the directory they are in rather than themselves:
    ///
    /// - `<dir>/init.tl` is `<dir>` — Lua's `?/init.lua`, which every Teal and Lua
    ///   project uses for a module that has submodules.
    /// - `<root>/<last>.tl`, where `<last>` is the final segment of the mount, is the
    ///   mount itself — Lua's `?/?.lua`, the entry of a flat package (`entry = "src"`
    ///   holding `<name>.tl` beside its other files). Only at the top of a mounted module:
    ///   everywhere else a file named after its directory is an ordinary submodule, and
    ///   giving it a second name is how one file came to answer to two.
    pub fn name_of(&self, root: &Path, file: &Path) -> Option<String> {
        self.name_of_relative(&strip_under(file, root)?)
    }

    /// [`name_of`](Self::name_of) for a path already relative to the root, for a caller
    /// that made both canonical once (the [`Resolver`] places thousands of files).
    pub(crate) fn name_of_relative(&self, rel: &Path) -> Option<String> {
        crate::naming::name_of(&self.mount, rel)
    }

    fn mount_last(&self) -> &str {
        crate::naming::mount_last(&self.mount)
    }
}

/// `file` relative to `root`, comparing canonical forms when both exist so that a
/// symlink (an installed dependency's entry link) or a `..` does not make one directory
/// two.
fn strip_under(file: &Path, root: &Path) -> Option<PathBuf> {
    let (f, r) = (canon(file), canon(root));
    f.strip_prefix(&r).ok().map(Path::to_path_buf)
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl Project {
    /// The project whose `htl.toml` or `mlua-pkg.toml` is nearest above `start` (a file
    /// or a directory). `Ok(None)` when there is neither.
    ///
    /// Finding the root is the caller's business — a command line or a macro knows where
    /// it starts, and a library embedding htl may know the root outright and call
    /// [`load`](Self::load) — so this is the one place in the model that walks up. The
    /// two manifests are looked for the way the rest of htl looks for them, a copy of a
    /// dependency's manifest inside a patched directory included; when both are found
    /// they have to name the same directory, because one project with two roots is two
    /// answers to every relative path in it.
    pub fn discover(start: &Path) -> Result<Option<Self>> {
        let config = HtlConfig::find(start)?;
        let manifest = pkg::MluaProject::find(start);
        let (root, config) = match (config, manifest) {
            (None, None) => return Ok(None),
            (Some((path, cfg)), m) => {
                let root = crate::parent_dir(&path);
                if let Some(m) = m
                    && canon(&m.root) != canon(&root)
                {
                    bail!(
                        "{} and {} name different project roots ({} and {}); a project \
                         has one root, so move one of them beside the other",
                        CONFIG_NAME,
                        pkg::MANIFEST_NAME,
                        root.display(),
                        m.root.display()
                    );
                }
                (root, cfg)
            }
            (None, Some(m)) => (m.root, HtlConfig::default()),
        };
        Self::load(&root, config).map(Some)
    }

    /// The model of the project rooted at `root`, configured by `config`.
    ///
    /// Reads what the directory holds and nothing above it: `mlua-pkg.toml` and its
    /// lockfile when present, the notes beside materialised declarations, and the
    /// `---@contract` markers in the files [`contract::resolve`](crate::contract::resolve)
    /// scans. A root that does not exist is not an error — a module whose directory is
    /// missing still has a place its names would be read from — except that `[check]
    /// paths` entries and contract directories that do not exist contribute no module,
    /// the way they contribute nothing to the search path.
    pub fn load(root: &Path, config: HtlConfig) -> Result<Self> {
        let mut modules = Vec::new();
        let manifest = root
            .join(pkg::MANIFEST_NAME)
            .is_file()
            .then(|| pkg::MluaProject::at(root));

        modules.push(own_module(root, &config, manifest.as_ref()));
        if let Some(m) = &manifest {
            modules.extend(dependency_modules(m));
        }
        let decl_root = resolve_path(root, &config.layout.types);
        modules.extend(crate_modules(&decl_root));
        let (contracts, problems) = crate::contract::resolve(root, &config);
        let mut accepted: Vec<PathBuf> = Vec::new();
        for c in &contracts {
            for dir in c.dirs(root) {
                if accepted.iter().any(|d| canon(d) == canon(&dir)) {
                    continue;
                }
                modules.push(Module {
                    name: display_under(&dir, root),
                    owner: Owner::Contract,
                    mount: String::new(),
                    roots: Roots {
                        source: Some(dir.clone()),
                        ..Roots::default()
                    },
                    home: Some(dir.clone()),
                });
                accepted.push(dir);
            }
        }
        for p in &config.check.paths {
            let dir = resolve_path(root, p);
            // A directory a contract already accepts from is that contract's: the
            // contract says more about it than "reachable", and one directory is one
            // module.
            if !dir.is_dir() || accepted.iter().any(|d| canon(d) == canon(&dir)) {
                continue;
            }
            modules.push(Module {
                name: p.clone(),
                owner: Owner::External,
                mount: String::new(),
                roots: Roots {
                    source: Some(dir.clone()),
                    ..Roots::default()
                },
                home: Some(dir.clone()),
            });
            accepted.push(dir);
        }
        modules.push(Module {
            name: "htl".into(),
            owner: Owner::Lib,
            mount: String::new(),
            roots: Roots {
                decl: Some(crate::lib_dir()),
                ..Roots::default()
            },
            home: Some(crate::lib_dir()),
        });
        Ok(Self {
            root: root.to_path_buf(),
            config,
            modules,
            links: manifest.as_ref().map(|m| m.entries.clone()),
            problems,
        })
    }

    /// The project's own module.
    pub fn own(&self) -> &Module {
        self.modules
            .iter()
            .find(|m| m.owner == Owner::Own)
            .expect("Project::load always makes the project's own module")
    }

    /// Which module `file` belongs to, in which role, and the name it answers to.
    /// `None` when no module's root holds it, or when it would be named by the empty
    /// string (see [`Module::name_of`]).
    ///
    /// The owner is the module whose root holds the file most specifically — the root
    /// with the most path components among those that contain it.
    ///
    /// The role is the test root's for a file under it, and otherwise the file's own
    /// ([`Role::of`]): a `.d.tl` is a declaration wherever it sits. A host's generated
    /// `src/host.d.tl` beside the sources it describes is as much a declaration as one
    /// under the declaration root, and a project that keeps all its `.d.tl` beside its
    /// `.tl` has one directory serving as both roots.
    pub fn locate(&self, file: &Path) -> Option<Place<'_>> {
        let target = canon(file);
        let mut best: Option<(usize, &Module, Role, &Path)> = None;
        for m in &self.modules {
            for (role, root) in m.roots.iter() {
                let r = canon(root);
                if !target.starts_with(&r) {
                    continue;
                }
                let depth = r.components().count();
                // Two roles of one module on one directory are one root; the role is
                // settled below, so the first of them is as good as the second.
                if best.is_none_or(|(d, ..)| depth > d) {
                    best = Some((depth, m, role, root));
                }
            }
        }
        let (_, module, root_role, root) = best?;
        let role = match root_role {
            Role::Test => Role::Test,
            _ => Role::of(file),
        };
        let name = module.name_of(root, file)?;
        Some(Place { module, role, name })
    }
}

/// What a checker is being set up to check, which decides the roots it may read from.
///
/// A test may `require` a helper beside it under the test root; the project's sources
/// may not reach into its tests. Everything else is visible from both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// The project's own sources, and anything they reach.
    Source,
    /// A test file: the sources' view plus the project's test root.
    Test,
}

impl Project {
    /// The directories to put on `package.path` for `view`, in the order they are
    /// consulted.
    ///
    /// This is where the model meets Lua's search: each module contributes the
    /// directories that make its files answer to the names [`Module::name_of`] gives
    /// them. For a module mounted at the top that is its roots themselves. For a
    /// dependency mounted at `x` it is the directory *holding* a directory named `x` —
    /// `package.path`'s templates are `<dir>/?.lua` and `<dir>/?/init.lua`, so only a
    /// directory named after the mount turns `x.sub` into a file. An installed
    /// dependency's link `entries/x` is named so, and `entries/` goes on the path once
    /// for all of them; a vendored copy is its own directory named after the dependency;
    /// a patched copy's entry is named after it in the usual layout (`src/x`), and is its
    /// own directory otherwise, which resolves `x` to `<entry>/x.tl` and nothing below
    /// it — the one mount a path cannot express.
    ///
    /// Contract directories are not listed. Each holds modules written against one record,
    /// and a `sites/*` contract is a row of directories holding the *same* names — one
    /// `one.tl` per site — so on one path they would hide each other. A contract directory
    /// is checked on its own, against its contract.
    ///
    /// The order is the project's own roots, then shipped declarations, then
    /// dependencies, then `[check] paths`. It decides nothing the
    /// model does not already decide, except between two files in different modules that
    /// answer one name. That is a conflict the model does not report yet; until it does,
    /// the order is what picks, and it picks the project's own file first.
    ///
    /// htl's own library is not listed: [`install_test_lib`](crate::Htl::install_test_lib)
    /// and `install_std` write it and put it on the path themselves. Directories are
    /// listed whether or not they exist, because the run cache keys an entry on them and a
    /// directory created later changes what a name means.
    pub fn search_dirs(&self, view: View) -> Vec<PathBuf> {
        let own = self.own();
        let mut out: Vec<PathBuf> = Vec::new();
        out.extend(own.roots.source.clone());
        out.extend(own.roots.decl.clone());
        if view == View::Test {
            out.extend(own.roots.test.clone());
        }
        let owned_by = |o: fn(&Owner) -> bool| self.modules.iter().filter(move |m| o(&m.owner));
        for m in owned_by(|o| matches!(o, Owner::Crate { .. })) {
            out.extend(m.roots.decl.clone());
        }
        // The links first: a dependency an install placed answers through its link, at its
        // require root, whatever that root is called. The directories below are for what
        // a link cannot cover — a patch or a copy in a tree nobody has installed in.
        out.extend(self.links.clone());
        for m in owned_by(|o| matches!(o, Owner::Patched | Owner::Vendored | Owner::Installed)) {
            let Some(root) = &m.roots.source else {
                continue;
            };
            match package_parent(m, root) {
                Some(up) => out.push(up),
                None => out.push(root.clone()),
            }
        }
        for m in owned_by(|o| matches!(o, Owner::External)) {
            out.extend(m.roots.source.clone());
        }
        let mut seen: Vec<PathBuf> = Vec::new();
        out.retain(|d| {
            let c = canon(d);
            let fresh = !seen.contains(&c);
            seen.push(c);
            fresh
        });
        out
    }
}

impl Project {
    /// The directories of [`search_dirs`](Self::search_dirs) that hold packages by name —
    /// the dependency links, and the parent of a dependency's root named after it (a
    /// patch's `src/<name>`, a copy's `lua/<name>`) — where a flat package's
    /// `<name>/<name>.tl` is `<name>` ([`naming`](crate::naming)). Every other directory
    /// on the path is a root of modules mounted at the top, where that file is
    /// `<name>.<name>`; the checker's path spells the two differently
    /// ([`Htl::add_package_path`](crate::Htl::add_package_path)).
    pub fn package_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.links.iter().cloned().collect();
        for m in self
            .modules
            .iter()
            .filter(|m| matches!(m.owner, Owner::Patched | Owner::Vendored | Owner::Installed))
        {
            if let Some(up) = m.roots.source.as_ref().and_then(|r| package_parent(m, r)) {
                out.push(up);
            }
        }
        out
    }
}

/// The directory holding a dependency's source `root` as a child named after the
/// dependency, when it is one: consulted as `<dir>/<name>`, it resolves the dependency
/// the way its link does.
fn package_parent(m: &Module, root: &Path) -> Option<PathBuf> {
    let named_after_mount = root.file_name().is_some_and(|f| f == m.mount_last());
    named_after_mount
        .then(|| root.parent().map(Path::to_path_buf))
        .flatten()
}

/// What a walk over a project's files is for, which decides whose files it enters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Reporting what is wrong: `htl check`, `htl fix`. A patched dependency is entered —
    /// the copy is the project's code, and its errors are the project's to fix.
    Check,
    /// Changing or judging the project's own code: `htl fmt`, `htl unused`. A patched
    /// dependency is left alone — its change is a diff against the revision it came from,
    /// and a reformatting of every file would bury it; what reaches it lives upstream.
    Own,
    /// Running the project's tests: `htl test`. A patched dependency's tests are its own
    /// suite, and running them would report a library's failures as the project's.
    Test,
}

impl Project {
    /// The directories a walk for `purpose` does not enter: the homes of the modules
    /// whose files are not the walk's to visit.
    ///
    /// An installed or vendored dependency is never entered: `mlua-pkg install` writes
    /// it, and every file in it is someone else's. A patched one is entered only to
    /// check it. The project's own module, contract directories and `[check] paths`
    /// inside the tree are walked as they always were; declarations a crate shipped are
    /// `.d.tl`, which no walk collects. Directories a walk never enters whatever it is for
    /// — build output, dot-directories — are the walker's own rule
    /// ([`crate::is_skipped_dir`]).
    pub fn not_walked(&self, purpose: Purpose) -> Vec<PathBuf> {
        self.modules
            .iter()
            .filter(|m| match m.owner {
                Owner::Installed | Owner::Vendored => true,
                Owner::Patched => purpose != Purpose::Check,
                _ => false,
            })
            .filter_map(|m| m.home.clone())
            .collect()
    }
}

/// One module name that more than one module implements, in one view.
///
/// A name has one owner. Two modules that both implement it — the project's `src/mathx.tl`
/// and a dependency `mathx` — leave the checker to pick whichever the search path meets
/// first, and nothing said which that was. A declaration beside an implementation is not a
/// second claim (the declaration types the implementation), and two declarations of one
/// name are `duplicate-declaration`'s to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The name both answer to.
    pub name: String,
    /// Each implementation, with the module that provides it, in the order the modules
    /// are listed ([`Project::modules`]).
    pub claims: Vec<Claim>,
}

/// One module's implementation of a name in a [`Conflict`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The module that provides it.
    pub module: Module,
    /// The file.
    pub file: PathBuf,
}

impl Module {
    /// How a report names the module: `this project`, `dependency mathx`, `patched
    /// dependency mathx`, `[check] paths entry vendor`, …
    pub fn describe(&self) -> String {
        match &self.owner {
            Owner::Own => "this project".to_string(),
            Owner::Patched => format!("patched dependency {}", self.name),
            Owner::Vendored => format!("vendored dependency {}", self.name),
            Owner::Installed => format!("dependency {}", self.name),
            Owner::Crate { .. } => format!("declarations shipped by {}", self.name),
            Owner::Contract => format!("contract directory {}", self.name),
            Owner::External => format!("[check] paths entry {}", self.name),
            Owner::Lib => "htl's own library".to_string(),
        }
    }
}

impl Project {
    /// The names more than one module implements, as `view` sees the project.
    ///
    /// Every file under every root the view reaches is placed ([`locate`](Self::locate)),
    /// so a file belongs to the module whose root holds it most specifically and is
    /// counted once. Contract directories are not in any view: each is checked on its own.
    /// The test root is in [`View::Test`] only.
    ///
    /// This reads directories, a dependency's included; it is asked once per check, not
    /// per file.
    pub fn conflicts(&self, view: View) -> Vec<Conflict> {
        use std::collections::BTreeMap;
        let mut by_name: BTreeMap<String, Vec<(usize, PathBuf)>> = BTreeMap::new();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for m in self.modules.iter().filter(|m| m.owner != Owner::Contract) {
            for (role, root) in m.roots.iter() {
                if role == Role::Test && view != View::Test {
                    continue;
                }
                let top = root.to_path_buf();
                let walker = walkdir::WalkDir::new(root)
                    .sort_by_file_name()
                    .into_iter()
                    .filter_entry(move |e| {
                        e.path() == top || !crate::is_skipped_dir(e.path(), &[])
                    });
                for e in walker.flatten() {
                    let file = e.path();
                    let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    let implements = name.ends_with(".lua")
                        || (name.ends_with(".tl") && !name.ends_with(".d.tl"));
                    if !e.file_type().is_file() || !implements {
                        continue;
                    }
                    if !seen.insert(canon(file)) {
                        continue;
                    }
                    let Some(place) = self.locate(file) else {
                        continue;
                    };
                    if place.role == Role::Test && view != View::Test {
                        continue;
                    }
                    let Some(i) = self.modules.iter().position(|x| x == place.module) else {
                        continue;
                    };
                    by_name
                        .entry(place.name)
                        .or_default()
                        .push((i, file.to_path_buf()));
                }
            }
        }
        // A name `[imports]` says which module it means is not a conflict: the project
        // said, and the rewrite below is what makes it so.
        let decided = self.config.import_targets();
        let covered = |name: &str| {
            decided
                .iter()
                .any(|(k, _)| name == k || name.starts_with(&format!("{k}.")))
        };
        by_name
            .into_iter()
            .filter(|(name, _)| !covered(name))
            .filter_map(|(name, mut claims)| {
                claims.sort_by_key(|(i, _)| *i);
                let first = claims.first()?.0;
                if claims.iter().all(|(i, _)| *i == first) {
                    return None;
                }
                Some(Conflict {
                    name,
                    claims: claims
                        .into_iter()
                        .map(|(i, file)| Claim {
                            module: self.modules[i].clone(),
                            file,
                        })
                        .collect(),
                })
            })
            .collect()
    }
}

impl Project {
    /// What `[imports]` gets wrong about this project: a `dep:` target naming no
    /// dependency.
    pub fn import_problems(&self) -> Vec<String> {
        self.config
            .import_targets()
            .into_iter()
            .filter_map(|(key, t)| match t {
                crate::config::ImportTarget::Dep(name) => {
                    let dep = name.split('.').next().unwrap_or(&name).to_string();
                    let known = self.modules.iter().any(|m| {
                        m.name == dep
                            && matches!(
                                m.owner,
                                Owner::Installed | Owner::Vendored | Owner::Patched
                            )
                    });
                    (!known).then(|| {
                        format!(
                            "[imports] {key} = \"dep:{name}\": the project has no dependency \
                             named {dep}"
                        )
                    })
                }
                crate::config::ImportTarget::Own(_) => None,
            })
            .collect()
    }
}

impl crate::Htl {
    /// Set this checker up for `project`, as `view` sees it: the working directory off the
    /// path ([`drop_cwd_search_path`](crate::Htl::drop_cwd_search_path)), its installed
    /// dependencies made reachable ([`prepare_deps`](crate::Htl::prepare_deps), when the
    /// project has an `mlua-pkg.toml`), the model's [`Resolver`] answering every name
    /// ([`install_resolver`](Self::install_resolver)), then the model's directories on the
    /// search path in the order they are consulted ([`Project::search_dirs`]), a directory
    /// of packages ([`Project::package_dirs`]) with a flat package's spelling and every
    /// other one without it.
    pub fn apply_model(&self, project: &Project, view: View) -> Result<()> {
        // The names come from the project's roots, so the working directory is not one of
        // the places they are looked for.
        self.drop_cwd_search_path()?;
        if project.root.join(pkg::MANIFEST_NAME).is_file() {
            self.prepare_deps(&pkg::MluaProject::at(&project.root))?;
        }
        self.install_resolver(Resolver::new(project))?;
        // Back to front, as `add_search_paths` does, each directory spelled as what it is.
        let packages: Vec<PathBuf> = project.package_dirs().iter().map(|d| canon(d)).collect();
        for d in project.search_dirs(view).iter().rev() {
            if packages.contains(&canon(d)) {
                self.add_package_path(d)?;
            } else {
                self.add_path(d)?;
            }
        }
        Ok(())
    }

    /// Have the checker ask `resolver` for every module name.
    ///
    /// Three functions go into the prelude, made in the checker's state: `resolve_name`
    /// (requirer, name) → kind and files, which the prelude's `tl.search_module` asks
    /// first; `rewrite_name` (requirer, name) → the name a `require` is to be written as,
    /// which its `tl.parse` wrapper applies; and `owns_file` (path) → whether a file is the
    /// model's, which holds a `package.path` search for a name the model does not have to
    /// the files outside it. The kinds are `found`, `ambiguous`, `missing`, `outside` and
    /// `hidden` ([`Resolution`]).
    pub fn install_resolver(&self, resolver: Resolver) -> Result<()> {
        let lua = self.checker_lua()?;
        let r = std::sync::Arc::new(resolver);
        let s = |p: Option<PathBuf>| p.map(|p| p.to_string_lossy().into_owned());
        let resolve = {
            let r = r.clone();
            lua.create_function(move |_, (requirer, name): (Option<String>, String)| {
                type Answer = (String, Option<String>, Option<String>, Option<String>);
                let answer: Answer = match r.resolve(requirer.as_deref().map(Path::new), &name) {
                    Resolution::Found(f) => (
                        "found".into(),
                        s(f.implementation),
                        s(f.declaration),
                        s(f.lua),
                    ),
                    Resolution::Ambiguous(claims) => {
                        let who: Vec<String> = claims
                            .iter()
                            .map(|(m, f)| format!("{m} ({})", f.display()))
                            .collect();
                        (
                            "ambiguous".into(),
                            Some(format!(
                                "'{name}' is implemented by more than one file: {}",
                                who.join(", ")
                            )),
                            None,
                            None,
                        )
                    }
                    Resolution::Missing => ("missing".into(), None, None, None),
                    Resolution::Outside => ("outside".into(), None, None, None),
                    Resolution::NotVisible(file, msg) => {
                        ("hidden".into(), Some(msg), s(Some(file)), None)
                    }
                };
                Ok(answer)
            })?
        };
        let rewrite = {
            let r = r.clone();
            lua.create_function(move |_, (requirer, name): (String, String)| {
                Ok(r.rewrite(Path::new(&requirer), &name))
            })?
        };
        let owns = lua.create_function(move |_, path: String| Ok(r.owns(Path::new(&path))))?;
        self.h.set("resolve_name", resolve)?;
        self.h.set("rewrite_name", rewrite)?;
        self.h.set("owns_file", owns)?;
        Ok(())
    }
}

/// The project's own module: named by `mlua-pkg.toml`'s `[package] name` when it has one
/// that parses, by the root directory otherwise; mounted at the top; its roots from
/// `[layout]`.
fn own_module(root: &Path, config: &HtlConfig, manifest: Option<&pkg::MluaProject>) -> Module {
    let name = manifest
        .and_then(pkg::MluaProject::package_name)
        .unwrap_or_else(|| {
            canon(root)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
    Module {
        name,
        owner: Owner::Own,
        mount: String::new(),
        roots: Roots {
            source: Some(resolve_path(root, &config.layout.source)),
            test: Some(resolve_path(root, &config.layout.tests)),
            decl: Some(resolve_path(root, &config.layout.types)),
        },
        home: Some(root.to_path_buf()),
    }
}

/// One module per dependency, mounted at the dependency's name.
///
/// A dependency can be in the tree more than one way at once — installed, and also
/// copied to a `target_dir` or taken into a `patch_dir` — and it is one module
/// regardless: the name is the dependency's, and only one copy is the one its names mean.
/// A patch is what the project edits, so it wins; a vendored copy is what the manifest
/// put in the tree, so it comes next; an installed dependency is the rest.
fn dependency_modules(p: &pkg::MluaProject) -> Vec<Module> {
    let mut out: Vec<Module> = Vec::new();
    let taken = |out: &[Module], name: &str| out.iter().any(|m| m.name == name);
    for patch in &p.patches {
        out.push(dependency(
            &patch.name,
            Owner::Patched,
            patch.entry.clone(),
            patch.dir.clone(),
        ));
    }
    // A `target_dir` copy is named by the manifest, not by its directory, and read from
    // its entry inside the copy, as a patch is.
    for copy in &p.copies {
        if !taken(&out, &copy.name) {
            out.push(dependency(
                &copy.name,
                Owner::Vendored,
                copy.entry.clone(),
                copy.dir.clone(),
            ));
        }
    }
    // Installed: every link under `entries/`, which is where a `require` reads an
    // installed dependency from, and every name the lockfile records, whose link an
    // install has yet to write. A name in either is one dependency, mounted at the link.
    let mut installed: Vec<String> = std::fs::read_dir(&p.entries)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    installed.extend(p.locked_deps());
    installed.sort();
    installed.dedup();
    for name in installed {
        if !taken(&out, &name) {
            let entry = p.entries.join(&name);
            let home = p.placed_at(&name);
            out.push(dependency(&name, Owner::Installed, entry, home));
        }
    }
    out
}

/// A dependency's module: its names read from `entry` under its own name, the whole of
/// `home` its own.
fn dependency(name: &str, owner: Owner, entry: PathBuf, home: PathBuf) -> Module {
    Module {
        name: name.to_string(),
        owner,
        mount: name.to_string(),
        roots: Roots {
            source: Some(entry),
            ..Roots::default()
        },
        home: Some(home),
    }
}

/// One module per directory of declarations a crate shipped, under `decl_root`.
///
/// Each is mounted at the top: `htl-mq`'s `mq.d.tl` declares `mq`, the name the crate
/// wrote it under, and that is what a consumer `require`s. The directory is nested in
/// the project's declaration root, and is its own root so that the file is `mq` and not
/// also `htl-mq.mq` ([`Project::locate`]).
fn crate_modules(decl_root: &Path) -> Vec<Module> {
    crate::materialised_types_dirs(decl_root)
        .into_iter()
        .map(|dir| {
            let note = crate::dep_dts::Note::read(&dir);
            let name = note.as_ref().map(|n| n.package.clone()).unwrap_or_else(|| {
                dir.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            Module {
                name,
                owner: Owner::Crate {
                    version: note.map(|n| n.version),
                },
                mount: String::new(),
                roots: Roots {
                    decl: Some(dir.clone()),
                    ..Roots::default()
                },
                home: Some(dir),
            }
        })
        .collect()
}

/// `dir` as a path relative to `root` when it is under it, for a module's name.
fn display_under(dir: &Path, root: &Path) -> String {
    dir.strip_prefix(root)
        .unwrap_or(dir)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test, removed first so a rerun starts clean.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("htl-model-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn name(p: &Project, file: &Path) -> Option<String> {
        p.locate(file).map(|pl| pl.name)
    }

    #[test]
    fn own_module_names_by_path_under_its_roots() {
        let root = scratch("own");
        write(&root.join(CONFIG_NAME), "");
        write(&root.join("src/util.tl"), "");
        write(&root.join("src/game/init.tl"), "");
        write(&root.join("src/game/board.tl"), "");
        write(&root.join("tests/util_test.tl"), "");
        write(&root.join("types/socket/http.d.tl"), "");
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        assert_eq!(name(&p, &root.join("src/util.tl")).as_deref(), Some("util"));
        assert_eq!(
            name(&p, &root.join("src/game/init.tl")).as_deref(),
            Some("game")
        );
        assert_eq!(
            name(&p, &root.join("src/game/board.tl")).as_deref(),
            Some("game.board")
        );
        let t = p.locate(&root.join("tests/util_test.tl")).unwrap();
        assert_eq!((t.role, t.name.as_str()), (Role::Test, "util_test"));
        let d = p.locate(&root.join("types/socket/http.d.tl")).unwrap();
        assert_eq!((d.role, d.name.as_str()), (Role::Decl, "socket.http"));
        assert_eq!(d.module.owner, Owner::Own);
        // A host's declaration beside the sources is a declaration all the same.
        write(&root.join("src/host.d.tl"), "");
        let h = p.locate(&root.join("src/host.d.tl")).unwrap();
        assert_eq!((h.role, h.name.as_str()), (Role::Decl, "host"));
        // A file of the project that no role covers has no name in the model.
        assert_eq!(name(&p, &root.join("main.tl")), None);
    }

    #[test]
    fn a_crates_declarations_are_its_own_module_and_keep_their_name() {
        let root = scratch("crate");
        write(
            &root.join("types/htl-mq").join(crate::DEP_TYPES_NOTE),
            "crate = \"htl-mq\"\nversion = \"0.2.0\"\nfiles = [\"mq.d.tl\"]\n",
        );
        write(&root.join("types/htl-mq/mq.d.tl"), "");
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        let pl = p.locate(&root.join("types/htl-mq/mq.d.tl")).unwrap();
        assert_eq!(pl.name, "mq", "not htl-mq.mq through the outer types/ root");
        assert_eq!(pl.module.name, "htl-mq");
        assert_eq!(
            pl.module.owner,
            Owner::Crate {
                version: Some("0.2.0".into())
            }
        );
    }

    #[test]
    fn a_dependency_is_mounted_at_its_name() {
        let dep = Module {
            name: "lshape".into(),
            owner: Owner::Installed,
            mount: "lshape".into(),
            roots: Roots::default(),
            home: None,
        };
        let root = Path::new("/p/.htl/modules/entries/lshape");
        let n = |f: &str| dep.name_of(root, &root.join(f));
        assert_eq!(n("init.tl").as_deref(), Some("lshape"));
        assert_eq!(n("sub.tl").as_deref(), Some("lshape.sub"));
        assert_eq!(n("deep/init.tl").as_deref(), Some("lshape.deep"));
        // A flat package's entry, and only at the top.
        assert_eq!(n("lshape.tl").as_deref(), Some("lshape"));
        assert_eq!(n("deep/lshape.tl").as_deref(), Some("lshape.deep.lshape"));
    }

    #[test]
    fn a_top_mounted_modules_own_init_has_no_name() {
        let own = Module {
            name: "p".into(),
            owner: Owner::Own,
            mount: String::new(),
            roots: Roots::default(),
            home: None,
        };
        let root = Path::new("/p/src");
        assert_eq!(own.name_of(root, &root.join("init.tl")), None);
        assert_eq!(own.name_of(root, Path::new("/elsewhere/x.tl")), None);
        // `?/?` is a mounted module's spelling; at the top it is an ordinary submodule.
        assert_eq!(
            own.name_of(root, &root.join("util/util.tl")).as_deref(),
            Some("util.util")
        );
    }

    #[test]
    fn patched_and_vendored_dependencies_come_from_the_manifest() {
        let root = scratch("deps");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n\n\
             [deps.mathx]\ngit = \"https://example.invalid/mathx\"\ntag = \"v1\"\npatch_dir = \"patches/mathx\"\n\n\
             [deps.lshape]\ngit = \"https://example.invalid/lshape\"\ntag = \"v1\"\ntarget_dir = \"lua/lshape\"\n",
        );
        // The copy's own manifest names its entry, as `htl pkg patch` leaves it.
        write(
            &root.join("patches/mathx").join(pkg::MANIFEST_NAME),
            "[package]\nname = \"mathx\"\nversion = \"1.0.0\"\nentry = \"src/mathx\"\n",
        );
        write(&root.join("patches/mathx/src/mathx/init.tl"), "");
        write(&root.join("patches/mathx/src/mathx/vec.tl"), "");
        write(&root.join("lua/lshape/init.tl"), "");
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        assert_eq!(p.own().name, "game");
        let patched = p
            .locate(&root.join("patches/mathx/src/mathx/vec.tl"))
            .unwrap();
        assert_eq!(patched.module.owner, Owner::Patched);
        assert_eq!(patched.name, "mathx.vec");
        let vendored = p.locate(&root.join("lua/lshape/init.tl")).unwrap();
        assert_eq!(vendored.module.owner, Owner::Vendored);
        assert_eq!(vendored.name, "lshape");
    }

    #[test]
    fn check_paths_are_external_modules_and_missing_ones_are_none() {
        let root = scratch("ext");
        write(&root.join("vendor/json.tl"), "");
        let cfg = HtlConfig::parse("[check]\npaths = [\"vendor\", \"nowhere\"]\n").unwrap();
        let p = Project::load(&root, cfg).unwrap();

        let ext: Vec<&Module> = p
            .modules
            .iter()
            .filter(|m| m.owner == Owner::External)
            .collect();
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0].name, "vendor");
        assert_eq!(
            name(&p, &root.join("vendor/json.tl")).as_deref(),
            Some("json")
        );
    }

    #[test]
    fn one_directory_as_source_and_decl_takes_the_role_by_extension() {
        let own = Module {
            name: "p".into(),
            owner: Owner::Own,
            mount: String::new(),
            roots: Roots {
                source: Some(PathBuf::from("/p/scripts")),
                test: None,
                decl: Some(PathBuf::from("/p/scripts")),
            },
            home: None,
        };
        let p = Project {
            root: PathBuf::from("/p"),
            config: HtlConfig::default(),
            modules: vec![own],
            links: None,
            problems: Vec::new(),
        };
        let d = p.locate(Path::new("/p/scripts/host.d.tl")).unwrap();
        assert_eq!((d.role, d.name.as_str()), (Role::Decl, "host"));
        let s = p.locate(Path::new("/p/scripts/main.tl")).unwrap();
        assert_eq!((s.role, s.name.as_str()), (Role::Source, "main"));
    }

    #[test]
    fn search_dirs_put_a_dependency_s_parent_on_the_path_and_tests_only_for_tests() {
        let root = scratch("search");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n\n\
             [deps.lshape]\ngit = \"https://example.invalid/lshape\"\ntag = \"v1\"\ntarget_dir = \"lua/lshape\"\n",
        );
        write(&root.join("lua/lshape/init.tl"), "");
        write(&root.join(".htl/modules/entries/mq/init.tl"), "");
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        let src = p.search_dirs(View::Source);
        assert_eq!(src[0], root.join("src"));
        assert_eq!(src[1], root.join("types"));
        assert!(src.contains(&root.join("lua")), "{src:?}");
        assert!(src.contains(&root.join(".htl/modules/entries")), "{src:?}");
        assert!(!src.contains(&root.join("tests")), "{src:?}");
        assert!(
            !src.contains(&root),
            "the root is not a source root: {src:?}"
        );
        let test = p.search_dirs(View::Test);
        assert!(test.contains(&root.join("tests")), "{test:?}");
    }

    #[test]
    fn contract_directories_stay_off_the_search_path() {
        let module = |name: &str, owner: Owner, dir: &str| Module {
            name: name.into(),
            owner,
            mount: String::new(),
            roots: Roots {
                source: Some(PathBuf::from(dir)),
                ..Roots::default()
            },
            home: Some(PathBuf::from(dir)),
        };
        let p = Project {
            root: PathBuf::from("/p"),
            config: HtlConfig::default(),
            modules: vec![
                module("p", Owner::Own, "/p/src"),
                module("sites/a", Owner::Contract, "/p/sites/a"),
                module("sites/b", Owner::Contract, "/p/sites/b"),
                module("vendor", Owner::External, "/p/vendor"),
            ],
            links: None,
            problems: Vec::new(),
        };
        let dirs = p.search_dirs(View::Source);
        assert!(!dirs.iter().any(|d| d.starts_with("/p/sites")), "{dirs:?}");
        assert!(dirs.contains(&PathBuf::from("/p/vendor")), "{dirs:?}");
    }

    #[test]
    fn a_walk_skips_a_dependency_s_whole_copy_and_a_patch_only_when_not_checking() {
        let root = scratch("walk");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n\n\
             [deps.mathx]\ngit = \"https://example.invalid/mathx\"\ntag = \"v1\"\npatch_dir = \"patches/mathx\"\n\n\
             [deps.lshape]\ngit = \"https://example.invalid/lshape\"\ntag = \"v1\"\ntarget_dir = \"lua/lshape\"\n",
        );
        write(&root.join("patches/mathx/src/mathx/init.tl"), "");
        write(&root.join("lua/lshape/init.tl"), "");
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        let check = p.not_walked(Purpose::Check);
        assert!(check.contains(&root.join("lua/lshape")), "{check:?}");
        assert!(!check.contains(&root.join("patches/mathx")), "{check:?}");
        for purpose in [Purpose::Own, Purpose::Test] {
            let skip = p.not_walked(purpose);
            // The copy, not its entry: its tests and manifest are the dependency's too.
            assert!(skip.contains(&root.join("patches/mathx")), "{skip:?}");
            assert!(skip.contains(&root.join("lua/lshape")), "{skip:?}");
        }
    }

    #[test]
    fn a_name_two_modules_implement_is_a_conflict_and_a_declaration_is_not() {
        let root = scratch("conflict");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
        );
        // An installed dependency `mathx`, with a submodule under its own name.
        write(&root.join(".htl/modules/entries/mathx/init.tl"), "");
        write(&root.join(".htl/modules/entries/mathx/vec.tl"), "");
        // The project implements `mathx` too, and `vec` at the top — which is not `mathx.vec`.
        write(&root.join("src/mathx.tl"), "");
        write(&root.join("src/vec.tl"), "");
        // A declaration beside an implementation elsewhere is not a second claim.
        write(&root.join("types/mq.d.tl"), "");
        write(&root.join(".htl/modules/entries/mq/init.tl"), "");
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        let conflicts = p.conflicts(View::Source);
        let names: Vec<&str> = conflicts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["mathx"], "{conflicts:?}");
        let owners: Vec<&Owner> = conflicts[0]
            .claims
            .iter()
            .map(|c| &c.module.owner)
            .collect();
        assert_eq!(owners, [&Owner::Own, &Owner::Installed]);
    }

    #[test]
    fn discover_refuses_two_roots() {
        let root = scratch("two-roots");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"p\"\nversion = \"0.1.0\"\n",
        );
        write(&root.join("sub").join(CONFIG_NAME), "");
        let err = Project::discover(&root.join("sub")).unwrap_err();
        assert!(err.to_string().contains("different project roots"), "{err}");

        let flat = scratch("one-root");
        write(&flat.join(CONFIG_NAME), "");
        let p = Project::discover(&flat).unwrap().unwrap();
        assert_eq!(canon(&p.root), canon(&flat));
    }
}
