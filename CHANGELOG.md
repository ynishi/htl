# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0](https://github.com/ynishi/htl/compare/v0.8.0...v0.9.0) - 2026-09-25

### Added

- htl build says a missing require once, the linker's line in place of the checker's
- a project with .tl at its root and no src/ is refused and told to set [layout] source = "."
- every diagnostic htl prints goes through the one formatter, its path spelled as htl check spells it
- the sink carries a diagnostic as a value, and its text is made in one place
- include_tl! and include_bundle! judge warnings and lints as htl check does, with one default
- a module declared only by a .d.tl is the environment's by the model's answer, not the linker's inference
- a host describes the directories it serves as a project model, and its checker and its resolver both come from that
- htl build, include_bundle!, htl unused and htl resolve take the host's names from the project model
- a file under a name the host provides is an error in the check and the run, not a lint
- htl check refuses a directory in no project, the library is handed the model, and a test holds include_tl! to htl check
- the project's directories come off package.path, and the model answers for every name it has
- a TealResolver names files by the model's rule, and a flat package's entry is spelled only in a directory of packages
- the cache asks the project model what each required name resolves to, instead of probing directories
- htl resolve reports the project model's answer — its files under the name — and says when two implement it
- a file answers to the name the project model gives it and no other
- coverage reports the project's own sources, not the dependencies the tests reach
- [imports] settles a name the project and a dependency share, without a rename
- a dependency does not see the project using it — a require that lands on the project's own files is an error at the call
- a module name two modules implement is an error in htl check, naming both owners
- the working directory is not searched — a checker set up from the model drops Lua's ./?.lua
- pkg::Project is pkg::MluaProject, and the model reads mlua-pkg only through it
- declarations htl brings into a project go to its declaration root, [layout] types
- which files a walk enters comes from the model — a module has a home, and a walk has a purpose
- a test is a file that loads the test library, and tests/ is the test root in [layout]
- every command and the macros set the checker up from the project model, and add_layout_paths is gone
- htl check reads the search path off the project model, and a file's own directory is no longer on it
- htl unused, htl resolve and the bundle entry name a file by the project model
- a project says where its own files live — [layout] source and types, and a directory cannot be both the project's and somebody else's
- generated Lua is terminated once, at the generator, so all five consumers get the same string ([#305](https://github.com/ynishi/htl/pull/305))
- a dependency's error and a contract check's errors come from the checker's items, so no report takes a diagnostic's text apart
- htl's own findings are Diagnostic values from where they are made, and nothing on the way re-reads their text
- a check carries each diagnostic in its parts, and htl fix classifies an error by the class the checker gave it
- a check carries how many syntax errors the parser reported, and htl fix skips by that count
- the project model knows which names the host provides, and where each is declared
- the checker asks model::Resolver for every module name, and the Lua-side name tables go
- model::Resolver answers which file a module name is, asked from which file, from the model alone
- a project is a set of modules, each owning its roots, and a file's name comes from the module that holds it

### Fixed

- htl test names a file as htl check does, and a generate failure carries its path as the file
- htl build says the linker's own errors as values, not by picking them out of the text
- every path in a report is spelled one way, relative to the working directory, and display_path keeps a leading ..
- htl check, htl fix and htl unused walk only the files a module of the project holds
- a contract is declared in a module of the project, and a marker at the project root is not read
- which files a contract holds, and under what names, is contract::held_name, read by the lint, the resolver and htl unused
- htl build judges a bundle's closure as htl check does, and htl run / gen judge on errors by a named policy
- htl fix walks as htl fmt does, and leaves a patched dependency alone
- a host's check of a module it already had reads a file that module requires, dropped in since
- htl unused walks the project from its root whichever manifest says where it is
- htl unused starts from main.tl where [layout] source puts it, and module names come from the naming rule
- a project keeps its store at its root, wherever the command ran from
- which directories are a dependency's, and whose a file is, come from the project model
- every command finds its project the same way, and one with two roots is refused by all of them
- a require of a name two files implement says so, in the check, at run time and in a bundle
- a module that declares a global is walked in every env that requires it, so htl check gives one answer whatever the file order ([#302](https://github.com/ynishi/htl/pull/302))
- htl gen and htl run apply htl.toml, so a file htl check accepts can be emitted and run ([#301](https://github.com/ynishi/htl/pull/301))
- an entry link points at the copy the project uses — a target_dir dependency at <target_dir>/<entry>, a patch at its entry
- a directory already on the checker's search path keeps its place, so a TealResolver's root no longer overrides the order a host stated ([#303](https://github.com/ynishi/htl/pull/303))

### Other

- help text and scaffold output say what the code does
- code comments that had fallen behind the code they sit on
- the reference is the doc comments; the README is the front door
- README drops three claims the code does not make
- the last of the README's design prose finds its item, or goes
- the rules the README alone carried go to the item that decides them
- the code docs carry every rule the README only half-shared with them
- README drops the design prose the code docs already carry
- README says what the code does where it had fallen behind
- Merge pull request #348 from ynishi/fix/walk-outside-modules
- htl check, htl unused and htl dts read the project's contracts from its model
- Merge pull request #337 from ynishi/feat/macro-verdict
- whether a run fails is htl_core::verdict, over the run's findings and a policy
- a test that run time and a bundle resolve module names with the model, as the check does
- the project model holds its contracts, each with its declaring module, directories, held modules and publication
- Merge pull request #335 from ynishi/refactor/check-scope
- what a run is checked against is project::Scope, and what a file says beyond its check is project::file_findings
- what a project says about itself as a whole is project::project_findings, not the tail of project::check
- Merge pull request #332 from ynishi/fix/fix-syntax-error-skip
- Merge pull request #330 from ynishi/fix/host-refresh-on-require
- htl-core's docs and tests build without dts again
- the walkers' docs point at the project model, not at functions #319 removed

## [0.8.0](https://github.com/ynishi/htl/compare/v0.7.0...v0.8.0) - 2026-09-22

### Added

- the window target: htl new --target window writes a windowed host on htl-mq, the first target that requires an entry script, and the fourth scaffolded host in e2e ([#290](https://github.com/ynishi/htl/pull/290))
- htl-mq, a macroquad window for an htl program — the mq host module, the loop, and the declaration it ships ([#289](https://github.com/ynishi/htl/pull/289))
- htl new writes mise.toml, the command's pin beside the crate's, and README says how mise carries htl ([#287](https://github.com/ynishi/htl/pull/287))

## [0.7.0](https://github.com/ynishi/htl/compare/v0.6.3...v0.7.0) - 2026-09-21

### Added

- the minor that 0.6.3's API change asks for is requested from inside htl-core, where release-plz reads it ([#284](https://github.com/ynishi/htl/pull/284))

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
