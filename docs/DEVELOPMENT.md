# Development

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/benchmark.py
python3 scripts/memory_stress.py
python3 scripts/smoke_ui.py --large
python3 scripts/animation_smoke.py
python3 scripts/config_smoke.py
python3 scripts/editor_smoke.py             # Unix/macOS PTY with a fake editor
```

Tests create disposable repositories and cover baseline transitions, repeated
edits, undo, delete/recreate, staged cancellation, ignores, retention, binary
files, symlinks, linked worktrees, HEAD boundaries, missed-event reconciliation,
atomic saves, nested repositories, cancellation, and narrow terminal rendering. The benchmark creates 100,000
tracked files, 10,000 local changes, and a 2,000-file burst; fixture creation is
excluded from startup/latency measurements. See [performance notes](PERFORMANCE.md)
for measured results and remaining validation scope.

[Livediff](https://github.com/SoCkEt7/Livediff) informed the live terminal viewing
reference: native notifications, burst coalescing, and a Rust terminal UI. Recon's
baseline and retained transition history implementation is independent.

For release checks and publishing, see [release operations](RELEASES.md).
For documentation images, see [screenshot capture](SCREENSHOTS.md).
