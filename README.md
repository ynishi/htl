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
| `htl check [paths] [--strict] [--lint +rule,-rule] [--no-cache] [--cache-mode per-module\|whole-run]` | type-check; htl lints as `lint:` (advisory, `--strict` fails on them); what has not changed is replayed from `.htl/` (see Caching) |
| `htl run <file.tl \| app.hb> [args]` | check then execute; `require` of a `.tl` with type errors fails |
| `htl test [paths] [--filter s] [--lib mod]` | `*_test.tl` and `tests/**/*.tl`, one isolated state per file |
| `htl fmt [paths] [--check] [--indent N]` | whitespace formatter (indentation from the syntax tree, blank lines, trailing space) |
| `htl gen <file.tl> [-o out.lua]` | readable Lua, the escape hatch out of htl |
| `htl build <entry.tl> -o app.hb [--debug] [--source] [--extra a,b] [--host x,y]` | link the entry's `require` closure into one bundle (see Bundles) |
| `htl pkg <args>` | passthrough to `mlua-pkg` at the nearest `mlua-pkg.toml` root |
| `htl dts [dir]` | write the `.d.tl` files declared by `#[host_module]` / `#[derive(TealRecord)]` from Rust source, no build needed (`check` / `run` / `test` / `build` do this automatically when inside a crate) |

`mlua-pkg.toml` is detected by walking up from the file: vendored deps become
visible to the checker and to `run` / `test` / `build` automatically. When a
directory is given, `check` / `fmt` / `build` / `test` walk the project's own files only:
`target/`, `node_modules/`, `.mlua-pkgs/` (or wherever `MLUA_PKG_DIR` points) and any
dot-directory are not entered, so dependencies' sources and tests stay theirs. A
directory passed explicitly is always walked. Files under `tests/` are checked with the
project root and `src/` on the search path, the same as `htl test`, so `htl check tests`
and `htl test` agree.

## Caching

`htl check` stores what it worked out under `.htl/cache/` at the project root and replays
whatever has not moved. The summary says how much: `[cached]` when everything came from the
store and no checker was built at all, `[36/48 cached]` when some of it did, and nothing
when none did. `--format json` carries the same as `summary.cached` and `summary.replayed`.
`htl init` puts `.htl/` in `.gitignore`; add it by hand in an existing project.

There are two separate controls. **Whether to cache** is `--no-cache`, which neither reads
nor writes. **How the cache is grained** is `--cache-mode`, or `[cache] mode` in `htl.toml`
with the flag overriding it:

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

An entry is used only when the module and everything it required still hash the same, every
directory a `require` could resolve in holds the same modules, and the binary that wrote the
entry is the one reading it. Content hashes throughout, no timestamps, so touching a file
without editing it invalidates nothing and a fresh checkout does not either. Anything
unexpected — a corrupt entry, an unreadable store, an htl upgrade — is a miss, which costs
the check it would have skipped and never the wrong answer.

Flags are part of the key when they change what a module reports and not when they only
change the verdict: `--lint` gets its own entries, `--strict` reuses them and differs in the
exit code alone.

`HTL_CACHE_DEBUG=1` prints why something was not replayed. `htl test` is not cached: a test
has to run whatever its types say.

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

`Result<T, E>` returns raise a Lua error on `Err` by default. With
`#[host_module(name = "store", errors = "return")]` they come back Lua-style instead:
`Ok(v)` -> `v, nil`, `Ok(())` -> `true, nil`, `Err(e)` -> `nil, tostring(e)`, and the
`.d.tl` says `function(...): T, string` (`boolean, string` for unit), so
`local ok, err = store:write(name, text)` needs no `pcall`.

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
wrong *type*; on its own it does not reject *missing* fields (every Teal record field is
nilable). Chain `.require_fields()` for contracts where every declared field is mandatory:
the module is then rejected at `require` naming the nil fields. Keep the default and
nil-guard on the host side when some fields are optional.

## Lints (`htl check`, `include_tl!`)

