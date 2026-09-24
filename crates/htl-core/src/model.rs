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
//! # Where a name is answered
//!
//! By the model's [`Resolver`], for every name the model has: the checker, the run-time
//! searcher, the linker and `htl resolve` all ask it, and none of the model's directories
//! is on `package.path` ([`Htl::apply_model`](crate::Htl::apply_model)). The path is left
//! to what the model does not have — the directories Lua searches for libraries installed
//! on the machine. Every command that checks, runs or bundles a file sets its checker up
//! from the model — `htl check`, `test`, `fix`, `gen`, `run`, `build`, `resolve` and the
//! `include_tl!` / `include_bundle!` macros — and a file that belongs to no project reads
//! its own directory instead ([`project::file_view`](crate::project::file_view)). The run
//! cache records, for each entry, what the resolver answers for every name the module
//! required ([`with_model`](crate::project::with_model)), and replays the entry only while
//! those answers stand.
//!
//! # What the model knows about the host
//!
//! Some names are answered by no file at all: the host registers them in
//! `package.preload` before any script runs. The model records which names those are and
//! where each is declared ([`Project::provides`], [`Provider`]), from the three places a
//! project may declare one:
//!
//! - a `#[host_module]` in the Rust crate around the project ([`Project::host_crate`]),
//!   read from its sources when the model is loaded;
//! - `[build] host` in `htl.toml`;
//! - `std.*`, which htl's own binary provides when it is built with the `std` feature.
//!
//! This is the host's place in the model, and it is a table of names rather than an
//! [`Owner`] with modules under it: a host module has no files the model owns — its
//! implementation is compiled Rust — so there is nothing for a root, a home or
//! [`locate`](Project::locate) to hold. The `.d.tl` a host generates for one is still a
//! file of whichever module's root it is written under, and is named there like any other
//! declaration.
//!
//! One more kind of name runs from no file of the project: a name the model has only a
//! declaration for — a `.d.tl` and no `.tl` or `.lua` — and none of the three sources
//! names ([`Provider::Declared`]). The environment provides it at run time: a Lua library
//! installed on the machine (`types/socket/http.d.tl` for LuaSocket), a module some host
//! registers that the model cannot read. A hand-written declaration is the ordinary way
//! a project types such a library, so it needs no configuration beside it; `[build]
//! host` is for a name the model cannot see at all, one with no declaration. This one is
//! read from the files, not from the configuration, so the [`Resolver`] answers it
//! ([`Resolver::provides`], [`Resolver::provided`]) — the model does not walk its roots
//! when it is loaded, and [`Project::provides`] stays the three configured sources.
//!
//! A file of the model that implements a name the host provides — a `src/host.tl` or
//! `src/host.lua` beside `#[host_module(name = "host")]` — is an error: the check would
//! read the file and a run with the host would load the host's module. The
//! [`Resolver`] answers such a name with [`Resolution::HostShadowed`], which `htl check`
//! reports at every `require` of the name and `htl run` / `htl test` raise at run time, the
//! way a name two files implement is an error in both. A `.d.tl` of the name is not an
//! implementation; it is how the host module is typed, and the checker reads it.
//!
//! # What this module does not do
//!
//! It does not load a host module's files. Everything that asks which names run from no
//! file of the project asks the model: the check and the run time through the resolver;
//! `htl build`, `include_bundle!` and `htl unused` through [`Project::provided`], which
//! they list the configured names from; and the linker and `htl resolve` through
//! [`Resolver::provides`], which adds the declared ones. The linker used to decide the
//! last kind itself — "a name that resolves to a `.d.tl` with no `.lua` behind it is the
//! host's" — from its own lookup; it now asks the resolver, and keeps that lookup only for
//! a file in no project, which has no model to ask (#321). `--host` and
//! `include_bundle!(host = [..])` add names a caller knows and the model cannot read (a
//! module registered by hand, or by another crate, with no declaration); they do not
//! replace it.
//!
//! A host that serves modules itself describes the directories it serves as a model too
//! ([`Project::for_host`], [`HostDir`]), and derives both sides from it: the checker with
//! [`Htl::apply_model`](crate::Htl::apply_model), the run with
//! [`TealResolver::from_project`](crate::pkg::TealResolver::from_project), which names
//! files by the same rule ([`naming`](crate::naming)) on every `require` rather than from
//! a table built once, because a host's directory changes while it runs.

