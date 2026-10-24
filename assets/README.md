# DLMan artwork

`dlman-logo.png` is the original supplied artwork, including its transparency.
Its alpha channel is the source of truth for the logo silhouette. The generator
renders that silhouette in solid white for desktop, mobile, browser extension
and website branding, preserving antialiased edges and transparent cutouts.
Generated SVG files embed this artwork; they are not vector tracings.

The Windows ICO stores its 256×256 image first because Tauri uses the first ICO
entry for window and tray icons. All smaller sizes remain available for Windows
to select at native resolution. Keep this ordering when regenerating icons.
The ICO includes native 16, 20, 24, 28, 32, 40, 48, 64, 96, 128 and 256 px frames.
Windows title bars and taskbars each receive an icon matched to their system
metrics and the window's DPI. The application refreshes both on monitor changes.
The Windows notification-area icon uses Explorer's taskbar DPI instead of the
main window's scale. Rust owns its lifecycle so webview reloads cannot duplicate it.

To regenerate the committed assets without building an application:

```powershell
python -m pip install Pillow==12.1.1
python tools/generate-branding.py
python tools/generate-branding.py --check
```

The generator creates PNG sizes, Windows ICO, macOS ICNS, mobile launcher icons,
extension icons, the About dialog logo and website favicons. iOS icons use an
opaque dark background so the white mark remains visible; Android adaptive foregrounds include safe-area padding.
The extension's existing copy script uses the exact matching desktop PNG sizes.
