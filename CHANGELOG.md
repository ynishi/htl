# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.16.0](https://github.com/ynishi/htl/compare/v0.15.0...v0.16.0) - 2026-10-21

### Added

- htl shapes writes a run-time shape and a typed Schema<T> declaration from each record, so a value from outside is narrowed once at the boundary instead of cast at every read
- htl lsp traces call hierarchies, the callers and the callees of a function across the project, from the uses the references index resolves
- htl lsp answers textDocument/references from the uses each file of the project resolves, kept current as the symbol index is, so an agent finds every caller rather than every word
- htl lsp answers the workspace-symbol request from an index of the project's declarations, kept current by checks, watched files and a stat of each file, so an agent finds a declaration by name
- htl lsp answers textDocument/documentSymbol with the declaration tree of an open file, so an agent lists a module's API without searching text
- htl lsp re-checks the open files when htl.toml, the package manifests, a declaration under types/ or the Rust htl dts reads changes, and the README says how an editor starts it
- htl lsp answers hover with the type tl checked and go-to-definition with the declaration, and a require's module with the file htl resolves it to
- htl lsp formats a buffer as htl fmt does, and offers htl fix's fixes as quick fixes and a fix-all action, only for the text the check read
- htl lsp, a language server on stdio whose diagnostics are htl check's, on open and on save, for the open files and the ones that require a saved one
- the bin scaffold names tokio's time beside rt and signal, the timer the deadline of a question the host asks Teal waits on
- TealRecord reads serde's skip, default, skip_serializing_if, transparent and flatten, in the declaration and the conversion from one reading, and htl dts writes the same declaration
- a Teal loop refuses a question with req:reject(reason), and the host reads it as AskError::Rejected, on mlua-isle 0.10
- the type index carries each declaration's type as a structure, so a tool that lowers a record reads tl's type object and not its printed form
- a declaration in the type index says which code is its own function body, so a call is attributed to the function it is written in
- TypeIndex::uses lists every name a checked file uses with the declaration it resolves to and whether it is a call, the reverse go-to-definition is built from
- Htl::declarations_of collects the declarations of a file it does not check, from a parse alone, as the type index's tree without the types
- the type index keeps the declarations a file makes as a tree, with their kind, range, name and type, and typeindex::Decl reads them back
- Htl::check_indexed keeps, beside a check, the type and the declaration tl's report gives each position, and CheckInfo::type_at reads them back
- htl::fix names what htl fix decides about one fix and applies one fix's edits as htl fix does, and Htl::format_text formats a text as htl fmt formats its file
- project::check_file is the per-file check htl check's walk asks, with the checker's answer from its store or from disk
- TealFields::write_fields takes the path read_fields takes, so a serde-crossed field inside a flattened struct or a tag payload fails to write under the name it fails to read under
- every serde word TealRecord refuses sits in one table with its escape, #[teal(serde)] crosses a field through serde under a serde feature, #[teal(skip)] leaves one out, and a derive refusal is one error again
- a data enum declares and crosses the way serde tags it — tag, tag + content, or untagged — and one serde would write externally tagged says so instead of disagreeing with the wire
- TealRecord reads serde's names — rename_all, rename, alias, rename_all_fields — so a field's Teal name and Lua key are the ones the JSON carries, and a serde word it does not read stops the build instead of being ignored
- htl re-exports serde and serde_json, and a TealRecord field may say the Teal type it crosses as with #[teal(as = "..")]
- task.answer(req, f) answers a question with what f returns and rejects it with the error f raised, so a bug in the decision reaches the host as a reason rather than a timeout
- htl::task::Ask asks the Teal loop a typed question with a deadline, and names the three ways it fails, so a host writes the wait and the safe default once rather than per kind

### Other

