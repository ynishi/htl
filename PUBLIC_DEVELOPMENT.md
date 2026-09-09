# Public development and information boundaries

htl is developed in public. This document says what a public artifact must
contain to be worth reading, and what must not cross into one. It outranks
[CONTRIBUTING.md](CONTRIBUTING.md), and it is the policy a coding agent applies
before writing an issue, a pull request, a commit message, or documentation.

## Default

Everything about htl is public by default: the design, the code, the plans, the
measurements, and the failures. That includes work that went nowhere and
decisions that were reversed, which are usually the parts worth reading.

What is private is what is not htl's to publish — the contents of a project that
uses htl and is not itself public, credentials, and non-public security reports.
Private does not mean important or commercial. It means somebody else has not
agreed to publication, or publishing would expose a credential or an unfixed
vulnerability.

## Two questions, deliberately separate

A public artifact is checked for two different things, and only one of them is an
incident:

- **Confidentiality** — does it disclose something that was not ours to disclose?
- **Completeness** — can a reader who has never seen the author's machine
  understand it and act on it?

An opaque reference is not a leak. It is a public artifact that fails the second
question, which is its own defect: it costs every reader something and gives them
nothing.

## Classification

### ALLOW

These belong in public issues, pull requests, commits, code comments and
documentation:

- the design, the alternatives, the decision, and what it cost;
- measurements, with what they were measured on and which build produced them;
- diagnostics and program output, quoted verbatim;
- `file:line` into this repository, and the quoted line;
- a description of a project that is not public, by its shape — "a roguelike of
  ~22,000 lines of Teal with a Rust host", "6 modules and ~850 lines, three
  `#[host_module]`s";
- an environment fact that is a property of the tool rather than of one machine
  (an environment variable htl itself reads, a Cargo profile, an MSRV).

### WARN

These are not disclosures and are not blocked. They are worth fixing because the
artifact does not stand on its own without the fix:

- an identifier only the author can resolve, offered in place of the description
  a reader needs: the name of a project that is not public, a directory layout
  particular to one checkout, an environment variable belonging to one project,
  a symbol prefix belonging to one codebase;
- evidence that exists only on the author's machine — "measured on my project" —
  without the shape that says whether the finding applies to the reader;
- a rationale that lives only in a private note, leaving the public artifact
  with the conclusion and none of the argument.

The fix is to add what the reader needs. When the opaque token carried nothing to
begin with — a project name in a sentence that already gives the size and the
host — the fix is to drop it, because the description beside it was always the
part doing the work. Where a name is load-bearing, such as the argument of
`#[c_export(prefix = ..)]`, use a stand-in and say that it is one.

Do not build a review process to guarantee that no WARN is ever published. Both
people and agents will write incomplete context sometimes; finding and fixing it
is ordinary documentation work.

### BLOCK

These must not be copied into a public artifact:

- the contents of a project that is not public — its source, its data, its saved
  state, its screenshots — unless each part has been cleared for publication on
  its own;
- credentials, tokens, signed URLs, and URLs carrying access-bearing parameters;
- material covered by somebody else's confidentiality;
- non-public vulnerability details, before there is a fix to disclose;
- absolute paths on a machine (`/Users/…`, `/home/…`, `C:\Users\…`). They name a
  person's account and resolve nowhere else.

Found before publication, remove it and carry on; nothing else is owed. Found
after, see [After a disclosure](#after-a-disclosure).

## What each artifact is for

- **Issue** — the problem, the evidence, the alternatives, and the acceptance
  criteria. Exploration belongs here, next to the change it argues for.
- **Pull request and commit** — what changed, why this and not the alternative,
  and what was actually run to verify it.
- **Code documentation** — the current contract: what a type is, what a function
  promises, and why it is built that way.
- **README** — the user-facing reference.

A reader with this repository and nothing else should be able to follow all four.

## Writing up evidence from a project that is not public

Most findings here come from using htl on something. That something is usually
not published, and the report is still expected to stand on its own:

- **Give the shape**: how large (files, lines), what it does in a phrase, how it
  is hosted, and which version of htl produced the result.
- **Quote the output verbatim**, minus machine paths.
- **Name the htl-side location** — `crates/…/file.rs:120`, a lint rule, a CLI
  flag — since that is what the reader can go and look at.
- **Do not name the project.** It cannot be cloned, searched or looked up, so it
  answers no question a reader has.

## Agent publication check

Before writing anything destined for the remote, in this order:

1. **BLOCK** — does the draft contain any of the above?
2. **Completeness** — can a reader who has never seen this machine understand the
   problem and act on it? If the answer rests on a name only the author knows,
   replace the name with what it stands for.
3. **Machine paths** — none, anywhere, including in quoted output.
4. **Redistribution** — does the change commit a file that originated elsewhere?
   That is a licensing question rather than a disclosure one; see below.

This check is deliberately narrow. It is not a search for every string that looks
private.

## Third-party material

Committing a file redistributes it as part of this repository, and the terms it
arrived under have to permit that. The vendored Teal compiler is the pattern to
follow: `crates/htl-core/vendor/tl.lua` sits beside `crates/htl-core/vendor/LICENSE.teal`,
so what it is and what it is distributed under are answered in the same
directory.

Before adding a file that did not originate here, state where it came from, under
what terms, and which notices must travel with it — beside the file, or in the
script that produces it. If that cannot be stated, do not commit it: generate an
equivalent, or fetch it at build time and leave it untracked.

## After a disclosure

Once protected content is published, it is disclosed. Deleting the commit,
force-pushing over it, or making the repository private does not undo that, and
treating any of them as the fix is the mistake this section exists to prevent.

If the exposed thing can be **invalidated** — a credential, a token, a signed URL
— rotate it first, before touching history. A published secret is compromised the
moment it is published, and rotation is the only step that contains anything.
Rewriting history afterwards is a tidiness decision: it does not reach other
people's clones, a fork keeps the commit until its owner removes it, and an old
clone can push it back.

If it **cannot** be invalidated — somebody's data, another party's confidential
material, an unfixed vulnerability — the work moves from the file to the people
affected: what was exposed, over what window, and how far it could have travelled.

htl publishes crates, so one more thing is worth knowing before assuming a
mistake can be withdrawn: `cargo yank` is not deletion. The version stays
downloadable and existing lockfiles keep resolving to it. crates.io will delete a
crate only under narrow conditions, and even a deletion does not retract copies
already downloaded or mirrored.

Write down the exposure window, the reach, and every takedown attempted. Where
that record is public it is an issue, and it closes when everything it describes
is done.
