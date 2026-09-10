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

```toml
[dependencies]
htl = "0.1"                    # embedding: engine + proc macros in one import
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
| `htl new <name>` / `htl init [dir]` | scaffold: `mlua-pkg.toml`, `src/<mod>/init.tl`, `src/main.tl`, `tests/`, README (`--lib` for no entry script; `--host <name>` for a Rust host, `--embed` being the shorthand for `--host rust`) |
| `htl check [paths] [--strict] [--lint +rule,-rule] [--no-cache] [--cache-mode per-module\|whole-run] [--explain-cache]` | type-check; htl lints as `lint:` (advisory, `--strict` fails on them); a module reached through `require` (an installed dep, a `[check] paths` dir) is checked with the file and its type errors are errors too, once per run, with the file that required it; what has not changed is replayed from `.htl/` (see Caching) |
| `htl run <file.tl \| app.hb> [args]` | check then execute; `require` of a `.tl` with type errors fails |
| `htl test [paths] [--filter s] [--lib mod] [--coverage] [--lcov file] [--junit file] [--no-cache]` | `*_test.tl` and `tests/**/*.tl`, one isolated state per file; checking is replayed from `.htl/`, the run never is (see Caching) |
| `htl fix [paths] [--rule a,b] [--unsafe] [--dry-run] [--diff] [--exit-non-zero-on-fix]` | apply the fixes diagnostics carry: the safe ones by default, `--unsafe` for the ones that may change what the program does (see Fixing) |
| `htl fmt [paths] [--check] [--indent N]` | whitespace formatter (indentation from the syntax tree, blank lines, trailing space) |
| `htl gen <file.tl> [-o out.lua]` | readable Lua, the escape hatch out of htl |
| `htl build <entry.tl> -o app.hb [--debug] [--source] [--extra a,b] [--host x,y] [--no-cache] [--explain-cache]` | link the entry's `require` closure into one bundle (see Bundles), replaying from the run cache what still holds (see Caching; the directory form is not cached) |
| `htl bundle info <app.hb> [--format json]` | what a bundle records, without running it: format, the htl that built it, payload kind, the Lua its bytecode is for, entry, modules, host-provided names |
| `htl unused [paths] [--format json] [--exit-non-zero-on-unused] [--no-cache]` | the complement of the same closure: modules no entry reaches, and `[deps]` no reached module requires (see Unused) |
| `htl pkg install` | fetch every dependency `mlua-pkg.toml` declares into `.htl/modules/` and write `mlua-pkg.lock`; the deps' own `types/` are then copied into the project's (see `types/`) |
| `htl pkg add <name> <git> [--tag t \| --rev r \| --branch b] [--entry dir] [--target-dir dir]` | write the dependency into the manifest (`install` fetches it); a `patch_dir` the entry already declared is kept |
| `htl pkg update [name] [--dry-run] [--force]` | refresh dependencies and bump the pins that follow releases, then install |
| `htl pkg clean [--all]` | remove cached packages the lockfile no longer refers to, or the whole cache |
| `htl pkg patch <dep> [--force]` | take that dependency's source into `patches/<dep>/`, where the project owns it and install resolves it from (see Patched dependencies) |
| `htl types add <library> [--from dir] [--force]` | the declarations a library never shipped, from [teal-types](https://github.com/teal-language/teal-types), into `types/` with the commit they came from recorded beside each |
| `htl cache status [path] [--entries]` / `htl cache clear [path]` | report what the store holds, or empty it (see Caching) |
| `htl dts [dir]` | write the `.d.tl` files this project declares: from Rust source, the ones `#[host_module]` / `#[derive(TealRecord)]` ask for, no build needed; from Teal, the module each `---@contract` type is declared in; from the crate graph, the ones a dependency ships (`[package.metadata.htl] dts`) into `types/<crate>/`. `check` / `run` / `test` / `build` do this automatically; exits non-zero when something it was asked to write could not be, and never on a file it only [left in place](#what-htl-dts-reports-and-what-it-exits-on) |

`mlua-pkg.toml` is detected by walking up from the file: installed deps become
visible to the checker and to `run` / `test` / `build` automatically. They go under
`.htl/modules/`, beside the check cache — htl decides that one location, and the installer
is [mlua-pkg](https://github.com/ynishi/mlua-pkg)'s library rather than its binary, so
there is no second process to agree with and nothing on `PATH` to install. `MLUA_PKG_DIR`
and a `target/` in the working directory, which the `mlua-pkg` binary reads, are not
consulted. *Vendored* is kept for the other
thing: a copy of a dependency committed to the repo, which a `target_dir` entry in the
manifest declares and nothing does by default. When a
directory is given, `check` / `fmt` / `build` / `test` walk the project's own files only:
`target/`, `node_modules/`, `.mlua-pkgs/` and any
dot-directory are not entered, so dependencies' sources and tests stay theirs. A
directory passed explicitly is always walked. A `target_dir` copy is not entered either,
and there the manifest is what says so: the copy sits in the repo under a name the project
chose, so nothing about the path tells it apart from the project's own code beside it.
`mlua-pkg install` rewrites it every time it runs — checking it would report a dependency's
errors as the project's, `htl fmt` would write a diff against upstream that the next
install undoes, and its `*_test.tl` are a dependency's suite (Go's `./...` has excluded
`vendor/` since 1.9 for the same reason). A `patch_dir` dependency is the one thing in
between: `check` reads it, `fmt` and `test` do not (see Patched dependencies). What is
not walked is still checked: a dependency is checked through the `require` that reaches
it, and a type error in it is reported as an error with the dependency's own path and
the file that required it —

```text
error: .htl/modules/vendored/mathx/init.tl:12:8: in local declaration: got string, expected number
  (required by src/geometry.tl)
```

— once per run however many files require it, and replayed from the cache like the
requirer's own diagnostics. Paths read against the directory the command ran in, whether
the walk or a `require` found the file, and one that lies outside it is written in full
rather than as a stack of `..`. `htl run` would refuse the module at that `require`; the
check says so first. Files under `tests/` are checked with the
project root and `src/` on the search path, the same as `htl test`, so `htl check tests`
and `htl test` agree.

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
frame. The innermost line says a value was nil; the frames say which caller passed it,
which is the part a reader who did not write the program cannot guess. `htl test` reports
the same for a failing test and for a file that raises while loading, and `--format json`
carries the text unchanged in each file's `error` and `failures`.

The exception is stripped bytecode, which is what a bundle holds by default and what
`include_tl_bytes!` embeds. Stripping drops the line numbers and the chunk's own name
together, so those frames say what raised and not where. A scaffolded Rust host
(`htl new --host rust`), failing inside its embedded module:

```text
runtime error: ?:-1: attempt to index a nil value
stack traceback:
	[C]: in metamethod 'index'
	?: in upvalue '?'
	?: in function 'sample.greet'
	src/main.tl:4: in main chunk
```

The last frame is the entry script, embedded with `include_tl!` as source, and it still
names its file and line; the two `?` frames are the stripped module, left with the
function names Lua recovered from the calls and nothing to open. A bundle is stripped
throughout unless it was built with `htl build --debug` (see Bundles). Whichever a host
ships, the `.tl` is still there, and running it is what gives the frames back —
`htl run src/main.tl`, the same failure:

```text
runtime error: src/sample/init.tl:9: attempt to index a nil value (local 'g')
stack traceback:
	[C]: in metamethod 'index'
	src/sample/init.tl:9: in upvalue 'title'
	src/sample/init.tl:14: in function 'sample.greet'
	src/main.tl:4: in main chunk
```

