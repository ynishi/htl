//! `htl api`: the project's public Teal surface, as one text file to commit.
//!
//! The declarations a project publishes are already generated and already committed —
//! the `.d.tl` for a Rust `#[host_module]`, the module a `---@contract` type is declared
//! in — but they are several files, written for a compiler. A reviewer looking at a pull
//! request that touches three of them has to read three generated files to answer one
//! question: did what a consumer can name change, and how.
//!
//! This gathers them into one sorted file. Nothing here is new information; the value is
//! that it is in one place, in a stable order, so an ordinary diff is the answer.
//!
//! What counts as *this project's* surface:
//!
//! - the `.d.tl` a Rust host in this tree publishes (`#[host_module(dts = ..)]`, and the
//!   records `#[teal(dts = ..)]` gives files of their own),
//! - the module each `---@contract` type is declared in, which is what an outside module
//!   author writes against,
//! - the module `mlua-pkg.toml` names as the package entry — what a consumer that depends
//!   on this package gets from `require("<name>")`.
//!
//! And what does not: the declarations a *dependency* ships, materialised under
//! `types/<crate>/`. Those describe somebody else's crate, and a diff of them is news
//! about an upgrade, not about this project's promise. The C header a `#[c_export]` block
//! writes is out too — it is a surface, but not a Teal one, and a C caller reads the
//! header itself.

use crate::config::HtlConfig;
use serde::Serialize;
use std::path::Path;

/// Which kind of surface an entry is. The order is the order entries are reported in:
/// the package entry first (what most consumers name), then the Rust host, then the
/// contracts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// The `mlua-pkg.toml` package entry: `require("<name>")`.
    Package,
    /// A `#[host_module]` in this tree.
    HostModule,
    /// A `#[derive(TealRecord)]` type with a `.d.tl` of its own.
    Record,
    /// The module a `---@contract` type is declared in.
    Contract,
}

impl Kind {
    /// How the kind is written in the report's header line.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Package => "package",
            Kind::HostModule => "host module",
            Kind::Record => "record",
            Kind::Contract => "contract",
        }
    }
}

/// One published declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub kind: Kind,
    /// What a consumer names: the module name, or the type's.
    pub name: String,
    /// The file this was read from or published to, relative to the project root.
    pub path: String,
    /// The declaration itself, one line per member, blank lines and the trailing
    /// `return <name>` dropped.
    pub declaration: Vec<String>,
}

/// The surface, and what could not be read of it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub entries: Vec<Entry>,
    /// Problems, for stderr: a contract that cannot be published, a package entry whose
    /// module is not where the manifest says. Deliberately not part of the file — the
    /// report is what the source says, and a machine that cannot read one file should not
    /// silently rewrite the committed surface into a shorter one.
    #[serde(skip)]
    pub notes: Vec<String>,
}

/// The format version, in the report's first line. A later reader (`--check`) can tell
/// what it is looking at before it parses.
pub const VERSION: u32 = 1;

const HEADER: &str = "\
# htl api 1
#
# The public Teal surface of this project: what a consumer of it can name. Written by
# `htl api`, and meant to be committed — a diff of this file is a change to that surface.
# Each entry is the published declaration, one line per member.
";

/// Collect the surface. `root` is the project root every path is reported relative to;
/// `cargo_root` is the Rust package in the tree, when there is one.
pub fn collect(root: &Path, cfg: &HtlConfig, cargo_root: Option<&Path>) -> Report {
    let mut report = Report::default();
    package_entry(root, &mut report);
    if let Some(cargo_root) = cargo_root {
        rust_declarations(root, cargo_root, &mut report);
    }
    contracts(root, cfg, &mut report);
    // Two runs with no source change write the same bytes: the scans are ordered already,
    // and this makes the order a property of the report rather than of the walk.
    report
        .entries
        .sort_by(|a, b| (a.kind, &a.name, &a.path).cmp(&(b.kind, &b.name, &b.path)));
    report.entries.dedup();
    report
}

/// The module `mlua-pkg.toml` names as the entry, declared: a consumer requires it by the
/// package name, so its records and functions are this project's most-named surface.
fn package_entry(root: &Path, report: &mut Report) {
    let manifest = root.join("mlua-pkg.toml");
    if !manifest.is_file() {
        return;
    }
    let m = match mlua_pkg::manifest::Manifest::from_path(&manifest) {
        Ok(m) => m,
        Err(e) => {
            report.notes.push(format!("reading mlua-pkg.toml: {e}"));
            return;
        }
    };
    let name = m.package.name.clone();
    let Some(entry) = m.package.entry.as_ref() else {
        return;
    };
    // `entry` is a directory, so `require("<name>")` looks for `<name>/init.tl`; a flat
    // package ships `<name>/<name>.tl` instead (README, "Layout of a project").
    let dir = root.join(entry);
    let candidates = [
        dir.join("init.d.tl"),
        dir.join("init.tl"),
        dir.join(format!("{name}.d.tl")),
        dir.join(format!("{name}.tl")),
    ];
    let Some(file) = candidates.into_iter().find(|p| p.is_file()) else {
        report.notes.push(format!(
            "package entry `{}` has no init.tl (or {name}.tl): nothing to report for \
             require(\"{name}\")",
            entry.display()
        ));
        return;
    };
    let Ok(src) = std::fs::read_to_string(&file) else {
        return;
    };
    // A `.d.tl` is already a declaration; a `.tl` is the implementation, and what a
    // consumer can name of it is the same transform `htl` publishes a contract module
    // with — bodies removed, functions folded into the record they are on.
    let text = if file.extension().and_then(|s| s.to_str()) == Some("tl")
        && !file.to_string_lossy().ends_with(".d.tl")
    {
        match crate::contract::declaration_of(&src) {
            Ok(t) => t,
            Err(msgs) => {
                for m in msgs {
                    report.notes.push(format!(
                        "{}:{m} declaring the package entry",
                        file.display()
                    ));
                }
                return;
            }
        }
    } else {
        src
    };
    report.entries.push(Entry {
        kind: Kind::Package,
        name,
        path: rel(root, &file),
        declaration: body(&text),
    });
}

