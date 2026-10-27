# Performance validation

Measured on 2026-10-27 using the release build, Rust 1.88.0, Cargo.lock in this
repository, macOS 26.7.1 arm64, Apple M4 Pro (14 cores: 10 performance and 4
efficiency), 48 GB memory, Mac16,7. Fixture files were on the machine's local
filesystem in the system temporary directory. These are synthetic repository
measurements, not a promise for all filesystems or repository contents.

## Large repository

Command: `python3 scripts/benchmark.py` after `cargo build --release --examples`.
The native watcher measurement ran outside the agent filesystem sandbox, which
otherwise denies creation of macOS FSEvents streams. Recon reports that failure
and continues periodic reconciliation when run in that restricted environment.

Fixture: 100,000 tracked small UTF-8 files in 100 directories, 10,000 working-tree
modifications, plus a tracked 1 MiB UTF-8 file. Small originals contain a path ID
and six repeated content lines (roughly 110 bytes); modifications contain a path
ID and one changed line. The 1 MiB file consists of 65,536 sixteen-byte lines.
There are 20 sequential full-file saves changing its first line, followed by
2,000 rewrites of small files. Initial fixture construction (102.707 seconds,
including Git initialization/add/commit) is excluded from startup measurements.
Settings are the defaults except animations disabled and the safety interval
extended to 300 seconds to isolate native notifications.

| Measurement | Observed | Specification target |
| --- | ---: | ---: |
| Opening snapshot, 10,000 changed paths | 4,337 ms | Record |
| 1 MiB edit, p95 of 20 observations | 109 ms | ≤250 ms visible |
| 1 MiB edit, maximum | 109 ms | — |
| 2,000-file burst, after final write | 703 ms | ≤2,000 ms tree convergence |
| Burst, including writes | 921 ms | — |
| 160×48 terminal render, p95 of 20 | 4.606 ms | — |
| Visible PTY output after 1 MiB edit, p95 / maximum | 127.74 / 127.90 ms | ≤250 ms |
| Help key during 10,000-file refresh | 5.45 ms | ≤100 ms |
| Full app idle CPU over ten seconds | <0.1% (sampler precision) | <1% |
| Settled background-worker CPU over five seconds | 0.02% of one core | <1% total app |
| Peak process RSS | 65.14 MiB | Record |

Worker latency measures delivery to the application model, including native event
latency, coalescing, Git status, content comparison, and diff computation. The
separate visible-output measurement launches the actual release TUI in a 160×48
pseudo-terminal on the same large repository, performs 20 full 1 MiB writes, and
waits for a unique changed-line marker in flushed terminal output. This includes
the UI loop, render, and terminal backend, but excludes terminal-emulator paint
time. Full-app idle CPU uses `ps` cumulative process CPU over ten settled seconds;
the reported change was 0.00 seconds at centisecond resolution, so the result is
reported as less than 0.1%, rather than claiming zero CPU.

## Memory pressure

Command: `python3 scripts/memory_stress.py`.

300 tracked files each contain a single 1 MiB UTF-8 line, then all 300 are
replaced with different 1 MiB lines. This saturates the default 256 MiB snapshot
text budget and 64 MiB history budget; text beyond the snapshot budget is replaced
by explicit fingerprint/size summaries. It also exercises oversized single-line
hunks and byte-driven eviction rather than just the 1,000-entry limit.

- Peak RSS: **345.61 MiB**.
- Snapshot completion: **1,543 ms**.
- Retained history: **65,016,641 bytes**; **269 entries evicted**.
- All 300 current-change rows remain available.

The reference-fixture process memory budget is **512 MiB at default settings**.
The stress script asserts this budget. This is an acceptance budget for these
fixtures, not an operating-system-enforced RSS limit. Git metadata, path counts,
native watcher allocations, allocator fragmentation, and custom limits all affect
process memory independently of the history cap. Notification capacity is 4,096,
command capacity 8, result capacity 4. Results flush at 64 files or roughly 1 MiB
of diff payload (a single entry may be larger). Text diffs are capped at twice the
configured text-file limit, with an additional absolute 4 MiB output cap.

## Reproduction and scope

```sh
cargo build --release --locked
cargo test --locked
python3 scripts/smoke_ui.py
python3 scripts/smoke_ui.py --large
python3 scripts/benchmark.py
python3 scripts/memory_stress.py
```

The pseudo-terminal smoke checks navigation, sidebar focus, scroll, help,
terminal resize, clear/reset, live changes, quit, and terminal restoration. The
large mode sends a help key during the opening refresh of 10,000 affected paths
and asserts a response under 100 ms. Unit tests also render 1×1, 30×10, 80×24 and
140×40 layouts. Normal automated watcher tests use a one-second reconciliation
fallback, so native event performance is validated by the separate benchmark.

Remaining performance validation: idle CPU over longer samples, terminal-emulator
paint behavior, Linux/Windows native
watchers, network filesystems, long-running churn, and large production
repositories with real ignore/filter/submodule configurations. The numeric reference-machine targets above pass on these fixtures; broader
production and platform validation remains separate.
