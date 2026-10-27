# Recon performance review

Reviewed on 2026-10-27. This review covers the working tree, including the
uncommitted UI and configuration changes present during the review.

Recon has a sound foundation: one ordered background worker, bounded channels,
burst coalescing, bounded retained text and history, streaming hashes for large
files, a diff computation deadline, and no idle redraws. The main opportunities
are eliminating repeated work proportional to the affected-file count or file
size.

The four findings below are P2 performance improvements. Start with visible-row
rendering and caching immutable HEAD data.

## 1. Build only visible file rows and cache the tree

Sources: [`tree()`](src/ui.rs#L88) and
[ListItem construction](src/ui.rs#L722).

Every frame rebuilds a `BTreeSet` of file and directory paths, checks collapse
state, and allocates `ListItem`s for every expanded row before Ratatui clips the
result to the viewport. Selection changes, search, and animation repeat work for
offscreen files.

Release-build measurements with a 160×48 `TestBackend`:

| Affected files | Median draw time |
| --- | ---: |
| 1,000 | 0.67 ms |
| 10,000 | 5.62 ms |
| 50,000 | 28.55 ms |
| 100,000 | 60.83 ms |

A 60 FPS frame has a 16.7 ms budget. The measured draw work alone exceeds that
budget at 50,000 affected files, before terminal output and painting costs.

Cache structural rows until paths, search, or collapse state changes. Construct
and style only the visible rows, while preserving selection and scroll behavior.
Cached counts and recency data can also avoid full-model work on animation frames.

## 2. Cache immutable HEAD fingerprints and content

Sources: [baseline lookup for each path](src/engine.rs#L183),
[`base()`](src/engine.rs#L112), and
[large HEAD blob reads](src/repository.rs#L312).

Every `update_paths()` iteration calls `base()`. In HEAD mode this reloads the
baseline through `head_content()`, even when HEAD has not changed. Large blobs
launch a fresh `git cat-file` process and are streamed and hashed in full on every
refresh. The working file is also read fully.

Ten repeated refreshes of one already-observed, modified 32 MiB file, with no
further edits, averaged approximately 80–81 ms per refresh in a repeat run. Both
path-scoped refresh and full reconciliation showed this cost. This measurement
includes status and comparison work; it does not isolate subprocess overhead.

Cache the HEAD tree and baseline fingerprints by blob identity. Bound cached text
separately so this optimization preserves the memory-budget behavior. Invalidate
or replace the baseline mapping when HEAD changes.

Reusing working-file observations could save additional reads, but needs careful
metadata validation and safety reconciliation so real edits are not missed.

## 3. Filter Git-ignored build noise before scheduling refresh work

Source: [watcher path classification](src/watch.rs#L251).

`classify()` calls `excluded()`, which checks custom ignores, `.git`, and
submodule descendants, but does not apply Git ignore rules. An ignored build
file therefore still queues path work. An ignored directory event requests a full
reconciliation.

A temporary unit probe created `build/artifact.bin` with `build/` in `.gitignore`.
It confirmed that Git considers the file ignored, while watcher classification
returns path work for the file and full reconciliation for its directory.

Heavy build output can consume queue capacity and cause unnecessary status scans,
content comparisons, and UI refreshes. The classification behavior is verified;
its end-to-end CPU and latency impact under build churn was not measured.

Filter known ignored untracked paths before queueing expensive work. Preserve
events for tracked paths, ignore-rule changes, and previously observed paths that
need removal from the model. Cache ignore decisions only with appropriate
invalidation.

## 4. Avoid sweeping every live file after each model batch

Source: [history reference collection and file retention](src/model.rs#L177).

Every `Update::Files` builds a retained-history path set and calls
`files.retain()`, scanning all live rows even when one row changed and no history
was evicted. Since worker batches flush at 64 paths or approximately 1 MiB of diff
payload, loading N changed paths in fixed-size batches incurs approximately
O(N² / 64) live-row visits.

A repeat synthetic run loading rows in 64-file batches, without history entries:

| Rows loaded | Model-only load time |
| --- | ---: |
| 10,000 | 5.1 ms |
| 50,000 | 67.2 ms |
| 100,000 | 375.3 ms |

Maintain history reference counts per path. Reconsider only changed paths and
paths whose last history reference was evicted, preserving the existing rules
for status-only rows and retained history.

## Measurement scope and validation

The focused probes used a disposable source copy and fixture repositories under
`/tmp`, locked dependencies, and an optimized release build. The tree/model probe
used a 160×48 `TestBackend`, animations disabled, an expanded tree, short paths,
and no history entries. The large-file probe used a tracked 32 MiB file changed
once before repeated refreshes. Fixture creation was excluded from the reported
refresh timings.

These are synthetic microbenchmarks, not terminal-emulator paint measurements or
end-to-end native-watcher benchmarks. The rendering figures count affected files
in the model, not total tracked files in the repository. Timings varied between
runs; the tables use the repeat-run results rather than treating individual
measurements as guarantees.

The working tree changed concurrently during review. The measured tree, engine,
model, repository, and watcher paths were checked against the checkout afterward;
subsequent UI changes were outside the measured rendering path. An earlier
transient undefined-`light` compilation error was corrected during the review and
is not an outstanding finding.

The latest working-tree validation command was:

```sh
CARGO_HOME=/tmp/recon-cargo cargo test --locked --offline
```

All 24 unit tests and 14 integration tests passed. The ignored-output
classification probe also passed in the temporary copy. Review probes did not
modify repository source files; their temporary harness is not part of the
repository.

Existing broader measurements and platform limitations are documented in
[`docs/PERFORMANCE.md`](docs/PERFORMANCE.md). Useful follow-up coverage includes
Git-ignored build churn, many large changed assets, deep directory trees, many
collapsed directories, long-running history churn, and Linux/Windows watchers.
