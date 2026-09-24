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
//! A name the host provides ([`Project::provides`]) is answered by no file: the host puts
//! it in `package.preload`, which Lua consults before any searcher. The model may still
//! hold a declaration of it — that is how a host module is typed — and the answer is then
//! [`Resolution::Found`] with only the declaration, as for any declared name. What it may
//! not hold is an implementation: a `.tl` or a `.lua` under the name would be what the
//! checker reads and what a run without the host loads, while every run with the host
//! loads the host's module instead. No order between the two is right in every phase, so
//! the answer is [`Resolution::HostShadowed`], an error wherever the name is asked for —
//! the way two files implementing one name are [`Resolution::Ambiguous`].
//!
//! A name the model has only a declaration for, and that none of those sources names, is
//! provided by the environment ([`Provider::Declared`]): its answer is still
//! [`Resolution::Found`] with only the declaration, and [`provides`](Resolver::provides)
//! is what says the environment provides it. That is a fact about the files, which this
//! table has and the [`Project`] does not, so it is answered here: `provides` and
//! [`provided`](Resolver::provided) are the project's configured names plus these. The
//! linker leaves such a name out of a bundle because this says so, and `htl resolve`
//! names the declaration.
//!
//! Names written into generated Lua are the ones [`rewrite`](Resolver::rewrite) gives:
//! `@<dependency>/<name>` for a dependency's module and `@/<name>` for the project's own,
//! wherever `[imports]` chose between the two. They mean the same thing whoever asks, so a
//! run-time `require`, which carries no requiring file, is answered the way the checker
//! was.

use super::{Owner, Project, Provider, Role};
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

/// A name the host provides that a file of the model implements as well: what
/// [`Resolution::HostShadowed`] carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShadowed {
    /// Where the model learned that the host provides the name.
    pub provider: Provider,
    /// The name's `.d.tl`, when the model has one: the host module's types, which the
    /// checker still reads for it.
    pub declaration: Option<PathBuf>,
    /// The implementations — a `.tl`, a `.lua`, or both — that would be checked, or run
    /// by a run without the host, in place of the host's module.
    pub files: Vec<PathBuf>,
    /// The error that says so, with the files as a message names them
    /// ([`Resolver::show`]): what `htl check` reports at the `require` and a run raises.
    pub message: String,
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
    /// The host provides the name ([`Project::provides`]) and a file of the model
    /// implements it too. An error in every phase: the check would read the file, a run
    /// with the host would load the host's module, and neither is the other. A
    /// declaration alone is not this — it is [`Found`](Resolution::Found), and how a
    /// host module gets its types.
    HostShadowed(HostShadowed),
}

