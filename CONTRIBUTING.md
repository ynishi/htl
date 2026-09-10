# Contributing

Conventions for changes to this repository, for people and coding agents alike.
The disclosure and public-artifact policy in
[PUBLIC_DEVELOPMENT.md](PUBLIC_DEVELOPMENT.md) outranks this file.

htl is small — four published crates and two examples in one workspace — so the
rules are few; the ones here exist because skipping them has cost something at
least once.

## Issues

Work starts from a GitHub issue. Open one for anything that is more than a typo:
a bug report from dogfooding, a feature, a refactor, a doc gap. An issue records
the problem and the evidence; the pull request records what was done about it.

### Labels

Assign at least one when you open an issue. The branch prefix follows the label.

| Label           | The change                                     | Branch      |
| --------------- | ---------------------------------------------- | ----------- |
| `bug`           | behaviour contradicts what it promises         | `fix/`      |
| `enhancement`   | behaviour that does not exist yet              | `feat/`     |
| `refactor`      | behaviour unchanged                            | `refactor/` |
| `chore`         | production code untouched — CI, tests, tooling | `chore/`    |
| `documentation` | prose only (README, doc comments)              | `docs/`     |

### What an issue says

- **Problem**: what happens, on what input. For dogfood reports: the project
  size, the command, the output, verbatim.
- **Evidence**: measured, not assumed. A timing says which build (debug or
  release) and what it was measured on. A "type error" quotes the line.
- **Proposal**: optional. The 4-axis habit from the design discussions applies —
  a patch, an architecture change, a narrower requirement, or "not now" are all
  legitimate answers, and the issue should say which one it is asking for.
- **Acceptance**: what `htl check` / `htl test` / the CLI prints when it is done.

What may appear in a public artifact, and what a public artifact owes its
reader, are [PUBLIC_DEVELOPMENT.md](PUBLIC_DEVELOPMENT.md)'s subject rather than
this file's. The short version: describe a project the evidence came from by its
shape and never by its name, keep machine paths out of everything, and write so
that somebody who has never seen this machine can act on what you wrote.

## Branches

Never work on `main`. One branch per issue, named `<type>/<slug>` with the
prefix from the label table above. How you keep `main` available while you work
on it — a second clone, a worktree under the gitignored `.worktrees/`, stashing
— is your own business, and so is cleaning it up afterwards.

## Verification

The definition of green is the whole workspace, and it is cheap enough to run
every time. The recipes are named for the moment they guard:

```bash
just pre-commit   # cargo fmt --all, then the workspace's tests and clippy
just pre-push     # the above, plus a full compile and every end-to-end case
```

**Green is a precondition, not a verification.** A change to the checker, the
lints, the test runner, the scaffold or the bundle format is run against a real
project before the pull request is opened — and through the binary a user
installs, not the one the worktree happens to have built:

```bash
cargo install --path crates/htl-cli
htl new sample                       # --embed, --host ffi --lib: whichever the change touches
cd sample && htl check . && htl test .
```

`target/debug/htl` is not that binary. If no `.tl` project is at hand, `htl new`
writes one in a second, so not having one is not a reason to skip this: "not
verified on a dogfood project" is not a report this repository accepts. Say what
was run — the project, the command, the arguments — and what came out.

`HTL_PROFILE=1` prints per-phase timings; a performance change quotes them,
before and after, and says which build produced them.

## Documentation

Documentation lives in the code, godoc / rustdoc style, and it is written
thick rather than thin: the design is in there, not only the signatures.

- The crate-level (`//!` in `lib.rs`) and module-level docs are the front page:
  what the crate is for, how its parts fit together, and why it is built that
  way — the place a design note goes, the way a Go package comment "provides
  information relevant to the package as a whole and sets expectations" and a
  Rust crate root "summarizes the role of the crate and explains why you would
  want to use it".
- Each type and function documents what it promises; how it does it today is a
  comment in the body, where it changes with the code.
- The same for `.tl`: the module record is the public API, `---` comments above
  it and its functions are the doc.

`cargo doc` is where a reader is sent. When a comment and an issue or a chat
disagree, the code wins, then the comment.

Beyond doc comments there is one place, and nothing else: README.md, the
user-facing reference (CLI table, embedding, lints, tests, `htl.toml`, bundles,
pitfalls). A new flag, lint or config key is not done until it is in there.

Write a rule once, where the thing it constrains is defined, and link to it
from anywhere else; a second copy is the one nobody updates.

## Commits

```text
<subject: what changed, one line>

<prose: the problem, why this fix and not the alternative, what it cost>

Verified: <what was run, and the outcome>

Refs #<issue>
```

- Formatting and clippy fixes go in their own commits.
- Never commit `workspace/`, `.worktrees/`, `.claude/`, `*.hb` or local agent
  state. If a commit needs `git add -f`, stop: something is filed wrong.
- The version bump (`[workspace.package] version` and the three internal
  dependency versions in `Cargo.toml`) rides with the last change of a release,
  not in a commit of its own; that commit's subject ends with the version,
  `(0.1.N)`, which is how the history reads as a release log.

## Pull requests

One issue per pull request, against `main`. Before opening it, run `just pre-push`
on the final tree, plus the installed-binary run above when the change calls for
one.

The body records what changed, what was verified (the commands and their
outcome, and on which project), and what it deliberately does not cover, and
ends with `Refs #<issue>`. Longer bodies are easier to write as a file and pass
with `--body-file`; `workspace/` is gitignored and a fine place for one.

## Working with coding agents

What this repository tells an agent is `AGENTS.md` at the root. It is a page of
pointers into this file and the README, not a second copy of either.
