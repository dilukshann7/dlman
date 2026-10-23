# IDM features missing or incomplete in DLMan

Reviewed: 23–24 October 2026. Scope: the current working tree, including existing uncommitted changes, at base commit `df99e13e23086d8ece061511b2ce807e65f2def7`.

## Evidence and boundaries

- DLMan: read the transfer engine, types, scheduler, persistence, CLI, desktop commands/UI, browser extension, and relevant shared code. Source and control-flow observations establish implementation coverage, not successful execution.
- Installed IDM: `C:\Program Files (x86)\Internet Download Manager\IDMan.exe`, file/product version `6, 42, 61, 3`, x86 PE32, 6,043,648 bytes. SHA-256: `c7c80b420579d774f445b6b3f9282c0850760ef790373c0ece0f856a0a05d1d5`.
- REA resource evidence: `ev_36ca9a1242174b18659b1df9cdf2f1b7ab5b17af04117ac7e524365445e02e20`. Complete bounded resource inventory: 436 payloads, 887 examined directory entries. Retained locally in [PE evidence](2026-10-23-pe-evidence.json).
- Decoded selected menu, dialog, and string-table payloads at the recorded file offsets. These establish shipped UI labels and explanatory text; they do not recover IDM algorithms or prove execution.
- Current official IDM documentation supplies documented behavior. It may describe a newer release than the installed executable. Installed resources and current documentation are separate evidence sources.
- No build, dev server, tests, application launch, browser session capture, live downloads, or performance benchmark was run. No implementation files were changed.
- “Missing” below means no implementation found in the audited source paths and entry points. It is a bounded negative finding, not proof about every possible dependency or external helper.

## Main feature gaps

