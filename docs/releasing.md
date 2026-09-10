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

Two things say so, and neither of them asks crates.io. `cargo test --workspace` carries the
assertion for the key this was learned on: the scaffold round-trip test reads the `htl.toml`
`htl new` wrote and fails if `[toolchain]` is in it, which is a check on the commit that adds
the key rather than on the release that would break. And `just e2e-scaffold-packaged`
scaffolds all three host profiles and builds them against the four `.crate` files the publish
would upload, patched at the extracted trees through `--config` so each generated
`Cargo.toml` is still byte for byte the one a user gets. The chain below runs it before the
first `cargo publish`, so what goes out is gated by it.

All three profiles, because the manifest is written per profile and so is the question.
`--host rust` and `--host rust --lib` pin `htl = "0.4"`; `--host ffi --lib` pins
`htl = { version = "0.4", features = ["ffi"] }`, and whether the crate carries a feature is
not answered by whether it exists at that version. The version is the same in all three —
every profile takes it from `htl_dep_version()`, derived from the CLI's own release.

What the gate builds against is this workspace's own tarballs, so it resolves a key against
the htl about to be published rather than against the one already out there. That gap is what
the checklist below is for: a key added to `htl-core` this cycle is readable by the crate
the *next* cycle's scaffold pins, and the release where the scaffold starts writing it is the
one after the release that publishes it. The arithmetic is the whole answer, and it is worth
more than a build that could only confirm it a release later.

### At each release

- [ ] Did this release publish an `htl.toml` key the scaffold does not write yet? If so, the
      *next* release is where the scaffold starts writing it, and that is the release to
      open with the change. **Owed now: `[toolchain]` and `[lint.rules]` (the level per rule
      that replaced `[lint] enable` / `disable`) are both published by 0.4.0 — so the first
      release after 0.4.0 is where the scaffold starts writing them, and that is the release
      to open with the change. Until 0.4.0 is on crates.io they stay out.**
- [ ] Did anything added to the scaffold's templates this cycle need an unpublished htl?
      Same answer, same list, and it reaches features as well as `htl.toml` keys: a profile
      may only name a feature the htl it pins already carries. This cycle the `ffi` host
      profile is the case to know about: it pins `htl = { version = "0.4", features =
      ["ffi"] }`, and the `ffi` feature is published by the same 0.4.0, so the two arrive
      together. `e2e-scaffold-packaged` scaffolds that profile and builds it, so the feature
      is resolved against the `htl` tarball about to be published — the same bytes, one step
      before the registry holds them. Whether the *already published* crate carries it is not
      asked anywhere any more, which is why this checkbox is a question rather than a build:
      answer it from the list above, not from a green CI run.
- [ ] Run `just e2e-scaffold-packaged` before the first `cargo publish`, as the chain below
      does. It is the step that can still change what goes out; every step after it cannot.

## The chain

The gate first, then four crates published in dependency order, each waiting for the
previous one to be visible in the index before it can be verified.

```sh
cargo publish --dry-run --workspace && \
just e2e-scaffold-packaged && \
cargo publish -p htl-core && sleep 30 && \
cargo publish -p htl-macros && sleep 30 && \
cargo publish -p htl && sleep 30 && \
cargo publish -p htl-cli && \
git tag v<version> && git push origin main && git push origin v<version>
```

`cargo publish --dry-run --workspace` packages all four and stops short of uploading, which
is the cheap way to be told about a manifest the registry would refuse. `just
e2e-scaffold-packaged` then takes the same four tarballs to the other side and builds a
consumer out of them: the CLI installed from the extracted `htl-cli`, the three host
profiles it scaffolds, each compiled against the extracted `htl`, `htl-core` and
`htl-macros`. Between them they ask, of the artefact rather than of the checkout, both
halves of "would this release work" that can be asked without a registry.

They go first because everything below them is irreversible. A version on crates.io cannot
be replaced, only yanked and superseded, and the four steps below are four of those.

The previous release put the scaffold check between the third crate and the fourth and
argued that the window in which a broken scaffold is installable is therefore zero: a
generated project depends on `htl`, which is third, and `htl-cli`, which writes it, is
fourth, so a failure there stops the chain before anyone can obtain the CLI. That is true,
and it is not what a gate is for. The recovery it offered — fix the scaffold, let `htl-cli`
publish at the next patch, since the pin it writes is minor-level (`0.4` whether the CLI
says 0.4.0 or 0.4.1) and still resolves — describes living with the failure rather than
undoing it. Three crates are on crates.io at a version whose CLI does not exist, the release
is half-shipped until another one is made, and nothing that went out comes back. A gate is a
step whose answer can still change what is published; once the first `cargo publish`
returns, no step below it is one.

What the gate cannot see is what no local check can: crates.io's own acceptance of the
upload, index propagation between the steps (the reason for the `sleep 30`s), and docs.rs,
whose build environment is not this one.

Once the last publish is through, the registry question is a lookup rather than a build —
whether the index carries what a generated project pins:

```sh
curl -sS --fail https://index.crates.io/3/h/htl | grep '"vers":"<version>"'
```

That is all it can say, and all that is left to ask: whether a project pinning it builds was
answered by the gate, against the same bytes, before any of this ran.

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
