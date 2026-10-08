//! Which of htl's nine markers a project has actually written, and where.
//!
//! `htl check` answers whether the files are correct. `htl unused` answers what nothing
//! reaches. Neither answers how much of the toolchain a project has taken up: `---@struct`,
//! `---@optional`, `---@sealed`, `---@extensible`, `---@nilable`, `---@contract`,
//! `---@required`, `---@async` and `---@noyield` are each silent until a declaration
//! carries them, by design — [`crate::lint`]'s module doc says so of four of the lints
//! they drive: "Four rules are silent until a record or a function carries a marker." A
//! project that never heard of a marker checks exactly as clean as one that read the
//! section describing it and decided against it, and nothing before this command told the
//! two apart (#304).
//!
//! # What `used` is
//!
//! The census [`CheckInfo::markers`](crate::CheckInfo::markers) already took while checking
//! every file — one entry per `(marker, declaration)` pair, found at the position the
//! marker lints themselves read from: for a field, that is the line of its own `name:
//! type` (a type that wraps to the next line is not counted, the same way a record's
//! field and its type disagreeing on the line keeps `---@struct` from reading it either),
//! and an interface's fields are the interface's own, never the implementing record's,
//! however many records `is` it. `used` is the count of
//! those entries for a marker, and [`Feature::sites`] is where each one is: `kind` is
//! `"record"`, `"field"` or `"function"`, whichever the marker sits on, `name` is the
//! declaration's own name (qualified for a nested record or a field: `Foo.Bar`,
//! `Foo.field`), and `line` is the
//! declaration's own line (the marker is on it or on the line directly above).
//!
//! # What `applicable` is
//!
//! The issue's shape has a second column: how many declarations the project *could* mark
//! today, on the evidence of its own code — a record every construction site already sets
//! every field of, say, for `---@struct`. That evidence comes from a lint, not from this
//! command, so that the count and the locations a reader is sent to cannot drift apart:
//! this command never invents a denominator a lint has not supplied. Until one exists for
//! a marker, [`Feature::applicable`] for it is `None` rather than `0` — a marker with
//! nothing counted is not the same claim as a marker nothing is applicable to, and this
//! command cannot tell the two apart without a lint behind the count, so it says neither.
//!
//! `---@struct` is the first row with one: [`crate::unmarked_structs`] (#304),
//! [`crate::lint`]'s `unmarked-struct` rule's own input, is a record declared among the
//! files the walk checked, built whole at every one of its construction sites, and left
//! unmarked — [`unmarked_structs`](crate::unmarked_structs)'s doc has the four conditions;
//! this command narrows its result to the paths asked about the same way [`Feature::sites`]
//! are (a candidate counts under the path its *declaration* is under, not a construction
//! site's). [`Feature::candidates`] is that list; `applicable` is its length. No other
//! marker has a lint like it yet, so every other row stays `None`, for the reason above.
//! [`Summary::applicable`] sums the rows that do, which today is just the one — not yet a
//! project-wide ratio ("N of M opportunities"): one denominator does not make a whole, and
//! the summary line does not claim one until more rules supply a count to add to it.
//!
//! # A report, not a gate
//!
//! The exit code is always success. A percentage that fails a build invites marking records
//! to move the number rather than because the project earned it; the established shape for
//! this kind of measure is a ratchet against regression once the number has settled, not a
//! bar a single run must clear, and that is for later, if ever. This release only displays.
//!
//! # `.d.tl` is not counted
//!
//! `htl dts` writes `---@async` / `---@noyield` into a declaration itself, so a `.d.tl`'s
//! own markers are not a project author's writing — the census this reads already excludes
//! them ([`crate::CheckInfo::markers`]'s own doc), and the file contributes nothing to any
//! row.
//!
//! # Replays from `.htl/`
//!
//! The census is read off the same check `htl check` and `htl unused` run, so an unchanged
//! project replays it from the store rather than parsing anything new — `--explain-cache`
//! shows the hits, as it does for `htl unused`.
//!
//! # What it prints
//!
//! ```text
//! feature          used  applicable
//! ---@struct          2           1
//! ---@optional        1           -
//! ---@sealed          0           -
//! ---@extensible      0           -
//! ---@nilable         1           -
//! ---@contract        0           -
//! ---@required        0           -
//! ---@async           0           -
//! ---@noyield         0           -
//! htl adopt: 3 of 9 features used, 4 markers in 2 files
//! ```
//!
//! `--detail` inserts, between the table and the summary line, one line per site, grouped
//! by feature, a feature with no site and no candidate omitted; within a feature a used
//! site comes first, then a candidate, each candidate's line ending with how many
//! construction sites it has:
//!
//! ```text
//!   ---@struct      src/a.tl:3   Foo
//!   ---@struct      src/b.tl:10  Bar
//!   ---@struct      src/a.tl:11  Point  (applicable: built whole at 2 sites)
//!   ---@optional    src/a.tl:4   Foo.x
//!   ---@nilable     src/a.tl:8   find
//! ```
//!
//! and when [`Summary::check_errors`] is more than zero, one line before the summary:
//!
//! ```text
//! htl adopt: 2 error(s) in the project's check (htl check says which); a file with a syntax error contributes no markers
//! ```
//!
//! `--format json`: `{ features: [{ marker, used, applicable, sites: [{ file, line, kind,
//! name }], candidates: [{ file, line, name, sites }] }], summary: { features, used,
//! markers, files, check_errors, applicable } }`. `applicable` is `null`, not `0`, for a
//! row with no lint behind it (see above); `sites[].file` and `candidates[].file` are
//! relative to the working directory, the way every other path this tool prints is
//! ([`project::display_path`]).
use crate::cache;
use crate::project::{self, Config};
use anyhow::Result;
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// htl's nine markers, without their `---@`, in the order the table always prints them —
/// every one of them, whether the project uses it or not.
pub const FEATURES: [&str; 9] = [
    "struct",
    "optional",
    "sealed",
    "extensible",
    "nilable",
    "contract",
    "required",
    "async",
    "noyield",
];

