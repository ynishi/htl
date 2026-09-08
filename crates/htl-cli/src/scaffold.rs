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

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub struct Options {
    pub lib: bool,
    pub embed: bool,
}

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
    let mut files: Vec<(PathBuf, String)> = vec![
        (dir.join("mlua-pkg.toml"), t_manifest(name, &m)),
        (dir.join("htl.toml"), t_htl_toml()),
        (dir.join("types").join("README.md"), t_types_readme()),
        (dir.join("src").join(&m).join("init.tl"), t_module(&m)),
        (dir.join("tests").join(format!("{m}_test.tl")), t_test(&m)),
        (dir.join(".gitignore"), t_gitignore(opts.embed)),
        (dir.join("README.md"), t_readme(name, &m, opts)),
    ];
    if !opts.lib {
        files.push((dir.join("src").join("main.tl"), t_main(&m, opts.embed)));
    }
    if opts.embed {
        files.push((dir.join("Cargo.toml"), t_cargo(name)));
        files.push((dir.join("src").join("main.rs"), t_main_rs(&m, opts.lib)));
    }

    let mut written = Vec::new();
    for (path, text) in files {
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

fn t_manifest(name: &str, m: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n# Consumers `require(\"{name}\")` -> this directory's init.tl\nentry = \"src/{m}\"\n\n[deps]\n# lshape = {{ git = \"https://github.com/ynishi/lshape\", tag = \"v0.1\" }}\n"
    )
}

fn t_module(m: &str) -> String {
    format!(
        "local record {m}\n   record Greeting\n      who: string\n      text: string\n   end\nend\n\n\
         function {m}.greet(who: string): {m}.Greeting\n   return {{ who = who, text = \"hello, \" .. who }}\nend\n\n\
         return {m}\n"
    )
}

fn t_main(m: &str, embed: bool) -> String {
    if embed {
        format!(
            "local {m} = require(\"{m}\")\nlocal host = require(\"host\") -- Rust side, see src/main.rs (types: src/host.d.tl)\n\n\
             local g = {m}.greet(arg and arg[1] or \"teal\")\nprint(g.text)\n\n\
             print(host:greet(g.who))\nlocal p: host.Point = host:scale({{ x = 1, y = 2 }}, 3)\nprint(\"scaled:\", p.x, p.y)\n"
        )
    } else {
        format!(
            "local {m} = require(\"{m}\")\n\nlocal g = {m}.greet(arg and arg[1] or \"teal\")\nprint(g.text)\n"
        )
    }
}

fn t_test(m: &str) -> String {
    format!(
        "local t = require(\"htl.test\")\nlocal {m} = require(\"{m}\")\n\n\
         t.describe(\"{m}.greet\", function()\n   t.it(\"greets by name\", function()\n      \
         t.expect({m}.greet(\"teal\")):to_equal({{ who = \"teal\", text = \"hello, teal\" }})\n   end)\nend)\n"
    )
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

fn t_gitignore(embed: bool) -> String {
    // `.htl/` holds the run cache and the installed deps: generated, machine-local, and
    // keyed on absolute paths, so it is never worth sharing. One line covers both.
    let mut s = String::from(".htl/\n*.hb\n");
    if embed {
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

fn t_cargo(name: &str) -> String {
    let htl = htl_dep_version();
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n# authors / license / repository: fill in yourself\n\n\
         [dependencies]\nhtl = \"{htl}\"\nanyhow = \"1\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::htl_dep_version_of;

    #[test]
    fn dep_version_is_the_shortest_compatible_requirement() {
        assert_eq!(htl_dep_version_of("0.2.0"), "0.2");
        assert_eq!(htl_dep_version_of("0.2.7"), "0.2");
        assert_eq!(htl_dep_version_of("0.10.1"), "0.10");
        assert_eq!(htl_dep_version_of("1.0.0"), "1");
        assert_eq!(htl_dep_version_of("2.3.4"), "2");
    }
}

fn t_main_rs(m: &str, lib: bool) -> String {
    let main_const = if lib {
        String::new()
    } else {
        "const MAIN: &str = include_tl!(\"src/main.tl\");\n".to_string()
    };
    let use_line = if lib {
        "use htl::{Htl, TealRecord, host_module};\n"
    } else {
        "use htl::{Htl, TealRecord, host_module, include_tl};\n"
    };
    let run = if lib {
        format!(
            "    let g: htl::mlua::Table = h.lua().load(\"return require('{m}').greet('rust')\").eval()?;\n    println!(\"{{}}\", g.get::<String>(\"text\")?);\n"
        )
    } else {
        // `exec` hands the arguments over as `...`; `set_arg` is what fills the `arg`
        // table main.tl reads, the way `htl run` does, so the same script runs both ways.
        "    let args: Vec<String> = std::env::args().skip(1).collect();\n    h.set_arg(\"main.tl\", &args)?; // `arg[1]`.. as under `htl run`; `exec` alone passes `...`\n    h.exec(MAIN, \"=main.tl\", &args)?;\n".to_string()
    };
    format!(
        "//! Rust host: the Teal sources are type-checked at `cargo build` and embedded.\n\n\
         {use_line}\n\
         /// Crosses the Rust <-> Teal boundary as a plain table (`host.Point` on the Teal side).\n\
         #[derive(TealRecord, Clone)]\npub struct Point {{\n    pub x: f64,\n    pub y: f64,\n}}\n\n\
         pub struct Host;\n\n\
         /// Exposed to Teal as `require(\"host\")`. Its declaration is written to `src/host.d.tl`\n\
         /// by this macro at build time, and by `htl dts` / `htl check` without building.\n\
         #[host_module(name = \"host\", dts = \"src/host.d.tl\", records = [Point])]\n\
         impl Host {{\n\
         \x20   pub fn greet(&self, who: &str) -> String {{\n        format!(\"hello from Rust, {{who}}\")\n    }}\n\n\
         \x20   pub fn scale(&self, p: Point, k: f64) -> Point {{\n        Point {{ x: p.x * k, y: p.y * k }}\n    }}\n\
         }}\n\n\
         // Teal sources, checked at build. Keep these after `#[host_module]` (same file, source order)\n\
         // so the declaration exists when they are checked.\n\
         const LIB: &[u8] = htl::include_tl_bytes!(\"src/{m}/init.tl\");\n{main_const}\n\
         fn main() -> anyhow::Result<()> {{\n    let h = Htl::new()?;\n    Host.htl_preload(&h)?;\n    h.preload_bytes(\"{m}\", LIB)?;\n{run}    Ok(())\n}}\n"
    )
}
