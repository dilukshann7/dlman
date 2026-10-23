# Bundled media tool sources and notices

The executables are separate programs launched by Rusty. Exact versions,
download URLs, executable hashes and notice hashes are in the packaged
`tools/media_tools_lock.json`.

- yt-dlp 2026.08.19: <https://github.com/yt-dlp/yt-dlp/tree/2026.08.19>.
  `yt-dlp-LICENSE` and `yt-dlp-THIRD_PARTY_LICENSES.txt` are copied from that
  tag. The combined Windows executable includes GPLv3+ components. Its aggregate
  notice lists component sources and the upstream source-request contact.
- Deno v2.9.7: <https://github.com/denoland/deno/tree/v2.9.7>.
  `deno-LICENSE.md`, `deno-node-LICENSE`, `deno-undici-LICENSE` and
  `deno-webgpu-LICENSE.md` are copied respectively from `LICENSE.md`,
  `cli/tsc/dts/node/LICENSE`, `cli/tsc/dts/node/undici/LICENSE` and
  `ext/webgpu/LICENSE.md` at that tag. Its source dependency versions are
  recorded in the upstream Cargo.lock and submodule revisions.
- FFmpeg/ffprobe: the exact build is linked in the lock file from
  <https://github.com/yt-dlp/FFmpeg-Builds/releases>. `ffmpeg-build-NOTICE.txt`
  preserves the build project's README and upstream build/source links;
  `ffmpeg-COPYING.GPLv3` preserves the GPL version 3 text.

Keep these notices with both installer and portable distributions. A change to
a tool version requires updating its binaries, hashes and matching notices.


## DLMan platform additions

- macOS x64/arm64 FFmpeg and ffprobe 9.0.2 archives are pinned to Martin
  Riedl's versioned build directories in `tools/media-tools.lock.json`.
  Build provenance, configure flags, library versions and GPL source/build
  information: https://ffmpeg.martin-riedl.de/ and
  https://git.martin-riedl.de/ffmpeg/build-script . Each versioned directory
  also publishes `versions.txt`, `codecs.txt`, and SHA-256 checksum files.
- Linux FFmpeg/ffprobe uses the pinned GPL static yt-dlp FFmpeg build
  N-127617-g94cba3150c. Corresponding FFmpeg source:
  https://github.com/FFmpeg/FFmpeg/tree/94cba3150c .
- macOS/Linux yt-dlp standalone release 2026.08.19 includes its EJS components;
  Deno 2.9.7 uses its matching architecture release. The license texts and
  third-party notices above apply to their corresponding tool versions.

These external programs are distributed as separate executable resources.
DLMan's own source license remains unchanged.
