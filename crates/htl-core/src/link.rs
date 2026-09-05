//! Linking: the `require` closure of one entry file, as a [`Bundle`].
//!
//! Starting at the entry, every `require("<literal>")` is followed (only string
//! literals: a `require(expr)` cannot be resolved statically, list its targets under
//! `extra`). `.tl` modules are type-checked and generated; plain `.lua` modules (a
//! vendored dependency, say) are taken as they are. A name that resolves only to a
//! `.d.tl` declaration is recorded as host-provided, as is anything listed in `host`.
//! Any other unresolved `require` is an error: the point of a bundle is that "module
//! not found" happens here, not on the first `require` at the customer's machine.

use crate::bundle::{Bundle, Kind, Module};
use crate::{CheckInfo, Htl, RequireSite};
use anyhow::{Context, Result};
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct LinkOptions {
    /// Keep debug info (line numbers, local names) in bytecode. Off = stripped.
    pub debug: bool,
    /// Store generated Lua source instead of bytecode (portable across Lua builds).
    pub source: bool,
    /// Modules to include even if no literal `require` reaches them.
    pub extra: Vec<String>,
    /// Modules the host provides at run time (besides those declared only by a `.d.tl`).
    pub host: Vec<String>,
}

/// One linked module: where it came from and how it was stored.
#[derive(Debug, Clone)]
pub struct LinkedModule {
    pub name: String,
    pub path: PathBuf,
    pub typed: bool,
}

#[derive(Debug, Default)]
pub struct Linked {
    pub bundle: Bundle,
    pub modules: Vec<LinkedModule>,
    pub host_modules: Vec<String>,
    /// Type errors, lints and unresolved requires; the bundle is only meaningful when empty.
    pub errors: Vec<String>,
    pub lints: Vec<String>,
    pub checks: Vec<(PathBuf, CheckInfo)>,
}

/// Link `entry` (a `.tl` file) and everything it requires. The checker's search path
/// must already cover the project (`add_path` / `apply_project` / `apply_config`).
pub fn link(h: &Htl, entry: &Path, opts: &LinkOptions) -> Result<Linked> {
    let mut out = Linked::default();
    let entry_name = entry
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "main".into());
    let host_declared: HashSet<String> = opts.host.iter().cloned().collect();
    let mut host: BTreeSet<String> = BTreeSet::new();
    let mut queued: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, PathBuf)> = VecDeque::new();
    queue.push_back((entry_name.clone(), entry.to_path_buf()));
    queued.insert(entry_name.clone());
    for name in &opts.extra {
        match classify(h, name, None)? {
            Target::File(p) => {
                if queued.insert(name.clone()) {
                    queue.push_back((name.clone(), p));
                }
            }
            Target::Host => {
                host.insert(name.clone());
            }
            Target::Missing => out.errors.push(format!("extra module '{name}' not found on the search path")),
        }
    }

    while let Some((name, path)) = queue.pop_front() {
        let typed = path.extension().is_none_or(|e| e != "lua");
        let (code, requires) = if typed {
            let (code, ci) = h.gen_lua(&path)?;
            out.errors.extend(ci.errors.iter().cloned());
            out.lints.extend(ci.lints.iter().cloned());
            let reqs = ci.requires.clone();
            out.checks.push((path.clone(), ci));
            (code, reqs)
        } else {
            let src = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let reqs = h.lua_requires(&src, &path)?;
            (Some(src), reqs)
        };
        for r in &requires {
            if queued.contains(&r.module) || host.contains(&r.module) {
                continue;
            }
            match classify(h, &r.module, r.path.as_deref())? {
                Target::File(p) => {
                    queued.insert(r.module.clone());
                    queue.push_back((r.module.clone(), p));
                }
                Target::Host => {
                    host.insert(r.module.clone());
                }
                Target::Missing if host_declared.contains(&r.module) => {
                    host.insert(r.module.clone());
                }
                Target::Missing => out.errors.push(unresolved(&path, r)),
            }
        }
        let Some(code) = code else { continue };
        let payload = if opts.source {
            Module { name: name.clone(), kind: Kind::Source, payload: code.into_bytes() }
        } else {
            let bc = h.compile_with(&name, &code, !opts.debug)?;
            Module { name: name.clone(), kind: Kind::Bytecode, payload: bc }
        };
        out.bundle.modules.push(payload);
        out.modules.push(LinkedModule { name, path, typed });
    }

    out.host_modules = host.iter().cloned().collect();
    out.bundle.entry = entry_name;
    out.bundle.htl_version = env!("CARGO_PKG_VERSION").to_string();
    out.bundle.host_modules = out.host_modules.clone();
    if !opts.source {
        out.bundle.fingerprint = h.fingerprint()?;
    }
    Ok(out)
}

fn is_decl(p: &Path) -> bool {
    p.to_string_lossy().ends_with(".d.tl")
}

fn unresolved(from: &Path, r: &RequireSite) -> String {
    format!(
        "{}:{}:{}: require(\"{}\") is not on the search path: nothing to bundle. If the host \
         provides it, declare it in a `{}.d.tl` or list it under `[build] host` in htl.toml; \
         if it is reached only through a dynamic require, list it under `[build] extra`",
        from.display(),
        r.line,
        r.col,
        r.module,
        r.module.replace('.', "/")
    )
}

enum Target {
    /// A file to bundle (`.tl` typed, or a plain `.lua`).
    File(PathBuf),
    /// Declared only (`.d.tl` with no `.lua` behind it): the host provides it.
    Host,
    Missing,
}

/// What a `require(name)` points at for the linker. `found` is the checker's own
/// resolution when already known (a require site); otherwise it is looked up.
fn classify(h: &Htl, name: &str, found: Option<&Path>) -> Result<Target> {
    let (found, lua) = match found {
        Some(p) => (Some(p.to_path_buf()), None),
        None => h.resolve_module(name)?,
    };
    let Some(p) = found else { return Ok(Target::Missing) };
    if !is_decl(&p) {
        return Ok(Target::File(p));
    }
    // A declaration: is there a `.lua` implementation on the path behind it (a vendored
    // dependency typed by a `.d.tl`)? Then that is what gets bundled.
    let lua = match lua {
        Some(l) => Some(l),
        None => h.resolve_module(name)?.1,
    };
    Ok(match lua {
        Some(l) => Target::File(l),
        None => Target::Host,
    })
}
