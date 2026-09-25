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

An exit code of 1 is a verdict — an error in a file, a finding at `deny`, a failing test,
a name that resolves to nothing. A command that could not get as far as a verdict — a
directory in no project, an `htl.toml` that does not parse, a file it cannot read, a flag
it does not take — says why and exits 2, whichever command it was.

Installed deps just work after `htl pkg install`. They go under `.htl/modules/`, beside
the check cache — htl decides that one location, and the installer is
[mlua-pkg](https://github.com/ynishi/mlua-pkg)'s library rather than its binary, so
there is no second process to agree with and nothing on `PATH` to install. `MLUA_PKG_DIR`
and a `target/` in the working directory, which the `mlua-pkg` binary reads, are not
consulted. Inside, `entries/<name>` is where `require("<name>")` looks (`src/<name>` for a
library `htl new --lib` wrote). *Vendored* is kept for the other thing: a copy of a
dependency committed to the repo, which a `target_dir` entry in the manifest declares.
When a directory is given, `check` / `fmt` / `build` / `test` do not enter `target/`,
`node_modules/`, `.mlua-pkgs/` or any dot-directory. A `target_dir` copy is not entered
either, and there the manifest is what says so: the copy sits in the repo under a name
the project chose, so nothing about the path tells it apart from the project's own code
beside it. `mlua-pkg install` rewrites it every time it runs — checking it would report a
dependency's errors as the project's, `htl fmt` would write a diff against upstream that
the next install undoes, and its `*_test.tl` are a dependency's suite (Go's `./...` has
excluded `vendor/` since 1.9 for the same reason). A `.tl` that belongs to no module of
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

`htl run` would refuse the module at that `require`; the check says so first. A file
under the project's `tests/` may read the project's sources and `tests/` itself, the same
as under `htl test`, so `htl check tests` and `htl test` agree; the project's sources
cannot `require` what is under `tests/`. `htl check <file>` works on a file in no
project; `htl check <dir>` with neither an `htl.toml` nor an `mlua-pkg.toml` above the
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

Nothing had to be mapped back. Teal keeps the input's line breaks when it generates Lua,
and htl loads every chunk under its source's own name, so a Lua frame is already a Teal
frame. `htl test` reports the same for a failing test and for a file that raises while
loading, and `--format json` carries the text unchanged in each file's `error` and
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
`htl init` puts `.htl/` in `.gitignore` — one line for the cache and the installed deps
beside it; add it by hand in an existing project.

`htl build` and the macros (`include_bundle!`, `include_tl!`, `include_tl_bytes!`) read
the same store, and `htl build` says so with the same `[cached]` / `[31/32 cached]`
suffix (the directory form of `build`, the older snapshot, is not cached). `build` takes
`--no-cache` and `--explain-cache`; for the macros, `HTL_NO_CACHE=1` turns the store off
and `HTL_CACHE_DEBUG=1` has them say how much they replayed or why they did not. On a
30-module, 16,000-line project (release CLI): a cold build 1.3 s; nothing edited 0.03 s;
one leaf edited 0.7 s, `[30/32 cached]` — the leaf and the entry that requires it are
generated, the rest replay; the module every other one requires edited, 1.3 s again,
since every entry read it.

**Whether to cache** is `--no-cache`. **How the cache is grained** is `--cache-mode`, or
`[cache] mode` in `htl.toml` with the flag overriding it:

| mode | entry | an edit costs |
|---|---|---|
| `per-module` (default) | one per module | that module, whatever requires it, and what those pull in |
| `whole-run` | one for the walk | the whole walk, wherever the edit landed |

Measured on a 57-module project of about 14,000 lines, release build, wall clock:

| | per-module | whole-run |
|---|---|---|
| cold (`--no-cache`) | 1.81 s | 1.81 s |
| nothing edited | 0.018 s | 0.017 s |
| a module nothing requires | 0.33 s | 1.75 s |
| a module 10 others require | 0.81 s | 1.75 s |
| the type module 32 others require | 1.74 s | 1.75 s |

**What per-module saves depends entirely on where the edit lands.** Editing a leaf is a
hundredfold; editing the module at the bottom of the dependency graph saves nothing at all,
because everything above it has to be checked again anyway. Neither mode is slower than a
cold check. `whole-run` keeps one entry per invocation rather than one per module, which is
the reason to reach for it if the number of files in `.htl/` becomes a problem before
eviction lands.

A level is in the key because it is written in the same spec as which rules run — moving
one rule between `warn` and `deny` changes no diagnostic, and re-checks anyway.

`htl test` shares the store for checking a test file and generating its Lua. The summary
says how many files had their checking reused (`27 checked from cache`), and `--no-cache`
opts out as it does for `htl check`. The modules a test requires are stored too. Measured
on a 27-file suite: 4.65 s without any of this, 5.03 s on the first run (which stores what
it generated) and 3.03 s on every run after.

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

`include_tl!` checks the file with the search path `htl check` gives it. In a project —
an `htl.toml` or `mlua-pkg.toml` above the file — that is the project's `[layout]`
directories and its dependencies, so a crate whose Teal lives in `scripts/` says
`[layout] source = "scripts"`. A crate with neither file is no project, and the file
reads the modules beside it.

`exec` passes its arguments to the script as `...` and nothing else. A script that reads
`arg[1]`, as `htl run` lets it, needs `h.set_arg("main.tl", &args)?` before `exec`: that
fills the `arg` table the way the `lua` CLI and `htl run` do, so the same `main.tl` runs
unchanged both ways (`htl new --embed` writes both calls).

