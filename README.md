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
               ──#[c_export]──▶ extern "C" wrappers + host.h   (feature `ffi`: a caller that is not Rust)
```

## Install

```sh
cargo install htl-cli          # binaries: htl, cargo-htl  (so `cargo htl <verb>` works)
```

Or a prebuilt binary from the [latest release](https://github.com/ynishi/htl/releases/latest)
— macOS (Apple Silicon, Intel), Linux (aarch64, x86_64) and Windows (x86_64). The
installers put `htl` and `cargo-htl` in `$CARGO_HOME/bin`, where `cargo install` would:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/ynishi/htl/releases/latest/download/htl-cli-installer.sh | sh
brew install ynishi/tap/htl-cli
powershell -ExecutionPolicy Bypass -c "irm https://github.com/ynishi/htl/releases/latest/download/htl-cli-installer.ps1 | iex"
```

Those put one `htl` on a machine. Two projects asking for two — which `[toolchain] htl`
(see `htl.toml`) refuses by design, naming `cargo install htl-cli --version` as the way
out, a swap for the second project — hand the per-directory part to a version manager.
[mise](https://mise.jdx.dev) needs nothing from htl: its `cargo:` backend installs from
crates.io (the prebuilt binary when `cargo-binstall` is present), `github:` takes the
release asset, and either writes the pin into the project's `mise.toml` and switches on
`cd`:

```sh
mise use cargo:htl-cli@0.8.0          # mise.toml: "cargo:htl-cli" = "0.8.0"
mise use github:ynishi/htl@v0.8.0     # the prebuilt binary from the release
```

`htl new` writes that `mise.toml` (the `cargo:` form, at the version that wrote the
project) when the CLI is a published release, so a scaffolded project carries its pin
from the start; the file is inert without mise, delete it if that is not in use.

```toml
[dependencies]
htl = "0.8"                    # embedding: engine + proc macros in one import
```

| crate | role |
|---|---|
| `htl` | umbrella: re-exports `htl-core` and (feature `macros`, default on) the proc macros. Depend on this one. |
| `htl-core` | engine: `Htl`, lints, fmt, bundle, test runner, mlua-pkg resolver |
| `htl-macros` | `include_tl!` / `include_tl_bytes!` / `TealRecord` / `host_module` / `c_export`; generated code targets `::htl::` |
| `htl-cli` | the `htl` / `cargo-htl` binaries |

## CLI

| command | what it does |
|---|---|
| `htl new <name>` / `htl init [dir]` | scaffold: `mlua-pkg.toml`, `htl.toml`, `src/<mod>/init.tl`, `src/main.tl`, `tests/<mod>_test.tl`, `types/README.md`, `.gitignore`, README — and `mise.toml` when the CLI is a published release (`--lib` for no entry script, which leaves `src/main.tl` out; `--target <name>` for what will run the output, `--embed` being the shorthand for `--target bin`; `--htl <req>` for which htl the project depends on; `--no-x` for no `htlx` dependency) |
| `htl check [paths] [--strict] [--lint rule=level] [--list-lints] [--format json] [--no-cache] [--cache-mode per-module\|whole-run] [--explain-cache]` | type-check; htl lints as `lint:`, advisory at their default level and fatal at `deny` (`--strict` promotes every `warn` to `deny`; `--list-lints` prints every rule with its default level and exits); a module reached through `require` (an installed dep, a `[check] paths` dir) is checked with the file and its type errors are errors too, once per run, with the file that required it; what has not changed is replayed from `.htl/` (see Caching) |
| `htl run <file.tl \| app.hb> [args]` | check then execute; `require` of a `.tl` with type errors fails |
| `htl test [paths] [--filter s] [--lib mod] [--lint rule=level] [--fail-fast] [-v \| -q] [--slow ms] [--update] [--seed n] [--coverage [--coverage-lines]] [--lcov file] [--junit file] [--format json] [--no-cache] [--explain-cache]` | every `.tl` that loads the test library (`htl.test`, or `--lib`), one isolated state per file; checking is replayed from `.htl/`, the run never is (see Caching; the flags are under Tests) |
| `htl fix [paths] [--rule a,b] [--unsafe] [--dry-run] [--diff] [--allow-dirty] [--allow-no-vcs] [--exit-non-zero-on-fix] [--format json]` | apply the fixes diagnostics carry: the safe ones by default, `--unsafe` for the ones that may change what the program does (see Fixing) |
| `htl fmt [paths] [--check] [--indent N]` | whitespace formatter (indentation from the syntax tree, blank lines, trailing space) |
| `htl gen <file.tl> [-o out.lua]` | readable Lua, the escape hatch out of htl |
| `htl build <entry.tl \| dir> -o app.hb [-m main] [--debug] [--source] [--extra a,b] [--host x,y] [--no-cache] [--explain-cache]` | link the entry's `require` closure into one bundle (see Bundles), replaying from the run cache what still holds (see Caching; the directory form, whose entry module `-m` names, is not cached); a bundle is the `hb` target, so a project whose `[build] target` is anything else — `bin`, `cdylib`, `window` — is refused (see Build targets) |
| `htl bundle info <app.hb> [--format json]` | what a bundle records, without running it: format, the htl that built it, payload kind, the Lua its bytecode is for, entry, modules, host-provided names |
| `htl unused [paths] [--format json] [--exit-non-zero-on-unused] [--no-cache] [--explain-cache]` | the complement of the same closure: modules no entry reaches, and `[deps]` no reached module requires (see Unused) |
| `htl resolve <module> [path] [--format json]` | which file `require("<module>")` resolves to: every file of the project that answers to the name, which one is read, and which crate or dependency each came from (see `types/`); for a name the host provides, which source says so; exits 1 when the name resolves to nothing, to two implementations, or to a file under a name the host provides |
| `htl pkg install` | fetch every dependency `mlua-pkg.toml` declares into `.htl/modules/` and write `mlua-pkg.lock`; the deps' own `types/` are then copied into the project's (see `types/`) |
| `htl pkg add <name> <git> [--tag t \| --rev r \| --branch b] [--entry dir] [--target-dir dir]` | write the dependency into the manifest (`install` fetches it); a `patch_dir` the entry already declared is kept |
| `htl pkg update [name] [--dry-run] [--force]` | refresh dependencies and bump the pins that follow releases, then install |
| `htl pkg clean [--all]` | remove cached packages the lockfile no longer refers to, or the whole cache |
| `htl pkg patch <dep> [--force]` | take that dependency's source into `patches/<dep>/`, where the project owns it and install resolves it from (see Patched dependencies) |
| `htl types add <library> [--from dir] [--force]` | the declarations a library never shipped, from [teal-types](https://github.com/teal-language/teal-types), into `types/` with the commit they came from recorded beside each |
| `htl cache status [path] [--entries] [--format json]` / `htl cache clear [path]` | report what the store holds, or empty it (see Caching) |
| `htl dts [dir]` | write the `.d.tl` files this project declares: from Rust source, the ones `#[host_module]` / `#[derive(TealRecord)]` ask for, no build needed; from Teal, the module each `---@contract` type is declared in; from the crate graph, the ones a dependency ships (`[package.metadata.htl] dts`) into `types/<crate>/`. Every command that reads the project does this first (`check` / `run` / `test` / `build` / `fix` / `unused` / `resolve` / `gen`); exits non-zero when something it was asked to write could not be — a crate's declaration, or a `---@contract` type that cannot be published — never on a file it only [left in place](#what-htl-dts-reports-and-what-it-exits-on), and with nothing to write at all (no Cargo package above, no contract type) it is an error |

Exit code 1 is a verdict (an error in a file, a finding at `deny`, a failing test, a name
that resolves to nothing); 2 is a command that could not reach one (a directory in no
project, an `htl.toml` that does not parse, a flag it does not take).

Installed deps just work after `htl pkg install`. They go under `.htl/modules/`, beside
the check cache; the installer is [mlua-pkg](https://github.com/ynishi/mlua-pkg)'s
library. Inside, `entries/<name>` is where `require("<name>")` looks (`src/<name>` for a
library `htl new --lib` wrote). *Vendored* is kept for the other thing: a copy of a
dependency committed to the repo, which a `target_dir` entry in the manifest declares.
When a directory is given, `check` / `fmt` / `build` / `test` do not enter `target/`,
`node_modules/`, `.mlua-pkgs/`, any dot-directory, or a `target_dir` copy, which
`mlua-pkg install` rewrites. A `.tl` that belongs to no module of
the project is named once, with where it goes: `1 file(s) belong to no module of the
project and were not checked: stray.tl; move them under src/, or set [layout] source =
"." if the project root is where its modules are`. A project that keeps its `.tl` at the
root with no source directory is refused: `htl: 3 file(s) at the project root belong to
no module (lib.tl, lib_test.tl, main.tl): add [layout] source = "." to htl.toml, or move
them under src/`, exit 2. A type error in a dependency is reported with the dependency's
own path and the file that required it —

```text
error: .htl/modules/entries/mathx/init.tl:12:8: in local declaration: got string, expected number
  (required by src/geometry.tl)
```

`htl check <file>` works on a file in no project; `htl check <dir>` with neither an `htl.toml` nor an `mlua-pkg.toml` above the
directory is an error that says what it looked for — a directory is checked as a project,
and `htl init` makes it one.

A failure at run time names Teal, not the Lua htl generated — the file and the line that
raised, and the same for every frame that reached it. `htl run boom.tl`, where `boom.tl`
calls into `depth.tl`:

```text
runtime error: ./depth.tl:8: attempt to index a nil value (local 'c')
stack traceback:
	[C]: in metamethod 'index'
	./depth.tl:8: in function 'depth.field'
	./depth.tl:12: in function 'depth.describe'
	boom.tl:3: in main chunk
```

`htl test` prints the same, and `--format json` carries it in each file's `error` and
`failures`.

The exception is stripped bytecode, which is what a bundle holds by default and what
`include_tl_bytes!` embeds. A scaffolded Rust host (`htl new --target bin`), failing
inside its embedded module:

```text
runtime error: ?:-1: attempt to index a nil value
stack traceback:
	[C]: in metamethod 'index'
	?: in upvalue '?'
	?: in function 'sample.greet'
	src/main.tl:4: in main chunk
```

A bundle keeps the frames when built with `htl build --debug` (see Bundles). Running the
`.tl` gives them back too — `htl run src/main.tl`, the same failure:

```text
runtime error: src/sample/init.tl:9: attempt to index a nil value (local 'g')
stack traceback:
	[C]: in metamethod 'index'
	src/sample/init.tl:9: in upvalue 'title'
	src/sample/init.tl:14: in function 'sample.greet'
	src/main.tl:4: in main chunk
```

A host embedding htl in a program whose users did not write it shows them
`htl::user_message(&err)` instead (see Embedding in Rust).

## Caching

`htl check` stores what it worked out under `.htl/cache/` at the project root. The summary
says how much: `[cached]`, `[36/48 cached]`, or nothing when none was replayed.
`--format json` carries the same as `summary.cached` and `summary.replayed`.
`htl init` puts `.htl/` in `.gitignore`; add it by hand in an existing project.

`htl build` and the macros (`include_bundle!`, `include_tl!`, `include_tl_bytes!`) read
the same store, and `htl build` says so with the same `[cached]` / `[31/32 cached]`
suffix (the directory form of `build`, the older snapshot, is not cached). `build` takes
`--no-cache` and `--explain-cache`; for the macros, `HTL_NO_CACHE=1` turns the store off
and `HTL_CACHE_DEBUG=1` has them say how much they replayed or why they did not.

**Whether to cache** is `--no-cache`. **How the cache is grained** is `--cache-mode`, or
`[cache] mode` in `htl.toml` with the flag overriding it:

| mode | entry | an edit costs |
|---|---|---|
| `per-module` (default) | one per module | that module, whatever requires it, and what those pull in |
| `whole-run` | one for the walk | the whole walk, wherever the edit landed |

Which mode wins is on
[`cache::Mode`](https://docs.rs/htl/latest/htl/cache/enum.Mode.html).

A level is in the key because it is written in the same spec as which rules run — moving
one rule between `warn` and `deny` changes no diagnostic, and re-checks anyway.

`htl test` shares the store for checking a test file and generating its Lua. The summary
says how many files had their checking reused (`27 checked from cache`), and `--no-cache`
opts out as it does for `htl check`. The modules a test requires are stored too.

`htl cache status` says what the store holds — entries by kind, total size, how recently they
were used, and with `--entries` the files each one covers. `htl cache clear` empties it.

`--explain-cache` (or `HTL_CACHE_DEBUG=1`) prints why each lookup missed and one line at the
end with what the run did: hits, misses, entries written, entries evicted.

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
    h.exec(MAIN, "@scripts/main.tl", &[])?;         // frames read scripts/main.tl:<line>
    Ok(())
}
```

`include_tl!` checks the file with the search path `htl check` gives it; a crate whose
Teal lives in `scripts/` says `[layout] source = "scripts"`.

A script that reads `arg[1]` needs `h.set_arg("main.tl", &args)?` before `exec`
(`htl new --embed` writes both calls).

Where the checker did not see it — the far side of a cast, a `load` from a string, Lua
source the host gave `exec` — Lua 5.4 reads `"10" + 1` as `11`; `h.strict_strings()?` in
`preload` makes that an error
([`Htl::strict_strings`](https://docs.rs/htl/latest/htl/struct.Htl.html#method.strict_strings)).
A value that arrives as `any` is converted once, where it arrives (`s:match("^%-?%d+$")`
and `math.tointeger`, or `tonumber`), never by a cast. A host parameter
that may see a value from that edge is a `Strict<T>` — `n: Strict<i64>` is declared
`integer`; `Strict<String>`, `Strict<bool>` and `Strict<f64>` are the same for their
kinds; it derefs to `T`.

The macros run the checker — htl-core and the vendored Lua that hosts `tl` — inside the
proc macro, and the `dev` profile compiles a proc macro and its dependencies under
`[profile.dev.build-override]`, whose default is `opt-level = 0`. Left there, a
`cargo build` that touches a `.tl` runs the checker about three times slower than the
release-built CLI: on a 30-module, 16,000-line project, 3.8 s against 1.3 s for
`htl build` of the same closure, and 1.6 s once the host's `Cargo.toml` says

```toml
[profile.dev.build-override]
opt-level = 3
```

`htl new --embed` writes that section; add it by hand to a host that predates it (one
rebuild of the macro's dependencies, then every build after).

A table `#[derive(TealRecord)]` converts that does not fit says which record, which
field, what the record declared and what arrived:

```text
Outcome.cause: expected string, got nil
Outcome.depth: expected integer, got string
Recording.outcome.cause: expected string, got nil
```

A record marked `---@contract`, with `---@required` on the fields that must be there, is
the check-time counterpart (see "Data from outside the program").

What each Rust shape becomes:

| Rust | Teal declaration | crosses as |
|---|---|---|
| `struct Point { x: f64, y: f64 }` | `record Point` | a table |
| `enum Mode { Fast, Careful }` | `enum Mode "Fast" "Careful" end` | the variant name, a string; any other string is refused: `Mode: expected one of "Fast", "Careful", got "fst"` |
| `enum Shape { Dot, Circle(f64), Rect { w: f64, h: f64 } }` | `record Shape_Dot`, `record Shape_Circle`, `record Shape_Rect`, each `where self.kind == "…"`, and `type Shape = Shape_Dot \| Shape_Circle \| Shape_Rect` | a table with `kind`; a newtype payload under `value`, struct fields under their names; `union-exhaustive` counts the variants, and a missing field reads `Shape.Rect.h: expected number, got nil` |
| `#[teal(rename_all = "snake_case")] enum State { Open, InReview }` | `enum State "open" "in_review" end` | the renamed word: `"open"` is accepted, `"Open"` is refused (`State: expected one of "open", "in_review", got "Open"`) |
| `struct Label(String)` | `type Label = string` | whatever the inner type crosses as |
| `Option<T>` | `T` as a field and as a return, `name?: T` as a method parameter | nil where the Rust side has `None`; the mark on a parameter is what lets a caller write `api:find("x")` |
| `Vec<T>` / `&[T]` / `[T; N]` / `VecDeque<T>` / `HashSet<T>` | `{T}` | a table used as a sequence |
| `HashMap<K, V>` / `BTreeMap<K, V>` | `{K:V}` | a table keyed by `K` |
| `mlua::Value` / `serde_json::Value` | `any` | unchanged: the deliberate escape hatch |

A data-carrying enum is declared nested in the host module (`records = [Shape]`), where
its variant records are reachable as `host.Shape_Circle` for `is`; `#[teal(dts = ..)]` on
one is refused. `uses = [Name]` imports every kind with `local type Name =
require("Name")`.

`#[teal(rename_all = "..")]` on an enum takes serde's set — `lowercase`, `UPPERCASE`,
`PascalCase`, `camelCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`,
`SCREAMING-KEBAB-CASE` — and `#[teal(name = "..")]` on one variant overrides it.

`Result<T, E>` returns raise a Lua error on `Err` by default. With
`#[host_module(name = "store", errors = "return")]` they come back Lua-style instead, so
`local ok, err = store:write(name, text)` needs no `pcall`.

`#[host_module]` writes `scripts/host.d.tl` when it expands, so `scripts/main.tl` sees
`host:scale(p: Point, k: number): Point` and `host.Point`. An `Option<T>` parameter is
declared `name?: T`, so a caller may write `api:find("x")`. Another host type comes
in as `UserDataRef<T>` (`UserDataRefMut<T>` to mutate it, `UserDataOwned<T>` to keep it)
and is declared as `T`, the same name a method returning it declared; types from other
modules come in via `uses = [Name]`.

A `src/host.tl` beside `#[host_module(name = "host")]` is, in a project, an error
reported at every `require` of the name:

```text
src/main.tl:1:14: 'host' is provided by the host (#[host_module] in Cargo.toml's crate)
and also implemented by src/host.tl: the host's module is what runs, so this file would be
checked and never run — rename it, or stop providing the name
```

The same holds for a `src/host.lua`, and for the other two routes: `[build] host` in
`htl.toml` (the message says `[build] host in htl.toml`) and `std.*` (`htl's std`). A
file in no project is reported by the `host-module-shadowed` lint instead (see "Lints").

### Your own `Lua`

A host running Teal it did not write builds the `Lua` itself and hands it to
[`Htl::with_checker_lua`](https://docs.rs/htl/latest/htl/struct.Htl.html#method.with_checker_lua),
which says what htl needs from it and where mlua's limits end:

```rust
use htl::Htl;
use htl::mlua::{Lua, LuaOptions, StdLib};

let checker = Htl::new()?;                     // the checker keeps everything it needs
// SAFETY: a state that loads bundles has to accept binary chunks, which mlua's safe
// `new_with` refuses; this one loads only what the host hands it.
let lua = unsafe {
    Lua::unsafe_new_with(StdLib::ALL_SAFE ^ StdLib::OS ^ StdLib::IO, LuaOptions::default())
};
lua.set_memory_limit(8 << 20)?;                // mlua's: past it, an allocation is `MemoryError`
let h = Htl::with_checker_lua(&checker, lua)?; // the program runs here; `os` and `io` are nil
```

`Htl::from_lua(lua)` is the shared form.

### Shipping the declaration to your users (`[package.metadata.htl] dts`)

A crate that registers a module in someone else's Lua state names the declaration files
its macros write, in its manifest:

```toml
# your-crate/Cargo.toml
[package.metadata.htl]
dts = ["dts/mq.d.tl"]      # written by this crate's own #[host_module(dts = "dts/mq.d.tl")]
```

A crate with one declaration per module in one directory names the directory and a `*`
instead of the list: `dts = ["types/mine/*.d.tl"]`.

A crate whose modules have a namespace — one that registers `mine.thing` — says where
its paths start:

```toml
[package.metadata.htl]
dts_root = "types"
dts = ["types/mine/thing.d.tl", "types/other/log.d.tl"]
```

which lands at `types/<crate>/mine/thing.d.tl`.

Every project that depends on the crate then gets them under `types/<crate>/` from `htl
dts` (and from every command that generates before it works). Commit them.

#### What `htl dts` reports, and what it exits on

`htl dts` says what happened to each declaration, one line each; `check` / `run` / `test`
/ `build` / `fix` / `unused` / `resolve` / `gen` print only what moved, prefixed `dts:`:

| line | meaning |
|---|---|
| `wrote <file>` | written now |
| `unchanged <file>` | already what it should be |
| `not written: <why>` | asked for and not written: a crate names a file in `[package.metadata.htl] dts` that is not a `.d.tl`, or is not in the package, or does not start at the `dts_root` that manifest declares, or names two that would be one file under `types/<crate>/`, or the file could not be written |
| `left in place: <file>` | under `types/<crate>/` from an earlier run, and not what is read now — the crate is gone from the graph, or still there and no longer naming the file, or one this binary carries itself ([`std.*`](#the-native-modules-std), whose copy an htl built without that feature may have written: the crate is still a dependency, and the line says so) |

With nothing to write at all — no `Cargo.toml` with a `[package]` above, and no
`---@contract` type — it is an error rather than a quiet success, and exits 2.

### Opening a window (`htl-mq`)

`htl new --target window` writes all of the below.

```toml
[dependencies]
htl = "0.8"
htl-mq = "0.8"          # macroquad comes with it
```

```rust
use htl::bundle::Bundle;
use htl::mlua::Table;
use htl::{Htl, include_bundle};

// `mq` is htl-mq's: its declaration is `types/htl-mq/mq.d.tl`, which `htl dts` writes.
// `host` is this crate's `#[host_module]`, which the build knows without being told.
const MAIN: &[u8] = include_bundle!("src/main.tl", host = ["game", "mq"], debug = true);

fn main() -> anyhow::Result<()> {
    let h = Htl::new()?;
    game::preload(&h)?;                       // the project's own host module and Teal
    htl_mq::Mq.htl_preload(&h)?;              // `require("mq")`
    h.install_bundle(&Bundle::decode(MAIN)?)?;
    let game: Table = h.lua().load("return require('main')").eval()?;
    htl_mq::run(h, game, htl_mq::conf("game", 800, 600))
}
```

The entry script returns the game table:

```lua
local mq = require("mq")
local x = 40.0

return {
   update = function(dt: number): boolean
      x = x + 120 * dt
      return not mq:is_key_pressed("Escape")
   end,
   draw = function()
      mq:clear_background({r = 0.08, g = 0.08, b = 0.12, a = 1})
      mq:draw_circle(x, 300, 24, {r = 1, g = 0.6, b = 0.2, a = 1})
      mq:draw_text("fps " .. mq:fps(), 16, 32, 28, {r = 1, g = 1, b = 1, a = 1})
   end,
}
```

`HTL_MQ_FRAMES=60` stops after sixty frames, and `HTL_MQ_SHOT=out.png` writes the last
frame drawn as a PNG. `run` reads them; `run_with` takes a `Hooks` instead, and
`Hooks::NONE` turns them off; `xvfb-run` on a machine with no display.

Not in `htl-mq`: textures, audio, and the web target.

### Publishing a crate that embeds Teal

A crate whose `mlua-pkg.toml` names a dependency ships the dependency itself, as a
[patched copy](#patched-dependencies-htl-pkg-patch):

```bash
htl pkg patch htlx    # patches/htlx/ in the tree, patch_dir in mlua-pkg.toml
git add patches/htlx mlua-pkg.toml mlua-pkg.lock
```

That is the whole recipe. Nothing else is written by hand: no `[check] paths` pointing
at the copy, no `exclude` in `Cargo.toml`.

### `async fn` (feature `async`)

A method may be `async`, in the same `impl` as the sync ones:

```rust
#[host_module(name = "api")]
impl Api {
    pub fn seen(&self) -> u32 { self.calls }
    pub async fn fetch(&self, path: String) -> String { /* … */ }
}
```

Three things follow from mlua, not from htl:

- **The executor is yours**: an async method runs under `call_async` or an
  `AsyncThread` you drive.
- **The receiver is borrowed across every await.** `add_async_method` hands over a
  `UserDataRef<T>` that the future holds until it resolves, so nothing else may take the
  value exclusively meanwhile. Prefer `&self` over `&mut self`.
- **The future must be `'static`**, and `Send` as well when mlua's `send` feature is on.

The feature is off by default.

Runtime resolution through mlua-pkg. A host describes the directories it serves once, as
a project model (features `pkg` and `dts`):

```rust
use htl::model::{HostDir, Project, View};

let project = Project::for_host(root, &[
    HostDir::Modules("scripts".into()),     // scripts/a/b.tl is a.b
    HostDir::Packages("mods".into()),       // mods/mathx/mathx.tl is mathx, mods/mathx/sub.tl mathx.sub
    HostDir::Declarations("types".into()),  // types/htl-mq/mq.d.tl is mq
]);
h.apply_model(&project, View::Source)?;     // the checker: what every htl command uses

let mut reg = mlua_pkg::Registry::new();
reg.add(NativeResolver::new().add("host", |lua| { /* Rust table */ }));
reg.add(htl::pkg::TealResolver::from_project(&project)?); // .tl -> check + gen; .d.tl -> type-only table
reg.add(mlua_pkg::resolvers::FsResolver::new(root.join("scripts"))?);
reg.install(h.lua())?;
// or, with an mlua-pkg.toml (`find` is None without one):
// htl::pkg::MluaProject::find(dir).expect("mlua-pkg.toml").registry()?
```

`Htl::apply_config`, `Htl::add_path`, `Htl::add_package_path` and a `TealResolver` over one
directory (`TealResolver::new(dir)`, `.holding_packages()`) are still there, for a host
that does not describe a project; they part ways with the resolver (#320).

A module that exists only at run time is either declared (`Tasks.d.tl` in the host's
tree) or handed to the user as a typed constructor (`return tsk.define({ ... })`);
[`TealResolver`](https://docs.rs/htl/latest/htl/pkg/struct.TealResolver.html) has both
shapes.

A host chooses by audience: `htl::developer_message(&err)` returns the cause with the
frames below it; `htl::user_message(&err)` returns the innermost cause alone. The C ABI
takes the second (see below).

For mod / plugin directories, `TealResolver::new("mods")?.expect_type("defs.Mod")` holds
every served module to a record type; `.require_fields(["name", "monsters"])` names the
fields that must be present, and `.require_all_fields()` takes every declared field.
These settings go on a resolver per contract directory, registered before
`TealResolver::from_project`; a `[[contract]]` in `htl.toml` builds them for you
(`htl::pkg::contract_resolvers`).

### A C ABI for a host that is not Rust (feature `ffi`)

`#[host_module]` hands the Rust host to Lua. `#[c_export]` hands the same `impl` block to
a caller that is not written in Rust — a Unity script, a Swift app, a Python REPL — as a
C ABI.

```rust
use htl::{Htl, c_export, ffi};

pub struct Game { h: Htl, depth: i32 }

#[c_export(prefix = "game", header = "include/game.h")]
impl Game {
    // The opener: options as one JSON object, plus the flag `game_interrupt` sets.
    pub fn open(options: &str, interrupt: ffi::Interrupt) -> Result<Self, String> {
        let h = Htl::new().map_err(|e| e.to_string())?;
        interrupt.install(&h).map_err(|e| e.to_string())?;   // hook: stops a runaway mod
        Ok(Game { h, depth: 0 })
    }
    pub fn frame(&self) -> String { /* … */ }                // char *: the text
    pub fn state(&self) -> Frame { /* … */ }                 // char *: JSON, via serde
    pub fn key(&mut self, k: &str) -> Result<(), String> { } // int: a status
    pub fn depth(&self) -> i32 { self.depth }                // int status, value in `out`
}
```

Add `htl = { version = "…", features = ["ffi"] }` and `crate-type = ["rlib", "cdylib"]`
(plus `"staticlib"` for Unity on iOS).

**What the ABI promises.**

| C | Rust | |
|---|---|---|
| `const char *` | `&str` / `String`, or any serde type as JSON | borrowed for the call; free it when you like afterwards |
| `char *` | a `String` or a serde type returned | **ours**: hand it back to `game_free`, always |
| `int` | a status, never a value | `GAME_OK` and friends |
| `int *` | the out-parameter an `i32` result is written through | so no function returns three meanings in one `int` |
| `game_handle *` | the opaque handle | from `game_open`, to `game_close` |

**The status enum**, as `int`: `OK` 0, `ERR` 1, `BAD_HANDLE` 2, `NOT_FOUND` 3, `LUA` 4,
`PANIC` 5, `WRONG_THREAD` 6, `INTERRUPTED` 7. `game_last_status()`, `game_last_error()`
and `game_last_error_into(buf, len)` carry the failure of the last call.

**Conventions the generated code fixes**: `game_open` takes one JSON object; records
cross as JSON text; `GAME_ABI_VERSION` is the shape of the functions.

**Both ways of holding Teal are in this repository, built and run on every commit**:
[`examples/`](examples/README.md) has `embed`, where `include_tl!`, `include_bundle!`,
`#[derive(TealRecord)]` and `#[host_module]` all meet in one binary, and `resolver`, where
nothing is embedded and `require` goes through mlua-pkg at run time. Its README says what
each one prints and which line of the output is the point.

**A project of this shape is `htl new --lib --target cdylib <name>`** (see [The cdylib target](#the-cdylib-target---target-cdylib)):
the crate types, the feature, the `#[c_export]` block and — the part a reader of an ABI
actually needs — a caller in C and a caller in Python that do the round trip and free
what they are handed.

## Lints (`htl check`, `include_tl!`)

Every rule has a **level**: `allow`, `warn` or `deny`. The `default` column below is each
rule's level for a project that says nothing:

```toml
[lint.rules]
nil-index = "deny"        # this one stops the run
no-any = "warn"           # allow by default; see it while you migrate, without failing CI
"tl:hint" = "allow"       # quote a name with a `:` — TOML has no bare key for it
```

| rule | default | catches |
|---|---|---|
| `nil-index` | warn | `t[k].x`, `t[k]:m()`, `t[k]()`, `t[k][j]` — Teal types a map/array lookup as `V`, not `V \| nil` |
| `nil-return` | warn | the same four shapes over a call — `f(x).y`, `f(x):m()`, `f(x)()`, `f(x)[k]` — where `f` is declared `---@nilable`. Silent until a declaration carries the marker (see below) |
| `nil-return-unchecked` | allow | the local such a call was bound to, used as the base of a chain before any statement looks at it — `local d = f(x)` then `d:upper()`. One report per local, at the first use. Off by default: it is a flow question, and the shapes it gets wrong are the ones where something did check (see below) |
| `htlx-available` | allow | a `for i = 1, #t do` loop whose whole body is a function [htl-x](https://github.com/ynishi/htl-x) already has — `list.map`, `list.to_set`, `list.filter` — in a project that depends on htl-x. Silent in a project that does not. Off by default: what it reports is right, and the call it names is a library's (see below) |
| `struct-fields` | warn | a table built for a record marked `---@struct` that leaves out a field the record declares and `---@optional` does not exempt. Silent until a record carries the marker (see below). `htl fix` spells the missing fields at the site, as a suggestion it never applies |
| `sealed-record` | warn | a table built for a record marked `---@sealed`, or an `as` cast to one, outside the file that declares it — outside the functions the marker names, when it names any (`---@sealed(gate.judge)`). Silent until a record carries the marker (see below) |
| `enum-exhaustive` | warn | `if e == "a" ... elseif e == "b" ... end` over an enum with a value left unhandled and no `else`; enums nested in records and enums from required modules count |
| `enum-cast` | warn | `e as E` where `E` is an enum and the checker types `e` as `string`: `as` is erased, so the word enters the enum with nothing checking it. A string literal (`"open" as E`) and a value already typed as the enum are not reported (see below) |
| `enum-table` | warn | a table constructor whose declared type maps an enum (`{string: E}`, `{E: T}`) and that leaves a value of the enum out, or lists a word that is not one. An array of the enum (`{E}`) is a selection, not a mapping, and is not reported. `htl fix enum-table` fills a `{string: E}` one in |
| `union-exhaustive` | warn | `if x is A ... elseif x is B ... end` over a union with a variant never tested and no `else`. The variants come from the checker, so a chain that predates a variant is reported once the union gains it (see "Unions of records") |
| `shadow-local` | warn | a local / loop var / parameter reusing the name an enclosing scope bound to a `require`d module: the message names the module, where it was required, and that the module is unreachable for the rest of that scope. Shadowing an *ordinary* outer local is not this rule — it is `tl:redeclaration`, which reports the same line and column and says more about it (see below) |
| `no-global` | warn | `global` declarations |
| `no-any` | allow | explicit `any` annotations and `as any` casts |
| `explicit-number` | allow | `local n = 0` (inferred `integer`) that is later assigned a number expression (`n = n * 1.5`, `n = a / b`): names the declaration and the assignment; write `local n: number = 0`. Plain integer counters are not reported |
| `class-record` | allow | a record declaring metamethods (`metamethod __index: Actor` = a class): its metatable is attached by `setmetatable` at run time and is not part of the value, so serialization and the Rust boundary drop it; keep such records out of saved data and host signatures |
| `duplicate-declaration` | warn | two `.d.tl` for one module: an order decides which is read — the project's own first, then dependencies', crates', `[check] paths` — and nothing in either file says so. Names the one read and the one that was not (see "Project config") |
| `host-module-shadowed` | warn | for a file in no project (no `htl.toml` or `mlua-pkg.toml` above it): a `require` of a name a `#[host_module]` in the surrounding crate registers that resolved to a Teal file of that name: `package.preload` beats the path searcher at run time, so the file is what is checked and the host is what runs. Reported at the require, naming both. In a project the same state is an error rather than this lint (see "Embedding in Rust") |
| `contract` | warn | a module under a `[[contract]]` directory that does not satisfy the contract's type or its `---@required` fields, and a `---@contract` marker that cannot be turned into a contract or published (see "Data from outside the program") |
| `contract-unenforced` | warn | a contract the host never builds resolvers for, so it is documentation rather than a run-time guarantee. Say where the enforcement lives with `[[contract]] enforced_by` when the scan cannot see it |
| `require-cycle` | warn | a loop in the require graph of the files `htl check <dir>` just checked, e.g. `a.tl -> b.tl -> a.tl`. Teal types the back edge as an opaque circular require, so without this the symptom is "cannot index" somewhere else |

Teal's own warnings are named too, in a namespace of their own: see
[Teal's own warnings](#teals-own-warnings-tl) below.

Every name in either table takes a level: `--lint contract=deny`, `[lint.rules]
require-cycle = "allow"`, `HTL_LINTS=tl:unused=allow`, and `htl check --list-lints` lists
them all with their defaults. `htl fix` also takes two names of its own, `forward-ref` and
`tl:error`, for `--rule` and `[fix]` only. See [Fixing](#fixing-htl-fix).

`+rule` and `-rule` are the older spelling of `=warn` and `=allow`. Later entries win:

```
htl check src --lint nil-index=deny,no-any=warn,-tl:hint
```

`[lint] strict` and `--strict`: for this run, every `warn` counts as `deny`. A run's
summary says both: `0 error(s), 2 warning(s), 1 lint(s), 1 at deny`.

Silence one occurrence with a trailing `-- htl: allow(nil-index)`, at the line the
finding points at; Teal's kinds too: `-- htl: allow(tl:hint)`. There is one
exception: `contract-unenforced` points at the `---@contract` marker, and a marker owns
the rest of its line, so a comment there is read as an argument to it. Turn that one off
by name, or answer it with `enforced_by`.

Where two rules land on one line — a local over a required module is both
`tl:redeclaration` and `shadow-local` — a line that wants both quiet says
`-- htl: allow(tl:redeclaration, shadow-local)`.

For `include_tl!` and `include_bundle!`, `HTL_LINTS=no-any=warn,-shadow-local`
configures which rules run and at what level, as `--lint` does for the command.
`HTL_LINT=deny` makes every finding fail the build; `HTL_LINT=warn` lets the build
through whatever the levels say.

Everything `htl` prints with an `[htl <rule>]` name is a finding about your code;
`htl dts`'s lines are not ([What `htl dts`
reports](#what-htl-dts-reports-and-what-it-exits-on)).

### Teal's own warnings (`tl:*`)

Teal's own warnings are reported under their kind's name in the `tl:` namespace — as
`warning: src/a.tl:5:10: unused variable n: integer [htl tl:unused]`, and as
`"rule": "tl:unused"` in `--format json`:

| rule | default | catches |
|---|---|---|
| `tl:unused` | warn | a local, parameter, label or loop variable nothing uses |
| `tl:unread` | warn | a variable written and never read after |
| `tl:redeclaration` | warn | a declaration over a name already declared, naming the kind declared and the line and column of the one it shadows. This is where shadowing is reported, `shadow-local` having been narrowed to the one thing the compiler cannot say — that the shadowed name was a required module. It also sees two declarations in the *same* scope, which `shadow-local` never could |
| `tl:unknown` | warn | a variable the checker cannot resolve |
| `tl:branch` | warn | a test that can never hold, e.g. `x is B` where `x` has been narrowed out of `B` |
| `tl:hint` | warn | the compiler's suggestions: `.` where `:` was meant, `pairs` over an array, a `string.format` pattern that does not match its arguments, and more |
| `tl:debug` | warn | the checker reporting an ambiguity in what it inferred |

### Records built whole (`---@struct`)

`---@struct` on a record says every field it declares is set where it is built, except
the ones marked `---@optional`:

```tl
local record MonsterDef   ---@struct
   id: string
   hp: integer
   inflicts: Status       ---@optional
   ---@optional
   home: BranchId
end
```

Every table built as a `MonsterDef` must then set `id` and `hp`; `inflicts` and `home` may
be absent.

Growing a record that already has construction sites: add the field with `---@optional`
on it, fill the sites, then delete the marker line
([`lint.lua`](crates/htl-core/src/lint.lua), `struct-fields`):

```tl
   ---@optional   -- new: remove once every site sets it
   color: string
```

`htl fix --diff` spells the missing field into each site as a suggestion it never writes
(`color = htl_fixme("string")`).

When the key the literal sets is a near miss for the one it wants, the message names it:

```text
MonsterDef is built without color (the literal sets `colour`)
```

One edit counts as a near miss in any name, two once the name is at least eight characters
long.

Both marker forms work: trailing on the field's own line, or on a line of its own above
it. Every construction site counts
— a bare literal, an element of an array or map of that record, a literal passed as a
typed argument, and a function's `return`.

Data arriving from outside the program is `---@contract`'s question (see "Data from
outside the program").

### Records built where they are declared (`---@sealed`)

`---@sealed` says a record is built only in its declaring file, or only in the functions
the marker names:

```tl
local record gate
   record Judged        ---@sealed
      verdict: Verdict
      at: integer
   end

   ---@sealed(gate.open, gate.reopen)
   record Draft
      who: string
   end
end
```

A table built as a `Judged`, or an `as` cast to one, outside `gate.tl` is reported:

```text
`gate.Judged` is sealed: built only in gate.tl
```

Naming functions narrows it (`built only in gate.tl by gate.open or gate.reopen`).
`-- htl: allow(sealed-record)` keeps one site the project stands behind.

A test that compares a whole sealed value is reported too; it either carries
`-- htl: allow(sealed-record)` or asserts the fields it is about
(`t.expect(j.verdict):to_equal("yes")`).

### Records a table may carry more than (`---@extensible`)

`---@extensible` is about the key the declaration has never heard of
([`prelude.lua`](crates/htl-core/src/prelude.lua), `extensible_declared`):

```tl
   record Mod              ---@contract ---@extensible
      name: string         ---@required
      monsters: {Monster}  ---@required
      factions: {Faction}
   end
```

A table built as a `Mod` may now set keys the record does not declare.

On a contract record it travels with the published declaration (`---@contract("mods")
---@extensible` in `types/defs.d.tl`), so a mod author checking against what was
published gets what the declaring project has.

A program that wants to *read* what it did not declare wants a map field —
`extra: {string: any}`.

What it costs: a misspelled **optional** field becomes silence.

### Functions that may return nothing (`---@nilable`)

`---@nilable` says the first return value may be nothing:

```tl
   -- nil when there is no parent.
   parent: function(p: string): string      ---@nilable
   ---@nilable
   find: function(s: string, pat: string): string
```

What the marker buys is the `nil-return` lint over indexing the call itself:

```
lint: src/main.tl:4:19: call result may be nil at runtime: path.parent is marked ---@nilable; bind it to a local and nil-check first [htl nil-return]
```

`f(x).y`, `f(x):m()`, `f(x)()` and `f(x)[k]` are the four shapes. Silence one
occurrence with a trailing `-- htl: allow(nil-return)`.

The marker goes where the function is **declared**, in both forms — trailing, or on a line
of its own above it. mlua-batteries writes it on its own declarations, so a project using
`std.*` gets the rule without writing anything.

**Following the local (`nil-return-unchecked`, off by default).** A second rule reports
the first use of the local as the base of a chain before anything checks it:

```
lint: src/main.tl:5:8: 'd' may be nil at runtime: it comes from path.parent, which is marked ---@nilable, and nothing checks it before this [htl nil-return-unchecked]
```

A project that wants it writes it down:

```toml
[lint.rules]
nil-return-unchecked = "warn"   # or "deny" to fail the run on it
```

One occurrence is silenced with a trailing `-- htl: allow(nil-return-unchecked)`.

### A loop a dependency already has (`htlx-available`, off by default)

This rule reports three loops [htl-x](https://github.com/ynishi/htl-x) already has, each
the body of a `for i = 1, #t do` with nothing else in it:

| loop | call |
|---|---|
| `out[i] = f(t[i])`, or `out[#out + 1] = f(t[i])` | `list.map(t, f)` |
| `out[t[i]] = true` | `list.to_set(t)` |
| `if p(t[i]) then out[#out + 1] = t[i] end` | `list.filter(t, p)` |

```
lint: src/find.tl:16:4: this loop is list.map(rows, row_summary): htlx is a dependency of this project, and `require("htlx.list")` has it [htl htlx-available]
```

```toml
[lint.rules]
htlx-available = "warn"
```

The finding carries the rewrite as a `htl fix` suggestion, which `htl fix --diff` shows
and `htl fix` never applies.

### The string boundary of an enum (`enum-cast`, `enum-table`)

`enum-cast` reports `h.state as defs.State` when the checker types the value as `string`.
`-- htl: allow(enum-cast)` keeps a cast the project stands behind.

The hand-written answer is a table, and `enum-table` is what keeps it level with the enum:

```tl
local states: {string: defs.State} = {
   open = "open",
   assigned = "assigned",
   closed = "closed",
   missed = "missed",
   escalated = "escalated",
   withdrawn = "withdrawn",
}

local function stored_state(s: string): defs.State
   return states[s] or "open"   -- total: a word nobody knows falls to the default
end
```

`htl fix enum-table` fills a `{string: E}` table in; for `{E: T}` it reports and changes
nothing.

## Project config (`htl.toml`)

`htl check` / `htl test` / `htl fmt` / `htl fix` / `htl resolve` / `htl gen` / `htl run` /
`htl build` / `include_tl!` all read the `htl.toml` at the project root. Flags and
`HTL_LINTS` / `HTL_LINT` override it (`htl new` writes a commented one).

```toml
[toolchain]
htl = "0.8"               # the htl command this project expects; a mismatch is refused

[lint]
strict = true             # every warn counts as deny: fails htl check, htl fix,
                          # htl build and include_tl! (not htl test / run / gen)

[lint.rules]              # allow = not reported, warn = reported, deny = fails the run
nil-index = "deny"
class-record = "warn"     # allow by default: seen without failing the run
shadow-local = "allow"
"tl:hint" = "allow"       # a warning kind of the Teal compiler; quote the `:`

[fmt]
indent = 3

[layout]                  # where this project's own files live
source = "src"            # its .tl; "." for a flat project
types = "types"           # hand-written .d.tl for modules something else provides
tests = "tests"           # tests, and the helpers only tests may require

[check]
paths = ["mods", "~/.cache/tsk/sdk"]   # extra dirs require() resolves from while checking

[imports]
mathx = "dep:mathx"       # a name the project and a dependency share: which one it means

[build]
target = "bin"            # what runs this project's output: hb (the default when absent),
                          # bin, cdylib, window (see "Build targets")

[[contract]]              # where this project accepts modules written outside it
dir = "mods"              # relative to htl.toml; "sites/*" = every subdirectory of sites/
# module = "Site"         # optional: only this module name (in each dir) is held to it
# exclude = ["defs"]      # optional: modules in dir not held to it (a helper, an SDK)
```

`[toolchain] htl` is a cargo requirement (`"0.8"` = 0.8.x) on the *command*
([`ToolchainConfig`](https://docs.rs/htl/latest/htl/config/struct.ToolchainConfig.html)).
A command outside the requirement is refused; htl installs nothing, so the
answer is `cargo install htl-cli` — or, when another project on the same machine needs
another `htl`, a version manager (see Install). `htl new` leaves the key out; add it by
hand when a project should.

`htl check` prints one line when the `htl = "0.8"` in the project's `Cargo.toml` does
not admit the command running — `htl 0.8.0; Cargo.toml asks for htl 0.7.1 — the crate
and the CLI are meant to move together (cargo install htl-cli --version 0.7.1, or bump
the dependency)`. A warning and nothing more.

`[layout]` is where this project's own files live: `source` holds its `.tl`, `types`
holds the hand-written `.d.tl` for modules something else provides, and `tests` holds its
tests and the helpers only tests may `require` (see Tests). Each defaults to the
directory it names (`src`, `types`, `tests`). `"."` is a flat project, with the sources
beside `htl.toml`.

`[layout] source` naming the directory that `[layout] types` or a `[check] paths` entry
names is refused.

A file answers to one name: `src/util/util.tl` is `util.util`, not `util`, and a crate's
`types/htl-mq/mq.d.tl` is `mq`, not `htl-mq.mq`.

When two modules implement one name — the project's own `src/mathx.tl` and a dependency
`mathx`, or a module under `[check] paths` of the same name — `htl check` reports an
error at each file, naming both. Rename one of them. A dependency's own submodules are
under its name (`mathx.vec`).

`[imports]` settles a shared name without a rename:

```toml
[imports]
mathx = "dep:mathx"          # require("mathx"), require("mathx.vec"): the dependency's
mathx_local = "own:mathx"    # the project's own mathx, under a name of its choosing
```

A `dep:` entry makes the dependency's modules answer to `@<dependency>/<name>` —
`require("mathx")` in the project is generated as `require("@mathx/mathx")`, and the
dependency's own `require("mathx.vec")` as `require("@mathx/mathx.vec")`. An `own:`
entry makes the project's own module answer to `@/<name>` (`require("mathx_local")`
above is generated as `require("@/mathx")`).

`[check] paths` is for modules the host supplies at run time from somewhere the
checker would not look (an SDK cache, a mods dir). `types/` is searched without any
configuration; `htl new` creates it.

A Rust crate that registers a module in its user's Lua state names the declarations it
ships in its manifest (`[package.metadata.htl] dts = ["dts/mq.d.tl"]`, see "Embedding in
Rust"). `htl dts` — and `check` / `run` / `test` / `build` and the other commands that
generate before they work — writes each of those files to `types/<crate>/<file>` (`wrote
types/htl-mq/mq.d.tl`). A crate whose modules have a namespace names the directory its
paths start at (`dts_root`, see "Shipping the declaration to your users"). A note beside
them (`.htl-dts`) records which crate and version they came from.

How the crate graph is read, and what a lockfile does to it, is on
[`dep_dts`](https://docs.rs/htl/latest/htl/dep_dts/index.html).

`htl types add <library>` takes one library's declarations from teal-types, as
`types/<library>/<module>.d.tl`; `socket/http.d.tl` stays `require("socket.http")`.
`htl pkg` and `htl types add` both write a `.src` note beside each file: what published
it, at which commit, and the path it had there.

A name `types/` already has is reported and left alone (`--force` replaces it).

`duplicate-declaration` reports which of two declarations of one module was read
([`HtlConfig::search_paths`](https://docs.rs/htl/latest/htl/config/struct.HtlConfig.html#method.search_paths)
has the order).

### Which file a name resolves to (`htl resolve`)

`htl resolve <module>` answers which of these files is in effect, and what it hides:

```console
$ htl resolve mq
htl resolve mq: src/mq.d.tl

  order  file                  kind         status
  1      src/mq.d.tl           declaration  read
  2      types/mq.d.tl         declaration  shadowed by 1
  3      types/htl-mq/mq.d.tl  declaration  shadowed by 1  (shipped by htl-mq 0.2.0)

  answered by the project model
```

A `.lua` under a declaration reads `runtime, typed by <n>` rather than `shadowed`. A name the project
does not have is looked up on Lua's search path, and the report ends with `searched, in
order: …`.

A name the host provides — a `#[host_module]` in the crate around the project, `[build]
host`, `std.*`:

```console
$ htl resolve host
htl resolve host: provided by the host (#[host_module] in Cargo.toml's crate), typed by src/host.d.tl

  order  file           kind         status
  1      src/host.d.tl  declaration  read

  answered by the project model
```

A name the project has only a declaration for — no `.tl`, no `.lua`:

```console
$ htl resolve socket.http
htl resolve socket.http: types/socket/http.d.tl, provided by the environment (declared by types/socket/http.d.tl)

  order  file                    kind         status
  1      types/socket/http.d.tl  declaration  read

  answered by the project model
```

A `.tl` or `.lua` of the project under a name the host provides (see "Embedding in
Rust"): the header is that error, the file's row is `refused`, and the command exits 1.

```console
$ htl resolve host
htl resolve host: error: 'host' is provided by the host (#[host_module] in Cargo.toml's crate) and also implemented by src/host.tl: the host's module is what runs, so this file would be checked and never run — rename it, or stop providing the name

  order  file           kind         status
  1      src/host.tl    source       refused
  2      src/host.d.tl  declaration  read

  answered by the project model
```

A name that resolves to nothing exits non-zero; `--format json` carries the same rows
("Machine-readable output"). `htl.test` is answered too: no
file of the project implements it — `htl test` preloads the library into the state it
runs — and its declaration is one the binary carries and writes out for the checker, so
the one row is that `.d.tl`, read, and the header says `provided by the environment`.

### Data from outside the program (`---@contract`)

`htl.toml` says *where* modules arrive; the record says *what* they must be.

```tl
local record defs
   record Mod              ---@contract
      name: string         ---@required
      monsters: {Monster}  ---@required
      items: {Item}        ---@required
      factions: {Faction}
      npcs: {Npc}
   end
end
return defs
```

Every module under `mods/` must return a value assignable to `defs.Mod` and set the three
marked fields.

A module that sets a key the record does not declare is refused;
[`---@extensible`](#records-a-table-may-carry-more-than----extensible) beside
`---@contract` allows it.

A bare `---@contract` inherits the directory from `htl.toml`; `---@contract("plugins")`,
`---@contract(module = "S")` and `---@contract(exclude = "defs modkit")` name or narrow
it, as `[[contract]] module` and `exclude` do in `htl.toml`.

htl publishes the declaring module as `types/defs.d.tl` (`---@contract(dts =
"sdk/defs.d.tl")` sends it somewhere else), regenerated by `htl dts` and by every command
that reads the project. Commit the result.

What it writes is the declaring module with its bodies removed:

```tl
function defs.describe(m: Mod): string    -->    describe: function(m: Mod): string
   return m.name
end
```

Two lints follow:

- `contract` — a module under the directory whose return value is not assignable to the
  record, or whose returned table literal leaves a `---@required` field out; the literal
  is found through `return { … }`, `return define({ … })`, `return { … } as T`, and
  `local m: T = { … } … m.f = … return m`. A marker that cannot be turned into a contract
  is reported here too.
- `contract-unenforced` — the host never builds resolvers for the contract; say where the
  enforcement lives with `enforced_by` when the scan cannot see it.

Hosts build their resolvers from the same markers:

```rust
let (path, cfg) = htl::config::HtlConfig::find(Path::new("."))?.expect("htl.toml");
let mut reg = mlua_pkg::Registry::new();
for r in htl::pkg::contract_resolvers(&htl::parent_dir(&path), &cfg)? {
    reg.add(r); // TealResolver for <root>/mods, expecting the record marked
                // ---@contract for that directory and its ---@required fields
}
```

Enforcement the scan cannot see — a Lua-side validator that checks the table before the
host uses it, a resolver in a sibling crate, generated code, or one built by hand on
purpose — is named instead:

```toml
[[contract]]
dir = "mods"
enforced_by = "mods/_validate.lua"   # relative to htl.toml; ~ and absolute paths work
```

## Patched dependencies (`htl pkg patch`)

A dependency needs one line changed. `htl pkg patch mathx` copies its package root out
of the pinned revision and into `patches/mathx/`, writes `patch_dir = "patches/mathx"`
onto that dependency in `mlua-pkg.toml`, and records the commit it was taken from as
`patch_base` in the lockfile.

```text
  patched patches/mathx (mathx at 3f2a9c1)
  dropped .git, .github, .gitignore (the repository's, not the package's)
```

From there the directory is the project's code: edited, diffed, reviewed and committed
with git like anything else in the tree. The copy's entry directory is on the search path
because the manifest names it
([Publishing a crate that embeds Teal](#publishing-a-crate-that-embeds-teal)).

**What is checked, and what is not.** `htl check` walks the copy; `htl fmt`, `htl fix`
and `htl test` do not touch it
([`Purpose`](https://docs.rs/htl/latest/htl/model/enum.Purpose.html)). A check that reads
a patched dependency ends `htl check: 15 file(s) + 10 in patched dependencies, 0
error(s), ...`; under `--format json` the whole is `files` and the second half is
`patched` ("Machine-readable output").

**Upgrading.** When the pin moves, every install says so until the patch is refreshed or
removed:

```text
  patch   patches/mathx is not in use (taken from 3f2a9c1, mathx is now at 8b07e44)
          carry the change forward: commit it, then `htl pkg patch mathx`
          drop it: remove patch_dir from mlua-pkg.toml and delete patches/mathx
```

`htl pkg patch` on an already patched dependency refreshes the copy; a directory with
uncommitted changes is refused, and `--force` discards them.

## The native modules (`std.*`)

```lua
local json = require("std.json")         -- typed via std/json.d.tl, inside the binary
local str = require("std.string")
local pretty = require("std.pretty")

local rows: {Row} = json.decode(text)    -- decode is generic: annotate the result
print(pretty.dump({ name = str.trim(name), rows = #rows }))
```

`std.*` is [mlua-batteries](https://github.com/ynishi/mlua-batteries) — Rust modules
reached from Teal — under the namespace `std`: the `htl` binary carries the crate's
default set, `json`, `env`, `path`, `time`, `string`, `validate`, `pretty` and `argparse`,
preloads each as `std.<name>` (and the namespace table as `require("std")`), and `htl
check`, `htl test` and `include_tl!` type them without the project holding a copy. A
result-style call is `pcall`, or a host module under `errors = "return"`; what `pcall`
receives is one line — `json.decode: EOF while parsing an object at line 1 column 1`.

Under a Rust host the `std` feature (on by default, off with `default-features = false`)
brings the crate in, and `h.install_std()?` in the host's `preload` — which `htl new
--target` writes — installs it.

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
file is refused before it runs.

Matchers: `to_equal`,
`to_not_equal`, `to_be_truthy` / `to_be_falsy`, `to_be_nil` / `to_not_be_nil`,
`to_be_close`, `to_be_greater_than` / `to_be_less_than` / `to_be_at_least` /
`to_be_at_most`, `to_contain` / `to_not_contain` (substring or array element),
`to_match` / `to_not_match` (Lua pattern), `to_have_length`, `to_error`. A function returning two values is asserted with
`t.expect_all(f()):to_equal(false, "no door")`.

**A test file is a `.tl` that loads the test library** — `require("htl.test")`, or the
module `--lib` names — wherever it is and whatever it is called.

`tests/` (`[layout] tests`) is the project's place for tests and for helpers only tests
may reach; a helper is named by its path below it (`tests/common/fx.tl` is `common.fx`).

A matcher that is not one of these is a type error too — `invalid key 'to_be' in type
Expect<integer>` — with the list above appended.

Snapshots: `t.expect(session.frame(s)):to_match_snapshot("first floor")` compares the
value with `tests/__snapshots__/session_test/first_floor.snap` for
`tests/session_test.tl`; the first run writes the file, and `htl test --update` rewrites
a differing one. A name used twice in one file is an error.

Coverage: `htl test --coverage` prints, per `.tl` module of the project's own, how many
of its statements ran (`executed/all  %`), and a total. Under a module it names the
functions nothing entered:

```text
coverage: src/combat.tl      124/181   68.4%
          never ran: resolve_counter (61), flee_path (130)
```

`--coverage-lines` adds the unexecuted line ranges under those.

`--lcov coverage.info` writes the same run as an lcov tracefile, which is what Codecov,
Coveralls, GitLab, `genhtml` and editor gutters read (it implies `--coverage`). One
record per module: `FN` / `FNDA`, `DA` per line with a count of `1` or `0`, `LF` / `LH`,
no `BRDA`; `SF` is relative to the project root.

`--junit report.xml` writes the run as a JUnit XML report: Jenkins' JUnit plugin,
GitLab's report ingestion, and the GitHub Actions reporters all take this file.

Randomness: the runner seeds each file and prints the seed of every run; `--seed` takes
it back:

```text
htl test: seed 8014255196 (repeat with --seed 8014255196)
```

`t.rng()` is that stream, shaped like `math.random` (`rng()`, `rng(m)`, `rng(m, n)`);
`math.random` is the same stream, so a test already using it repeats too.

Runner: `htl test [paths] [--filter substr] [--lib MOD] [--lint rule=level] [--fail-fast]
[-v | -q] [--slow MS] [--update] [--seed N] [--coverage [--coverage-lines]] [--lcov FILE]
[--junit FILE] [--format json] [--no-cache] [--explain-cache]`. Each file runs in a fresh
state; `-v` prints every test with its time,
`-q` only failures (with their details), errors and the summary line, `--slow 50` the
tests over 50 ms, `--fail-fast` stops at the first failure. `HTL_PROFILE=1` prints per-phase and per-file timings to stderr. Any library exposing
`run(filter, opts) -> {passed, failed, failures, tests?, snapshots_written?, snapshots_updated?}`
(and optionally `configure({snapshot_dir, update, mkdir})`) plugs in via `--lib`
(bring its `.d.tl`); files that use no such library pass if they run to completion.

## Fixing (`htl fix`)

Some diagnostics carry a mechanical fix; `htl check` marks them `(fixable: htl fix)`
and `--format json` carries the edits. `htl fix [paths]` applies them:

- Every fix has an applicability: `safe` (what the program does at run time is
  unchanged), `unsafe` (it may change; applied only with `--unsafe`), `suggest` (shown,
  never applied). Today: a forward reference gets its declaration inserted into the
  record (safe); `explicit-number` gets `: number` (safe); `enum-table` gets the entries
  a `{string: E}` lookup is missing (safe); `struct-fields` gets the fields the site
  leaves out, one entry each, in the order the record declares them, laid out where the
  entries already there are (**suggest** — what it writes is `hp = htl_fixme("integer")`);
  `no-global` becomes `local` (unsafe). `htl.toml` `[fix] unsafe = ["no-global"]`
  promotes a rule, `disable = [..]` turns its fix off; `--rule a,b` limits a run.
- **The names those three take** are every rule `htl check --list-lints` names, plus two
  that it does not: `forward-ref` and `tl:error`. A name from neither set is refused
  (`--rule forwardref` is an error, not a run that fixed nothing). `tl:error` was called
  `error` before; writing the old name says so.
- A file git reports as modified or staged is refused (`--allow-dirty`), and so is a
  file outside a repository (`--allow-no-vcs`). `--dry-run` reports without writing;
  `--diff` prints a unified diff per file instead.
- Everything applied is listed (`fixed: file:line: rule (safe)`), as is everything
  skipped and why. What is left is reported and judged as `htl check` reports and judges
  it; the summary line says which it was (`1 at deny`, `... under strict`).
  `--exit-non-zero-on-fix` also fails when a file changed, for CI.

## Machine-readable output

`htl check --format json`, `htl test --format json` and `htl unused --format json` print
one JSON document on stdout and nothing on stderr; `htl resolve`, `cache status` and
`bundle info` print both forms on stdout.

- `check`: `{ files, patched, diagnostics: [{ severity: "error"|"warning"|"lint", file,
  line, col, rule?, message, fix?, required_by?, origin? }], summary: { errors, warnings,
  lints, denied, strict, ok, cached, replayed } }`. `fix` is the rewrite `htl fix` would
  apply, when the diagnostic carries one: `{ applicability: "safe"|"unsafe"|"suggest",
  edits: [{ line, col, end_line, end_col, text }] }` (see Fixing). `rule` is the lint
  rule (`nil-index`), a Teal warning's kind (`tl:unused`), or the fix class of an error
  (`forward-ref`, `tl:error`). `required_by` is the file whose `require` pulled a module
  in; `origin` is `"dependency"`, `"external"`, or absent for the project's own.
- `test`: `{ files: [{ path, ok, diagnostics, error?, file_level, passed, failed,
  failures, tests: [{ name, ok, ms }], duration_ms, snapshots_written,
  snapshots_updated }], summary: { files, files_run, passed, failed, files_with_errors,
  replayed, duration_ms, ok, seed }, coverage?: { modules: [{ path, executed, total,
  unexecuted: [[first, last]], never_ran?: [{ name, line }] }], executed, total } }`
  (`coverage` with `--coverage`; `never_ran` is absent when every function of the module
  ran).
- `unused`: `{ modules: [{ path, module? }], dependencies: [{ name }], entries: [{ path,
  module?, kind: "main"|"test"|"contract"|"build"|"host" }], summary: { considered,
  reached, entries, modules, dependencies, no_entry, check_errors, ok } }`.
- `resolve`: `{ module, read?, candidates: [{ order, path, dir, kind:
  "source"|"declaration"|"lua", status: "read"|"shadowed"|"runtime"|"ambiguous"|"refused",
  shadowed_by?, origin?: { kind: "crate"|"dependency"|"vendored"|"patched", name, version? } }],
  answered_by: "model"|"path", searched: [dir], provided_by?, error?, summary: { candidates,
  shadowed, ok } }`. `provided_by` is worded as the text header words it: `#[host_module]
  in Cargo.toml's crate`, `[build] host in htl.toml`, `htl's std`, or `declared by <the
  .d.tl>`.

GitHub Actions annotations from a check, for instance:

```sh
htl check . --format json | jq -r '.diagnostics[] |
  "::\(if .severity == "error" then "error" else "warning" end) file=\(.file),line=\(.line),col=\(.col)::\(.message)"'
```

## Bundles (`htl build`)

`htl build src/main.tl -o app.hb` follows `require("<literal>")` from the entry and
links everything it reaches into one file. A missing `require` is said once, and the
line says where to declare the module: in an `x.d.tl`, under `[build] host`, or under
`[build] extra` for a dynamic `require`. `htl run app.hb` runs it; a host does
`Htl::run_bundle(&Bundle::decode(bytes)?, &args)` after registering its modules.

- Payload is stripped Lua 5.4 bytecode by default; `--debug` keeps the line numbers and
  the local names, `--source` stores generated Lua instead.
- **Portability.** `install_bundle` checks the Lua chunk header before the first
  `require` and refuses on mismatch, naming both sides: `compiled for Lua 5.4, format 0,
  4/8/8, little-endian by htl 0.1.19, but this host runs ... on htl 0.2.0`. `--source`
  is for a host the header refuses.
- `htl bundle info app.hb` prints what the file records — format version, the htl that
  built it, payload kind, the Lua the bytecode is for, entry, modules, host-provided
  names; `--format json` for the same.
- A dynamic `require(expr)` cannot be followed: list its targets under `[build] extra`
  in `htl.toml` (or `--extra`). The names the host provides, and a name declared and
  nothing else, are left out of the bundle without being listed. `[build] host` (or
  `--host`) is for a name the project cannot see at all.
- A second `install_bundle` writes nothing. `Htl::replace_bundle(&b, keep)` puts a
  newer bundle into a state that is already running; `replace_bundle(&b, &["world"])`
  keeps `world`'s evaluated table. The returned `Replaced` lists `dropped`, `kept` and
  `added`.
- `htl build <dir>` (the older form) still bundles every `.tl` under a directory.

From Rust, `include_bundle!` does the same at `cargo build`:

```rust
const BUNDLE: &[u8] = htl::include_bundle!("src/main.tl", extra = ["modkit"]);
// The host's names are the project's, as for `htl build`: `Host`'s `#[host_module]`, `[build]
// host` and `std.*` are not bundled. `host = [..]` is optional, for a module the project
// cannot see (registered by hand, or by another crate). payload = "source" for a target
// whose Lua header differs (big-endian, non-default number types; see Portability above);
// debug = true keeps line numbers. [build] extra in htl.toml is merged in.
Host { .. }.htl_preload(&h)?;
h.run_bundle(&htl::bundle::Bundle::decode(BUNDLE)?, &args)?;
```

Doing the same from a `build.rs` with `htl::link::link` works too: take the bundle
through `Linked::bundle()` / `into_bundle()`, and emit `cargo:rerun-if-changed=<file>`
for each of `Linked::inputs()`.

## Unused (`htl unused`)

`htl unused` reports a module nobody requires, and a dependency declared and never used:

```text
module: src/legacy/parser.tl (legacy.parser)
dependency: strx
htl unused: 1 module, 1 dependency [68 considered, 67 reached, 39 entries]
```

**Where it starts is not a guess.** The equivalent tools for JavaScript need a plugin
per framework to work out where a project starts; here the project has already said, in
the files `htl test`, `htl build`, `[[contract]]` and a Rust host are pointed at:

- `main.tl` in the source root (`[layout] source`, `src/` by default), or beside the
  manifest;
- every test file, as `htl test` discovers them — so a module used only by a test is
  reached, not reported;
- every module under a `[[contract]]` directory, nested ones included: those are loaded by name at run
  time, from a mods directory the project does not own. The `exclude`d ones too —
  `exclude` says a module is not held to the contract, not that nothing loads it;
- anything named in `[build] extra`, which is where a dynamic `require(expr)` already has
  to list its targets for `htl build` to bundle them, and every name the host provides —
  a `#[host_module]` in the crate around the project, `[build] host`, `std.*` — the names
  `htl build` leaves to the host;
- the file a Rust host embeds: the first argument of an `include_bundle!` / `include_tl!`
  / `include_tl_bytes!` in the crate around the project. A project whose `main` is in Rust
  has no `src/main.tl`, and its entry is named there and nowhere else.

The exit code is 0 whatever it finds, unless `--exit-non-zero-on-unused` says otherwise.
`htl fix` deletes nothing.

## Layout of a project (`htl new`)

```text
<name>/
├── mlua-pkg.toml          [package] entry = "src/<mod>"  → consumers require("<name>")
├── htl.toml               [fmt], and commented [lint] / [check] / [[contract]] to fill in;
│                          [build] target with --target. Read by CLI and macro
├── mise.toml              "cargo:htl-cli" = "<this CLI>": the command, for mise
│                          (written by a released CLI, not by a checkout build)
├── .gitignore             .htl/ (the cache and the installed deps) and *.hb; a target
│                          adds its own lines
├── README.md              the commands to run, and what the manifest's entry is for
├── src/<mod>/init.tl      the module (require("<mod>") from src/ and tests/)
├── types/README.md        .d.tl the project consumes (hand-written, and <crate>/ copied
│                          from a dependency) and publishes (a ---@contract type) go here
├── patches/<dep>/         a dependency taken into the tree (htl pkg patch), committed;
│                          checked, not formatted, its tests not run
├── src/main.tl            entry script (left out with --lib)
└── tests/<mod>_test.tl     a test (it loads htl.test); helpers beside it are not run
```

`mlua-pkg.toml` names one dependency from the start: `htlx`, the collections Lua does not
have ([htl-x](https://github.com/ynishi/htl-x) — `htlx.list` / `tablex` / `seq` /
`ordered`, pure Teal), pinned at an exact tag, so the README's first step is `htl pkg
install`. `htl new --no-x` leaves the line out.

A name that cannot be the directory, the package, the module, the record and the crate
at once is refused before anything is written: `htl new pub --embed` and `htl new end`
are refused by name, and `pubs`, `my-lib` and a plain (target-less) `htl new match` are
not.

### Build targets (`--target <name>`)

**A build target is what runs this project's output.**

| target | what runs the output | output | Rust in the project |
|---|---|---|---|
| `hb` (the default) — plain `htl new`, then `htl build` | the `htl` binary, `htl run app.hb` | a `.hb` bundle | no |
| `bin` | the OS, as a binary | a binary (library + a thin `main.rs`) | the user's crate |
| `cdylib` | a C / Python / Unity caller | `cdylib` + `staticlib` + a header | the user's crate |
| `window` | the OS, as a window | a binary that opens a window (library + `main.rs` calling `htl_mq::run`) | the user's crate, plus `htl-mq` |

`htl init --target bin` adds the Rust side to a project that predates it.

`[build] target` in `htl.toml` records what runs the output (absent means `hb`).
`htl new --target <name>` writes it.

#### Which htl the project depends on (`--htl <main | path:<checkout>>`)

A target that writes Rust writes a `Cargo.toml`, and the `htl` that manifest pins is the
one the `htl` binary was built with.

The same release, as the *command*, goes into `mise.toml` — `"cargo:htl-cli" = "0.8.0"`
(see Install). A checkout or `main` pin writes no such file.

`--htl main` pins `main` by hand, and `--htl path:<checkout>` another clone (the
checkout's *root*).

#### The bin target (`--target bin`)

`--target bin`, and `--embed` which is its shorthand, add a Cargo package to that tree:

```text
├── Cargo.toml             htl + anyhow, and [profile.dev.build-override] opt-level = 3
├── src/lib.rs             #[host_module] Host, its records, the embedded module,
│                          and pub fn preload(&Htl) registering both
├── src/host.d.tl          generated from src/lib.rs — by cargo build, and by
│                          htl dts / htl check without building
└── src/main.rs            the binary: preload, then the bundle of src/main.tl it embeds
                           (omitted with --lib)
```

#### The cdylib target (`--target cdylib`)

`htl new --lib --target cdylib <name>` is the same library with the C ABI on top, for a
caller that is not written in Rust:

```text
├── Cargo.toml             crate-type = ["rlib", "cdylib", "staticlib"], htl with
│                          features = ["ffi"], serde
├── src/lib.rs             the #[host_module] the scripts call, and a #[c_export] Game
│                          the caller holds: open / a text call / JSON / a status / close
├── include/<mod>.h        written by #[c_export] at cargo build (and by htl dts), committed
├── examples/c/            main.c + a Makefile: every returned char * goes back to _free
└── examples/python/       run.py: restype = c_void_p and ctypes.cast, never c_char_p
```

#### The window target (`--target window`)

`htl new --target window <name>` is the `bin` shape with the window on it, plus
[`htl-mq`](#opening-a-window-htl-mq):

```text
├── Cargo.toml             htl + htl-mq (under the same pin) + anyhow
├── src/lib.rs             #[host_module] Fx — the project's own GPU side — the embedded
│                          engine, and preload registering it, `fx` and htl-mq's `mq`
├── src/fx.d.tl            generated from src/lib.rs by cargo build / htl dts / htl check
├── src/<mod>/init.tl      the engine: balls in a box, pure rules, no window
├── src/main.tl            the game table htl_mq::run drives: update(dt), draw()
├── src/main.rs            the binary: preload, then htl_mq::run
└── types/htl-mq/mq.d.tl   the dependency's declaration, copied in by htl check
```

`htl test` runs the engine with no display. For the window itself, `HTL_MQ_FRAMES=60
HTL_MQ_SHOT=out.png cargo run` stops after sixty frames and writes the last one as a PNG.

## Unions of records (`where`)

Teal refuses a union of two record types on its own (`cannot discriminate a union
between multiple table types: A | B`); a `where` clause on each record lifts it:

```tl
local record Monster
   where self.kind == "monster"
   kind: string
   hp: integer
end

local record Item
   where self.kind == "item"
   kind: string
   weight: number
end

local function describe(e: Monster | Item): string
   if e is Monster then
      return "hp " .. tostring(e.hp)      -- e.weight here is an error
   else
      return "weight " .. tostring(e.weight)
   end
end
```

`where` takes an expression that uses `self` **once**; comparing an enum-typed tag field
works the same way and is the usual shape. Inside a narrowed branch the other variant's
fields are not in scope — reaching for one is `invalid key 'weight' in record 'e' of type
Monster` — and a partially narrowed value keeps its remaining variants, so after `is A`
over `A | B | C` the value is `B | C` and a field only `B` has is still an error.

One record cannot answer to two tag values (`cannot use argument 'self' multiple times
in macroexp`), so a union is worth its records only when the variants carry different
data; otherwise an enum field on one record is enough.

A variant nobody handled is reported by the `union-exhaustive` lint, with the same
exemptions as `enum-exhaustive`.

A record cannot open with a field called `where`; the quoted spelling works anywhere,
first line included:

```tl
local record FindArgs
   ["where"]: any     -- read back as args["where"]; args.where does not parse
   pkg: string
end
```

This is a Teal feature, not an htl one; it is documented here because the error above is
what a reader meets first, and it reads like a dead end rather than a pointer to `where`.

## Pitfalls the checker now names

- **Case-insensitive filesystems (macOS, Windows)**: `require("site")` from a file
  called `Site.tl` resolves to that very file; htl says so, and that one of the names
  has to change.
- **Numeric inference**: `local n = 0` is `integer`, `0.0` is `number`; opt into the
  `explicit-number` lint to be told where an annotation is missing.
- **Forward references**: `function world.tick` calling `world.observe` that is
  defined further down is "invalid key 'observe' in record 'world'"; htl names the later
  definition and hands over the line to paste into the record (`observe: function(w:
  World, what: string)`).
- **A union of two records**: "cannot discriminate a union between multiple table
  types" reads like a limit on the type system, and it is a limit on `is`, which each
  record can lift for itself with a `where` clause (see above).
- **A field called `where`**: only on a record or interface body's *first* line, where
  Teal reads the union predicate before the fields. The bare "syntax error" and the
  follow-on error on the next field's line become one message naming the keyword and the
  two ways out — `["where"]: <type>`, or any other field first (see above).
- **Multi-value call in last position**: `t.expect(can_cast(x))` with `can_cast`
  returning `boolean, string` is a 2-argument call; htl names the expanding call and the
  two fixes (bind first, or parenthesize to keep the first value).

## Running against an unpublished htl

A consumer that needs a change before it is on crates.io points at a checkout, in its own
`Cargo.toml`:

```toml
[patch.crates-io]
htl = { path = "/path/to/htl/crates/htl" }
htl-core = { path = "/path/to/htl/crates/htl-core" }
htl-macros = { path = "/path/to/htl/crates/htl-macros" }
```

All three, not one; then `cargo update -p htl -p htl-core -p htl-macros` once, and the
same command again with the block deleted to go back.

## What is deliberately not here

- No Teal fork: `tl.lua` is vendored verbatim (0.24.8, MIT).
- No token-level formatting: `htl fmt` recomputes indentation and whitespace only.
- No Luau: PUC Lua 5.4 / LuaJIT via mlua features, and no dual bytecode-plus-source
  payload (see Bundles, Portability).
- `.d.tl` files are written syntactically, from Rust source and from Teal (the module a
  `---@contract` type is declared in); there is no reflection on types either way.

## License

MIT OR Apache-2.0. Teal (`crates/htl-core/vendor/tl.lua`) is MIT, see
`crates/htl-core/vendor/LICENSE.teal`.
