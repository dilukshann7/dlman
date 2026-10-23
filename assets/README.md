# DLMan artwork

`dlman-logo.png` is the original supplied artwork, including its transparency.
Keep it as the source of truth for desktop, mobile, browser extension and website
branding. Generated SVG files embed this artwork; they are not vector tracings.

The Windows ICO stores its 256×256 image first because Tauri uses the first ICO
entry for window and tray icons. All smaller sizes remain available for Windows
to select at native resolution. Keep this ordering when regenerating icons.

To regenerate the committed assets without building an application:

```powershell
python -m pip install Pillow==12.1.1
python tools/generate-branding.py
python tools/generate-branding.py --check
```

The generator creates PNG sizes, Windows ICO, macOS ICNS, mobile launcher icons,
extension icons, the About dialog logo and website favicons. iOS icons use an
opaque white background; Android adaptive foregrounds include safe-area padding.
The extension's existing copy script uses the exact matching desktop PNG sizes.
