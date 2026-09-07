//! `---@contract` / `---@required`: the contract a directory of modules must satisfy,
//! declared where the type is declared.
//!
//! ```tl
//! local record defs
//!    record Mod              ---@contract("mods")
//!       name: string         ---@required
//!       monsters: {Monster}  ---@required
//!       factions: {Faction}
//!    end
//! end
//! return defs
//! ```
//!
//! Every module directly under `mods/` must return a value assignable to `defs.Mod` and
//! set `name` and `monsters`; `factions` is there for the mods that want it. That last
//! part is why the fields are marked rather than counted: a record cannot say which of
//! its own fields are mandatory (every Teal record field is nilable and there is no `?`
//! for them), and taking all of them would break every module written before the field
//! was added.
//!
//! `htl.toml` holds the directory and nothing else:
//!
//! ```toml
//! [[contract]]
//! dir = "mods"
//! ```
//!
//! A bare `---@contract` inherits it — one line in the file a reader opens first, saying
//! where this project accepts modules — and `---@contract("<dir>")` overrides it, which
//! is what a project with more than one contract writes.
//!
//! The markers are comments, so the file stays valid Teal and other tooling ignores them.
//! Reading them is a scan of the search paths, not a type-check: a record is found by the
//! line it is declared on, the way `---@struct` is (see `prelude.lua`).

use crate::config::{Contract, HtlConfig, RequireFields};
use anyhow::Result;
use std::path::{Path, PathBuf};

/// A contract as it applies: which directory, which type, which fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// Directory relative to the project root, `*` in one segment expanding to every
    /// subdirectory at that level (`"sites/*"`).
    pub dir: String,
    /// `"<module>.<Type>"`, e.g. `"defs.Mod"` — the module that declares the record and
    /// the path to it inside that module.
    pub type_path: String,
    /// The fields a module's returned table must set, from `---@required`.
    pub require_fields: RequireFields,
    /// Only this module name in each matched dir is held to the contract.
    pub module: Option<String>,
    /// Module names in the dir that are not held to the contract.
    pub exclude: Vec<String>,
    /// Where to publish the declaration, from `---@contract(dts = "…")`.
    pub dts: Option<String>,
    /// The file the marker is in, and the line it is on: where to point when something
    /// about this contract is wrong.
    pub declared_in: PathBuf,
    pub declared_at: usize,
}

impl Resolved {
    /// Concrete contract directories under `root` (expands one `*` segment). Missing
    /// directories are dropped; a literal `dir` that does not exist yields nothing.
    pub fn dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut acc = vec![root.to_path_buf()];
        for seg in self.dir.split('/').filter(|s| !s.is_empty() && *s != ".") {
            let mut next = Vec::new();
            for base in &acc {
                if seg == "*" {
                    if let Ok(rd) = std::fs::read_dir(base) {
                        let mut subs: Vec<PathBuf> = rd
                            .flatten()
                            .map(|e| e.path())
                            .filter(|p| p.is_dir() && !crate::is_skipped_dir(p, &[]))
                            .collect();
                        subs.sort();
                        next.extend(subs);
                    }
                } else {
                    let p = base.join(seg);
                    if p.is_dir() {
                        next.push(p);
                    }
                }
            }
            acc = next;
        }
        acc
    }

    /// Is `module` (a file stem under a contract dir) held to this contract? The module
    /// that declares the type is not held to it.
    pub fn applies_to(&self, module: &str) -> bool {
        if self
            .type_path
            .split_once('.')
            .is_some_and(|(m, _)| m == module)
        {
            return false;
        }
        if self.exclude.iter().any(|e| e == module) {
            return false;
        }
        match &self.module {
            Some(only) => only == module,
            None => true,
        }
    }
}

/// What the `---@contract` on one record says, before the directory is settled.
struct Marker {
    dir: Option<String>,
    module: Option<String>,
    exclude: Option<Vec<String>>,
    dts: Option<String>,
}

/// Contracts this project declares, and what is wrong with the ones it does not.
///
/// The scan covers [`HtlConfig::search_paths`] — where the checker resolves modules
/// from — one level deep, plus `<sub>/init.tl`, which is the shape Teal's own path
/// templates resolve. A marker anywhere else is not found, and the module name of a file
/// nested deeper cannot be written as `<module>.<Type>` anyway.
///
/// The second half of the pair is diagnostics: a bare marker with no `[[contract]]` to
/// inherit from, two markers claiming one directory, a marker on a record that is not
/// nested inside its module. They are returned rather than raised because one broken
/// contract should not take the other contracts of the project with it.
pub fn resolve(root: &Path, cfg: &HtlConfig) -> (Vec<Resolved>, Vec<String>) {
    let mut out = Vec::new();
    let mut problems = Vec::new();
    for file in scan_targets(root, cfg) {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        if !src.contains("---@contract") {
            continue;
        }
        match read_file(&file, &src, cfg) {
            Ok(found) => out.extend(found),
            Err(msgs) => problems.extend(msgs),
        }
    }
    // Two contracts for one directory would each have to be the one enforced there.
    for i in 0..out.len() {
        if let Some(j) = out[..i].iter().position(|c| c.dir == out[i].dir) {
            problems.push(format!(
                "{}:{}:1: {} claims directory {:?}, which {} already claims at {}:{} \
                 [htl contract]",
                out[i].declared_in.display(),
                out[i].declared_at,
                out[i].type_path,
                out[i].dir,
                out[j].type_path,
                out[j].declared_in.display(),
                out[j].declared_at,
            ));
        }
    }
    (out, problems)
}