- every command assembles its checker through checker_for, which goes through the library's project::checker over project::Scope::new, so run, build, gen, shapes, resolve and fix resolve a require the way check does
- the embed example's Call is a serde type crossing as serde names it, the README says a serde type is written once, and the dts doc's serde material reads as one section
- examples/embed --ask shows a host asking Teal through Ask, with the kind's deadline and the deny default in one place, and the README says how a host asks
- the type index splits its rows with as_chunks, which the stable toolchain's clippy asks for in place of chunks_exact with a constant size
- the TealRecord derive doc lists the #[teal(..)] words and links to htl::dts for what each serde word does, instead of stating the rules a second time
- an end-to-end case adds one question to a scaffolded bin host and runs it, so the host htl new writes is shown to carry Ask without a change to its template
- std.proc's timed tests bound a prompt return at 3 s, the middle of the gap to the 5 s child they rule out, not the 1 s a loaded machine crosses

## [0.15.0](https://github.com/ynishi/htl/compare/v0.14.0...v0.15.0) - 2026-10-11

### Added

- a scaffolded bin host runs its program on the executor, as htl run does
- a scaffolded host depends on htl-std and installs it, and htl pin moves its line with htl's
- htl run, test, check, resolve and build carry std.fs and std.proc, with no host
- htl-std, htl's asynchronous I/O for Teal, starting with std.fs
- a #[host_module] name may be dotted; the require key is the whole name and the record is its last segment
- a binary registers the libraries it adds to every state htl-core builds
- std.proc.run, a child process run to completion as one await

### Other

- the bin scaffold test derives the htl-std line from a release pin as the scaffold writes it
- the scaffold's unit tests expect the executor host and the async feature
- std.* is htl's re-export of Rust libraries, and the I/O slice is htl-std's

## [0.14.0](https://github.com/ynishi/htl/compare/v0.13.0...v0.14.0) - 2026-10-09

### Added

- htl pin <release | main | path:<checkout>> moves the htl a scaffolded project depends on, and --htl release names the release this CLI is
- htl check --statistics prints how many findings each rule produced, instead of the findings, and the JSON check document carries the same rows
- unmarked-sealed reports a record built and cast only in its own file and carrying no ---@sealed, and htl adopt's ---@sealed row counts those records as applicable
- unmarked-struct reports a record built whole at every construction site and carrying no ---@struct, and htl adopt's ---@struct row counts those records as applicable
- htl adopt reports how many declarations carry each of htl's nine markers, and where; the checker's census rides in the cache so an unchanged project replays it
- include_tl! and include_bundle! rewrite the crate's host_module .d.tl declarations before checking, so the order of mod declarations no longer matters
- records = [geom::Point] nests a record declared in another file of the crate, read through its mod declaration
- a uses entry names a module path, `uses = [Mode, task = "htl.task"]`, so a .d.tl imports another library's types under the name it uses them by
- type-guard, a lint whose fix rewrites type(v) == "<tag>" on an any or union variable to v is T, the spelling Teal narrows under, and drops the casts the guard made redundant
- a #[host_module] method takes &Lua, filled from the closure's own handle and left out of the .d.tl

### Fixed

- await on a literal or a unary expression is refused as "applies to a call", whatever the literal's kind
- htl fmt keeps an await or async keyword written on a line of its own inside its block, and indents the line that completes it
- a statement-level await <call> that opens a block keeps its indentation under htl fmt
- await-missing reads the async keyword at the callee's declaration, not anywhere on its line
- an await inside an if checks: the node walk descends into if_blocks, the plain array Teal keeps an if's branches in
- struct-fields holds a literal only to the fields the record's own body declares, and a nested record's same-named field no longer takes the outer field's ---@optional
- the resolver tests build their scratch directory canonical, so a requirer they never write places under the same roots on macOS

### Other

- Merge remote-tracking branch 'origin/main' into feat/unmarked-sealed
- every test temp directory is a TempDir that removes itself when the test passes, and the helpers are named tempdir for what they make
- the checker prelude and the runtime state load one chunk for what both need, instead of each carrying a copy
- Htl::from_lua names with_checker_lua as the route for a state that only runs, and the crate doc shows it on mlua-isle's AsyncIsle
- Htl::new and with_checker_lua say why the state is unsafe_new (debug), not that a safe state refuses bundles
- the channel test orders its sender after events() by a handshake, not a 50 ms sleep
- a_child_left_sleeping_is_dropped_before_the_run_returns checks the drop, not the clock

