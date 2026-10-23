# YouTube implementation ledger

The approved chat plan is the implementation specification. Public individual
videos and Shorts, video quality selection and MP3 conversion are supported entry
points through the extension and existing desktop Add Download dialog.

## Constraints

- Preserve DLMan's current visual design and existing HTTP/HLS/DASH behavior.
- Cover the existing four desktop release targets.
- No application builds, Rust compilation, development servers, live downloads,
  or release publication without a separate request.

## Progress

- Planning and source comparison complete.
- Source implementation complete on `codex/youtube-support`.
- Desktop and extension TypeScript no-emit checks pass.
- Eight JavaScript fixtures and five Python package fixtures pass.
- New and modified Rust sources parse with rustfmt; compilation and Rust tests
  were not run. Native runtime and all-platform package acceptance remain unverified.
- The Windows tool resources were copied from the reference package, verified by
  hash and architecture, and left Git-ignored. Foreign-platform tool archives
  are pinned for release preparation but were not downloaded or executed.
- A whole-change source review and its follow-up fixes are complete.

## Rulings

- Work in the clean current checkout on a feature branch. No new worktree, baseline compilation, or unsolicited commits.
- Rust tests will be authored and reviewed but not executed because they compile.
- Persist media metadata as optional JSON so a malformed individual job can fail
  without making the entire download history unreadable.

- Ruling: install only locked frontend dependencies with lifecycle scripts disabled
  to enable TypeScript checking; offline dependencies were unavailable. No app builds.
- Ruling: package preparation computes per-file hashes from checksum-verified
  archives, and the core build script embeds them. This avoids trusting a writable
  runtime manifest and supports platform bundles without committing binaries.
