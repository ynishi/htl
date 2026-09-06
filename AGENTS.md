# htl — pointers for coding agents

- What htl is, the CLI, embedding, lints, tests, `htl.toml`, bundles, pitfalls:
  [README.md](README.md)
- Issue labels, branches, verification, commit format, pull requests:
  [CONTRIBUTING.md](CONTRIBUTING.md)
- Releasing, the crates.io version limit, running a consumer against an
  unpublished htl: [docs/releasing.md](docs/releasing.md)
- Never work on `main`; one branch per issue (`<type>/<slug>`), a worktree under
  `.worktrees/` if `main` needs to stay checked out.
- Green is `cargo test --workspace` and `cargo clippy --workspace --all-targets`;
  changes to the checker, lints, runner or bundles are also run on a dogfood
  `.tl` project before they ship, and the report says what was run.
- Internal paths and identifiers stay out of public issues, PRs and commits.
