<p align="center">
  <img src="apps/desktop/src-tauri/icons/128x128@2x.png" alt="DLMan" width="80" height="80">
</p>

<h1 align="center">DLMan</h1>

<p align="center">
  <strong>A free, open-source download manager, continued by Dilukshan.</strong><br>
  Parallel downloads • YouTube video and MP3 • Desktop and browser integration
</p>

<p align="center">
  <a href="https://github.com/dilukshann7/dlman/releases">
    <img src="https://img.shields.io/github/v/release/dilukshann7/dlman?style=flat-square" alt="Release">
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/License-MIT-blue.svg?style=flat-square" alt="License">
  </a>
  <a href="https://github.com/dilukshann7/dlman/stargazers">
    <img src="https://img.shields.io/github/stars/dilukshann7/dlman?style=flat-square" alt="Stars">
  </a>
  <a href="https://github.com/dilukshann7/dlman/issues">
    <img src="https://img.shields.io/github/issues/dilukshann7/dlman?style=flat-square" alt="Issues">
  </a>
</p>

<p align="center">
  <a href="https://github.com/dilukshann7/dlman/releases"><strong>📥 Download</strong></a> •
  <a href="docs/INSTALL.md"><strong>📖 Install Guide</strong></a> •
  <a href="CHANGELOG.md"><strong>📋 Changelog</strong></a> •
  <a href="CONTRIBUTING.md"><strong>🤝 Contributing</strong></a>
</p>

---

## Thanks to the original creator

