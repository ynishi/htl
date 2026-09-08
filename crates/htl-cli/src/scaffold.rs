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
//! └── Cargo.toml + src/lib.rs    Rust host (only with a host): #[host_module] exposing
//!     + src/main.rs               `host` to Teal (declaration -> src/host.d.tl), the
//!                                 module embedded with include_tl_bytes!, and a thin
//!                                 binary on top when there is an entry script
//! ```
//!
//! # The Rust host is a profile, not a flag
//!
//! Everything above the `Cargo.toml` line is the same for every project. What varies is
//! the Rust host, and that is one value rather than a growing set of booleans: a
//! [`HostProfile`] says which crate shape it is (`[lib] crate-type`), what it depends on,
//! whether it wants an entry script, which Rust files it writes, and which Teal sample it
//! starts the project from. [`PROFILES`] is the registry, `--host <name>` picks an entry
//! from it, and a new kind of host is one more entry rather than another flag threaded
//! through every template.
//!
//! # A host is a library crate with a thin binary on top
//!
//! Every host writes `src/lib.rs`: the `#[host_module]`, its records, the embedded Teal
//! module, `preload` registering both, and a Rust test that goes through `preload`. When
//! the project has an entry script the host also writes `src/main.rs`, a few lines that
//! call `preload` and `exec` the script. So what a project grows — a second host module,
//! a C ABI layer, a window loop — grows in the library, and the binary never holds logic.
//!
//! [`scaffold`] therefore does the same three things whatever it is asked for: pick the
//! host, turn the profile into a list of paths and bodies, write the ones that do not
//! exist yet. No template branches on `--embed`; the ones that differ between hosts are
//! separate templates, chosen by the profile.
//!
//! Bodies where doubling every brace for `format!` cost more than it was worth — the Rust
//! host, the Teal sample — live under `crates/htl-cli/templates/` and are read with
//! `include_str!`, filled by replacing `{{name}}` / `{{mod}}` / `{{htl}}`. They stay in
//! this crate rather than being fetched, so `htl_dep_version()` keeps pinning a scaffold
//! to the htl release that wrote it. Short TOML and Markdown stay inline.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub struct Options {
    pub lib: bool,
    /// The Rust host to write, already resolved against [`PROFILES`] by
    /// [`resolve_host`], so nothing downstream can be asked for a host that does not
    /// exist or does not fit `--lib`.
    pub host: Option<&'static HostProfile>,
}

/// What a template is filled with: the package name and its Teal identifier. Nothing a
/// body needs branches on the flags any more — the profile decides which body is written
/// in the first place — so this is the whole of it.
pub struct Ctx<'a> {
    pub name: &'a str,
    pub module: &'a str,
}

/// A dependency line in the host's `Cargo.toml`.
pub enum Dep {
    /// This CLI's own release, derived at run time (see [`htl_dep_version`]) so a scaffold
    /// follows the htl that produced it.
    Htl,
    /// A literal requirement.
    Version(&'static str),
}

/// What a host has to say about `src/main.tl`. `--lib` is the user's side of the same
/// question, and the two are reconciled once, in [`resolve_host`], before anything is
/// written.
// `Requires` and `Forbids` are the two halves of the matrix hole; the registry holds only
// `Either` until #103 and #104 land their profiles, and until then the code that reads them
// is exercised by this module's tests.
#[allow(dead_code)]
#[derive(PartialEq, Eq, Clone, Copy)]
pub enum Script {
    /// The host runs an entry script and cannot be built without one (a window loop).
    Requires,
    /// The host is a library for someone else to call and has no entry point (a C ABI).
    Forbids,
    /// Either shape works; `--lib` decides.
    Either,
}

impl Script {
    fn accepts(self, lib: bool) -> bool {
        match self {
            Script::Requires => !lib,
            Script::Forbids => lib,
            Script::Either => true,
        }
    }
}

/// One Rust file a host writes, relative to the project root.
pub struct RustFile {
    pub path: &'static str,
    pub body: fn(&Ctx<'_>) -> String,
}

/// The Teal sample a project starts from. A host that dictates a different shape (a
/// `Game` record for a window host, an entry script that requires `host`) points these at
/// its own templates; everything else uses [`DEFAULT_TEAL`].
pub struct TealSample {
    pub module: fn(&Ctx<'_>) -> String,
    pub test: fn(&Ctx<'_>) -> String,
    /// `src/main.tl`, written only when the project has an entry script.
    pub main: fn(&Ctx<'_>) -> String,
}

/// A kind of Rust host: everything that differs between them, as data.
pub struct HostProfile {
    /// How it is named in the registry and on the command line (`--host <name>`).
    pub name: &'static str,
    /// `[lib] crate-type = [...]` beyond the default `rlib`, which needs no section at
    /// all: `["cdylib", "staticlib"]` for a C ABI host.
    pub lib_crate_types: &'static [&'static str],
    pub deps: &'static [(&'static str, Dep)],
    /// Does this host run an entry script, refuse one, or leave it to `--lib`?
    pub script: Script,
    /// `src/lib.rs`: the host module, the embedded Teal, `preload`. Always written.
    pub lib: RustFile,
    /// `src/main.rs`: the thin entry, written only when there is an entry script.
    pub main: Option<RustFile>,
    /// Anything else the host ships: `examples/`, `include/`, a header.
    pub extra: &'static [RustFile],
    pub teal: TealSample,
}

/// The default host: a library crate holding the host module and the embedded scripts,
/// with a thin binary on top when the project has an entry script.
const RUST: HostProfile = HostProfile {
    name: "rust",
    lib_crate_types: &[],
    deps: &[("htl", Dep::Htl), ("anyhow", Dep::Version("1"))],
    script: Script::Either,
    lib: RustFile {
        path: "src/lib.rs",
        body: rust_lib_rs,
    },
    main: Some(RustFile {
        path: "src/main.rs",
        body: rust_main_rs,
    }),
    extra: &[],
    teal: TealSample {
        module: teal_module,
        test: teal_test,
        // The entry script talks to the Rust side, so it is the host's, not the default.
        main: rust_main_tl,
    },
};

/// Every host kind there is. #103 (a C ABI library) and #104 (a macroquad window) are one
/// entry each.
pub static PROFILES: &[HostProfile] = &[RUST];

/// The host `--embed` is shorthand for.
pub const DEFAULT_HOST: &str = "rust";

/// What a project without a Rust host starts from.
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

/// Every registered host name, in registry order: what `--host` accepts and what a typo
/// is answered with.
pub fn host_names() -> Vec<&'static str> {
    PROFILES.iter().map(|p| p.name).collect()
}

pub fn profile(name: &str) -> Option<&'static HostProfile> {
    PROFILES.iter().find(|p| p.name == name)
}

