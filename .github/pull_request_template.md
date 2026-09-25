<!--
This file is the rule for a pull request's body, not only its shape. CONTRIBUTING.md
links here instead of restating it. `scripts/check-body` checks a body against the
sections below before it is posted, and CI checks it again on the pull request.

Copy this file, fill in every section, and delete the comments. Do not leave a
section empty: write "none" and say why.
-->

## What changed

<!--
What the change does, in terms a reader of the code can check: the types, functions
and files it touches, and what each now does.

Then why this approach, and not the alternative the issue or the design discussion
raised. Give the alternative and the reason it was not taken.
-->

## Acceptance

<!--
One row per acceptance criterion of the issue named in `Refs #N` below. Quote each
criterion from the issue, word for word, and number it as the issue does. A
criterion this PR does not meet goes under "Not covered", not here.

- "Expected" is the correct output. Write it BEFORE running anything, from the
  criterion and the README / doc comments. Writing it after looking at the output
  approves whatever came out.
- "Actual" is what the installed binary (`cargo install --path crates/htl-cli`)
  printed: the lines themselves, verbatim, minus machine paths
  (see PUBLIC_DEVELOPMENT.md). Not "the output was correct".
- "Fails before" applies to a fix. Say whether the same check fails on the build
  before this change, and what it printed there. A check that also passes before
  the change did not look at the defect.

These are not evidence and do not go in this table:
- "the output is identical to / unchanged from the previous build". That shows
  nothing changed, not that anything is right; wrong output stays wrong.
- where or how often something was run, or which script ran it;
- "tests pass" on its own. Say which test checks which criterion.

For a refactor, the criterion is "behaviour unchanged". There, before/after
equality is the evidence; say what was compared.
-->

| # | Criterion (quoted from the issue) | Expected | Actual | Fails before |
|---|---|---|---|---|
| 1 |  |  |  |  |

## Checks

<!--
One line per item: what was done, and what came of it. The gate's name and its
outcome are enough ("`just pre-push`: passed, 856 tests, 0 failed"); the command
lines themselves are not needed. An item that does not apply says "n/a" and why.
A line with no result is not a check: "done" or a ticked box says nothing.

These are the work the change needed. They are not the evidence that it is right;
that is the Acceptance table above.
-->

- Tests added or changed:
- `just pre-commit` (fmt, tests, clippy):
- `just pre-push` (full build, end-to-end cases):
- End-to-end cases added or changed:
- Run through the installed binary on a project (CONTRIBUTING.md, Verification):
- Docs updated (README, doc comments):

## Unintended changes

<!--
Optional. What else the change altered in the output or the API, found by
comparing with the previous build, and why each is intended. This section is where
a before/after comparison goes. It is never evidence for a row above.
-->

## Not covered

<!--
What this PR deliberately leaves out: criteria of the issue it does not meet, and
problems found on the way that belong elsewhere. Name the issue or PR that takes
each, or say that none does yet.
-->

<!--
The last line is `Refs #N`. Never write close / closes / fix / fixes / resolve /
resolves before `#N` anywhere in this body: GitHub reads them and closes the issue
on merge. `land.sh` closes the issue when the whole of it has landed.
-->
Refs #