Frames are for whoever wrote the Teal. A host embedding htl in a program whose users did
not write it shows them `htl::user_message(&err)` instead — the innermost cause alone (see
Embedding in Rust).

## Caching

`htl check` stores what it worked out under `.htl/cache/` at the project root and replays
whatever has not moved. The summary says how much: `[cached]` when everything came from the
store and no checker was built at all, `[36/48 cached]` when some of it did, and nothing
when none did. `--format json` carries the same as `summary.cached` and `summary.replayed`.
`htl init` puts `.htl/` in `.gitignore` — one line for the cache and the installed deps
beside it; add it by hand in an existing project.

`htl build` and the macros (`include_bundle!`, `include_tl!`, `include_tl_bytes!`) read
the same store. A typed module in the closure whose entry still holds is taken from it —
the Lua it generated, and what checking it reported — instead of being generated again,
and `htl build` says so with the same `[cached]` / `[31/32 cached]` suffix (the directory
form of `build`, the older snapshot, is not cached). The entries are the ones `htl test`
writes for the modules its test files reach (`module`, holding the generated Lua beside
what checking said), keyed by the file and the lint selection and stamped without the
binary, so a test run feeds the next build, a build feeds the next test run, and a
`cargo build` replays what `htl build` generated. Bytecode is never stored: compiling the
stored Lua is milliseconds, and the bundle a replayed build writes is byte-for-byte the
one a cold build writes. `build` takes `--no-cache` and `--explain-cache` and is always
per-module; the macros keep a store only in a project with an `htl.toml`, and never under
`target/` (the copy `cargo publish` verifies) or a registry checkout — `HTL_NO_CACHE=1`
turns it off, `HTL_CACHE_DEBUG=1` has them say how much they replayed or why they did
not. Only `htl check` bounds the store: a build or an expansion sees one closure and
would evict the rest of the project. On a 30-module, 16,000-line project (release CLI): a
cold build 1.3 s; nothing edited 0.03 s; one leaf edited 0.7 s, `[30/32 cached]` — the
leaf and the entry that requires it are generated, the rest replay; the module every
other one requires edited, 1.3 s again, since every entry read it.

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
name it requires still resolves where it did, and the binary that wrote the entry is the one
reading it. Content hashes throughout, no timestamps, so touching a file without editing it
invalidates nothing and a fresh checkout does not either. Anything unexpected — a corrupt
entry, an unreadable store, an htl upgrade — is a miss, which costs the check it would have
skipped and never the wrong answer.

Only the names a module actually requires are watched. Adding a module nothing requires
leaves every existing entry valid; adding one that could answer to a name something does
require invalidates the modules asking for that name, whether or not the checker would still
have picked the old file. Writing a new module is a normal thing to do while working, and it
costs a check of that module rather than of the project.

The store is bounded, at four entries per module or 256, whichever is larger. A run that
finds it over the bound drops what it did not itself use: entries whose files are gone go
first, then the oldest until it fits. A dropped entry is a miss on the next run and nothing
worse. Eviction is where mtimes are allowed, because being wrong there costs a check rather
than a wrong answer; invalidation still refuses them.

Flags are part of the key when they change what a module reports and not when they only
change the verdict: `--lint` gets its own entries, `--strict` reuses them and differs in the
exit code alone.

`htl test` shares the store, for the half of its work that does not depend on the outcome:
checking a test file and generating its Lua. **The run is never cached** — a test has to run
to say whether it passes, and it does, every time. The summary says how many files had their
checking reused (`27 checked from cache`), and `--no-cache` opts out as it does for `htl
check`.

The modules a test requires are stored too, and put in front of the module searcher before
the file runs, so requiring one does not check and generate it mid-execution. Measured on a
27-file suite: 4.65 s without any of this, 5.03 s on the first run (which stores what it
generated) and 3.03 s on every run after.

`htl cache status` says what the store holds — entries by kind, total size, how recently they
were used, and with `--entries` the files each one covers. `htl cache clear` empties it. Both
find the store beside `htl.toml`, which is not necessarily where you are standing:
`htl check src` run from anywhere in a repo writes to the project root.

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

`exec` passes its arguments to the script as `...` and nothing else. A script that reads
`arg[1]`, as `htl run` lets it, needs `h.set_arg("main.tl", &args)?` before `exec`: that
fills the `arg` table the way the `lua` CLI and `htl run` do, so the same `main.tl` runs
unchanged both ways (`htl new --embed` writes both calls).

The second argument to `exec` is the chunk name: the name every frame of a run-time
failure inside that chunk is reported under. `@<path>` is a source location and prints as
the path, so `@scripts/main.tl` gives a reader something to open; `=<label>` is a bare
label, the honest answer for a module no file backs, which is how htl registers its own
test library as `=htl.test`. `preload` takes no chunk name and uses the `.tl` a `require`
of that module name would have found (`foo.bar` → `@foo/bar.tl`); `preload_at` takes one
when the source is somewhere else, or when there is no source.

`preload_bytes` is the exception, and it is worth knowing before reading a failure from
an embedded module. A compiled chunk carries the name it was compiled under, and stripping
drops that name along with the line numbers — so a frame from `include_tl_bytes!` reads
`?: in function 'util.greet'`, whatever name the load was given. The Teal is still there
to run: `htl run scripts/util.tl` and `htl test` name the file and the line.

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

`#[derive(TealRecord)]` is checked in one direction at build time and one at runtime:
the `.d.tl` it writes is what the Teal side is compiled against, while a table coming
back the other way is compared field by field as it converts. A table that does not fit
says which record, which field, what the record declared and what arrived:

```text
Outcome.cause: expected string, got nil
Outcome.depth: expected integer, got string
Recording.outcome.cause: expected string, got nil
```

A Teal record literal may leave fields out and `htl check` is right to pass it, so this
message is the whole signal for that direction; `require_fields` in `htl.toml` is the
check-time counterpart when a module's table is meant to be complete.

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
its variant records are reachable as `host.Shape_Circle` for `is`; a `.d.tl` module of
its own could export only the union, so `#[teal(dts = ..)]` on one is refused. Unit
enums and newtypes may stand alone, and `uses = [Name]` imports every kind with
`local type Name = require("Name")`.

A variant reaches Teal under its Rust name unless the enum says otherwise:
`#[teal(rename_all = "..")]` on the enum takes serde's set — `lowercase`, `UPPERCASE`,
`PascalCase`, `camelCase`, `snake_case`, `SCREAMING_SNAKE_CASE`, `kebab-case`,
`SCREAMING-KEBAB-CASE`, spelled as serde spells them, so a type that is also `Serialize`
can say the same thing twice and the two agree — and `#[teal(name = "..")]` on one
variant overrides it. The word is what the declaration lists, what a value must say to
cross, what the message lists when it does not, and, for a data-carrying enum, what
`where self.kind == "in_review"` tests; the variant *records* keep their Rust names
(`Shape_InReview`), since a Teal identifier cannot be `kebab-case`. Two variants that end
up with the same word are refused at expansion, naming both. Record fields have no
equivalent — they are declared under their Rust names, and `#[teal(..)]` on a field is
refused rather than ignored.

