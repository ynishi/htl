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
| `htl build <entry.tl \| dir> -o app.hb [-m main] [--debug] [--source] [--extra a,b] [--host x,y] [--no-cache] [--explain-cache]` | link the entry's `require` closure into one bundle (see Bundles), replaying from the run cache what still holds (see Caching; the directory form, whose entry module `-m` names, is not cached); a bundle is the `hb` target, so a project whose `[build] target` is anything else — `bin`, `cdylib`, `window` — is refused (see Layout of a project) |
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
| `htl dts [dir]` | write the `.d.tl` files this project declares: from Rust source, the ones `#[host_module]` / `#[derive(TealRecord)]` ask for, no build needed; from Teal, the module each `---@contract` type is declared in; from the crate graph, the ones a dependency ships (`[package.metadata.htl] dts`) into `types/<crate>/`. Every command that reads the project does this first (`check` / `run` / `test` / `build` / `fix` / `unused` / `resolve` / `gen`); exits non-zero when something it was asked to write could not be — a crate's declaration, or a `---@contract` type that cannot be published — never on a file it only [left in place](https://docs.rs/htl/latest/htl/dep_dts/index.html), and with nothing to write at all (no Cargo package above, no contract type) it is an error |

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
raised, and the same for every frame that reached it:

```text
runtime error: ./depth.tl:8: attempt to index a nil value (local 'c')
stack traceback:
	[C]: in metamethod 'index'
	./depth.tl:8: in function 'depth.field'
	./depth.tl:12: in function 'depth.describe'
	boom.tl:3: in main chunk
```

