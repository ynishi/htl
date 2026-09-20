# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.3](https://github.com/ynishi/htl/compare/v0.6.2...v0.6.3) - 2026-09-20

### Other

- `htl new` refuses a project name Rust or Teal cannot carry, before it writes anything ([#281](https://github.com/ynishi/htl/pull/281))
- A patched dependency is on the search path in its own right, so the copy in a tarball resolves without a link ([#279](https://github.com/ynishi/htl/pull/279))
- A check says how many of its files are a patched dependency's, and when Cargo.toml asks for an htl this command is not ([#277](https://github.com/ynishi/htl/pull/277))
- The scaffold's [check] example stops at a relative dir, and an unknown matcher names the matchers ([#276](https://github.com/ynishi/htl/pull/276))
- Nothing is written in the tree cargo verifies, and a patched dependency is not a project of its own ([#275](https://github.com/ynishi/htl/pull/275))
- The patch takes the package, not the repository around it: every dot-entry at the copy's root is dropped, and the report names them ([#274](https://github.com/ynishi/htl/pull/274))
- A `where` field on a record body's first line is explained, and its follow-on error dropped ([#273](https://github.com/ynishi/htl/pull/273))
- The unpatched scaffold gate becomes post-publish: the published CLI, the published crate, run by hand after a release; e2e patches nothing ([#272](https://github.com/ynishi/htl/pull/272))
- `std.argparse` takes a hyphen-leading value for an option that requires one: mlua-batteries 0.7.3 ([#278](https://github.com/ynishi/htl/pull/278))

## [0.6.2](https://github.com/ynishi/htl/compare/v0.6.1...v0.6.2) - 2026-09-20

### Other

- README says the four ways to install the CLI, not only cargo ([#262](https://github.com/ynishi/htl/pull/262))

## [0.6.1](https://github.com/ynishi/htl/compare/v0.6.0...v0.6.1) - 2026-09-20

### Other

- A scaffold pins the htl its CLI was built with, where it is: no window, no default to raise, no knows_* to ask ([#257](https://github.com/ynishi/htl/pull/257))
- 0.4 leaves the scaffold's window, and the three questions every remaining pin answers yes to are gone with it ([#254](https://github.com/ynishi/htl/pull/254))
- The scaffold's default is 0.6: a project written today pins the release whose linker serves the bundle host the scaffold writes ([#252](https://github.com/ynishi/htl/pull/252))
- The tag builds the binaries: cargo-dist makes the GitHub Release, five archives, two installers and a Homebrew formula from the v<version> tag ([#256](https://github.com/ynishi/htl/pull/256))