pub mod resolver;
pub use resolver::{Found, HostShadowed, Resolution, Resolver};

use crate::config::{CONFIG_NAME, HtlConfig, resolve_path};
use crate::pkg;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Where a project is, as [`Project::find_root`] found it: before its modules are read.
#[derive(Debug, Clone)]
pub struct ProjectRoot {
    /// The directory holding its `htl.toml` and / or `mlua-pkg.toml`.
    pub root: PathBuf,
    /// Its `htl.toml`, when it has one: a project described by `mlua-pkg.toml` alone is
    /// configured by the defaults.
    pub config_file: Option<PathBuf>,
    /// What `htl.toml` says, or the defaults.
    pub config: HtlConfig,
}

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
    /// `mlua-pkg.toml`. The one directory through which every installed dependency —
    /// linked under `vendored/` or copied to a `target_dir` — answers to its name, and what
    /// the run cache keys a dependency's files on.
    pub links: Option<PathBuf>,
    /// What reading the project could not make sense of, as messages. The model is built
    /// from the rest; nothing here stops a load. Today these come from `---@contract`
    /// markers that do not parse.
    pub problems: Vec<String>,
    /// The Rust crate that hosts the project: the nearest directory at or above the root
    /// whose `Cargo.toml` has a `[package]` ([`find_cargo_package_root`]). `None` for a
    /// project with no Cargo package around it, which has no `#[host_module]` names.
    ///
    /// A walk for a Cargo manifest, not for a project: [`Project::find_root`] stays the one
    /// walk for `htl.toml` / `mlua-pkg.toml`.
    ///
    /// [`find_cargo_package_root`]: crate::dts::find_cargo_package_root
    pub host_crate: Option<PathBuf>,
    /// Every name the host provides, with where the name comes from, one entry per name,
    /// in name order. Read with [`provides`](Self::provides) and
    /// [`provided`](Self::provided).
    providers: Vec<(String, Provider)>,
}

/// Where a name that runs from no file of the project comes from — the sources a
/// project may declare a host-provided name in, and the one the files themselves say.
///
/// A host module has no files the model owns: its implementation is Rust, compiled into
/// whatever runs the project, and what the project holds of it is at most a `.d.tl`. So
/// the model does not describe it as a [`Module`] with roots and a home; it records the
/// name and which of these said so.
///
/// The first three are read from the configuration and the crate around the project when
/// the model is loaded, and [`Project::provides`] answers them. The fourth,
/// [`Declared`](Provider::Declared), is read from the files: only the [`Resolver`], which
/// walks the model's roots, knows that a name has a declaration and nothing else, so
/// [`Resolver::provides`] is the one that answers all four.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Registered by a `#[host_module]` in the host crate's Rust sources
    /// ([`Project::host_crate`]): the name is the attribute's `name = ".."`, or the impl
    /// target lowercased — the key the generated code puts in `package.preload`.
    HostModule,
    /// Named in `htl.toml`'s `[build] host`: a module the project says the host provides,
    /// for a host whose registration the model cannot read (one written by hand, or in
    /// another crate).
    Build,
    /// A `std.*` module — and `std` itself — which htl's own binary and
    /// [`Htl::install_std`](crate::Htl::install_std) provide. Only in a build with the
    /// `std` feature; without it nothing provides these names.
    Std,
    /// Declared and nothing else: the model has a `.d.tl` under the name and no `.tl` or
    /// `.lua`, and none of the three sources above names it. The environment provides it
    /// at run time — a Lua library installed on the machine, a module a host registers
    /// that the model cannot read (by hand, or from another crate) — and the declaration
    /// is how the project says so and how it is typed.
    ///
    /// Where the declaration sits does not matter. The declaration root (`[layout]
    /// types`) is where hand-written ones go, a crate's under `types/<crate>/`; a host's
    /// generated `src/host.d.tl` beside the sources says the same. A `.lua` behind the
    /// declaration makes the name the project's own — the `.lua` is what runs, and is
    /// bundled — and a `.tl` makes it an implementation, so neither is `Declared`.
    ///
    /// Only [`Resolver::provides`] answers it, from the files. It is never
    /// [`Resolution::HostShadowed`]: a name with an implementation is not `Declared` by
    /// definition.
    Declared,
}

