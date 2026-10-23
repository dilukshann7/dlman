# YouTube downloads

DLMan supports individual public YouTube videos and Shorts through the existing
browser video button, context menu, and desktop Add Download dialog. Select a
video quality or **Audio only · MP3**, choose the normal destination and queue,
then download now or choose Start Later. A watch link with playlist parameters
downloads only the current video. Playlist-only links, live streams, DRM, and
videos requiring an account are unsupported. Browser cookies are never sent to
the YouTube tools.

Video choices include audio and are merged without video transcoding. MP4 is
preferred; WebM/MKV qualities remain available. MP3 converts the best available
audio using FFmpeg VBR quality 0. MP3 file size is unknown until conversion ends.
Download All includes video qualities, not the separate MP3 conversion choice.

YouTube jobs share DLMan's download list, queues, destination and category
controls, speed limits, pause/resume, Retry, and cancel. Pause stops the process
tree and retains partial data. Resume re-extracts fresh URLs for the same format.
If that format disappears, the job fails explicitly instead of changing quality.
Transfer, conversion, verification and publication all occupy the queue slot.
The completed file is published only after ffprobe verifies its streams.

Partial files use the configured temp-storage policy. Their chosen UUID-specific
directory is persisted in the job's `youtube.stage_dir` metadata, including in
data exports. Changing the destination requires pausing first; the retained
stage stays in place and the completed file is copied to the new destination.
Removing an incomplete history entry retains its stage for manual recovery.
Completed output files are deleted only when explicitly requested.

## Preparing packages

Run from the repository root before a requested desktop package build:

```powershell
python tools/fetch-media-tools.py --platform windows-x64
```

Other supported values are `linux-x64`, `macos-x64`, and `macos-arm64`. The script
does not build or execute tools. It downloads only versioned archives recorded
in `tools/media-tools.lock.json`, verifies SHA-256, extracts the four executables,
checks their architectures and sets Unix execute permissions. Resources and
license notices are copied under the Tauri resource directory. Binary files are
ignored by Git. The existing release matrix prepares its matching tools before
building, with media resources required for release jobs.

The core build script checks the prepared manifest against the pinned source
lock and target triple, then embeds the extracted executable hashes. At runtime
DLMan resolves the Tauri resource directory and verifies every tool against those
embedded hashes. It never trusts a writable runtime checksum manifest. Source
builds without prepared tools still support ordinary downloads and report a
YouTube-specific missing-tools error. No runtime installation, Python dependency,
PATH lookup, tool self-update, external yt-dlp plugins or remote EJS components
are used.

Tool updates are a reviewed source change: update the versioned URLs and archive
hashes, refresh license notices, rerun preparation, and package only when asked.
Signed packaging that modifies executable bytes must regenerate the embedded
hashes from the final signed tools before compiling the host.

## Verification boundary

Offline JavaScript and package fixtures can be run without building the app:

```powershell
node --test shared/tests/*.test.mjs
python -m unittest discover -s tools/tests -v
```

Rust tests cover provider metadata, selected output verification, process
containment and persistence. Running them compiles Rust and requires a separate
request under this workspace's instructions. Live YouTube extraction, actual
merging/conversion, installed browser handoff, and acceptance on each desktop
platform remain separate runtime checks.
