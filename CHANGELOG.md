# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.2](https://github.com/ynishi/htl/compare/v0.6.1...v0.6.2) - 2026-09-20

### Other

- README says the four ways to install the CLI, not only cargo ([#262](https://github.com/ynishi/htl/pull/262))

## [0.6.1](https://github.com/ynishi/htl/compare/v0.6.0...v0.6.1) - 2026-09-20

### Other

- A scaffold pins the htl its CLI was built with, where it is: no window, no default to raise, no knows_* to ask ([#257](https://github.com/ynishi/htl/pull/257))
- 0.4 leaves the scaffold's window, and the three questions every remaining pin answers yes to are gone with it ([#254](https://github.com/ynishi/htl/pull/254))
- The scaffold's default is 0.6: a project written today pins the release whose linker serves the bundle host the scaffold writes ([#252](https://github.com/ynishi/htl/pull/252))
- The tag builds the binaries: cargo-dist makes the GitHub Release, five archives, two installers and a Homebrew formula from the v<version> tag ([#256](https://github.com/ynishi/htl/pull/256))
