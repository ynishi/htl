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

# The gate at the end is last because it is the slowest of the three scaffold recipes, and
# because it wants what CI always has and a working checkout often does not: a committed
# tree. Run this on one — from a dirty tree the gate stops on the uncommitted files by
# design, and names them.
# Everything CI runs on a push, in the same order, so a failure there reproduces here.
ci: build check e2e e2e-scaffold e2e-scaffold-published e2e-scaffold-packaged

# Compile everything, including tests and benches, without running any of it.
build:
    cargo build --workspace --all-targets

# The CLI and the embedding example, end to end, as the second CI job does.
e2e:
    cargo run -q -p htl-cli --bin htl -- check examples/tl/util.tl examples/tl/main.tl examples/tl/util_test.tl
    cargo run -q -p htl-cli --bin htl -- test examples/tl/util_test.tl
    cargo run -q -p embed
    cargo run -q -p embed -- --bundle

# Every host `--host` offers, end to end: scaffolded into a temporary directory outside
# this repository, pointed back at this checkout so it is *this* htl that is embedded,
# then built, tested and run — including, for the C ABI host, the reference callers in C
# and Python that load the library it builds. The snapshot tests pin what the scaffold
# writes byte for byte; only this says the bytes compile and work.
e2e-scaffold:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(pwd)"
    # A scaffolded project depends on the released htl, which is what a user gets; for the
    # change under test to be the one that runs, point the three crates at this checkout.
    # Its artefacts land beside the workspace's under a directory of their own, because
    # the scaffold sets `[profile.dev.build-override]` and sharing one target directory
    # would rebuild the proc macro dependencies on every switch.
    {{just_executable()}} _scaffold-hosts "cargo run -q -p htl-cli --bin htl --" \
      "${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold" \
      "$root/crates/htl" "$root/crates/htl-core" "$root/crates/htl-macros"

