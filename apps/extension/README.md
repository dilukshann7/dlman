# DLMan Browser Extension

<img src="public/icon/128.png" alt="DLMan" width="64" height="64">

Modern browser extension for DLMan — capture downloads directly from your browser.

This is the extension for [Dilukshan's DLMan fork](https://github.com/dilukshann7/dlman),
based on the original project by [Novin Code](https://github.com/novincode/dlman).
It uses the shared purple globe-and-arrow branding and adds public YouTube
video/Shorts quality selection and MP3 handoff. See the root README for
[original-author credit and rewrite details](../../README.md#thanks-to-the-original-creator).

## 📥 Download

Get the latest extension from [GitHub Releases](https://github.com/dilukshann7/dlman/releases/latest):

| Browser | Download |
|---------|----------|
| ![Chrome](https://img.shields.io/badge/Chrome-4285F4?logo=googlechrome&logoColor=white) | [Download for Chrome](https://github.com/dilukshann7/dlman/releases/latest) |
| ![Firefox](https://img.shields.io/badge/Firefox-FF7139?logo=firefox&logoColor=white) | [Download for Firefox](https://github.com/dilukshann7/dlman/releases/latest) |
| ![Edge](https://img.shields.io/badge/Edge-0078D7?logo=microsoftedge&logoColor=white) | Use Chrome version |
| ![Brave](https://img.shields.io/badge/Brave-FB542B?logo=brave&logoColor=white) | Use Chrome version |

> **Note:** Extension files are named `dlman-extension-chrome-vX.Y.Z.zip` and `dlman-extension-firefox-vX.Y.Z.zip`

---

## 🔧 Installation

### Chrome / Edge / Brave

1. Download the Chrome extension zip from the latest release
2. Extract the zip file to a folder
3. Open your browser and go to `chrome://extensions` (or `edge://extensions`)
4. Enable **Developer mode** (toggle in the top right)
5. Click **Load unpacked**
6. Select the extracted folder
7. Done! The DLMan icon will appear in your toolbar

### Firefox

1. Download the Firefox extension zip from the latest release
2. Extract the zip file
3. Open Firefox and go to `about:debugging#/runtime/this-firefox`
4. Click **Load Temporary Add-on**
5. Select any file from the extracted folder
6. Done! The DLMan icon will appear in your toolbar

> **Note:** Firefox temporary add-ons are removed when Firefox closes. For permanent installation, the extension would need to be signed by Mozilla.

---

## ✨ Features

- 🚀 **Download Interception** — Automatically capture downloads and send to DLMan
- 🎯 **Context Menu** — Right-click any link → "Download with DLMan"
- 📋 **Batch Downloads** — Download multiple files at once
- ⚡ **Real-time Updates** — See download progress in the popup
- 🔒 **Per-Site Control** — Enable/disable on specific sites
- 🌙 **Dark Mode** — System-aware theme support

---

## YouTube

Use the existing video button to select a video quality or **Audio only · MP3**.
The desktop app opens its normal destination dialog. Download All presents each
video quality for approval without including MP3. You can also paste a YouTube
link directly into the desktop Add Download dialog. Requires the matching desktop
version with bundled media tools. Single public videos and Shorts only; playlists,
live streams, DRM, and account-required videos are unsupported. See
[YouTube support](../../docs/youtube-support.md) for packaging and verification.

## 🔗 How It Works

The extension connects to the DLMan desktop app running on your computer.

**Requirements:**
- DLMan desktop app must be running
- Default integration port: `7899` (configurable in DLMan Settings → Extensions)

When you click a download link or right-click → "Download with DLMan", the extension sends the URL to DLMan, which handles the download with:
- Multi-segment parallel downloading
- Pause and resume capability
- Queue management
- Speed limiting

---

## ⚙️ Settings

Click the extension icon → Settings to configure:

- **Auto-intercept** — Capture downloads automatically
- **File patterns** — Which file types to intercept
- **Disabled sites** — Sites where DLMan won't intercept

---

## 🔍 Troubleshooting

### Extension not connecting?

1. Make sure DLMan desktop app is running
2. Check the connection port in DLMan Settings → Extensions
3. Look for the extension icon — a red badge means connection failed
4. Click the extension icon to see detailed connection status

### Downloads not being intercepted?

1. Verify the extension shows "Connected" when you click it
2. Check that "Auto-intercept downloads" is enabled
3. Verify the file type matches your intercept patterns
4. Make sure the website isn't in your disabled sites list

---

## 🛠️ Development

```bash
# Install dependencies
pnpm install

# Development mode
pnpm --filter @dlman/extension dev          # Chrome
pnpm --filter @dlman/extension dev:firefox  # Firefox

# Build
pnpm --filter @dlman/extension build:chrome   # Chrome
pnpm --filter @dlman/extension build:firefox  # Firefox
```

### Building from Source

```bash
# Clone the repository
git clone https://github.com/dilukshann7/dlman.git
cd dlman

# Install dependencies
pnpm install

# Build extensions
pnpm --filter @dlman/extension build:chrome
pnpm --filter @dlman/extension build:firefox
```

Built extensions will be in `apps/extension/dist/`.

---

## 📜 License

MIT — Same as the main DLMan project.
