"""Release artifacts must match the manifest and their Minisign signatures."""
import base64
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

SPEC = importlib.util.spec_from_file_location("updater_release", Path(__file__).parents[1] / "updater_release.py")
updater = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(updater)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.key = Ed25519PrivateKey.generate()
        self.key_id = b"12345678"
        raw_key = b"Ed" + self.key_id + self.key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        self.public = base64.b64encode(b"untrusted comment: public key\n" + base64.b64encode(raw_key) + b"\n").decode()
        names = ["DLMan_1.14.0_x64-setup.exe", "DLMan_1.14.0_x64_en-US.msi",
                 "DLMan_x64.app.tar.gz", "DLMan_aarch64.app.tar.gz", "DLMan_1.14.0_amd64.AppImage"]
        for name in names:
            artifact = self.root / name
            artifact.write_bytes(name.encode())
            self.sign(artifact)

    def sign(self, artifact):
        signature = self.key.sign(hashlib.blake2b(artifact.read_bytes(), digest_size=64).digest())
        packet = b"ED" + self.key_id + signature
        trusted = b"timestamp:1234567890\tfile:" + artifact.name.encode()
        global_signature = self.key.sign(signature + trusted)
        text = (b"untrusted comment: signature\n" + base64.b64encode(packet) + b"\ntrusted comment: " + trusted
                + b"\n" + base64.b64encode(global_signature) + b"\n")
        Path(str(artifact) + ".sig").write_text(base64.b64encode(text).decode(), encoding="utf-8")

    def build(self, version="v1.14.0"):
        return updater.build_manifest(self.root, version, "dilukshann7/dlman", self.public, "Changes")

    def test_complete_manifest_preserves_windows_installer_types_and_both_macos_architectures(self):
        manifest = self.build()
        self.assertEqual(manifest["version"], "1.14.0")
        platforms = manifest["platforms"]
        self.assertEqual(set(platforms), {"windows-x86_64", "windows-x86_64-msi", "windows-x86_64-nsis",
                                          "darwin-x86_64", "darwin-aarch64", "linux-x86_64"})
        self.assertTrue(platforms["windows-x86_64-msi"]["url"].endswith(".msi"))
        self.assertTrue(platforms["windows-x86_64"]["url"].endswith(".exe"))
        self.assertIn("/download/v1.14.0/", platforms["darwin-aarch64"]["url"])
        self.assertEqual(json.loads(json.dumps(manifest)), manifest)

    def test_missing_platform_or_signature_fails(self):
        (self.root / "DLMan_x64.app.tar.gz.sig").unlink()
        with self.assertRaisesRegex(ValueError, "signature"):
            self.build()

    def test_wrong_version_fails(self):
        with self.assertRaisesRegex(ValueError, "version"):
            self.build("v1.15.0")

    def test_patched_appimage_requires_a_fresh_signature(self):
        artifact = self.root / "DLMan_1.14.0_amd64.AppImage"
        artifact.write_bytes(artifact.read_bytes() + b"patched update info")
        with self.assertRaisesRegex(ValueError, "signature"):
            self.build()
        self.sign(artifact)
        self.assertIn("linux-x86_64", self.build()["platforms"])

    def test_different_key_and_forged_trusted_comment_are_rejected(self):
        artifact = self.root / "DLMan_1.14.0_amd64.AppImage"
        signature = Path(str(artifact) + ".sig")
        packet = base64.b64decode(signature.read_text()).replace(b"timestamp:1234567890", b"timestamp:9999999999")
        signature.write_text(base64.b64encode(packet).decode())
        with self.assertRaisesRegex(ValueError, "signature"):
            self.build()

    def test_duplicate_platform_assets_are_rejected(self):
        artifact = self.root / "Another_1.14.0_x64-setup.exe"
        artifact.write_bytes(b"duplicate")
        self.sign(artifact)
        with self.assertRaisesRegex(ValueError, "exactly one"):
            self.build()


if __name__ == "__main__":
    unittest.main()
