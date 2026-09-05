# htl — Holistic Typed Lua

[Teal](https://github.com/teal-language/tl) (typed Lua) with the toolchain hidden
behind `cargo`. One binary type-checks, lints, formats, tests, bundles and runs
`.tl`; one proc macro makes Teal type errors fail `cargo build`; one resolver puts
`.tl` modules into [mlua-pkg](https://github.com/ynishi/mlua-pkg)'s `require`
chain. The Teal compiler (`tl.lua`) is embedded in the mlua state — there is no
`luarocks`, no `tl` CLI, no generated `.lua` in your tree.

```text
scripts/foo.tl ──include_tl!──▶ cargo build   (Teal type error = rustc error, with span)
               ──htl run ─────▶ check → gen → load, in one mlua state
               ──htl build────▶ stripped Lua 5.4 bytecode bundle (.hb), no source shipped
Rust impl Host ──#[host_module]▶ UserData impl + host.d.tl   (Rust signature change breaks .tl at build)
```

## Install

```sh
cargo install htl-cli          # binaries: htl, cargo-htl  (so `cargo htl <verb>` works)
cargo install mlua-pkg         # optional: `htl pkg install` delegates to it
```

```toml
[dependencies]
htl = "0.1"                    # embedding: engine + proc macros in one import
```

| crate | role |
|---|---|
| `htl` | umbrella: re-exports `htl-core` and (feature `macros`, default on) the proc macros. Depend on this one. |
| `htl-core` | engine: `Htl`, lints, fmt, bundle, test runner, mlua-pkg resolver |
| `htl-macros` | `include_tl!` / `include_tl_bytes!` / `TealRecord` / `host_module`; generated code targets `::htl::` |
| `htl-cli` | the `htl` / `cargo-htl` binaries |

## CLI

| command | what it does |
|---|---|
| `htl new <name>` / `htl init [dir]` | scaffold: `mlua-pkg.toml`, `src/<mod>/init.tl`, `src/main.tl`, `tests/`, README (`--lib`, `--embed` for a Rust host) |
| `htl check [paths] [--strict] [--lint +rule,-rule]` | type-check; htl lints as `lint:` (advisory, `--strict` fails on them) |
| `htl run <file.tl \| app.hb> [args]` | check then execute; `require` of a `.tl` with type errors fails |
| `htl test [paths] [--filter s] [--lib mod]` | `*_test.tl` and `tests/**/*.tl`, one isolated state per file |
| `htl fmt [paths] [--check] [--indent N]` | whitespace formatter (indentation from the syntax tree, blank lines, trailing space) |
| `htl gen <file.tl> [-o out.lua]` | readable Lua, the escape hatch out of htl |
| `htl build <dir> -o app.hb [--entry main]` | stripped-bytecode bundle of a module tree |
| `htl pkg <args>` | passthrough to `mlua-pkg` at the nearest `mlua-pkg.toml` root |
| `htl dts [dir]` | write the `.d.tl` files declared by `#[host_module]` / `#[derive(TealRecord)]` from Rust source, no build needed (`check` / `run` / `test` / `build` do this automatically when inside a crate) |

`mlua-pkg.toml` is detected by walking up from the file: vendored deps become
visible to the checker and to `run` / `test` / `build` automatically. When a
directory is given, `check` / `fmt` / `build` / `test` walk the project's own files only:
`target/`, `node_modules/`, `.mlua-pkgs/` (or wherever `MLUA_PKG_DIR` points) and any
dot-directory are not entered, so dependencies' sources and tests stay theirs. A
directory passed explicitly is always walked.

## Embedding in Rust

```rust
use htl::{Htl, TealRecord, host_module, include_tl, include_tl_bytes};

#[derive(TealRecord, Clone)]           // Teal record <-> plain table (IntoLua / FromLua)
pub struct Point { pub x: f64, pub y: f64 }

pub struct Host { started: std::time::Instant }

#[host_module(name = "host", dts = "scripts/host.d.tl", records = [Point])]
impl Host {
    pub fn uptime_ms(&self) -> u64 { self.started.elapsed().as_millis() as u64 }
    pub fn scale(&self, p: Point, k: f64) -> Point { Point { x: p.x * k, y: p.y * k } }
    pub fn greet(name: &str) -> String { format!("hello, {name}") }            // static
    pub fn parse(s: &str) -> Result<i64, std::num::ParseIntError> { s.parse() } // Err -> Lua error
}

const MAIN: &str = include_tl!("scripts/main.tl");              // checked at cargo build
const UTIL: &[u8] = include_tl_bytes!("scripts/util.tl");       // same, as stripped bytecode

fn main() -> anyhow::Result<()> {
    let h = Htl::new()?;
    Host { started: std::time::Instant::now() }.htl_preload(&h)?;
    h.preload_bytes("util", UTIL)?;
    h.exec(MAIN, "=main.tl", &[])?;
    Ok(())
}
```

`#[host_module]` turns the plain `impl` into a `mlua::UserData` impl and writes
`scripts/host.d.tl` when it expands, so `scripts/main.tl` sees
`host:scale(p: Point, k: number): Point` and `host.Point`. Change a Rust signature
and the next `cargo build` fails inside the `.tl` that relied on it. `&str`,
`&[T]` and `&Record` parameters are accepted (`&mut` is not); nested records come
from structs in the same source file, records from other modules via
`uses = [Name]` + their own `.d.tl`.

Runtime resolution through mlua-pkg:

```rust
let mut reg = mlua_pkg::Registry::new();
reg.add(NativeResolver::new().add("host", |lua| { /* Rust table */ }));
reg.add(htl::pkg::TealResolver::new("scripts")?);     // .tl / init.tl -> check + gen; .d.tl -> type-only table
reg.add(mlua_pkg::resolvers::FsResolver::new("scripts")?);
reg.install(h.lua())?;
// or, with an mlua-pkg.toml: htl::pkg::Project::find(dir)?.registry()
```

A `.tl` that fails its type check is `Some(Err)` in mlua-pkg's terms: it never falls
through to a later resolver. Native modules must be registered *before* the Teal
resolver and described by a `.d.tl` for the checker.

Teal resolves every `require("literal")` at check time, and htl keeps it that way. When a
module exists only at run time (the user's `Tasks.tl` that a long-built host loads), the
same two shapes that TypeScript, Kotlin scripting and Gradle use apply:

- **Declare it** (`declare module` / `.d.ts` in TS terms): ship `Tasks.d.tl` in the host's
  tree with the contract (`local tsk = require("tsk")  local Tasks: tsk.Tasks  return Tasks`).
  The build checks the host's scripts against the declaration; at run time a
  `TealResolver` rooted at the user's project serves the real file.
- **Hand the user a typed constructor** (`defineConfig` / `satisfies UserConfig` in TS
  terms): the SDK exports `define: function(t: tsk.Tasks): tsk.Tasks` and the user writes
  `return tsk.define({ ... })`. Field-level errors with line numbers, no annotation on the
  user's side, and `expect_type` becomes a belt-and-braces check.

A dynamic `require(name_in_a_variable)` typed as `any` is the escape hatch, like
GDScript's `load()` or a shorthand `declare module "x"`; use it only when the module name
itself is unknown until run time.

Errors that come out of running Lua (a host function's `Err`, a Lua `error(...)`) carry
mlua's `stack traceback:`; `htl::user_message(&err)` returns the innermost cause alone,
which is what `htl run` / `htl test` print.

For mod / plugin directories, `TealResolver::new("mods")?.expect_type("defs.Mod")` holds
every served module to a record type: a mod that returns the wrong shape is rejected at
`require` time even if it never annotates its own return value. It rejects fields of the
wrong *type*; it does not reject *missing* fields (every Teal record field is nilable), so
nil-guard optional data on the host side.

## Lints (`htl check`, `include_tl!`)

| rule | default | catches |
|---|---|---|
| `nil-index` | on | `t[k].x`, `t[k]:m()`, `t[k]()`, `t[k][j]` — Teal types a map/array lookup as `V`, not `V \| nil` |
| `enum-exhaustive` | on | `if e == "a" ... elseif e == "b" ... end` over an enum with a value left unhandled and no `else`; enums nested in records and enums from required modules count |
| `shadow-local` | on | a local / loop var / parameter reusing an enclosing local's name |
| `no-global` | on | `global` declarations |
| `no-any` | off | explicit `any` annotations and `as any` casts |
| `explicit-number` | off | an unannotated local initialized with a numeric literal: `local n = 0` infers `integer`, `0.0` infers `number`, and a later `n = n * 1.5` fails; write `local n: number = 0` |

Silence one occurrence with a trailing `-- htl: allow(nil-index)`. `include_tl!`
treats lints as errors (`HTL_LINT=warn` downgrades, `HTL_LINTS=+no-any,-shadow-local`
configures).

## Tests

```lua
local t = require("htl.test")            -- typed via test.d.tl
t.describe("util.add", function()
   t.it("adds", function()
      t.expect(util.add({x=1,y=2}, {x=10,y=20})):to_equal({x=11,y=22})
   end)
end)
```

`expect(x)` is generic, so `t.expect(1 + 1):to_equal("2")` is a *type* error and the
file is refused before it runs. Any library exposing
`run(filter) -> {passed, failed, failures}` plugs in via `--lib`; files that use no
such library pass if they run to completion.

## Layout of a project (`htl new`)

```text
<name>/
├── mlua-pkg.toml          [package] entry = "src/<mod>"  → consumers require("<name>")
├── src/<mod>/init.tl      the module (require("<mod>") from src/ and tests/)
├── src/main.tl            entry script
└── tests/<mod>_test.tl
```

mlua-pkg's `entry` is a directory, so a consumer's `require("<name>")` looks for
`<name>/init.tl`. A flat package can instead ship `<name>/<name>.tl` (e.g. `entry = "src"`
with `src/<name>.tl`); htl resolves that form in the checker and in `TealResolver`.

## Pitfalls the checker now names

- **Case-insensitive filesystems (macOS, Windows)**: `require("site")` from a file
  called `Site.tl` resolves to that very file. Teal reports it as "no type information
  for required module"; htl appends that the module resolved to the requiring file
  itself and that one of the names has to change.
- **Numeric inference**: `local n = 0` is `integer`, `0.0` is `number`; opt into the
  `explicit-number` lint to be told where an annotation is missing.

## What is deliberately not here

- No Teal fork: `tl.lua` is vendored verbatim (0.24.8, MIT) and swapped as a file.
- No token-level formatting: `htl fmt` recomputes indentation and whitespace only.
- No Luau: PUC Lua 5.4 / LuaJIT via mlua features; bundles are bound to the Lua
  generation of the `htl` that built them.
- `.d.tl` files come from Rust source syntactically (`htl dts`, and the macros at
  expansion time write the same text). There is no reflection on types: a field of
  type `Foo` is declared as `Foo` and it is on you that a Teal `Foo` exists.

## License

MIT OR Apache-2.0. Teal (`crates/htl/vendor/tl.lua`) is MIT, see `vendor/LICENSE.teal`.