/// What a report is asked about: the paths, the config, and the store's switches. As
/// [`crate::unused::Options`] — no `lint` here either, for the same reason: the census is
/// read off a check's own result rather than judged by one.
pub struct Options<'a> {
    /// The paths as the command line spelled them. They narrow the report, not the walk,
    /// as [`crate::unused`]'s do; empty means the working directory.
    pub paths: &'a [PathBuf],
    /// `htl.toml`, already loaded ([`crate::project::config_of`]).
    pub config: &'a Config,
    /// The project's [model](crate::model) ([`crate::project::model_of`]).
    pub model: Option<&'a crate::model::Project>,
    /// The run cache's switches ([`crate::project::cache_options`]): the census comes from
    /// a check, and a check replays.
    pub cache: cache::Options,
}

/// One marked declaration ([`crate::MarkerSite`] via [`cache::MarkerSiteJson`]), under the
/// feature it names.
#[derive(Serialize, Debug, Clone)]
pub struct Site {
    /// As every other path in this tool's output reads ([`project::display_path`]).
    pub file: PathBuf,
    /// Line of the declaration, counted from 1 — always the declaration's own line, never
    /// the marker's: the marker itself is on that line (trailing) or on the line directly
    /// above it (on a line of its own), and either way `line` is the declaration's.
    pub line: usize,
    /// What the marker is on: `"record"`, `"field"` or `"function"`.
    pub kind: String,
    /// The declaration's own name, qualified for a nested record or a record field
    /// (`Foo.Bar`, `Foo.field`).
    pub name: String,
}

/// One declaration a lint has found the evidence to call a candidate for a marker — a
/// row's `applicable` count, in `--detail` form ([`crate::UnmarkedStruct`] for
/// `---@struct`, relativised the way [`Site::file`] is).
#[derive(Serialize, Debug, Clone)]
pub struct Candidate {
    /// The declaration's own file, as every other path this tool prints is
    /// ([`project::display_path`]).
    pub file: PathBuf,
    /// Line of the declaration itself.
    pub line: usize,
    /// The declaration's own name.
    pub name: String,
    /// How many construction sites made it a candidate.
    pub sites: usize,
}

