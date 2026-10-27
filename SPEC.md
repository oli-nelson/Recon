# Recon

Recon is a high-performance live Git diff viewer for the terminal, written in
Rust. It provides a beautiful, intuitive view of work as it happens: a tree of
affected files alongside an animated history of incoming diff hunks.

## Startup, baselines, and reset

- A new instance immediately shows the latest local changes against `HEAD`,
  including staged, unstaged, and untracked files. Ignored untracked files are
  excluded by default.
- The initial hunk stack contains the existing local diff, labeled as the opening
  snapshot. Subsequent entries record newly observed edits.
- Pressing `c` clears the tree and hunk history and establishes the current file
  contents as the session baseline. Only subsequent activity appears; existing
  local changes do not immediately reappear.
- Pressing `g` replaces the view and history with a fresh snapshot of the latest
  local changes against the current `HEAD`. Watching continues from that snapshot.
- These commands affect Recon's view only. They never modify files, stage changes,
  discard edits, or alter Git history.
- Display the active baseline (`HEAD` or the last clear) and refresh progress.
- The Git snapshot shows final working-tree contents against `HEAD`, with
  staged/unstaged status also visible. A file whose index changes cancel out in
  the working tree still appears with its Git status and an explanation when
  there is no net working-tree hunk.

## File tree and activity

- Show only affected files and their parent directories. Include files represented
  in retained history, even when they no longer have a net change.
- New files fly in from the right and use a distinct new-file color.
- Modified files that are not yet visible fly in from the right and use a distinct
  modified-file color. Further edits briefly animate the existing row.
- Editing a new file preserves its new-file color.
- Deleted files animate out toward the right, then leave a greyed-out tombstone.
- Separate persistent status from transient activity: symbols and colors show
  status relative to the active baseline; brief animation highlights new activity.
- Reverting to baseline preserves the file's historical entries and marks its row
  as having no net change. Recreating a deleted file restores its live row, with
  status determined relative to the baseline; deletion history remains visible.
- Keep paths in stable order, allow directory expansion/collapse, and preserve
  selection by file identity as other changes arrive.
- Use symbols or labels alongside colors. Support reduced motion and disabling
  animations entirely.

## Hunk history panel

- A toggleable side panel shows a chronological history of incoming diff hunks,
  grouped into file-update entries labeled with path, time, and activity type.
- Pressing `d` hides or shows the sidebar in both aggregate hunk-stack and
  focused-file diff modes. While hidden, the tree uses the available width and
  history continues to accumulate. Restore the previous filter and scroll state
  when reopening; selecting a file does not automatically reopen a hidden sidebar.
  Hiding the sidebar while it has focus returns focus to the tree.
- The stack is history. Repeated saves append entries rather than replacing
  earlier hunks with the file's current diff.
- After the opening/reset snapshot, each entry compares the previous observed
  contents with the newly observed contents. Undo produces a reverse-edit entry,
  even if it restores the baseline.
- Coalesce rapid filesystem notifications into one observed content update.
  History records observed transitions, not every low-level event or keystroke.
  Notifications without content changes do not create duplicate entries.
- Animate new entries into the stack. Oversized hunks start collapsed with a
  summary and can be expanded interactively.
- Selecting a file filters history to that file without deleting other entries.
  Pressing `Esc` restores the aggregate history.
- Follow incoming entries at the live end. Scrolling back pauses following and
  preserves reading position. Indicate new activity and provide a return-to-live
  action.
- Bound retained history by configurable entry and byte limits. Evict oldest
  entries and indicate truncation. Retained entries must remain readable after
  later file edits. Clearing/resetting discards history.
- History is in memory for this instance; persistence is outside the first release.
  Eviction does not remove tree rows with a current net change. Rows with no net
  change can disappear once no retained history references them.

## Keyboard and navigation