Thank you to **[Novin Code (@novincode)](https://github.com/novincode)** for creating
the original **[DLMan](https://github.com/novincode/dlman)** and sharing it under
the MIT license. The original creator and upstream contributors built the
Rust/Tauri download manager that this work is based on. Their copyright and
license notices are retained in [LICENSE](LICENSE).

## Our rewrite

This repository is **[Dilukshan's DLMan fork](https://github.com/dilukshann7/dlman)**.
Our rewrite builds on the original Rust backend, React interface, Tauri desktop
shell, and browser extension. It adds the following changes while retaining the
existing download manager foundation:

- **YouTube videos and Shorts:** inspect public individual videos and select a
  quality from the desktop Add Download dialog or browser video button.
- **MP3 downloads:** choose audio-only conversion alongside video qualities.
- **Managed transfers:** YouTube jobs use the existing download list, queues,
  destinations, speed limits, and pause/resume controls. Partial data is retained
  and final output is verified before publication.
- **Bundled media tools:** checksum-pinned yt-dlp, Deno, FFmpeg, and ffprobe are
  prepared for the four desktop release targets, with third-party notices.
- **New branding:** the supplied purple globe-and-arrow logo is used throughout
  the app, extension, native icon sets, and website assets. See [artwork](assets/README.md).

See [YouTube support](docs/youtube-support.md) for supported URLs, limitations,
tool preparation, and verification details. The current source has offline
fixtures and TypeScript checks; native compilation, live YouTube downloads, and
all-platform package acceptance remain unverified.

DLMan targets Windows, macOS, and Linux. Links in this README point to this fork;
upstream releases do not include these changes.

---

## Features

### ⚡ Speed & Performance
- Multi-segment parallel downloads (1–32 segments per file)
- Cached CDN URLs to avoid redirect overhead on resume
- Real-time bandwidth monitoring
- Per-download and per-queue speed limits

### 🔄 Reliability
- Pause and resume anytime
- Auto-resume after crashes or restarts
- Automatic retry for failed downloads
- SQLite-backed persistence (no half-broken files)

### ⏱️ Scheduling & Automation
- Schedule queues to start/stop at specific times
- Select active days of the week
- Post-download actions: Sleep, Shutdown, Hibernate
- Run custom commands after downloads complete
- Queue auto-advance: next downloads start automatically when slots free up

### 🗂️ Organization
- Queue management with priorities
- Categories with custom download folders
- Batch import multiple URLs
- Drag and drop support
- Quick "Clear Filters" button when any filter is active

### 🔐 Site Logins & Credentials
- Save site login credentials per domain
- Auto-apply credentials for authenticated downloads (HTTP Basic Auth)
- Auth detection during URL probe — warning shown before download starts
- Credential prompt on 401/403 failures with retry
- Manage saved logins in Settings

### YouTube Videos and MP3
- Public videos and Shorts with quality selection and merged audio/video
- MP3 conversion through the existing browser video button and Add Download dialog
- Bundled extraction tools for all desktop release targets; see [YouTube support](docs/youtube-support.md)

### 🎨 Interface
- Modern, clean UI
- Dark and light themes
- Real-time segment visualization
- Desktop notifications
- macOS dock badge for active downloads
- In-app update notifications

### 🧰 Ecosystem
- CLI tool for scripts and automation
- Browser extensions for Chrome, Firefox, Edge
- 100% free and open source

---

## Download

Use this fork's [Releases](https://github.com/dilukshann7/dlman/releases) page for
published packages. The formats below describe the release targets; a package
is available only after a corresponding release has been published.

<p align="center">
  <a href="https://github.com/dilukshann7/dlman/releases/latest">
    <img src="https://img.shields.io/badge/Windows-0078D6?style=for-the-badge&logo=windows&logoColor=white" alt="Windows">
  </a>
  <a href="https://github.com/dilukshann7/dlman/releases/latest">
    <img src="https://img.shields.io/badge/macOS-000000?style=for-the-badge&logo=apple&logoColor=white" alt="macOS">
  </a>
  <a href="https://github.com/dilukshann7/dlman/releases/latest">
    <img src="https://img.shields.io/badge/Linux-FCC624?style=for-the-badge&logo=linux&logoColor=black" alt="Linux">
  </a>
</p>

| Platform | Downloads |
|----------|-----------|
| **Windows** | `.msi`, `.exe` |
| **macOS (Intel)** | `.dmg` (x64) |
| **macOS (Apple Silicon)** | `.dmg` (aarch64) |
| **Linux** | `.deb`, `.rpm`, `.AppImage` |

👉 [View all downloads](https://github.com/dilukshann7/dlman/releases/latest)

For platform-specific instructions, see the [Install Guide](docs/INSTALL.md).

---

## Browser Extension

Capture downloads directly from your browser.

| Browser | Download |
|---------|----------|
| **Chrome / Edge / Brave** | [📦 Download Extension](https://github.com/dilukshann7/dlman/releases/latest) |
| **Firefox** | [📦 Download Extension](https://github.com/dilukshann7/dlman/releases/latest) |

Extension files are named `dlman-extension-chrome-vX.Y.Z.zip` and `dlman-extension-firefox-vX.Y.Z.zip`.

For installation instructions and features, see the [Extension README](apps/extension/README.md).

---

## CLI

DLMan includes a command-line interface for automation and scripting:

```bash
# Basic download
dlman https://example.com/file.zip

# Custom output folder and 8 segments
dlman https://example.com/file.zip -o ~/Downloads -s 8

# See all options
dlman --help
```

See the [CLI documentation](docs/CLI.md) for more.

---

## Architecture

DLMan is built with modern technologies:

| Component | Technology |
|-----------|------------|
| **Backend** | Rust, Tauri v2 |
| **Frontend** | React, TypeScript, Tailwind CSS |
| **Database** | SQLite (sqlx) |
| **Downloads** | tokio, reqwest |

For technical details, see [Architecture](docs/ARCHITECTURE.md) and [Core Engine](docs/CORE.md).

---

## Contributing

Contributions are welcome! Whether it's bug fixes, features, or documentation:

1. Read the [Contributing Guide](CONTRIBUTING.md)
2. Check the [Development Guide](docs/DEVELOPMENT.md)
3. Look at [open issues](https://github.com/dilukshann7/dlman/issues)

### Quick Start

```bash
git clone https://github.com/dilukshann7/dlman.git
cd dlman
pnpm install
pnpm dev
```

---

## Support the original creator

The donation links below support **Novin Code**, the original DLMan creator.
For this fork, you can [report issues](https://github.com/dilukshann7/dlman/issues),
contribute changes, or star [the repository](https://github.com/dilukshann7/dlman).

<p align="center">
  <a href="https://github.com/sponsors/novincode">
    <img src="https://img.shields.io/badge/Sponsor-GitHub-EA4AAA?style=for-the-badge&logo=github-sponsors&logoColor=white" alt="GitHub Sponsors">
  </a>
  <a href="https://buymeacoffee.com/codeideal">
    <img src="https://img.shields.io/badge/Buy%20Me%20a%20Coffee-FFDD00?style=for-the-badge&logo=buy-me-a-coffee&logoColor=black" alt="Buy Me a Coffee">
  </a>
</p>

<p align="center">
  <a href="https://github.com/sponsors/novincode">
    <img src="docs/become-supporter.png" alt="Become a Supporter" width="256">
  </a>
</p>

Support the original project financially or share it with others.

- GitHub Sponsors: https://github.com/sponsors/novincode
- Buy Me a Coffee: https://buymeacoffee.com/codeideal

#### Upstream supporters

| Supporter | How they helped |
|-----------|-----------------|
| @tom_doerr | Shared DLMan on X/Twitter: https://x.com/tom_doerr/status/2070526815467143459 |

---

## Documentation

| Document | Description |
|----------|-------------|
| [Install Guide](docs/INSTALL.md) | Platform-specific installation |
| [CLI](docs/CLI.md) | Command-line interface |
| [Architecture](docs/ARCHITECTURE.md) | System design |
| [Core Engine](docs/CORE.md) | Download engine internals |
| [Extension](docs/EXTENSION.md) | Browser extension development |
| [Development](docs/DEVELOPMENT.md) | Setting up for development |
| [Release](docs/RELEASE.md) | Release process |
| [YouTube support](docs/youtube-support.md) | Video/MP3 behavior, bundled tools, and verification limits |
| [Artwork](assets/README.md) | Shared logo source and icon regeneration |

---

<p align="center">
  MIT License · Original project © <a href="https://github.com/novincode">Novin Code</a>
</p>

<p align="center">
  <sub>Original DLMan by Novin Code and upstream contributors. Rewrite and continued development by <a href="https://github.com/dilukshann7">Dilukshan</a> and <a href="https://github.com/dilukshann7/dlman/graphs/contributors">contributors</a>.</sub>
</p>