# The three host projects, and everything asked of them, in one place: `e2e-scaffold` above
# and `e2e-scaffold-packaged` below differ in exactly two things — which htl writes the
# projects ({{htl}}) and which trees they are built against ({{htl_path}}, {{core_path}},
# {{macros_path}}) — and in nothing else. That is the point of the split: a release gate
# that checked less than the loop that runs on every commit would be the wrong way round.
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
    # Each project is built in a subshell: the scaffolding command above has to be back in
    # this workspace, not in the one that was just scaffolded.
    {{htl}} new "$dir/hostsample" --host rust
    (
      cd "$dir/hostsample"
      # What a user's project holds, and what these must keep holding whoever built them:
      # a plain pin on the released htl, with nothing in the manifest redirecting it. The
      # patch above is handed to cargo through --config, off to one side, so that a
      # generated Cargo.toml stays byte for byte the one a user gets.
      grep -q '^htl = ' Cargo.toml
      ! grep -q 'patch.crates-io' Cargo.toml
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
      grep -q '^htl = ' Cargo.toml
      ! grep -q 'patch.crates-io' Cargo.toml
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
      # features = ["ffi"] }` is still a plain pin, and still nothing patches it here.
      grep -q '^htl = ' Cargo.toml
      ! grep -q 'patch.crates-io' Cargo.toml
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
# `cargo publish` would upload rather than against this checkout. Everything `e2e-scaffold`
# cannot see lives in the difference between the two — `include` / `exclude` deciding
# which files reach the tarball, an `include_str!` target that is untracked or ignored, the
# manifest normalisation that strips `[workspace]` and rewrites every path dependency to
# its version key — and each of those breaks a release while leaving every check that runs
# on the checkout green. So this asks the question of the artefact, and the chain
# (docs/releasing.md § The chain) asks it before the first `cargo publish`, because after
# that there is nothing left to do with the answer.
e2e-scaffold-packaged:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(pwd)"
    # The same target directory as the two recipes around it: mlua and the graph beneath it
    # are compiled once, and what is built again is the htl crates, from their new paths.
    target="${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold"
    # Four crates in one command, because three of them depend on each other at versions
    # nobody has published: while it verifies each tarball by building it, cargo overlays
    # the packages it is building on the registry, so `htl`'s requirement on `htl-core`
    # 0.4.0 resolves to the `htl-core` 0.4.0 being packaged beside it. No `--allow-dirty`:
    # what is under test has to be what git has, or it is not the tarball that would be
    # uploaded — and a clean worktree is also what puts .cargo_vcs_info.json inside it.
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

# The same scaffold, built against the htl on crates.io rather than this checkout: no
# `[patch.crates-io]`, so the project compiles against the crate it actually pins — a
# release behind the workspace, which is what a user has. That gap is the point. A key
# written into htl.toml before a release carries it, a template that only the workspace's
# checker accepts, a dependency requirement that does not resolve: none of it is visible to
# `e2e-scaffold` above, which patches the disagreement away by construction.
#
# It runs in the same CI job as `e2e-scaffold` and against the same target directory, so
# mlua and the rest of the graph are compiled once and only the three htl crates are built
# twice. Building is enough: the failure this exists to catch is an expansion-time one, and
# `include_tl!` runs during the build.
#
# What it is for is the weeks before a release rather than the release: the commit that
# teaches the scaffold to write a key no published htl can parse is caught here, on the
# commit that writes it, and a gate at the release would find it a cycle later with the
# change already merged.
#
# The gap being one release is a premise, not a law: the pin comes from the CLI's own
# version, so on the release commit — where the number has moved and nothing is published
# under it — the pin names a version crates.io does not have, and cargo says so at
# resolution, about a crate that does not exist, instead of anything about the scaffold. So
# the recipe asks the registry which case it is in rather than assuming, and when the answer
# is "not published yet" it says which pin went unanswered and defers. Nothing in the
# release chain waits on that deferral any more: `e2e-scaffold-packaged` above asks the
# same question of the tarballs, before the first `cargo publish` rather than in the middle
# of the four, and it needs no published crate to ask it.
e2e-scaffold-published:
    #!/usr/bin/env bash
    set -euo pipefail
    root="$(pwd)"
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    cargo run -q -p htl-cli --bin htl -- new "$dir/published" --host rust --lib
    (
      cd "$dir/published"
      # What makes this build the one a user gets: it pins the release, and nothing
      # redirects that pin at a checkout.
      grep -q '^htl = ' Cargo.toml
      ! grep -q 'patch.crates-io' Cargo.toml
    )
    # Read back out of the manifest rather than recomputed here, so what is looked up below
    # is the requirement the generated project will hand cargo, character for character.
    pin="$(sed -n 's/^htl = "\([^"]*\)"$/\1/p' "$dir/published/Cargo.toml")"
    test -n "$pin"
    # crates.io's sparse index at its documented layout: a three-character name lives under
    # 3/<first character>/<name>, one JSON object per line. `cargo info htl@<pin>` cannot
    # answer this from in here — it resolves the name against the workspace first and reports
    # the unpublished version as though it were a release.
    curl -sS --fail --max-time 60 https://index.crates.io/3/h/htl -o "$dir/index"
    # `htl_dep_version()` writes `0.<minor>` under 0.y.z and `<major>` above it, and for both
    # of those the versions cargo's caret accepts are exactly the ones beginning `<pin>.`, so
    # the prefix is the whole question. A yanked version answers no.
    if grep "\"vers\":\"$pin\." "$dir/index" | grep -qv '"yanked":true'; then
      (
        cd "$dir/published"
        cargo build --target-dir "${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold"
      )
    else
      # Deferred, and to be read as deferred: nothing was built and nothing was proved. The
      # scaffold is correct or not either way, and the run that finds out is the gate, which
      # asks the tarballs instead of the registry and so has an answer on this commit too.
      echo "e2e-scaffold-published: DEFERRED, nothing built."
      echo "  The scaffold pins htl = \"$pin\" and crates.io has no release matching it, which"
      echo "  is the release commit and no other: the version has moved and the publish has"
      echo "  not happened. Whether a project pinning \"$pin\" builds is answered on this commit"
      echo "  by 'just e2e-scaffold-packaged', which builds the same three projects against the"
      echo "  .crate files the publish would upload, and which the chain in docs/releasing.md"
      echo "  runs before the first 'cargo publish'."
    fi

# Every benchmark: the figures in the README come from these. Ten samples each; a few minutes.
bench:
    cargo bench -p htl-core --bench check
    cargo bench -p htl-core --bench fix
    cargo bench -p htl-cli --bench cached_check

# A release binary, for timing against a real project rather than a generated one.
release:
    cargo build --release -p htl-cli

# Check a real project twice with a release build, to see the cache work: just dogfood <path>
dogfood project:
    cargo build --release -p htl-cli
    ./target/release/htl check {{project}} --no-cache --explain-cache
    ./target/release/htl check {{project}} --explain-cache
    ./target/release/htl cache status {{project}}
