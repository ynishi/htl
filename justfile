# Development tasks for htl. `just` on its own lists them.
#
# The three gates are named for a moment rather than for their contents: `pre-commit`,
# `pre-push`, `pre-publish`. Each is a superset of the one before it, because each of those
# moments is harder to take back than the last — a commit is amended, a push is reverted in
# public, a publish is yanked and never replaced — and a name that says *when* is the one
# thing a person can act on without reading the recipe. The parts they are assembled from
# stay runnable on their own for when you already know which answer you want: `fmt`,
# `check`, `build`, `e2e`, `e2e-scaffold-packaged`, `bench`.
#
# These wrap what CONTRIBUTING.md already asks for, so that "did I run everything?" has one
# answer instead of four commands to remember in the right order.

_default:
    @just --list

# Everything before a commit: format, then the green CONTRIBUTING defines.
pre-commit: fmt check

# `check` directly rather than a dependency on `pre-commit`, which would also run `fmt`.
# Reformatting the tree at push time either does nothing, or it edits files the commits
# being pushed do not contain — and then what goes out is not what was reviewed, while the
# reformat itself goes out unrecorded. Formatting belongs at the commit above, where it is
# still part of a commit.
#
# `build` and `e2e` are here and not there because of what the two moments ask. A commit is
# a local checkpoint, and the question then is whether the workspace still holds together,
# which `check` answers on its own. A push hands the branch to CI and to whoever reads it,
# so the questions worth asking are the ones CI would otherwise ask minutes later: this is
# the same work its two jobs do, and a red run there reproduces here. `build` is not
# redundant beside `check` — run after `cargo test` and clippy have both finished green it
# still compiles, because it is the one of the three that emits each target's artefact
# rather than checking it.
# Everything before a push: the green, a full compile, and every end-to-end case.
pre-push: check build e2e

# The gate goes last, and not only because it is the slowest thing here. It asks git what
# belongs in a tarball, so it is the one recipe that an uncommitted change can stop — a
# change to a file one of the four crates ships, that is, which it names; a dirty justfile
# or doc does not stop it, because neither would have reached a tarball either way. Running
# it after everything cheaper means its minute is spent on a tree that has already answered
# every question answerable without packaging.
# Everything before a publish: `pre-push`, then the release gate on the four tarballs.
pre-publish: pre-push e2e-scaffold-packaged

# Format in place.
fmt:
    cargo fmt --all

# `cargo test` and not `cargo test --workspace`, and the two are no longer the same thing:
# `e2e` is a workspace member that `default-members` leaves out, so the bare form runs every
# test in the repository except the three that scaffold a Cargo project and build it from
# nothing. Those are minutes, they are what `just e2e` is for, and putting them here would
# put them in front of every commit and in both toolchains of the check job.
#
# clippy keeps `--workspace --all-targets`, which is what stops `e2e` rotting: it is
# compiled and linted by this recipe and by CI, and only ever run by `e2e` below.
# Green: the whole workspace, tests and lints. What CI runs.
check:
    cargo test
    cargo clippy --workspace --all-targets

# Compile everything, including tests and benches, without running any of it.
build:
    cargo build --workspace --all-targets