impl Provider {
    /// Rank when two sources name one module: the higher wins.
    ///
    /// The more specific source wins. A `#[host_module]` is the registration itself, read
    /// from the code that performs it; `[build] host` is the project saying a host
    /// provides the name, which may be that same registration written down a second time;
    /// `std.*` is what any htl binary carries, whoever the host is. So a name both
    /// registered and listed is reported as registered, and a host that registers a
    /// `std.*` name of its own is the one that answers to it. A declaration alone says
    /// least — that something provides the name, not what — so
    /// [`Declared`](Provider::Declared) is the lowest, and any of the others that names
    /// the name is the answer instead.
    fn rank(self) -> u8 {
        match self {
            Provider::HostModule => 3,
            Provider::Build => 2,
            Provider::Std => 1,
            Provider::Declared => 0,
        }
    }

    /// The word the checker's Lua state carries a provider as, and
    /// [`Provision::Provided`](crate::Provision::Provided) hands on: `host_module`,
    /// `build`, `std`, `declared`. A word rather than this type because the linker, which
    /// reads it, is compiled without the model.
    pub fn tag(self) -> &'static str {
        match self {
            Provider::HostModule => "host_module",
            Provider::Build => "build",
            Provider::Std => "std",
            Provider::Declared => "declared",
        }
    }
}

/// One directory a host serves modules from, and how its files are named: what
/// [`Project::for_host`] builds a host's model from.
///
/// The three are the three ways a directory can hold modules, each with the naming rule
/// ([`crate::naming`]) applied from a different mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDir {
    /// A directory of modules mounted at the top: `<dir>/a/b.tl` is `a.b`, `<dir>/a/init.tl`
    /// is `a`, and `<dir>/util/util.tl` is `util.util` and nothing else. A host's script
    /// directory. What [`TealResolver::new`](crate::pkg::TealResolver::new) serves.
    Modules(PathBuf),
    /// A directory of packages, each a child directory mounted at its name:
    /// `<dir>/mathx/init.tl` and a flat package's `<dir>/mathx/mathx.tl` are `mathx`,
    /// `<dir>/mathx/sub.tl` is `mathx.sub`, and `<dir>/a/b/a/b.tl` is `a.b.a.b`. A mod
    /// directory with one folder per mod. What
    /// [`holding_packages`](crate::pkg::TealResolver::holding_packages) serves.
    Packages(PathBuf),
    /// A declaration root: `.d.tl` mounted at the top, and every `<dir>/<crate>/` that
    /// `htl dts` materialised a crate's declarations into a root of its own, so that
    /// `types/htl-mq/mq.d.tl` is `mq` — the name the crate wrote it under — and not
    /// `htl-mq.mq`. The layout `[layout] types` has in a project.
    Declarations(PathBuf),
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

