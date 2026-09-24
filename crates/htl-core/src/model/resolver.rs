//! The one answer to "which file is this module name, asked from this file".
//!
//! Lua answers it with `package.path`: a list of templates (`<dir>/?.lua`,
//! `<dir>/?/init.lua`, `<dir>/?/?.lua`) tried in order, first hit wins. A template does not
//! know whose file it finds, so it hands out names nobody gave: the `?/?` template makes a
//! project's `src/util/util.tl` answer to `util`, and a directory on the path inside
//! another one (`types/<crate>/` inside `types/`) makes one file answer to two names. And
//! every place that needed an answer — the checker, the run-time searcher, the linker,
//! `htl resolve`, a host's `TealResolver` — had its own copy of the templates, so their
//! answers could differ.
//!
//! [`Resolver`] answers from the project [model](super) instead. It is built once per
//! command from every file the model's modules hold, keyed by the name the model gives
//! each ([`Module::name_of`](super::Module::name_of)), and a name has the files the model
//! says it has and no others. The requiring file matters for three things: `[imports]`,
//! which is the project's say over its own `require`s; the test root, which only tests may
//! read; and `@<dependency>/<name>`, a dependency's module under a name no other module
//! can claim. A name no module has is [`Resolution::Outside`] the model, and only then is
//! the caller's own search — for libraries installed for the machine — the one that
//! answers.
//!
//! A dependency's file does not see the project that uses it: asked from one, a name only
//! the project's own module has is [`Resolution::NotVisible`], with the error to report.
//! Teal caches a module by its name and answers a second `require` of it without asking
//! again, so that answer is also asked for where every file passes once — the prelude's
//! `tl.parse` wrapper — and reported there, at the `require`.
//!
//! Names written into generated Lua are the ones [`rewrite`](Resolver::rewrite) gives:
//! `@<dependency>/<name>` for a dependency's module and `@/<name>` for the project's own,
//! wherever `[imports]` chose between the two. They mean the same thing whoever asks, so a
//! run-time `require`, which carries no requiring file, is answered the way the checker
//! was.

use super::{Owner, Project, Role};
use crate::config::ImportTarget;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A file of the model under one name, as the table holds it.
#[derive(Debug, Clone)]
struct Entry {
    /// Index into the project's modules.
    module: usize,
    /// The root it was found under plays this role.
    role: Role,
    file: PathBuf,
}

/// What one name resolves to, when the model has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The module that has the name.
    pub module: String,
    /// Its `.tl`, when it has one: what the checker checks and the run generates.
    pub implementation: Option<PathBuf>,
    /// Its `.d.tl`, when it has one: the types, when there is no `.tl`.
    pub declaration: Option<PathBuf>,
    /// Its `.lua`, when it has one: what runs behind a declaration.
    pub lua: Option<PathBuf>,
}

/// The answer to one `require`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// The model has the name, and one module implements it.
    Found(Found),
    /// More than one module implements the name, or one module has two implementations
    /// of it (`src/util.tl` beside `src/util/init.tl`). Each file, with its module.
    Ambiguous(Vec<(String, PathBuf)>),
    /// A name the model is the only authority for — `@<dependency>/…` — and it has no
    /// such module.
    Missing,
    /// A name no module of the model has. The caller may look for it elsewhere: a library
    /// installed for the machine is found by Lua's own search path.
    Outside,
    /// A dependency asked for a name only the project's own module has: the file it would
    /// have read, and the error that says so.
    NotVisible(PathBuf, String),
}

/// The model's name → file table, and how a requiring file changes a name.
/// See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Resolver {
    project: Project,
    table: BTreeMap<String, Vec<Entry>>,
    imports: Vec<(String, ImportTarget)>,
    /// Every file in the table, canonical: what [`owns`](Self::owns) is asked about.
    known: std::collections::HashSet<PathBuf>,
}

