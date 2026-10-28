# Agent Development Guide

## Development

- Recon is a Rust terminal application. Requires Rust 1.88 or later and Git.
- Validate changes with `cargo fmt --check`, `cargo clippy --all-targets --locked
  -- -D warnings`, and `cargo test --locked`.
- Validate release automation with `bash scripts/test-release.sh`. Its disposable
  repositories and fake GitHub CLI do not publish releases or contact GitHub.
- Preserve unfamiliar user changes and keep work within the requested scope.
- Report what was actually validated. Do not infer Windows or Linux validation
  from a macOS run.

## Changelog and releases

- When making changes, update the `Unreleased` section of [CHANGELOG.md](CHANGELOG.md)
  with concise, user-facing entries for noteworthy changes (features, fixes,
  breaking changes, or meaningful operational/documentation changes). Skip routine
  internal churn and avoid duplicate entries. Consolidate related changes into
  coherent entries rather than listing each commit. Revise or remove existing
  Unreleased entries when later changes supersede them, so the section describes
  the current behavior rather than obsolete intermediate implementations. Keep
  noteworthy breaking changes and migration guidance explicit.
  Keep previously released sections unchanged; only after all release builds and
  tests pass does GitHub Actions promote `Unreleased` into a dated version section
  and create a new empty `Unreleased` section. The local release helper only
  dispatches that workflow. See [docs/RELEASES.md](docs/RELEASES.md) for release operations.
- Release versions must match `Cargo.toml`, Recon's entry in `Cargo.lock`, the
  `v<version>` tag, archive names, and `recon --version`. The workflow applies the
  requested version before compilation and commits both manifests at publication.
