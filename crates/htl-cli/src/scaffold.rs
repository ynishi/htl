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
//! └── Cargo.toml + src/main.rs   Rust host (only with --embed): #[host_module] exposing
//!                                `host` to Teal (declaration -> src/host.d.tl), scripts
//!                                embedded with include_tl! / include_tl_bytes!
//! ```
//!
//! # The Rust host is a profile, not a flag
//!
//! Everything above the `Cargo.toml` line is the same for every project. What varies is
//! the Rust host, and that is one value rather than a growing set of booleans: a
//! [`HostProfile`] says which crate shape it is (`[lib] crate-type`), what it depends on,
//! whether it wants an entry script, which Rust files it writes, and which Teal sample it
//! starts the project from. [`PROFILES`] is the registry; `bin` — the host `--embed` has
//! always written — is the only entry today, and a new kind of host is one more entry
//! rather than another flag threaded through every template.
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
    pub embed: bool,
}

/// What a template is filled with: the package name, its Teal identifier, and the flags,
/// for the bodies that still read them.
pub struct Ctx<'a> {
    pub name: &'a str,
    pub module: &'a str,
    pub opts: &'a Options,
}

/// A dependency line in the host's `Cargo.toml`.
pub enum Dep {
    /// This CLI's own release, derived at run time (see [`htl_dep_version`]) so a scaffold
    /// follows the htl that produced it.
    Htl,
    /// A literal requirement.
    Version(&'static str),
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
    /// `src/main.tl`, written only when the host wants an entry script.
    pub main: fn(&Ctx<'_>) -> String,
}

/// A kind of Rust host: everything that differs between them, as data.
pub struct HostProfile {
    /// How it is named in the registry (and, from #106 on, on the command line).
    pub name: &'static str,
    /// `[lib] crate-type = [...]`. Empty for a binary crate, which is what `bin` is, and
    /// then the section is not written at all.
    pub lib_crate_types: &'static [&'static str],
    pub deps: &'static [(&'static str, Dep)],
    /// Does this project get `src/main.tl`? `bin` runs either way and leaves it to
    /// `--lib`; a host whose shape decides it answers without looking.
    pub wants_script: fn(&Options) -> bool,
    pub rust: &'static [RustFile],
    pub teal: TealSample,
}

/// The `--embed` host: a binary crate whose `main` embeds the scripts.
const BIN: HostProfile = HostProfile {
    name: "bin",
    lib_crate_types: &[],
    deps: &[("htl", Dep::Htl), ("anyhow", Dep::Version("1"))],
    wants_script: |opts| !opts.lib,
    rust: &[RustFile {
        path: "src/main.rs",
        body: bin_main_rs,
    }],
    teal: TealSample {
        module: teal_module,
        test: teal_test,
        // The entry script talks to the Rust side, so it is the host's, not the default.
        main: bin_main_tl,
    },
};

/// Every host kind there is. #103 (a C ABI library) and #104 (a macroquad window) are one
/// entry each.
pub static PROFILES: &[HostProfile] = &[BIN];

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
const T_BIN_MAIN_TL: &str = include_str!("../templates/bin/main.tl");
const T_BIN_MAIN_RS: &str = include_str!("../templates/bin/main.rs");
const T_BIN_MAIN_RS_NO_SCRIPT: &str = include_str!("../templates/bin/main_no_script.rs");

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

/// The host to scaffold. `--embed` means `bin`; #106 turns this into `--host <name>`
/// looked up in the same registry.
fn selected_host(opts: &Options) -> Option<&'static HostProfile> {
    opts.embed
        .then(|| profile("bin").expect("the `bin` profile is registered"))
}

fn profile(name: &str) -> Option<&'static HostProfile> {
    PROFILES.iter().find(|p| p.name == name)
}

/// Everything the scaffold would write, in the order it is reported, before anything
/// touches the disk.
fn plan(dir: &Path, name: &str, module: &str, opts: &Options) -> Vec<(PathBuf, String)> {
    let ctx = Ctx { name, module, opts };
    let host = selected_host(opts);
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
    if host.map_or(!opts.lib, |h| (h.wants_script)(opts)) {
        files.push((dir.join("src").join("main.tl"), (teal.main)(&ctx)));
    }
    if let Some(h) = host {
        files.push((dir.join("Cargo.toml"), t_cargo(name, h)));
        for f in h.rust {
            files.push((dir.join(f.path), (f.body)(&ctx)));
        }
    }
    files
}