impl Resolver {
    /// The table for `project`: every `.tl`, `.d.tl` and `.lua` under the roots of its
    /// modules, each under the name the model gives it. Contract directories are left out
    /// — each holds the same names as the next, and is checked on its own.
    pub fn new(project: &Project) -> Self {
        let mut table: BTreeMap<String, Vec<Entry>> = BTreeMap::new();
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for m in project
            .modules
            .iter()
            .filter(|m| m.owner != Owner::Contract)
        {
            for (_, root) in m.roots.iter() {
                let top = root.to_path_buf();
                let walker = walkdir::WalkDir::new(root)
                    .sort_by_file_name()
                    .into_iter()
                    .filter_entry(move |e| {
                        e.path() == top || !crate::is_skipped_dir(e.path(), &[])
                    });
                for e in walker.flatten() {
                    let file = e.path();
                    let base = file.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if !e.file_type().is_file()
                        || !(base.ends_with(".tl") || base.ends_with(".lua"))
                    {
                        continue;
                    }
                    if !seen.insert(super::canon(file)) {
                        continue;
                    }
                    let Some(place) = project.locate(file) else {
                        continue;
                    };
                    let Some(module) = project
                        .modules
                        .iter()
                        .position(|x| std::ptr::eq(x, place.module))
                    else {
                        continue;
                    };
                    let role = if place.role == Role::Test {
                        Role::Test
                    } else {
                        Role::of(file)
                    };
                    table.entry(place.name).or_default().push(Entry {
                        module,
                        role,
                        file: file.to_path_buf(),
                    });
                }
            }
        }
        Self {
            project: project.clone(),
            table,
            imports: project.config.import_targets(),
            // Every file placed: what `owns` answers from.
            known: seen,
        }
    }

    /// Whether `file` is one of the model's: a search that found it under a name the
    /// model did not give it found the wrong thing (a `package.path` template's alias).
    pub fn owns(&self, file: &Path) -> bool {
        self.known.contains(&super::canon(file))
    }

    /// The name a `require` in `requirer` is to be written as, when it is not the name it
    /// was written as: what `[imports]` makes of it in the project's own files, and, in
    /// the files of a dependency an `[imports]` entry points at, `@<dependency>/<name>`
    /// for a name under the dependency's own name — so that the dependency keeps meaning
    /// its own module where the project's has the same name.
    ///
    /// What comes back is a name every asker resolves the same way, since it is what the
    /// generated Lua and a bundle carry, and a run-time `require` carries no requirer.
    pub fn rewrite(&self, requirer: &Path, name: &str) -> Option<String> {
        if name.starts_with('@') {
            return None;
        }
        let from = self.project.locate(requirer)?;
        match from.module.owner {
            Owner::Own => self
                .imported(name)
                .map(|(n, own)| if own { format!("@/{n}") } else { n }),
            Owner::Installed | Owner::Vendored | Owner::Patched => {
                let dep = &from.module.name;
                let named = self.imports.iter().any(|(_, t)| {
                    matches!(t, ImportTarget::Dep(n) if n.split('.').next() == Some(dep.as_str()))
                });
                let under = name == dep || name.starts_with(&format!("{dep}."));
                (named && under).then(|| format!("@{dep}/{name}"))
            }
            _ => None,
        }
    }