impl Owner {
    /// Whose problem an error in this module's files is, as a diagnostic says it in its
    /// `origin`: `dependency` for what an install or a crate brought in — an installed or
    /// vendored copy, a crate's declarations — which the project changes by changing the
    /// dependency; `external` for a `[check] paths` or contract directory, supplied from
    /// outside; none for the project's own, a patch it took over included, and htl's
    /// library.
    ///
    /// `htl resolve` names the same modules by how they arrived (`crate`, `vendored`, …);
    /// both read the owner, so a file is never one thing to the one and another to the other.
    pub fn origin(&self) -> Option<&'static str> {
        match self {
            Owner::Installed | Owner::Vendored | Owner::Crate { .. } => Some("dependency"),
            Owner::External | Owner::Contract => Some("external"),
            Owner::Own | Owner::Patched | Owner::Lib => None,
        }
    }
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
        match Self::find_root(start)? {
            Some(r) => Self::load(&r.root, r.config).map(Some),
            None => Ok(None),
        }
    }

    /// Where the project above `start` is, without building it: its root, its `htl.toml`
    /// when it has one, and the configuration — the walk [`discover`](Self::discover) does,
    /// for a caller that needs the root or the config before, or instead of, the model.
    ///
    /// This is the one walk up for a manifest. Everything that asks "which project is
    /// this" asks it here, so an `htl.toml` and an `mlua-pkg.toml` naming two different
    /// roots are refused by every command rather than only by those that happened to build
    /// the model this way.
    pub fn find_root(start: &Path) -> Result<Option<ProjectRoot>> {
        let config = HtlConfig::find(start)?;
        let manifest = pkg::MluaProject::find(start);
        let found = match (config, manifest) {
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
                ProjectRoot {
                    root,
                    config_file: Some(path),
                    config: cfg,
                }
            }
            (None, Some(m)) => ProjectRoot {
                root: m.root,
                config_file: None,
                config: HtlConfig::default(),
            },
        };
        Ok(Some(found))
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
        let host_crate = crate::dts::find_cargo_package_root(root);
        let providers = providers(host_crate.as_deref(), &config);
        Ok(Self {
            root: root.to_path_buf(),
            config,
            modules,
            links: manifest.as_ref().map(|m| m.entries.clone()),
            problems,
            host_crate,
            providers,
        })
    }

    /// The model of what a host serves, for a host that has no `htl.toml` to
    /// [`load`](Self::load): `root` and the directories in `dirs`, each read the way its
    /// [`HostDir`] says (#320).
    ///
    /// # Why a host describes its directories
    ///
    /// A host that serves `.tl` modules at run time — a game's mod directory, a plugin
    /// folder — used to say where they are twice: once to the checker
    /// ([`Htl::add_path`](crate::Htl::add_path),
    /// [`add_package_path`](crate::Htl::add_package_path),
    /// [`apply_config`](crate::Htl::apply_config)) and once to the run
    /// ([`TealResolver::new`](crate::pkg::TealResolver::new),
    /// [`holding_packages`](crate::pkg::TealResolver::holding_packages)). The two did not
    /// name files the same way: the checker read the directories through `package.path`
    /// templates, which answer `require("a.b")` with `a/b/a/b.tl` and read a crate's
    /// `types/htl-mq/mq.d.tl` as both `mq` and `htl-mq.mq`, while the run read them by the
    /// naming rule ([`crate::naming`]). A script could check and then fail to load.
    ///
    /// Described once, as a model, both sides are derived from the one description: the
    /// checker with [`Htl::apply_model`](crate::Htl::apply_model) — the call every `htl`
    /// command makes for a project — and the run with
    /// [`TealResolver::from_project`](crate::pkg::TealResolver::from_project). Every name
    /// is then answered by the same module, from the same file, on both sides.
    ///
    /// # What the model holds
    ///
    /// - The host's own module ([`Owner::Own`]), named after `root`'s last component, with
    ///   `root` as its home and no roots: a host's code is Rust, and what it serves are
    ///   other people's modules. A model has exactly one `Own` module, so this is it.
    /// - Per [`HostDir::Modules`], one module mounted at the top, owned
    ///   [`Owner::External`]: modules the host did not write and reaches anyway, which is
    ///   what a `[check] paths` entry is to a project.
    /// - Per [`HostDir::Packages`], one module per child directory, mounted at the child's
    ///   name and owned [`Owner::Installed`]: each child is a package put there by
    ///   something other than the host's author, the way an install puts a dependency
    ///   under `.htl/modules`, and like a dependency it sees its own modules and the rest
    ///   of the host's, not a project's own. The children are read when this is called;
    ///   a package directory added later is a new model (a file added later to a package
    ///   that was there is not — see [`from_project`](crate::pkg::TealResolver::from_project)).
    /// - Per [`HostDir::Declarations`], one [`Owner::External`] module whose declaration
    ///   root is the directory, and one [`Owner::Crate`] module per `<dir>/<crate>/`
    ///   materialised by `htl dts`, as [`load`](Self::load) builds them: `types/htl-mq/mq.d.tl`
    ///   is `mq`, and nothing else.
    /// - htl's own library ([`Owner::Lib`]), as for every project.
    ///
    /// A relative directory is relative to `root`. A directory that does not exist
    /// contributes a module with nothing in it, as a missing root does in a loaded project.
    ///
    /// The model names no host-provided names ([`provides`](Self::provides) answers `None`):
    /// a host registers its modules in `package.preload` itself, and the run-time resolver
    /// steps aside for a declaration whose name is preloaded.
    pub fn for_host(root: &Path, dirs: &[HostDir]) -> Self {
        let root = root.to_path_buf();
        let config = HtlConfig::default();
        let mut modules = vec![Module {
            name: canon(&root)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            owner: Owner::Own,
            mount: String::new(),
            roots: Roots::default(),
            home: Some(root.clone()),
        }];
        for d in dirs {
            match d {
                HostDir::Modules(dir) => {
                    let dir = root.join(dir);
                    modules.push(Module {
                        name: display_under(&dir, &root),
                        owner: Owner::External,
                        mount: String::new(),
                        roots: Roots {
                            source: Some(dir.clone()),
                            ..Roots::default()
                        },
                        home: Some(dir),
                    });
                }
                HostDir::Packages(dir) => {
                    let dir = root.join(dir);
                    let mut children: Vec<(String, PathBuf)> = std::fs::read_dir(&dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .filter(|e| e.path().is_dir())
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                        .filter(|(n, _)| !n.starts_with('.'))
                        .collect();
                    children.sort();
                    for (name, path) in children {
                        modules.push(dependency(&name, Owner::Installed, path.clone(), path));
                    }
                }
                HostDir::Declarations(dir) => {
                    let dir = root.join(dir);
                    modules.push(Module {
                        name: display_under(&dir, &root),
                        owner: Owner::External,
                        mount: String::new(),
                        roots: Roots {
                            decl: Some(dir.clone()),
                            ..Roots::default()
                        },
                        home: Some(dir.clone()),
                    });
                    modules.extend(crate_modules(&dir));
                }
            }
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
        Self {
            root,
            config,
            modules,
            links: None,
            problems: Vec::new(),
            host_crate: None,
            providers: Vec::new(),
        }
    }

    /// Whether the host provides `name`, and from which of the three sources the model
    /// reads when it is loaded — a `#[host_module]`, `[build] host`, `std.*`. `None` for a
    /// name none of them declares — which is every name a module of the project answers
    /// to, and every name nothing answers to.
    ///
    /// A name two sources declare is answered once, by the more specific
    /// ([`Provider::HostModule`] over [`Provider::Build`] over [`Provider::Std`]).
    ///
    /// Never [`Provider::Declared`]: that is a fact about the files under the name, and
    /// the model does not walk its roots when it is loaded — its [`Resolver`] does. A
    /// caller that has one asks [`Resolver::provides`], which answers this and that.
    pub fn provides(&self, name: &str) -> Option<Provider> {
        self.providers
            .binary_search_by(|(n, _)| n.as_str().cmp(name))
            .ok()
            .map(|i| self.providers[i].1)
    }

    /// Every name the host provides, with its source, in name order: the table
    /// [`provides`](Self::provides) reads, for a caller that needs all of it (a lint that
    /// asks of every `require`, a linker leaving names out of a bundle). The three
    /// configured sources only, as for `provides`; [`Resolver::provided`] adds the
    /// declared names.
    pub fn provided(&self) -> impl Iterator<Item = (&str, Provider)> {
        self.providers.iter().map(|(n, p)| (n.as_str(), *p))
    }

    /// The project's own module.
    pub fn own(&self) -> &Module {
        self.modules
            .iter()
            .find(|m| m.owner == Owner::Own)
            .expect("Project::load always makes the project's own module")
    }

    /// The module whose home holds `file` most specifically: whose directory the file is
    /// in, which is not always a module it is named by. A patched dependency's own tests
    /// sit in its copy (`patches/mathx/tests/`) and outside its entry, where no name reaches
    /// them; they are the dependency's all the same. `None` for a file outside every home —
    /// outside the project and everything it took on.
    ///
    /// A module's roots are its as well as its home: an installed dependency is read
    /// through its link under `entries/`, which lies outside the copy the link points at.
    ///
    /// What a file's origin and a walk's count of a dependency's files are decided by;
    /// [`locate`](Self::locate) is what names it.
    pub fn home_of(&self, file: &Path) -> Option<&Module> {
        let target = canon(file);
        self.modules
            .iter()
            .flat_map(|m| {
                m.home
                    .iter()
                    .map(PathBuf::as_path)
                    .chain(m.roots.iter().map(|(_, r)| r))
                    .map(move |d| (canon(d), m))
            })
            .filter(|(d, _)| target.starts_with(d))
            .max_by_key(|(d, _)| d.components().count())
            .map(|(_, m)| m)
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
    /// project has an `mlua-pkg.toml`), and the model's [`Resolver`] answering every name
    /// ([`install_resolver`](Self::install_resolver)).
    ///
    /// None of the model's directories goes on `package.path`. A directory on the path is
    /// read through templates that do not know whose files they find — the reason the
    /// model exists — and with the resolver answering first they could only ever answer
    /// what the model does not have, under a name the model does not give. What stays on
    /// the path is what Lua was built to search (`/usr/local/share/lua/5.4/…`), for a
    /// library installed on the machine.
    ///
    /// `view` is kept for the callers' sake: which roots a file may read is the resolver's
    /// to decide, from the requiring file, and a test sees the test root because it is
    /// under it.
    pub fn apply_model(&self, project: &Project, view: View) -> Result<()> {
        // The names come from the project's roots, so the working directory is not one of
        // the places they are looked for.
        self.drop_cwd_search_path()?;
        if project.root.join(pkg::MANIFEST_NAME).is_file() {
            self.prepare_deps(&pkg::MluaProject::at(&project.root))?;
        }
        // No directory of the model goes on the path: the resolver answers every name the
        // model has, from the file the model names, and the path is left to what the model
        // does not have — the directories Lua itself searches for libraries installed on
        // the machine. `view` is the resolver's to apply, from the requiring file.
        let _ = view;
        self.install_resolver(Resolver::new(project))
    }

    /// Have the checker ask `resolver` for every module name.
    ///
    /// Five functions go into the prelude, made in the checker's state: `resolve_name`
    /// (requirer, name) → kind and files, which the prelude's `tl.search_module` asks
    /// first; `rewrite_name` (requirer, name) → the name a `require` is to be written as,
    /// which its `tl.parse` wrapper applies; and `owns_file` (path) → whether a file is the
    /// model's, which holds a `package.path` search for a name the model does not have to
    /// the files outside it; and `claims_name` (name) → every file of the model under the
    /// name ([`Resolver::claims`]), which `duplicate-declaration` lists declarations from.
    /// The kinds are `found`, `ambiguous`, `missing`, `outside`, `hidden` and `shadowed`
    /// ([`Resolution`]). `shadowed` carries the error and the name's declaration, when it
    /// has one, which is what the checker types the `require` from.
    ///
    /// And `provides_name` (name) → who provides a name that runs from no file of the project
    /// ([`Resolver::provides`], as [`Provider`]'s tag), `"none"` for a name the model has
    /// files for that are what runs, and nil for a name the model has nothing under: what
    /// the linker asks before it leaves a declared name out of a bundle
    /// ([`Htl::model_provides`](crate::Htl::model_provides)).
    ///
    /// A further one, `refresh_model` (), builds the table again from the same project: the
    /// table is read from the directories once, here, and a host's directory gains files
    /// while it runs. [`TealResolver::from_project`](crate::pkg::TealResolver::from_project)
    /// calls it before it checks a file the table does not have, so the check of a module
    /// dropped in after the host started reads the host's directories as they are now, as
    /// the run does. No `htl` command calls it: a command's tree does not change under it.
    ///
    /// That covers the file being served, not what it requires: a module the table has
    /// had since the host started may require one dropped in since, and the check asks
    /// `resolve_name` for that name, not the resolver. So the last one, `watch_model` (),
    /// has `resolve_name` do the same for a name the table answers `outside` or `missing`
    /// when a directory of the model now has a file under it (a `stat` per spelling of
    /// the name per root, not a walk). `TealResolver::from_project` calls it; a command,
    /// which does not, asks nothing more than the table.
    pub fn install_resolver(&self, resolver: Resolver) -> Result<()> {
        let lua = self.checker_lua()?;
        let r = std::sync::Arc::new(std::sync::RwLock::new(resolver));
        // A poisoned lock is a panic in a closure below, which already failed its call; the
        // table itself is whole, since it is only ever replaced.
        fn read(r: &std::sync::RwLock<Resolver>) -> std::sync::RwLockReadGuard<'_, Resolver> {
            r.read().unwrap_or_else(std::sync::PoisonError::into_inner)
        }
        let s = |p: Option<PathBuf>| p.map(|p| p.to_string_lossy().into_owned());
        // Set by `watch_model`: the table is a running host's, whose directories gain files.
        let live = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resolve = {
            let r = r.clone();
            let live = live.clone();
            lua.create_function(move |_, (requirer, name): (Option<String>, String)| {
                type Answer = (String, Option<String>, Option<String>, Option<String>);
                let requirer = requirer.as_deref().map(Path::new);
                let mut resolution = read(&r).resolve(requirer, &name);
                // A module the host has had since it started may require one dropped in
                // since: the name is outside the table, and in the directory.
                if matches!(resolution, Resolution::Outside | Resolution::Missing)
                    && live.load(std::sync::atomic::Ordering::Relaxed)
                    && read(&r).gained(&name)
                {
                    let fresh = read(&r).rebuilt();
                    *r.write().unwrap_or_else(std::sync::PoisonError::into_inner) = fresh;
                    resolution = read(&r).resolve(requirer, &name);
                }
                let r = read(&r);
                let answer: Answer = match resolution {
                    Resolution::Found(f) => (
                        "found".into(),
                        s(f.implementation),
                        s(f.declaration),
                        s(f.lua),
                    ),
                    Resolution::Ambiguous(claims) => {
                        let who: Vec<String> = claims
                            .iter()
                            .map(|(m, f)| format!("{m} ({})", r.show(f)))
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
                    Resolution::HostShadowed(h) => {
                        ("shadowed".into(), Some(h.message), s(h.declaration), None)
                    }
                };
                Ok(answer)
            })?
        };
        let rewrite = {
            let r = r.clone();
            lua.create_function(move |_, (requirer, name): (String, String)| {
                Ok(read(&r).rewrite(Path::new(&requirer), &name))
            })?
        };
        let claims = {
            let r = r.clone();
            lua.create_function(move |_, name: String| {
                Ok(read(&r)
                    .claims(&name)
                    .into_iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>())
            })?
        };
        let owns = {
            let r = r.clone();
            lua.create_function(move |_, path: String| Ok(read(&r).owns(Path::new(&path))))?
        };
        let provides = {
            let r = r.clone();
            lua.create_function(move |_, name: String| {
                let r = read(&r);
                Ok(match r.provides(&name) {
                    Some(p) => Some(p.tag()),
                    None if matches!(
                        r.resolve(None, &name),
                        Resolution::Outside | Resolution::Missing
                    ) =>
                    {
                        None
                    }
                    None => Some("none"),
                })
            })?
        };
        let refresh = lua.create_function(move |_, ()| {
            let fresh = read(&r).rebuilt();
            *r.write().unwrap_or_else(std::sync::PoisonError::into_inner) = fresh;
            Ok(())
        })?;
        let watch = lua.create_function(move |_, ()| {
            live.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        })?;
        self.h.set("claims_name", claims)?;
        self.h.set("resolve_name", resolve)?;
        self.h.set("rewrite_name", rewrite)?;
        self.h.set("owns_file", owns)?;
        self.h.set("provides_name", provides)?;
        self.h.set("refresh_model", refresh)?;
        self.h.set("watch_model", watch)?;
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

/// The names the host provides, from the three sources the model reads, one entry per
/// name sorted by it; where two sources name one module the higher [`Provider::rank`]
/// keeps it.
///
/// The host crate is scanned here, once per load, with the same scan `htl check` ran for
/// its lint ([`host_module_names`](crate::dts::host_module_names)): a substring test per
/// `.rs` file, and a parse only of those that mention `host_module`.
fn providers(host_crate: Option<&Path>, config: &HtlConfig) -> Vec<(String, Provider)> {
    use std::collections::BTreeMap;
    let mut table: BTreeMap<String, Provider> = BTreeMap::new();
    let mut add = |name: String, p: Provider| {
        let slot = table.entry(name).or_insert(p);
        if p.rank() > slot.rank() {
            *slot = p;
        }
    };
    #[cfg(feature = "std")]
    for n in crate::batteries::module_names() {
        add(n, Provider::Std);
    }
    for n in &config.build.host {
        add(n.clone(), Provider::Build);
    }
    for n in host_crate
        .map(crate::dts::host_module_names)
        .unwrap_or_default()
    {
        add(n, Provider::HostModule);
    }
    table.into_iter().collect()
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
            host_crate: None,
            providers: Vec::new(),
        };
        let d = p.locate(Path::new("/p/scripts/host.d.tl")).unwrap();
        assert_eq!((d.role, d.name.as_str()), (Role::Decl, "host"));
        let s = p.locate(Path::new("/p/scripts/main.tl")).unwrap();
        assert_eq!((s.role, s.name.as_str()), (Role::Source, "main"));
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

    /// A Cargo package at `root` whose `src/lib.rs` registers `host` with a
    /// `#[host_module]`, the way `htl new --embed` scaffolds one.
    fn host_crate(root: &Path) {
        write(
            &root.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        write(
            &root.join("src/lib.rs"),
            "use htl::host_module;\n\npub struct Host;\n\n\
             #[host_module(name = \"host\", dts = \"src/host.d.tl\")]\n\
             impl Host {\n    pub fn greet(&self, who: &str) -> String {\n        \
             format!(\"hello, {who}\")\n    }\n}\n",
        );
    }

    #[test]
    fn a_host_module_in_the_crate_around_the_project_is_provided_by_it() {
        let root = scratch("host-module");
        write(&root.join(CONFIG_NAME), "");
        host_crate(&root);
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        assert_eq!(p.host_crate.as_deref().map(canon), Some(canon(&root)));
        assert_eq!(p.provides("host"), Some(Provider::HostModule));
        assert_eq!(p.provides("nothing.provides.this"), None);
        assert!(
            p.provided()
                .any(|(n, pr)| n == "host" && pr == Provider::HostModule),
            "{:?}",
            p.provided().collect::<Vec<_>>()
        );
    }

    #[test]
    fn build_host_names_a_provided_module_and_a_host_module_outranks_it() {
        let root = scratch("build-host");
        let cfg = HtlConfig::parse("[build]\nhost = [\"game\", \"host\"]\n").unwrap();
        let p = Project::load(&root, cfg.clone()).unwrap();
        assert_eq!(p.provides("game"), Some(Provider::Build));
        assert_eq!(p.provides("host"), Some(Provider::Build));

        // The same name registered by a `#[host_module]` is answered once, by the
        // registration.
        host_crate(&root);
        let p = Project::load(&root, cfg).unwrap();
        assert_eq!(p.provides("host"), Some(Provider::HostModule));
        assert_eq!(p.provides("game"), Some(Provider::Build));
        assert_eq!(p.provided().filter(|(n, _)| *n == "host").count(), 1);
    }

    #[test]
    fn a_project_with_no_cargo_package_has_no_host_modules() {
        let root = scratch("no-crate");
        write(&root.join(CONFIG_NAME), "");
        // A `#[host_module]` in a file that belongs to no Cargo package registers nothing.
        write(
            &root.join("src/lib.rs"),
            "#[host_module(name = \"host\")]\nimpl Host {}\n",
        );
        let p = Project::load(&root, HtlConfig::default()).unwrap();

        assert_eq!(p.host_crate, None);
        assert_eq!(p.provides("host"), None);
        assert!(!p.provided().any(|(_, pr)| pr == Provider::HostModule));
    }

    #[cfg(feature = "std")]
    #[test]
    fn std_modules_are_provided_by_the_binary() {
        let root = scratch("std");
        let p = Project::load(&root, HtlConfig::default()).unwrap();
        assert_eq!(p.provides("std.json"), Some(Provider::Std));
        assert_eq!(p.provides("std"), Some(Provider::Std));
        assert_eq!(p.provides("json"), None, "only under the std prefix");
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
