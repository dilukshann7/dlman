# Refresh download address

Ordinary HTTP(S) downloads that are paused or failed offer **Refresh download address** in the context menu, overflow menu, and download information dialog. YouTube and HLS/DASH jobs retain their separate recovery paths.

Open the refresh dialog, then either paste a fresh file URL and select **Verify & Resume**, or open the source page and start the same file download again with browser interception enabled. DLMan offers the captured link for explicit confirmation. **Add as new download** ends refresh and opens the ordinary new-download dialog for an unrelated capture.

Only one refresh session can be open. It expires after 10 minutes. Cancelling or expiring the session preserves the existing address and partial files. If the original page was not captured, visit it manually or paste a replacement URL.

## Verification and preservation

DLMan checks the actual segment files and compares up to three deterministic samples of up to 4 KiB against exact HTTP byte ranges from the replacement. Existing known total sizes and conflicting strong ETags must agree. Unknown totals must be supplied by valid `Content-Range` responses before retained bytes can be reused. Sampling checks compatibility; it does not prove that every byte of two files is identical.

Missing partials, changed bytes, invalid or ignored ranges, unexpected HTML responses, encoded bodies, authentication failures, and inconsistent lengths leave the existing address and files untouched. Downloads explicitly named `.html`, `.htm`, or `.xhtml` retain HTML support and still undergo byte verification. DLMan does not automatically discard partial data or restart a failed verification.

After successful verification, one SQLite transaction replaces the address and applicable HTTP metadata while preserving job identity, destination, filename, queue, category, timestamps, and segment rows. The existing resume path then starts the transfer using current settings. A failure to start retains the verified address for another Resume attempt. Transfer workers validate ranges and response lengths before retaining appended data.

Refresh reserves the job against competing starts, retries, and metadata changes. Cancellation and deletion invalidate pending verification. Browser candidates stay in memory; candidate events omit cookies and signed query strings. Request cookies and Basic credentials are never forwarded across an origin boundary. Accepted browser cookies follow DLMan's existing cookie persistence behavior.

New ordinary browser downloads retain their source-page URL. Existing database records acquire nullable source-page and scratch-directory columns; existing HTTP validator columns are reused. Persisting the scratch directory allows jobs to find their partials after temp-storage settings change. Legacy jobs search current cache candidates and reject missing required partials.

## Validation boundary

The implementation includes JavaScript eligibility and store tests plus Rust tests for verification, range safety, redirects, reservation lifecycle, migration, and persistence. JavaScript tests and TypeScript/source checks can run without a desktop server or build. Rust tests, native desktop/browser acceptance, and actual downloads require a separately requested compilation/runtime check.

Implementation checks completed on 2026-10-24:

- All 32 JavaScript tests passed, including 11 refresh-specific eligibility and store tests. The early-capture regression was reproduced before its fix and passed afterward.
- Desktop and extension TypeScript checks passed with `tsc --noEmit`.
- Changed Rust sources parsed with `rustfmt --emit stdout`; Cargo metadata resolved offline without compilation.
- An in-memory SQLite check executed the authored upsert and rebind statements, verifying metadata and segment preservation plus nullable migration defaults. This does not exercise Rust/sqlx binding at runtime.
- An independent source review identified credential-resolution races, unguarded output moves, weak/strong ETag precedence, unknown-size persistence, and early-capture delivery. Each was addressed with source changes and regression tests.
- The 24 Rust tests were authored and parsed but not compiled or executed. No builds, dev servers, desktop launches, browser acceptance runs, commits, or pushes were performed.

Implementation decisions: reuse the current workspace to preserve existing edits; persist the scratch directory to prevent temp-policy changes from redirecting resumed writes; resolve saved credentials at the actual transfer start under its mutation guard; keep accepted-cookie persistence consistent with existing downloads. Bounded sample verification deliberately does not guarantee whole-file identity.
