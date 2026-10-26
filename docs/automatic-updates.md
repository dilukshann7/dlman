# Automatic desktop updates

DLMan checks the stable GitHub release three seconds after settings load and every six hours thereafter. The existing automatic-check preference disables these checks; the About dialog can always check manually. Checks only discover releases. Clicking **Download Update** downloads and verifies the package. **Install and Restart** requires another click and remains disabled until transfer workers, media extraction, and merging have finished.

Windows updates preserve MSI versus NSIS installer type. macOS supports Intel and Apple Silicon. Linux AppImage supports in-app updates; `.deb` and `.rpm` installations direct users to their package manager or the release packages. Unsupported builds retain release links. Downloaded packages are session-local, and installer failures retain the verified package for retry.

## Signing setup

The public verification key is committed in `apps/desktop/src-tauri/tauri.conf.json`. The corresponding private key and password were generated outside the repository in `C:\Users\Dilukshan\.tauri\dlman-updater\signing.key` and `C:\Users\Dilukshan\.tauri\dlman-updater\password.txt`, in a Windows ACL-protected directory. Back up both privately. Losing this key prevents signing updates trusted by already-installed clients.

The repository requires these GitHub Actions secrets:

- `TAURI_SIGNING_PRIVATE_KEY`: contents of `signing.key`.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: contents of `password.txt`.

To upload them using the current Git credential helper without displaying their values, install `PyNaCl` and run `python tools/configure-updater-secrets.py`. The helper disables interactive credential prompts and reports authentication or permission failures without printing credentials. Alternatively, configure the secrets in the repository's Actions settings. These are updater signatures, separate from Windows Authenticode and Apple signing/notarization.

For a local release build, set the same two environment variables before invoking Tauri. No signing key is needed for source checks or development. Never commit private keys, password files, environment files, or credential output.

## Release pipeline

1. The tag must match the versions in the workspace, root package, desktop package, and Tauri configuration. Missing signing credentials fail before release creation.
2. Each platform builds signed updater artifacts and uploads them to the draft release. Per-job manifest publication is disabled.
3. Linux patches AppImage update information, generates its zsync file, then re-signs and replaces the AppImage and signature. Signing must happen after the final byte modification.
4. The final job downloads the uploaded payloads and signatures. `tools/updater_release.py` verifies Minisign's artifact signature and signed trusted comment against the committed public key, requires every platform and Windows installer type, and emits one `latest.json` with version-pinned download URLs.
5. Only after existing installer/extension checks and updater verification pass is the release published. Prereleases remain outside GitHub's stable latest feed.

The first updater-enabled release must be installed manually. Older installations only know how to open the releases page. Do not rotate the public key without planning a migration signed by the previous key.

## Validation

Run `node --experimental-strip-types --test apps/desktop/tests/app-update.test.mjs apps/desktop/tests/update-scheduler.test.mjs`, `python -m unittest discover -s tools/tests -p test_updater_release.py`, and `pnpm --filter @dlman/desktop exec tsc --noEmit` for focused checks. Python verification requires `tools/requirements-updater.txt`.

Rust admission/signature tests are supplied but require compilation. Native installer acceptance still requires an explicitly authorized build and installation on Windows MSI/NSIS, both macOS architectures, and writable/read-only AppImage installations. Validate blocked installation while transferring or merging, retry after permission failures, preservation of settings/history, and successful relaunch before claiming native acceptance.