| rule | default | catches |
|---|---|---|
| `nil-index` | on | `t[k].x`, `t[k]:m()`, `t[k]()`, `t[k][j]` — Teal types a map/array lookup as `V`, not `V \| nil` |
| `enum-exhaustive` | on | `if e == "a" ... elseif e == "b" ... end` over an enum with a value left unhandled and no `else`; enums nested in records and enums from required modules count |
| `shadow-local` | on | a local / loop var / parameter reusing an enclosing local's name; when that outer local is a `require`d module the message says which module and where it was required |
| `no-global` | on | `global` declarations |
| `no-any` | off | explicit `any` annotations and `as any` casts |
| `explicit-number` | off | `local n = 0` (inferred `integer`) that is later assigned a number expression (`n = n * 1.5`, `n = a / b`): names the declaration and the assignment; write `local n: number = 0`. Plain integer counters are not reported |
| `class-record` | off | a record declaring metamethods (`metamethod __index: Actor` = a class): its metatable is attached by `setmetatable` at run time and is not part of the value, so serialization and the Rust boundary drop it; keep such records out of saved data and host signatures |
| `require-cycle` | on (project-level) | a loop in the require graph of the files `htl check <dir>` just checked, e.g. `a.tl -> b.tl -> a.tl`. Teal types the back edge as an opaque circular require, so without this the symptom is "cannot index" somewhere else |

Silence one occurrence with a trailing `-- htl: allow(nil-index)`. `include_tl!`
treats lints as errors (`HTL_LINT=warn` downgrades, `HTL_LINTS=+no-any,-shadow-local`
configures).

## Project config (`htl.toml`)

`htl check` / `htl test` / `htl fmt` / `include_tl!` all read the nearest `htl.toml`
above the file, so the CLI and the build agree. Flags and `HTL_LINTS` / `HTL_LINT`
override it (`htl new` writes a commented one).

```toml
[lint]
enable  = ["class-record", "explicit-number"]
disable = ["shadow-local"]
strict  = true            # lints fail check/test and include_tl!; false makes the macro advisory

[fmt]
indent = 3

[check]
paths = ["mods", "~/.cache/tsk/sdk"]   # extra dirs require() resolves from while checking

[[contract]]              # static form of TealResolver::expect_type / require_fields
dir = "mods"              # relative to htl.toml; "sites/*" = every subdirectory of sites/
type = "defs.Mod"         # every module directly under `dir` must return this record
require_fields = true     # ... with every declared field present in the returned table
exclude = ["modkit"]      # modules in `dir` not held to it (an SDK the host writes there)
# module = "Site"         # or: only this module name (in each dir) is held to it
```

`[check] paths` is for modules the host supplies at run time from somewhere the
checker would not look (an SDK cache, a mods dir): the CLI, `include_tl!` and
`contract_resolvers` all add them, plus the `htl.toml` dir, its `src/` and its
`types/`. `types/` is the conventional home for hand-written `.d.tl` (the
DefinitelyTyped shape: declarations the module's author did not ship, written by the
user), searched without any configuration; `htl new` creates it.

Source beats declaration: when both `defs.tl` and a `defs.d.tl` are reachable, the
checker reads the `.tl`, wherever the two sit on the path (Teal's own order is `.d.tl`
first). So a `.d.tl` a host writes out for external script authors never shadows the
source it was made from inside the repo, and a check that runs before the host has
rewritten it still sees the current types.

A `[[contract]]` adds two lints:

- `contract` — a module under `dir` whose return value is not assignable to `type`, or
  (with `require_fields`) whose returned table literal leaves a declared field out, is
  reported at `htl check` time instead of at the first `require`. The literal is found
  through `return { … }`, `return define({ … })`, `return { … } as T`, and
  `local m: T = { … } … m.f = … return m`.
- `contract-unenforced` — a contract is only a guarantee if the host enforces it. When a
  Cargo package is found, `htl check` scans its Rust sources for
  `expect_type("<type>")` (plus `.require_fields()` when required) or for the
  config-driven helpers below, and otherwise tells you what to add.

Hosts get resolvers from the same file, so the two cannot drift:

```rust
let (path, cfg) = htl::config::HtlConfig::find(Path::new("."))?.expect("htl.toml");
let mut reg = mlua_pkg::Registry::new();
for r in htl::pkg::contract_resolvers(&htl::parent_dir(&path), &cfg)? {
    reg.add(r); // TealResolver for <root>/mods with expect_type("defs.Mod").require_fields()
}
```

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