/// One row of the table: one of htl's nine markers.
#[derive(Serialize, Debug, Clone)]
pub struct Feature {
    /// The marker's name, without its `---@`.
    pub marker: String,
    /// How many declarations carry it — `sites.len()`.
    pub used: usize,
    /// How many declarations a lint has found the evidence to call candidates for this
    /// marker, or `None` when no such lint exists yet (see the module doc). Never a
    /// made-up number: a feature without evidence is left without a denominator rather
    /// than given one. `Some(candidates.len())` when it is some.
    pub applicable: Option<usize>,
    /// Where each one is, ordered by file, then line, then name, then kind (the tie-break
    /// two declarations on one line need).
    pub sites: Vec<Site>,
    /// What `applicable` counts, when it counts anything — empty for a feature whose
    /// `applicable` is `None`. Ordered the same way [`sites`](Self::sites) is.
    pub candidates: Vec<Candidate>,
}

/// The counts a summary line is made of.
#[derive(Serialize, Debug, Clone, Default)]
pub struct Summary {
    /// htl's markers: 9, always, whether used or not.
    pub features: usize,
    /// How many of the nine features have at least one site.
    pub used: usize,
    /// Every site of every feature — the total count the table's `used` column adds to.
    pub markers: usize,
    /// Distinct files that carry at least one marked declaration.
    pub files: usize,
    /// The sum of every feature's `applicable`, over the features that have one — not a
    /// project-wide ratio (see the module doc): a row with no lint behind it contributes
    /// nothing here, the same way it contributes nothing to `used`.
    pub applicable: usize,
    /// Errors in the project's check, project-wide (not narrowed to the paths asked
    /// about) — outside a project (no `htl.toml`), the paths given, since there is no
    /// wider project to walk — `htl check` says which file and which error. Only a file
    /// with a syntax error contributes no markers to the census; a file with a type error
    /// still yields a full one, so this count is not automatically the shortfall it might
    /// look like.
    pub check_errors: usize,
}

/// The nine-feature table and the counts behind it.
#[derive(Serialize, Debug, Clone)]
pub struct Report {
    /// htl's nine markers, always in [`FEATURES`] order.
    pub features: Vec<Feature>,
    /// The counts behind the summary line.
    pub summary: Summary,
}