/// Files a marker can be found in: those directly under a search path, and the
/// `init.tl` of an immediate subdirectory (`require("sub")` resolves to `sub/init.tl`).
fn scan_targets(root: &Path, cfg: &HtlConfig) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in cfg.search_paths(root) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<PathBuf> = Vec::new();
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && is_teal(&p) {
                here.push(p);
            } else if p.is_dir() && !crate::is_skipped_dir(&p, &[]) {
                for name in ["init.tl", "init.d.tl"] {
                    let init = p.join(name);
                    if init.is_file() {
                        here.push(init);
                    }
                }
            }
        }
        here.sort();
        out.extend(here);
    }
    out.dedup();
    out
}

fn is_teal(p: &Path) -> bool {
    p.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.ends_with(".tl"))
}

/// The module name a `require` would use for `file`: its stem, or the directory name
/// when the file is an `init.tl`.
fn module_name(file: &Path) -> Option<String> {
    let stem = file.file_name()?.to_str()?.trim_end_matches(".tl");
    let stem = stem.strip_suffix(".d").unwrap_or(stem);
    if stem == "init" {
        return Some(file.parent()?.file_name()?.to_str()?.to_string());
    }
    Some(stem.to_string())
}

/// Every contract declared in one file. `Err` carries what is wrong with the markers it
/// does have, one message per marker, so a file with two of them reports both.
fn read_file(file: &Path, src: &str, cfg: &HtlConfig) -> Result<Vec<Resolved>, Vec<String>> {
    let lines: Vec<&str> = src.lines().collect();
    let Some(module) = module_name(file) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    let mut problems = Vec::new();
    // A marker on its own line belongs to the line below it, so the record is what is
    // looked for first: asking about the marker line by line would attribute a trailing
    // `record Mod ---@contract` to the field declared under it as well.
    for (i, line) in lines.iter().enumerate() {
        let Some(record) = record_name(line) else {
            continue;
        };
        let Some(marker) = marker_on(&lines, i, "contract") else {
            continue;
        };
        let marker = match parse_marker(&marker) {
            Ok(m) => m,
            Err(e) => {
                problems.push(format!("{}:{}:1: {e} [htl contract]", file.display(), i + 1));
                continue;
            }
        };
        let Some(path) = type_path(&lines, i, &module, &record) else {
            problems.push(format!(
                "{}:{}:1: {record} is the module {module} returns, not a type inside it: \
                 a contract type is written as <module>.<Type>, so declare it as a record \
                 within one [htl contract]",
                file.display(),
                i + 1
            ));
            continue;
        };
        let dir = match (marker.dir, cfg.contract.as_slice()) {
            (Some(d), _) => d,
            (None, [one]) => one.dir.clone(),
            (None, []) => {
                problems.push(format!(
                    "{}:{}:1: ---@contract names no directory and htl.toml declares none: \
                     write ---@contract(\"<dir>\") here, or a [[contract]] dir = \"<dir>\" \
                     in htl.toml [htl contract]",
                    file.display(),
                    i + 1
                ));
                continue;
            }
            (None, many) => {
                problems.push(format!(
                    "{}:{}:1: ---@contract names no directory and htl.toml declares {}: \
                     write the directory on the marker [htl contract]",
                    file.display(),
                    i + 1,
                    many.len()
                ));
                continue;
            }
        };
        let inherited: Option<&Contract> = cfg.contract.iter().find(|c| c.dir == dir);
        out.push(Resolved {
            dir,
            type_path: path,
            require_fields: required_fields(&lines, i),
            module: marker.module.or_else(|| inherited.and_then(|c| c.module.clone())),
            exclude: marker
                .exclude
                .or_else(|| inherited.map(|c| c.exclude.clone()))
                .unwrap_or_default(),
            dts: marker.dts,
            declared_in: file.to_path_buf(),
            declared_at: i + 1,
        });
    }
    // A marker nothing picked up: it reads as a contract and does nothing, which is worse
    // than either being one or not being written.
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("---@contract")
            || record_name(line).is_some()
            || lines.get(i + 1).and_then(|l| record_name(l)).is_some()
        {
            continue;
        }
        problems.push(format!(
            "{}:{}:1: ---@contract is not on a record declaration [htl contract]",
            file.display(),
            i + 1
        ));
    }
    if problems.is_empty() {
        Ok(out)
    } else {
        Err(problems)
    }
}