The split follows Go / Rust rather than Jest: htl invests in the **runner** and keeps
the assertion surface small enough to read in one screen. Matchers: `to_equal`,
`to_not_equal`, `to_be_truthy` / `to_be_falsy`, `to_be_nil` / `to_not_be_nil`,
`to_be_close`, `to_be_greater_than` / `to_be_less_than` / `to_be_at_least` /
`to_be_at_most`, `to_contain` / `to_not_contain` (substring or array element),
`to_match` / `to_not_match` (Lua pattern), `to_have_length`, `to_error`. A function returning two values is asserted with
`t.expect_all(f()):to_equal(false, "no door")` (`t.expect(f())` is a 2-argument call and
a type error; the message says so).

Snapshots: `t.expect(session.frame(s)):to_match_snapshot("first floor")` compares
the value with `tests/__snapshots__/<test file>/<name>.snap`. The first run writes the
file (and says so); later runs fail with a `-expected +actual` line diff when the value
changed; `htl test --update` rewrites the differing ones. A string is stored as is, an
array of strings as its lines (a rendered screen), anything else in a sorted,
one-entry-per-line form, so the files read well in a review. A name used twice in one
file is an error.

Coverage: `htl test --coverage` prints, per `.tl` module the tests' checks depended on,
how many of its statements ran (`executed/all  %`), and a total; a module no test
reached shows `0/n`. `--coverage-lines` adds the unexecuted line ranges. Statements
are counted from the `.tl` syntax tree and matched against Lua's line hook (Teal keeps
line numbers when it generates Lua), so the numbers are `.tl` lines. The hook slows
the run, and code that runs inside a coroutine the program creates is not seen.

Runner: `htl test [paths] [--filter substr] [--fail-fast] [-v | -q] [--slow MS]
[--update] [--coverage [--coverage-lines]]`. Each file runs in a fresh state; `-v` prints every test with its time,
`-q` only failures (with their details), errors and the summary line, `--slow 50` the
tests over 50 ms, `--fail-fast` stops at the first failure. The run has one checker
(`htl::testing::TestSession`) and one fresh program state per file: globals,
`package.loaded` and module state never cross files, while a module is type-checked
and generated once and served to every file whose search path resolves that name to
the same file (`Htl::with_checker` is the same split for hosts that run many
programs). `HTL_PROFILE=1` prints per-phase and per-file timings to stderr. Any library exposing
`run(filter, opts) -> {passed, failed, failures, tests?, snapshots_written?, snapshots_updated?}`
(and optionally `configure({snapshot_dir, update, mkdir})`) plugs in via `--lib`
(bring its `.d.tl`); files that use no such library pass if they run to completion.

## Fixing (`htl fix`)

Some diagnostics carry a mechanical fix; `htl check` marks them `(fixable: htl fix)`
and `--format json` carries the edits. `htl fix [paths]` applies them:

- Every fix has an applicability: `safe` (what the program does at run time is
  unchanged), `unsafe` (it may change; applied only with `--unsafe`), `suggest` (shown,
  never applied). Today: a forward reference gets its declaration inserted into the
  record (safe); `explicit-number` gets `: number` (safe); `no-global` becomes `local`
  (unsafe). `htl.toml` `[fix] unsafe = ["no-global"]` promotes a rule, `disable = [..]`
  turns its fix off; `--rule a,b` limits a run.
- The working tree is the undo. A file git reports as modified or staged is refused
  (`--allow-dirty`), and so is a file outside a repository (`--allow-no-vcs`).
  `--dry-run` reports without writing; `--diff` prints a unified diff per file instead.
- A file with a syntax error is never touched. Type errors elsewhere do not block (a
  fix is often what removes one); after each pass the file is re-checked and put back
  if it has more errors than before. Edits that overlap an applied one wait for the
  next pass; passes are capped at 4; two passes producing the same edits are reported
  as fixes undoing each other.
- Everything applied is listed (`fixed: file:line: rule (safe)`), as is everything
  skipped and why. Exit code as `htl check` (remaining errors → 1);
  `--exit-non-zero-on-fix` also fails when a file changed, for CI.

## Machine-readable output

`htl check --format json` and `htl test --format json` print one JSON document on
stdout and nothing on stderr (the text form is stderr-only, so the two never mix).
The exit code is the same as in text mode. Field names are stable; fields may be
added, not renamed.

- `check`: `{ files, diagnostics: [{ severity: "error"|"warning"|"lint", file, line,
  col, rule?, message }], summary: { errors, warnings, lints, strict, ok } }`. `rule`
  is the lint rule (`nil-index`, `contract`, ...), split out of the message.