A string that looks like a number is one only where the program says so, and that is
three layers. Checked Teal is the checker's: `"10" + 1` and `s * 2` are type errors, on
an `any` as much as on a `string`, and `tonumber(...)` is a `number` that no `integer`
accepts. What the checker did not see is Lua's — the far side of a cast, `(v as integer)
+ 1` where `v` is the `"10"` that `std.json.decode` or `arg` handed over; a function
`load` built from a string; Lua source the host gave `exec` — and Lua 5.4 reads
`"10" + 1` there as `11`. `h.strict_strings()?` in `preload` makes that an error. A
value that arrives as `any` is converted once, where it arrives, by the program:
`s:match("^%-?%d+$")` and `math.tointeger` when hex, an exponent and surrounding
whitespace are not wanted, `tonumber` when they are, and never a cast. The third layer, a
host's own argument, is `#[host_module]`'s: a parameter that may see a value from that
edge is a `Strict<T>` — `n: Strict<i64>` is declared `integer`; `Strict<String>`,
`Strict<bool>` and `Strict<f64>` are the same for their kinds; it derefs to `T`.

The second argument to `exec` is the chunk name: the name every frame of a run-time
failure inside that chunk is reported under. `@<path>` is a source location and prints as
the path, so `@scripts/main.tl` gives a reader something to open; `=<label>` is a bare
label, the honest answer for a module no file backs, which is how htl registers its own
test library as `=htl.test`.

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
rebuild of the macro's dependencies, then every build after). The `.tl` edit loop belongs
to `htl check` / `htl test` in any case — an edit to a leaf module costs a few
milliseconds from the cache — and `cargo build` to the Rust host and the binary.

A table `#[derive(TealRecord)]` converts that does not fit says which record, which
field, what the record declared and what arrived:

```text
Outcome.cause: expected string, got nil
Outcome.depth: expected integer, got string
Recording.outcome.cause: expected string, got nil
```

A Teal record literal may leave fields out and `htl check` is right to pass it, so this
message is the whole signal for that direction; a record marked `---@contract`, with
`---@required` on the fields that must be there, is the check-time counterpart when a
module's table is meant to be complete (see "Data from outside the program").

The derive takes more than a struct, so a host's closed sets and aliases reach Teal as
declarations rather than as `any`:

| Rust | Teal declaration | crosses as |
|---|---|---|
| `struct Point { x: f64, y: f64 }` | `record Point` | a table |
| `enum Mode { Fast, Careful }` | `enum Mode "Fast" "Careful" end` | the variant name, a string; any other string is refused: `Mode: expected one of "Fast", "Careful", got "fst"` |
| `enum Shape { Dot, Circle(f64), Rect { w: f64, h: f64 } }` | `record Shape_Dot`, `record Shape_Circle`, `record Shape_Rect`, each `where self.kind == "…"`, and `type Shape = Shape_Dot \| Shape_Circle \| Shape_Rect` | a table with `kind`; a newtype payload under `value`, struct fields under their names; `union-exhaustive` counts the variants, and a missing field reads `Shape.Rect.h: expected number, got nil` |
| `#[teal(rename_all = "snake_case")] enum State { Open, InReview }` | `enum State "open" "in_review" end` | the renamed word: `"open"` is accepted, `"Open"` is refused (`State: expected one of "open", "in_review", got "Open"`) |
| `struct Label(String)` | `type Label = string` | whatever the inner type crosses as |
| `Option<T>` | `T` as a field and as a return, `name?: T` as a method parameter | nil where the Rust side has `None`: a Teal record field is nilable already and a return position has no `?`, while the mark on a parameter is what lets a caller write `api:find("x")` |
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

`#[host_module]` turns the plain `impl` into a `mlua::UserData` impl and writes
`scripts/host.d.tl` when it expands, so `scripts/main.tl` sees
`host:scale(p: Point, k: number): Point` and `host.Point`. Change a Rust signature
and the next `cargo build` fails inside the `.tl` that relied on it. `&str`,
`&[T]` and `&Record` parameters are accepted (`&mut` is not). An `Option<T>` parameter is
declared `name?: T`, so a caller may leave that argument out (or pass nil) and the method
sees `None`; Teal parses the mark only on a trailing run of parameters, so an `Option`
with a required parameter after it is declared as the plain `T` and has to be passed.
Another host type comes
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
`htl.toml` (the message says `[build] host in htl.toml`) and `std.*` (`htl's std`). The
host's `.d.tl` is not an implementation: it is how the module is typed, and the check
reads it. Which of the two gives up the name is the project's decision — rename the
file, or stop providing the name. A file in no project is reported by the
`host-module-shadowed` lint instead (see "Lints").

### Your own `Lua`

`Htl::new` opens every standard library — `debug`, `io` and `os` included — on a state
it makes itself. That is the right state for `htl run`, for the checker, and for a host
running Teal it wrote. A host running Teal it did not write (a mods directory, a script
a user dropped in) decides what that Teal may reach, and decides it on the `Lua`: the
libraries at construction, the allocator's bound, the hook that counts instructions.
htl takes the state the host built and adds no limit of its own:

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

`Htl::from_lua(lua)` is the same for the shared form, where the checker runs on the
host's state too; that state then needs what the checker needs as well.

The checker state `Htl::new` makes uses `string`, `table`, `math` and `package`, and `os.getenv` and
`io.stderr` on its debug paths — and it is not the state a mod runs in.

