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
Checking htl-core v0.2.0 -> v0.3.0 (major change)
 Summary no semver update required
```

and that second answer is what the chain below should not run without.

The bump itself rides with the last change of the release rather than a commit of its own
(CONTRIBUTING § Commits).

## The chain

Four crates, published in dependency order. Each waits for the previous one to be
visible in the index before it can be verified.

```sh
cargo publish -p htl-core && sleep 30 && \
cargo publish -p htl-macros && sleep 30 && \
cargo publish -p htl && sleep 30 && \
cargo publish -p htl-cli && \
git tag v<version> && git push origin main && git push origin v<version>
```

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
