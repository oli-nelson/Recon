# Recon

A Rust terminal app that watches a Git working tree and keeps a bounded, in-memory
history of the edits it observes. Newest entries appear at the top. The left pane is a stable tree of affected
paths; the right pane is an immutable edit stack. Select a file to
filter the stack. Git's index/worktree status appears alongside baseline status.

```sh
cargo build --release --locked
./target/release/recon                      # discover from current directory
./target/release/recon /path/to/worktree
./target/release/recon --once                # snapshot for noninteractive use
./target/release/recon --ignore 'generated/**' --ignore '*.tmp'
./target/release/recon --reduced-motion --theme light
./target/release/recon --fps 120             # frame submission cap (default 60)
```

Requires Rust 1.88 or later and Git. Linked worktrees and repositories without a
first commit are supported. No commands write, stage, reset, or commit repository
contents. Recon takes its opening snapshot against HEAD, including staged,
unstaged, and nonignored untracked files, comparing final working contents.

| Key | Action |
| --- | --- |
| `c` | Discard history and capture current contents as the new baseline |
| `g` | Discard history and take a fresh snapshot against HEAD |
| `/` | Live, case-insensitive file-path search; Enter keeps it, Esc cancels |
| `n` / `N` | Next / previous matching file while a search is active |
| `[` / `]` | Widen / narrow history; also drag the divider with the mouse |
| `j` / `k`, arrows | Navigate tree or scroll the focused history |
| `h` / `l`, arrows | Collapse / expand directories |
| `Tab` | Switch focus; switch panes in narrow terminals |
| `d` | Hide / show history without losing its filter or reading position |
| `Enter` | Open/focus selected file history; toggle directories or focused history entries |
| `PageUp` / `PageDown` | Scroll history, pausing live follow |
| `Ctrl-D` / `Ctrl-U` | Half-page down / up in the focused sidebar |
| `f` | Toggle live following; resume at the newest entry at the top |
| `Esc` | Show aggregate history |
| `?` / `q` | Help / quit |

The header shows the current branch (or detached HEAD / unborn branch) and updates
on branch switches, including switches between branches at the same commit.

Tree symbols: `+` new, `~` modified, `−` deleted tombstone, `·` no net change.
These describe the active baseline. `XY` independently describes the Git index
and working tree, with `??` for untracked files. Git letters occupy a fixed left
column, independent of indentation and motion. A small `●` beside each file
shows relative modification-time recency: newest timestamps are cyan, progressing
through blue and amber to muted for the oldest. Equal timestamps get equal colors.
The scale ranks all affected tree files and updates on observed filesystem changes;
it does not fade with wall time or shift when searching. Initial snapshots use
filesystem modification times. Staging alone preserves their age. Metadata-only touches may change
the rank without adding edit history. Deleted files use their observed deletion time. Modification
times are filesystem metadata and can be preserved by external tools; they are
not a reliable record of every edit.
A staged change canceled by the working contents has a status-only explanation. Reliably detected staged renames
show their origin; otherwise paths appear as deletion and creation. History uses
UTC timestamps. Oversized entries initially collapse. Wide lines are clipped to
the panel width; their retained text is unchanged.

Repeated saves record observed transitions. New entries appear above older entries;
live-follow stays at the top. Scroll down to read older history. While paused,
incoming entries preserve your reading position by entry identity. Undo records a reverse edit. Events
inside the debounce window coalesce; this is not a keystroke recorder. Files
reverted to baseline stay in the tree while retained history references them.
Deleted untracked files leave the tree immediately; their observed transitions
remain readable in history. Recreating them restores their tree rows.
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
- Transparent terminal background with dark/light foreground palettes. Rounded
  panels, connected history entries, clipped spring entrances with a bounded
  overshoot, eased exits, fading activity
  highlights, and a refresh spinner.
- Frame submissions capped at 60 FPS by default; `--fps 120` or `--fps 144`
  raises the cap (1–240). Motion follows elapsed time, so slow frames skip ahead
  without queuing animation work. Only activity and refreshes animate; idle
  operation does not redraw. Both panes retain their chosen width across toggles
  and terminal resizes. Narrow layouts keep the stored ratio for later.
- `--reduced-motion` retains brief bold activity emphasis; `--no-animations`
  removes it. `--theme light` changes the foreground palette.

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

There is no portable display-refresh/vsync callback in Recon's text terminal
connection. `--fps` controls application frame submission, not monitor refresh.
The terminal controls actual painting and monitor synchronization (for example,
[Ghostty's window-vsync](https://ghostty.org/docs/config/reference#window-vsync)
and [kitty's sync_to_monitor](https://sw.kovidgoyal.net/kitty/conf/#opt-kitty.sync_to_monitor)).
Recon brackets frames with synchronized-update sequences to reduce partial-frame
tearing on supporting terminals. That protocol groups a complete frame; it does
not provide display timing or guarantee the requested FPS. Unsupported terminals
can ignore the sequences.

Search matches substrings of the full repository-relative path, ignoring case.
Matching parents expand temporarily without changing saved collapse state. Search
also limits visible history to matching paths; selecting a result narrows it to
that file. Esc while typing restores the prior query and selection. Esc after
accepting clears the search and restores aggregate history. Search, sidebar width,
and focus are in-memory session preferences.

## User configuration

Recon creates `config.toml` with documented defaults on first use:

- Unix/macOS: `$XDG_CONFIG_HOME/recon/config.toml`, falling back to
  `~/.config/recon/config.toml` when the variable is empty or not absolute.
- Windows: `%APPDATA%\recon\config.toml` (Roaming AppData).
- `--config PATH` selects a different file and creates it if missing.

Command-line options override file settings. Existing files are never overwritten;
invalid or unknown settings report an error with the file path. Partial configs
are supported. Restart Recon after editing. The shipped
[default config](config.default.toml) documents every setting, including frame cap,
spring duration, initial sidebar width, theme, ignores, and memory
budgets. For example:

```toml
fps = 120
animation_ms = 800
sidebar_width = 68
theme = "dark"
ignore = ["generated/**"]
```

Boolean CLI flags can override either direction:
`--no-animations` enables that option, while `--no-animations=false` overrides a
config that disables animations. The same applies to `--reduced-motion=false`.
Configuration loading is separate from pure CLI parsing in the Rust library, so
library fixtures and benchmarks can retain deterministic defaults.

## Development and validation

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
python3 scripts/benchmark.py
python3 scripts/memory_stress.py
python3 scripts/smoke_ui.py --large
python3 scripts/animation_smoke.py
python3 scripts/config_smoke.py
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
