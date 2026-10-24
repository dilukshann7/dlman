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
                (16, 16), (20, 20), (24, 24), (28, 28), (32, 32), (40, 40),
                (48, 48), (64, 64), (96, 96), (128, 128), (256, 256),
            })
            for size in image.ico.sizes():
                frame = image.ico.getimage(size)
                self.assertEqual(frame.size, size)
                self.assertEqual(frame.mode, "RGBA")
                self.assertIsNotNone(frame.getbbox())

    def test_directory_offsets_still_point_to_complete_image_payloads(self):
        reserved, icon_type, count = struct.unpack_from("<HHH", self.icon)
        self.assertEqual((reserved, icon_type, count), (0, 1, 11))
        for index in range(count):
            length, offset = struct.unpack_from("<II", self.icon, 6 + index * 16 + 8)
            self.assertGreaterEqual(offset, 6 + count * 16)
            self.assertEqual(len(self.icon[offset:offset + length]), length)

    def test_taskbar_frames_use_the_original_artwork_without_a_second_resize(self):
        outputs = branding.generate()
        with Image.open(BytesIO(self.icon)) as icon:
            for size in (16, 20, 24, 28, 32, 40, 48, 64, 128):
                with self.subTest(size=size):
                    with Image.open(BytesIO(outputs[branding.ICONS / f"{size}x{size}.png"])) as native:
                        self.assertEqual(icon.ico.getimage((size, size)).tobytes(), native.tobytes())

    def test_visible_logo_pixels_are_white_and_negative_space_stays_transparent(self):
        outputs = branding.generate()
        with Image.open(BytesIO(outputs[ROOT / "apps/desktop/public/logo.png"])) as image:
            self.assertEqual(image.mode, "RGBA")
            self.assertEqual(image.getpixel((0, 0))[3], 0)
            visible = [pixel for pixel in image.get_flattened_data() if pixel[3] > 0]
            self.assertTrue(visible)
            self.assertTrue(all(pixel[:3] == (255, 255, 255) for pixel in visible))
            self.assertTrue(any(0 < pixel[3] < 255 for pixel in visible))

    def test_title_bar_icons_cover_common_windows_dpi_scales(self):
        outputs = branding.generate()
        for size in (16, 20, 24, 28, 32, 40, 48, 64):
            with self.subTest(size=size):
                with Image.open(BytesIO(outputs[branding.ICONS / f"{size}x{size}.png"])) as image:
                    self.assertEqual(image.size, (size, size))

    def test_opaque_mobile_icons_keep_the_white_logo_visible(self):
        outputs = branding.generate()
        with Image.open(BytesIO(outputs[branding.ICONS / "ios/AppIcon-20x20@3x.png"])) as image:
            self.assertEqual(image.mode, "RGB")
            colors = image.getcolors(image.width * image.height)
            self.assertGreater(len(colors), 1)
            self.assertTrue(any(color == (255, 255, 255) for _, color in colors))


if __name__ == "__main__":
    unittest.main()
