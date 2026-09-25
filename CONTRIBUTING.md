# Contributing

Conventions for changes to this repository, for people and coding agents alike.
The disclosure and public-artifact policy in
[PUBLIC_DEVELOPMENT.md](PUBLIC_DEVELOPMENT.md) outranks this file.

htl is small — five published crates and two examples in one workspace — so the
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

Always use [`.github/ISSUE_TEMPLATE/issue.md`](.github/ISSUE_TEMPLATE/issue.md).
Its comments are the rules for each section; they are not repeated here.

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
htl new sample                       # --embed, --target cdylib --lib: whichever the change touches
cd sample && htl check . && htl test .
```

`target/debug/htl` is not that binary. If no `.tl` project is at hand, `htl new`
writes one in a second, so not having one is not a reason to skip this: "not
verified on a dogfood project" is not a report this repository accepts. What the
pull request says about it is the Acceptance section of
[`.github/pull_request_template.md`](.github/pull_request_template.md).

A project `htl new` writes pins the htl the binary was built with: a binary built
from a checkout pins that checkout (`--htl path:<other-checkout>` names another),
so a change that has to be built against, and not only run by, this working copy
is one `htl new` and one `cargo build`, with nothing patched. Only a binary built
from the crates.io tarball pins a version; the scaffold never writes for a release
it does not link.

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

A commit message is free-form. [`.github/commit_message_template.txt`](.github/commit_message_template.txt)
is a reference to copy, or to set as git's `commit.template`. The one part that is
not free is the subject's prefix, which decides the next release (below).

- Formatting and clippy fixes go in their own commits.
- Never commit `workspace/`, `.worktrees/`, `.claude/`, `*.hb` or local agent
  state. If a commit needs `git add -f`, stop: something is filed wrong.
- The version bump is not written by hand. A dispatch of the `Release-plz` workflow
  opens a release PR with the bump and the CHANGELOG, and merging it publishes
  (`release-plz.toml` says how). The subject decides the size of the next release:
  a commit whose subject starts with `feat:` makes it a minor, which on 0.x is the
  unit that changes what a scaffold or a release pin can know; anything else is a
  patch. Every commit of a pull request reaches `main` — see Pull requests — so it
  is the commits release-plz reads, not the title the pull request carried.
- `feat:` is also what a change to a published crate's public API asks for — a
  signature, a return type, a field on a public struct, whatever a consumer of
  `htl-core`, `htl-macros` or `htl` could have written against. On 0.x cargo treats
  the minor as the compatibility unit, so `0.6` resolves to the newest `0.6.z` and a
  patch that changed an API breaks every consumer of the line on their next
  `cargo update`. A minor is what tells them. A PR that changes an API and says
  nothing else new is still `feat:`; the prose explains what moved. Between
  releases such changes accumulate on `main` and one `feat:` among them is enough
  — but it has to touch a file of a crate. release-plz reads the commits since
  the last tag *per package*, by the files they change, and a `feat:` that only
  edits this file or the justfile is invisible to it (it answered "already up to
  date" to exactly that). The five crates share one version, so one crate is
  enough for all five. 0.6.3 shipped an API change (`Project::patch`, `Patched`)
  as a patch; 0.7.0 is the release that says so, and the commit that asked for
  it is a doc comment on the API that moved.

## Pull requests

One issue per pull request, against `main`. Before opening it, run `just pre-push`
on the final tree, plus the installed-binary run above when the change calls for
one.

A pull request lands as a merge commit; squash and rebase are not used. So every
commit on the branch reaches `main` — a commit is a step of the work, and several
of them are expected rather than something to tidy away first.

Always use [`.github/pull_request_template.md`](.github/pull_request_template.md)
for the body. Its comments are the rules for each section; they are not repeated
here. Longer bodies are easier to write as a file and pass with `--body-file`;
`workspace/` is gitignored and a fine place for one.

## Working with coding agents

What this repository tells an agent is `AGENTS.md` at the root. It is a page of
pointers into this file and the README, not a second copy of either.
