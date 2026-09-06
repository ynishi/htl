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
        bail!("{} already exists and is not empty (use `htl init` to fill in a directory)", dir.display());
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
        format!("local {m} = require(\"{m}\")\n\nlocal g = {m}.greet(arg and arg[1] or \"teal\")\nprint(g.text)\n")
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
     `include_tl!`. Searched after `src/`; a `.tl` source anywhere on the path beats a\n\
     declaration, so nothing here can shadow an implementation. Declarations generated\n\
     from Rust (`#[host_module]`) are written next to the scripts, not here.\n"
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
     # Contract for a directory of modules (static form of TealResolver::expect_type):\n\
     # [[contract]]\n\
     # dir = \"mods\"             # relative to this file; \"sites/*\" = each subdirectory\n\
     # type = \"defs.Mod\"        # every module under `dir` must return this record\n\
     # require_fields = true    # ... with every declared field present\n\
     # exclude = [\"modkit\"]     # modules in `dir` not held to it (an SDK the host writes there)\n\
     # module = \"Site\"          # or: only this module name is held to it\n\
     # The host must enforce it too: htl::pkg::contract_resolvers(root, &config), or\n\
     # TealResolver::new(\"mods\").expect_type(\"defs.Mod\").require_fields() by hand.\n\
     # `htl check` reports `contract-unenforced` when neither appears in the Rust sources.\n"
        .to_string()
}

fn t_gitignore(embed: bool) -> String {
    let mut s = String::from(".mlua-pkgs/\n*.hb\n");
    if embed {
        s.push_str("/target\n");
    }
    s
}

fn t_readme(name: &str, m: &str, opts: &Options) -> String {
    let mut s = format!("# {name}\n\nTeal project managed with [htl](https://github.com/ynishi/htl).\n\n```sh\nhtl check .            # type-check + lints\n");
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

fn t_cargo(name: &str) -> String {
    format!(
        "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n# authors / license / repository: fill in yourself\n\n\
         [dependencies]\nhtl = \"0.1\"\nanyhow = \"1\"\n"
    )
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
        format!("    let g: htl::mlua::Table = h.lua().load(\"return require('{m}').greet('rust')\").eval()?;\n    println!(\"{{}}\", g.get::<String>(\"text\")?);\n")
    } else {
        "    let args: Vec<String> = std::env::args().skip(1).collect();\n    h.exec(MAIN, \"=main.tl\", &args)?;\n".to_string()
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