/// The model's name → file table, and how a requiring file changes a name.
/// See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Resolver {
    project: Project,
    /// Every root of every module, canonical, once: `(module, role, root)`. Placing a file
    /// is then one canonicalisation of the file and a comparison with each of these,
    /// rather than a canonicalisation of every root for every file.
    roots: Vec<(usize, Role, PathBuf)>,
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
        let roots: Vec<(usize, Role, PathBuf)> = project
            .modules
            .iter()
            .enumerate()
            .flat_map(|(i, m)| {
                m.roots
                    .iter()
                    .map(move |(role, r)| (i, role, super::canon(r)))
            })
            .collect();
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
                    let canonical = super::canon(file);
                    if !seen.insert(canonical.clone()) {
                        continue;
                    }
                    let Some((module, root_role, name)) = place(project, &roots, &canonical) else {
                        continue;
                    };
                    let role = if root_role == Role::Test {
                        Role::Test
                    } else {
                        Role::of(file)
                    };
                    table.entry(name).or_default().push(Entry {
                        module,
                        role,
                        file: file.to_path_buf(),
                    });
                }
            }
        }
        Self {
            project: project.clone(),
            roots,
            table,
            imports: project.config.import_targets(),
            // Every file placed: what `owns` answers from.
            known: seen,
        }
    }

    /// The table for the same project, read from its directories again: what a file added
    /// since [`new`](Self::new) walked them needs to have a name
    /// ([`Htl::install_resolver`](crate::Htl::install_resolver)'s `refresh_model`).
    pub(crate) fn rebuilt(&self) -> Self {
        Self::new(&self.project)
    }

    /// [`Project::locate`] over the canonical roots taken once in [`new`](Self::new).
    fn placed(&self, file: &Path) -> Option<super::Place<'_>> {
        let (module, role, name) = place(&self.project, &self.roots, &super::canon(file))?;
        let module = &self.project.modules[module];
        let role = if role == Role::Test {
            Role::Test
        } else {
            Role::of(file)
        };
        Some(super::Place { module, role, name })
    }

    /// `file` as a message names it: relative to the project's root when it is under it,
    /// as it is otherwise.
    pub fn show(&self, file: &Path) -> String {
        let shown: PathBuf = file
            .strip_prefix(&self.project.root)
            .unwrap_or(file)
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .collect();
        shown.display().to_string()
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
        let from = self.placed(requirer)?;
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
        let from = requirer.and_then(|r| self.placed(r));
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
            Resolution::Found(f) => return self.unless_provided(&name, f),
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
            return Resolution::NotVisible(
                hidden.file.clone(),
                format!(
                    "require(\"{name}\") in dependency {dep} reaches this project's own {}: a \
                     dependency sees its own modules and what it depends on, not the project \
                     using it",
                    self.show(&hidden.file)
                ),
            );
        }
        Resolution::Outside
    }

    /// `found` as the answer for `name`, unless the host provides the name and `found`
    /// has an implementation of it — then [`Resolution::HostShadowed`].
    ///
    /// Asked only for a plain name. `@/<name>` and `@<dependency>/<name>` are what
    /// `[imports]` and a dependency's own names are written as, and no host registers a
    /// name under either spelling, so a file answering one of them is what runs.
    fn unless_provided(&self, name: &str, found: Found) -> Resolution {
        let Some(provider) = self.project.provides(name) else {
            return Resolution::Found(found);
        };
        let files: Vec<PathBuf> = found
            .implementation
            .iter()
            .chain(found.lua.iter())
            .cloned()
            .collect();
        if files.is_empty() {
            return Resolution::Found(found);
        }
        let from = self.provided_by(name, provider);
        let shown: Vec<String> = files.iter().map(|f| self.show(f)).collect();
        let (these, them) = if files.len() == 1 {
            ("this file", "it")
        } else {
            ("these files", "them")
        };
        let message = format!(
            "'{name}' is provided by the host ({from}) and also implemented by {}: the \
             host's module is what runs, so {these} would be checked and never run — rename \
             {them}, or stop providing the name",
            shown.join(" and ")
        );
        Resolution::HostShadowed(HostShadowed {
            provider,
            declaration: found.declaration,
            files,
            message,
        })
    }

    /// Whether `name` runs from no file of the project, and who provides it: the host, by
    /// one of the three sources [`Project::provides`] answers, or else the environment,
    /// when the model has a declaration of the name and nothing else
    /// ([`Provider::Declared`]). `None` for a name a file of the model implements, and for
    /// a name the model has nothing under.
    ///
    /// Asked as a run-time `require` asks, with no requiring file — the question is what
    /// runs, and a run carries only the name. So `@<dependency>/<name>` and `@/<name>`,
    /// which is how generated Lua spells `[imports]` and a dependency's own names, are
    /// answered as well: `@mathx/mathx.raw` with only `raw.d.tl` behind it is declared.
    /// The configured sources name no `@`-spelled name, so for those only the files speak.
    pub fn provides(&self, name: &str) -> Option<Provider> {
        self.project
            .provides(name)
            .or_else(|| self.declaration_only(name).map(|_| Provider::Declared))
    }

    /// Every name that runs from no file of the project, with who provides it, in name
    /// order: [`Project::provided`] and every name of the table [`provides`](Self::provides)
    /// answers [`Provider::Declared`] for. One entry per name, the configured source first
    /// where a name has both (a `#[host_module]` with its generated `.d.tl` is the host's).
    pub fn provided(&self) -> Vec<(String, Provider)> {
        let mut out: BTreeMap<String, Provider> = self
            .project
            .provided()
            .map(|(n, p)| (n.to_string(), p))
            .collect();
        for name in self.table.keys() {
            if !out.contains_key(name) && self.declaration_only(name).is_some() {
                out.insert(name.clone(), Provider::Declared);
            }
        }
        out.into_iter().collect()
    }

    /// The declaration `name` is answered by when the model has a declaration of it and no
    /// implementation — no `.tl`, and no `.lua` in any role: what makes a name
    /// [`Provider::Declared`] when no configured source names it. Asked with no requiring
    /// file, as [`provides`](Self::provides) says.
    fn declaration_only(&self, name: &str) -> Option<PathBuf> {
        match self.resolve(None, name) {
            Resolution::Found(Found {
                implementation: None,
                lua: None,
                declaration,
                ..
            }) => declaration,
            _ => None,
        }
    }

    /// Who provides a name, as a message says it: `#[host_module] in Cargo.toml's crate`,
    /// `[build] host in htl.toml`, `htl's std`, and for a name the environment provides,
    /// the declaration that says so — `declared by types/socket/http.d.tl`, shown as
    /// [`show`](Self::show) shows a file. `name` is only read for that last one.
    ///
    /// The one wording for it, so the error [`Resolution::HostShadowed`] carries and
    /// `htl resolve`'s report of a provided name name the source the same way.
    pub fn provided_by(&self, name: &str, provider: Provider) -> String {
        match provider {
            Provider::HostModule => {
                let cargo = self
                    .project
                    .host_crate
                    .as_deref()
                    .map(|c| self.show(&c.join("Cargo.toml")))
                    .unwrap_or_else(|| "Cargo.toml".into());
                format!("#[host_module] in {cargo}'s crate")
            }
            Provider::Build => format!("[build] host in {}", crate::config::CONFIG_NAME),
            Provider::Std => "htl's std".into(),
            Provider::Declared => match self.declaration_only(name) {
                Some(d) => format!("declared by {}", self.show(&d)),
                // Asked of a name that has more than its declaration: say what the
                // variant means rather than name a file that is not the reason.
                None => "a declaration".into(),
            },
        }
    }

    /// [`resolve`](Self::resolve)'s answer as a string that changes whenever the answer
    /// does: the kind, and every file in it. What the run cache probes with
    /// ([`crate::cache::Cache::with_answers`]).
    pub fn fingerprint(&self, requirer: Option<&Path>, name: &str) -> String {
        let s = |p: &Option<PathBuf>| {
            p.as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        match self.resolve(requirer, name) {
            Resolution::Found(f) => format!(
                "found\0{}\0{}\0{}\0{}",
                f.module,
                s(&f.implementation),
                s(&f.declaration),
                s(&f.lua)
            ),
            Resolution::Ambiguous(v) => {
                let files: Vec<String> = v
                    .iter()
                    .map(|(_, f)| f.to_string_lossy().into_owned())
                    .collect();
                format!("ambiguous\0{}", files.join("\0"))
            }
            Resolution::Missing => "missing".into(),
            Resolution::Outside => "outside".into(),
            Resolution::NotVisible(f, _) => format!("hidden\0{}", f.display()),
            Resolution::HostShadowed(h) => {
                let files: Vec<String> = h
                    .files
                    .iter()
                    .map(|f| f.to_string_lossy().into_owned())
                    .collect();
                format!(
                    "shadowed\0{:?}\0{}\0{}",
                    h.provider,
                    s(&h.declaration),
                    files.join("\0")
                )
            }
        }
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

/// The module whose root holds `canonical` most specifically, that root's role, and the
/// name the file answers to — [`Project::locate`]'s rule, over roots made canonical once.
fn place(
    project: &Project,
    roots: &[(usize, Role, PathBuf)],
    canonical: &Path,
) -> Option<(usize, Role, String)> {
    let (module, role, root) = roots
        .iter()
        .filter(|(_, _, r)| canonical.starts_with(r))
        .max_by_key(|(_, _, r)| r.components().count())?;
    let rel = canonical.strip_prefix(root).ok()?;
    let name = project.modules[*module].name_of_relative(rel)?;
    Some((*module, *role, name))
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
    fn a_file_under_a_name_the_host_provides_is_shadowed_and_a_declaration_is_not() {
        let root = scratch("host");
        write(&root.join("src/game.d.tl"), "");
        let cfg = || HtlConfig::parse("[build]\nhost = [\"game\"]\n").unwrap();
        let main = root.join("src/main.tl");

        // The declaration alone is how the host's module is typed.
        let r = Resolver::new(&Project::load(&root, cfg()).unwrap());
        match r.resolve(Some(&main), "game") {
            Resolution::Found(f) => {
                assert_eq!(f.declaration, Some(root.join("src/game.d.tl")));
                assert_eq!((f.implementation, f.lua), (None, None));
            }
            other => panic!("{other:?}"),
        }
        assert!(r.fingerprint(None, "game").starts_with("found\0"));

        // An implementation beside it — `.tl` and `.lua` both — is the error.
        write(&root.join("src/game.tl"), "");
        write(&root.join("src/game.lua"), "");
        let r = Resolver::new(&Project::load(&root, cfg()).unwrap());
        let answer = r.resolve(None, "game");
        assert_eq!(
            answer,
            r.resolve(Some(&main), "game"),
            "asked from anywhere"
        );
        match answer {
            Resolution::HostShadowed(h) => {
                assert_eq!(h.provider, Provider::Build);
                assert_eq!(h.declaration, Some(root.join("src/game.d.tl")));
                assert_eq!(
                    h.files,
                    vec![root.join("src/game.tl"), root.join("src/game.lua")]
                );
                assert_eq!(
                    h.message,
                    "'game' is provided by the host ([build] host in htl.toml) and also \
                     implemented by src/game.tl and src/game.lua: the host's module is what \
                     runs, so these files would be checked and never run — rename them, or \
                     stop providing the name"
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(
            r.fingerprint(None, "game").starts_with("shadowed\0"),
            "the run cache keys on its own answer"
        );

        // The same file under a name the host does not provide is only a module.
        let r = Resolver::new(&Project::load(&root, HtlConfig::default()).unwrap());
        assert!(matches!(r.resolve(None, "game"), Resolution::Found(_)));
    }

    #[test]
    fn a_name_with_only_a_declaration_is_provided_by_the_environment() {
        let root = scratch("declared");
        // Declared: a hand-written declaration in the declaration root, and one beside the
        // sources, neither with anything behind it.
        write(&root.join("types/socket/http.d.tl"), "");
        write(&root.join("src/native.d.tl"), "");
        // Not declared: a `.lua` behind the declaration is what runs, and is bundled.
        write(&root.join("types/mq.d.tl"), "");
        write(&root.join("src/mq.lua"), "");
        // Not declared: an implementation.
        write(&root.join("src/util.tl"), "");
        // A name the host provides keeps its source, declaration or not: a
        // `#[host_module]` with the `.d.tl` it generates beside the sources, and `[build]
        // host`.
        write(
            &root.join("Cargo.toml"),
            "[package]\nname = \"game\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        );
        write(
            &root.join("src/lib.rs"),
            "use htl::host_module;\n\npub struct Host;\n\n\
             #[host_module(name = \"host\", dts = \"src/host.d.tl\")]\n\
             impl Host {\n    pub fn greet(&self) {}\n}\n",
        );
        write(&root.join("src/host.d.tl"), "");
        write(&root.join("src/game.d.tl"), "");
        let cfg = HtlConfig::parse("[build]\nhost = [\"game\", \"remote\"]\n").unwrap();
        let r = Resolver::new(&Project::load(&root, cfg).unwrap());

        assert_eq!(r.provides("socket.http"), Some(Provider::Declared));
        assert_eq!(
            r.provided_by("socket.http", Provider::Declared),
            "declared by types/socket/http.d.tl"
        );
        assert_eq!(r.provides("native"), Some(Provider::Declared));
        assert_eq!(
            r.provided_by("native", Provider::Declared),
            "declared by src/native.d.tl"
        );
        assert_eq!(r.provides("mq"), None, "the .lua is what runs");
        assert_eq!(r.provides("util"), None);
        assert_eq!(
            r.provides("host"),
            Some(Provider::HostModule),
            "HostModule wins"
        );
        assert_eq!(
            r.provides("game"),
            Some(Provider::Build),
            "the host's, not declared"
        );
        assert_eq!(
            r.provides("remote"),
            Some(Provider::Build),
            "with no file at all"
        );
        assert_eq!(r.provides("nothing"), None);
        // The project's own table is the three configured sources, and knows no file.
        let p = Project::load(&root, HtlConfig::default()).unwrap();
        assert_eq!(p.provides("socket.http"), None);

        let listed: Vec<(String, Provider)> = r
            .provided()
            .into_iter()
            .filter(|(n, _)| !n.starts_with("std") && !n.starts_with("htl"))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("game".to_string(), Provider::Build),
                ("host".to_string(), Provider::HostModule),
                ("native".to_string(), Provider::Declared),
                ("remote".to_string(), Provider::Build),
                ("socket.http".to_string(), Provider::Declared),
            ]
        );
        // A declared name is found as any declaration is, never shadowed.
        assert!(matches!(
            r.resolve(None, "socket.http"),
            Resolution::Found(Found {
                implementation: None,
                lua: None,
                ..
            })
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
