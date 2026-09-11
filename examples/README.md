# Examples

Two host crates, and between them the two ways a Rust program can hold Teal: everything
decided at `cargo build`, or everything decided at run time. Both are workspace members, so
every `cargo build --workspace --all-targets` compiles them on both toolchains CI runs, and
`just e2e` runs them — including the one case here that has to fail.

## `embed/` — checked and embedded at `cargo build`

The only binary in this repository where `include_tl!`, `include_bundle!`,
`#[derive(TealRecord)]` and `#[host_module]` all meet. The module doc of
[`embed/src/main.rs`](embed/src/main.rs) says what each one is doing;
[`embed/scripts/main.tl`](embed/scripts/main.tl) is the Teal side calling it. No `.tl` is
read at run time.

```bash
cargo run -p embed                    # main.tl from source, util.tl as stripped bytecode
cargo run -p embed -- --bundle a b    # the same program as one linked bundle
cargo build -p embed --features bad   # must fail: a Teal type error is a Rust build error
```

The first two print the same walk over the host API — a record into `host:scale` and back
out, `&str` and `&[f64]` parameters, a `Result` raising into a `pcall`, a `Mode` crossing as
a string, a `Shape` built in Teal and narrowed back with `is`, `store`'s `errors = "return"`
handing back `value, err` — and end with the arguments they were given (`args: 2 a b`). Two
lines are the reason the walk is there:

```text
host:pace('fst') ->	false	bad argument #2 to `Host.pace`: Mode: expected one of "Fast", "Careful", got "fst"
store:write bad ->	nil	invalid name: no/slash
```

They differ in one line, inside the `pcall` on `host.parse_int`: the traceback reads
`scripts/main.tl:19` from source and `?: in main chunk` from the bundle. Stripping drops the
line numbers along with the chunk name, which is what a bundle is.

`--features bad` adds `include_tl!("scripts/bad.tl")`, whose second line asks for a `string`
from a method that returns a `number`:

```text
error: Teal type check failed:
       …/examples/embed/scripts/bad.tl:4:27: in local declaration: s: got number, expected string
   --> examples/embed/src/main.rs:152:19
```

## `resolver/` — resolved at run time, nothing embedded

`require` goes through mlua-pkg's `Registry` with three resolvers chained, first match wins;
the module doc of [`resolver/src/main.rs`](resolver/src/main.rs) has the chain and what each
link serves. Edit a `.tl` and re-run — there is nothing to rebuild.

```bash
cargo run -p resolver           # entry point defaults to `main`
cargo run -p resolver -- broken # any module name works as the entry point
```

The default run prints this and exits 0:

```text
resolver-example	shift:	2	3
host.double(21):	42.0
legacy.lua:	hello, teal (from legacy.lua)
require('broken') ->	false
```

`resolver-example` is `host.name`, from the table `NativeResolver` builds in Rust; `2 3` came
back through `util.tl`, typed by a `shape.d.tl` that `TealResolver` answers at run time with
an empty table, because a declaration has nothing to run. `legacy.lua` is `FsResolver`:
`legacy.d.tl` declares that module for the checker, and `TealResolver` sees the sibling
`.lua` and steps aside. The last line is the one worth reading twice — `broken.tl` has a type
error, and the `require` fails on it rather than falling through to the next resolver. The
checker's message, with the file and the column, is what the script prints after it.

Naming `broken` as the entry point takes that same failure outside the `pcall`: the message
goes to stderr and the process exits 1.