## [0.13.0](https://github.com/ynishi/htl/compare/v0.12.0...v0.13.0) - 2026-10-07

### Added

- htl.task forwards channel, after, ticker, select and select_raw, typed in task.d.tl, and a task's case taken by select is its one await
- htl::task::RecvChannel / SendChannel / Request, which a #[host_module] declares as htl.task's types in a .d.tl that imports it
- htl-core builds on mlua 0.12.2 and mlua-isle 0.9, whose task library has channels, timers and select

### Other

- the README's Tasks section and the htl crate doc name channels, timers, select and the typed host channels

## [0.12.0](https://github.com/ynishi/htl/compare/v0.11.0...v0.12.0) - 2026-09-30

### Added

- htl test --allow-empty passes a run that found no test file, and --format json prints its document whether or not the flag is given
- a cache entry hashes and probes the module's require closure, so an edit two requires away is not replayed over
- a comment-only line of a .tl survives generation at its own line, so the --- doc reaches the Lua htl gen and include_tl! write

### Fixed

- the stand-in a Teal require of a declared-only name gets stays out of package.loaded, so a plain Lua pcall(require) after it is still false
- a -- line inside a block comment stays empty under generation, and the comment lines after a .tl's last line of code reach the Lua at their own numbers

### Other

- the htl-cli tests patch a scaffolded project the way e2e does, so the window test checks under the packaged gate when the pinned release is not on crates.io yet

## [0.11.0](https://github.com/ynishi/htl/compare/v0.10.0...v0.11.0) - 2026-09-29

### Added

- a check with findings names htl check --explain <rule> once per rule before its summary, and the design prose above four rules points at the registry instead of restating it
- htl check --explain <rule> prints why a rule exists and how to decide its cases, from an explanation every rule of the registry carries

### Fixed

- htl test's store never records or preloads a .d.tl as a module, so a Lua module typed by a declaration beside it survives the second run

## [0.10.0](https://github.com/ynishi/htl/compare/v0.9.0...v0.10.0) - 2026-09-28

### Added

- a require the generator erases is marked in the census and left out of the link, so a bundle that imports a record of types asks the host for nothing
- htl-mq declares its game table as record Game with ---@noyield fields, #[teal(noyield)] on a TealRecord field writes the marker, and the window scaffold binds the table to mq.Game
- #[host_module] and htl dts write ---@noyield for a sync fn's Function parameters, and #[teal(yields)] / #[teal(noyield)] on a parameter override it
- async-as-sync-callback reads ---@noyield on a host's declaration, the parameters and record fields a host calls from C, so a host's own boundary is reported like table.sort's
- async-as-sync-callback, the fifth async rule: an async function handed to table.sort, string.gsub, xpcall's handler or a __tostring, where a C call cannot yield
- the four async lints (await-missing, await-outside-async, await-non-async, task-escape), and ---@async on a host async fn's declaration
- async / await syntax under [lang] async: local async function, await on a call, async local as a child task, on Teal's own lines
- htl.task, the task library a program on the executor spawns and awaits with, preloaded and declared like htl.test
- htl run and htl test run the program as a root on mlua-isle's executor, so a host's async fn is callable from Teal and Ctrl-C cancels with a grace
- one owner for a state's debug hook, so Interrupt, coverage and a host callback register instead of replacing each other
- global-redeclaration reports one global name declared at two sites, and the crate doc says the three ways a host value gets a type
- a declaration is never served, so the chain reaches the module behind it, and nothing is built in the checker's state for a program state to receive

### Fixed

- bundle_info counts the bytes the file has, not 72, so the test does not fail with the length of the version string
- a global travels the require chain, so a module served from the store hands its requirer what its own requires would have declared
- a module that declares a global is walked once per run, and its globals are handed to each env that requires it
- require-cycle reads the graph that runs, so a loop of type-only requires the generator erases is not reported

### Other

- the README's Async section in one place, and the embed example runs the issue's pair through a host's async fn

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