    /// Which file `name` is, asked from `requirer` — the file whose `require` it is, when
    /// the caller knows it.
    ///
    /// A requirer in the project's own module has `[imports]` applied first, and is the
    /// only one that may read the test root when it is itself under it. With no requirer
    /// — a run-time `require`, which carries only the name — every name the model has is
    /// answered.
    pub fn resolve(&self, requirer: Option<&Path>, name: &str) -> Resolution {
        let from = requirer.and_then(|r| self.project.locate(r));
        let own = from.as_ref().is_some_and(|p| p.module.owner == Owner::Own);
        let test = from.as_ref().is_some_and(|p| p.role == Role::Test);
        let dependency = from.as_ref().is_some_and(|p| {
            matches!(
                p.module.owner,
                Owner::Installed | Owner::Vendored | Owner::Patched
            )
        });
        let own_only = |e: &Entry| self.project.modules[e.module].owner == Owner::Own;
        let (name, only_own) = match own.then(|| self.imported(name)).flatten() {
            Some((rewritten, only_own)) => (rewritten, only_own),
            None => (name.to_string(), false),
        };
        if only_own {
            return self.pick(&name, own_only);
        }
        if let Some(own_name) = name.strip_prefix("@/") {
            return self.pick(own_name, own_only);
        }
        if let Some(rest) = name.strip_prefix('@') {
            let Some((dep, inner)) = rest.split_once('/') else {
                return Resolution::Missing;
            };
            return self.pick(inner, |e| {
                let m = &self.project.modules[e.module];
                m.name == dep
                    && matches!(m.owner, Owner::Installed | Owner::Vendored | Owner::Patched)
            });
        }
        // The test root is read by tests. A requirer the caller could not name is let
        // through: a run-time `require` inside a test carries no file.
        let sees = |e: &Entry| e.role != Role::Test || test || requirer.is_none() || from.is_none();
        // A dependency sees its own modules and what it depends on, not the project using
        // it.
        let others = |e: &Entry| !dependency || !own_only(e);
        match self.pick(&name, |e| sees(e) && others(e)) {
            Resolution::Missing => {}
            r => return r,
        }
        if dependency
            && let Some(hidden) = self
                .table
                .get(&name)
                .and_then(|v| v.iter().find(|e| own_only(e) && e.role != Role::Test))
        {
            let dep = from
                .as_ref()
                .map(|p| p.module.name.clone())
                .unwrap_or_default();
            let shown: PathBuf = hidden
                .file
                .strip_prefix(&self.project.root)
                .unwrap_or(&hidden.file)
                .components()
                .filter(|c| !matches!(c, std::path::Component::CurDir))
                .collect();
            return Resolution::NotVisible(
                hidden.file.clone(),
                format!(
                    "require(\"{name}\") in dependency {dep} reaches this project's own {}: a \
                     dependency sees its own modules and what it depends on, not the project \
                     using it",
                    shown.display()
                ),
            );
        }
        Resolution::Outside
    }

    /// Every file of the model that answers to `name`, whatever its role, in the order
    /// the model lists its modules — what a report of the name lists. `htl resolve` shows
    /// these and says which one is read ([`resolve`](Self::resolve)).
    pub fn claims(&self, name: &str) -> Vec<PathBuf> {
        let mut entries: Vec<&Entry> = self
            .table
            .get(name)
            .map(|v| v.iter().collect())
            .unwrap_or_default();
        entries.sort_by_key(|e| e.module);
        entries.into_iter().map(|e| e.file.clone()).collect()
    }

    /// The name `[imports]` makes of `name` in the project's own files, if it makes one: the
    /// entry for it or for a name above it, the rest carried across. A `dep:` target comes
    /// back as `@<dependency>/<name>`; an `own:` target as the project's name, with `true`
    /// to say that only the project's own module may answer it — the other owner of the
    /// name is what the entry was written to set aside.
    fn imported(&self, name: &str) -> Option<(String, bool)> {
        for (key, target) in &self.imports {
            let rest = if name == key {
                ""
            } else if let Some(r) = name.strip_prefix(&format!("{key}.")) {
                r
            } else {
                continue;
            };
            let (base, own_only) = match target {
                ImportTarget::Own(n) => (n.clone(), true),
                ImportTarget::Dep(n) => {
                    let dep = n.split('.').next().unwrap_or(n);
                    (format!("@{dep}/{n}"), false)
                }
            };
            let name = if rest.is_empty() {
                base
            } else {
                format!("{base}.{rest}")
            };
            return Some((name, own_only));
        }
        None
    }