The limits are mlua's, and so are their edges, which are worth knowing before relying
on one. An instruction hook fires only while Lua is executing Lua, so a host function
that blocks is one instruction; `set_global_hook` reaches the coroutines a script
starts, `set_hook` one thread. A thread has one hook, and a script with `debug` can
replace it — leave `debug` out of a state that runs Teal you do not trust. A memory
limit is checked after Lua's emergency collection, and `MemoryError` is what comes back.
`htl check` settles what a module *is*; what it may *do* is settled here, by the host.

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
dts` (and from `check` / `run` / `test` / `build` and the other commands that generate
before they work). Keep the files
current the way this repository does — the macro rewrites them, CI diffs them — and commit
them; they are what a consumer's checkout copies from, before anything of yours is built.

#### What `htl dts` reports, and what it exits on

`htl dts` says what happened to each declaration, one line each. The commands that generate
before they work (`check` / `run` / `test` / `build` / `fix` / `unused` / `resolve` / `gen`)
print only what moved, prefixed `dts:` — `dts: wrote …`, `dts: not written: …`, `dts: left
in place: …` — and never an `unchanged` line:

| line | meaning |
|---|---|
| `wrote <file>` | written now |
| `unchanged <file>` | already what it should be |
| `not written: <why>` | asked for and not written: a crate names a file in `[package.metadata.htl] dts` that is not a `.d.tl`, or is not in the package, or does not start at the `dts_root` that manifest declares, or names two that would be one file under `types/<crate>/`, or the file could not be written |
| `left in place: <file>` | under `types/<crate>/` from an earlier run, and not what is read now — the crate is gone from the graph, or still there and no longer naming the file, or one this binary carries itself ([`std.*`](#the-native-modules-std), whose copy an htl built without that feature may have written: the crate is still a dependency, and the line says so) |

With nothing to write at all — no `Cargo.toml` with a `[package]` above, and no
`---@contract` type — it is an error rather than a quiet success, and exits 2.

### Opening a window (`htl-mq`)

`htl new --target window` writes all of the below; this is what it writes, and why.

`htl new --target bin` writes a host whose script runs to completion. A game wants the
other shape — a window, a frame loop, input, drawing — and the part of that which is the
same for every project is the `htl-mq` crate: macroquad's drawing and input as a host
module `mq`, the loop that drives a Teal game table, and the declaration that `htl check`
reads. The project keeps its engine and its rules in Teal, and its own `#[host_module]`
beside `mq` for whatever wants the GPU.

```toml
[dependencies]
htl = "0.8"
htl-mq = "0.8"          # macroquad comes with it, which is why it is not a feature of `htl`
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

`htl test` runs without a display: `require("mq")` resolves to the declaration, which
declares and does nothing, so an engine module that takes what it needs as arguments is
testable headless, and `main.tl` — the one file that calls `mq` — is not what a test
requires. Every `mq` method calls macroquad and panics without a window, which is the
other reason to keep the loop out of the engine.

`HTL_MQ_FRAMES=60` stops after sixty frames, and `HTL_MQ_SHOT=out.png` writes the last
frame drawn as a PNG. `run` reads them; `run_with` takes a `Hooks` instead, and
`Hooks::NONE` turns them off. A machine with no
display fails before the first frame (`XOpenDisplay() failed!` on Linux); `xvfb-run`
is enough to get the PNG out of one.

Not in `htl-mq`: textures and audio (they need asset paths, which is a host decision),
and the web target (macroquad's wasm path and mlua's are different targets).

### Publishing a crate that embeds Teal

A crate whose Teal has no dependency needs nothing said here: the `.tl` is in the package
because `src/` is, and the macro reads it where cargo puts it. A crate whose
`mlua-pkg.toml` names a dependency ships one thing more — the dependency itself, as a
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

- **The executor is yours.** mlua yields to whatever is polling and provides nothing of
  its own, so an async method runs under `call_async` (which creates the coroutine for
  you) or an `AsyncThread` you drive. Called from a plain `load(..).eval()` there is
  nothing to suspend, and Lua raises rather than blocking.
- **The receiver is borrowed across every await.** `add_async_method` hands over a
  `UserDataRef<T>` that the future holds until it resolves, so nothing else may take the
  value exclusively meanwhile. Prefer `&self` over `&mut self`.
- **The future must be `'static`**, and `Send` as well when mlua's `send` feature is on.

The feature is off by default: it turns on mlua's `async`, and a host with no async
method should be built as it was without it.

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

Native modules must be registered *before* the Teal resolver and described by a `.d.tl`
for the checker.