- `test`: `{ files: [{ path, ok, diagnostics, error?, file_level, passed, failed,
  failures, tests: [{ name, ok, ms }], duration_ms, snapshots_written,
  snapshots_updated }], summary: { files, files_run, passed, failed, files_with_errors,
  duration_ms, ok }, coverage?: { modules: [{ path, executed, total, unexecuted:
  [[first, last]] }], executed, total } }` (`coverage` with `--coverage`).

GitHub Actions annotations from a check, for instance:

```sh
htl check . --format json | jq -r '.diagnostics[] |
  "::\(if .severity == "error" then "error" else "warning" end) file=\(.file),line=\(.line),col=\(.col)::\(.message)"'
```

## Bundles (`htl build`)

`htl build src/main.tl -o app.hb` follows `require("<literal>")` from the entry and
links everything it reaches into one file: `.tl` modules type-checked and generated,
plain `.lua` modules (vendored dependencies) as they are. A `require` that resolves only
to a `.d.tl` is recorded as **host-provided** (a Rust `#[host_module]`, a `preload`);
any other unresolved `require` is a build error, so "module not found" happens here
and not on the first `require` at the user's machine. `htl run app.hb` runs it; a host
does `Htl::run_bundle(&Bundle::decode(bytes)?, &args)` after registering its modules,
and is refused up front, naming them, if one is missing.

- Payload is stripped Lua 5.4 bytecode by default. `--debug` keeps line numbers and
  local names (tracebacks with lines; module names survive stripping since the loader
  supplies them). `--source` stores generated Lua instead: larger and readable, but
  loads on any Lua build.
- The bundle carries the compiling Lua's bytecode header (version, instruction /
  integer / number sizes, endianness). Lua's own version byte is `0x54` for every
  5.4.x, so this is what `install_bundle` checks, with a readable message on mismatch.
- A dynamic `require(expr)` cannot be followed: list its targets under `[build] extra`
  in `htl.toml` (or `--extra`). Modules the host provides without a `.d.tl` go under
  `[build] host` (or `--host`).
- Bundled modules are installed as `package.preload` entries, the same place a host
  puts its own (a name the host preloaded first is left alone: the host wins). So
  everything that defers to preload, a `.d.tl` stepping aside for the implementation
  or an mlua-pkg resolver over a mods dir, sees bundled modules too, and files on disk
  do not override the bundle.
- `htl build <dir>` (the older form) still bundles every `.tl` under a directory.

From Rust, `include_bundle!` does the same at `cargo build` and keeps the guarantee
`include_tl!` gives a single file: every linked `.tl` / `.lua` / `.d.tl` is tracked,
so an edit rebuilds, and a Teal type error anywhere in the closure fails the build.

```rust
const BUNDLE: &[u8] = htl::include_bundle!("src/main.tl", host = ["host"], extra = ["modkit"]);
// payload = "source" for cross-compiling (bytecode is produced by the build machine's
// Lua); debug = true keeps line numbers. [build] extra / host in htl.toml are merged in.
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

## Layout of a project (`htl new`)

```text
<name>/
├── mlua-pkg.toml          [package] entry = "src/<mod>"  → consumers require("<name>")
├── htl.toml               [lint] / [fmt] / [[contract]] shared by the CLI and include_tl!
├── src/<mod>/init.tl      the module (require("<mod>") from src/ and tests/)
├── types/                 hand-written .d.tl for host-provided modules (searched by default)
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
- **Forward references**: `function world.tick` calling `world.observe` that is
  defined further down is "invalid key 'observe' in record 'world'", because Teal adds
  a record's fields in source order. htl names the later definition and hands over the
  line to paste into the record (`observe: function(w: World, what: string)`), which
  also makes the record the module's declared API; moving the definition up is the
  other fix.
- **Multi-value call in last position**: `t.expect(can_cast(x))` with `can_cast`
  returning `boolean, string` is a 2-argument call, and Teal reports "wrong number of
  arguments" at `expect`. htl names the expanding call and the two fixes (bind first,
  or parenthesize to keep the first value).

## Releasing and using a local checkout

`docs/releasing.md`: the publish order, the crates.io per-crate 24-hour version limit
and what to do when it hits, and how a consumer runs against an unpublished htl with
`[patch.crates-io]` (all three crates, plus one `cargo update -p htl -p htl-core -p
htl-macros`, without which cargo keeps the locked version and warns that the patch
was not used).

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