    /// The entries of `name` that `keep` admits, as one answer.
    fn pick(&self, name: &str, keep: impl Fn(&Entry) -> bool) -> Resolution {
        // In the order the model lists its modules — the project's own first — whatever
        // order the walk met the files in: of two declarations of one name, the project's
        // is the one read, and `duplicate-declaration` says which was not.
        let mut entries: Vec<&Entry> = self
            .table
            .get(name)
            .map(|v| v.iter().filter(|e| keep(e)).collect())
            .unwrap_or_default();
        entries.sort_by_key(|e| e.module);
        if entries.is_empty() {
            return Resolution::Missing;
        }
        let is = |e: &Entry, ext: &str| e.file.to_string_lossy().ends_with(ext);
        let implementations: Vec<&&Entry> = entries
            .iter()
            .filter(|e| e.role != Role::Decl && (is(e, ".tl") || is(e, ".lua")) && !is(e, ".d.tl"))
            .collect();
        let tl: Vec<&&&Entry> = implementations.iter().filter(|e| is(e, ".tl")).collect();
        // Two `.tl` (a module and its directory's `init.tl`), or implementations in two
        // modules: no order decides between them.
        let modules: std::collections::BTreeSet<usize> =
            implementations.iter().map(|e| e.module).collect();
        if tl.len() > 1 || modules.len() > 1 {
            return Resolution::Ambiguous(
                implementations
                    .iter()
                    .map(|e| (self.project.modules[e.module].describe(), e.file.clone()))
                    .collect(),
            );
        }
        let module = modules.iter().next().copied().unwrap_or(entries[0].module);
        let first = |ext: &str, not: Option<&str>| {
            entries
                .iter()
                .find(|e| is(e, ext) && not.is_none_or(|n| !is(e, n)))
                .map(|e| e.file.clone())
        };
        Resolution::Found(Found {
            module: self.project.modules[module].name.clone(),
            implementation: first(".tl", Some(".d.tl")),
            declaration: first(".d.tl", None),
            lua: first(".lua", None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HtlConfig;
    use crate::pkg;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("htl-resolver-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn implementation(r: Resolution) -> Option<PathBuf> {
        match r {
            Resolution::Found(f) => f.implementation,
            other => panic!("not found: {other:?}"),
        }
    }

    #[test]
    fn a_file_answers_to_its_model_name_and_no_template_invents_another() {
        let root = scratch("names");
        write(&root.join("src/util/util.tl"), "");
        write(&root.join("src/game/init.tl"), "");
        write(
            &root.join("types/shipper").join(crate::DEP_TYPES_NOTE),
            "crate = \"shipper\"\nversion = \"0.1.0\"\nfiles = []\n",
        );
        write(&root.join("types/shipper/mq.d.tl"), "");
        let r = Resolver::new(&Project::load(&root, HtlConfig::default()).unwrap());
        let main = root.join("src/main.tl");

        assert_eq!(r.resolve(Some(&main), "util"), Resolution::Outside);
        assert_eq!(
            implementation(r.resolve(Some(&main), "util.util")),
            Some(root.join("src/util/util.tl"))
        );
        assert_eq!(
            implementation(r.resolve(Some(&main), "game")),
            Some(root.join("src/game/init.tl"))
        );
        match r.resolve(Some(&main), "mq") {
            Resolution::Found(f) => {
                assert_eq!(f.declaration, Some(root.join("types/shipper/mq.d.tl")));
                assert_eq!(f.module, "shipper");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(r.resolve(Some(&main), "shipper.mq"), Resolution::Outside);
    }

    #[test]
    fn the_test_root_is_read_by_tests() {
        let root = scratch("tests");
        write(&root.join("tests/helper.tl"), "");
        write(&root.join("tests/a_test.tl"), "");
        let r = Resolver::new(&Project::load(&root, HtlConfig::default()).unwrap());
        assert_eq!(
            r.resolve(Some(&root.join("src/main.tl")), "helper"),
            Resolution::Outside,
            "a source does not see the test root"
        );
        assert_eq!(
            implementation(r.resolve(Some(&root.join("tests/a_test.tl")), "helper")),
            Some(root.join("tests/helper.tl"))
        );
        assert!(
            matches!(r.resolve(None, "helper"), Resolution::Found(_)),
            "a run-time require carries no file"
        );
    }

    #[test]
    fn imports_and_dependency_names() {
        let root = scratch("imports");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
        );
        write(&root.join(".htl/modules/entries/mathx/init.tl"), "");
        write(&root.join(".htl/modules/entries/mathx/vec.tl"), "");
        write(&root.join(".htl/modules/entries/mathx/raw.d.tl"), "");
        write(&root.join(".htl/modules/entries/mathx/raw.lua"), "");
        write(&root.join("src/mathx.tl"), "");
        let main = root.join("src/main.tl");

        // No `[imports]`: two modules implement `mathx`.
        let r = Resolver::new(&Project::load(&root, HtlConfig::default()).unwrap());
        assert!(
            matches!(r.resolve(Some(&main), "mathx"), Resolution::Ambiguous(v) if v.len() == 2)
        );

        let cfg =
            HtlConfig::parse("[imports]\nmathx = \"dep:mathx\"\nmine = \"own:mathx\"\n").unwrap();
        let r = Resolver::new(&Project::load(&root, cfg).unwrap());
        assert_eq!(
            implementation(r.resolve(Some(&main), "mathx")),
            Some(root.join(".htl/modules/entries/mathx/init.tl"))
        );
        assert_eq!(
            implementation(r.resolve(Some(&main), "mathx.vec")),
            Some(root.join(".htl/modules/entries/mathx/vec.tl"))
        );
        assert_eq!(
            implementation(r.resolve(Some(&main), "mine")),
            Some(root.join("src/mathx.tl"))
        );
        match r.resolve(None, "@mathx/mathx.raw") {
            Resolution::Found(f) => {
                assert_eq!(f.lua, Some(root.join(".htl/modules/entries/mathx/raw.lua")));
                assert_eq!(
                    f.declaration,
                    Some(root.join(".htl/modules/entries/mathx/raw.d.tl"))
                );
                assert_eq!(f.implementation, None);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(r.resolve(None, "@mathx/mathx.nope"), Resolution::Missing);
        assert_eq!(r.resolve(None, "@nosuch/x"), Resolution::Missing);
    }

    #[test]
    fn a_dependency_does_not_see_the_projects_own_modules() {
        let root = scratch("dep-view");
        write(
            &root.join(pkg::MANIFEST_NAME),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\n",
        );
        write(&root.join(".htl/modules/entries/mathx/init.tl"), "");
        write(&root.join(".htl/modules/entries/other/init.tl"), "");
        write(&root.join("src/util.tl"), "");
        let r = Resolver::new(&Project::load(&root, HtlConfig::default()).unwrap());
        let dep = root.join(".htl/modules/entries/mathx/init.tl");
        match r.resolve(Some(&dep), "util") {
            Resolution::NotVisible(file, msg) => {
                assert_eq!(file, root.join("src/util.tl"));
                assert!(
                    msg.contains("in dependency mathx reaches this project's own src/util.tl"),
                    "{msg}"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            r.resolve(Some(&dep), "other"),
            Resolution::Found(_)
        ));
        assert!(matches!(
            r.resolve(Some(&root.join("src/main.tl")), "util"),
            Resolution::Found(_)
        ));
    }

    #[test]
    fn a_module_beside_its_directorys_init_is_ambiguous() {
        let root = scratch("init");
        write(&root.join("src/demo.tl"), "");
        write(&root.join("src/demo/init.tl"), "");
        let r = Resolver::new(&Project::load(&root, HtlConfig::default()).unwrap());
        assert!(matches!(
            r.resolve(Some(&root.join("src/main.tl")), "demo"),
            Resolution::Ambiguous(v) if v.len() == 2
        ));
    }
}