| IDM feature | DLMan status | Evidence and practical difference |
| --- | --- | --- |
| Refresh an expired download address and resume the existing job | Missing generic workflow | IDM documents opening the original page and capturing a replacement URL into the waiting job [W1]. DLMan's `apps/desktop/src-tauri/src/commands.rs:330` update contract accepts speed limit, category, and destination, not a replacement URL. `crates/dlman-core/src/lib.rs:502` retries the existing address. YouTube metadata reextraction is a separate, present workflow. |
| Dynamic file segmentation | Partial | IDM documents splitting the largest remaining segment when connections become available [W2]. DLMan calculates equal initial ranges in `crates/dlman-core/src/engine/download_task.rs:816`, then starts workers for those ranges at line 488. No ongoing range splitting or rebalancing was found. Parallel downloading itself already exists. This does not establish a speed difference on a given connection. |
| Site Grabber: recursive crawling, filters, saved projects, offline website mirroring | Missing | IDM documents recursive website projects and offline mirroring [W3]. DLMan collects current-page anchors in `apps/extension/src/entrypoints/content.ts:72` and handles the result in `background.ts:844`; no recursive traversal, asset-link rewriting, or saved crawl projects were found. “Download all links” already exists. |
| Synchronization queues | Missing | IDM can periodically check queued files for changes and replace local copies [W4]. DLMan's `Queue` at `crates/dlman-types/src/lib.rs:153` has no synchronization kind, and its scheduler starts/stops ordinary queues. Scheduled downloads already exist. |
| Data quotas over a time window | Missing | IDM documents budgets such as 40 MB/hour or 150 MB per 4 hours, with automatic waiting/resumption [W4]. DLMan has speed limits, but no transferred-byte budget/window in `Settings`, `Queue`, or `Schedule` (`crates/dlman-types/src/lib.rs:153`, `:203`, `:297`). |
| FTP and MMS transfer protocols | Missing | IDM lists HTTP, HTTPS, FTP, and MMS [W5]. DLMan's normal transfer uses reqwest HTTP requests (`crates/dlman-core/src/engine/segment_worker.rs:188`), and desktop URL intake accepts only HTTP/HTTPS (`apps/desktop/src/lib/url-intake.ts:17`). The `ftp` credential comment is not a transfer implementation. |
| NTLM, Kerberos, and Negotiate authentication | Missing | IDM lists these authentication schemes [W5]. DLMan applies HTTP Basic and cookies (`crates/dlman-core/src/engine/segment_worker.rs:208`); no handlers for the listed enterprise schemes were found. Browser cookies and site logins should not be listed as wholly missing. |
| Advanced proxy integration | Partial, with a concrete wiring gap | IDM documents per-protocol proxies, SOCKS, configuration scripts, and using a browser's proxy [W6]. DLMan has manual HTTP/HTTPS proxy UI and client-building code, but ordinary manager initialization passes `None` (`crates/dlman-core/src/lib.rs:98`, `engine/manager.rs:123`); saving settings never calls `update_proxy` (`lib.rs:890`). YouTube separately receives saved proxy settings (`media/youtube/transfer.rs:47`, `:81`). No explicit SOCKS feature or PAC/browser-proxy handoff was found. Environment-driven proxy behavior may still apply. |
| Generate numbered batch URLs | Missing | Installed IDM dialog resource 280 describes generating sequential filenames through an asterisk pattern, e.g. `img001.jpg` through `img100.jpg` (text file offset 3,619,386; REA resource evidence above). DLMan's batch dialog parses literal URLs (`apps/desktop/src/components/dialogs/BatchImportDialog.tsx:140`, `apps/desktop/src/lib/utils.ts:78`). Existing batch import is not a range generator. |
| Run an antivirus scanner after each downloaded file | Missing dedicated integration | IDM documents scanner executable/arguments and a downloaded-file placeholder [W6]. No per-file scan configuration or completion hook was found in DLMan. Its queue-level custom command is present (`apps/desktop/src/lib/events.ts:409`, `apps/desktop/src-tauri/src/commands.rs:704`) and differs in timing and per-file context. |
| Per-server connection-count exceptions | Missing rules | IDM exposes server-specific connection counts [W6]. DLMan has global and queue segment counts (`crates/dlman-types/src/lib.rs:160`, `:297`), but no host-to-connection rule table was found. |
| Preview ZIP contents before downloading | Missing | IDM documents ZIP preview from the download information dialog [W7]. No remote archive preview path was found in the core or desktop source. |
| Floating desktop drop target | Missing | IDM documents a separate drop window [W7]. DLMan has main-window drag/drop (`apps/desktop/src/components/DropZoneOverlay.tsx:34`); its window manager focuses the main window (`apps/desktop/src-tauri/src/window_manager.rs:21`). No floating basket was found. |
| Customize toolbar buttons and download-list columns | Missing | IDM documents toolbar order/styles and column visibility/order/width [W8]. DLMan renders download item rows (`apps/desktop/src/components/downloads/DownloadList.tsx:398`) and a fixed menu/toolbar; no equivalent customization controls were found. Themes already exist. |
| Monitor system clipboard automatically | Missing background monitoring | IDM documents detecting copied download URLs [W7]. DLMan supports paste events and explicit clipboard actions (`apps/desktop/src/App.tsx:203`, `hooks/useKeyboardShortcuts.tsx:107`), but no system-wide clipboard-change listener was found in the desktop Rust or frontend paths. |
| Capture downloads from applications outside supported browser extensions | Missing native interception layer | IDM documents advanced integration beyond its normal browser extension [W5, W6]. DLMan exposes browser-extension HTTP/WebSocket handoff and deep-link entry points (`apps/desktop/src-tauri/src/browser_server.rs`, `lib.rs`), with no OS-level application/network interception implementation found. This is a source-architecture comparison, not a claim that IDM captures every possible application or protocol. |

## Existing features with incomplete coverage

### Generic DASH transfer wiring

DASH detection and parsing exist, but the normal desktop transfer path is incomplete:

1. `apps/desktop/src/components/dialogs/NewDownloadDialog.tsx:429` forwards `protocol: 'dash'` to `start_media_download`.
2. `apps/desktop/src-tauri/src/commands.rs:296` accepts both `hls` and `dash`, then calls `download_hls_stream` without forwarding the protocol.
3. `crates/dlman-core/src/lib.rs:968` constructs `DetectedMedia` with `MediaProtocol::Hls`.
4. The resolver dispatches by that enum (`crates/dlman-core/src/media/mod.rs:106`).

Observed control flow therefore routes generic DASH through the HLS parser. This is a DLMan implementation gap identified during comparison; exact IDM format/site coverage was not runtime-tested. YouTube has an independent managed yt-dlp path and is not covered by this finding.

### HLS support

Direct video and an HLS transfer path exist. `crates/dlman-core/src/media/hls.rs:12` explicitly excludes AES-128 encrypted segments, byte-range playlists, and live streams. These are DLMan limits, not individually verified IDM advantages. No DRM-support claim is made for either product.

### Import/export

Import/export is present, not wholly missing:

