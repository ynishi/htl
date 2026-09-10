# htl — pointers for coding agents

- What htl is, the CLI, embedding, lints, tests, `htl.toml`, bundles, pitfalls,
  running a consumer against an unpublished htl: [README.md](README.md)
- Issue labels, branches, verification, commit format, pull requests:
  [CONTRIBUTING.md](CONTRIBUTING.md)
- Never work on `main`; one branch per issue (`<type>/<slug>`), a worktree under
  `.worktrees/` if `main` needs to stay checked out.
- Green is `cargo test --workspace` and `cargo clippy --workspace --all-targets`;
  changes to the checker, lints, runner or bundles are also run on a dogfood
  `.tl` project before they ship, and the report says what was run.
- What may go into anything public, and what a public artifact owes its reader:
  [PUBLIC_DEVELOPMENT.md](PUBLIC_DEVELOPMENT.md). It outranks CONTRIBUTING, and
  its check runs before you write an issue, a PR body or a commit message.