`Result<T, E>` returns raise a Lua error on `Err` by default. With
`#[host_module(name = "store", errors = "return")]` they come back Lua-style instead:
`Ok(v)` -> `v, nil`, `Ok(())` -> `true, nil`, `Err(e)` -> `nil, tostring(e)`, and the
`.d.tl` says `function(...): T, string` (`boolean, string` for unit), so
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
and is declared as `T`, the same name a method returning it declared; nested types come
from `#[derive(TealRecord)]` structs and enums in the same source file, types from
other modules via `uses = [Name]` + their own `.d.tl`.

The name a host module registers is a name the Teal sources no longer own. The host puts
it in `package.preload`, which Lua consults before any path searcher, so a `scripts/host.tl`
sitting beside `#[host_module(name = "host")]` is the file the check reads and never the
module the program runs — green check, successful build, `attempt to call a nil value` at
the first function the two do not share. `host-module-shadowed` reports that at the
`require`, naming both. It is a lint rather than a fix: which of the two should give up the
name is the project's decision, and htl moves neither resolution order.

### Shipping the declaration to your users (`[package.metadata.htl] dts`)

A crate that registers a module in someone else's Lua state has to hand them the
declaration of it too — their `htl check` searches their project, not your package. Name
the files your macros write, in your manifest:

```toml
# your-crate/Cargo.toml
[package.metadata.htl]
dts = ["dts/mq.d.tl"]      # written by this crate's own #[host_module(dts = "dts/mq.d.tl")]
```

Every project that depends on the crate then gets them under `types/<crate>/` from `htl
dts` (and from `check` / `run` / `test`, which generate before they work). Keep the files
current the way this repository does — the macro rewrites them, CI diffs them — and commit
them; they are what a consumer's checkout copies from, before anything of yours is built.

#### What `htl dts` reports, and what it exits on

`htl dts` says what happened to each declaration, one line each (`check` / `run` / `test`
/ `build` print the same lines, prefixed `dts:`):

| line | meaning |
|---|---|
| `wrote <file>` | written now |
| `unchanged <file>` | already what it should be |
| `not written: <why>` | asked for and not written: a crate names a file in `[package.metadata.htl] dts` that is not a `.d.tl`, or is not in the package, or names two that would be one file under `types/<crate>/`, or the file could not be written |
| `left in place: <file>` | under `types/<crate>/` from an earlier run, and no longer shipped — the crate is gone from the graph, or still there and no longer naming the file |

**The exit code is about `not written` and nothing else.** It is non-zero when a
declaration this command was asked to write could not be written — so a CI step that
regenerates declarations does not pass having written nothing. `left in place` fails
nothing: the file is still there and still checked, and whether to delete it is the
project's call, since a script may still `require` the module and the dependency may be
back on the next branch. `htl dts` deletes nothing under `types/` on its own.

Neither line is a lint. They are this command reporting on its own job, so they carry no
`[htl <rule>]` name, they are not in `--list-lints` or `[lint]`, `-- htl: allow(...)`
does not apply, and `htl check --format json` never carries one — a lint is a finding
about your code, and these two are about files this command was asked to write.

### `async fn` (feature `async`)

A method may be `async`, in the same `impl` as the sync ones and with no annotation
saying which the block contains. It is registered through mlua's async variant, and its
Teal declaration is the one the same signature produces without `async` — a function that
yields internally and hands back the same values is an ordinary call from Lua, and Teal
has no way to say otherwise.

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
mlua's `stack traceback:`, and there are two things to do with it. A host chooses by
audience: `htl::developer_message(&err)` returns the cause with the frames below it, which
is what `htl run` and `htl test` print because whoever runs them wrote the Teal;
`htl::user_message(&err)` returns the innermost cause alone, which is what a program puts
in front of people who did not and cannot act on a stack. The C ABI takes the second
(see below).

For mod / plugin directories, `TealResolver::new("mods")?.expect_type("defs.Mod")` holds
every served module to a record type: a mod that returns the wrong shape is rejected at
`require` time even if it never annotates its own return value. It rejects fields of the
wrong *type*; on its own it does not reject *missing* fields (every Teal record field is
nilable). Chain `.require_fields(["name", "monsters"])` to name the fields that must be
present: the module is rejected at `require` naming the nil ones, and a field added to
the record later stays optional until it is added to the list, so the type can grow
without breaking the modules already written against it. `.require_all_fields()` takes
every declared field, for types that are settled.

### A C ABI for a host that is not Rust (feature `ffi`)

`#[host_module]` hands the Rust host to Lua. `#[c_export]` hands the same `impl` block to
a caller that is not written in Rust — a Unity script, a Swift app, a Python REPL — as a
C ABI, with the header written from the same breakdown so a rename moves both.

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

**What the ABI promises.** Everything a caller holds is an opaque handle pointer, a
`char *` this library allocated, or an `int`. There are no structs by value, no `bool`,
no bare enums and no variadics — the list every host language breaks on, one way or
another (C# marshals a returned `string` and then frees it with `CoTaskMemFree`; Python's
`restype = c_char_p` copies and leaks the original).

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
`PANIC` 5, `WRONG_THREAD` 6, `INTERRUPTED` 7. A `char *` function answers `NULL` when it
fails and `game_last_status()` says which of these it was. `game_last_error()` is the
message — a pointer owned by the library, valid until the next call *on that thread* —
and `game_last_error_into(buf, len)` copies it into a buffer of the caller's own.

**Panics do not cross.** Since Rust 1.81 a panic reaching an `extern "C"` frame aborts
the process, which for a plugin host means taking the editor down with it. Every wrapper
catches it, records it as `PANIC` with the panic's message, and **poisons the handle**:
every later call on it answers `PANIC` without running anything.

**One handle, one thread.** The handle records the thread that opened it and every entry
checks it, so using it elsewhere is `WRONG_THREAD` rather than a data race
(`game_threadsafe()` answers `0`, the `sqlite3_threadsafe()` convention). The exception is
`game_interrupt(h)`, callable from any thread: it sets an atomic that a Lua debug hook
turns into an error at the next tick, which is how a runaway mod is stopped from a host
that cannot preempt it. One interrupt stops one run; the handle stays usable.

**Conventions the generated code fixes**, so a project does not decide them again:
`game_open` takes one JSON object — pass absolute paths, a seed and names in it rather
than expecting the library to read the environment or the working directory; records
cross as JSON text and an object payload carries `"v"`, a schema version separate from
`GAME_ABI_VERSION` (which is the shape of the functions, and what a host that never
unloads a library compares before calling anything else).

`htl dts` writes the header too, so it can be regenerated and diffed without a build.