Stripped bytecode — a bundle without `--debug`, `include_tl_bytes!` — has no frames to
give; the `.tl` is still there, and `htl run src/main.tl` has them. A host whose users
did not write the Teal shows them `htl::user_message(&err)` instead
([`developer_message`](https://docs.rs/htl/latest/htl/fn.developer_message.html)).

## Caching

`htl check` stores what it worked out under `.htl/cache/` at the project root and replays
what still holds; the summary says how much (`[cached]`, `[36/48 cached]`). `htl build`,
`htl test` and the macros read the same store. `--no-cache` turns it off and
`--explain-cache` (`HTL_CACHE_DEBUG=1`) says why a lookup missed; `--cache-mode` /
`[cache] mode` picks `per-module` (the default) or `whole-run`; `htl cache status` and
`htl cache clear` look after the store. What makes an entry and what invalidates it:
[`htl::cache`](https://docs.rs/htl/latest/htl/cache/index.html).

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

The reference is [docs.rs/htl](https://docs.rs/htl): its front page walks this example,
and the pages behind it are

- [`host_module`](https://docs.rs/htl/latest/htl/attr.host_module.html) — parameters,
  `errors = "return"`, `Option`, `UserDataRef`, `uses`, `async fn` (feature `async`);
- [`dts`](https://docs.rs/htl/latest/htl/dts/index.html) — what each Rust shape becomes
  on the Teal side (`#[derive(TealRecord)]`, `rename_all`, data enums as unions);
- [`Strict<T>`](https://docs.rs/htl/latest/htl/teal/struct.Strict.html) and
  [`Htl::strict_strings`](https://docs.rs/htl/latest/htl/struct.Htl.html#method.strict_strings)
  — the string boundary at run time;
- [`Htl::with_checker_lua`](https://docs.rs/htl/latest/htl/struct.Htl.html#method.with_checker_lua)
  — a host running Teal it did not write builds the `Lua` itself;
- [`dep_dts`](https://docs.rs/htl/latest/htl/dep_dts/index.html) — shipping a crate's
  `.d.tl` to its users (`[package.metadata.htl] dts`), and what `htl dts` reports;
- [`Htl::apply_project`](https://docs.rs/htl/latest/htl/struct.Htl.html#method.apply_project)
  — publishing a crate whose Teal has dependencies (`htl pkg patch`, committed);
- [`Project::for_host`](https://docs.rs/htl/latest/htl/model/struct.Project.html#method.for_host)
  and [`TealResolver`](https://docs.rs/htl/latest/htl/pkg/struct.TealResolver.html) —
  a host that serves modules at run time, mods and plugins included
  (`contract_resolvers`, `expect_type`);
- [`cexport`](https://docs.rs/htl/latest/htl/cexport/index.html) and
  [`ffi`](https://docs.rs/htl/latest/htl/ffi/index.html) — `#[c_export]`, a C ABI for
  a caller that is not Rust (feature `ffi`; `htl new --lib --target cdylib`);
- [htl-mq](https://docs.rs/htl-mq) — a window (`htl new --target window`).

**Both ways of holding Teal are in this repository, built and run on every commit**:
[`examples/`](examples/README.md) has `embed`, where `include_tl!`, `include_bundle!`,
`#[derive(TealRecord)]` and `#[host_module]` all meet in one binary, and `resolver`, where
nothing is embedded and `require` goes through mlua-pkg at run time. Its README says what
each one prints and which line of the output is the point.

## Lints (`htl check`, `include_tl!`)

Every rule has a level — `allow`, `warn` or `deny` — and a project sets it by name:

```toml
[lint.rules]
nil-index = "deny"        # this one stops the run
no-any = "warn"           # allow by default; see it while you migrate, without failing CI
"tl:hint" = "allow"       # quote a name with a `:` — TOML has no bare key for it
```

The same names go on the command line (`--lint nil-index=deny,no-any=warn,-tl:hint`) and
in `HTL_LINTS`; `[lint] strict` / `--strict` make every `warn` count as `deny`; a
trailing `-- htl: allow(nil-index)` silences one occurrence. `htl check --list-lints`
prints every rule with its default level, and
[`htl::lint`](https://docs.rs/htl/latest/htl/lint/index.html) is the reference: what
each rule catches, Teal's own warnings under `tl:*`, and the four markers
(`---@struct`, `---@sealed`, `---@extensible`, `---@nilable`) that turn a rule on for
a record or a function.

## Project config (`htl.toml`)

Every command that reads the project, and `include_tl!`, read the `htl.toml` at the
project root; flags and `HTL_LINTS` / `HTL_LINT` override it, and `htl new` writes a
commented one. The keys, with what each is for:

```toml
[toolchain]
htl = "0.8"               # the htl command this project expects; a mismatch is refused

[lint]
strict = true             # every warn counts as deny

[lint.rules]
nil-index = "deny"

[layout]                  # where this project's own files live: source / types / tests
source = "src"            # "." for a flat project

[check]
paths = ["mods"]          # extra dirs require() resolves from while checking

[imports]
mathx = "dep:mathx"       # a name the project and a dependency share: which one it means

[build]
target = "bin"            # what runs this project's output: hb (default), bin, cdylib, window

[[contract]]              # where this project accepts modules written outside it
dir = "mods"
```

[`htl::config`](https://docs.rs/htl/latest/htl/config/index.html) has the full sample
and every key; [`htl::model`](https://docs.rs/htl/latest/htl/model/index.html) is how a
file gets its name, which module owns it, and what a dependency may see;
[`htl::contract`](https://docs.rs/htl/latest/htl/contract/index.html) is `---@contract`
/ `---@required`, the shape a directory of modules from outside the program must have,
and what `htl` publishes for their authors.

### Which file a name resolves to (`htl resolve`)

```console
$ htl resolve mq
htl resolve mq: src/mq.d.tl

  order  file                  kind         status
  1      src/mq.d.tl           declaration  read
  2      types/mq.d.tl         declaration  shadowed by 1
  3      types/htl-mq/mq.d.tl  declaration  shadowed by 1  (shipped by htl-mq 0.2.0)

  answered by the project model
```

Every status, the host-provided and environment-provided answers, and the JSON form:
[`htl::resolve`](https://docs.rs/htl/latest/htl/resolve/index.html).

## Patched dependencies (`htl pkg patch`)

`htl pkg patch mathx` copies the dependency's package root into `patches/mathx/`, writes
`patch_dir` onto it in `mlua-pkg.toml`, and records the revision as `patch_base`; from
there the directory is the project's code, and `htl check` walks it. What is dropped,
what happens when the pin moves, and `--force`:
[`MluaProject::patch`](https://docs.rs/htl/latest/htl/pkg/struct.MluaProject.html#method.patch).

## The native modules (`std.*`)

```lua
local json = require("std.json")         -- typed via std/json.d.tl, inside the binary
local rows: {Row} = json.decode(text)    -- decode is generic: annotate the result
```

`json`, `env`, `path`, `time`, `string`, `validate`, `pretty` and `argparse`, from
[mlua-batteries](https://github.com/ynishi/mlua-batteries); every function raises, so a
result-style call is `pcall`. Under a Rust host, `h.install_std()?` in `preload` (feature
`std`, on by default). The rest is
[`htl::batteries`](https://docs.rs/htl/latest/htl/batteries/index.html).

## Tests

```lua
local t = require("htl.test")            -- typed via test.d.tl
t.describe("util.add", function()
   t.it("adds", function()
      t.expect(util.add({x=1,y=2}, {x=10,y=20})):to_equal({x=11,y=22})
   end)
end)
```

A test file is any `.tl` that loads the test library, wherever it is; `htl test` runs
each in a state of its own. Matchers, snapshots, the seeded `t.rng()`, `--coverage` /
`--lcov` / `--junit`, and the `--lib` contract for another library:
[`htl::testing`](https://docs.rs/htl/latest/htl/testing/index.html).

## Fixing (`htl fix`)

Some diagnostics carry a mechanical fix; `htl check` marks them `(fixable: htl fix)` and
`htl fix [paths]` applies the `safe` ones (`--unsafe` for the rest; `suggest` is shown
under `--diff` and never written). Which rules carry a fix, the `[fix]` keys, and the
flags: [`htl::fix`](https://docs.rs/htl/latest/htl/fix/index.html).

## Machine-readable output

`--format json` on `check`, `test`, `unused` and `resolve` prints one JSON document on
stdout, with the same exit code as the text form. The shapes:
[`htl_cli::report`](https://docs.rs/htl-cli/latest/htl_cli/report/index.html),
[`htl::unused`](https://docs.rs/htl/latest/htl/unused/index.html),
[`htl::resolve`](https://docs.rs/htl/latest/htl/resolve/index.html).

## Bundles (`htl build`)

`htl build src/main.tl -o app.hb` follows `require("<literal>")` from the entry and
links everything it reaches into one file of stripped bytecode (`--debug`, `--source`);
`htl run app.hb` runs it, `htl bundle info app.hb` says what it holds, and a Rust host
embeds the same thing with `include_bundle!`. Host-provided names, portability, the
store: [`htl::link`](https://docs.rs/htl/latest/htl/link/index.html) and
[`htl::bundle`](https://docs.rs/htl/latest/htl/bundle/index.html).

## Unused (`htl unused`)

`htl unused` reports a module nobody requires and a dependency declared and never used,
walked from the entries the project already declares (`main.tl`, tests, contracts,
`[build] extra`, what a Rust host embeds); exit 0 unless `--exit-non-zero-on-unused`.
[`htl::unused`](https://docs.rs/htl/latest/htl/unused/index.html).

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

`mlua-pkg.toml` names one dependency from the start, `htlx`
([htl-x](https://github.com/ynishi/htl-x), pure Teal collections), so the first step is
`htl pkg install`; `--no-x` leaves it out. `--target bin | cdylib | window` adds a Rust
host (`--embed` is `--target bin`; `htl init --target` for an existing project), and
the `htl` it pins is the one the binary was built with (`--htl main`, `--htl
path:<checkout>` to move it). What each target produces and what the scaffold writes
for it: [`BuildTarget`](https://docs.rs/htl/latest/htl/build_target/enum.BuildTarget.html)
and [htl-cli](https://docs.rs/htl-cli).

## Unions of records (`where`)

Teal refuses a union of two record types on its own; a `where self.kind == "…"` clause
on each record lifts it, `is` narrows, and `union-exhaustive` reports a chain that leaves
a variant untested ([`htl::lint`](https://docs.rs/htl/latest/htl/lint/index.html)). A
record cannot open with a field called `where`; `["where"]: <type>` works anywhere.

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
