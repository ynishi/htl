//! `htl pin <release|main|path:<checkout>> [dir] [--no-update]`: move the htl a project
//! `htl new` already wrote depends on.
//!
//! A scaffolded project pins four things to one tree — `Cargo.toml`'s `htl` line (and
//! `htl-mq`'s, under the window target), `mise.toml`, `[toolchain] htl` in `htl.toml` when
//! the project wrote one, and `Cargo.lock` — because [`crate::scaffold::HtlPin`] decides
//! all four at `htl new` time and nothing moves them afterwards (see `scaffold.rs`, "What
//! the output pins"). This module moves the same four by hand: read back out of the tree
//! `htl new` already wrote rather than computed fresh, and finished with the `cargo
//! update` that makes the lockfile agree.
//!
//! `release` is this CLI's own version, by name, so `htl pin release` is the way back
//! from `main` or `path:` once the checkout a project moved to is cut — before that, the
//! released number does not have to be on crates.io for this to write it, because
//! nothing here resolves it; only the `cargo update` at the end does, after the files are
//! written. A number is refused for the reason `htl new --htl <number>` refuses one:
//! this binary writes `htl.toml` keys and macro output for the htl it links, not for an
//! arbitrary release, and the CLI that writes for that release is on crates.io beside it.
//!
//! # Why a line this does not recognise is refused rather than guessed
//!
//! The scaffold writes one of three shapes for `htl` — a bare version; `git` + `branch`;
//! `path` — plus an optional `features` list, and nothing else. A line in any other shape
//! is one a person wrote by hand, for a reason this command was never told: a `workspace =
//! true` inheritance, a `rev` pinned past a `tag`, an `optional` dependency. Guessing at it
//! — dropping the parts this does not understand, say — would be silently discarding that
//! reason; refusing, and naming the `[patch.crates-io]` block instead, leaves the decision
//! with whoever already made it. That block stays the documented fallback for exactly this
//! case, and for every consumer this command does not reach at all — one that is not an
//! `htl new` tree in the first place. README, "Running against an unpublished htl":
//! <https://github.com/ynishi/htl#running-against-an-unpublished-htl>
//!
//! A manifest with no `htl` dependency at all is not this case: it is not a shape this
//! does not recognise, it is a crate `htl new` never wrote, and the `[patch.crates-io]`
//! block would do nothing for it. That gets its own, different refusal — see
//! [`cmd_pin`]'s first check.
//!
//! # Why `[toolchain] htl` is removed rather than rewritten under `main` / `path:`
//!
//! The key is a published-release requirement ([`htl::config::ToolchainConfig`]): `main`
//! and a checkout have no release to ask for. Leaving the old number behind would refuse
//! the very CLI the project was just pointed at; absent is the value every project `htl new
//! --htl main` already writes, not one this command invents for the occasion.
//!
//! # Why this bypasses the project's own `[toolchain]` check
//!
//! Every other command loads the project through [`crate::load_config`], which refuses to
//! run at all when `[toolchain] htl` excludes it — the whole point of the key. A project
//! someone wants to `htl pin` is exactly a project whose pin needs to change, which is the
//! one case that refusal must not catch: the command that moves a project off a toolchain
//! it no longer satisfies cannot itself require satisfying it first. So this walks up to
//! the nearest `Cargo.toml` with [`htl::dts::find_cargo_package_root`] instead — the same
//! root `htl dts` finds a crate by — and never calls `load_config`.
//!
//! # Why the old value is read from the parsed item rather than the file text
//!
//! A dependency line can be spelled `htl = "0.13.0"`, `htl="0.13.0"`, `htl   = "0.13.0"`
//! or `"htl" = "0.13.0"` — all four are the same TOML, and cargo reads all four the same
//! way. Finding "the `htl` line" by searching the raw text for the literal substring
//! `"htl = "` reads only the first spelling, panics on the rest, and — far worse — can
//! match an unrelated `htl = ...` line in a different table altogether (`[dev-
//! dependencies]`, a stray `[patch.crates-io]` block above `[dependencies]`) and report a
//! move that never touched `[dependencies]`. The old value is read off the `Item`
//! [`recognised_features`] already found under `doc["dependencies"]["htl"]`, which is
//! correct by construction: whatever table and whatever spelling, toml_edit already
//! resolved it to the right entry.
//!
//! # Why every file is read and parsed before anything is written
//!
//! `htl.toml` is parsed after `Cargo.toml`'s new text is already computed; if it does not
//! parse — or cannot even be read, `chmod 000` and the like, which [`read_if_present`]
//! tells apart from "there is no such file" — the whole command fails with nothing
//! written at all, not even the `Cargo.toml` that was otherwise ready to go. `mise.toml`
//! existing as something other than a plain file (a directory, say) is refused here for
//! the same reason, before `Cargo.toml` is touched, rather than discovered midway through
//! writing when `fs::write` or `fs::remove_file` meets it and fails.
//!
//! Each file is still reported as it is written, though, rather than all three only once
//! every write has gone through: a later file's write failing — after the manifest and
//! `mise.toml` already went through, a disk that filled up partway, say — still leaves
//! the lines for what did succeed on stderr, instead of silence about files that were, in
//! fact, moved.

