//! The help is what a reader who has only the binary gets, and it rots the moment a
//! README section is renamed. Two checks hold it: a help string that names the README
//! carries a URL, and every anchor those URLs point at is a heading `README.md` has.
//!
//! Both read the clap tree (`htl_cli::command()`) rather than a copy of the help kept
//! beside them, so a command added tomorrow is covered without anyone remembering to
//! add it here. No network: an anchor is checked against the file in this repository.

use clap::Command;

/// Where the guide lives. Every in-help link to a section starts with this.
const GUIDE: &str = "https://github.com/ynishi/htl#";

/// Every string the CLI could print as help, each labelled with where it came from so a
/// failure names the doc comment to fix.
fn help_strings() -> Vec<(String, String)> {
    let mut out = Vec::new();
    walk(&htl_cli::command(), "htl", &mut out);
    out
}

fn walk(cmd: &Command, path: &str, out: &mut Vec<(String, String)>) {
    for (what, text) in [
        ("about", cmd.get_about()),
        ("long_about", cmd.get_long_about()),
        ("before_long_help", cmd.get_before_long_help()),
        ("after_long_help", cmd.get_after_long_help()),
    ] {
        if let Some(text) = text {
            out.push((format!("`{path}` {what}"), text.to_string()));
        }
    }
    for arg in cmd.get_arguments() {
        let id = arg.get_id();
        for (what, text) in [("help", arg.get_help()), ("long_help", arg.get_long_help())] {
            if let Some(text) = text {
                out.push((format!("`{path}` --{id} {what}"), text.to_string()));
            }
        }
        for value in arg.get_possible_values() {
            if let Some(text) = value.get_help() {
                out.push((
                    format!("`{path}` --{id} {} help", value.get_name()),
                    text.to_string(),
                ));
            }
        }
    }
    for sub in cmd.get_subcommands() {
        walk(sub, &format!("{path} {}", sub.get_name()), out);
    }
}

/// The anchors `README.md` offers, by GitHub's rule: the heading lowercased, everything
/// that is not a letter, digit, space, hyphen or underscore dropped, spaces hyphenated.
/// Lines inside a fenced block are not headings however many `#` they start with.
fn readme_anchors(readme: &str) -> Vec<String> {
    let mut anchors = Vec::new();
    let mut fenced = false;
    for line in readme.lines() {
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        let heading = rest.trim_start_matches('#');
        if !heading.starts_with(' ') {
            continue;
        }
        anchors.push(anchor_of(heading.trim()));
    }
    anchors
}

fn anchor_of(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// The anchors a help string links to: what follows `…/htl#` up to the first character
/// an anchor cannot contain (the `)` or the newline that closes the sentence).
fn anchors_linked(text: &str) -> Vec<String> {
    text.match_indices(GUIDE)
        .map(|(at, _)| {
            text[at + GUIDE.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                .collect()
        })
        .collect()
}

fn readme() -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A cross-reference the reader cannot follow is not a cross-reference. Somebody reading
/// `--help` may have installed the binary from crates.io and have no checkout at all.
#[test]
fn a_help_string_that_names_the_readme_carries_a_link() {
    let unlinked: Vec<String> = help_strings()
        .into_iter()
        .filter(|(_, text)| text.contains("README") && !text.contains("https://"))
        .map(|(where_, text)| format!("  {where_}: {text}"))
        .collect();
    assert!(
        unlinked.is_empty(),
        "these help strings send the reader to the README without a way to reach it:\n{}",
        unlinked.join("\n"),
    );
}

/// And a link to a section that no longer exists is worse than none: it reads as though
/// it were checked. Renaming a heading in `README.md` fails here.
#[test]
fn every_linked_anchor_is_a_heading_the_readme_has() {
    let anchors = readme_anchors(&readme());
    let missing: Vec<String> = help_strings()
        .into_iter()
        .flat_map(|(where_, text)| {
            anchors_linked(&text)
                .into_iter()
                .map(move |a| (where_.clone(), a))
        })
        .filter(|(_, a)| !anchors.contains(a))
        .map(|(where_, a)| format!("  {where_}: #{a}"))
        .collect();
    assert!(
        missing.is_empty(),
        "these help links name a section README.md does not have:\n{}\n\
         the anchors it does have:\n  {}",
        missing.join("\n"),
        anchors.join("\n  "),
    );
}

/// The links have to be reached before either check above means anything, and the root
/// is where a reader with nothing else starts.
#[test]
fn the_root_long_help_carries_the_guide_and_the_api_docs() {
    let long = htl_cli::command().render_long_help().to_string();
    assert!(
        long.contains("https://github.com/ynishi/htl"),
        "`htl --help` does not link the guide:\n{long}"
    );
    assert!(
        long.contains("https://docs.rs/htl"),
        "`htl --help` does not link the API documentation an embedding host reads:\n{long}"
    );
}

/// `-h` is the one somebody types mid-command, and it stays the summary it is: the long
/// form is where the prose and the URLs went.
#[test]
fn the_short_help_stays_a_summary() {
    let short = htl_cli::command().render_help().to_string();
    assert!(
        !short.contains("http"),
        "`htl -h` grew a URL; that belongs in --help:\n{short}"
    );
    let lines = short.lines().count();
    assert!(lines <= 31, "`htl -h` is {lines} lines:\n{short}");
}

/// clap's own check of the tree the two tests above walk.
#[test]
fn the_command_tree_is_well_formed() {
    htl_cli::command().debug_assert();
}

/// The first cell of each row in `README.md`'s `## CLI` table.
fn readme_cli_cells(readme: &str) -> Vec<String> {
    let section = readme
        .split_once("\n## CLI\n")
        .map(|(_, rest)| rest)
        .unwrap_or_else(|| panic!("README.md has no `## CLI` section"));
    let section = section
        .split_once("\n## ")
        .map(|(body, _)| body)
        .unwrap_or(section);
    section
        .lines()
        .filter(|l| l.starts_with('|'))
        .skip(2)
        .map(|row| {
            row.trim_start_matches('|')
                .split('|')
                .next()
                .unwrap_or("")
                .trim()
                .to_string()
        })
        .collect()
}

/// Issue #541: `README.md`'s `## CLI` table has one row per subcommand `htl --help` lists,
/// and each row's first column names only `htl <subcommand>` — flags and arguments live in
/// `htl <cmd> --help`, not in a second copy in the README that would drift from `Cmd`.
#[test]
fn the_readme_cli_table_has_one_row_per_subcommand() {
    let cells = readme_cli_cells(&readme());
    let mut table_cmds = Vec::with_capacity(cells.len());
    for cell in &cells {
        let sub = cell
            .strip_prefix("`htl ")
            .and_then(|s| s.strip_suffix('`'))
            .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
            .unwrap_or_else(|| {
                panic!(
                    "CLI table cell `{cell}` must be `` `htl <subcommand>` `` with no flags or \
                     arguments (send the reader to `htl <cmd> --help` instead)"
                )
            });
        table_cmds.push(sub.to_string());
    }

    let mut clap_cmds: Vec<String> = htl_cli::command()
        .get_subcommands()
        .filter(|s| !s.is_hide_set() && s.get_name() != "help")
        .map(|s| s.get_name().to_string())
        .collect();

    let mut table_sorted = table_cmds.clone();
    table_sorted.sort();
    clap_cmds.sort();
    assert_eq!(
        table_sorted, clap_cmds,
        "README.md's `## CLI` table rows do not match the subcommands of `htl --help`"
    );
}
