# Dynamic segmentation implementation record

The approved plan enables automatic splitting of the largest unfinished HTTP(S)
range when a connection slot becomes available. Initial fan-out and the existing
connection setting remain. Splits leave at least 1 MiB on either side.

## Constraints and decisions

- Implementation used the existing checkout on `codex/dynamic-segmentation`,
  without a separate worktree or new dependencies. The user subsequently
  authorized focused commits, pushing the branch, and merging into the fork's
  `main` branch. The source-only verification boundary remains in effect.
- The user's source-only verification boundary overrides skill instructions to
  run red/green tests, baseline builds, fixture servers, and full test suites.
  Tests are written before the corresponding implementation where practical,
  but their execution and Rust type/runtime verification remain deferred.
- Split acknowledgements follow durable donor checkpointing and an atomic SQLite
  update. Segment IDs remain stable and independent of connection slots.
- The donor keeps its original response and stops writing at its shortened end.
  A split advances its rollback checkpoint to preserve committed bytes.

## Progress

- Implemented the continuously scheduled worker pool, acknowledged tail splits,
  atomic persistence, durable rollback boundary, and saved-file reconciliation.
- Resume and restart paths use the configured connection limit rather than the
  number of stored ranges. Ranges retain stable IDs across repeated splits.
- Desktop progress finds segments by ID and renders them in byte order, with
  bar widths proportional to their byte ranges.
- Assembly validates coverage and exact lengths, publishes a synced staging
  file, and retains all partials until completion has been persisted.
- Authored 24 regression tests: 5 range-planning tests, 3 worker-control tests,
  14 persistence/transfer/assembly tests, and 2 desktop store tests.

## Source review and follow-up fixes

An independent read-only review identified four issues. The implementation now
uses metadata-only updates for live speed, queue, category, and destination
edits; ordinary progress checkpoints do not advance the response rollback
boundary; observing pause marks the manager handle as stopping; unsuccessful
assembly removes staging files after closing their handles.

Deletion waits for worker drainage under the manager mutation guard, reloads the
latest split map, and removes every partial and any interrupted staging file.
Completion writes only transfer-owned fields, preserving concurrent user edits.
Range-map events reload current user metadata before publication.

Changing an ordinary active download's destination now requires its workers to
stop first, because the task owns its destination and scratch paths for the
duration of the transfer. This prevents metadata from pointing to a different
location than the output being assembled.

## Verification

- `pnpm --filter @dlman/desktop exec tsc --noEmit`: passed.
- `git diff --check`: passed.
- Rust sources parsed with `rustfmt --edition 2021 --config
  skip_children=true,disable_all_formatting=true --emit stdout`; no Rust
  compilation or type checking was performed.
- `node --check apps/desktop/tests/segmentation.test.mjs`: syntax check passed;
  the tests were not executed.

Rust tests, HTTP fixtures, builds, app launches, and native UI validation remain
unrun under the approved source-only verification boundary. The authored tests
have not been compiled or executed; runtime correctness is not claimed.