**A project of this shape is `htl new --lib --host ffi <name>`** (see [The C ABI host](#the-c-abi-host---host-ffi)):
the crate types, the feature, the `#[c_export]` block and — the part a reader of an ABI
actually needs — a caller in C and a caller in Python that do the round trip and free
what they are handed.

## Lints (`htl check`, `include_tl!`)

| rule | default | catches |
|---|---|---|
| `nil-index` | on | `t[k].x`, `t[k]:m()`, `t[k]()`, `t[k][j]` — Teal types a map/array lookup as `V`, not `V \| nil` |
| `struct-fields` | on | a table built for a record marked `---@struct` that leaves out a field the record declares and `---@optional` does not exempt. Silent until a record carries the marker (see below). `htl fix` spells the missing fields at the site, as a suggestion it never applies |
| `sealed-record` | on | a table built for a record marked `---@sealed`, or an `as` cast to one, outside the file that declares it — outside the functions the marker names, when it names any (`---@sealed(gate.judge)`). Silent until a record carries the marker (see below) |
| `enum-exhaustive` | on | `if e == "a" ... elseif e == "b" ... end` over an enum with a value left unhandled and no `else`; enums nested in records and enums from required modules count |
| `enum-cast` | on | `e as E` where `E` is an enum and the checker types `e` as `string`: `as` is erased, so the word enters the enum with nothing checking it. A string literal (`"open" as E`) and a value already typed as the enum are not reported (see below) |
| `enum-table` | on | a table constructor whose declared type maps an enum (`{string: E}`, `{E: T}`) and that leaves a value of the enum out, or lists a word that is not one. An array of the enum (`{E}`) is a selection, not a mapping, and is not reported. `htl fix enum-table` fills a `{string: E}` one in |
| `union-exhaustive` | on | `if x is A ... elseif x is B ... end` over a union with a variant never tested and no `else`. The variants come from the checker, so a chain that predates a variant is reported once the union gains it (see "Unions of records") |
| `shadow-local` | on | a local / loop var / parameter reusing an enclosing local's name; when that outer local is a `require`d module the message says which module and where it was required |
| `no-global` | on | `global` declarations |
| `no-any` | off | explicit `any` annotations and `as any` casts |
| `explicit-number` | off | `local n = 0` (inferred `integer`) that is later assigned a number expression (`n = n * 1.5`, `n = a / b`): names the declaration and the assignment; write `local n: number = 0`. Plain integer counters are not reported |
| `class-record` | off | a record declaring metamethods (`metamethod __index: Actor` = a class): its metatable is attached by `setmetatable` at run time and is not part of the value, so serialization and the Rust boundary drop it; keep such records out of saved data and host signatures |
| `host-module-shadowed` | on (always) | a `require` of a name a `#[host_module]` in the surrounding crate registers that resolved to a Teal file of that name: `package.preload` beats the path searcher at run time, so the file is what is checked and the host is what runs. Reported at the require, naming both (see "Rust host") |
| `require-cycle` | on (project-level) | a loop in the require graph of the files `htl check <dir>` just checked, e.g. `a.tl -> b.tl -> a.tl`. Teal types the back edge as an opaque circular require, so without this the symptom is "cannot index" somewhere else |

Silence one occurrence with a trailing `-- htl: allow(nil-index)`. `include_tl!`
treats lints as errors (`HTL_LINT=warn` downgrades, `HTL_LINTS=+no-any,-shadow-local`
configures).

A lint is a finding about your code, and everything `htl` prints with an `[htl <rule>]`
name is one. `htl dts`'s `not written` and `left in place` lines are the command
reporting on the declarations it was asked to write, not findings, and are not in this
table: [What `htl dts` reports](#what-htl-dts-reports-and-what-it-exits-on).

### Records built whole (`---@struct`)

Every Teal record field is nilable and Teal has `?` for function parameters but not for
record fields, so a record the program builds itself still reads as if any field might be
absent. `---@struct` says it does not:

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

A misspelled field is the case where two rules each hold half the answer: the checker says
`unknown field colour` about the key that exists, and this says `color` is missing. When
the key the literal sets is a near miss for the one it wants, the message names it instead
of repeating the standing advice, because "mark it `---@optional`" is the wrong fix for a
typo:

```text
MonsterDef is built without color (the literal sets `colour`)
```

One edit counts as a near miss in any name, two once the name is at least eight characters
long. An extra key that is nothing like the missing one is not offered.

The markers go where the record is **declared**, and the report lands where it is
**built**, so an SDK can declare the shape its mods must fill in. Both marker forms work:
trailing on the field's own line, or on the line above it. Every construction site counts
— a bare literal, an element of an array or map of that record, a literal passed as a
typed argument, and a function's `return`.

This is a lint, not a type. The file stays valid Teal and other tooling ignores the
comment; use sites still see a nilable field. What it removes is the reason to guard, and
the doubt about whether a field was ever set. Data arriving from outside the program — a
mod's return value, a save file, a host — is a different question, and `[[contract]]` with
`require_fields` is what checks that.

### Records built where they are declared (`---@sealed`)

Some records mean "this went through the check": a `Judged` that only `gate.judge()` is
supposed to produce, a state a transition may mint and nobody else. In Rust that is a
private field and a constructor; in Teal a record is built by writing `{ ... }` with the
right keys anywhere, and `{ ok = true } as Judged` gets past even a mismatch, so the
invariant the record stands for is a comment. `---@sealed` is that comment made a rule:

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

Inside the declaring file everything is allowed — the marker is about the boundary, not
the owner. Naming functions narrows it to those, inside that file: a `Draft` built in
`gate.open` passes and one built in `gate.other` is reported (`built only in gate.tl by
gate.open or gate.reopen`). The function is matched on the name as written or on its last
segment, and a function assigned rather than declared (`gate.judge = function() ... end`)
counts as the function it is written inside. `-- htl: allow(sealed-record)` keeps one site
the project stands behind.

A test that compares a whole sealed value builds one, and is reported like anywhere else:
`t.expect(gate.judge("yes")):to_equal({ verdict = "yes", at = 1 })` writes a literal typed
as `gate.Judged` in a file that is not `gate.tl`, which is the rule working rather than
misfiring. Both ways through are ordinary. Either the assertion carries
`-- htl: allow(sealed-record)`, which says this literal exists to be compared and never
leaves the test, or the test asserts the fields it is about
(`t.expect(j.verdict):to_equal("yes")`), which builds nothing and says which field
differed when it fails.

A record nested inside a sealed one is not sealed by that; mark it too if it should be.
Like `---@struct`, this is a lint and not a type: the file stays valid Teal, other tooling
ignores the comment, and what it adds is the one thing a run-time check cannot — that no
other code minted the value. It pairs with `---@struct` on the same record, which says
every field is set where this says who may set them; both report at the same site with
their own message.

### The string boundary of an enum (`enum-cast`, `enum-table`)

A Teal enum is a string at run time and `as` is erased along with the types, so
`h.state as defs.State` promises nothing: with `"opne"` in the store the value is false
against every variant, drops out of every branch, and nothing raises. `enum-exhaustive`
guards the `if` chain; it cannot see that the value never entered the set. `enum-cast`
reports the cast when the checker types the value as `string` — a value it already types
as the enum is a cast that restates what is known, and `"open" as defs.State` is checked
by the literal itself, so neither is reported. `-- htl: allow(enum-cast)` keeps a cast the
project stands behind.

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

The words a constructor lists are its keys, in both shapes that map the enum —
`{string: E}` above and `{E: T}`, a table of one thing per value — and the message names
both the values of the enum that are missing and the words that are not values of it. Add
a value to the enum and every such table reports, which is the `enum-exhaustive` story for
constructors.

An array of the enum (`{E}`) is left alone: a list of the styles one branch uses or the
behaviours one test walks is a selection, and asking it for every value is noise. So are
an empty constructor (which is how a table that is filled later is written) and one whose
keys are not all literals, since a computed key leaves the word set unknown; a table built
by a call has no constructor to look at, the exemption `enum-exhaustive` makes as well. A
`{string: E}` map none of whose keys is a value of the enum is some other map and not this
lookup.

`htl fix enum-table` fills a `{string: E}` table in — `name = "name"` per missing value,
laid out where the entries already there are. For `{E: T}` it reports and changes nothing:
what an entry maps to is not something a fix can invent.

## Project config (`htl.toml`)

`htl check` / `htl test` / `htl fmt` / `include_tl!` all read the nearest `htl.toml`
above the file, so the CLI and the build agree. Flags and `HTL_LINTS` / `HTL_LINT`
override it (`htl new` writes a commented one).

```toml
[toolchain]
htl = "0.3"               # the htl command this project expects; a mismatch is refused

[lint]
enable  = ["class-record", "explicit-number"]
disable = ["shadow-local"]
strict  = true            # lints fail check/test and include_tl!; false makes the macro advisory

[fmt]
indent = 3

[check]
paths = ["mods", "~/.cache/tsk/sdk"]   # extra dirs require() resolves from while checking

[[contract]]              # where this project accepts modules written outside it
dir = "mods"              # relative to htl.toml; "sites/*" = every subdirectory of sites/
# module = "Site"         # optional: only this module name (in each dir) is held to it
```

`[toolchain] htl` is a cargo requirement (`"0.3"` = 0.3.x) on the *command*, which
`Cargo.toml` does not pin — it pins the crate a Rust host builds against. The command
is what decides whether the project checks: three lints were added on one day and all
three are on by default, so a project green under the release before them is red under
the release after, on unchanged sources. Written down, that arrives as a version the
project moved to rather than as a difference between two machines. A command outside
the requirement is refused before anything is read, naming both versions and this file;
htl installs nothing, so the answer is `cargo install htl-cli`. Leave the key out and any
command runs the project, as before — which is what `htl new` currently writes, because a
scaffolded Rust host builds against the released `htl` crate and that crate rejects a key
newer than itself (`docs/releasing.md` § What the scaffold may write). Add it by hand to
pin a project whose htl already knows it.

`[check] paths` is for modules the host supplies at run time from somewhere the
checker would not look (an SDK cache, a mods dir): the CLI, `include_tl!` and
`contract_resolvers` all add them, plus the `htl.toml` dir, its `src/` and its
`types/`. `types/` is the conventional home for `.d.tl` (the DefinitelyTyped shape:
declarations the module's author did not ship), searched without any configuration;
`htl new` creates it.

Four kinds arrive there. The ones written by hand; the ones a Rust dependency ships; the
ones a Lua dependency published; and the ones for a library that published none of its
own. Only the first are anyone's to edit — the rest are copies, and a change to one
belongs in the crate or package it came from.

A Rust crate that registers a module in its user's Lua state names the declarations it
ships in its manifest (`[package.metadata.htl] dts = ["dts/mq.d.tl"]`, see "Embedding in
Rust"). `htl dts` — and `check` / `run` / `test`, which generate before they work —
resolves the crate graph with `cargo metadata` and writes each of those files to
`types/<crate>/<file>`, reported like the project's own (`wrote types/htl-mq/mq.d.tl`) and
committed like them. That directory is on the search path in its own right, so the module
keeps the name it was declared under whatever the crate is called: `htl-mq`'s `mq.d.tl` is
`require("mq")`. A note beside them (`.htl-dts`) records which crate and version they came
from, and is what tells the directory apart from one laid out by hand, where the path below
`types/` is the module name.

Nothing is built to do it, and a project whose dependencies are already resolved and
fetched needs no network. A manifest edited since the last resolve is resolved again, which
writes `Cargo.lock` and may fetch — what the next `cargo build` would do anyway, and what
having the dependency's files on disk to copy from requires. Where the graph cannot be
resolved at all, that is reported and the committed copies go on being what the project
checks against. `include_tl!` never runs cargo at all — it reads `types/`, as it always
has. A hand-written `types/mq.d.tl` beside a shipped one is a
`duplicate-declaration` (the hand-written one is read), which is the message wanted when a
project upgrades a crate that has started shipping its own. Dropping the dependency leaves
the file where it is and says so: what a committed declaration is still for is the
project's to decide, not this command's.

A package keeps its own declarations at `types/` in its root, which is outside the entry
directory `require` resolves through, so `htl pkg` copies them in. `htl types add
<library>` is the other half: teal-types is where the Teal ecosystem collects declarations
for libraries that ship none, as `types/<library>/<module>.d.tl`, and `add` takes one
library's worth. The library's own directory is dropped and the path below it kept, since
that path is the module name — `socket/http.d.tl` stays `require("socket.http")`.

Both write a `.src` note beside each file: what published it, at which commit, and the
path it had there. Nothing else records that. `luasocket-tl-type` is versioned `0.0.2-1`
against a luasocket at 3.x, its rockspec declares no dependency on luasocket, and its
source names no revision — so without the note, a declaration carries no evidence of what
it was written against.

Copying rather than searching the installed deps is what makes them survive a fresh clone:
`.htl/` is gitignored and empty until someone installs, `types/` is committed. A name
`types/` already has is reported and left alone (`--force` replaces it): two libraries
publishing a module of the same name is a real situation, and there is no registry to
arbitrate it with.

Source beats declaration: when both `defs.tl` and a `defs.d.tl` are reachable, the
checker reads the `.tl`, wherever the two sit on the path (Teal's own order is `.d.tl`
first). So a `.d.tl` a host writes out for external script authors never shadows the
source it was made from inside the repo, and a check that runs before the host has
rewritten it still sees the current types.

Between two *declarations* of one module there is no such rule, only position: the
directories above are consulted in the order they are listed, and the first hit is the
one read. `duplicate-declaration` reports it — a project that keeps a hand-written
`xlib.d.tl` under `types/` and also has one arriving from a `[check] paths` directory is
told which is in effect and which is not, rather than being left to work out why a type
is not what the file in front of it says.

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

Every module directly under `mods/` must return a value assignable to `defs.Mod` and set
the three marked fields. `factions` and `npcs` are for the mods that want them, and that
asymmetry is the point: a record cannot say which of its own fields are mandatory (every
Teal record field is nilable and there is no `?` for them), and holding modules to *all*
of them would break every one written before a field was added. Marking the mandatory
ones lets the type grow.

The default is the opposite of `---@struct`'s, and each marker says which regime its
record is under: `---@struct` is about a record the program builds itself, where a new
field is mandatory unless marked `---@optional`; `---@contract` is about a value arriving
from outside, where a new field is optional unless marked `---@required`.

A bare `---@contract` inherits the directory from `htl.toml`, which is what a project with
one contract writes. `---@contract("plugins")` names its own, `---@contract(module = "S")`
narrows a directory to one module name, and both can be given at once.

The module the contract type is declared in is what an outside author writes their
modules against, so htl publishes it: `types/defs.d.tl` here, alongside the `.d.tl` a
Rust host's `#[host_module]` writes, regenerated by `htl dts` and by check / run / test /
build. `---@contract(dts = "sdk/defs.d.tl")` sends it somewhere else. Commit the result,
the same as the Rust-generated ones: it is what makes a fresh clone check before anything
has been built.

What it writes is the declaring module with its bodies removed: each function the module
exported becomes a field of the record it was on, keeping its parameter names and its doc
comment, which is what a hand-written `.d.tl` says.

```tl
function defs.describe(m: Mod): string    -->    describe: function(m: Mod): string
   return m.name
end
```

A `local function` is not part of what the module declares and leaves nothing behind, a
field the record already declares is left as the author wrote it, and a method keeps the
`self` its definition left implicit. The published marker names its directory outright,
since whoever reads the declaration does not have the `htl.toml` a bare `---@contract`
inherits from. A function on a table the module declares no record for is reported rather
than dropped — a declaration missing a function is worse than one that was not written.
A module declaring two contract types is one file and is published once, both markers
named outright.

`types/` is searched, so the published declaration puts the claim on the path a second
time. That is one record found twice, not a second claimant of the directory, and only two
*different* records claiming one directory is reported. Nor do two contract directories
collide over a name: a contract directory is resolved as a directory and is not on the
project's search path, so `mods_a/one.tl` and `mods_b/one.tl` are two modules, each held
to the record its own directory is under.

Two lints follow:

- `contract` — a module under the directory whose return value is not assignable to the
  record, or whose returned table literal leaves a `---@required` field out, is reported
  at `htl check` time instead of at the first `require`. The literal is found through
  `return { … }`, `return define({ … })`, `return { … } as T`, and
  `local m: T = { … } … m.f = … return m`. A marker that cannot be turned into a contract
  — one naming no directory in a project whose `htl.toml` declares none or several, two
  markers claiming one directory, a marker on the record a module returns rather than on
  one inside it — is reported here too.
- `contract-unenforced` — a contract is only a guarantee if the host enforces it. When a
  Cargo package is found, `htl check` scans its Rust sources for `contract_resolvers(`
  and otherwise tells you to add it, or to say where it is enforced with `enforced_by`.

Hosts build their resolvers from the same markers, so the two cannot drift:

```rust
let (path, cfg) = htl::config::HtlConfig::find(Path::new("."))?.expect("htl.toml");
let mut reg = mlua_pkg::Registry::new();
for r in htl::pkg::contract_resolvers(&htl::parent_dir(&path), &cfg)? {
    reg.add(r); // TealResolver for <root>/mods, expecting the record marked
                // ---@contract for that directory and its ---@required fields
}
```

That one call is what `contract-unenforced` looks for. A resolver assembled by hand from
`TealResolver::new(…).expect_type(…).require_fields([…])` still works, but it restates
what the record already says, which is the drift the marker exists to remove.

Enforcement the scan cannot see — a Lua-side validator that checks the table before the
host uses it, a resolver in a sibling crate, generated code, or one built by hand on
purpose — is named instead:

```toml
[[contract]]
dir = "mods"
enforced_by = "mods/_validate.lua"   # relative to htl.toml; ~ and absolute paths work
```

That contract is then not held to the scan, and the others in the project still are. It
takes a path rather than a `true` because the file has to exist: a name that points at
nothing is reported under the same rule, whether or not the call was found elsewhere, so
the key stays a claim `htl check` can hold to something rather than a per-contract off
switch.

## Patched dependencies (`htl pkg patch`)

A dependency needs one line changed. `htl pkg patch mathx` copies its package root — the
whole package, so its `types/` comes with it — out of the pinned revision and into
`patches/mathx/`, writes `patch_dir = "patches/mathx"` onto that dependency in
`mlua-pkg.toml`, and records the commit it was taken from as `patch_base` in the lockfile.

```text
  patched patches/mathx (mathx at 3f2a9c1)
```

From there the directory is the project's code: edited, diffed, reviewed and committed
with git like anything else in the tree. There is no patch file and nothing is applied —
`htl pkg install` leaves the directory alone and resolves the dependency from it. This is
the shape of Cargo's `[patch]` with a `path` source, and of Go's `replace` pointing at a
directory in the module tree. Removing `patch_dir` and the directory returns the
dependency to its fetched form at the next install.

**What is checked, and what is not.** The copy is committed, project-owned code whose
errors are the project's to fix, so `htl check` walks it and names the dependency each
directory stands in for. `htl fmt` and `htl test` do not touch it: formatting it would
turn every file into a diff against its base and hide the change inside it, and its
`*_test.tl` are the dependency's suite rather than the project's. `.htl/modules` is not
descended into at all, patched or otherwise; its modules are checked through the
`require` that reaches them and their errors reported against the requirer, never offered
to `htl fix` (a fix there would go at the next install — patching is how a dependency is
edited). The criterion for walking is who writes the directory — one that install
regenerates (`target_dir`) is skipped, one that the project edits is checked.

**Upgrading.** A patch is bound to the revision it was taken from. When the pin moves —
the dependency was upgraded — install fetches the new revision and resolves from it, the
copy is left alone, and every install says so until the patch is refreshed or removed:

```text
  patch   patches/mathx is not in use (taken from 3f2a9c1, mathx is now at 8b07e44)
          carry the change forward: commit it, then `htl pkg patch mathx`
          drop it: remove patch_dir from mlua-pkg.toml and delete patches/mathx
```

Install does not fail over it; the project builds against the new upstream. `htl pkg
patch` on an already patched dependency refreshes the copy from the revision the pin now
resolves to and records that as the new base. The copy is overwritten rather than merged,
so carrying the project's own change forward onto it is a merge git performs — which is
why a directory with uncommitted changes is refused, naming them, and why `--force` (which
discards them) is a flag rather than the default. Outside a repository the question cannot
be asked at all, and that is said rather than guessed at.

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
the value with `tests/__snapshots__/<test file>/<name>.snap`. The first run writes the
file (and says so); later runs fail with a `-expected +actual` line diff when the value
changed; `htl test --update` rewrites the differing ones. A string is stored as is, an
array of strings as its lines (a rendered screen), anything else in a sorted,
one-entry-per-line form, so the files read well in a review. A name used twice in one
file is an error.

Coverage: `htl test --coverage` prints, per `.tl` module the tests' checks depended on,
how many of its statements ran (`executed/all  %`), and a total; a module no test
reached shows `0/n`. Under a module it names the functions nothing entered, since the
percentage says how much was missed and not what:

```text
coverage: src/combat.tl      124/181   68.4%
          never ran: resolve_counter (61), flee_path (130)
```

`--coverage-lines` adds the unexecuted line ranges under those. Statements are counted
from the `.tl` syntax tree and matched against Lua's line hook (Teal keeps line numbers
when it generates Lua), so the numbers are `.tl` lines. A function counts as entered
when a line strictly between its `function` and its `end` ran: defining a function runs
both of those lines, so neither says anything about calls. A function with nothing in
between — written on one line, or with an empty body — is not reported. The hook slows
the run, and code that runs inside a coroutine the program creates is not seen.

`--lcov coverage.info` writes the same run as an lcov tracefile, which is what Codecov,
Coveralls, GitLab, `genhtml` and editor gutters read; it implies `--coverage`, and the
table and `--format json` are unchanged. One record per module the table lists: `FN` /
`FNDA` from the functions above (`1` when the body was entered, `0` when not), `DA` per
line a statement starts on, with a count of `1` or `0` — the hook records whether a line
ran, not how often, and consumers treat any non-zero as covered. Two statements starting
on one line share the entry, so `LF` / `LH` differ from the table's `total` / `executed`
by exactly those lines. There is no branch data and no `BRDA`. `SF` is relative to the
project root (the `htl.toml` directory) rather than to where the command ran, so the
file resolves against the repository wherever CI stood; a module outside the root is
written absolute.

`--junit report.xml` writes the run as a JUnit XML report, which is what a CI reads to
show which tests failed rather than a log to scroll: Jenkins' JUnit plugin, GitLab's
report ingestion, and the GitHub Actions reporters all take this file. One `<testsuite>`
per test file, one `<testcase>` per test with `classname` (the file), `name` (the suite
and test name as the text output composes them) and `time` in seconds; a failing case
carries a `<failure>` with the message printed under the file, traceback included. The
totals are the summary line's: the same cases, the same failures. A file that failed to
type-check, or raised outside any test, is a suite whose cases could not run, so it
carries an `<error>` and no cases — the distinction a report makes between a test that
said no and a file that never got to ask. Nothing is ever `<skipped>`: `--filter`
selects before the run, so an excluded test is absent rather than skipped, while a file
with no tests did run and is there as a suite with no cases. The flag composes with
`--filter`, `--seed` and `--format json`, and changes neither the text nor the document.

Randomness: the runner seeds each file before it runs, prints the seed of every run, and
takes it back with `--seed`, so a test that draws is one whose failure can be looked at
again:

```text
htl test: seed 8014255196 (repeat with --seed 8014255196)
```

`t.rng()` is that stream, shaped like `math.random` (`rng()`, `rng(m)`, `rng(m, n)`);
`math.random` is the same stream, so a test already using it repeats too. It *is*
`math.random`, so the argumentless form returns a float in [0, 1) and the other two an
integer; the declaration types all three `integer`, since this Teal resolves an overload
by declaration order rather than by arity and cannot type the forms apart. Each file's
seed is derived from the run's seed and the file's own path rather than drawn from one
shared stream, so what a file draws does not depend on which other files ran or in what
order — running it alone, or with `--filter`, reproduces what it did in the full run. A
test that calls `math.randomseed` itself takes over from there; the runner does not seed
again.

Runner: `htl test [paths] [--filter substr] [--fail-fast] [-v | -q] [--slow MS]
[--update] [--seed N] [--coverage [--coverage-lines]] [--lcov FILE] [--junit FILE]`. Each file runs in a fresh state; `-v` prints every test with its time,
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
  record (safe); `explicit-number` gets `: number` (safe); `enum-table` gets the entries
  a `{string: E}` lookup is missing (safe — the entry it adds is the identity mapping the
  table already states for every other value); `struct-fields` gets the fields the site
  leaves out, one entry each, in the order the record declares them, laid out where the
  entries already there are (**suggest** — there is no honest value to put in one. A
  record field has a type and no zero, `hp: integer` is not `0`, and `hp = nil`
  type-checks, so a fix that filled the gap would satisfy the lint, pass the checker and
  ship the wrong value silently. What it writes is `hp = htl_fixme("integer")`: the
  declared type to read, in a call to a name the project does not have, so the checker
  refuses it until a person replaces it); `no-global` becomes `local` (unsafe). `htl.toml` `[fix] unsafe = ["no-global"]` promotes a rule, `disable = [..]`
  turns its fix off; `--rule a,b` limits a run.
- The working tree is the undo. A file git reports as modified or staged is refused
  (`--allow-dirty`), and so is a file outside a repository (`--allow-no-vcs`).
  `--dry-run` reports without writing; `--diff` prints a unified diff per file instead.
  A `suggest` fix is listed as skipped and its insertion printed under `--diff`, as a
  second diff headed `<file> (suggested)`, since nothing ever writes it.
- A file with a syntax error is never touched. Type errors elsewhere do not block (a
  fix is often what removes one); after each pass the file is re-checked and put back
  if it has more errors than before. Edits that overlap an applied one wait for the
  next pass; passes are capped at 4; two passes producing the same edits are reported
  as fixes undoing each other.
- Everything applied is listed (`fixed: file:line: rule (safe)`), as is everything
  skipped and why. Exit code as `htl check` (remaining errors → 1);
  `--exit-non-zero-on-fix` also fails when a file changed, for CI.

## Machine-readable output

`htl check --format json`, `htl test --format json` and `htl unused --format json` print
one JSON document on
stdout and nothing on stderr (the text form is stderr-only, so the two never mix).
The exit code is the same as in text mode. Field names are stable; fields may be
added, not renamed.

- `check`: `{ files, diagnostics: [{ severity: "error"|"warning"|"lint", file, line,
  col, rule?, message, required_by?, origin? }], summary: { errors, warnings, lints,
  strict, ok } }`. `rule` is the lint rule (`nil-index`, `contract`, ...), split out of
  the message. An error in a module the check reached through `require` has `file` set
  to that module and `required_by` to the file that required it; `origin` is
  `"dependency"` (installed under `.htl/modules`, or a vendored copy) or `"external"`
  (a `[check] paths` or contract directory), and absent for a file of the project's own.
- `test`: `{ files: [{ path, ok, diagnostics, error?, file_level, passed, failed,
  failures, tests: [{ name, ok, ms }], duration_ms, snapshots_written,
  snapshots_updated }], summary: { files, files_run, passed, failed, files_with_errors,
  duration_ms, ok, seed }, coverage?: { modules: [{ path, executed, total, unexecuted:
  [[first, last]], never_ran?: [{ name, line }] }], executed, total } }` (`coverage`
  with `--coverage`; `never_ran` is absent when every function of the module ran).
- `unused`: `{ modules: [{ path, module? }], dependencies: [{ name }], entries: [{ path,
  module?, kind: "main"|"test"|"contract"|"build"|"host" }], summary: { considered,
  reached, entries, modules, dependencies, no_entry, check_errors, ok } }`. The two kinds
  carry the names the text form prints them under (`module:` / `dependency:`); `module`
  is the name a `require` would have to spell, absent when the search path gives the file
  none. `check_errors` is what the check behind the graph reported: a file that does not
  check contributes no edges, so a report from a run with any is a guess.

GitHub Actions annotations from a check, for instance:

```sh
htl check . --format json | jq -r '.diagnostics[] |
  "::\(if .severity == "error" then "error" else "warning" end) file=\(.file),line=\(.line),col=\(.col)::\(.message)"'
```

## Bundles (`htl build`)

`htl build src/main.tl -o app.hb` follows `require("<literal>")` from the entry and
links everything it reaches into one file: `.tl` modules type-checked and generated,
plain `.lua` modules (a dependency's own sources) as they are. A `require` that resolves only
to a `.d.tl` is recorded as **host-provided** (a Rust `#[host_module]`, a `preload`);
any other unresolved `require` is a build error, so "module not found" happens here
and not on the first `require` at the user's machine. `htl run app.hb` runs it; a host
does `Htl::run_bundle(&Bundle::decode(bytes)?, &args)` after registering its modules,
and is refused up front, naming them, if one is missing.

- Payload is stripped Lua 5.4 bytecode by default, and stripping takes the traceback
  with it: every frame of a run-time failure reads `?`, with no line. `--debug` keeps
  the line numbers and the local names, and its frames read `depth:8` — the module the
  bundle knows, since a bundle holds modules rather than files. `--source` stores
  generated Lua instead: larger and readable, bound to no Lua build, and named the same
  way as `--debug`.
- **Portability.** A bytecode bundle runs on any host whose Lua chunk header matches
  the one it was compiled by: version, bytecode format, the sizes of instruction /
  integer / number, and endianness. Nothing about the CPU or the OS is in a Lua chunk,
  and the Lua htl vendors has a 4-byte instruction, 8-byte integer and 8-byte double on
  every 64-bit little-endian platform, so a bundle built on an arm64 Mac loads on
  x86_64 Linux and cross-building between mainstream desktop and server targets needs
  nothing. `--source` is for the cases the header refuses: a big-endian target, a host
  whose Lua was built with a non-default `LUA_INT_TYPE` / `LUA_FLOAT_TYPE`, and a bundle
  that has to outlive a Lua upgrade (mlua pins the Lua htl vendors, and a new one may
  change the format). `install_bundle` checks the header before the first `require`
  and refuses on mismatch, naming both sides: `compiled for Lua 5.4, format 0, 4/8/8,
  little-endian by htl 0.1.19, but this host runs ... on htl 0.2.0`. The htl version
  is advisory (a bundle from an older htl whose header agrees still loads); it is in
  the message because the header alone cannot say why two 5.4 builds disagree.
- `htl bundle info app.hb` prints what the file records — format version, the htl that
  built it, payload kind, the Lua the bytecode is for in the same words as the mismatch
  message, entry, modules, host-provided names — without creating a Lua state. That is
  what a build step checks in and a bug report pastes; `--format json` for the same. A
  `--source` bundle says its Lua is `any`; a format 1 bundle (`HTLB\x01`, before the
  fingerprint) says it was not recorded.
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
// payload = "source" for a target whose Lua header differs (big-endian, non-default
// number types; see Portability above); debug = true keeps line numbers.
// [build] extra / host in htl.toml are merged in.
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

`htl check` answers whether every file it was given is correct. It never answers whether
every file it was given is *reached*. `htl unused` asks the second question of the same
graph a bundle is linked from — a module nobody requires, and a dependency declared and
never used, both keep checking clean forever:

```text
module: src/legacy/parser.tl (legacy.parser)
dependency: strx
htl unused: 1 module, 1 dependency [68 considered, 67 reached, 39 entries]
```

Nothing new is parsed. `htl check` already resolves every `require` — that is what the
`require-cycle` lint reads and what a cached entry carries — so this walks those edges
from the entries and reports the complement, replaying the check from `.htl/` when
nothing moved.

**Where it starts is not a guess.** The equivalent tools for JavaScript need a plugin
per framework to work out where a project starts; here the project has already said, in
the files `htl test`, `htl build`, `[[contract]]` and a Rust host are pointed at:

- `src/main.tl` (or `main.tl` at the root);
- every test file, as `htl test` discovers them — so a module used only by a test is
  reached, not reported;
- every module directly under a `[[contract]]` directory: those are loaded by name at run
  time, from a mods directory the project does not own. The `exclude`d ones too —
  `exclude` says a module is not held to the contract, not that nothing loads it;
- anything named in `[build] extra` / `[build] host`, which is where a dynamic
  `require(expr)` already has to list its targets for `htl build` to bundle them;
- the file a Rust host embeds: the first argument of an `include_bundle!` / `include_tl!`
  / `include_tl_bytes!` in the crate around the project. A project whose `main` is in Rust
  has no `src/main.tl`, and its entry is named there and nowhere else.

A project with none of these gets a message saying so rather than a list of everything.

`paths` narrows what is **reported**, never what is walked: reachability is a property of
the project, so `htl unused src` still reads `tests/`, and a module only a test requires
stays quiet. The dependency question is asked of `mlua-pkg.toml` `[deps]`: a name counts
as required when a reached module says `require("mathx")` or `require("mathx.vec")`, or
when what a reached module required resolved to a file inside that dependency — both,
because a dependency that is declared but not installed resolves to nothing and is still
required by name.

The exit code is 0 whatever it finds, unless `--exit-non-zero-on-unused` says otherwise:
"unused" is a question about intent, and CI should opt in to failing on it rather than
out. Deleting is nobody's business here either — `htl fix` applies mechanical rewrites,
and "this module is unreachable" is not one of those; the fix is a decision.

**Exports are not a kind.** A third question — which field of a module record no reached
module reads — was considered and left out. htl's premise is a Rust host embedding Teal,
so a module's caller is routinely outside the Teal sources entirely: a `#[host_module]`
calling into a preloaded module, a `---@contract` type published for mod authors, an SDK
a consumer requires. Every one of those reads a field no walk of this project's `.tl` can
see, and a rule that fires on them is a rule nobody can act on.

## Layout of a project (`htl new`)

```text
<name>/
├── mlua-pkg.toml          [package] entry = "src/<mod>"  → consumers require("<name>")
├── htl.toml               [toolchain] / [lint] / [fmt] / [[contract]], read by CLI and macro
├── src/<mod>/init.tl      the module (require("<mod>") from src/ and tests/)
├── types/                 .d.tl the project consumes (hand-written, and <crate>/ copied
│                          from a dependency) and publishes (a ---@contract type)
├── patches/<dep>/         a dependency taken into the tree (htl pkg patch), committed;
│                          checked, not formatted, its tests not run
├── src/main.tl            entry script
└── tests/<mod>_test.tl
```

mlua-pkg's `entry` is a directory, so a consumer's `require("<name>")` looks for
`<name>/init.tl`. A flat package can instead ship `<name>/<name>.tl` (e.g. `entry = "src"`
with `src/<name>.tl`); htl resolves that form in the checker and in `TealResolver`.

### The Rust host (`--host <name>`)

`--host rust`, and `--embed` which is its shorthand, add a Cargo package to that tree:

```text
├── Cargo.toml             htl + anyhow, and [profile.dev.build-override] opt-level = 3
├── src/lib.rs             #[host_module] Host, its records, the embedded module,
│                          and pub fn preload(&Htl) registering both
├── src/host.d.tl          generated from src/lib.rs — by cargo build, and by
│                          htl dts / htl check without building
└── src/main.rs            the binary: preload, then src/main.tl (omitted with --lib)
```

The host is a library with a thin binary on top, not a binary that happens to hold a
host. What a project grows — a second `#[host_module]`, an `extern "C"` layer, a window
loop, a Rust test — grows in `src/lib.rs`, and every entry point reaches it through
`preload`: `src/main.rs` is the six lines that call `preload` and `exec` the script, and
another crate that embeds this one calls the same function. `--lib` means the project has
no entry script, so there is nothing for the binary to run and it is not written at all —
what is left is the library, which is the part someone else embeds.

`--host` names an entry in a registry of host kinds rather than adding a flag per kind:
`rust` and `ffi` are the two today, and the flag reports the rest as they arrive. A name
that is not registered is refused with the ones that are, before the directory is
created, so a typo leaves nothing behind. `htl init --host rust` adds the Rust side to a
project that predates it and lists the files it kept rather than skipping them in
silence.

### The C ABI host (`--host ffi`)

`htl new --lib --host ffi <name>` is the same library with the C ABI on top, for a caller
that is not written in Rust:

```text
├── Cargo.toml             crate-type = ["rlib", "cdylib", "staticlib"], htl with
│                          features = ["ffi"], serde
├── src/lib.rs             the #[host_module] the scripts call, and a #[c_export] Game
│                          the caller holds: open / a text call / JSON / a status / close
├── include/<mod>.h        written by #[c_export] at cargo build (and by htl dts), committed
├── examples/c/            main.c + a Makefile: every returned char * goes back to _free
└── examples/python/       run.py: restype = c_void_p and ctypes.cast, never c_char_p
```

The two callers are the point. Each language has one way of reading a `char *` that looks
natural and is wrong — C never makes you think about the pointer at all, and Python's
`restype = c_char_p` copies the string and drops the pointer, leaking it every call — so
the scaffold ships a caller in each that does it correctly, and the CI job runs both
against the library it just built.

`--host ffi` requires `--lib` and is refused without it: a `cdylib` has no entry point of
its own, so there is no `src/main.tl` for a binary to run. What the callers do have is a
Lua `error()` and a Rust `Err` to handle — the generated `greet` refuses an empty name in
Teal and `reset` refuses a no-op in Rust — so both error paths are in front of the reader
rather than described.

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

This is a Teal feature, not an htl one; it is documented here because the error above is
what a reader meets first, and it reads like a dead end rather than a pointer to `where`.

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
- **A union of two records**: "cannot discriminate a union between multiple table
  types" reads like a limit on the type system, and it is a limit on `is`, which each
  record can lift for itself with a `where` clause (see above).
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