use crate::scaffold::{HtlPin, dep_value, t_mise};
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use toml_edit::{DocumentMut, Value};

/// README, "Running against an unpublished htl": the block a `Cargo.toml` this command
/// does not recognise still has, by hand, and the command that resolves it against the
/// checkout named there.
const PATCH_BLOCK: &str = "[patch.crates-io]\n\
htl = { path = \"/path/to/htl/crates/htl\" }\n\
htl-core = { path = \"/path/to/htl/crates/htl-core\" }\n\
htl-macros = { path = \"/path/to/htl/crates/htl-macros\" }\n";

pub(crate) fn cmd_pin(htl: &str, dir: Option<PathBuf>, no_update: bool) -> Result<ExitCode> {
    let pin = HtlPin::parse(Some(htl))?;

    // `path:` is resolved against this process's working directory, the way a person
    // typing it at the shell means it — not against `dir` (which may name a project
    // nowhere near here) and not against the project root (which the manifest's own
    // relative paths would be, but a path typed on the command line is not one of
    // those). Canonicalising also catches a typo here, before anything is touched,
    // rather than inside `cargo update` with a message about a lockfile instead of a
    // checkout.
    let pin = match pin {
        HtlPin::Path(p) => match canonical_checkout(&p)? {
            Some(checkout) => HtlPin::Path(checkout),
            None => {
                eprintln!(
                    "htl pin: path:{} is not an htl checkout (no crates/htl/Cargo.toml there)",
                    p.display()
                );
                return Ok(ExitCode::from(2));
            }
        },
        other => other,
    };

    let start = dir.unwrap_or_else(|| PathBuf::from("."));
    let Some(root) = htl::dts::find_cargo_package_root(&start) else {
        eprintln!(
            "htl pin: no Cargo.toml with a [package] at or above {}",
            start.display()
        );
        return Ok(ExitCode::from(2));
    };

    // --- Read and recognise, nothing written yet. ---

    let manifest_path = root.join("Cargo.toml");
    let mut cargo_doc = fs::read_to_string(&manifest_path)
        .with_context(|| format!("reading {}", manifest_path.display()))?
        .parse::<DocumentMut>()
        .with_context(|| format!("parsing {}", manifest_path.display()))?;

    let htl_item = cargo_doc
        .get("dependencies")
        .and_then(|i| i.as_table_like())
        .and_then(|d| d.get("htl"));
    // Not "a line this does not recognise" — see the module doc — but a manifest that
    // never named htl in the first place, which the `[patch.crates-io]` refusal has
    // nothing useful to say about.
    if htl_item.is_none() {
        eprintln!(
            "htl pin: Cargo.toml has no htl dependency; this is not a project htl new \
             wrote (nothing to move)"
        );
        return Ok(ExitCode::from(2));
    }
    let Some(htl_features) = recognised_features(htl_item) else {
        return Ok(refuse_unrecognised(htl_item, "htl"));
    };
    let old_htl = value_text(htl_item.expect("checked is_none() above"));
    let new_htl = dep_value(&pin.keys_for("htl"), &refs(&htl_features));

    let has_mq = cargo_doc
        .get("dependencies")
        .and_then(|i| i.as_table_like())
        .map(|d| d.contains_key("htl-mq"))
        .unwrap_or(false);
    let mq_item = if has_mq {
        cargo_doc
            .get("dependencies")
            .and_then(|i| i.as_table_like())
            .and_then(|d| d.get("htl-mq"))
    } else {
        None
    };
    let mq_features = if has_mq {
        match recognised_features(mq_item) {
            Some(f) => Some(f),
            None => return Ok(refuse_unrecognised(mq_item, "htl-mq")),
        }
    } else {
        None
    };
    let old_mq = mq_item.map(value_text);
    let new_mq = mq_features
        .as_ref()
        .map(|f| dep_value(&pin.keys_for("htl-mq"), &refs(f)));

    // `mise.toml`: what it should say under this pin, computed without writing it. A
    // directory (or anything else that is not a plain file) where it is expected is
    // refused here, before `Cargo.toml` is touched, rather than discovered midway
    // through the write phase below.
    let mise_path = root.join("mise.toml");
    if mise_path.exists() && !mise_path.is_file() {
        eprintln!(
            "htl pin: {} exists but is not a regular file; nothing was changed",
            mise_path.display()
        );
        return Ok(ExitCode::from(2));
    }
    let mise_existed = mise_path.is_file();
    let mise_new_body = match &pin {
        HtlPin::Release(_) => {
            Some(t_mise(&pin).expect("a Release pin always has a mise.toml body"))
        }
        HtlPin::Main | HtlPin::Path(_) => None,
    };
    let mise_before = read_if_present(&mise_path)?;
    let mise_unchanged = match &mise_new_body {
        Some(body) => mise_before.as_deref() == Some(body.as_str()),
        None => !mise_existed,
    };

    // `[toolchain] htl` in `htl.toml`, only when the project wrote one: parsed now, so a
    // syntax error in it — or the file simply being unreadable, `chmod 000` and the like,
    // which `read_if_present` does not conflate with "there is no such file" — is caught
    // here, before `Cargo.toml` or `mise.toml` are touched, rather than after they are
    // already rewritten.
    let htl_toml_path = root.join("htl.toml");
    let htl_toml_before = read_if_present(&htl_toml_path)?;
    let mut htl_toml_doc = htl_toml_before
        .as_deref()
        .map(|t| {
            t.parse::<DocumentMut>()
                .with_context(|| format!("parsing {}", htl_toml_path.display()))
        })
        .transpose()?;
    let had_toolchain_key = htl_toml_doc
        .as_ref()
        .and_then(|d| d.get("toolchain"))
        .and_then(|i| i.as_table_like())
        .map(|t| t.contains_key("htl"))
        .unwrap_or(false);
    let mut toolchain_report = "unchanged".to_string();
    if had_toolchain_key {
        let doc = htl_toml_doc
            .as_mut()
            .expect("had_toolchain_key implies a parsed doc");
        let tc = doc
            .get_mut("toolchain")
            .and_then(|i| i.as_table_like_mut())
            .expect("had_toolchain_key just confirmed [toolchain] is a table");
        match &pin {
            HtlPin::Release(v) => {
                set_value_keeping_decor(tc, "htl", Value::from(v.clone())).with_context(|| {
                    format!(
                        "{}'s [toolchain] htl is not a plain value",
                        htl_toml_path.display()
                    )
                })?;
                toolchain_report = format!("[toolchain] htl = \"{v}\"");
            }
            HtlPin::Main | HtlPin::Path(_) => {
                tc.remove("htl");
                if tc.is_empty() {
                    doc.as_table_mut().remove("toolchain");
                }
                toolchain_report = "key removed".to_string();
            }
        }
        if doc.to_string()
            == *htl_toml_before
                .as_ref()
                .expect("had_toolchain_key implies text")
        {
            toolchain_report = "unchanged".to_string();
        }
    }

    // --- Everything above is computed and nothing has failed: now write, reporting
    // each file right after its own write succeeds — see the module doc's last
    // paragraph for why. ---

    let mut cargo_changed = false;
    if new_htl != old_htl {
        let deps = cargo_doc
            .get_mut("dependencies")
            .and_then(|i| i.as_table_like_mut())
            .context("no [dependencies] table in Cargo.toml")?;
        set_value_keeping_decor(
            deps,
            "htl",
            new_htl.parse().with_context(|| {
                format!("parsing the value htl pin computed for htl: {new_htl}")
            })?,
        )?;
        cargo_changed = true;
    }
    if let (Some(old), Some(new)) = (&old_mq, &new_mq)
        && old != new
    {
        let deps = cargo_doc
            .get_mut("dependencies")
            .and_then(|i| i.as_table_like_mut())
            .context("no [dependencies] table in Cargo.toml")?;
        set_value_keeping_decor(
            deps,
            "htl-mq",
            new.parse()
                .with_context(|| format!("parsing the value htl pin computed for htl-mq: {new}"))?,
        )?;
        cargo_changed = true;
    }
    if cargo_changed {
        fs::write(&manifest_path, cargo_doc.to_string())
            .with_context(|| format!("writing {}", manifest_path.display()))?;
    }
    let mut cargo_line = format!("htl pin: Cargo.toml  htl: {old_htl} -> {new_htl}");
    if let Some(new_mq) = &new_mq {
        cargo_line.push_str(&format!(
            "  htl-mq: {} -> {new_mq}",
            old_mq.as_deref().unwrap_or("")
        ));
    }
    eprintln!("{cargo_line}");

    let mise_report = if mise_unchanged {
        "unchanged"
    } else {
        match &mise_new_body {
            Some(body) => {
                fs::write(&mise_path, body)
                    .with_context(|| format!("writing {}", mise_path.display()))?;
                "written"
            }
            None => {
                fs::remove_file(&mise_path)
                    .with_context(|| format!("removing {}", mise_path.display()))?;
                "removed"
            }
        }
    };
    eprintln!("htl pin: mise.toml: {mise_report}");

    if toolchain_report != "unchanged" {
        let after = htl_toml_doc
            .as_ref()
            .expect("toolchain_report changed implies a parsed doc")
            .to_string();
        fs::write(&htl_toml_path, &after)
            .with_context(|| format!("writing {}", htl_toml_path.display()))?;
    }
    eprintln!("htl pin: htl.toml: {toolchain_report}");

    // `cargo update` is what makes `Cargo.lock` agree with the manifest above — the one
    // step with no local answer, so `--no-update` prints it instead of running it. Every
    // file above is already written and reported by the time this runs, so a failure
    // here still has those lines above it.
    let mut names = vec!["htl", "htl-core", "htl-macros"];
    if has_mq {
        names.push("htl-mq");
    }
    if no_update {
        let flags = names
            .iter()
            .map(|p| format!("-p {p}"))
            .collect::<Vec<_>>()
            .join(" ");
        eprintln!("htl pin: --no-update; run `cargo update {flags}` to update Cargo.lock");
        return Ok(ExitCode::SUCCESS);
    }

    let mut cmd = Command::new(cargo());
    cmd.arg("update");
    for p in &names {
        cmd.args(["-p", p]);
    }
    cmd.current_dir(&root);
    let status = cmd
        .status()
        .with_context(|| format!("running `cargo update` in {}", root.display()))?;
    if !status.success() {
        eprintln!(
            "htl pin: cargo update failed (exit {}); the files above are rewritten — fix \
             the cause and run the command again",
            status.code().unwrap_or(-1)
        );
        return Ok(ExitCode::from(2));
    }

    Ok(ExitCode::SUCCESS)
}

