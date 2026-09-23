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
//! # What this module does not do yet
//!
//! It builds the model and answers questions about files. Checking, testing, building
//! and the run cache still assemble `package.path` the way they did before, and a name
//! still resolves through that path. Moving each of them onto the model is what the model
//! is for; until then it is a description of the project that the rest of htl can be
//! compared against. Host modules — the `.d.tl` a Rust host generates from
//! `#[host_module]` — are not loaded here: they are known to whoever compiled the host,
//! and enter a project through the file they are written to.

use crate::config::{CONFIG_NAME, HtlConfig, resolve_path};
use crate::pkg;
use anyhow::{Result, bail};
use std::path::{Component, Path, PathBuf};

/// Where a project keeps its tests when nothing says otherwise, relative to its root.
///
/// `htl new` writes `tests/<mod>_test.tl`, and a test file under this directory can
/// `require` the project's sources by their own names. It is the test root of the
/// project's own module ([`Roots::test`]).
pub const TESTS_DIR: &str = "tests";

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
        let rel = strip_under(file, root)?;
        let mut parts: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        let last = parts.pop()?;
        parts.push(module_stem(&last).to_string());
        let names_its_directory = parts.last().is_some_and(|s| s == "init")
            || (parts.len() == 1 && !self.mount.is_empty() && self.mount_last() == parts[0]);
        if names_its_directory {
            parts.pop();
        }
        let mut name: Vec<&str> = Vec::new();
        if !self.mount.is_empty() {
            name.push(&self.mount);
        }
        name.extend(parts.iter().map(String::as_str));
        (!name.is_empty()).then(|| name.join("."))
    }

    fn mount_last(&self) -> &str {
        self.mount.rsplit('.').next().unwrap_or(&self.mount)
    }
}

/// `util.d.tl` -> `util`, `util.tl` -> `util`, `util.lua` -> `util`; anything else as is.
fn module_stem(file_name: &str) -> &str {
    [".d.tl", ".tl", ".lua"]
        .iter()
        .find_map(|ext| file_name.strip_suffix(ext))
        .unwrap_or(file_name)
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
        let manifest = pkg::Project::find(start);
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
            .then(|| pkg::Project::at(root));

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
        });
        Ok(Self {
            root: root.to_path_buf(),
            config,
            modules,
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

/// The project's own module: named by `mlua-pkg.toml`'s `[package] name` when it has one
/// that parses, by the root directory otherwise; mounted at the top; its roots from
/// `[layout]` and [`TESTS_DIR`].
fn own_module(root: &Path, config: &HtlConfig, manifest: Option<&pkg::Project>) -> Module {
    let name = manifest
        .and_then(|m| mlua_pkg::manifest::Manifest::from_path(&m.manifest).ok())
        .map(|m| m.package.name)
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
            test: Some(root.join(TESTS_DIR)),
            decl: Some(resolve_path(root, &config.layout.types)),
        },
    }
}

/// One module per dependency, mounted at the dependency's name.
///
/// A dependency can be in the tree more than one way at once — installed, and also
/// copied to a `target_dir` or taken into a `patch_dir` — and it is one module
/// regardless: the name is the dependency's, and only one copy is the one its names mean.
/// A patch is what the project edits, so it wins; a vendored copy is what the manifest
/// put in the tree, so it comes next; an installed dependency is the rest.
fn dependency_modules(p: &pkg::Project) -> Vec<Module> {
    let mut out: Vec<Module> = Vec::new();
    let taken = |out: &[Module], name: &str| out.iter().any(|m| m.name == name);
    for patch in &p.patches {
        out.push(dependency(&patch.name, Owner::Patched, patch.entry.clone()));
    }
    for copy in &p.vendored_copies {
        let name = copy
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !taken(&out, &name) {
            out.push(dependency(&name, Owner::Vendored, copy.clone()));
        }
    }
    // The lockfile lists what an install placed, and `entries/<name>` is the link to each
    // one's entry directory. No lockfile, nothing installed.
    if let Ok(lock) = mlua_pkg::lockfile::Lockfile::read(&p.lockfile) {
        for locked in &lock.pkg {
            if !taken(&out, &locked.name) {
                out.push(dependency(
                    &locked.name,
                    Owner::Installed,
                    p.entries.join(&locked.name),
                ));
            }
        }
    }
    out
}

fn dependency(name: &str, owner: Owner, entry: PathBuf) -> Module {
    Module {
        name: name.to_string(),
        owner,
        mount: name.to_string(),
        roots: Roots {
            source: Some(entry),
            ..Roots::default()
        },
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
                    decl: Some(dir),
                    ..Roots::default()
                },
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
        };
        let p = Project {
            root: PathBuf::from("/p"),
            config: HtlConfig::default(),
            modules: vec![own],
            problems: Vec::new(),
        };
        let d = p.locate(Path::new("/p/scripts/host.d.tl")).unwrap();
        assert_eq!((d.role, d.name.as_str()), (Role::Decl, "host"));
        let s = p.locate(Path::new("/p/scripts/main.tl")).unwrap();
        assert_eq!((s.role, s.name.as_str()), (Role::Source, "main"));
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
