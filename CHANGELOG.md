# Changelog

Noteworthy changes to Recon are recorded here. Add entries under Unreleased;
the validated release workflow moves them into a dated version section and creates
a fresh Unreleased section. Released versions are listed newest first.

## [Unreleased]

## [0.1.2] - 2026-10-30

- Show bulleted unpushed commit titles in an automatically sized panel below Files
  only when commits are pending, with newest commits last. Toggle it with `u` or the
  Files header button, shown only while commits are pending; refresh as the
  branch or its upstream changes. Cap its
  height at five titles by default, configurable with `unpushed_visible_commits`
  or `--unpushed-visible-commits`.
- Keep large change trees responsive by caching their structure and rendering only
  visible rows; update live rows without rescanning the entire model per batch.
- Reuse HEAD blob fingerprints and bounded text across refreshes, and suppress
  Git-ignored build events while preserving tracked files and prior observations.

## [0.1.1] - 2026-10-28

- Dissolve newly created and restored file labels into the tree with the reverse
  of the deletion animation, keeping Git status and tree guides fixed.

## [0.1.0] - 2026-10-28

- Add an end-user quick start, detailed user guide, and reproducible screenshots
  demonstrating Git status, successive edits, and fresh comparison baselines.
  Include the linked documentation and images in release archives.

- View live Git changes in a terminal file tree, including staged, unstaged, and
  untracked files in repositories and linked worktrees. Compare against HEAD or
  capture current contents as a fresh baseline without modifying the repository.
- Keep a bounded, in-memory history of observed edits, with collapsible hunks,
  live following, clear entry selection, and vertical and horizontal navigation.
  Summarize binary and oversized files without retaining their full contents.
- Search file paths, filter history by file, and open the focused file in your
  VISUAL/EDITOR with `e`, returning to Recon when the editor exits.
- Customize keyboard shortcuts, themes, animation, ignore patterns, and memory
  limits through a configuration file and command-line options.
- Follow filesystem changes with native watchers and periodic reconciliation,
  reporting gaps when intermediate edits may have been missed. Handle configured
  Git line endings, executable-bit settings, and plain-file symlink checkouts.
- Build native release archives for macOS Intel and Apple Silicon, Windows x64,
  and Linux x64 through GitHub Actions, with SHA-256 checksums, changelog-derived
  release notes, and matching `--version` output.
