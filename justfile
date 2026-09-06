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
ci: build check e2e

# Compile everything, including tests and benches, without running any of it.
build:
    cargo build --workspace --all-targets

# The CLI and the embedding example, end to end, as the second CI job does.
e2e:
    cargo run -q -p htl-cli --bin htl -- check examples/tl/util.tl examples/tl/main.tl examples/tl/util_test.tl
    cargo run -q -p htl-cli --bin htl -- test examples/tl/util_test.tl
    cargo run -q -p embed
    cargo run -q -p embed -- --bundle

# The benchmarks behind the caching figures in the README. Ten samples each; a few minutes.
bench:
    cargo bench -p htl-core --bench check
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
