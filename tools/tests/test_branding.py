"""Check the icon package consumed by Tauri without building the app."""

import importlib.util
from io import BytesIO
from pathlib import Path
import struct
import unittest

from PIL import Image


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("branding", ROOT / "tools/generate-branding.py")
branding = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(branding)


class WindowsIconTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.icon = branding.generate()[branding.ICONS / "icon.ico"]

    def test_tauri_first_entry_uses_the_largest_available_image(self):
        # Tauri's CachedIcon::new_ico decodes entries()[0] as its window icon.
        # Pillow's default ascending order would enlarge a 16 px image on Windows.
        width, height = self.icon[6], self.icon[7]
        self.assertEqual((width or 256, height or 256), (256, 256))

    def test_windows_can_still_decode_every_native_icon_size(self):
        with Image.open(BytesIO(self.icon)) as image:
            self.assertEqual(image.ico.sizes(), {
                (16, 16), (24, 24), (32, 32), (48, 48),
                (64, 64), (128, 128), (256, 256),
            })
            for size in image.ico.sizes():
                frame = image.ico.getimage(size)
                self.assertEqual(frame.size, size)
                self.assertEqual(frame.mode, "RGBA")
                self.assertIsNotNone(frame.getbbox())

    def test_directory_offsets_still_point_to_complete_image_payloads(self):
        reserved, icon_type, count = struct.unpack_from("<HHH", self.icon)
        self.assertEqual((reserved, icon_type, count), (0, 1, 7))
        for index in range(count):
            length, offset = struct.unpack_from("<II", self.icon, 6 + index * 16 + 8)
            self.assertGreaterEqual(offset, 6 + count * 16)
            self.assertEqual(len(self.icon[offset:offset + length]), length)


if __name__ == "__main__":
    unittest.main()
