# Earlier video detection and ad filtering

## Goal and implementation

The extension discovers downloadable video before playback from HTML5 sources,
Video.js data configuration, literal JW Player setup/source lists, KVS flashvars,
and structured VideoObject metadata. Related qualities remain attached to one
player. Signed URLs, frame rates, and trailing slash endpoints are preserved.

DOM snapshots cover permitted frames and accessible open shadow roots. Known
player containers retain their identity when the video element is reused for a
preroll. Unassociated network requests never attach to the largest or first
player. Successful network responses can confirm extensionless sources; the
MAIN-world hook can additionally link fetched buffers to an MSE blob and its
actual video element. HLS playlist references retain their parent manifest.

One shared browser classifier excludes known ad hosts, explicit ad paths, and
transport resources. Player ad containers and playback state also contribute to
classification. Short duration, looping, muted audio, or autoplay alone do not
exclude legitimate videos. Ad filtering does not modify website playback.

## Automatic extraction and handoff

A visible unresolved player in the active tab receives a disabled
“Detecting video…” button. Anonymous webpage inspection runs through the existing
native extraction service. Ambiguous page-wide configuration is not assigned
to one of several unrelated players. Normal discovery and explicit, credentialed
browser capture retain separate authorization boundaries.

The extension uses short request/poll steps, a global automatic inspection queue,
and the native preview's 120-second lifetime. Failed results require explicit
retry, reconnection, or a changed document/player. Hidden tabs defer work.
The native service separately preserves the single automatic inspection limit
across extension-worker restarts; explicit native inspection retains the second
service slot. Temporary native capacity errors remain queued.

The new extension-facing endpoints are:

- POST /api/web-media/inspect
- GET /api/web-media/inspect/:id
- POST /api/web-media/download

Download intake validates the inspection ID, original URL, and stable selection
before opening the existing desktop dialog. The selected format passes through
web_media_selection; the desktop re-inspects and revalidates it before transfer.
Parsed HLS renditions retain their parent master and external audio groups;
resolution/frame-rate hints select a complete native format rather than a silent
child playlist. Unavailable or ambiguous quality hints require explicit retry.
Automatic inspection never starts a download or automatically shares cookies.
YouTube retains its dedicated provider and explicit browser-session flow.

## Execution record

- Implemented in the existing codex/webpage-video-detection checkout. Existing
  uncommitted webpage-video changes and unrelated edits were retained.
- Used fixture tests for shared classification, source parsing, queue behavior,
  actual DOM detector/overlay adapters, capture, and the background authorization
  adapter. Added desktop hook tests for selection preservation and rejection.
- Added a Rust test for native automatic-inspection ownership. Existing native
  preview tests already cover expired and mismatched selections.
- Final review found and corrected HLS parent/audio preservation, stale queued
  player eligibility, hidden overlay visibility, and tray-hidden window handoff.
- JavaScript regression checks and TypeScript checks run without application
  builds. Rust changes receive stdin-only syntax inspection; compiled Rust tests,
  packaged-browser behavior, native extraction, and end-to-end downloading remain
  unverified.
- Final verification: all 109 JavaScript tests passed, desktop and extension
  TypeScript checks passed, and all five changed Rust files passed syntax
  inspection without compilation.

Coverage depends on the page's player structure and supported extractors.
Closed shadow roots, inaccessible frames, DRM, account-required media, playlists,
and live recording retain existing limitations. URL classification cannot
reliably remove ads already stitched into a single stream.