/// The text after `---@<name>` on line `i` or the line above it, `None` when the marker
/// is not there. `Some("")` for a bare marker; `Some("(…)")` when it has arguments.
///
/// The line above counts only when the marker is the whole of it. A marker trailing a
/// declaration belongs to that declaration, and reading it from the line below as well
/// would make one `---@required` mark two fields.
fn marker_on(lines: &[&str], i: usize, name: &str) -> Option<String> {
    let needle = format!("---@{name}");
    let above = i
        .checked_sub(1)
        .and_then(|p| lines.get(p))
        .filter(|l| l.trim_start().starts_with("---"));
    for line in [lines.get(i), above].into_iter().flatten() {
        if let Some(rest) = line.split(&needle).nth(1) {
            // `---@contracts` is not `---@contract`.
            if rest
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            {
                return Some(rest.trim().to_string());
            }
        }
    }
    None
}

/// `("dir", module = "X", dts = "path")` — every part optional, and the whole thing
/// optional. The first argument, if it is a bare string, is the directory.
fn parse_marker(rest: &str) -> Result<Marker, String> {
    let mut m = Marker {
        dir: None,
        module: None,
        exclude: None,
        dts: None,
    };
    if rest.is_empty() {
        return Ok(m);
    }
    let Some(args) = rest.strip_prefix('(').and_then(|r| r.split(')').next()) else {
        return Err(format!(
            "---@contract takes no arguments or a parenthesised list, got {rest:?}"
        ));
    };
    for (n, arg) in args.split(',').map(str::trim).enumerate() {
        if arg.is_empty() {
            continue;
        }
        match arg.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("module", v)) => m.module = Some(unquote(v)?),
            // Space-separated inside one string: a Lua comment is not a place for a list
            // literal, and the names are module names, which have no spaces in them.
            Some(("exclude", v)) => {
                m.exclude = Some(
                    unquote(v)?
                        .split_whitespace()
                        .map(str::to_string)
                        .collect(),
                )
            }
            Some(("dts", v)) => m.dts = Some(unquote(v)?),
            Some((k, _)) => return Err(format!("---@contract has no {k:?} argument")),
            None if n == 0 => m.dir = Some(unquote(arg)?),
            None => return Err(format!("---@contract: {arg:?} is not <name> = <value>")),
        }
    }
    Ok(m)
}

fn unquote(v: &str) -> Result<String, String> {
    let t = v.trim();
    t.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .map(str::to_string)
        .ok_or_else(|| format!("---@contract: {v:?} is not a quoted string"))
}

/// The record declared on `line`, if it declares one.
fn record_name(line: &str) -> Option<String> {
    let after = line.split("record").nth(1)?;
    let name: String = after
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// `<module>.<Type>` for the record declared at `lines[i]`: the module, then every
/// record enclosing this one *except* the outermost, then the record itself. `None` when
/// the record is the outermost one — that is the module, and a contract type has to be a
/// type inside a module for `require("<module>")` to reach it.
fn type_path(lines: &[&str], i: usize, module: &str, record: &str) -> Option<String> {
    let mut names = vec![record.to_string()];
    let mut depth = indent_of(lines[i]);
    for line in lines[..i].iter().rev() {
        if line.trim().is_empty() {
            continue;
        }
        let d = indent_of(line);
        if d < depth && let Some(n) = record_name(line) {
            names.push(n);
            depth = d;
        }
    }
    // The outermost record is the module itself, whatever it is called.
    names.pop()?;
    if names.is_empty() {
        return None;
    }
    names.reverse();
    Some(format!("{module}.{}", names.join(".")))
}

/// Fields marked `---@required` in the record declared at `lines[i]`, in the order they
/// are declared. The record ends at the first `end` indented no deeper than it.
fn required_fields(lines: &[&str], i: usize) -> RequireFields {
    let base = indent_of(lines[i]);
    let mut names = Vec::new();
    for j in i + 1..lines.len() {
        let line = lines[j];
        if line.trim_start().starts_with("end") && indent_of(line) <= base {
            break;
        }
        let name: String = line
            .trim_start()
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() || !line.trim_start()[name.len()..].trim_start().starts_with(':') {
            continue;
        }
        if marker_on(lines, j, "required").is_some() {
            names.push(name);
        }
    }
    RequireFields::Named(names)
}
