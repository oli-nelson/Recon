# Recon

A Rust terminal app that watches a Git working tree and keeps a bounded, in-memory
history of the edits it observes. The left pane is a stable tree of affected
paths; the right pane is an immutable chronological hunk stack. Select a file to
filter the stack. Git's index/worktree status appears alongside baseline status.

```sh
cargo build --release --locked
./target/release/recon                      # discover from current directory
./target/release/recon /path/to/worktree
./target/release/recon --once                # snapshot for noninteractive use
./target/release/recon --ignore 'generated/**' --ignore '*.tmp'
./target/release/recon --reduced-motion --theme light
```

Requires Rust 1.88 or later and Git. Linked worktrees and repositories without a
first commit are supported. No commands write, stage, reset, or commit repository
contents. Recon takes its opening snapshot against HEAD, including staged,
unstaged, and nonignored untracked files, comparing final working contents.

| Key | Action |
| --- | --- |
| `c` | Discard history and capture current contents as the new baseline |
| `g` | Discard history and take a fresh snapshot against HEAD |
| `j` / `k`, arrows | Navigate tree or scroll the focused history |
| `h` / `l`, arrows | Collapse / expand directories |
| `Tab` | Switch focus; switch panes in narrow terminals |
| `d` | Hide / show history without losing its filter or reading position |
| `Enter` | Expand / collapse the history entry at the top of the panel |
| `PageUp` / `PageDown` | Scroll history, pausing live follow |
| `End` | Resume following the live end |
| `Esc` | Show aggregate history |
| `?` / `q` | Help / quit |

Tree symbols: `+` new, `~` modified, `−` deleted tombstone, `·` no net change.
These describe the active baseline. `[XY]` independently describes the Git index
and working tree, with `??` for untracked files. A staged change canceled by the
working contents has a status-only explanation. Reliably detected staged renames
show their origin; otherwise paths appear as deletion and creation. History uses
UTC timestamps. Oversized entries initially collapse. Wide lines are clipped to
the panel width; their retained text is unchanged.

Repeated saves append observed transitions. Undo records a reverse edit. Events
inside the debounce window coalesce; this is not a keystroke recorder. Files
reverted to baseline stay in the tree while retained history references them.
HEAD changes automatically start a fresh snapshot with an explanatory boundary
notice. Staging alone updates Git status without adding a content edit.

`--help` lists configuration. Defaults:

- 1,000 history entries or 64 MiB, whichever is reached first.
- 1 MiB maximum textual file; larger/binary files get size/status summaries.
- 256 MiB retained snapshot text budget. Beyond it, fingerprints remain accurate
  and unavailable text is explicitly summarized. Clear must read all candidate
  files to capture a baseline; its progress appears in the footer.
- 75 ms burst debounce, with a 250 ms maximum coalescing window.
- Safety reconciliation every 30 seconds. Overflow and watcher errors mark a
  history gap; only observed endpoints are compared. `--reconcile-seconds`
  controls the safety interval. Inaccessible files retain their last observation
  and retry on reconciliation.
- Dark theme and short activity animations. `--reduced-motion` retains brief bold
  activity emphasis, `--no-animations` removes it. `--theme light` is also available.

A single background worker owns Git and diff computation. Bounded notification,
command, and result channels prevent work from accumulating indefinitely. Normal
file events query only the affected paths; metadata changes, directory changes,
and safety recovery reconcile Git status. Refreshes publish small batches with
progress, while terminal input stays on the UI thread. Overflow replaces queued
path work with a reconciliation. Results are ordered by one worker, so older
parallel computations cannot overwrite newer observations. The UI builds only
visible history lines. Idle operation performs no terminal redraws.

Symlink targets are displayed as text without opening their destination; parent
symlinks are not traversed. Submodules are opaque paths; nested events are
ignored and dirty status reconciles periodically. Native watchers may still
subscribe to nested directories at the operating-system level. Non-UTF-8 path
names produce an explicit error; non-UTF-8 contents get binary summaries. Git
clean/smudge filters and CRLF normalization are not applied to content comparisons:
Recon compares actual bytes on disk with HEAD blob bytes. A multi-file baseline
capture observes each file once; it is not an atomic filesystem snapshot.

## Development and validation

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/benchmark.py
python3 scripts/memory_stress.py
python3 scripts/smoke_ui.py --large
```

Tests create disposable repositories and cover baseline transitions, repeated
edits, undo, delete/recreate, staged cancellation, ignores, retention, binary
files, symlinks, linked worktrees, HEAD boundaries, missed-event reconciliation,
atomic saves, nested repositories, cancellation, and narrow terminal rendering. The benchmark creates 100,000
tracked files, 10,000 local changes, and a 2,000-file burst; fixture creation is
excluded from startup/latency measurements. See [performance notes](docs/PERFORMANCE.md)
for measured results and remaining validation scope.

[Livediff](https://github.com/SoCkEt7/Livediff) informed the live terminal viewing
reference: native notifications, burst coalescing, and a Rust terminal UI. Recon's
baseline and retained transition history implementation is independent.