/// Count how many declarations of the project carry each of htl's nine markers, and where.
///
/// A replay of the check `htl check` and `htl unused` already run: the files are collected
/// as [`crate::unused::unused`] collects them, the check runs with a sink that discards its
/// diagnostics (they are `htl check`'s to report, not this command's) and no lint selection,
/// and the census it hands back ([`project::Report::markers`]) is folded into the nine
/// features. The exit status that goes with a report like this is the caller's business,
/// not this function's — it only ever returns `Ok`, or an error a check itself would raise
/// (a directory that does not exist, a config that does not parse).
pub fn adopt(opts: &Options<'_>) -> Result<Report> {
    let given: Vec<PathBuf> = if opts.paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        opts.paths.to_vec()
    };
    // The project's root, from its model, as every other command takes it; outside any
    // project the working directory stands in for one.
    let root = match opts.model {
        Some(m) => m.root.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    // The walk is always the whole project, as `unused`'s is: the check behind the census
    // has to resolve and replay exactly as it does for the project as a whole -- require
    // closures included -- and narrowing it to one directory would be a different check,
    // not this one answered about a smaller place. What narrows is the report, below
    // (`asked`): only the files under the paths given are folded into the counts.
    let walk: Vec<PathBuf> = match opts.model {
        Some(_) => vec![root.clone()],
        None => given.clone(),
    };
    let skip = project::not_walked(opts.model, &walk, crate::model::Purpose::Own);
    let (files, _) =
        project::held_by_modules(opts.model, &walk, crate::collect_tl_skipping(&walk, &skip)?)?;

    // Discarded: the diagnostics are `htl check`'s to report, not this command's. Only the
    // census (`checked.markers`) and the error count are read back.
    let mut sink = project::Sink::new(project::Collect::default());
    let checked = project::check(
        &mut sink,
        &files,
        &project::Options {
            paths: &walk,
            config: opts.config,
            model: opts.model,
            lint: None,
            cache: opts.cache,
        },
    )?;

    // The counts are of the files under the paths given (`given`, canonicalised): a
    // marker found while walking the whole project above but under a file outside these
    // paths does not contribute to any row. With no path, `given` is the working
    // directory, so every file under it counts.
    let asked: Vec<PathBuf> = given.iter().map(|p| canon(p)).collect();
    let mut features: Vec<Feature> = FEATURES
        .iter()
        .map(|&marker| Feature {
            marker: marker.to_string(),
            used: 0,
            applicable: None,
            sites: Vec::new(),
            candidates: Vec::new(),
        })
        .collect();
    let mut files_with_site: HashSet<PathBuf> = HashSet::new();
    for (f, sites) in &checked.markers {
        if sites.is_empty() {
            continue;
        }
        let c = canon(f);
        if !asked.iter().any(|a| c == *a || c.starts_with(a)) {
            continue;
        }
        files_with_site.insert(c);
        for s in sites {
            let Some(i) = FEATURES.iter().position(|&m| m == s.marker) else {
                continue;
            };
            features[i].sites.push(Site {
                file: PathBuf::from(project::display_path(f)),
                line: s.line,
                kind: s.kind.clone(),
                name: s.name.clone(),
            });
        }
    }
    let mut markers = 0usize;
    for feat in &mut features {
        // By name then kind too, beyond file and line: two declarations on one line (the
        // Lua side ties the same way, for the same reason) would otherwise order
        // however the walk that produced them happened to, run to run.
        feat.sites.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then(a.line.cmp(&b.line))
                .then(a.name.cmp(&b.name))
                .then(a.kind.cmp(&b.kind))
        });
        feat.used = feat.sites.len();
        markers += feat.used;
    }

    // `---@struct`'s denominator: the same four-condition census `unmarked-struct` reads
    // (`crate::unmarked_structs`), over the construction-site census `checked` read back
    // through the cache rather than a fresh `CheckInfo` -- the json shape the lint's own
    // call converts from a `CheckInfo`, this one converts from `cache::StructSiteJson`,
    // so the two callers share the function's body instead of each walking the four
    // conditions themselves (#304).
    let struct_sites: Vec<(PathBuf, Vec<crate::StructSite>)> = checked
        .struct_sites
        .iter()
        .map(|(f, sites)| {
            (
                f.clone(),
                sites
                    .iter()
                    .map(|s| crate::StructSite {
                        record_file: s.record_file.clone(),
                        record_line: s.record_line,
                        record_name: s.record_name.clone(),
                        marked: s.marked,
                        line: s.line,
                        col: s.col,
                        complete: s.complete,
                    })
                    .collect(),
            )
        })
        .collect();
    let mut candidates: Vec<Candidate> = crate::unmarked_structs(&struct_sites)
        .into_iter()
        .filter(|c| {
            let f = canon(&c.file);
            asked.iter().any(|a| f == *a || f.starts_with(a))
        })
        // `unmarked_structs` does not read allow comments — it is not a lint, and
        // `requires_only` carries no source text to read one from — so a project that
        // silenced `unmarked-struct` on this declaration (`-- htl: allow(unmarked-struct)`)
        // would otherwise see the lint stay quiet while this count still moved: the same
        // check `Lints::keep` makes for the lint's own finding, shared through
        // `lint::line_is_allowed` so the two cannot drift on what "allowed" means.
        .filter(|c| !crate::lint::line_is_allowed(&c.file, c.line, "unmarked-struct"))
        .map(|c| Candidate {
            file: PathBuf::from(project::display_path(&c.file)),
            line: c.line,
            name: c.name,
            sites: c.sites,
        })
        .collect();
    candidates.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    let struct_idx = FEATURES
        .iter()
        .position(|&m| m == "struct")
        .expect("struct is one of FEATURES");
    features[struct_idx].applicable = Some(candidates.len());
    features[struct_idx].candidates = candidates;

    let summary = Summary {
        features: FEATURES.len(),
        used: features.iter().filter(|f| f.used > 0).count(),
        markers,
        files: files_with_site.len(),
        check_errors: checked.errors,
        applicable: features.iter().filter_map(|f| f.applicable).sum(),
    };
    Ok(Report { features, summary })
}

/// As [`crate::unused`]'s own `canon`: resolved so a file named two different ways is the
/// one entry either way, falling back to the path as given when it does not exist.
fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}