/// `p`, canonicalised against this process's working directory (an already-absolute `p`
/// is canonicalised as given — resolving `..` and symlinks, nothing more). `None` when
/// the result does not have `crates/htl/Cargo.toml` under it — including when `p` does
/// not exist at all, which `canonicalize` itself refuses.
fn canonical_checkout(p: &Path) -> Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    let Ok(canonical) = fs::canonicalize(&absolute) else {
        return Ok(None);
    };
    if canonical
        .join("crates")
        .join("htl")
        .join("Cargo.toml")
        .is_file()
    {
        Ok(Some(canonical))
    } else {
        Ok(None)
    }
}

/// `fs::read_to_string`, but a missing file is `Ok(None)` rather than an error — the one
/// read in this module where "this project never wrote one" (proceed, untouched) and
/// "the file is there and something else is wrong with it" (a permission bit, a
/// directory where a file belongs) must not be treated alike. Only
/// [`std::io::ErrorKind::NotFound`] is "no file"; everything else propagates with the
/// path attached, before this command has written anything.
fn read_if_present(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Overwrite `table[key]`'s value with `new`, keeping the entry's own decor — a comment
/// on the line above it, anything after it on the same line — rather than the fresh
/// default spacing a brand new `Item` carries. `TableLike::insert` does not do this: it
/// reformats the key afresh and drops the old `Item`, decor included, outright.
fn set_value_keeping_decor(
    table: &mut dyn toml_edit::TableLike,
    key: &str,
    new: Value,
) -> Result<()> {
    let slot = table
        .get_mut(key)
        .and_then(|i| i.as_value_mut())
        .with_context(|| format!("{key} is not a plain value"))?;
    let decor = slot.decor().clone();
    let mut new = new;
    *new.decor_mut() = decor;
    *slot = new;
    Ok(())
}

/// The `cargo` to run: the one running this command when cargo is what started it, the
/// same rule [`htl::dep_dts`]'s own `cargo()` follows for `cargo metadata` — a toolchain
/// selected for the build is the toolchain the lock is resolved with.
fn cargo() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// The shape the scaffold writes for a dependency's value: a bare version string, or an
/// inline table naming exactly one source (`version`; or `git` + `branch`; or `path`) and
/// optionally `features`. `None` for anything else — a `[dependencies.<name>]` table,
/// `workspace = true`, `optional`, `rev`, `tag`, a `default-features` key — because
/// reading one of those and writing back only the part this understands is how a
/// hand-tuned line gets silently discarded.
fn recognised_features(item: Option<&toml_edit::Item>) -> Option<Vec<String>> {
    match item?.as_value()? {
        Value::String(_) => Some(Vec::new()),
        Value::InlineTable(t) => {
            let mut keys: Vec<&str> = t
                .iter()
                .map(|(k, _)| k)
                .filter(|k| *k != "features")
                .collect();
            keys.sort_unstable();
            if !matches!(keys.as_slice(), ["version"] | ["branch", "git"] | ["path"]) {
                return None;
            }
            match t.get("features") {
                None => Some(Vec::new()),
                Some(Value::Array(arr)) => {
                    let mut feats = Vec::with_capacity(arr.len());
                    for v in arr.iter() {
                        feats.push(v.as_str()?.to_string());
                    }
                    Some(feats)
                }
                Some(_) => None,
            }
        }
        _ => None,
    }
}

/// The dependency's current value, exactly as [`dep_value`] would spell it: the item's
/// own [`Value`], decor cleared, rendered. Read from the parsed item rather than the raw
/// file text — see the module doc, "Why the old value is read from the parsed item" —
/// so a key or a value spaced differently from the scaffold's own style is read
/// correctly instead of panicking or being confused with an unrelated line.
fn value_text(item: &toml_edit::Item) -> String {
    let mut v = item
        .as_value()
        .expect("recognised_features(Some(_)) returned Some only for a Value")
        .clone();
    v.decor_mut().clear();
    v.to_string()
}

/// What to call the dependency in a refusal: the item's own spelling (decor cleared, so a
/// trailing comment does not leak into the message) when it is a value, a
/// `[dependencies.<name>]` stand-in when it is a table, and a note when it is absent —
/// read from the parsed document rather than the raw file text, for the same reason
/// [`value_text`] is. The caller handles an absent `htl` itself with a different
/// message (see the module doc); the `None` branch here is only ever reached for
/// `htl-mq`, which has no dependency of its own to be absent from if `has_mq` said it
/// was there.
fn refuse_unrecognised(item: Option<&toml_edit::Item>, name: &str) -> ExitCode {
    let found = match item {
        None => format!("no {name} dependency in [dependencies]"),
        Some(toml_edit::Item::Value(v)) => {
            let mut v = v.clone();
            v.decor_mut().clear();
            format!("{name} = {v}")
        }
        Some(_) => format!("[dependencies.{name}]"),
    };
    eprintln!(
        "htl pin: Cargo.toml's {name} line is not one the scaffold wrote ({found}); write the \
         patch by hand:\n\n{PATCH_BLOCK}\n\
         All three, not one; then `cargo update -p htl -p htl-core -p htl-macros` once, and \
         the same command again with the block deleted to go back."
    );
    ExitCode::from(2)
}

fn refs(features: &[String]) -> Vec<&str> {
    features.iter().map(String::as_str).collect()
}
