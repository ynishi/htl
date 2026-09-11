//! `htl new` / `htl init`: project scaffolding.
//!
//! Layout (chosen so the same tree works for `htl run` / `htl test` locally *and*
//! as an mlua-pkg dependency for consumers):
//!
//! ```text
//! <name>/
//! ├── mlua-pkg.toml          [package] entry = "src/<mod>"  -> consumers require("<name>")
//! ├── src/<mod>/init.tl      the module (require("<mod>") from src/ and from tests/)
//! ├── src/main.tl            entry script            (omitted with --lib)
//! ├── tests/<mod>_test.tl    htl.test sample
//! ├── .gitignore
//! ├── README.md
//! └── Cargo.toml + src/lib.rs    Rust host (only with a target): #[host_module] exposing
//!     + src/main.rs               `host` to Teal (declaration -> src/host.d.tl), the
//!                                 module embedded with include_tl_bytes!, and a thin
//!                                 binary on top when there is an entry script
//! ```
//!
//! A target may add to that: the `cdylib` one writes `examples/c/` and `examples/python/`
//! beside the library, because a C ABI whose reference caller nobody wrote is a C ABI
//! every caller gets wrong in the same three ways.
//!
//! # The target is a [`BuildTarget`]; the profile is what the scaffold writes for it
//!
//! Everything above the `Cargo.toml` line is the same for every project. What varies is
//! the target — what will run this project's output, and therefore what Rust it needs —
//! and that is one value rather than a growing set of booleans. The value is
//! [`BuildTarget`], which lives in `htl-core` because `htl.toml` records it as
//! `[build] target`; what it means for the *crate* — its `[lib] crate-type`, whether it
//! takes an entry script — is derived there, from the target, rather than restated here.
//!
//! What is left for this module is the scaffold's own half: a [`TargetProfile`] says what
//! the target depends on, which Rust files it writes, which Teal sample it starts the
//! project from, and what its README has to say. [`PROFILES`] is the registry of the
//! targets that scaffold, `--target <name>` picks an entry from it, and a new kind of
//! target is one more [`BuildTarget`] arm and one more entry rather than another flag
//! threaded through every template.
//!
//! # Every target with Rust in it is a library crate with a thin binary on top
//!
//! Each target that writes Rust writes `src/lib.rs`: the `#[host_module]`, its records,
//! the embedded Teal module, `preload` registering both, and a Rust test that goes through
//! `preload`. When the project has an entry script it also writes `src/main.rs`, a few
//! lines that call `preload` and `exec` the script. So what a project grows — a second
//! host module, a C ABI layer, a window loop — grows in the library, and the binary never
//! holds logic. The `cdylib` target is that taken to its end: a `#[c_export]` block in the
//! same library and no binary at all, which is why it is the target that answers
//! [`Script::Forbids`](htl::build_target::Script::Forbids).
//!
//! [`scaffold`] therefore does the same three things whatever it is asked for: pick the
//! target, turn the profile into a list of paths and bodies, write the ones that do not
//! exist yet. No template branches on `--embed`; the ones that differ between targets are
//! separate templates, chosen by the profile.
//!
//! Bodies where doubling every brace for `format!` cost more than it was worth — the Rust
//! host, the Teal sample, a C caller — live under `crates/htl-cli/templates/` and are
//! read with `include_str!`, filled by replacing `{{name}}` / `{{mod}}` / `{{MOD}}` /
//! `{{htl}}`. They stay in this crate rather than being fetched, so `htl_dep_version()`
//! keeps pinning a scaffold to the htl release that wrote it. Short TOML and Markdown
//! stay inline.

use anyhow::{Context, Result, anyhow, bail};
use htl::BuildTarget;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub struct Options {
    pub lib: bool,
    /// The target to write for, already resolved against [`PROFILES`] by
    /// [`resolve_target`], so nothing downstream can be asked for a target that does not
    /// exist or does not fit `--lib`.
    pub target: Option<&'static TargetProfile>,
}

/// What a template is filled with: the package name and its Teal identifier. The
/// templates themselves branch on nothing — the profile decides which body is written in
/// the first place — but the README is one body for every project, and what it tells the
/// reader to run does depend on whether there is an entry script, so that answer travels
/// with the names.
pub struct Ctx<'a> {
    pub name: &'a str,
    pub module: &'a str,
    /// Is there a `src/main.tl` to run? The inverse of `--lib`, already reconciled with
    /// the target by [`resolve_target`].
    pub script: bool,
}

/// A dependency line in the project's `Cargo.toml`: what to require, and the features the
/// target turns on.
pub struct DepLine {
    pub name: &'static str,
    pub req: Dep,
    /// Empty writes the short `name = "req"` form; anything else writes the table.
    pub features: &'static [&'static str],
}

impl DepLine {
    /// The common case: a dependency with no features of its own.
    const fn plain(name: &'static str, req: Dep) -> Self {
        DepLine {
            name,
            req,
            features: &[],
        }
    }
}

