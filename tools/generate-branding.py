"""Generate app, extension and website icons from the supplied DLMan artwork.

Requires Pillow: python -m pip install Pillow
Run from any directory: python tools/generate-branding.py [--check]
This only resizes/packages artwork; it never builds or starts an application.
"""

import argparse
import base64
from io import BytesIO
from pathlib import Path
import re

from PIL import Image, ImageOps


ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "assets/dlman-logo.png"
ICONS = ROOT / "apps/desktop/src-tauri/icons"


def generate() -> dict[Path, bytes]:
    with Image.open(SOURCE) as original:
        source = ImageOps.pad(original.convert("RGBA"), (max(original.size),) * 2,
                              method=Image.Resampling.LANCZOS, color=(0, 0, 0, 0))
    outputs: dict[Path, bytes] = {}

    def png(size: int, *, opaque: bool = False, foreground: bool = False) -> bytes:
        image = source.resize((size, size), Image.Resampling.LANCZOS)
        if foreground:
            # Keep adaptive-icon artwork inside Android's central safe area.
            inner = round(size * 66 / 108)
            image = Image.new("RGBA", (size, size))
            image.paste(source.resize((inner, inner), Image.Resampling.LANCZOS),
                        ((size - inner) // 2,) * 2)
        if opaque:
            background = Image.new("RGBA", image.size, "white")
            background.alpha_composite(image)
            image = background.convert("RGB")
        buffer = BytesIO()
        image.save(buffer, format="PNG", optimize=True)
        return buffer.getvalue()

    desktop = {"icon.png": 512, "128x128@2x.png": 256, "StoreLogo.png": 50}
    desktop.update({f"{size}x{size}.png": size for size in (16, 32, 48, 64, 128)})
    desktop.update({f"Square{size}x{size}Logo.png": size
                    for size in (30, 44, 71, 89, 107, 142, 150, 284, 310)})
    for name, size in desktop.items():
        outputs[ICONS / name] = png(size)

    # Preserve all existing iOS naming variants, including duplicate scale slots.
    for path in sorted((ICONS / "ios").glob("*.png")):
        match = re.fullmatch(r"AppIcon-([\d.]+)(?:x[\d.]+)?@(\d+)x(?:-1)?\.png", path.name)
        if not match:
            raise ValueError(f"Unknown iOS icon size: {path.name}")
        outputs[path] = png(round(float(match[1]) * int(match[2])), opaque=True)

    for density, size in {"mdpi": 48, "hdpi": 72, "xhdpi": 96,
                          "xxhdpi": 144, "xxxhdpi": 192}.items():
        folder = ICONS / "android" / f"mipmap-{density}"
        for name in ("ic_launcher.png", "ic_launcher_round.png"):
            outputs[folder / name] = png(size)
        outputs[folder / "ic_launcher_foreground.png"] = png(round(size * 108 / 48), foreground=True)

    for name, format_name, size in (("icon.ico", "ICO", 256), ("icon.icns", "ICNS", 1024)):
        buffer = BytesIO()
        source.resize((size, size), Image.Resampling.LANCZOS).save(buffer, format=format_name)
        outputs[ICONS / name] = buffer.getvalue()

    def svg(size: int) -> bytes:
        encoded = base64.b64encode(png(size)).decode("ascii")
        return (f'<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" '
                f'viewBox="0 0 {size} {size}"><image width="{size}" height="{size}" '
                f'href="data:image/png;base64,{encoded}"/></svg>\n').encode("utf-8")

    for size in (16, 32, 48, 128):
        folder = ROOT / "apps/extension/public/icon"
        outputs[folder / f"{size}.png"] = png(size)
        outputs[folder / f"{size}.svg"] = svg(size)

    outputs[ROOT / "apps/desktop/public/logo.png"] = png(256)
    for name, size in {"logo.png": 256, "icon.png": 512, "icon-64.png": 64, "favicon.png": 32}.items():
        outputs[ROOT / "apps/next/public" / name] = png(size)
    outputs[ROOT / "apps/next/public/favicon.svg"] = svg(256)
    return outputs


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Check generated assets without changing files")
    args = parser.parse_args()
    outputs = generate()
    stale = []
    for path, contents in outputs.items():
        if args.check:
            existing = path.read_bytes() if path.exists() else None
            if existing is not None and path.suffix == ".svg":
                existing = existing.replace(b"\r\n", b"\n")
            if existing != contents:
                stale.append(str(path.relative_to(ROOT)))
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(contents)
    if stale:
        parser.exit(1, "Stale or missing branding assets:\n" + "\n".join(stale) + "\n")
    print(f"{'Verified' if args.check else 'Generated'} {len(outputs)} branding assets from {SOURCE.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