/// Write every template file that does not exist yet. Returns the paths written.
/// With `must_be_new`, the directory must not exist (or be empty).
pub fn scaffold(dir: &Path, name: &str, opts: &Options, must_be_new: bool) -> Result<Vec<PathBuf>> {
    if must_be_new && dir.exists() && dir.read_dir()?.next().is_some() {
        bail!(
            "{} already exists and is not empty (use `htl init` to fill in a directory)",
            dir.display()
        );
    }
    let m = module_ident(name);

    let mut written = Vec::new();
    for (path, text) in plan(dir, name, &m, opts) {
        if path.exists() {
            continue;
        }
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        written.push(path);
    }
    Ok(written)
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

fn bin_main_tl(ctx: &Ctx<'_>) -> String {
    fill(T_BIN_MAIN_TL, ctx)
}

/// `--lib` leaves the project without an entry script, so this host evaluates the module
/// instead of executing one. Two bodies rather than one body with holes in it: the pair
/// is what the `lib` host of #106 replaces.
fn bin_main_rs(ctx: &Ctx<'_>) -> String {
    let t = if ctx.opts.lib {
        T_BIN_MAIN_RS_NO_SCRIPT
    } else {
        T_BIN_MAIN_RS
    };
    fill(t, ctx)
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
    let mut s = format!(
        "# {name}\n\nTeal project managed with [htl](https://github.com/ynishi/htl).\n\n```sh\nhtl check .            # type-check + lints\n"
    );
    if !opts.lib && !opts.embed {
        s.push_str("htl run src/main.tl    # run the entry script\n");
    }
    s.push_str("htl test               # tests/*_test.tl via htl.test\nhtl fmt .              # whitespace formatter\nhtl pkg install        # fetch [deps] from mlua-pkg.toml\n");
    if opts.embed {
        s.push_str("cargo run              # Rust host with the scripts embedded (type-checked at build)\n");
        if !opts.lib {
            s.push_str("                       # (src/main.tl requires the Rust `host`, so `htl run` cannot run it)\n");
        }
    }
    s.push_str(&format!(
        "```\n\nModule: `src/{m}/init.tl` (`require(\"{m}\")` from `src/` and `tests/`).\n\n\
         `mlua-pkg.toml` `entry = \"src/{m}\"` only matters to *consumers* that depend on this\n\
         package through mlua-pkg: they get it as `require(\"{name}\")`. "
    ));
    if opts.embed {
        s.push_str(
            "The Rust host in `src/main.rs`\nembeds the scripts directly and does not use it.\n\n\
             `src/host.d.tl` is generated from `#[host_module]` in `src/main.rs`: `cargo build` writes it,\n\
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
    use super::{Dep, HostProfile, Options, PROFILES, TealSample, htl_dep_version_of, t_cargo};

    #[test]
    fn dep_version_is_the_shortest_compatible_requirement() {
        assert_eq!(htl_dep_version_of("0.2.0"), "0.2");
        assert_eq!(htl_dep_version_of("0.2.7"), "0.2");
        assert_eq!(htl_dep_version_of("0.10.1"), "0.10");
        assert_eq!(htl_dep_version_of("1.0.0"), "1");
        assert_eq!(htl_dep_version_of("2.3.4"), "2");
    }

    /// `--embed` resolves through the registry, so a missing `bin` entry is a panic at the
    /// first scaffold rather than a file that quietly stops being written.
    #[test]
    fn the_registry_holds_the_host_embed_asks_for() {
        assert!(PROFILES.iter().any(|p| p.name == "bin"));
        let bin = PROFILES.iter().find(|p| p.name == "bin").unwrap();
        assert!((bin.wants_script)(&Options {
            lib: false,
            embed: true
        }));
        assert!(!(bin.wants_script)(&Options {
            lib: true,
            embed: true
        }));
    }

    /// A binary host has no `[lib]` section; a library host gets one from its profile,
    /// which is the whole of what `Cargo.toml` has to know about the crate shape.
    #[test]
    fn cargo_toml_takes_the_crate_shape_from_the_profile() {
        let bin = PROFILES.iter().find(|p| p.name == "bin").unwrap();
        let toml = t_cargo("sample", bin);
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
            wants_script: |_| false,
            rust: &[],
            teal: TealSample {
                module: super::teal_module,
                test: super::teal_test,
                main: super::teal_main,
            },
        }
    }
}