/// Turn `--host` / `--embed` / `--lib` into the host to write, or into the reason there
/// is none to write. Called before the first file is created, so a refusal leaves the
/// directory as it was.
///
/// `--embed` is the shorthand for `--host rust` and stays one: clap's value parser only
/// applies to `--host`, so the two are reconciled here rather than pretended to be one
/// flag. Giving both is fine when they agree.
pub fn resolve_host(
    host: Option<&str>,
    embed: bool,
    lib: bool,
) -> Result<Option<&'static HostProfile>> {
    let name = match (host, embed) {
        (None, false) => return Ok(None),
        (None, true) => DEFAULT_HOST,
        (Some(n), false) => n,
        (Some(n), true) if n == DEFAULT_HOST => n,
        (Some(n), true) => bail!(
            "--embed is the shorthand for --host {DEFAULT_HOST}, so it cannot be given with --host {n}; drop one of them"
        ),
    };
    let Some(p) = profile(name) else {
        bail!(
            "unknown host `{name}`; registered hosts: {}",
            host_names().join(", ")
        );
    };
    if !p.script.accepts(lib) {
        bail!("{}", script_mismatch(p, lib));
    }
    Ok(Some(p))
}

/// Why a host and `--lib` do not fit, and which hosts do. Its own function because the
/// message is the whole point of refusing here rather than at the first write.
fn script_mismatch(p: &HostProfile, lib: bool) -> String {
    let fits: Vec<&str> = PROFILES
        .iter()
        .filter(|c| c.script.accepts(lib))
        .map(|c| c.name)
        .collect();
    let fits = if fits.is_empty() {
        "none".to_string()
    } else {
        fits.join(", ")
    };
    if lib {
        format!(
            "the `{}` host runs an entry script, which --lib leaves out; hosts that work with --lib: {fits}",
            p.name
        )
    } else {
        format!(
            "the `{}` host writes no entry script, so it needs --lib; hosts that write one: {fits}",
            p.name
        )
    }
}

/// Everything the scaffold would write, in the order it is reported, before anything
/// touches the disk.
fn plan(dir: &Path, name: &str, module: &str, opts: &Options) -> Vec<(PathBuf, String)> {
    let ctx = Ctx { name, module };
    let host = opts.host;
    let teal = host.map_or(&DEFAULT_TEAL, |h| &h.teal);

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
        (dir.join(".gitignore"), t_gitignore(host.is_some())),
        (dir.join("README.md"), t_readme(name, module, opts)),
    ];
    // `--lib` is what "no entry script" means, and a host that disagrees with it was
    // already refused, so the question is answered here for hosted and plain alike.
    if !opts.lib {
        files.push((dir.join("src").join("main.tl"), (teal.main)(&ctx)));
    }
    if let Some(h) = host {
        files.push((dir.join("Cargo.toml"), t_cargo(name, h)));
        files.push((dir.join(h.lib.path), (h.lib.body)(&ctx)));
        // The binary exists to run the entry script, so without one it is not written —
        // and a host that has no binary to write says so with `main: None`.
        if !opts.lib
            && let Some(m) = &h.main
        {
            files.push((dir.join(m.path), (m.body)(&ctx)));
        }
        for f in h.extra {
            files.push((dir.join(f.path), (f.body)(&ctx)));
        }
    }
    files
}