# What is left here is what `cargo test` will not run on its own: a binary crate embedding
# htl, and three Cargo projects scaffolded from nothing and built. Neither is a claim about
# this workspace, and both are minutes. The CLI's own behaviour on files — check, test, the
# run cache, `cache status`, and the two suites meant to fail — moved to
# `crates/htl-cli/tests/sample_project.rs`, and the three host projects to `e2e/`, where a
# failure names a file, a line and what it saw instead of reporting that a `grep` exited 1.
# `just check` runs the first of those; this runs the second, by package name.
# The embedding example and every host `--host` offers, end to end.
e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    # The embed example through both the include_tl! and the include_bundle! path, and the
    # resolver example, which resolves its `.tl` at run time instead of embedding any. Both
    # were workspace members that every `--workspace` command compiled; only `embed` was
    # ever run.
    cargo run -q -p embed
    cargo run -q -p embed -- --bundle
    cargo run -q -p resolver
    # `embed`'s `bad` feature adds an `include_tl!` of a module that does not type check,
    # and its manifest says so — "demonstrates a Teal type error failing the Rust build".
    # Nothing had ever run it, so the demonstration was a claim. Spelled `if …; then exit 1`
    # rather than `! cargo build`, because bash exempts a `!`-inverted command from `set -e`:
    # that is the form that let a gate in this file report nothing however the manifest
    # looked, and it would fail here in the direction that looks like success.
    if cargo build -q -p embed --features bad 2>/dev/null; then
      echo 'embed --features bad compiled: a Teal type error did not fail the Rust build' >&2
      exit 1
    fi
    # Every host `--host` offers, scaffolded outside this repository and built against this
    # checkout. That was 87 lines of bash here; it is now three tests in the `e2e` member
    # crate, which `default-members` keeps out of `cargo test` and this line asks for by
    # name. The crate finds the binary and the target directory itself and defaults every
    # path to this checkout, so there is nothing to pass — `e2e-scaffold-packaged` below
    # sets the five variables that point the same three tests somewhere else.
    cargo test -p e2e

