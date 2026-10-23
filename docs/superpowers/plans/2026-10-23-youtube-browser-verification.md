# YouTube Browser Verification Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement the tasks in order. Do not compile Rust, build applications, start development servers, or publish anything without a separate request.

**Goal:** Let users explicitly share a verified YouTube browser session through the existing DLMan extension when anonymous extraction encounters a bot challenge.

**Architecture:** The desktop creates a random, single-use, ten-minute challenge for one canonical video and opens that video in the normal browser. The extension opens its own approval window. After explicit approval it reads only cookies applicable to that YouTube URL and submits them to the existing loopback server with the challenge. The core keeps the session in memory for thirty minutes, uses a protected temporary Netscape cookie file for its bundled tools, and persists only whether a job requires verification.

**Tech Stack:** Existing Rust/Tokio/Tauri backend, React desktop, WXT browser extension, Node's built-in test runner, and bundled yt-dlp.

**Spec:** This document records the scope selected in chat: use the DLMan extension, not direct browser-profile extraction.

## Global Constraints

- No application builds, Rust compilation, development servers, downloads, or publication.
- Keep existing unrelated working-tree changes intact; do not commit without a request.
- Keep passwords, cookies, challenge tokens, and signed stream URLs out of history, exports, diagnostic messages, and logs.
- Require explicit approval in an extension-owned page, validate the current tab again before sharing, and reject private browsing and unrelated cookie domains.
- Preserve anonymous downloads, existing quality selection, and rejection of DRM, live streams, and account-only media.
- A browser session is not a guarantee against YouTube's additional restrictions or proof-of-origin requirements.

## Review Focus

- Expired, replayed, cancelled, or wrong-video challenges must never install a session.
- Browser navigation or private/container tabs must not accidentally share another session.
- Cookie files must be protected before writing secrets and removed after the child process exits, including cancellation.
- Preview caching must distinguish anonymous and verified sessions; verified jobs must fail explicitly when their in-memory session is lost.
- Closing the dialog, removing a verification fragment, or an unavailable extension must not leave indefinite polling or leak credentials.

### Task 1: Shared verification boundary

- [x] Add executable JavaScript tests for link validation, cookie filtering, explicit sharing, expiry, and navigation changes.
- [x] Observe the new tests fail before implementing the shared helper module and declarations.
- [x] Implement the helper module and rerun the focused tests.

### Task 2: Core session lifecycle and tool integration

- [x] Add Rust regression tests for challenge matching, replay, expiry, cancellation, cookie validation, and temporary-file cleanup; review without running because execution compiles Rust.
- [x] Add a bounded in-memory verification store and serializable, non-secret desktop responses.
- [x] Add protected temporary cookie files and attach them to metadata and transfer commands only after explicit session sharing.
- [x] Persist only the required-session flag and distinguish preview cache entries by session identity.

### Task 3: Browser and desktop flow

- [x] Add desktop commands for beginning, polling, and cancelling a challenge and a loopback submission endpoint.
- [x] Add an extension-owned approval window and validate its sender before reading browser cookies.
- [x] Add Verify in browser and Cancel controls to the existing YouTube error state; retry inspection after sharing completes.

### Task 4: Verification and documentation

- [x] Run the JavaScript suite, TypeScript no-emit checks, Rust formatting/parser checks, and scoped diff checks.
- [x] Review the end-to-end source flow and address actionable findings.
- [x] Document session lifetime, use, and runtime verification limits. Rust compilation, browser acceptance, and actual YouTube extraction with cookies remain unverified until explicitly requested.

Verification: 13 JavaScript tests pass; desktop and extension TypeScript no-emit checks pass; changed Rust modules parse, new core/type sources pass rustfmt, scoped diffs pass whitespace checks, and locked offline Cargo metadata resolves without compilation. An independent source reviewer checked the flow; its three actionable findings (stale tab fragments, cookie-file lifetime, and anonymous-job retries) were addressed and re-reviewed. No runtime cookies were read, no Rust tests or native acceptance were run, and no app or extension build or development server was started.

## Execution rulings

- Ruling: implement in the existing dirty checkout and preserve unrelated edits; the feature is requested here and no separate checkout or commit is needed.
- Ruling: use per-token extension-storage entries for pending request metadata, never cookies; concurrent approval windows must not overwrite each other's requests.
- Ruling: require the original verification fragment before and after cookie access; if sign-in navigation removes it, the user starts a fresh challenge using the verified browser session.
- Ruling: allow extension submissions that omit Origin, while rejecting non-extension Origins; the random single-use token remains mandatory in every case.
- Ruling: an existing anonymous job may use a newly shared session for the same video, but must persist its required-session flag before any credentials are used.
