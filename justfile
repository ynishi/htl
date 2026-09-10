# Development tasks for htl. `just` on its own lists them.
#
# These wrap what CONTRIBUTING.md already asks for, so that "did I run everything?" has one
# answer instead of four commands to remember in the right order.

_default:
    @just --list

# Everything before a commit: format, then the green CONTRIBUTING defines.
preflight: fmt check

# Green: the whole workspace, tests and lints. What CI runs.
check:
    cargo test --workspace
    cargo clippy --workspace --all-targets

# Format in place.
fmt:
    cargo fmt --all

# Fail if anything is unformatted, without changing it.
fmt-check:
    cargo fmt --all -- --check

# The gate at the end is last because it is the slower of the two end-to-end recipes, and
# because it asks git what to put in a tarball. An uncommitted change to a file one of the
# four crates ships stops it, and it names that file; a change to anything else — this
# justfile, a doc, a test — does not, because it would not have reached the tarball either
# way. So a dirty checkout is not a reason not to run this, but a dirty *crate* is a reason
# it will refuse.
# Everything CI runs on a push, in the same order, so a failure there reproduces here.
ci: build check e2e e2e-scaffold-packaged

# Compile everything, including tests and benches, without running any of it.
build:
    cargo build --workspace --all-targets

# One recipe rather than four, because the cases below ask one question of four surfaces, and
# because each of them answers with a pass or a fail rather than with figures for a person to
# read and judge.
# The CLI, the embedding example, the run cache and every host `--host` offers, end to end.
e2e:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(pwd)"
    # Built once and then invoked by path, rather than through `cargo run` each time: the
    # cache case below runs from a directory outside this workspace, where `cargo run` has no
    # manifest to find, and the scaffold case wants the same binary anyway.
    cargo build -q -p htl-cli --bin htl
    htl="${CARGO_TARGET_DIR:-$root/target}/debug/htl"
    # The CLI on the repository's own .tl (examples/tl/failing/ is meant to fail and is not
    # run here), and the embed example through both the include_tl! and the include_bundle!
    # path.
    "$htl" check examples/tl/util.tl examples/tl/main.tl examples/tl/util_test.tl
    "$htl" test examples/tl/util_test.tl
    cargo run -q -p embed
    cargo run -q -p embed -- --bundle
    # The run cache, checked twice over the same three files: once against a store that does
    # not exist yet, once against the one the first run left. A store lives beside the
    # `htl.toml` a project has and this repository's root has none, so the CLI falls back to
    # the working directory — which is why the pair runs from a temporary one. That is what
    # makes "cold" mean cold on a machine that has checked this repository before, and it
    # leaves nothing behind in the checkout.
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    files=("$root/examples/tl/util.tl" "$root/examples/tl/main.tl" "$root/examples/tl/util_test.tl")
    (
      cd "$dir"
      # Two claims per run, and only the second is the point. --explain-cache prints the
      # store's own counters, and the check prints its summary, which ends in `[cached]`
      # only when every file in the walk was replayed rather than checked. Both are on
      # stderr, where everything a person reads goes (stdout is kept for --format json), so
      # the run is captured whole rather than by stream. A store that was written and a run
      # that was answered out of it are different facts, and a case that asserted the first
      # would pass while the cache saved nothing.
      "$htl" check "${files[@]}" --explain-cache >cold.log 2>&1
      cat cold.log
      grep -q '^htl cache: 0 hit, 0 missed, 3 written,' cold.log
      "$htl" check "${files[@]}" --explain-cache >warm.log 2>&1
      cat warm.log
      grep -q '^htl cache: 3 hit, 0 missed, 0 written,' warm.log
      grep -q ' \[cached\]$' warm.log
      # And the store from the outside, which is the other half of the same fact: three
      # modules were checked, so three entries are what the second run had to read.
      "$htl" cache status >status.log 2>&1
      cat status.log
      grep -q '^htl cache: 3 entries,' status.log
    )
    # Every host `--host` offers: scaffolded into a temporary directory outside this
    # repository, pointed back at this checkout so it is *this* htl that is embedded, then
    # built, tested and run — including, for the C ABI host, the reference callers in C and
    # Python that load the library it builds. The snapshot tests pin what the scaffold writes
    # byte for byte; only this says the bytes compile and work. The artefacts land beside the
    # workspace's under a directory of their own, because the scaffold sets
    # `[profile.dev.build-override]` and sharing one target directory would rebuild the proc
    # macro dependencies on every switch.
    {{just_executable()}} _scaffold-hosts "$htl" \
      "${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold" \
      "$root/crates/htl" "$root/crates/htl-core" "$root/crates/htl-macros"

