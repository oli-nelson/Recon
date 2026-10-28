# Changelog

Noteworthy changes to Recon are recorded here. Add entries under Unreleased;
the validated release workflow moves them into a dated version section and creates
a fresh Unreleased section. Released versions are listed newest first.

## [Unreleased]

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
