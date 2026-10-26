#!/usr/bin/env python3
"""Upload the existing local DLMan updater key using the Git credential helper.

Requires PyNaCl. Credentials and key material are never printed or written to the repository.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import subprocess
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", default="dilukshann7/dlman")
    parser.add_argument("--key-dir", type=Path, default=Path.home() / ".tauri" / "dlman-updater")
    parser.add_argument("--verify-only", action="store_true", help="Read secret metadata without replacing values")
    args = parser.parse_args()
    from nacl.public import PublicKey, SealedBox

    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", GCM_INTERACTIVE="never", GCM_GUI_PROMPT="0")
    credential = subprocess.run(["git", "credential", "fill"], input="protocol=https\nhost=github.com\n\n",
                                text=True, capture_output=True, env=env)
    fields = dict(line.split("=", 1) for line in credential.stdout.splitlines() if "=" in line)
    token = fields.get("password")
    if credential.returncode or not token:
        raise SystemExit("No noninteractive GitHub credential available. Configure the two repository Actions secrets manually.")

    def request(path, body=None):
        payload = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request("https://api.github.com/repos/" + args.repo + path, data=payload,
            method="GET" if body is None else "PUT", headers={"Authorization": "Bearer " + token,
            "Accept": "application/vnd.github+json", "User-Agent": "DLMan-Updater-Setup",
            "Content-Type": "application/json", "X-GitHub-Api-Version": "2022-11-28"})
        try:
            with urllib.request.urlopen(req, timeout=30) as response:
                content = response.read()
                return json.loads(content) if content else None
        except urllib.error.HTTPError as error:
            raise SystemExit(f"GitHub rejected updater-secret setup (HTTP {error.code}). Configure repository Actions secrets manually.") from None

    if args.verify_only:
        for name in ("TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"):
            if request("/actions/secrets/" + name)["name"] != name:
                raise SystemExit("Unexpected secret metadata returned by GitHub")
            print("Verified repository Actions secret:", name)
        return
    public = request("/actions/secrets/public-key")
    box = SealedBox(PublicKey(base64.b64decode(public["key"])))
    for name, path in {
        "TAURI_SIGNING_PRIVATE_KEY": args.key_dir / "signing.key",
        "TAURI_SIGNING_PRIVATE_KEY_PASSWORD": args.key_dir / "password.txt",
    }.items():
        value = path.read_text(encoding="utf-8").strip().encode()
        request("/actions/secrets/" + name, {"key_id": public["key_id"],
            "encrypted_value": base64.b64encode(box.encrypt(value)).decode()})
        print("Configured repository Actions secret:", name)


if __name__ == "__main__":
    main()
