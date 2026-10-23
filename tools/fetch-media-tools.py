"""Prepare checksum-pinned desktop resources. Does not build or run executables."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import tarfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "tools/media-tools.lock.json"
OUTPUT = ROOT / "apps/desktop/src-tauri/resources/media-tools"
CACHE = ROOT / "target/media-tool-cache"
LIMIT = 600 * 1024 * 1024


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def architecture(path, platform):
    """Inspect the executable header without executing foreign platform binaries."""
    with path.open("rb") as source:
        header = source.read(4096)
        if header[:2] == b"MZ":
            offset = struct.unpack_from("<I", header, 0x3C)[0]
            source.seek(offset)
            pe = source.read(6)
            return platform == "windows-x64" and pe[:4] == b"PE\0\0" and struct.unpack_from("<H", pe, 4)[0] == 0x8664
        if header[:4] == b"\x7fELF":
            return platform == "linux-x64" and header[4:6] == b"\x02\x01" and struct.unpack_from("<H", header, 18)[0] == 62
        if header[:4] == b"\xcf\xfa\xed\xfe":
            cpu = struct.unpack_from("<I", header, 4)[0]
            return cpu == {"macos-x64": 0x1000007, "macos-arm64": 0x100000C}.get(platform)
        if header[:4] in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
            count = struct.unpack_from(">I", header, 4)[0]
            step = 32 if header[:4] == b"\xca\xfe\xba\xbf" else 20
            wanted = {"macos-x64": 0x1000007, "macos-arm64": 0x100000C}.get(platform)
            return 0 < count <= 32 and any(struct.unpack_from(">I", header, 8 + i * step)[0] == wanted for i in range(count))
    return False


def fetch(entry):
    CACHE.mkdir(parents=True, exist_ok=True)
    target = CACHE / entry["sha256"]
    if target.is_file():
        if digest(target) != entry["sha256"]:
            raise ValueError("Cached media archive checksum differs from lock")
        return target
    temporary = target.with_suffix(".partial")
    temporary.unlink(missing_ok=True)
    request = urllib.request.Request(entry["url"], headers={"User-Agent": "DLMan-media-packager"})
    with urllib.request.urlopen(request, timeout=60) as response, temporary.open("xb") as output:
        total = 0
        while chunk := response.read(1024 * 1024):
            total += len(chunk)
            if total > LIMIT:
                raise ValueError("Media archive exceeds package limit")
            output.write(chunk)
    if digest(temporary) != entry["sha256"]:
        temporary.unlink()
        raise ValueError(f"Media archive checksum mismatch: {entry['url']}")
    temporary.replace(target)
    return target


def extract(entry, archive, destination):
    destination.mkdir(parents=True, exist_ok=True)
    if entry["archive"] == "raw":
        shutil.copyfile(archive, destination / next(iter(entry["outputs"])))
        return
    opener = zipfile.ZipFile if entry["archive"] == "zip" else tarfile.open
    with opener(archive) as source:
        members = source.infolist() if entry["archive"] == "zip" else source.getmembers()
        for name in entry["outputs"]:
            choices = [member for member in members if Path(member.filename if entry["archive"] == "zip" else member.name).name == name]
            if len(choices) != 1:
                raise ValueError(f"Missing or ambiguous media tool: {name}")
            member = choices[0]
            size = member.file_size if entry["archive"] == "zip" else member.size
            if size > LIMIT or (entry["archive"] != "zip" and not member.isfile()):
                raise ValueError("Unsafe media archive member")
            reader = source.open(member) if entry["archive"] == "zip" else source.extractfile(member)
            with reader, (destination / name).open("wb") as output:
                shutil.copyfileobj(reader, output, length=1024 * 1024)


def prepare(platform):
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    target = lock["targets"][platform]
    # Prepare separately, then replace only files owned by the packager.
    stage = CACHE / (platform + "-prepared")
    stage.mkdir(parents=True, exist_ok=True)
    files = {}
    for entry in target["tools"]:
        archive = fetch(entry)
        unpack = stage / entry["name"]
        extract(entry, archive, unpack)
        for source_name, output_name in entry["outputs"].items():
            path = unpack / source_name
            if not architecture(path, platform):
                raise ValueError(f"Wrong executable architecture for {platform}: {source_name}")
            path.chmod(0o755)
            files[output_name] = digest(path)
    OUTPUT.mkdir(parents=True, exist_ok=True)
    for entry in target["tools"]:
        for source_name, output_name in entry["outputs"].items():
            shutil.copy2(stage / entry["name"] / source_name, OUTPUT / output_name)
    notices = ROOT / "tools/media-licenses"
    shutil.copytree(notices, OUTPUT / "licenses", dirs_exist_ok=True)
    manifest = {"platform": platform, "files": files, "source_lock_sha256": digest(LOCK),
                "archives": [entry["sha256"] for entry in target["tools"]]}
    (OUTPUT / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"Prepared {platform}: {len(files)} verified executables")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=["windows-x64", "linux-x64", "macos-x64", "macos-arm64"])
    prepare(parser.parse_args().platform)
