# Releasing htl

## Before the chain: measure what moved

The version number is a measurement, not a judgement. `cargo semver-checks` compares the
working tree against what is on crates.io and says which half has to move:

```sh
cargo semver-checks check-release -p htl-core --baseline-version <the published version>
```

About 30 seconds, and it needs nothing said about the crate. Any finding at all moves the
minor under `0.y.z` — cargo treats the minor as the major there — and would move the major
after 1.0. No findings means the patch is enough.

The check that is hard to make by eye is `constructible_struct_adds_field`: **a new public
field on a struct whose fields are all public is a minor release**, because it breaks every
struct literal a consumer wrote. Five changes did that here without saying so —
`Contract.enforced_by` (#65), `HostMethod.is_async` (#63), `Project.patches` (#77),
`Project.vendored_copies` (#88), `CheckInfo.dependency_errors` (#81). Removing a public
field, removing a method and changing a function's arity are the same class and are easier
to see coming.

`htl-core` is the crate to check: `htl` re-exports it wholesale, so its surface is the one
a consumer holds.

Run it once before deciding the number and once after writing it in. With the number moved
the same command answers

```
Checking htl-core v0.3.0 -> v0.4.0 (major change)
 Summary no semver update required
```

and that second answer is what the chain below should not run without.

The bump itself rides with the last change of the release rather than a commit of its own
(CONTRIBUTING § Commits).

## What the scaffold may write

The same arithmetic, one release out of phase: **a project `htl new` writes may only name
configuration the htl it pins can read, so anything added to `htl.toml` reaches the scaffold
one release after it reaches the library.**

A host project depends on the *released* crate — `htl_dep_version()` derives `htl = "0.4"`
from the CLI's own version, deliberately, so a project follows the htl that produced it.
`HtlConfig` is `#[serde(deny_unknown_fields)]`, so a key that exists only in this workspace
is not ignored out there; it is fatal, inside `include_tl!`, at the project's first
`cargo build`:

```
error: include_tl!: .../htl.toml: parsing htl.toml: TOML parse error at line 8, column 2
       unknown field `toolchain`, expected one of `lint`, `fmt`, `check`, ...
```

So each key crosses in two steps: it lands in `htl-core` and is published, and only the
release *after* that may write it into a scaffold. `[toolchain]` is the key this was learned
on (#153); it was written a release early and every project `htl new` wrote in between
failed to build. `[lint.rules]` is the second (#147), and it is out of the scaffolded
`htl.toml` until a published htl parses it.

A **commented** example counts as writing it. It is one user action away from being a key,
and the user who uncomments it is building against the pinned release, so the failure is the
same one arriving later. The rule cuts both ways when a key is replaced rather than added:
`[lint.rules]` took over from `[lint] enable` / `disable`, and showing those instead would
hand a fresh project a line the CLI that just made it rejects — breaking `htl check`, the
first thing a user runs. A scaffold that cannot name either names neither, and points at a
command instead (`htl check --list-lints`), which answers from whichever binary is in hand.

`just e2e-scaffold-published` is what says so during the cycle: it scaffolds and builds with
no `[patch.crates-io]`, against the crate the project actually pins, and the CI scaffold job
runs it. `just e2e-scaffold` cannot — it points the three crates at this checkout, where
every key this branch added exists.

It says so during the cycle and not at the end of one. The pin is derived from the CLI's own
version, so on the release commit it names the version being released, and that version is
not on crates.io until the chain puts it there. A build attempted then reports that
`htl = "0.4"` selects nothing — a fact about the registry, not about the scaffold, and one
that turns the CI scaffold job red for as long as the release takes. So the recipe asks the
index which case it is in, and when the pin is unpublished it names the pin it could not
resolve and defers rather than building. What it defers to is the chain below, which runs it
after `htl` is published and before `htl-cli` is: the one moment the question both has an
answer and can still change what goes out.

### At each release

- [ ] Did this release publish an `htl.toml` key the scaffold does not write yet? If so, the
      *next* release is where the scaffold starts writing it, and that is the release to
      open with the change. **Owed now: `[toolchain]` and `[lint.rules]` (the level per rule
      that replaced `[lint] enable` / `disable`) are both published by 0.4.0 — so the first
      release after 0.4.0 is where the scaffold starts writing them, and that is the release
      to open with the change. Until 0.4.0 is on crates.io they stay out.**
- [ ] Did anything added to the scaffold's templates this cycle need an unpublished htl?
      Same answer, same list. This cycle the `ffi` host profile is the case to know about:
      it pins `htl = { version = "0.4", features = ["ffi"] }`, and the `ffi` feature is
      published by the same 0.4.0, so the two arrive together. Nothing here checks that —
      `e2e-scaffold-published` scaffolds `--host rust --lib` only, so a feature a profile
      names is never resolved against the registry.
- [ ] Run `just e2e-scaffold-published` from inside the chain, not before it. On the release
      commit the pin is the version being released, so the recipe defers and says so; that
      deferral is not a pass and the step in the chain is where it stops being one. Do not
      drop the step to make the chain shorter.

## The chain

Four crates, published in dependency order. Each waits for the previous one to be
visible in the index before it can be verified.

```sh
cargo publish -p htl-core && sleep 30 && \
cargo publish -p htl-macros && sleep 30 && \
cargo publish -p htl && sleep 30 && \
just e2e-scaffold-published && \
cargo publish -p htl-cli && \
git tag v<version> && git push origin main && git push origin v<version>
```

The scaffold check sits between the third crate and the fourth because that is the only
place it can be asked and still be worth asking. A generated project depends on `htl`
alone, and `htl` is third; `htl-cli`, the binary that writes the scaffold and derives its
pin, is fourth. In the gap between them crates.io holds exactly what a scaffold pins, and
nobody can yet obtain the CLI that pins it — so a scaffold that does not build against the
crates just published stops the chain before `htl-cli` goes out. The window in which a
broken scaffold is installable is not narrowed; there is no window.

Recovery from a failure there is cheaper than it sounds. Three crates are out and are
right — what failed is what the CLI writes, not what the library is. Fix the scaffold and
let `htl-cli` publish at the next patch: the pin it writes is minor-level (`0.4` whether the
CLI says 0.4.0 or 0.4.1), so it still names the `htl` already on crates.io and still
resolves. Nothing has to be yanked and no number has to move.

The chain is `&&`-joined on purpose: if a step fails, nothing after it runs, so a
failure leaves no half-tagged, half-pushed state. Before re-running, check which
crates went out (`cargo search htl-core` or the crates.io page). A crate already
published at that version fails with "already exists", so drop the published ones
from the chain and run the rest.

## When crates.io refuses a version

crates.io caps how many versions of one crate can be published per 24 hours; a
patch release per dogfood round reaches it. The refusal is a
`429 Too Many Requests` ("published too many versions of this crate in the last
24 hours") at upload time, and nothing is uploaded. Then:

1. Push the commit anyway; the tag waits for the publish.
2. Consumers who need the fix now use the local checkout (next section).
3. Re-run the same chain once the window has passed.

Batching two or three fixes into one version avoids it without holding anything
back.

## Using an unpublished htl from a local checkout

In the consumer's `Cargo.toml`:

```toml
[patch.crates-io]
htl = { path = "/path/to/htl/crates/htl" }
htl-core = { path = "/path/to/htl/crates/htl-core" }
htl-macros = { path = "/path/to/htl/crates/htl-macros" }
```

All three are needed: `htl` re-exports `htl-core`, and the proc macros in
`htl-macros` run `htl-core` at expansion time, so patching only `htl` mixes two
versions.

**The patch is ignored until the lockfile is updated.** `Cargo.lock` keeps the
resolved version, and cargo says so rather than switching:

```
warning: patch `htl v0.2.0 (...)` was not used in the crate graph
```

Run `cargo update -p htl -p htl-core -p htl-macros` once; the lock then points at the
local paths. To go back after the version is on crates.io, delete the `[patch.crates-io]`
block and run the same `cargo update` again.

Whether the consumer needs anything else depends on which number moved. Under `0.y.z`
cargo treats the minor as the major, so `htl = "0.1.19"` accepts `0.1.20` and nothing
else changes, while it does **not** accept `0.2.0`: a release that bumps the minor is one
where every consumer edits its requirement, which is the point of bumping it.
