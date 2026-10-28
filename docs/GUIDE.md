# Recon user guide

[Back to the README](../README.md) · [Configuration reference](../config.default.toml)

## Navigation and shortcuts

| Key | Action |
| --- | --- |
| `c` | Discard history and capture current contents as the new baseline |
| `g` | Discard history and take a fresh snapshot against HEAD |
| `/` | Live, case-insensitive file-path search; Enter keeps it, Esc cancels |
| `n` / `N` | Next / previous matching file while a search is active |
| `[` / `]` | Widen / narrow history; also drag the divider with the mouse |
| `j` / `k`, arrows | Navigate tree or move the focused history selection |
| `h` / `l`, Left/Right | Files: collapse/expand directories; history: scroll horizontally |
| `Tab` | Switch focus; switch panes in narrow terminals |
| `d` | Hide / show history without losing its filter or reading position |
| `Enter` | Open/focus selected file history; toggle directories or focused history entries |
| `e` | Open the focused file in your editor |
| `PageUp` / `PageDown` | Scroll history, pausing live follow |
| `Ctrl-D` / `Ctrl-U` | Half-page down / up in the focused sidebar |
| `f` | Toggle live following; resume at the newest entry at the top |
| `Esc` | Show aggregate history |
| `?` / `q` | Help / quit |

## Reading history

When history has focus, an accent marker identifies the selected entry and the
pane's bottom border names the file and expand/collapse action for Enter. The
marker remains visible when the entry's header is scrolled offscreen. Movement
keys move directly between history entries, revealing the selected entry when it
is offscreen. Page Up/Down and Ctrl-U/D scroll through long hunks.
Brief background checks keep the watching indicator and shortcut footer stable;
refresh progress appears only after 250 ms. Startup progress remains immediate.

Press `e` on a file in the tree or on a history entry to open that file using
`VISUAL`, then `EDITOR` if `VISUAL` is unset or blank. Without either, Recon uses
`vi` on Unix/macOS or Notepad on Windows. Set the variable to an executable or a
quoted command with arguments, such as `EDITOR='code --wait'`. Quote executable
paths containing spaces when adding arguments, especially on Windows. Recon
suspends its terminal UI while the editor runs, then resumes watching on exit;
include your GUI editor's wait option to keep it suspended until editing finishes.
Deleted files report a notice. The `edit` keybind can replace the default `e`.

## Baselines and file status

The header labels the active comparison as **HEAD** or **last clear
(contents at last clear)**. A coloured newest-to-oldest age-dot legend appears
at the top right only when it fits beside the full header text. The Files box's bottom border shows change counts and the observed Git
root path, shortening long paths from the left to keep the repository name visible.
Wide file panes also show entry counts and history size there. The single-row header
and one-row footer leave the remaining height for files and history; narrow
terminals use shorter labels. Progress, warnings, and search
share the footer with help/quit hints. Notifications disappear after three seconds
or immediately on keyboard, mouse, or paste input, restoring the keybind hints.
An accent-to-muted gradient sweeps across notification text to show time remaining;
reduced-motion and disabled-animation modes keep the text static. It also shows the current branch (or detached HEAD / unborn branch) and updates
on branch switches, including switches between branches at the same commit.

Tree symbols: `+` new, `~` modified, `−` deleted tombstone, `·` no net change.
These describe the active baseline. `XY` independently describes the Git index
and working tree, with `??` for untracked files. Git letters occupy a fixed left
column, independent of indentation and motion. A small `●` beside each file
shows relative modification-time recency using the gradient shown in the header.
The original theme runs from cyan through blue and amber to muted for the oldest;
Ashen runs from warm orange through gold and teal to muted. Equal timestamps get equal colors.
The scale ranks all affected tree files and updates on observed filesystem changes;
it does not fade with wall time or shift when searching. Initial snapshots use
filesystem modification times. Staging alone preserves their age. Metadata-only touches may change
the rank without adding edit history. Deleted files use their observed deletion time. Modification
times are filesystem metadata and can be preserved by external tools; they are
not a reliable record of every edit.
A staged change canceled by the working contents has a status-only explanation. Reliably detected staged renames
show their origin; otherwise paths appear as deletion and creation. History uses
UTC timestamps. Oversized entries initially collapse. Wide lines are clipped to
the panel width; focus history and use Left/Right or h/l to scroll through them.
Entry headers and selection markers stay fixed, and retained text is unchanged.