| Key | Behavior |
| --- | --- |
| `c` | Clear the view and observe from current contents |
| `g` | Reset to the latest local Git changes |
| `j` / `k`, Down / Up | Navigate tree files or scroll the focused history panel |
| `h` / `l`, Left / Right | Collapse/expand tree directories |
| `Tab` | Switch focus between tree and visible history panel |
| `d` | Hide/show the sidebar, for both hunk history and focused file diffs |
| `Enter` | Expand/collapse the focused history entry or hunk |
| `PageUp` / `PageDown` | Scroll history by a page |
| `End` | Return to the live end and resume following |
| `Esc` | Clear the file filter and show all history |
| `?` | Show keybinding help |
| `q` | Quit |

Navigating to a file selects it and filters history. Scrolling the panel does not
change file selection. Adapt to narrow terminals while keeping both views
accessible, and preserve navigation state across terminal resizes.

## Repository and file handling

- Discover the repository from a supplied path or the current directory, including
  linked Git worktrees. For a repository without a first commit, use an empty
  initial baseline.
- Respect Git ignore rules and allow additional user-defined ignore patterns.
- Show reliably identified renames as renames; otherwise use deletion and creation.
  Correctness must not depend on expensive rename detection.
- Show binary and oversized files with status/size summaries instead of unbounded
  textual diffs. Make text-diff size limits configurable.
- Handle symlinks without following them outside the repository. Show submodule
  status without recursively watching nested repositories by default.
- Detect changes to `HEAD`, the index, and ignore rules as well as working files.
  Keep Git status current when staging or committing. Staging alone does not
  append a content-edit event.
- On a branch switch or other `HEAD` change, mark the boundary and rebuild from a
  fresh Git snapshot. Explain the automatic reset so baselines are not silently
  mixed.
- Recover from atomic saves, temporary removals, inaccessible files, and watcher
  overflow. Reconcile missed events and identify history gaps; do not invent
  intermediate edits that were not observed.

## Performance

Use [Livediff](https://github.com/SoCkEt7/Livediff) as a reference for live terminal
diff viewing. Validate Recon independently on representative large repositories.

- Use filesystem notifications, burst coalescing, and incremental refreshes. Avoid
  a full repository scan for every event.
- Keep Git queries and diff computation outside the UI loop. Bound work queues
  and concurrency; discard stale results superseded by newer contents.
- Render visible content and redraw on activity. Run animation ticks only while
  needed; the UI should be effectively idle when nothing changes.
- Batch or shorten animations during heavy activity so they do not accumulate a
  backlog. Prioritize responsive input and convergence to current contents.
- Bound memory used by history, snapshots, diff results, and pending work. Check
  file-size limits before loading contents for textual diffs.

Initial targets, to validate and tune on a documented reference machine:

| Scenario | Target |
| --- | --- |
| Repository scale | 100,000 tracked paths and 10,000 locally changed files |
| Normal edit to text files up to 1 MiB | p95 visible update within 250 ms after the final write |
| Burst affecting 2,000 small text files | Tree converges within 2 seconds after writes stop; diff computation may continue with progress visible |
| Input during refresh | Response within 100 ms |
| Idle CPU after settling | Below 1% of one core |
| Retained history defaults | 1,000 file-update entries or 64 MiB, whichever is reached first |

Record startup time and peak process memory as well. Document benchmark hardware,
repository fixtures, file sizes, and settings for reproducible measurements. The
history cap is not a cap on total process memory; establish a total memory budget
from these measurements before declaring the performance milestone complete.

## Delivery order and verification

1. Implement baseline/reset semantics, repository discovery, the live tree,
   selected-file diffs, and bounded background refresh work.
2. Add historical aggregate hunks, file filtering, collapse/expand, scrolling,
   retention limits, and recovery behavior.
3. Polish animations, themes, reduced motion, narrow-terminal layouts, and large
   repository performance against the documented targets.

Verify baseline transitions, repeated edits, undo, delete/recreate, retention, and
watcher reconciliation. Use burst fixtures to check responsiveness and ensure
stale results cannot overwrite newer state.
