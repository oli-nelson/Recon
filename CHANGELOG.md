# Changelog

Add concise, user-facing changes under Unreleased. After every release build and
test succeeds, GitHub Actions promotes these notes into a dated version section
and creates a fresh Unreleased section. Keep released sections unchanged.

## [Unreleased]

- Watch Git working trees and linked worktrees with a live file tree and bounded,
  in-memory edit history, including staged, unstaged, and untracked changes.
- Configure themes, animation, keyboard shortcuts, ignore patterns, and history
  limits through a configuration file or command-line options.
- Publish tested macOS Intel/Apple Silicon, Windows x64, and Linux x64 executable
  archives with checksums, changelog release notes, and matching `--version` output.
- Bundle local-only Git support without requiring a Homebrew OpenSSL installation.
- Avoid false edit hunks from Git-configured CRLF line endings, untrusted executable
  bits, and plain-file symlink checkouts on Windows and shared filesystems.
- Avoid duplicate filesystem watches and symlink traversal; keep periodic recovery
  responsive without spinning when native watchers cannot start. Preserve literal
  backslashes in Unix filenames.
- Keep the watching indicator and shortcut footer stable during brief background
  refreshes; show progress when work lasts longer than a quarter second.
- Mark the selected history entry and show which file Enter will expand or collapse,
  including when its header is scrolled offscreen. Navigate selection within the
  pane before scrolling, and keep the same entry selected after folding.
- Use Left/Right and h/l to scroll wide hunks horizontally when history has focus,
  without changing file-tree folders. Keep entry headers and selection markers fixed.
