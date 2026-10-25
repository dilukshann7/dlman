# Pasted webpage video detection

DLMan inspects public, on-demand video webpages and HLS/DASH manifests in the existing Add Download dialog. It uses the bundled, verified yt-dlp and FFmpeg tools. Ordinary files retain the normal download flow; existing YouTube interfaces remain compatible.

## Implementation record

- Backend: shared format normalization, managed transfer primitives, generic inspection and capture service, additive job persistence, and lifecycle routing.
- Desktop: debounced inspection, stable quality selection, capture source selection, and unchanged destination/category/queue controls.
- Extension: explicit localhost capture bridge, tab-bound discovery, and temporary domain-scoped playback context.
- Verification: fixture-based JavaScript tests, TypeScript checks, Rust parsing/format checks, and focused Rust tests authored for later compiled execution.

The approved scope excludes builds, Cargo compilation, dev servers, app launches, DRM, account-required videos, playlists, live recording, and new audio conversion modes. Native extraction and browser-to-desktop acceptance require an explicitly requested rebuild and live check.

## Progress

- Started on `codex/webpage-video-detection` from a clean checkout.
- Interface decision: reuse existing video preview and selection wire shapes, with a separate `web_media` persisted job field and generic commands. YouTube validation remains separate.
- Browser capture tickets expire after 10 minutes. Captured request context remains only in memory for up to 30 minutes so closing the dialog does not invalidate a queued transfer. After expiry or restart, recapture is required.

## HTML5 and KVS players

The generic native extractor handles HTML5/Video.js `<video><source>` markup and KVS `flashvars` configuration, including alternate quality URLs. DLMan accepts progressive video metadata with unknown codecs, retains the extractor's stable format IDs and quality labels, and re-extracts the source webpage before each transfer. Explicit audio-only, silent, and adaptive-track declarations retain their existing handling.

Browser capture reads source elements even when `preload="none"` leaves `readyState` at zero. It also reads literal KVS `video_url`, `video_alt_url`, and numbered alternate URLs with their matching `_text` labels from inline scripts. This fallback never evaluates page JavaScript; obfuscated KVS URLs remain the responsibility of the native extractor or actual network playback detection.

Signed query strings and trailing slashes on media endpoints remain intact. KVS `v-acctoken` is treated as a rotating authorization parameter when identifying a captured source; different resource and quality selectors remain distinct. Credentialed browser capture remains limited to an explicitly claimed capture ticket. Normal video overlays also perform passive source discovery and anonymous inspection; see [Earlier video detection and ad filtering](earlier-video-detection.md).

The supplied example page was fetched as HTML on 2026-10-25: it returned HTTP 200 with one video element and two `video/mp4` sources labeled 720p and 480p. This verifies the page structure, not native extraction or downloading. No media was downloaded during inspection.

## Prior webpage-video verification

- All 78 JavaScript tests pass. These cover extraction of literal KVS configuration, HTML5 qualities before playback, preserved signed URLs, trailing-slash endpoints, rejected executable expressions, and the actual extension content-script adapter, in addition to existing inspection, capture, file, and YouTube regressions.
- Desktop and extension TypeScript checks (`tsc --noEmit`) pass without emitting application builds.
- All 19 changed or new Rust files pass syntax inspection with rustfmt using whitespace-normalized input; the normalization avoids pre-existing formatter failures in `commands.rs` and does not rewrite that file. Focused Rust tests are authored for progressive metadata with absent codecs, quality labels and stable selections, explicit silent/adaptive tracks, rotating KVS tokens, persistence, and existing capture behavior. These tests have not been compiled or executed.
- No application build, dev server, app launch, native extraction, or end-to-end download acceptance was run.