`Htl::apply_config`, `Htl::add_path`, `Htl::add_package_path` and a `TealResolver` over one
directory (`TealResolver::new(dir)`, `.holding_packages()`) are still there, for a host
that does not describe a project; they part ways with the resolver (#320).

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
(plus `"staticlib"` for Unity on iOS). `cargo build` writes `include/game.h` and the
library exports `game_*` and nothing else.

**What the ABI promises.**

| C | Rust | |
|---|---|---|
| `const char *` | `&str` / `String`, or any serde type as JSON | borrowed for the call; free it when you like afterwards |
| `char *` | a `String` or a serde type returned | **ours**: hand it back to `game_free`, always |
| `int` | a status, never a value | `GAME_OK` and friends |
| `int *` | the out-parameter an `i32` result is written through | so no function returns three meanings in one `int` |
| `game_handle *` | the opaque handle | from `game_open`, to `game_close` |

Any other signature is a compile error naming the type and this set: a `bool` parameter,
a struct by value, a float, an integer of another width, a generic or an `async fn` does
not build, rather than building and going wrong on the far side.

**The status enum**, as `int`: `OK` 0, `ERR` 1, `BAD_HANDLE` 2, `NOT_FOUND` 3, `LUA` 4,
`PANIC` 5, `WRONG_THREAD` 6, `INTERRUPTED` 7. `game_last_status()`, `game_last_error()`
and `game_last_error_into(buf, len)` carry the failure of the last call.

**Conventions the generated code fixes**, so a project does not decide them again:
`game_open` takes one JSON object — pass absolute paths, a seed and names in it rather
than expecting the library to read the environment or the working directory; records
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

A comment silences the names it lists and no others, which matters where two rules land on
one line: a local over a required module is both a redeclaration and the thing
`shadow-local` reports, at the same position, so a line that wants both quiet says
`-- htl: allow(tl:redeclaration, shadow-local)`. The two say different things about that
line — one that a name is shadowed, the other which module it was — which is the whole of
why both are still reported there and nowhere else.

For `include_tl!` and `include_bundle!`, `HTL_LINTS=no-any=warn,-shadow-local`
configures which rules run and at what level, as `--lint` does for the command.
`HTL_LINT=deny` makes every finding fail the build; `HTL_LINT=warn` lets the build
through whatever the levels say.

`htl build` judges the bundle's closure the same way, by the same `[lint.rules]` — a rule
the project turned off is not reported there either — and writes no bundle when the
closure fails.

Everything `htl` prints with an `[htl <rule>]` name is a finding about your code — a lint
of htl's own, or a warning the vendored compiler raised. `htl dts`'s `not written` and
`left in place` lines are the command reporting on the declarations it was asked to write,
not findings, and are in neither table: [What `htl dts`
reports](#what-htl-dts-reports-and-what-it-exits-on).

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
be absent. Adding an unmarked field makes the construction sites that predate it report,
which is the point — the default for a new field is mandatory, and `---@optional` is the
exception you write on purpose.

Growing a record that already has construction sites is that report arriving at all of
them at once, which is the moment a project either edits every site in one sitting or
takes the field back out. The way through is two steps, and the first is to add the field
with `---@optional` on it — a marker that says "not yet", where the ones above it say "not
always":

```tl
   ---@optional   -- new: remove once every site sets it
   color: string
```

Nothing is reported, so the field can land while the sites are still short. Fill them at
whatever pace the work allows and then delete the marker line: every site that still
leaves the field out is reported, and a clean check is what tells you the last one is
done. Deleting it early is how to read that list at any point in between — `htl check` names every site that is short,
and `htl fix --diff` spells the missing field into each one as a suggestion it never
writes (`color = htl_fixme("string")`, a call the checker refuses wherever it lands). That
is a checklist and a line to paste from, not the migration done for you; a field has no
honest default, which is why nothing fills it in.

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

This is a lint, not a type. The file stays valid Teal and other tooling ignores the
comment; use sites still see a nilable field. What it removes is the reason to guard, and
the doubt about whether a field was ever set. Data arriving from outside the program — a
mod's return value, a save file, a host — is a different question, and a record marked
`---@contract`, with `---@required` on its mandatory fields, is what checks that.

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

A test that compares a whole sealed value builds one, and is reported like anywhere else:
`t.expect(gate.judge("yes")):to_equal({ verdict = "yes", at = 1 })` writes a literal typed
as `gate.Judged` in a file that is not `gate.tl`, which is the rule working rather than
misfiring. Both ways through are ordinary. Either the assertion carries
`-- htl: allow(sealed-record)`, which says this literal exists to be compared and never
leaves the test, or the test asserts the fields it is about
(`t.expect(j.verdict):to_equal("yes")`), which builds nothing and says which field
differed when it fails.

Like `---@struct`, this is a lint and not a type: the file stays valid Teal, other tooling
ignores the comment, and what it adds is the one thing a run-time check cannot — that no
other code minted the value. It pairs with `---@struct` on the same record, which says
every field is set where this says who may set them; both report at the same site with
their own message.

### Records a table may carry more than (`---@extensible`)

Every Teal record is closed: a table typed as a record may not carry a key the record does
not declare. That is an error rather than a lint, so no allow comment and no `[lint]`
setting reaches it. The two markers above move a different boundary — `---@optional` and
`---@required` decide which *declared* fields a literal may leave out. `---@extensible` is
about the key the declaration has never heard of:

```tl
   record Mod              ---@contract ---@extensible
      name: string         ---@required
      monsters: {Monster}  ---@required
      factions: {Faction}
   end
```

A table built as a `Mod` may now set keys the record does not declare.

The marker goes where the record is **declared**, in both forms — trailing, or on a line
of its own above it — like `---@struct` and `---@sealed`. A record nested inside an
extensible one is not extensible by that; mark it too if it should be. On a contract
record it travels with the published declaration (`---@contract("mods") ---@extensible` in
`types/defs.d.tl`), so a mod author checking against what was published gets what the
declaring project has.

Three things it deliberately does not do:

- **The keys stay unreadable.** `m.extra` through the record type is still an error
  (`invalid key 'extra' in record 'm'`). The marker buys tolerance where a value is built
  and nothing else. A program that wants to *read* what it did not declare wants a map
  field — `extra: {string: any}` — which works today and needs nothing from htl. That is
  the right answer when the keys are to be used and the wrong one at a data boundary,
  since every producer then has to nest its extra keys under an agreed name, which is a
  change to the wire shape rather than to the type.
- **It changes nothing for an unmarked record**, which stays closed, as every record is
  today.

What it costs is one case, and it is worth knowing before you write the marker: a
misspelled **optional** field becomes silence. `colour` is no longer an unknown field, and
`struct-fields` has nothing to say because nothing is missing — the required case is still
caught, the optional case is not. That is the price of the marker rather than an
oversight. A near-miss heuristic here would fire on the very keys the marker exists to
allow, and a warning that is wrong whenever the marker is doing its job is worse than the
silence.

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
of its own above it. mlua-batteries (0.7.3, the version htl's `std` feature takes) writes it on
`path.parent` / `filename` / `stem` / `ext` and `env.get` / `home`, so a project using
`std.*` gets the rule without writing anything; `regex.find` / `captures` carry it too, in a
host that turns that module on (it is not in the default set htl carries, see `std.*`).

An unmarked function says nothing: no marker means *unknown*, not *nilable*, which is why
adding the rule is silent on a project until someone writes a marker or depends on a
declaration that has one.

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

`htl fix enum-table` fills a `{string: E}` table in — `name = "name"` per missing value,
laid out where the entries already there are. For `{E: T}` it reports and changes nothing:
what an entry maps to is not something a fix can invent.

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

`[toolchain] htl` is a cargo requirement (`"0.8"` = 0.8.x) on the *command*. The command
is what decides whether the project checks: three lints were added on one day and all
three are reported by default, so a project quiet under the release before them says
three new things under the release after — on unchanged sources, and fatally if it runs
`--strict`. A command outside the requirement is refused; htl installs nothing, so the
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
names is refused. Spelling does not get around it: `lib`, `./lib` and `./lib/.` are one
directory. `types` listed under `[check] paths` is accepted — the two make the same claim,
so the entry is redundant rather than wrong — and `[layout] tests` is compared with
nothing.

A file answers to one name: `src/util/util.tl` is `util.util`, not `util`, and a crate's
`types/htl-mq/mq.d.tl` is `mq`, not `htl-mq.mq`.

When two modules implement one name — the project's own `src/mathx.tl` and a dependency
`mathx`, or a module under `[check] paths` of the same name — `htl check` reports an
error at each file, naming both. Rename one of them. A `require` of such a name is an
error as well, naming both files — in the check, at run time and in a bundle, a `require`
in a plain `.lua` included — and so is one of a name a module implements twice
(`src/demo.tl` beside `src/demo/init.tl`). A dependency's own submodules are under its
name (`mathx.vec`).

`[imports]` settles a shared name without a rename:

```toml
[imports]
mathx = "dep:mathx"          # require("mathx"), require("mathx.vec"): the dependency's
mathx_local = "own:mathx"    # the project's own mathx, under a name of its choosing
```

A `dep:` entry makes the dependency's modules answer to `@<dependency>/<name>` —
`require("mathx")` in the project is generated as `require("@mathx/mathx")`, and the
dependency's own `require("mathx.vec")` as `require("@mathx/mathx.vec")`. An `own:`
entry makes the project's own module answer to `@/<name>` — `require("mathx_local")`
above is generated as `require("@/mathx")` — and only the project's own module can answer
that, so the dependency of the same name is set aside for it. A `dep:` entry naming a
dependency the project does not have is an error at `htl.toml`.

`[check] paths` is for modules the host supplies at run time from somewhere the
checker would not look (an SDK cache, a mods dir). `htl check`
does not search the `htl.toml` directory itself: a `.tl` there is the project's only when
`source = "."` says the sources are there. `types/` is searched without any
configuration; `htl new` creates it.

Four kinds arrive there, and `types/` below means the declaration root wherever
`[layout] types` puts it: every one of them is written there. The ones written by hand; the ones a Rust dependency ships; the
ones a Lua dependency published; and the ones for a library that published none of its
own. Only the first are anyone's to edit — the rest are copies, and a change to one
belongs in the crate or package it came from.

A Rust crate that registers a module in its user's Lua state names the declarations it
ships in its manifest (`[package.metadata.htl] dts = ["dts/mq.d.tl"]`, see "Embedding in
Rust"). `htl dts` — and `check` / `run` / `test` / `build` and the other commands that
generate before they work — writes each of those files to `types/<crate>/<file>` (`wrote
types/htl-mq/mq.d.tl`). A crate whose modules have a namespace names the directory its
paths start at (`dts_root`, see "Shipping the declaration to your users"). A note beside
them (`.htl-dts`) records which crate and version they came from.

Nothing is built to do it, and a project whose dependencies are already resolved and
fetched needs no network. A manifest edited since the last resolve is resolved again, which
writes `Cargo.lock` and may fetch — what the next `cargo build` would do anyway, and what
having the dependency's files on disk to copy from requires. Where the graph cannot be
resolved at all, that is reported and the committed copies go on being what the project
checks against. A hand-written `types/mq.d.tl` beside a shipped one is a
`duplicate-declaration` (the hand-written one is read), which is the message wanted when a
project upgrades a crate that has started shipping its own.

`htl types add <library>` takes one library's declarations from teal-types, as
`types/<library>/<module>.d.tl`; `socket/http.d.tl` stays `require("socket.http")`.
`htl pkg` and `htl types add` both write a `.src` note beside each file: what published
it, at which commit, and the path it had there.

Copying rather than searching the installed deps is what makes them survive a fresh clone:
`.htl/` is gitignored and empty until someone installs, `types/` is committed. A name
`types/` already has is reported and left alone (`--force` replaces it): two libraries
publishing a module of the same name is a real situation, and there is no registry to
arbitrate it with.

Between two *declarations* of one module there is only an order: the project's own comes
first, then its dependencies', then the declarations crates ship, then `[check] paths`,
and the first is the one read. `duplicate-declaration` reports it.

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

The rows are in the order a name is answered, by kind first: a source beats a declaration
wherever the two sit, so row 1 is not necessarily the earliest directory. A `.lua` under
a declaration reads `runtime, typed by <n>` rather than `shadowed`. A name the project
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

A name that resolves to nothing says so and exits non-zero, so a script can ask. `--format
json` carries the same rows ("Machine-readable output"). `htl.test` is answered too: no
file of the project implements it — `htl test` preloads the library into the state it
runs — and its declaration is one the binary carries and writes out for the checker, so
the one row is that `.d.tl`, read, and the header says `provided by the environment`.

### Data from outside the program (`---@contract`)

`htl.toml` says *where* modules arrive; the record says *what* they must be. Marking the
record is what makes the contract discoverable — a directory carries no evidence of which
of a project's records is the one its modules must satisfy.

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

The default is the opposite of `---@struct`'s, and each marker says which regime its
record is under: `---@struct` is about a record the program builds itself, where a new
field is mandatory unless marked `---@optional`; `---@contract` is about a value arriving
from outside, where a new field is optional unless marked `---@required`.

Both markers are about the fields the record declares. A module that sets a key it does
*not* declare — written against a newer SDK than this declaration is — is refused by the
checker, and [`---@extensible`](#records-a-table-may-carry-more-than----extensible) beside
`---@contract` is what allows it.

A bare `---@contract` inherits the directory from `htl.toml`, which is what a project with
one contract writes. `---@contract("plugins")` names its own, `---@contract(module = "S")`
narrows a directory to one module name, `---@contract(exclude = "defs modkit")` names
modules in the directory that are not held to it — a helper, an SDK the host writes there;
a `.d.tl` is never held to a contract and needs no listing — and any of these can be given
at once. `[[contract]] module` and `exclude` in `htl.toml` say the same two things there.

The module the contract type is declared in is what an outside author writes their
modules against, so htl publishes it: `types/defs.d.tl` here, alongside the `.d.tl` a
Rust host's `#[host_module]` writes, regenerated by `htl dts` and by every command that
reads the project (check / run / test / build / fix / unused / resolve / gen).
`---@contract(dts = "sdk/defs.d.tl")` sends it somewhere else. Commit the result,
the same as the Rust-generated ones: it is what makes a fresh clone check before anything
has been built.

What it writes is the declaring module with its bodies removed:

```tl
function defs.describe(m: Mod): string    -->    describe: function(m: Mod): string
   return m.name
end
```

Two lints follow:

- `contract` — a module under the directory whose return value is not assignable to the
  record, or whose returned table literal leaves a `---@required` field out, is reported
  at `htl check` time instead of at the first `require`. The literal is found through
  `return { … }`, `return define({ … })`, `return { … } as T`, and
  `local m: T = { … } … m.f = … return m`. A marker that cannot be turned into a contract
  — one naming no directory in a project whose `htl.toml` declares none or several, two
  markers claiming one directory, a marker on the record a module returns rather than on
  one inside it, a marker in a file at the project root, which is no module's — is
  reported here too.
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
because the manifest names it, so the dependency resolves in a clone that has never
installed anything and in the copy `cargo package` verifies
([Publishing a crate that embeds Teal](#publishing-a-crate-that-embeds-teal)). This is
the shape of Cargo's `[patch]` with a `path` source, and of Go's `replace` pointing at a
directory in the module tree. Removing `patch_dir` and the directory returns the
dependency to its fetched form at the next install.

**What is checked, and what is not.** `htl check` walks the copy; `htl fmt`, `htl fix`
and `htl test` do not touch it (what `htl check` reports there — and `htl fix` reports
it too, without fixing it — is fixed by hand). `.htl/modules` is not
descended into at all, patched or otherwise; its modules are checked through the
`require` that reaches them and their errors reported against the requirer, never offered
to `htl fix` (a fix there would go at the next install — patching is how a dependency is
edited). The criterion for walking is who writes the directory — one that install
regenerates (`target_dir`) is skipped, one that the project edits is checked.

Those extra files are counted apart, so the number does not move without saying why: a
check that reads a patched dependency ends `htl check: 15 file(s) + 10 in patched
dependencies, 0 error(s), ...`, the two adding up to every file the walk visited. A
project with no patch prints the one number, as before. Under `--format json` the whole
is `files` and the second half is `patched` ("Machine-readable output").

**Upgrading.** When the pin moves, every install says so until the patch is refreshed or
removed:

```text
  patch   patches/mathx is not in use (taken from 3f2a9c1, mathx is now at 8b07e44)
          carry the change forward: commit it, then `htl pkg patch mathx`
          drop it: remove patch_dir from mlua-pkg.toml and delete patches/mathx
```

`htl pkg patch` on an already patched dependency refreshes the copy; a directory with
uncommitted changes is refused, and `--force` discards them. Outside a repository the
question cannot be asked at all, and that is said rather than guessed at.

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
check`, `htl test` and `include_tl!` type them without the project holding a copy. Every
one raises on failure rather than returning `nil, err`, so a result-style call is
`pcall`, or a host module under `errors = "return"`. What `pcall` receives is a string,
one line — `json.decode: EOF while parsing an object at line 1 column 1`.

What version of the modules a script sees follows where the script runs. Under `htl run`
/ `htl test` it is the binary's, pinned like everything else the binary does by
`[toolchain] htl` in `htl.toml`. Under a Rust host it is the `htl` crate's, pinned by the
host's `Cargo.toml`: the `std` feature (on by default, off with `default-features =
false`) brings the crate in, and `h.install_std()?` in the host's `preload` — which
`htl new --target` writes — installs it. A host that leaves the call out has its scripts
typed against `std.*` and failing at the first `require`, which is the same standing
`htl.test` has always had in a host.

In a bundle, `std.*` is a host module: `htl build` files it with the modules the running
binary provides rather than trying to bundle Rust, and `htl run x.hb` preloads it before
the bundle starts.

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
Keep tests in a file of their own rather than in the module: a module that loads the
test library loads it wherever the module is required, including in the program that
ships it.

A matcher that is not one of these is a type error too — `invalid key 'to_be' in type
Expect<integer>` — with the list above appended.

A test builds values far more often than it asserts them, and what it builds is usually
one valid value with a single thing varied, so a record with a handful of fields and a
dozen tests is a dozen places that spell every field. Adding a field to it then means a
dozen edits, and under `---@struct` a dozen reports at once. A factory in a helper module
beside the tests turns those into one place — a table of defaults, and a parameter that
names what this test varies:

```tl
local record factory
   record Over          -- what a test varies, not the record itself
      id: string
      hp: integer
   end
end

function factory.make_def(over: factory.Over): defs.MonsterDef
   return {
      id = over.id or "rat",
      hp = over.hp or 3,
      color = "grey",
   }
end
```

`factory.make_def{ hp = 1 }` then reads as the one thing the test is about, and a field
added to `MonsterDef` is filled in the factory and nowhere else. The overlay is a record
of its own, listing the fields a test may vary — usually fewer than all of them — and it
has to be: typed as `MonsterDef` it would make every call a literal built as that record,
reported like any other construction site, which is the factory handing back exactly what
it was written to remove. No lint asks for any of this; it is one way of writing tests
among others, and it is here because the marker is what makes the cost of the other way
arrive all at once.

Snapshots: `t.expect(session.frame(s)):to_match_snapshot("first floor")` compares
the value with `__snapshots__/<test file's stem>/<name>.snap` in the test file's own
directory — `tests/__snapshots__/session_test/first_floor.snap` for
`tests/session_test.tl`, and beside the module for a test kept under `src/` — where every
run of characters in the name outside letters, digits, `-`, `.` and `_` is one `_`. The
first run writes the
file (and says so); later runs fail with a `-expected +actual` line diff when the value
changed; `htl test --update` rewrites the differing ones. A name used twice in one
file is an error.

Coverage: `htl test --coverage` prints, per `.tl` module of the project's own, how many
of its statements ran (`executed/all  %`), and a total. Under a module it names the
functions nothing entered:

```text
coverage: src/combat.tl      124/181   68.4%
          never ran: resolve_counter (61), flee_path (130)
```

`--coverage-lines` adds the unexecuted line ranges under those. Statements are counted
from the `.tl` syntax tree and matched against Lua's line hook (Teal keeps line numbers
when it generates Lua), so the numbers are `.tl` lines. The hook slows the run, and code
that runs inside a coroutine the program creates is not seen.

`--lcov coverage.info` writes the same run as an lcov tracefile, which is what Codecov,
Coveralls, GitLab, `genhtml` and editor gutters read; it implies `--coverage`, and the
table and `--format json` are unchanged. One record per module: `FN` / `FNDA`, `DA` per
line with a count of `1` or `0`, `LF` / `LH`, no `BRDA`; `SF` is relative to the project
root.

`--junit report.xml` writes the run as a JUnit XML report: Jenkins' JUnit plugin,
GitLab's report ingestion, and the GitHub Actions reporters all take this file. The flag
composes with `--filter`, `--seed` and `--format json`, and changes neither the text nor
the document.

Randomness: the runner seeds each file before it runs, prints the seed of every run, and
takes it back with `--seed`, so a test that draws is one whose failure can be looked at
again:

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
  a `{string: E}` lookup is missing (safe — the entry it adds is the identity mapping the
  table already states for every other value); `struct-fields` gets the fields the site
  leaves out, one entry each, in the order the record declares them, laid out where the
  entries already there are (**suggest** — what it writes is `hp = htl_fixme("integer")`);
  `no-global` becomes `local` (unsafe). `htl.toml` `[fix] unsafe = ["no-global"]`
  promotes a rule, `disable = [..]` turns its fix off; `--rule a,b` limits a run.
- **The names those three take** are every rule `htl check --list-lints` names, plus two
  that it does not: `forward-ref` and `tl:error`. A name from neither set is refused
  (`--rule forwardref` is an error, not a run that fixed nothing). `tl:error` was called
  `error` before; writing the old name says so.
- The working tree is the undo. A file git reports as modified or staged is refused
  (`--allow-dirty`), and so is a file outside a repository (`--allow-no-vcs`).
  `--dry-run` reports without writing; `--diff` prints a unified diff per file instead.
  A `suggest` fix is listed as skipped and its insertion printed under `--diff`, as a
  second diff headed `<file> (suggested)`, since nothing ever writes it.
- Everything applied is listed (`fixed: file:line: rule (safe)`), as is everything
  skipped and why. What is left is reported and judged as `htl check` reports and judges
  it; the summary line says which it was (`1 at deny`, `... under strict`). Like `htl check`,
  it first regenerates the `.d.tl` declarations the project publishes, `--dry-run` or not.
  `--exit-non-zero-on-fix` also fails when a file changed, for CI.

## Machine-readable output

`htl check --format json`, `htl test --format json` and `htl unused --format json` print
one JSON document on stdout and nothing on stderr. `htl resolve` is a report rather than
a run, so both of its forms go to stdout, as `cache status` and `bundle info` do. The
exit code is the same as in text mode.

- `check`: `{ files, patched, diagnostics: [{ severity: "error"|"warning"|"lint", file,
  line, col, rule?, message, fix?, required_by?, origin? }], summary: { errors, warnings,
  lints, denied, strict, ok, cached, replayed } }`. `fix` is the rewrite `htl fix` would
  apply, when the diagnostic carries one: `{ applicability: "safe"|"unsafe"|"suggest",
  edits: [{ line, col, end_line, end_col, text }] }` (see Fixing). `rule` is the lint
  rule (`nil-index`, `contract`, ...) or a Teal warning's kind (`tl:unused`, ...), kept
  apart from the message; on an error the checker raised it is the class `htl fix` files
  that error's fix under (`forward-ref`, `tl:error`), the same name `htl fix --format
  json` gives it. An error in a module the check reached through `require` has `file`
  set to that module and `required_by` to the file that required it; `origin` is
  `"dependency"` (installed under `.htl/modules`, a vendored copy, or the declarations a
  crate ships under `types/<crate>/`) or `"external"` (a `[check] paths` or contract
  directory), and absent for a file of the project's own, a patched dependency included.
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
`Htl::run_bundle(&Bundle::decode(bytes)?, &args)` after registering its modules, and is
refused up front, naming them, if one is missing.

- Payload is stripped Lua 5.4 bytecode by default, and stripping takes the traceback
  with it: every frame of a run-time failure reads `?`, with no line. `--debug` keeps
  the line numbers and the local names, and its frames read `depth:8` — the module the
  bundle knows, since a bundle holds modules rather than files. `--source` stores
  generated Lua instead: larger and readable, bound to no Lua build, and named the same
  way as `--debug`.
- **Portability.** `install_bundle` checks the Lua chunk header before the first
  `require` and refuses on mismatch, naming both sides: `compiled for Lua 5.4, format 0,
  4/8/8, little-endian by htl 0.1.19, but this host runs ... on htl 0.2.0`. `--source`
  is for a host the header refuses.
- `htl bundle info app.hb` prints what the file records — format version, the htl that
  built it, payload kind, the Lua the bytecode is for in the same words as the mismatch
  message, entry, modules, host-provided names — without creating a Lua state. That is
  what a build step checks in and a bug report pastes; `--format json` for the same. A
  `--source` bundle says its Lua is `any`; a format 1 bundle (`HTLB\x01`, before the
  fingerprint) says it was not recorded.
- A dynamic `require(expr)` cannot be followed: list its targets under `[build] extra`
  in `htl.toml` (or `--extra`). The names the host provides, and a name declared and
  nothing else, are left out of the bundle without being listed. `[build] host` (or
  `--host`) is for a name the project cannot see at all.
- Bundled modules are installed as `package.preload` entries, the same place a host
  puts its own (a name the host preloaded first is left alone: the host wins). So
  everything that defers to preload, a `.d.tl` stepping aside for the implementation
  or an mlua-pkg resolver over a mods dir, sees bundled modules too, and files on disk
  do not override the bundle.
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
through `Linked::bundle()` / `into_bundle()` (an `Err` lists every type error; `link`
itself returns `Ok` so the whole list can be shown, and never hands out a bundle with
a module missing), and emit `cargo:rerun-if-changed=<file>` for each of
`Linked::inputs()`. Name files, not the directory: cargo compares the mtime of the
path it is given, and editing a file inside a directory does not change the
directory's.

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
Deleting is nobody's business here either — `htl fix` applies mechanical rewrites, and
"this module is unreachable" is not one of those; the fix is a decision.

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

`htl build` is the first command that acts on what the key records. A bundle is the `hb`
target, so in a project whose target is any other — `bin`, `cdylib`, `window` — the build
says which target the project is, who
runs that output and which command builds it — `cargo build` — and writes nothing. The
record is a decision the project made rather than a note about itself; dropping
`[build] target` from `htl.toml` is how a project with Rust in it asks for a bundle anyway.

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

The host is a library with a thin binary on top, not a binary that happens to hold a
host. What a project grows — a second `#[host_module]`, an `extern "C"` layer, a window
loop, a Rust test — grows in `src/lib.rs`, and every entry point reaches it through
`preload`: `src/main.rs` calls `preload`, then runs the bundle it embedded at `cargo build`
— `include_bundle!("src/main.tl", …)`, the entry script and its `require` closure, handed
to `run_bundle` with the process arguments; nothing is read from a file at run time — and
another crate that embeds this one calls the same function. `--lib` means the project has
no entry script, so there is nothing for the binary to run and it is not written at all —
what is left is the library, which is the part someone else embeds.

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

What the callers have is a Lua `error()` and a Rust `Err` to handle — the generated
`greet` refuses an empty name in Teal and `reset` refuses a no-op in Rust — so both error
paths are in front of the reader rather than described.

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

Both generated declarations are committed, and the first command in a fresh clone is
`htl check .`: it writes `src/fx.d.tl` and copies `types/htl-mq/mq.d.tl` out of the
dependency, and `cargo build` reads the second of them when `include_bundle!` links `mq`.

`htl test` runs the engine with no display. For the window itself, `HTL_MQ_FRAMES=60
HTL_MQ_SHOT=out.png cargo run` stops after sixty frames and writes the last one as a PNG.

## Unions of records (`where`)

Teal refuses a union of two record types on its own:

```text
cannot discriminate a union between multiple table types: A | B
```

The refusal is about run time, not syntax: `is` narrows with a `type()` check, and two
records are both `table`. A record can supply its own discriminator with a `where` clause,
and then the union type-checks and `is` narrows it:

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

That "once" is the cost of the form, and it decides where the form belongs. One record
cannot answer to two tag values:

```text
cannot use argument 'self' multiple times in macroexp
```

So a type with seven tag values needs seven records, and it is worth writing them only
when the variants carry different data. Where several tags carry the *same* data, a union
buys nothing an enum field on one record does not already give: the branches are guarded
by `enum-exhaustive` either way, and the declarations are the only thing that grew.

A worked example from a project that decided against one. Its `Effect` has five fields and
seven tag values, but only four payload shapes among them — `power`, `power` + `damage`,
`status`, and nothing at all. As a union that is seven records, four of them structurally
identical, around thirty lines of declaration, to gain field safety at the one place it is
read. It stayed an enum plus a record, and that was the right call.

The question to ask is not "does this have a tag" — plenty of records do — but "do the
variants hold different things". When they do, the union pays for itself at every use
site. When they do not, the tag was already saying it.

A variant nobody handled is not a type error — an `is` chain that covers `A` and `B` and
falls through compiles, and goes on compiling when `C` joins the union — so the
`union-exhaustive` lint reports it. It reads the union's members from the checker rather
than from the tests, and stays quiet for a chain with an `else`, for a single `is` (that
is a guard, not a dispatch), and where every branch returns and code follows, which is the
`else` written differently. Those are the same exemptions `enum-exhaustive` makes.

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
  defined further down is "invalid key 'observe' in record 'world'", because Teal adds
  a record's fields in source order. htl names the later definition and hands over the
  line to paste into the record (`observe: function(w: World, what: string)`), which
  also makes the record the module's declared API; moving the definition up is the
  other fix.
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

All three, not one. `htl` re-exports `htl-core`, and the proc macros in `htl-macros` run
`htl-core` at expansion time, so patching only `htl` builds two versions of the same code
into one graph.

**The patch is ignored until the lockfile is updated.** `Cargo.lock` keeps the version it
already resolved, and cargo says so rather than switching:

```
warning: patch `htl v0.4.0 (...)` was not used in the crate graph
```

Run `cargo update -p htl -p htl-core -p htl-macros` once and the lock points at the local
paths. To go back once the version is published, delete the `[patch.crates-io]` block and
run the same `cargo update` again.

## What is deliberately not here

- No Teal fork: `tl.lua` is vendored verbatim (0.24.8, MIT) and swapped as a file.
- No token-level formatting: `htl fmt` recomputes indentation and whitespace only.
- No Luau: PUC Lua 5.4 / LuaJIT via mlua features. Bytecode bundles are bound to the
  Lua chunk header of the `htl` that built them, which every 64-bit little-endian host
  shares (see Bundles, Portability); there is no dual bytecode-plus-source payload,
  since shipping the source is what the bytecode form exists to avoid.
- `.d.tl` files are written syntactically, from Rust source (`htl dts`, and the
  macros at expansion time write the same text) and from Teal (the module a
  `---@contract` type is declared in). There is no reflection on types either way: a
  Rust field of type `Foo` is declared as `Foo` and it is on you that a Teal `Foo`
  exists, and a Teal signature is carried across as it was written.

## License

MIT OR Apache-2.0. Teal (`crates/htl-core/vendor/tl.lua`) is MIT, see
`crates/htl-core/vendor/LICENSE.teal`.
