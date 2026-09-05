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
//! └── Cargo.toml + src/main.rs   Rust host embedding the scripts (only with --embed)
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
        (dir.join("src").join(&m).join("init.tl"), t_module(&m)),
        (dir.join("tests").join(format!("{m}_test.tl")), t_test(&m)),
        (dir.join(".gitignore"), t_gitignore(opts.embed)),
        (dir.join("README.md"), t_readme(name, &m, opts)),
    ];
    if !opts.lib {
        files.push((dir.join("src").join("main.tl"), t_main(&m)));
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

fn t_main(m: &str) -> String {
    format!(
        "local {m} = require(\"{m}\")\n\nlocal g = {m}.greet(arg and arg[1] or \"teal\")\nprint(g.text)\n"
    )
}

fn t_test(m: &str) -> String {
    format!(
        "local t = require(\"htl.test\")\nlocal {m} = require(\"{m}\")\n\n\
         t.describe(\"{m}.greet\", function()\n   t.it(\"greets by name\", function()\n      \
         t.expect({m}.greet(\"teal\")):to_equal({{ who = \"teal\", text = \"hello, teal\" }})\n   end)\nend)\n"
    )
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
    if !opts.lib {
        s.push_str("htl run src/main.tl    # run the entry script\n");
    }
    s.push_str("htl test               # tests/*_test.tl via htl.test\nhtl fmt .              # whitespace formatter\nhtl pkg install        # fetch [deps] from mlua-pkg.toml\n");
    if opts.embed {
        s.push_str("cargo run              # Rust host with the scripts embedded (type-checked at build)\n");
    }
    s.push_str(&format!("```\n\nModule: `src/{m}/init.tl` (`require(\"{m}\")`). Consumers depending on this package get it as `require(\"{name}\")`.\n"));
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
        "use htl::include_tl;\nconst MAIN: &str = include_tl!(\"src/main.tl\");\n".to_string()
    };
    let run = if lib {
        format!("    let g: htl::mlua::Table = h.lua().load(\"return require('{m}').greet('rust')\").eval()?;\n    println!(\"{{}}\", g.get::<String>(\"text\")?);\n")
    } else {
        "    let args: Vec<String> = std::env::args().skip(1).collect();\n    h.exec(MAIN, \"=main.tl\", &args)?;\n".to_string()
    };
    format!(
        "//! Rust host: the Teal sources are type-checked at `cargo build` and embedded.\n\n\
         use htl::Htl;\n\n\
         const LIB: &[u8] = htl::include_tl_bytes!(\"src/{m}/init.tl\");\n{main_const}\n\
         fn main() -> anyhow::Result<()> {{\n    let h = Htl::new()?;\n    h.preload_bytes(\"{m}\", LIB)?;\n{run}    Ok(())\n}}\n"
    )
}