- Core/CLI JSON export/import: `crates/dlman-core/src/lib.rs:1551`, `:1568`; `apps/cli/src/commands.rs:400`.
- Desktop export includes download summaries and queue definitions (`apps/desktop/src/components/layout/MenuBar.tsx:158`).
- Desktop import only re-adds URLs/destinations/queue IDs (`MenuBar.tsx:202`), ignoring exported queue definitions and saved job state. This is not a complete round trip.
- No IDM export-format interoperability was found. Installed IDM menu resource 129 includes export/import actions at file offsets 3,655,284 and 3,655,358. Moving unfinished transfers is a distinct operation from exporting links; IDM's migration instructions also require temporary files and settings [W9].

### Retry and resume completeness

Ordinary byte-range resume exists. Its remaining limitations matter when assessing recovery parity:

- `crates/dlman-core/src/engine/persistence.rs:248` always binds `None` for ETag and Last-Modified, despite corresponding database columns. No active `If-Range` implementation was found. Do not assume changing URLs or remote files is validated safely.
- Multi-segment transfers have retry count/delay logic (`engine/download_task.rs:540`), while the single-segment path invokes its worker once (`:426`).
- The multi-segment retry delay retains the active download status (`:579`); queue slot counting includes it (`crates/dlman-core/src/queue.rs:218`). IDM documents moving failed jobs to the end of a queue before retrying [W4].

These are source findings. No live corruption, retry, crash, or recovery scenario was executed.

## Already present; do not count as blanket gaps

- Parallel HTTP/HTTPS downloads, persisted partial ranges, pause/resume, download history, and crash-state restoration.
- Multiple queues, order controls, concurrency limits, and per-download/per-queue speed controls.
- Weekday/time scheduling and queue completion actions, including sleep, shutdown, hibernate, and a custom command.
- Categories, destination folders, literal URL batch import, current-page link collection, and main-window drag/drop.
- Browser capture extensions, session cookies for supported single-download paths, and saved Basic-auth site credentials.
- Direct media detection, limited HLS transfer, YouTube/Shorts selection and managed video/MP3 transfers.
- JSON import/export, notifications, themes, translations, update checks, and a CLI.

These are observed source capabilities; this audit does not certify successful native, browser, or cross-platform execution.

## Investigation checklist and remaining unknowns

| Question | Status |
| --- | --- |
| Identify the actual installed IDM and current DLMan checkout | Answered: exact artifact identity and working-tree boundary recorded. |
| Separate absent features from existing or partly connected features | Answered within the audited source paths. |
| Establish IDM's public feature behavior without guessing its internals | Answered through official documentation and shipped resource text. |
| Recover or validate IDM's proprietary download algorithm | Not attempted; documentation is not decompilation or runtime evidence. |
| Compare actual speed, reliability, website compatibility, and media-format coverage | Unresolved; requires separate controlled runtime comparisons. |
| Establish released DLMan binary behavior | Unresolved; the audit describes current source, including uncommitted changes. |

The highest-impact feature gaps are expired-address refresh, dynamic range rebalancing, and Site Grabber. The proxy and DASH wiring findings are existing implementation problems worth resolving before treating those capabilities as complete. This prioritization is analyst judgment, not a benchmark result.

## Official IDM sources

- **W1:** [Refreshing an expired download address](https://www.internetdownloadmanager.com/register/new_faq/sites2_3.html).
- **W2:** [Dynamic segmentation and connection reuse](https://www.internetdownloadmanager.com/support/segmentation.html).
- **W3:** [Site Grabber introduction](https://www.internetdownloadmanager.com/support/idm-grabber/idm_grabber.html).
- **W4:** [Scheduler, synchronization queues, retries, and quotas](https://www.internetdownloadmanager.com/support/idm-scheduler/idm_scheduler.html).
- **W5:** [Features: protocols, authentication, and advanced integration](https://www.internetdownloadmanager.com/features2.html).
- **W6:** [Options: connection exceptions, proxy/SOCKS, antivirus, site logins](https://www.internetdownloadmanager.com/support/using_idm/options.html).
- **W7:** [Starting downloads: ZIP preview, clipboard monitoring, drop target](https://www.internetdownloadmanager.com/support/using_idm/starting.html).
- **W8:** [Main window and toolbar/column customization](https://www.internetdownloadmanager.com/support/using_idm/using_idm.html).
- **W9:** [Moving settings and unfinished downloads to another system](https://register.internetdownloadmanager.com/register/new_faq/functions17.html).
