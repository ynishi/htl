//! The naming rule: which module name a file answers to, and — the same rule read the
//! other way — which files may answer to a name.
//!
//! # One rule, two directions
//!
//! A file's name is its path relative to the root that holds it, extension dropped,
//! separators turned into dots, under the mount of the module the root belongs to
//! ([`name_of`]). The project model names every file it holds this way
//! (`model::Module::name_of`), and builds its name → file table from the result. A host's
//! `TealResolver` cannot build a table: its directory is the host's, and a file dropped
//! into it after the host started is still one a `require` should find. So it goes the
//! other way ([`candidates`]) — from a name to the files that could answer it — and keeps
//! only those that [`name_of`] names back to the name asked for. The inverse is defined by
//! the rule rather than written beside it, which is what keeps the two directions from
//! drifting apart the way the copies of Lua's `package.path` templates did.
//!
//! # The spellings of a directory
//!
//! Two files name the directory they are in rather than themselves:
//!
//! - `<dir>/init.tl` is `<dir>`, everywhere — Lua's `?/init.lua`, which every Teal and
//!   Lua project uses for a module that has submodules.
//! - `<mount>.tl` at the top of a mounted module is the mount — Lua's `?/?.lua`, the entry
//!   of a flat package (`entry = "src"` holding `<name>.tl` beside its other files). Only
//!   there: anywhere else a file named after its directory is an ordinary submodule, and a
//!   template that gave it the directory's name as well is how one file came to answer to
//!   two names.
//!
//! A module mounted at the top (the project's own, a host's script directory) has no mount
//! to spell, so `src/util/util.tl` is `util.util` and never `util`.

use std::path::{Component, Path, PathBuf};

/// The extensions a module file may have, in the order a name's files are listed: the
/// implementation, its declaration, plain Lua. `.d.tl` before `.tl` would matter to a
/// suffix test; here each is joined to a stem, so the order is only the listing's.
pub const EXTENSIONS: [&str; 3] = [".tl", ".d.tl", ".lua"];

/// The module name the file at `rel` (relative to a root of a module mounted at `mount`,
/// `""` for the top) answers to. `None` when the path names nothing — the top's own
/// `init.tl`, which would be named by the empty string.
///
/// For `<a>/<b>.tl` under mount `m`: `m.a.b`, with `.tl`, `.d.tl` or `.lua` dropped and an
/// empty mount contributing nothing; `<a>/init.tl` is `m.a`; and at the top of a mounted
/// module, `<last>.tl` — `<last>` the final segment of the mount — is `m` itself.
pub fn name_of(mount: &str, rel: &Path) -> Option<String> {
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
        || (parts.len() == 1 && !mount.is_empty() && mount_last(mount) == parts[0]);
    if names_its_directory {
        parts.pop();
    }
    let mut name: Vec<&str> = Vec::new();
    if !mount.is_empty() {
        name.push(mount);
    }
    name.extend(parts.iter().map(String::as_str));
    (!name.is_empty()).then(|| name.join("."))
}

/// Every path, relative to a root of a module mounted at `mount`, whose file would answer
/// to `name` — [`name_of`] read backwards. Implementations first, then declarations, then
/// plain Lua (the order of [`EXTENSIONS`]); within each, the file before the directory's
/// `init`. Empty when `name` is not under `mount` at all.
///
/// Two paths of one extension in the answer are two spellings of one name — `util.tl` and
/// `util/init.tl` — and a caller that finds both has found an ambiguity, not a choice.
pub fn candidates(mount: &str, name: &str) -> Vec<PathBuf> {
    let rest = if mount.is_empty() {
        Some(name)
    } else if name == mount {
        Some("")
    } else {
        name.strip_prefix(mount).and_then(|r| r.strip_prefix('.'))
    };
    let Some(rest) = rest else {
        return Vec::new();
    };
    // The stems a file of this name could have; `name_of` decides which of them really
    // are this name (the mount's own `<last>` spelling only names the mount).
    let stems: Vec<PathBuf> = if rest.is_empty() {
        vec![PathBuf::from(mount_last(mount)), PathBuf::from("init")]
    } else {
        let dir: PathBuf = rest.split('.').collect();
        vec![dir.clone(), dir.join("init")]
    };
    let mut out = Vec::new();
    for ext in EXTENSIONS {
        for stem in &stems {
            let mut file = stem.clone().into_os_string();
            file.push(ext);
            let file = PathBuf::from(file);
            if name_of(mount, &file).as_deref() == Some(name) {
                out.push(file);
            }
        }
    }
    out
}

/// The final segment of a dotted mount: `a.b` → `b`.
pub fn mount_last(mount: &str) -> &str {
    mount.rsplit('.').next().unwrap_or(mount)
}

/// `util.d.tl` -> `util`, `util.tl` -> `util`, `util.lua` -> `util`; anything else as is.
fn module_stem(file_name: &str) -> &str {
    [".d.tl", ".tl", ".lua"]
        .iter()
        .find_map(|ext| file_name.strip_suffix(ext))
        .unwrap_or(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: Vec<PathBuf>) -> Vec<String> {
        v.into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn at_the_top_a_file_named_after_its_directory_is_a_submodule() {
        assert_eq!(
            name_of("", Path::new("util/util.tl")).as_deref(),
            Some("util.util")
        );
        assert_eq!(
            names(candidates("", "util")),
            [
                "util.tl",
                "util/init.tl",
                "util.d.tl",
                "util/init.d.tl",
                "util.lua",
                "util/init.lua"
            ]
        );
        assert_eq!(name_of("", Path::new("init.tl")), None);
        assert_eq!(
            names(candidates("", "init")),
            ["init/init.tl", "init/init.d.tl", "init/init.lua"]
        );
    }

    #[test]
    fn a_mounted_module_is_spelled_by_its_entry_only_at_its_top() {
        assert_eq!(
            name_of("mathx", Path::new("mathx.tl")).as_deref(),
            Some("mathx")
        );
        assert_eq!(
            names(candidates("mathx", "mathx")),
            [
                "mathx.tl",
                "init.tl",
                "mathx.d.tl",
                "init.d.tl",
                "mathx.lua",
                "init.lua"
            ]
        );
        // `mathx.mathx` is the submodule under the entry directory, never the entry file.
        assert_eq!(
            names(candidates("mathx", "mathx.mathx")),
            ["mathx/init.tl", "mathx/init.d.tl", "mathx/init.lua"]
        );
        assert_eq!(names(candidates("mathx", "other")), Vec::<String>::new());
    }

    #[test]
    fn every_candidate_names_back_to_the_name() {
        for (mount, name) in [
            ("", "a.b.c"),
            ("x", "x"),
            ("x", "x.y.z"),
            ("a.b", "a.b"),
            ("", "init.x"),
        ] {
            for c in candidates(mount, name) {
                assert_eq!(
                    name_of(mount, &c).as_deref(),
                    Some(name),
                    "{mount} {name} {}",
                    c.display()
                );
            }
        }
    }
}
