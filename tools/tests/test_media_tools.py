"""Offline package preparation fixtures; never execute downloaded tools."""
import importlib.util
import json
from pathlib import Path
import struct
import tempfile
import unittest
import zipfile

MODULE = Path(__file__).resolve().parents[1] / "fetch-media-tools.py"
spec = importlib.util.spec_from_file_location("media_packager", MODULE)
packager = importlib.util.module_from_spec(spec)
spec.loader.exec_module(packager)


class MediaToolsTests(unittest.TestCase):
    def test_all_targets_have_exactly_four_checksum_pinned_tools(self):
        lock = json.loads(packager.LOCK.read_text(encoding="utf-8"))
        self.assertEqual(set(lock["targets"]), {"windows-x64", "linux-x64", "macos-x64", "macos-arm64"})
        for platform, target in lock["targets"].items():
            expected = {name + (".exe" if platform.startswith("windows") else "") for name in ["yt-dlp", "deno", "ffmpeg", "ffprobe"]}
            actual = {name for tool in target["tools"] for name in tool["outputs"].values()}
            self.assertEqual(actual, expected)
            for tool in target["tools"]:
                self.assertRegex(tool["sha256"], r"^[a-f0-9]{64}$")
                self.assertNotIn("/latest/", tool["url"])
                self.assertTrue(tool["url"].startswith("https://"))

    def test_cross_target_binary_rejection_and_universal_mac_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            file = Path(directory) / "binary"
            elf = bytearray(128); elf[:6] = b"\x7fELF\x02\x01"; struct.pack_into("<H", elf, 18, 62)
            file.write_bytes(elf)
            self.assertTrue(packager.architecture(file, "linux-x64"))
            self.assertFalse(packager.architecture(file, "macos-arm64"))
            universal = bytearray(128); universal[:4] = b"\xca\xfe\xba\xbe"
            struct.pack_into(">I", universal, 4, 2)
            struct.pack_into(">I", universal, 8, 0x1000007)
            struct.pack_into(">I", universal, 28, 0x100000C)
            file.write_bytes(universal)
            self.assertTrue(packager.architecture(file, "macos-x64"))
            self.assertTrue(packager.architecture(file, "macos-arm64"))
            self.assertFalse(packager.architecture(file, "windows-x64"))

    def test_extracts_only_requested_members_without_archive_paths(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); archive = root / "archive.zip"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("nested/bin/ffmpeg", b"fixture")
                output.writestr("../../unrequested", b"must not escape")
            packager.extract({"archive":"zip", "outputs":{"ffmpeg":"ffmpeg"}}, archive, root / "output")
            self.assertEqual((root / "output/ffmpeg").read_bytes(), b"fixture")
            self.assertEqual(list((root / "output").iterdir()), [root / "output/ffmpeg"])

    def test_rejects_ambiguous_archive_members(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); archive = root / "archive.zip"
            with zipfile.ZipFile(archive, "w") as output:
                output.writestr("first/ffmpeg", b"first")
                output.writestr("second/ffmpeg", b"second")
            with self.assertRaisesRegex(ValueError, "ambiguous"):
                packager.extract({"archive":"zip", "outputs":{"ffmpeg":"ffmpeg"}}, archive, root / "output")

    def test_rejects_changed_cached_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            previous = packager.CACHE
            try:
                packager.CACHE = Path(directory)
                sha = "a" * 64
                (packager.CACHE / sha).write_bytes(b"modified")
                with self.assertRaisesRegex(ValueError, "checksum"):
                    packager.fetch({"sha256":sha})
            finally:
                packager.CACHE = previous


if __name__ == "__main__":
    unittest.main()