/// What a scaffold run did: the files it created, and the ones it left alone because they
/// were already there. `htl init` reports both, so asking an existing project for a host
/// says which of that host's files it did not touch rather than silently skipping them.
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

/// The three placeholders a template file may use. Plain `str::replace`: the bodies are
/// ours, so there is nothing to escape and no engine to depend on.
fn fill(template: &str, ctx: &Ctx<'_>) -> String {
    template
        .replace("{{name}}", ctx.name)
        .replace("{{mod}}", ctx.module)
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
     holds. Declarations generated from Rust (`#[host_module]`) are written next to the\n\
     scripts, not here. Both are committed.\n"
        .to_string()
}

fn t_htl_toml() -> String {
    "# htl project settings (htl check / htl test / htl fmt / include_tl! all read this).\n\
     # Command-line flags and HTL_LINTS / HTL_LINT override it.\n\n\
     [lint]\n\
     # enable  = [\"class-record\", \"explicit-number\"]   # opt-in rules (htl check --list-lints)\n\
     # disable = [\"shadow-local\"]\n\
     # strict  = true   # lints fail check/test and include_tl!; false makes the macro advisory\n\n\
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

fn t_gitignore(has_host: bool) -> String {
    // `.htl/` holds the run cache and the installed deps: generated, machine-local, and
    // keyed on absolute paths, so it is never worth sharing. One line covers both.
    let mut s = String::from(".htl/\n*.hb\n");
    if has_host {
        s.push_str("/target\n");
    }
    s
}

fn t_readme(name: &str, m: &str, opts: &Options) -> String {
    let host = opts.host.is_some();
    let script = !opts.lib;
    let mut s = format!(
        "# {name}\n\nTeal project managed with [htl](https://github.com/ynishi/htl).\n\n```sh\nhtl check .            # type-check + lints\n"
    );
    if script && !host {
        s.push_str("htl run src/main.tl    # run the entry script\n");
    }
    s.push_str("htl test               # tests/*_test.tl via htl.test\nhtl fmt .              # whitespace formatter\nhtl pkg install        # fetch [deps] from mlua-pkg.toml\n");
    if host {
        if script {
            s.push_str("cargo run              # the binary: preload, then src/main.tl (type-checked at build)\n");
            s.push_str("                       # (src/main.tl requires the Rust `host`, so `htl run` cannot run it)\n");
        }
        s.push_str(
            "cargo test             # the library's Rust test: the module loaded through preload\n",
        );
    }
    s.push_str(&format!(
        "```\n\nModule: `src/{m}/init.tl` (`require(\"{m}\")` from `src/` and `tests/`).\n\n\
         `mlua-pkg.toml` `entry = \"src/{m}\"` only matters to *consumers* that depend on this\n\
         package through mlua-pkg: they get it as `require(\"{name}\")`. "
    ));
    if host {
        s.push_str(
            "The Rust host is a library:\n`src/lib.rs` holds the `#[host_module]`, embeds this module, and registers both in\n\
             `preload(&Htl)`. ",
        );
        if script {
            s.push_str(
                "`src/main.rs` is a few lines on top of it — `preload`, then the entry\nscript. Grow the library, not the binary.\n\n",
            );
        } else {
            s.push_str(
                "There is no binary: call `preload` from whatever embeds this\ncrate, and grow the library.\n\n",
            );
        }
        s.push_str(
            "`src/host.d.tl` is generated from `#[host_module]` in `src/lib.rs`: `cargo build` writes it,\n\
             and so does `htl dts` / `htl check` without building, so the Teal side always sees the\n\
             current Rust signatures.\n",
        );
    } else {
        s.push_str("Ignore it if nobody depends on this package.\n");
    }
    s
}

/// The `htl` requirement a scaffolded host declares: this CLI's own release, in the
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

/// The host's `Cargo.toml`, assembled from the profile: crate shape, then dependencies,
/// then the one profile setting every host needs.
fn t_cargo(name: &str, host: &HostProfile) -> String {
    let mut s = format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n# authors / license / repository: fill in yourself\n\n"
    );
    if !host.lib_crate_types.is_empty() {
        let types: Vec<String> = host
            .lib_crate_types
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect();
        s.push_str(&format!("[lib]\ncrate-type = [{}]\n\n", types.join(", ")));
    }
    s.push_str("[dependencies]\n");
    for (dep_name, dep) in host.deps {
        let req = match dep {
            Dep::Htl => htl_dep_version(),
            Dep::Version(v) => (*v).to_string(),
        };
        s.push_str(&format!("{dep_name} = \"{req}\"\n"));
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
        DEFAULT_HOST, Dep, HostProfile, Result, RustFile, Script, TealSample, host_names,
        htl_dep_version_of, profile, resolve_host, script_mismatch, t_cargo,
    };

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
    fn the_registry_holds_the_host_embed_asks_for() {
        assert!(host_names().contains(&DEFAULT_HOST));
        let rust = profile(DEFAULT_HOST).unwrap();
        assert_eq!(rust.lib.path, "src/lib.rs");
        assert_eq!(rust.main.as_ref().unwrap().path, "src/main.rs");
        // The default host is the one shape that works either way.
        assert!(rust.script.accepts(true) && rust.script.accepts(false));
    }

    /// The refusal, as a string. A plain `unwrap_err()` would ask `&HostProfile` for
    /// `Debug` — a derive on the whole registry to print a message no passing test sees.
    fn err<T>(r: Result<T>) -> String {
        r.err().expect("expected a refusal").to_string()
    }

    #[test]
    fn embed_and_host_are_the_same_request() {
        let by_flag = resolve_host(None, true, false).unwrap().unwrap();
        let by_name = resolve_host(Some("rust"), false, false).unwrap().unwrap();
        assert_eq!(by_flag.name, by_name.name);
        // Both, agreeing, is not an error.
        assert!(resolve_host(Some("rust"), true, false).is_ok());
    }

    #[test]
    fn an_unknown_host_is_refused_with_the_registered_names() {
        // `.err().unwrap()`, not `unwrap_err()`: the Ok side is a `&HostProfile`, and
        // making the registry `Debug` for the sake of a test message is the wrong trade.
        let e = err(resolve_host(Some("nope"), false, false));
        assert!(e.contains("unknown host `nope`"), "{e}");
        for n in host_names() {
            assert!(e.contains(n), "{e}");
        }
    }

    #[test]
    fn embed_disagreeing_with_host_is_refused() {
        let e = err(resolve_host(Some("other"), true, false));
        assert!(
            e.contains("--embed is the shorthand for --host rust"),
            "{e}"
        );
    }

    /// The two halves of the matrix hole, with the message that names a way out. No
    /// registered host has either shape yet (#103 and #104 bring them), so the profiles
    /// are built here rather than looked up.
    #[test]
    fn a_host_that_disagrees_with_lib_names_the_hosts_that_do_not() {
        let needs = HostProfile {
            name: "mq",
            script: Script::Requires,
            ..probe()
        };
        let msg = script_mismatch(&needs, true);
        assert!(msg.contains("the `mq` host runs an entry script"), "{msg}");
        assert!(msg.contains("hosts that work with --lib: rust"), "{msg}");

        let refuses = HostProfile {
            name: "ffi",
            script: Script::Forbids,
            ..probe()
        };
        let msg = script_mismatch(&refuses, false);
        assert!(msg.contains("so it needs --lib"), "{msg}");
        assert!(msg.contains("hosts that write one: rust"), "{msg}");
    }

    #[test]
    fn script_requirements_are_read_from_the_profile() {
        assert!(Script::Requires.accepts(false) && !Script::Requires.accepts(true));
        assert!(Script::Forbids.accepts(true) && !Script::Forbids.accepts(false));
        assert!(Script::Either.accepts(true) && Script::Either.accepts(false));
    }

    /// A crate whose only shape is the default `rlib` has no `[lib]` section; a library
    /// host that needs more gets one from its profile, which is the whole of what
    /// `Cargo.toml` has to know about the crate shape.
    #[test]
    fn cargo_toml_takes_the_crate_shape_from_the_profile() {
        let rust = profile(DEFAULT_HOST).unwrap();
        let toml = t_cargo("sample", rust);
        assert!(!toml.contains("[lib]"), "{toml}");
        assert!(toml.contains("anyhow = \"1\"\n"), "{toml}");

        let libbed = HostProfile {
            lib_crate_types: &["rlib", "cdylib"],
            ..probe()
        };
        let toml = t_cargo("sample", &libbed);
        assert!(
            toml.contains("[lib]\ncrate-type = [\"rlib\", \"cdylib\"]\n"),
            "{toml}"
        );
    }

    fn probe() -> HostProfile {
        HostProfile {
            name: "probe",
            lib_crate_types: &[],
            deps: &[("htl", Dep::Htl)],
            script: Script::Either,
            lib: RustFile {
                path: "src/lib.rs",
                body: super::rust_lib_rs,
            },
            main: None,
            extra: &[],
            teal: TealSample {
                module: super::teal_module,
                test: super::teal_test,
                main: super::teal_main,
            },
        }
    }
}
