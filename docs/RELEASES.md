# Releases

Maintain user-facing notes under `Unreleased` in `CHANGELOG.md`. The manually
requested release workflow tests and builds one exact source commit, then promotes
the changelog, updates Cargo versions, tags, and publishes only after every build
succeeds. The local helper only dispatches; it never edits, commits, tags, or pushes.

## Request a release

1. Review and consolidate `Unreleased`. Keep released sections unchanged.
2. Commit and push the intended release branch. Leave the branch at that commit
   until publication completes; concurrent changes make finalization fail safely.
3. With GitHub CLI installed and authenticated, run:

   ```sh
   bash scripts/release.sh 0.1.0
   # Prerelease example:
   bash scripts/release.sh 0.2.0-rc.1
   ```

   The helper requires a clean checkout matching the branch on `origin`, handles
   SSH host aliases, and supplies an expected-source-SHA guard. Alternatively use
   **Actions → Release → Run workflow**, select a branch, and enter the version.
4. Follow the workflow. Pull the release commit after it succeeds.

Versions accept an optional `v` prefix and use `X.Y.Z` or
`X.Y.Z-{alpha,beta,rc}.N`, without leading zeroes; core components are 0–9999.
Prereleases are marked as such and never become GitHub's latest stable release.

## Builds and artifacts

| Platform | Architecture | Runner | Target |
| --- | --- | --- | --- |
| macOS | Intel x64 | macos-15-intel | x86_64-apple-darwin |
| macOS | Apple Silicon | macos-15 | aarch64-apple-darwin |
| Windows | x64 | windows-2025 | x86_64-pc-windows-msvc |
| Linux | x64 | ubuntu-22.04 | x86_64-unknown-linux-gnu |

Each native job applies the requested version to `Cargo.toml` and Recon's entry
in `Cargo.lock`, then runs formatting, tests, strict Clippy, and a locked release
build with Rust 1.88.0. It executes the actual release binary and requires
`recon --version` to equal `recon <version>` before packaging. Clap reads its
version from Cargo at compile time; no separate runtime version is maintained.

Downloads are `recon-<version>-<platform>.tar.gz` for `macos-x64`, `macos-arm64`,
`windows-x64`, and `linux-x64`, plus `SHA256SUMS`. Each archive contains `recon`
(Windows: `recon.exe`), `README.md`, the documented default configuration, changelog, and `docs/`
(including the user guide and screenshots).
Extract the matching archive, place the executable on your PATH, and run
`recon --version`. Windows users can extract `.tar.gz` with `tar` or an archive tool.

Binaries are unsigned and macOS binaries are not notarized. Use a compatible
terminal and install Git for the repositories you inspect. Linux binaries target
glibc-based distributions compatible with Ubuntu 22.04 or newer; they are not
static musl builds. Libgit2 is bundled with networking disabled, so Homebrew
OpenSSL and libssh2 are not required. Windows may need the Microsoft Visual C++ x64
runtime. Native terminal interaction and installation should be checked manually
on each platform; local macOS tests do not establish Windows or Linux support.

## Publication and recovery

The workflow prepares changelog/release notes without changing the repository.
Only after all native jobs succeed does it commit the promoted changelog and
Cargo versions, create annotated `v<version>`, and atomically push the branch and
tag with an exact source-SHA lease. The tagged source has the same Cargo version
as the compiled artifacts, so rebuilding a release tag reports the right version.
An initial release may already have the requested Cargo version; unchanged
manifests need no diff. It creates a draft GitHub Release, uploads all archives and
SHA-256 checksums, then publishes with changelog-derived notes.

- Enable Actions and access to the listed hosted runners. The publish job uses
  `GITHUB_TOKEN` with `contents: write`; branch and tag rules must permit the
  release commit and tag. No protection bypass or personal token is configured.
- Release runs are serialized. If tests/builds fail, remote refs and changelog
  remain unchanged. If the branch advances, request a new run from the new commit.
- If publication fails after the atomic push, use **Re-run failed jobs** in the
  same workflow run. Do not rerun the successful validation job: it deliberately
  rejects existing tags. Finalization verifies run ownership, source parent,
  changelog, version files, and branch ancestry before resuming an owned draft.
- Published assets are never overwritten. Only a draft belonging to the same run
  may have uploads replaced. Expired artifacts or conflicting state require
  deliberate manual recovery; scripts never delete remote tags or rewrite releases.

## Local checks

```sh
bash scripts/test-release.sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Release-script tests require Bash, Python 3.11+, Git, jq, tar, and shasum. They use
disposable local remotes, fake binaries, and a fake `gh` to exercise version and
changelog validation, packaging, dispatch guards, branch races, rejected pushes,
draft retry, prereleases, and immutable published releases. They do not run a
hosted workflow or build native packages.
