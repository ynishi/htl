# `dep_dts` fixture

Two crates: `dep` ships a Teal declaration and names it in its manifest, `consumer`
depends on `dep` by path and is the project `htl dts` runs in.

`dep_dts.rs` copies the pair into a scratch directory before running anything — `htl dts`
writes into `consumer/types/`, and a test that wrote into the checkout would be a test
that only passes once.

The manifests are `Cargo.toml.in`, renamed on the way into the scratch directory. A real
`Cargo.toml` here would be a package nested inside this workspace: `cargo` would want it
to be a member or excluded, and it would travel inside `htl-cli`'s published archive.
Nothing else about them is a stand-in — what the test resolves is what cargo resolves.
