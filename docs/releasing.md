# Releasing htl

Four crates, published in dependency order. Each waits for the previous one to be
visible in the index before it can be verified.

```sh
cargo publish -p htl-core && sleep 30 && \
cargo publish -p htl-macros && sleep 30 && \
cargo publish -p htl && sleep 30 && \
cargo publish -p htl-cli && \
git tag v0.1.N && git push origin main && git push origin v0.1.N
```

The chain is `&&`-joined on purpose: if a step fails, nothing after it runs, so a
failure leaves no half-tagged, half-pushed state. Check which crates went out
(`cargo search htl-core` or the crates.io page) before re-running; a crate that was
already published for that version fails with "already exists" and the rest continues
from where it stopped once you drop the published ones from the chain.

## crates.io rate limit

crates.io limits how many **versions of one crate** can be published in 24 hours. Nine
versions of `htl-core` in a day were accepted; the tenth was refused:

```
error: failed to publish htl-core v0.1.20 to registry at https://crates.io
Caused by:
  the remote server responded with an error (status 429 Too Many Requests):
  You have published too many versions of this crate in the last 24 hours
```

Nothing is uploaded when this happens. What to do:

1. Push the commit anyway (`git push origin main`); the tag waits for the publish.
2. Consumers who need the fix now use the local checkout (next section).
3. Re-run the same chain once the window has passed.

Patch releases per dogfood round add up quickly; batching two or three fixes into one
version avoids the limit without holding anything back.

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
warning: patch `htl v0.1.20 (...)` was not used in the crate graph
```

Run `cargo update -p htl -p htl-core -p htl-macros` once; the lock then points at the
local paths. To go back after the version is on crates.io, delete the `[patch.crates-io]`
block and run the same `cargo update` again. A caret requirement such as
`htl = "0.1.19"` already accepts `0.1.20`, so nothing else changes.