# The release gate: the CLI suite and the same three host projects, run against the four
# `.crate` files `cargo publish` would upload rather than against this checkout. Everything
# the scaffold case of `e2e` cannot see lives in the difference between the two —
# `include` / `exclude` deciding which files reach the tarball, an `include_str!` target
# that is ignored, the manifest normalisation that strips `[workspace]` and rewrites every
# path dependency to its version key — and each of those breaks a release while leaving
# every check that runs on the checkout green. `htl-core` is the crate with an `include`,
# and it is narrow (`src/**/*`, `lua/**/*`, `vendor/**/*`), so a file added anywhere else
# in that crate and read with `include_str!` is in every checkout and in no tarball.
#
# Two steps ask that, not one. `cargo package` builds each extracted crate to verify it, so
# a missing `include_str!` target fails there, before anything below it runs; what the
# scaffolding after it adds is the same tarballs compiled the way a consumer compiles them,
# with the features a consumer turns on — `htl` with `ffi` for the C ABI host, which the
# default-feature verification above never builds.
#
# It is its own recipe rather than a case in `e2e` because it needs a clean worktree and
# several minutes, and a case that refuses to run while a commit is being written does not
# belong in the recipe run while one is. `pre-publish` is what runs it, and it has to be the
# last thing asked before the first `cargo publish`: a version on crates.io is yanked and
# superseded, never replaced, so this is the last step whose answer can still change what
# goes out.
# The release gate: the CLI suite and three scaffolded hosts, asked of the four tarballs a publish would upload.
e2e-scaffold-packaged:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(pwd)"
    # The same target directory `e2e` uses for its own scaffolds: mlua and the graph beneath
    # it are compiled once, and what is built again is the htl crates, from their new paths.
    target="${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold"
    # Four crates in one command, because three of them depend on each other at versions
    # nobody has published: while it verifies each tarball by building it, cargo overlays
    # the packages it is building on the registry, so `htl`'s requirement on `htl-core`
    # 0.4.0 resolves to the `htl-core` 0.4.0 being packaged beside it. No `--allow-dirty`:
    # what is under test has to be what git has, or it is not the tarball that would be
    # uploaded — and a clean worktree is also what puts .cargo_vcs_info.json inside it.
    # Cargo scopes that to the files it is about to ship, which is the right scope and
    # narrower than it sounds: an uncommitted README or justfile is not refused here,
    # because neither is in any of the four tarballs.
    cargo package --target-dir "$target" -p htl-core -p htl-macros -p htl -p htl-cli
    ver="$(cargo pkgid -p htl-core | sed 's/.*[#@]//')"
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    # Extracted out of the repository, rather than read where cargo leaves its own unpack,
    # for two reasons. The tarball is the artefact and the unpack is a by-product of
    # verifying it, so untarring is the step that says what a consumer receives. And that
    # unpack sits under this workspace root with `[workspace]` stripped from its manifest,
    # which is precisely the arrangement cargo refuses to build: it walks up, finds this
    # workspace, and reports a package that believes it is not in one.
    for crate in htl-core htl-macros htl htl-cli; do
      tar -xzf "$target/package/$crate-$ver.crate" -C "$dir"
    done
    # The binary that writes the scaffolds below, built from the extracted tree, so that
    # the CLI under test is the one being shipped and the thirteen templates it reads with
    # `include_str!` are proved to be in the tarball rather than only in the checkout. Its
    # three library dependencies are patched at their extracted trees for the same reason
    # the projects below are: the version they name is not on crates.io while this runs,
    # and this is the run that decides whether it should be. --debug because the question
    # is what it builds and what it writes, and a debug build shares the graph the projects
    # below compile against instead of compiling a second one. cargo reports the htl-macros
    # patch as unused here and is right to: the CLI takes `htl` with default features off,
    # so the proc macros are not in its graph. It is passed anyway, because the alternative
    # to patching a crate that is not published is resolving it, and if this dependency ever
    # arrives the failure should not be a resolution error about crates.io.
    cargo install --locked --debug --path "$dir/htl-cli-$ver" --root "$dir/cli" \
      --target-dir "$target" \
      --config "patch.crates-io.htl.path='$dir/htl-$ver'" \
      --config "patch.crates-io.htl-core.path='$dir/htl-core-$ver'" \
      --config "patch.crates-io.htl-macros.path='$dir/htl-macros-$ver'"
    # Two packages, pointed at the CLI just installed and at the three extracted trees
    # instead of at this checkout. `e2e` is the same three tests `e2e` runs; `htl-cli` is
    # its whole integration suite — 206 tests across 34 files that spawn `htl_bin()`, which
    # is `HTL_TEST_BIN` when it is set. Five variables are the whole of the difference
    # between the two gates, as five positional arguments were when this was a shared bash
    # recipe — a release gate that checked less than the loop running on every commit would
    # be the wrong way round, and one implementation is how that stays true.
    #
    # What the second package adds is not the file set. `cargo package` verifies each
    # tarball by building it, so an `include_str!` target left out of one fails above,
    # before this line runs, and every file these four crates ship is read that way. What
    # is left is everything the shipped binary *does* that compiling it does not ask:
    # the bytes `htl new` writes for each host profile, what `fmt` writes, which lints
    # fire, what the run cache reports. The three scaffold tests see one corner of that —
    # they build what `htl new` wrote, so a template can differ from this checkout's copy
    # in any way that still compiles and they stay green.
    #
    # Both packages in one `cargo test`, in this workspace's own target directory: nothing
    # here sets `CARGO_TARGET_DIR`, so `$target` above is for the packaging and the
    # scaffolds only. It is not free the first time even so — `check` builds the six
    # default members together, which unifies `htl` to five features, and asking for two
    # packages resolves it to (dts, pkg), a different unit — so the first run in a given
    # target directory compiles that graph. The two sets then coexist: measured at +35 s
    # once and +8 s after, with `cargo test` still finding everything it left behind.
    #
    # The variables are set on the command rather than exported: `HTL_TEST_BIN` left in a
    # shell is a suite reporting on a binary that stopped matching the source.
    HTL_TEST_BIN="$dir/cli/bin/htl" \
    HTL_E2E_TARGET="$target" \
    HTL_PATCH_HTL="$dir/htl-$ver" \
    HTL_PATCH_CORE="$dir/htl-core-$ver" \
    HTL_PATCH_MACROS="$dir/htl-macros-$ver" \
      cargo test -p htl-cli -p e2e

# Every benchmark: the figures in the README come from these. Ten samples each; a few minutes.
bench:
    cargo bench -p htl-core --bench check
    cargo bench -p htl-core --bench fix
    cargo bench -p htl-cli --bench cached_check