/// What a dependency line requires.
pub enum Dep {
    /// This CLI's own release, derived at run time (see [`htl_dep_version`]) so a scaffold
    /// follows the htl that produced it.
    Htl,
    /// A literal requirement.
    Version(&'static str),
}

/// One file a target's scaffold writes, relative to the project root. Mostly Rust —
/// `src/lib.rs`, `src/main.rs` — but a target that ships reference callers writes their C,
/// Python and build glue the same way, as a path and a body.
pub struct ScaffoldFile {
    pub path: &'static str,
    pub body: fn(&Ctx<'_>) -> String,
}

/// The Teal sample a project starts from. A target that dictates a different shape (a
/// `Game` record for a window loop, an entry script that requires `host`) points these at
/// its own templates; everything else uses [`DEFAULT_TEAL`].
pub struct TealSample {
    pub module: fn(&Ctx<'_>) -> String,
    pub test: fn(&Ctx<'_>) -> String,
    /// `src/main.tl`, written only when the project has an entry script.
    pub main: fn(&Ctx<'_>) -> String,
}

/// The per-target scaffold data: what the scaffold writes for one [`BuildTarget`].
///
/// What a build target *is* — and what follows from it, the crate types and the
/// entry-script rule — is defined on [`BuildTarget`], not here. This is the other half:
/// the files, the dependencies and the prose that only `htl new` has an opinion about.
pub struct TargetProfile {
    /// Which build target this scaffolds for. Its name is the name in the registry and on
    /// the command line (`--target <name>`), and its `crate_types` / `entry` are what the
    /// `Cargo.toml` and the `--lib` reconciliation below read.
    pub target: BuildTarget,
    pub deps: &'static [DepLine],
    /// `src/lib.rs`: the host module, the embedded Teal, `preload`. Always written.
    pub lib: ScaffoldFile,
    /// `src/main.rs`: the thin entry, written only when there is an entry script.
    pub main: Option<ScaffoldFile>,
    /// Anything else the target ships: `examples/`, `include/`, a header.
    pub extra: &'static [ScaffoldFile],
    /// What this target builds that is not worth committing, as `.gitignore` lines
    /// (filled like a template, so a path may name the module). What it *generates* and
    /// commits — `src/host.d.tl`, the C header — is deliberately not here.
    pub ignore: &'static [&'static str],
    pub teal: TealSample,
    /// The lines this target adds to the README's command block, and the paragraphs that
    /// follow it. The README is the one body every project has and every target has
    /// something different to say in, so it is written here rather than branched on the
    /// target's name where the file is assembled.
    pub readme_commands: fn(&Ctx<'_>) -> String,
    pub readme_prose: fn(&Ctx<'_>) -> String,
}

/// The default target: the OS runs the output as a binary. A library crate holding the
/// host module and the embedded scripts, with a thin binary on top when the project has an
/// entry script.
const BIN: TargetProfile = TargetProfile {
    target: BuildTarget::Bin,
    deps: &[
        DepLine::plain("htl", Dep::Htl),
        DepLine::plain("anyhow", Dep::Version("1")),
    ],
    lib: ScaffoldFile {
        path: "src/lib.rs",
        body: rust_lib_rs,
    },
    main: Some(ScaffoldFile {
        path: "src/main.rs",
        body: rust_main_rs,
    }),
    extra: &[],
    ignore: &["/target"],
    teal: TealSample {
        module: teal_module,
        test: teal_test,
        // The entry script talks to the Rust side, so it is the host's, not the default.
        main: rust_main_tl,
    },
    readme_commands: rust_readme_commands,
    readme_prose: rust_readme_prose,
};

/// The C ABI target: a caller that is not written in Rust runs the output. The same
/// library, plus `#[c_export]` and the two reference callers.
///
/// It is a library and nothing else — a `cdylib` has no entry point of its own, and the
/// caller that loads it brings its own `main` — so the
/// [`Script::Forbids`](htl::build_target::Script::Forbids) that
/// [`BuildTarget::Cdylib`] answers refuses `--target cdylib` without `--lib` rather than
/// writing a `src/main.rs` nothing would run. The `staticlib` alongside is what Unity on
/// iOS links; it costs a second artefact and nothing else.
const CDYLIB: TargetProfile = TargetProfile {
    target: BuildTarget::Cdylib,
    deps: &[
        DepLine {
            name: "htl",
            req: Dep::Htl,
            // The C ABI runtime the generated wrappers call, and `#[c_export]` itself.
            features: &["ffi"],
        },
        DepLine::plain("anyhow", Dep::Version("1")),
        DepLine {
            name: "serde",
            req: Dep::Version("1"),
            // The options come in as JSON and the records go out as JSON.
            features: &["derive"],
        },
    ],
    lib: ScaffoldFile {
        path: "src/lib.rs",
        body: ffi_lib_rs,
    },
    main: None,
    extra: &[
        ScaffoldFile {
            path: "examples/c/main.c",
            body: ffi_example_c,
        },
        ScaffoldFile {
            path: "examples/c/Makefile",
            body: ffi_example_makefile,
        },
        ScaffoldFile {
            path: "examples/python/run.py",
            body: ffi_example_py,
        },
    ],
    // The C caller's binary. The header the macro writes is *not* ignored: it is
    // generated and committed, like `src/host.d.tl`, so that reading the ABI does not
    // mean building the crate.
    ignore: &["/target", "examples/c/{{mod}}_c"],
    teal: TealSample {
        module: ffi_teal_module,
        test: ffi_teal_test,
        // Unreachable: `Script::Forbids` means there is never a `src/main.tl` to write.
        // The field is not an `Option` because every other target has one, so this is the
        // default sample, which is what `htl init --target cdylib` on a project that
        // already has a script would keep anyway.
        main: teal_main,
    },
    readme_commands: ffi_readme_commands,
    readme_prose: ffi_readme_prose,
};

/// Every target the scaffold writes Rust for. [`BuildTarget::Hb`] is not among them: it is
/// what plain `htl new` writes, which is the tree without a `Cargo.toml` at all. #104 (a
/// macroquad window) is one more entry.
pub static PROFILES: &[TargetProfile] = &[BIN, CDYLIB];

/// The target `--embed` is shorthand for.
pub const DEFAULT_TARGET: BuildTarget = BuildTarget::Bin;

/// What a project with no target of its own starts from.
static DEFAULT_TEAL: TealSample = TealSample {
    module: teal_module,
    test: teal_test,
    main: teal_main,
};

const T_MANIFEST: &str = include_str!("../templates/mlua-pkg.toml");
const T_TEAL_MODULE: &str = include_str!("../templates/teal/init.tl");
const T_TEAL_TEST: &str = include_str!("../templates/teal/test.tl");
const T_TEAL_MAIN: &str = include_str!("../templates/teal/main.tl");
const T_RUST_MAIN_TL: &str = include_str!("../templates/rust/main.tl");
const T_RUST_LIB_RS: &str = include_str!("../templates/rust/lib.rs");
const T_RUST_MAIN_RS: &str = include_str!("../templates/rust/main.rs");
const T_FFI_LIB_RS: &str = include_str!("../templates/ffi/lib.rs");
const T_FFI_TEAL_MODULE: &str = include_str!("../templates/ffi/init.tl");
const T_FFI_TEAL_TEST: &str = include_str!("../templates/ffi/test.tl");
const T_FFI_MAIN_C: &str = include_str!("../templates/ffi/main.c");
const T_FFI_MAKEFILE: &str = include_str!("../templates/ffi/Makefile");
const T_FFI_RUN_PY: &str = include_str!("../templates/ffi/run.py");

/// Teal identifier for a package name (`my-pkg` -> `my_pkg`).
pub fn module_ident(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// Every target the scaffold has a profile for, in registry order: what `--target`
/// accepts and what a typo is answered with. `hb` is deliberately absent — it is the tree
/// plain `htl new` writes, so naming it as a scaffold would be offering a flag for the
/// default.
pub fn target_names() -> Vec<&'static str> {
    PROFILES.iter().map(|p| p.target.name()).collect()
}

pub fn profile(t: BuildTarget) -> Option<&'static TargetProfile> {
    PROFILES.iter().find(|p| p.target == t)
}

/// Turn `--target` / `--embed` / `--lib` into the target to write for, or into the reason
/// there is none to write. Called before the first file is created, so a refusal leaves
/// the directory as it was.
///
/// `--embed` is the shorthand for `--target bin` and stays one: clap's value parser only
/// applies to `--target`, so the two are reconciled here rather than pretended to be one
/// flag. Giving both is fine when they agree — which is decided on the *name* the user
/// wrote, before it is parsed, so that `--embed --target typo` says which flag to drop
/// rather than which targets exist.
pub fn resolve_target(
    target: Option<&str>,
    embed: bool,
    lib: bool,
) -> Result<Option<&'static TargetProfile>> {
    let t = match (target, embed) {
        (None, false) => return Ok(None),
        (None, true) => DEFAULT_TARGET,
        (Some(n), true) if n != DEFAULT_TARGET.name() => bail!(
            "--embed is the shorthand for --target {DEFAULT_TARGET}, so it cannot be given with --target {n}; drop one of them"
        ),
        (Some(n), _) => BuildTarget::from_str(n).map_err(|e| anyhow!("{e}"))?,
    };
    let Some(p) = profile(t) else {
        bail!(
            "the `{t}` target is what plain `htl new` writes, so there is no scaffold to ask for; targets that scaffold: {}",
            target_names().join(", ")
        );
    };
    if !p.target.entry().accepts(lib) {
        bail!("{}", script_mismatch(p, lib));
    }
    Ok(Some(p))
}

/// Why a target and `--lib` do not fit, and which targets do. Its own function because the
/// message is the whole point of refusing here rather than at the first write.
fn script_mismatch(p: &TargetProfile, lib: bool) -> String {
    let fits: Vec<&str> = PROFILES
        .iter()
        .filter(|c| c.target.entry().accepts(lib))
        .map(|c| c.target.name())
        .collect();
    let fits = if fits.is_empty() {
        "none".to_string()
    } else {
        fits.join(", ")
    };
    if lib {
        // No target answers `Script::Requires` yet — #104's window loop is the one that
        // will — so `resolve_target` cannot reach this half today.
        format!(
            "the `{}` target runs an entry script, which --lib leaves out; targets that work with --lib: {fits}",
            p.target
        )
    } else {
        format!(
            "the `{}` target writes no entry script, so it needs --lib; targets that write one: {fits}",
            p.target
        )
    }
}

/// Everything the scaffold would write, in the order it is reported, before anything
/// touches the disk.
fn plan(dir: &Path, name: &str, module: &str, opts: &Options) -> Vec<(PathBuf, String)> {
    let ctx = Ctx {
        name,
        module,
        script: !opts.lib,
    };
    let target = opts.target;
    let teal = target.map_or(&DEFAULT_TEAL, |t| &t.teal);

    let mut files = vec![
        (dir.join("mlua-pkg.toml"), fill(T_MANIFEST, &ctx)),
        (dir.join("htl.toml"), t_htl_toml()),
        (dir.join("types").join("README.md"), t_types_readme()),
        (
            dir.join("src").join(module).join("init.tl"),
            (teal.module)(&ctx),
        ),
        (
            dir.join("tests").join(format!("{module}_test.tl")),
            (teal.test)(&ctx),
        ),
        (dir.join(".gitignore"), t_gitignore(target, &ctx)),
        (dir.join("README.md"), t_readme(&ctx, target)),
    ];
    // `--lib` is what "no entry script" means, and a target that disagrees with it was
    // already refused, so the question is answered here for every project alike.
    if !opts.lib {
        files.push((dir.join("src").join("main.tl"), (teal.main)(&ctx)));
    }
    if let Some(t) = target {
        files.push((dir.join("Cargo.toml"), t_cargo(name, t)));
        files.push((dir.join(t.lib.path), (t.lib.body)(&ctx)));
        // The binary exists to run the entry script, so without one it is not written —
        // and a target that has no binary to write says so with `main: None`.
        if !opts.lib
            && let Some(m) = &t.main
        {
            files.push((dir.join(m.path), (m.body)(&ctx)));
        }
        for f in t.extra {
            files.push((dir.join(f.path), (f.body)(&ctx)));
        }
    }
    files
}

/// What a scaffold run did: the files it created, and the ones it left alone because they
/// were already there. `htl init` reports both, so asking an existing project for a target
/// says which of that target's files it did not touch rather than silently skipping them.
pub struct Scaffolded {
    pub written: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
}

/// Write every template file that does not exist yet.
/// With `must_be_new`, the directory must not exist (or be empty).
pub fn scaffold(dir: &Path, name: &str, opts: &Options, must_be_new: bool) -> Result<Scaffolded> {
    if must_be_new && dir.exists() && dir.read_dir()?.next().is_some() {
        bail!(
            "{} already exists and is not empty (use `htl init` to fill in a directory)",
            dir.display()
        );
    }
    let m = module_ident(name);

    let mut out = Scaffolded {
        written: Vec::new(),
        kept: Vec::new(),
    };
    for (path, text) in plan(dir, name, &m, opts) {
        if path.exists() {
            out.kept.push(path);
            continue;
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        out.written.push(path);
    }
    Ok(out)
}

/// The four placeholders a template file may use. Plain `str::replace`: the bodies are
/// ours, so there is nothing to escape and no engine to depend on. `{{MOD}}` is the
/// module identifier upper-cased, which is how a generated C header spells its own
/// constants (`{{MOD}}_OK`), and therefore how a caller written in C has to spell them.
fn fill(template: &str, ctx: &Ctx<'_>) -> String {
    template
        .replace("{{name}}", ctx.name)
        .replace("{{mod}}", ctx.module)
        .replace("{{MOD}}", &ctx.module.to_uppercase())
        .replace("{{htl}}", &htl_dep_version())
}

fn teal_module(ctx: &Ctx<'_>) -> String {
    fill(T_TEAL_MODULE, ctx)
}

fn teal_test(ctx: &Ctx<'_>) -> String {
    fill(T_TEAL_TEST, ctx)
}

fn teal_main(ctx: &Ctx<'_>) -> String {
    fill(T_TEAL_MAIN, ctx)
}

fn rust_main_tl(ctx: &Ctx<'_>) -> String {
    fill(T_RUST_MAIN_TL, ctx)
}

fn rust_lib_rs(ctx: &Ctx<'_>) -> String {
    fill(T_RUST_LIB_RS, ctx)
}

fn rust_main_rs(ctx: &Ctx<'_>) -> String {
    fill(T_RUST_MAIN_RS, ctx)
}

fn ffi_lib_rs(ctx: &Ctx<'_>) -> String {
    fill(T_FFI_LIB_RS, ctx)
}

fn ffi_teal_module(ctx: &Ctx<'_>) -> String {
    fill(T_FFI_TEAL_MODULE, ctx)
}

fn ffi_teal_test(ctx: &Ctx<'_>) -> String {
    fill(T_FFI_TEAL_TEST, ctx)
}

fn ffi_example_c(ctx: &Ctx<'_>) -> String {
    fill(T_FFI_MAIN_C, ctx)
}

fn ffi_example_makefile(ctx: &Ctx<'_>) -> String {
    fill(T_FFI_MAKEFILE, ctx)
}

fn ffi_example_py(ctx: &Ctx<'_>) -> String {
    fill(T_FFI_RUN_PY, ctx)
}

fn t_types_readme() -> String {
    "# types/\n\n\
     Hand-written `.d.tl` declarations for modules the host provides at run time and\n\
     that ship no declaration of their own (a Rust crate re-exported to Lua, a runtime\n\
     SDK): `xlib.d.tl` here makes `require(\"xlib\")` typed in `htl check`, `htl test` and\n\
     `include_tl!`. Consulted after the project root and `src/`, before `[check] paths`;\n\
     a `.tl` source anywhere on the path beats a declaration, so nothing here can shadow\n\
     an implementation, and a second declaration of the same module is reported\n\
     (`duplicate-declaration`) rather than silently losing to one of them.\n\n\
     Files htl writes here are the ones the project *publishes*: the module a\n\
     `---@contract` type is declared in, for the authors of the modules that contract\n\
     holds. Declarations generated from this crate's own Rust (`#[host_module]`) are\n\
     written next to the scripts, not here. Both are committed.\n\n\
     So is `<crate>/`, when there is one: a dependency that names its declarations in\n\
     `[package.metadata.htl] dts` has them copied there by `htl dts` (and by check / run\n\
     / test). Edit the crate, not the copy — the next run writes it again.\n"
        .to_string()
}

/// The project's `htl.toml`.
///
/// It may only name keys the htl this scaffold *pins* can read. A host project depends on
/// the released crate ([`htl_dep_version`]), and `HtlConfig` is `deny_unknown_fields`, so a
/// key this workspace added but no release carries yet fails the project's first
/// `cargo build` inside `include_tl!` rather than being ignored. So a key crosses in two
/// steps: it lands in `htl-core` and is published, and only the release *after* that may
/// write it into a scaffold. `[toolchain]` is out for that reason, and it is the key this
/// was learned on — it was written a release early, and every project `htl new` wrote in
/// between failed to build.
///
/// `[lint.rules]` is out for the same reason, and a commented example of it would be too:
/// a comment is one user action away from being a key, and the user who uncomments it is
/// building against the pinned release. Nor may the `enable` / `disable` it replaced be
/// shown, since the CLI that just wrote the file refuses those. So the section names
/// neither and sends the reader to `htl check --list-lints`, which answers from the binary
/// they have.
///
/// `[build] target` landed in `htl-core` in this change and is read by every command that
/// loads this file, but it is not written here for exactly the reason above: the scaffold
/// pins the released `htl`, which does not know the key. Writing it is the release after
/// the one that publishes it.
fn t_htl_toml() -> String {
    "# htl project settings (htl check / htl test / htl fmt / include_tl! all read this).\n\
     # Command-line flags and HTL_LINTS / HTL_LINT override it.\n\n\
     [lint]\n\
     # strict = true   # warnings and lints fail htl check (not htl test); lints fail\n\
     #                   include_tl!; false makes the macro advisory\n\n\
     # Per rule: htl check --list-lints names every rule with the level it has by\n\
     # default, and the README's \"Lints\" section says how to change one.\n\n\
     [fmt]\n\
     indent = 3\n\n\
     [check]\n\
     # paths = [\"mods\", \"~/.cache/sdk\"]   # extra dirs require() resolves from while checking\n\
     # (src/ and types/ are always searched; hand-written .d.tl go under types/)\n\n\
     # Where this project accepts modules written outside it:\n\
     # [[contract]]\n\
     # dir = \"mods\"             # relative to this file; \"sites/*\" = each subdirectory\n\
     # module = \"Site\"          # optional: only this module name in each dir\n\
     #\n\
     # The shape those modules must have is declared on the record itself, so the two\n\
     # cannot drift apart:\n\
     #\n\
     #   local record defs\n\
     #      record Mod              ---@contract     -- inherits `dir` above;\n\
     #         name: string         ---@required     -- ---@contract(\"other\") overrides it\n\
     #         monsters: {Monster}  ---@required\n\
     #         factions: {Faction}                   -- unmarked: for the mods that want it\n\
     #      end\n\
     #   end\n\
     #\n\
     # The host must enforce it too: htl::pkg::contract_resolvers(root, &config).\n\
     # `htl check` reports `contract-unenforced` when that call is not in the Rust sources.\n\
     # enforced_by = \"mods/_validate.lua\"   # ...or name where it is enforced instead,\n\
     #                          # for a Lua-side validator, a sibling crate, generated\n\
     #                          # code. The file has to exist; a missing one is reported.\n"
        .to_string()
}

fn t_gitignore(target: Option<&'static TargetProfile>, ctx: &Ctx<'_>) -> String {
    // `.htl/` holds the run cache and the installed deps: generated, machine-local, and
    // keyed on absolute paths, so it is never worth sharing. One line covers both.
    let mut s = String::from(".htl/\n*.hb\n");
    for line in target.into_iter().flat_map(|t| t.ignore) {
        s.push_str(&fill(line, ctx));
        s.push('\n');
    }
    s
}

/// The project README: the part every project has, with the target's own lines spliced
/// into the command block and its own paragraphs after it.
fn t_readme(ctx: &Ctx<'_>, target: Option<&'static TargetProfile>) -> String {
    let (name, m) = (ctx.name, ctx.module);
    let mut s = format!(
        "# {name}\n\nTeal project managed with [htl](https://github.com/ynishi/htl).\n\n```sh\nhtl check .            # type-check + lints\n"
    );
    if ctx.script && target.is_none() {
        s.push_str("htl run src/main.tl    # run the entry script\n");
    }
    s.push_str("htl test               # tests/*_test.tl via htl.test\nhtl fmt .              # whitespace formatter\nhtl pkg install        # fetch [deps] from mlua-pkg.toml\n");
    if let Some(t) = target {
        s.push_str(&(t.readme_commands)(ctx));
    }
    s.push_str(&format!(
        "```\n\nModule: `src/{m}/init.tl` (`require(\"{m}\")` from `src/` and `tests/`).\n\n\
         `mlua-pkg.toml` `entry = \"src/{m}\"` only matters to *consumers* that depend on this\n\
         package through mlua-pkg: they get it as `require(\"{name}\")`. "
    ));
    match target {
        Some(t) => s.push_str(&(t.readme_prose)(ctx)),
        None => s.push_str("Ignore it if nobody depends on this package.\n"),
    }
    s
}

fn rust_readme_commands(ctx: &Ctx<'_>) -> String {
    let mut s = String::new();
    if ctx.script {
        s.push_str("cargo run              # the binary: preload, then src/main.tl (type-checked at build)\n");
        s.push_str("                       # (src/main.tl requires the Rust `host`, so `htl run` cannot run it)\n");
    }
    s.push_str(
        "cargo test             # the library's Rust test: the module loaded through preload\n",
    );
    s
}

fn rust_readme_prose(ctx: &Ctx<'_>) -> String {
    let mut s = String::from(
        "The Rust host is a library:\n`src/lib.rs` holds the `#[host_module]`, embeds this module, and registers both in\n\
         `preload(&Htl)`. ",
    );
    if ctx.script {
        s.push_str(
            "`src/main.rs` is a few lines on top of it — `preload`, then the entry\nscript. Grow the library, not the binary.\n\n",
        );
    } else {
        s.push_str(
            "There is no binary: call `preload` from whatever embeds this\ncrate, and grow the library.\n\n",
        );
    }
    s.push_str(HOST_DTL);
    s
}

/// The generated-declaration paragraph, which every target that writes a `#[host_module]`
/// says the same way.
const HOST_DTL: &str = "`src/host.d.tl` is generated from `#[host_module]` in `src/lib.rs`: `cargo build` writes it,\n\
     and so does `htl dts` / `htl check` without building, so the Teal side always sees the\n\
     current Rust signatures.\n";

fn ffi_readme_commands(ctx: &Ctx<'_>) -> String {
    format!(
        "cargo test             # the library's Rust tests: preload, and the generated header\n\
         cargo build            # the library, and include/{}.h from #[c_export]\n\
         make -C examples/c run          # the C caller   (after cargo build)\n\
         python3 examples/python/run.py  # the Python caller, the same round trip\n",
        ctx.module
    )
}

fn ffi_readme_prose(ctx: &Ctx<'_>) -> String {
    let m = ctx.module;
    let mut s = String::from(
        "This project is a library with two boundaries:\n\
         `src/lib.rs` holds the `#[host_module]` the *scripts* call and the `#[c_export]` block a\n\
         *caller that is not written in Rust* calls. There is no binary — a C ABI library has no\n\
         entry point of its own, which is why `--target cdylib` implies `--lib`.\n\n",
    );
    s.push_str(HOST_DTL);
    s.push_str(&format!(
        "\n## The C ABI\n\n\
         `cargo build` writes `include/{m}.h` from the same `impl` block, and leaves\n\
         `target/debug/lib{m}.{{so,dylib,dll}}` for a caller to load and `lib{m}.a` to link\n\
         statically (Unity on iOS wants the latter). Commit the header: it is generated, and so is\n\
         `src/host.d.tl`, and both are what someone reads without building this crate.\n\n\
         Everything that crosses is an opaque `{m}_handle *`, a `char *` this library allocated,\n\
         or an `int`. Three rules cover the whole ABI:\n\n\
         - **Every `char *` returned is yours to free**, with `{m}_free()` and nothing else. The\n\
           call that produced it is not finished until you do.\n\
         - **An `int` is a status, never a value** (`{}_OK` and friends, in the header). A call\n\
           that has both writes the value through an `int *out`.\n\
         - **One handle, one thread.** Using it from another answers `WRONG_THREAD`; the exception\n\
           is `{m}_interrupt()`, which any thread may call to stop a runaway script.\n\n\
         A failed call answers `NULL` or a status, and `{m}_last_error()` is the message —\n\
         a pointer valid until the next call *on that thread*, so copy it (or use\n\
         `{m}_last_error_into()`) rather than keeping it.\n\n\
         `examples/c/` and `examples/python/` are two callers doing the same round trip: open,\n\
         a text call, a JSON call, the two error paths, `free`, close. Each is written the way its\n\
         own language gets this wrong by default — see the comment at the top of both.\n",
        m.to_uppercase()
    ));
    s
}

/// The `htl` requirement a scaffolded project declares: this CLI's own release, in the
/// shortest form cargo's caret rule reads as "this line and its compatible updates" —
/// `"0.2"` for 0.2.x (`>=0.2.0, <0.3.0`), `"1"` once there is a 1.x (`>=1.0.0, <2.0.0`).
/// Derived, not written into the template, so the scaffold follows the release that
/// produced it and nobody has to remember the template at bump time.
pub fn htl_dep_version() -> String {
    htl_dep_version_of(env!("CARGO_PKG_VERSION"))
}

fn htl_dep_version_of(version: &str) -> String {
    let mut parts = version.split('.');
    let major = parts.next().unwrap_or("0");
    if major != "0" {
        return major.to_string();
    }
    format!("0.{}", parts.next().unwrap_or("0"))
}

/// The project's `Cargo.toml`, assembled from the profile: crate shape, then dependencies,
/// then the one profile setting every target with Rust in it needs.
fn t_cargo(name: &str, target: &TargetProfile) -> String {
    let mut s = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n# authors / license / repository: fill in yourself\n\n"
    );
    if !target.target.crate_types().is_empty() {
        let types: Vec<String> = target
            .target
            .crate_types()
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect();
        s.push_str(&format!("[lib]\ncrate-type = [{}]\n\n", types.join(", ")));
    }
    s.push_str("[dependencies]\n");
    for d in target.deps {
        let req = match d.req {
            Dep::Htl => htl_dep_version(),
            Dep::Version(v) => v.to_string(),
        };
        let name = d.name;
        if d.features.is_empty() {
            s.push_str(&format!("{name} = \"{req}\"\n"));
        } else {
            let feats: Vec<String> = d.features.iter().map(|f| format!("\"{f}\"")).collect();
            s.push_str(&format!(
                "{name} = {{ version = \"{req}\", features = [{}] }}\n",
                feats.join(", ")
            ));
        }
    }
    s.push_str(
        "\n# The Teal checker runs inside htl's proc macros; the dev profile would build it\n\
         # unoptimised and make every `cargo build` that touches a .tl about 3x slower.\n\
         [profile.dev.build-override]\nopt-level = 3\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::{
        BuildTarget, Ctx, DEFAULT_TARGET, Result, htl_dep_version_of, profile, resolve_target,
        script_mismatch, t_cargo, target_names,
    };
    use htl::build_target::Script;

    #[test]
    fn dep_version_is_the_shortest_compatible_requirement() {
        assert_eq!(htl_dep_version_of("0.2.0"), "0.2");
        assert_eq!(htl_dep_version_of("0.2.7"), "0.2");
        assert_eq!(htl_dep_version_of("0.10.1"), "0.10");
        assert_eq!(htl_dep_version_of("1.0.0"), "1");
        assert_eq!(htl_dep_version_of("2.3.4"), "2");
    }

    /// `--embed` resolves through the registry, so a missing entry is a panic at the
    /// first scaffold rather than a file that quietly stops being written.
    #[test]
    fn the_registry_holds_the_target_embed_asks_for() {
        assert!(target_names().contains(&DEFAULT_TARGET.name()));
        let bin = profile(DEFAULT_TARGET).unwrap();
        assert_eq!(bin.lib.path, "src/lib.rs");
        assert_eq!(bin.main.as_ref().unwrap().path, "src/main.rs");
        // The default target is the one shape that works either way.
        assert!(bin.target.entry().accepts(true) && bin.target.entry().accepts(false));
    }

    /// `hb` is a build target and not a scaffold: it is the tree `htl new` writes with no
    /// Rust in it, so the registry has no entry for it and the flag does not offer it.
    #[test]
    fn the_default_build_target_has_no_profile_and_is_not_offered() {
        assert!(profile(BuildTarget::Hb).is_none());
        assert!(!target_names().contains(&"hb"));
        let e = err(resolve_target(Some("hb"), false, false));
        assert!(
            e.contains("the `hb` target is what plain `htl new` writes"),
            "{e}"
        );
        assert!(e.contains("targets that scaffold: bin, cdylib"), "{e}");
    }

    /// The refusal, as a string. A plain `unwrap_err()` would ask `&TargetProfile` for
    /// `Debug` — a derive on the whole registry to print a message no passing test sees.
    fn err<T>(r: Result<T>) -> String {
        r.err().expect("expected a refusal").to_string()
    }

    #[test]
    fn embed_and_target_bin_are_the_same_request() {
        let by_flag = resolve_target(None, true, false).unwrap().unwrap();
        let by_name = resolve_target(Some("bin"), false, false).unwrap().unwrap();
        assert_eq!(by_flag.target, by_name.target);
        // Both, agreeing, is not an error.
        assert!(resolve_target(Some("bin"), true, false).is_ok());
    }

    #[test]
    fn an_unknown_target_is_refused_with_the_registered_names() {
        // `.err().unwrap()`, not `unwrap_err()`: the Ok side is a `&TargetProfile`, and
        // making the registry `Debug` for the sake of a test message is the wrong trade.
        let e = err(resolve_target(Some("nope"), false, false));
        assert!(e.contains("unknown target `nope`"), "{e}");
        for n in target_names() {
            assert!(e.contains(n), "{e}");
        }
    }

    #[test]
    fn embed_disagreeing_with_target_is_refused() {
        let e = err(resolve_target(Some("other"), true, false));
        assert!(
            e.contains("--embed is the shorthand for --target bin"),
            "{e}"
        );
    }

    /// The half of the matrix hole a registered target reaches, with the message that
    /// names a way out: `cdylib` answers [`Script::Forbids`], so `--lib` is what it needs.
    ///
    /// The other half — a target that *requires* a script, which #104's window loop will
    /// be — has no arm of [`BuildTarget`] to answer it now that the rule is derived from
    /// the target rather than stored beside it, so it cannot be built out of a probe
    /// profile any more. What is left to assert about it is [`Script::accepts`] itself,
    /// which `htl-core` tests, and this line.
    #[test]
    fn a_target_that_disagrees_with_lib_names_the_targets_that_do_not() {
        assert!(Script::Requires.accepts(false) && !Script::Requires.accepts(true));

        let msg = script_mismatch(profile(BuildTarget::Cdylib).unwrap(), false);
        assert!(
            msg.contains("the `cdylib` target writes no entry script"),
            "{msg}"
        );
        assert!(msg.contains("targets that write one: bin"), "{msg}");
    }

    /// A C ABI library is not a project with an entry script, and the two are reconciled
    /// before anything is written rather than at the first file.
    #[test]
    fn the_cdylib_target_is_refused_without_lib_and_taken_with_it() {
        let e = err(resolve_target(Some("cdylib"), false, false));
        assert!(e.contains("so it needs --lib"), "{e}");
        let ffi = resolve_target(Some("cdylib"), false, true)
            .unwrap()
            .unwrap();
        assert_eq!(ffi.target, BuildTarget::Cdylib);
        // No binary to write, and the reference callers travel with the profile.
        assert!(ffi.main.is_none());
        let paths: Vec<&str> = ffi.extra.iter().map(|f| f.path).collect();
        assert_eq!(
            paths,
            vec![
                "examples/c/main.c",
                "examples/c/Makefile",
                "examples/python/run.py"
            ]
        );
    }

    /// The C ABI needs a shared object to load and a static library to link, and the
    /// runtime the generated wrappers call is behind a feature, so the dependency line
    /// is the table form rather than a bare requirement.
    #[test]
    fn the_cdylib_cargo_toml_is_a_c_library_with_the_ffi_feature() {
        let toml = t_cargo("sample", profile(BuildTarget::Cdylib).unwrap());
        assert!(
            toml.contains("[lib]\ncrate-type = [\"rlib\", \"cdylib\", \"staticlib\"]\n"),
            "{toml}"
        );
        assert!(
            toml.contains(&format!(
                "htl = {{ version = \"{}\", features = [\"ffi\"] }}\n",
                super::htl_dep_version()
            )),
            "{toml}"
        );
        assert!(
            toml.contains("serde = { version = \"1\", features = [\"derive\"] }\n"),
            "{toml}"
        );
    }

    /// What the reader is told to run follows the project rather than the target's name:
    /// the C ABI target points at the two reference callers, and the default target at the
    /// binary it writes only when there is a script to run.
    #[test]
    fn the_readme_commands_come_from_the_target() {
        let ctx = |script| Ctx {
            name: "sample",
            module: "sample",
            script,
        };
        let bin = profile(DEFAULT_TARGET).unwrap();
        assert!((bin.readme_commands)(&ctx(true)).contains("cargo run"));
        assert!(!(bin.readme_commands)(&ctx(false)).contains("cargo run"));

        let ffi = (profile(BuildTarget::Cdylib).unwrap().readme_commands)(&ctx(false));
        assert!(ffi.contains("make -C examples/c run"), "{ffi}");
        assert!(ffi.contains("python3 examples/python/run.py"), "{ffi}");
        assert!(ffi.contains("include/sample.h"), "{ffi}");
    }

    /// A crate whose only shape is the default `rlib` has no `[lib]` section; a target
    /// that needs more gets one from [`BuildTarget::crate_types`], which is the whole of
    /// what `Cargo.toml` has to know about the crate shape.
    #[test]
    fn cargo_toml_takes_the_crate_shape_from_the_target() {
        let toml = t_cargo("sample", profile(DEFAULT_TARGET).unwrap());
        assert!(!toml.contains("[lib]"), "{toml}");
        assert!(toml.contains("anyhow = \"1\"\n"), "{toml}");

        let toml = t_cargo("sample", profile(BuildTarget::Cdylib).unwrap());
        assert!(
            toml.contains("[lib]\ncrate-type = [\"rlib\", \"cdylib\", \"staticlib\"]\n"),
            "{toml}"
        );
    }
}