## What gets recorded

Repeated saves record observed transitions. New entries appear above older entries;
live-follow stays at the top. Scroll down to read older history. While paused,
incoming entries preserve your reading position by entry identity. Undo records a reverse edit. Events
inside the debounce window coalesce; this is not a keystroke recorder. Files
reverted to baseline leave the tree once Git also reports no local changes;
their observed transitions remain readable in history.
Deleted untracked files leave the tree immediately; their observed transitions
remain readable in history. Recreating them restores their tree rows.
HEAD changes automatically start a fresh snapshot with an explanatory boundary
notice. Staging alone updates Git status without adding a content edit.

## Defaults and resource limits

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
- Transparent terminal background with dark (original default), light, and Ashen palettes. Rounded
  panels, connected history entries, clipped spring entrances with a bounded
  overshoot, eased exits, fading activity
  highlights, and a refresh spinner.
- Frame submissions capped at 60 FPS by default; `--fps 120` or `--fps 144`
  raises the cap (1–240). Motion follows elapsed time, so slow frames skip ahead
  without queuing animation work. Only activity and refreshes animate; idle
  operation does not redraw. Both panes retain their chosen width across toggles
  and terminal resizes. Narrow layouts keep the stored ratio for later.
- `--reduced-motion` retains brief bold activity emphasis; `--no-animations`
  removes it. `--theme light` or `--theme ashen` changes the palette. Ashen uses warm ember
  accents and greys inspired by the Ashen Neovim theme.

## Filesystem behavior and limitations

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
clean/smudge filters and working-tree encoding conversions are not executed.
For UTF-8 text with Git line-ending conversion enabled (`core.autocrlf`, `text`,
or `eol` attributes), Recon compares and displays LF-normalized text, so CRLF
checkouts do not invent edit hunks. `-text`, binary data, and symlink targets retain
their exact bytes. Without configured conversion, line endings remain significant.
Executable bits are ignored when `core.filemode=false` or on Windows; repositories
with `core.symlinks=false` compare tracked symlink targets against plain checkout
files. A multi-file baseline
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

## Search

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
[default config](../config.default.toml) documents every setting, including frame cap,
spring duration, initial sidebar width, theme, ignores, and memory
budgets. For example:

```toml
fps = 120
animation_ms = 800
sidebar_width = 68
theme = "dark"
ignore = ["generated/**"]
```

Keybinds can be replaced individually in a `[keybinds]` table. Help, footer hints,
and empty-history hints display your configured keys. Omitted actions keep their
defaults; an empty list disables an action. Duplicate keys across actions fail
with an error (unbind or move the other action when reusing its key).

```toml
theme = "ashen" # "dark" is the original default; "light" also supported

[keybinds]
quit = ["Ctrl-x"]
down = ["j", "Down"]
clear = ["Alt-c"]
```

Keys accept single case-sensitive characters, `Space`, `Enter`, `Esc`, `Tab`,
`BackTab`, arrows, `PageUp`, `PageDown`, `Home`, `End`, `Backspace`, `Delete`,
`Insert`, and `F1`–`F24`, with optional `Ctrl-`, `Alt-`, or `Shift-` prefixes. Prefer uppercase
characters for shifted letters. While entering a search, printable characters
edit the query and Enter/Esc/Backspace/Ctrl-U keep their editing functions.
Any key closes help. The complete action list is in the default config.

Boolean CLI flags can override either direction:
`--no-animations` enables that option, while `--no-animations=false` overrides a
config that disables animations. The same applies to `--reduced-motion=false`.
Configuration loading is separate from pure CLI parsing in the Rust library, so
library fixtures and benchmarks can retain deterministic defaults.