# The three host projects, and everything asked of them, in one place: the scaffold case of
# `e2e` above and `e2e-scaffold-packaged` below differ in exactly two things — which htl
# writes the projects ({{htl}}) and which trees they are built against ({{htl_path}},
# {{core_path}}, {{macros_path}}) — and in nothing else. That is the point of the split: a
# release gate that checked less than the loop that runs on every commit would be the wrong
# way round.
_scaffold-hosts htl target htl_path core_path macros_path:
    #!/usr/bin/env bash
    set -euo pipefail
    # Scaffolding inside the repository would leave a Cargo package in the checkout and
    # run the CLI against the repository's own htl.toml, so the project goes elsewhere
    # entirely and only its build artefacts come back.
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    target="{{target}}"
    sample=(--config "patch.crates-io.htl.path='{{htl_path}}'"
            --config "patch.crates-io.htl-core.path='{{core_path}}'"
            --config "patch.crates-io.htl-macros.path='{{macros_path}}'"
            --target-dir "$target")
    # What each of these projects holds, and what they must keep holding whoever built them:
    # a pin on the released htl, with nothing in the manifest redirecting it. The patch above
    # is handed to cargo through --config, off to one side, so that a generated Cargo.toml
    # stays byte for byte the one a user gets. Both halves are spelled `if …; then exit 1`
    # rather than `! grep …`, because that second form cannot fail a recipe: bash exempts a
    # command whose status is inverted with `!` from `set -e`, so it reported nothing however
    # the manifest looked.
    assert_unpatched_pin() {
      if ! grep -q '^htl = ' Cargo.toml; then
        echo "$PWD/Cargo.toml: names no htl to build against" >&2
        exit 1
      fi
      if grep -q 'patch.crates-io' Cargo.toml; then
        echo "$PWD/Cargo.toml: redirects its own pin, so this is not the manifest a user gets" >&2
        exit 1
      fi
    }
    # Each project is built in a subshell: the scaffolding command above has to be back in
    # this workspace, not in the one that was just scaffolded.
    {{htl}} new "$dir/hostsample" --host rust
    (
      cd "$dir/hostsample"
      assert_unpatched_pin
      cargo test "${sample[@]}"
      out="$(cargo run -q "${sample[@]}" -- Ada)"
      printf '%s\n' "$out"
      printf '%s\n' "$out" | grep -qx 'hello from Rust, Ada'
    )
    # --lib is the same host without a binary: the library still builds and its test still
    # goes through preload, and there is no entry point for cargo run to find.
    {{htl}} new "$dir/libsample" --host rust --lib
    (
      cd "$dir/libsample"
      test ! -e src/main.rs
      test ! -e src/main.tl
      assert_unpatched_pin
      cargo test "${sample[@]}"
    )
    # The C ABI host, whose callers are the part nothing else here compiles: the library
    # is built, the header the macro writes is checked, and the two reference hosts under
    # examples/ are run against the artefact — the Python one wherever python3 is, the C
    # one only where there is a compiler and a make.
    {{htl}} new "$dir/ffisample" --host ffi --lib
    (
      cd "$dir/ffisample"
      test ! -e src/main.rs
      test ! -e src/main.tl
      # The same pin, with the feature the profile needs on it — `htl = { version = "0.4",
      # features = ["ffi"] }` is still one `htl =` line naming the release, and still
      # nothing patches it here.
      assert_unpatched_pin
      cargo test "${sample[@]}"
      cargo build "${sample[@]}"
      # Written by #[c_export] at build time, not by the scaffold: it is not there until
      # the library is built, and then it declares what the callers below call.
      grep -q 'ffisample_handle \*ffisample_open(const char \*options_json);' include/ffisample.h
      test -f "$target/debug/libffisample.a"
      if command -v python3 >/dev/null; then
        out="$(CARGO_TARGET_DIR="$target" python3 examples/python/run.py)"
        printf '%s\n' "$out"
        printf '%s\n' "$out" | grep -q 'greet          -> the Python host: hello, Ada'
        printf '%s\n' "$out" | grep -q 'schema v1'
        # The Lua error the Teal module raises, as a status rather than as a crash.
        printf '%s\n' "$out" | grep -q 'greet("")      -> NULL, status 4'
      else
        echo 'no python3: skipping examples/python'
      fi
      if command -v cc >/dev/null && command -v make >/dev/null; then
        out="$(make -s -C examples/c run LIBDIR="$target/debug")"
        printf '%s\n' "$out"
        printf '%s\n' "$out" | grep -q '{"greeted":1,"greeter":"the C host","v":1}'
        printf '%s\n' "$out" | grep -q 'reset again    -> status 1'
      else
        echo 'no C compiler: skipping examples/c'
      fi
    )

# The release gate: the same three host projects, built against the four `.crate` files
# `cargo publish` would upload rather than against this checkout. Everything the scaffold
# case of `e2e` cannot see lives in the difference between the two — `include` / `exclude`
# deciding which files reach the tarball, an `include_str!` target that is ignored, the
# manifest normalisation that strips `[workspace]` and rewrites every path dependency to
# its version key — and each of those breaks a release while leaving every check that runs
# on the checkout green. `htl-core` is the crate with an `include`, and it is narrow
# (`src/**/*`, `lua/**/*`, `vendor/**/*`), so a file added anywhere else in that crate and
# read with `include_str!` is in every checkout and in no tarball.
#
# Two steps ask that, not one. `cargo package` builds each extracted crate to verify it, so
# a missing `include_str!` target fails there, before anything below it runs; what the
# scaffolding after it adds is the same tarballs compiled the way a consumer compiles them,
# with the features a consumer turns on — `htl` with `ffi` for the C ABI host, which the
# default-feature verification above never builds.
#
# It is its own recipe rather than a case in `e2e` because it needs a clean worktree and
# several minutes, and a case that refuses to run while a commit is being written does not
# belong in the recipe run while one is. The chain (docs/releasing.md § The chain) runs it
# before the first `cargo publish`, because after that there is nothing left to do with the
# answer.
# The release gate, asked of the four tarballs a publish would upload rather than of the checkout.
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
    {{just_executable()}} _scaffold-hosts "$dir/cli/bin/htl" "$target" \
      "$dir/htl-$ver" "$dir/htl-core-$ver" "$dir/htl-macros-$ver"

# Every benchmark: the figures in the README come from these. Ten samples each; a few minutes.
bench:
    cargo bench -p htl-core --bench check
    cargo bench -p htl-core --bench fix
    cargo bench -p htl-cli --bench cached_check

# A release binary, for timing against a real project rather than a generated one.
release:
    cargo build --release -p htl-cli
