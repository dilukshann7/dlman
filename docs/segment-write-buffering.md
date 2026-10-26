# Segment write buffering

Each segment worker uses a private Tokio `BufWriter<File>` with a **512 KiB**
capacity. Small HTTP chunks share a file write. Chunks at least as large as the
capacity can pass directly to the file without an extra buffering copy.
The additional buffer allocation is 512 KiB per active worker, or 4 MiB for
eight workers; this excludes the existing HTTP and Tokio file buffers.

HTTP validation, rate limiting, progress counters and events still operate on
each accepted network chunk. Accepted bytes can temporarily be ahead of file
length while buffered. Segment checkpoints never persist that lead:

- Checkpoint, pause and cancellation: flush the buffer, sync the underlying
  file, then save progress.
- Split: flush, sync, commit the donor/child ownership transaction, advance the
  rollback boundary, then acknowledge the split.
- Rollback: flush, truncate, restore the file cursor, correct counters, sync and
  save. Subsequent flushes cannot restore rejected bytes beyond the boundary.
- Completion: flush and sync before setting and saving the completion flag.

A disk failure while handling a split terminates the worker and drops its reply.
The coordinator receives the worker error through its existing task-drain path.
A rejected database split transaction retains the existing behavior: report the
rejection and continue the donor without changing ownership.

## Reproduce the benchmark

From the repository root:

```powershell
cargo test -p dlman-core --lib --release segment_write_buffer_benchmark -- --ignored --nocapture --test-threads=1
```

The benchmark is ignored during normal tests. It writes and verifies **6 GiB**
over its complete run, using isolated temporary files. Each case writes 64 MiB
total across either one or eight workers, using 16 KiB, 64 KiB, 256 KiB or 1 MiB
chunks. It compares an unbuffered file with 256 KiB, 512 KiB and 1 MiB buffers,
rotates capacity order over three repetitions, and reports median throughput.
Timing includes final flush and `sync_all`; file creation, payload preparation
and verification of every output byte are outside the timed region.

Writer-call counts count successful writes accepted by the underlying Tokio
file writer. They are not operating-system syscall counts. The harness uses
four Tokio runtime threads. It isolates disk batching rather than measuring
HTTP throughput, SQLite checkpoints, splits or application responsiveness.

## Results: 2026-10-26

Measured on Windows 11 Home Single Language, Intel Core i7-11800H, Rust
1.99.1, optimized release tests, using the system temporary directory on C:.
Values are median MiB/s from three repetitions:

| Chunk | Workers | Unbuffered | 256 KiB | 512 KiB | 1 MiB |
| --- | ---: | ---: | ---: | ---: | ---: |
| 16 KiB | 1 | 413.68 | 1264.40 | 1345.97 | 1243.32 |
| 64 KiB | 1 | 771.79 | 1104.47 | 1126.36 | 1129.54 |
| 256 KiB | 1 | 925.00 | 962.81 | 1133.78 | 1059.28 |
| 1 MiB | 1 | 1160.94 | 1337.88 | 1376.98 | 1310.06 |
| 16 KiB | 8 | 370.04 | 780.82 | 949.44 | 426.07 |
| 64 KiB | 8 | 417.60 | 432.69 | 465.44 | 438.25 |
| 256 KiB | 8 | 411.41 | 447.98 | 500.45 | 525.27 |
| 1 MiB | 8 | 466.29 | 488.07 | 465.07 | 458.17 |
| **Geometric mean** | | **563.48** | **777.02** | **838.10** | **737.56** |

Selection uses the smallest buffer whose geometric-mean throughput is within
5% of the best buffered candidate. The cutoff was 796.19 MiB/s; 256 KiB missed
it and **512 KiB** won. Its geometric mean was approximately **48.7% higher**
than the unbuffered baseline on this machine.

The selected capacity was **0.26% slower** than the baseline for 1 MiB chunks
with eight workers (465.07 versus 466.29 MiB/s). Three short repetitions do not
establish the significance of that difference. Larger chunks bypass the buffer,
so batching provides no writer-call reduction in that case. The 1 MiB candidate
also underperformed smaller buffers on several cases, particularly 16 KiB
chunks with eight workers. These local measurements are not a guarantee for
other disks, platforms or end-to-end downloads.

Writer calls per 64 MiB case were identical for one and eight workers:

| Chunk | Unbuffered | 256 KiB | 512 KiB | 1 MiB |
| --- | ---: | ---: | ---: | ---: |
| 16 KiB | 4096 | 256 | 128 | 64 |
| 64 KiB | 1024 | 256 | 128 | 64 |
| 256 KiB | 256 | 256 | 128 | 64 |
| 1 MiB | 64 | 64 | 64 | 64 |

## Regression verification

Worker tests cover pending buffers at checkpoints and splits, rollback cursor
restoration, rollback across an ordinary checkpoint, completion with empty,
exact-capacity, oversized and partial buffers, flush failures, and Windows sync
failures. Local HTTP fixtures cover a genuinely empty response, live pause and
cancellation followed by exact-byte resume, existing donor clipping and rejected
responses after checkpoints.

Both parallel and serial fixture-setup runs exposed a SQLite row-metadata panic
in SQLx 0.7.4 (`the len is 23 but the index is 23`). The worker-test fixture now
drops its initial pool and reopens the migrated temporary schema before seeding
the download. This isolates disk-I/O tests from stale connection metadata; it
does not fix production database initialization. No database code or
dependencies were changed as part of this buffering work.

The initial full core run reported **185 passed, 5 failed, 4 ignored**. Three
failures encountered the SQLx panic: `expiry_releases_reservation_and_stale_session_cannot_apply`,
`start_does_not_send_credentials_from_a_stale_resume_snapshot`, and the new
`failed_split_flush_stops_the_worker_without_acknowledging_ownership` test before
the fixture adjustment. Two unrelated media tests also failed:
`media::dash::tests::test_parse_simple_mpd` expected two representations but
parsed zero, and `media::youtube::tools::tests::identifies_bot_checks_without_exposing_raw_diagnostics`
expected lowercase `public` while the current message contains `Public`.

The desktop application and native UI were not built or launched.

After the fixture adjustment, all **17 worker tests passed**, including with
the default parallel test runner. The subsequent focused `engine::` run reported
**76 passed, 3 failed, 1 ignored**; its remaining failures all hit the same SQLx
panic in existing fixtures:

- `engine::download_task::dynamic_tests::deletion_waits_for_split_workers_and_removes_all_parts_and_staging`
- `engine::refresh::tests::saved_replacement_is_preserved_if_subsequent_start_is_rejected`
- `engine::refresh::tests::start_does_not_send_credentials_from_a_stale_resume_snapshot`

These failures prevent claiming a green engine or full core suite. The existing
compiler warning at `crates/dlman-core/src/lib.rs:633` (unnecessary parentheses)
also remains unchanged. An independent static review of the buffering change
reported no actionable findings.