/// The `.d.tl` this tree's Rust source asks for. Nothing is built to read them: the scan
/// is the one `htl dts` runs, pointed at a report instead of at the files.
fn rust_declarations(root: &Path, cargo_root: &Path, report: &mut Report) {
    let generated = match crate::dts::scan_crate(cargo_root) {
        Ok(g) => g,
        Err(e) => {
            report.notes.push(e);
            return;
        }
    };
    for g in generated {
        let Some((what, name)) = g.what.split_once(' ') else {
            continue;
        };
        let kind = match what {
            "host_module" => Kind::HostModule,
            // `record` / `enum` / `type`, from `RecordDecl::what`.
            "record" | "enum" | "type" => Kind::Record,
            // A C header is a surface, and not a Teal one.
            _ => continue,
        };
        report.entries.push(Entry {
            kind,
            name: name.to_string(),
            path: rel(root, &g.target),
            declaration: body(&g.text),
        });
    }
}

/// The module each `---@contract` type is declared in. The declaration is computed, not
/// read from the committed copy: the report says what the source publishes, so it is
/// right on a tree where `htl dts` has not been run since the last edit.
fn contracts(root: &Path, cfg: &HtlConfig, report: &mut Report) {
    let (resolved, problems) = crate::contract::resolve(root, cfg);
    report.notes.extend(problems);
    let (published, problems) = crate::contract::declarations(root, &resolved);
    report.notes.extend(problems);
    for p in &published {
        report.entries.push(Entry {
            kind: Kind::Contract,
            name: p.type_path.clone(),
            path: rel(root, &p.target),
            declaration: body(&p.text),
        });
    }
}

/// The declaration's lines: blank lines dropped (they carry nothing and would move when
/// the generator's spacing changes) and the trailing `return <name>` with them — a
/// consumer names what the record holds, not the statement that hands it over.
fn body(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if lines
        .last()
        .is_some_and(|l| l.starts_with("return ") && !l.contains('('))
    {
        lines.pop();
    }
    lines
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The report as the text file to commit.
pub fn render(report: &Report) -> String {
    let mut out = String::from(HEADER);
    if report.entries.is_empty() {
        // A file with nothing in it says two things at once — no surface, and a broken
        // command — so it says which. A project with no host module, no contract and no
        // package entry has a surface of nothing, and that is a fact worth committing.
        out.push_str(
            "\n\
             (nothing: this project publishes no host module declaration, no ---@contract\n\
             type, and no mlua-pkg.toml package entry)\n",
        );
        return out;
    }
    for e in &report.entries {
        out.push('\n');
        out.push_str(&header_line(e));
        out.push('\n');
        for l in &e.declaration {
            out.push_str("  ");
            out.push_str(l);
            out.push('\n');
        }
    }
    out
}

/// `<kind> <name>  <path>`, with what a consumer types beside a package entry.
fn header_line(e: &Entry) -> String {
    match e.kind {
        Kind::Package => format!(
            "{} {}  require(\"{}\")  {}",
            e.kind.label(),
            e.name,
            e.name,
            e.path
        ),
        _ => format!("{} {}  {}", e.kind.label(), e.name, e.path),
    }
}

/// The same report as JSON, for a reader that is not a person.
pub fn render_json(report: &Report) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "version": VERSION,
        "entries": report.entries,
    }))
    .unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_drops_blank_lines_and_the_return() {
        let lines = body("local record m\n\n   f: function(): integer\nend\n\nreturn m\n");
        assert_eq!(
            lines,
            vec![
                "local record m".to_string(),
                "   f: function(): integer".into(),
                "end".into()
            ]
        );
    }

    /// A record whose last field is a function returning something is not a `return`
    /// statement, and stays.
    #[test]
    fn body_keeps_a_field_named_like_a_return() {
        let lines = body("local record m\n   returner: function(): integer\nend\nreturn m\n");
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn an_empty_surface_says_so() {
        let text = render(&Report::default());
        assert!(text.contains("nothing:"), "{text}");
        assert!(text.starts_with("# htl api 1\n"), "{text}");
    }
}
