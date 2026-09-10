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

# Everything CI runs on a push, in the same order, so a failure there reproduces here.
ci: build check e2e e2e-scaffold e2e-scaffold-published

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
    # Scaffolding inside the repository would leave a Cargo package in the checkout and
    # run the CLI against the repository's own htl.toml, so the project goes elsewhere
    # entirely and only its build artefacts come back.
    dir="$(mktemp -d)"
    trap 'rm -rf "$dir"' EXIT
    # A scaffolded project depends on the released htl, which is what a user gets; for the
    # change under test to be the one that runs, point the three crates at this checkout.
    # Its artefacts land beside the workspace's under a directory of their own, because
    # the scaffold sets `[profile.dev.build-override]` and sharing one target directory
    # would rebuild the proc macro dependencies on every switch.
    sample=(--config "patch.crates-io.htl.path='$root/crates/htl'"
            --config "patch.crates-io.htl-core.path='$root/crates/htl-core'"
            --config "patch.crates-io.htl-macros.path='$root/crates/htl-macros'"
            --target-dir "${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold")
    # Each project is built in a subshell: `cargo run -p htl-cli` below has to be back in
    # this workspace, not in the one that was just scaffolded.
    cargo run -q -p htl-cli --bin htl -- new "$dir/hostsample" --host rust
    (
      cd "$dir/hostsample"
      cargo test "${sample[@]}"
      out="$(cargo run -q "${sample[@]}" -- Ada)"
      printf '%s\n' "$out"
      printf '%s\n' "$out" | grep -qx 'hello from Rust, Ada'
    )
    # --lib is the same host without a binary: the library still builds and its test still
    # goes through preload, and there is no entry point for cargo run to find.
    cargo run -q -p htl-cli --bin htl -- new "$dir/libsample" --host rust --lib
    (
      cd "$dir/libsample"
      test ! -e src/main.rs
      test ! -e src/main.tl
      cargo test "${sample[@]}"
    )
    # The C ABI host, whose callers are the part nothing else here compiles: the library
    # is built, the header the macro writes is checked, and the two reference hosts under
    # examples/ are run against the artefact — the Python one wherever python3 is, the C
    # one only where there is a compiler and a make.
    cargo run -q -p htl-cli --bin htl -- new "$dir/ffisample" --host ffi --lib
    target="${CARGO_TARGET_DIR:-$root/target}/e2e-scaffold"
    (
      cd "$dir/ffisample"
      test ! -e src/main.rs
      test ! -e src/main.tl
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
# The gap being one release is a premise, not a law: the pin comes from the CLI's own
# version, so on the release commit — where the number has moved and nothing is published
# under it — the pin names a version crates.io does not have, and cargo says so at
# resolution, about a crate that does not exist, instead of anything about the scaffold. So
# the recipe asks the registry which case it is in rather than assuming, and when the answer
# is "not published yet" it says which pin went unanswered and defers, because the release
# chain (docs/releasing.md § The chain) runs this between publishing `htl` and publishing
# `htl-cli` — where the same question has a published crate to be asked about.
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
      # scaffold is correct or not either way, and the run that finds out is the one the
      # chain makes, before the CLI that writes this pin is installable by anyone.
      echo "e2e-scaffold-published: DEFERRED, nothing built."
      echo "  The scaffold pins htl = \"$pin\" and crates.io has no release matching it, which"
      echo "  is the release commit and no other: the version has moved and the publish has"
      echo "  not happened. Whether a project pinning \"$pin\" builds is answered by the chain"
      echo "  in docs/releasing.md, which runs this recipe between 'cargo publish -p htl' and"
      echo "  'cargo publish -p htl-cli', and stops the chain there if the answer is no."
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
