use super::verification::{BrowserSession, CookieFile};
use super::{media_error, process::ProcessTree};
use crate::DlmanError;
use dlman_types::ProxySettings;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub(super) struct MediaTools {
    pub root: PathBuf,
    pub yt_dlp: PathBuf,
    pub ffprobe: PathBuf,
    deno: PathBuf,
}

impl MediaTools {
    pub fn load(root: &Path) -> Result<Self, DlmanError> {
        let platform = if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
            "windows-x64"
        } else if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
            "linux-x64"
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
            "macos-x64"
        } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
            "macos-arm64"
        } else {
            return Err(media_error("YouTube is unavailable on this architecture."));
        };
        let lock: Value = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/youtube-tools.json"
        )))
        .map_err(|_| media_error("Invalid bundled media tool manifest."))?;
        if lock["platform"].as_str() != Some(platform) {
            return Err(tool_error());
        }
        let files = lock["files"].as_object().ok_or_else(tool_error)?;
        if !root.is_absolute() || files.len() != 4 {
            return Err(tool_error());
        }
        for (name, expected) in files {
            let path = root.join(name);
            let info = std::fs::symlink_metadata(&path).map_err(|_| tool_error())?;
            if !info.is_file() || info.file_type().is_symlink() {
                return Err(tool_error());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if info.permissions().mode() & 0o111 == 0 {
                    return Err(tool_error());
                }
            }
            let mut input = std::fs::File::open(&path).map_err(|_| tool_error())?;
            let mut digest = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let n = input.read(&mut buffer).map_err(|_| tool_error())?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
            }
            if expected.as_str() != Some(format!("{:x}", digest.finalize()).as_str()) {
                return Err(tool_error());
            }
        }
        let exe = |name: &str| {
            root.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            })
        };
        Ok(Self {
            root: root.to_path_buf(),
            yt_dlp: exe("yt-dlp"),
            ffprobe: exe("ffprobe"),
            deno: exe("deno"),
        })
    }

    pub fn args(&self, proxy: &ProxySettings) -> Result<Vec<String>, DlmanError> {
        let mut args: Vec<String> = [
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-remote-components",
            "--no-js-runtimes",
            "--js-runtimes",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        args.extend([
            format!("deno:{}", self.deno.display()),
            "--no-playlist".into(),
            "--color".into(),
            "never".into(),
            "--socket-timeout".into(),
            "30".into(),
            "--ffmpeg-location".into(),
            self.root.display().to_string(),
        ]);
        match proxy.mode.as_str() {
            "none" => args.extend(["--proxy".into(), String::new()]),
            "manual" => {
                let value = proxy
                    .https_proxy
                    .as_deref()
                    .or(proxy.http_proxy.as_deref())
                    .unwrap_or("");
                if value.is_empty() {
                    args.extend(["--proxy".into(), String::new()]);
                } else {
                    let mut url = url::Url::parse(value)
                        .map_err(|_| media_error("Invalid configured proxy URL."))?;
                    if let Some(user) = proxy.username.as_deref().filter(|v| !v.is_empty()) {
                        url.set_username(user)
                            .map_err(|_| media_error("Invalid proxy credentials."))?;
                        url.set_password(proxy.password.as_deref())
                            .map_err(|_| media_error("Invalid proxy credentials."))?;
                    }
                    args.extend(["--proxy".into(), url.to_string()]);
                }
            }
            // System mode intentionally follows the same environment proxy configuration as reqwest.
            "system" => {}
            _ => return Err(media_error("Unsupported proxy mode.")),
        }
        Ok(args)
    }

    pub async fn metadata(
        &self,
        url: &str,
        proxy: &ProxySettings,
        cancel: CancellationToken,
        session: Option<&BrowserSession>,
    ) -> Result<Value, DlmanError> {
        let mut args = self.args(proxy)?;
        let cookies = session.map(CookieFile::create).transpose()?;
        if let Some(cookies) = &cookies {
            args.extend(["--cookies".into(), cookies.path().display().to_string()]);
        }
        args.extend([
            "--dump-single-json".into(),
            "--skip-download".into(),
            "--no-warnings".into(),
            "--".into(),
            url.into(),
        ]);
        let bytes = capture(
            &self.yt_dlp,
            &args,
            cancel,
            Duration::from_secs(150),
            8 * 1024 * 1024,
        )
        .await?;
        serde_json::from_slice(&bytes)
            .map_err(|_| media_error("The media tool returned invalid metadata."))
    }
}

fn tool_error() -> DlmanError {
    media_error("Bundled YouTube tools are missing or failed verification. Reinstall a complete DLMan package.")
}

pub(super) fn process_failure(stderr: &[u8], fallback: &str) -> DlmanError {
    // Classify bounded diagnostics without exposing proxy credentials, URLs or local paths.
    let diagnostic = String::from_utf8_lossy(stderr).to_lowercase();
    if diagnostic
        .lines()
        .any(|line| line.contains("sign in to confirm") && line.contains("not a bot"))
    {
        return media_error("YouTube blocked this request with a bot check. Public videos can also require verification. Use Verify in browser, complete the check, then share the session through the DLMan extension.");
    }
    media_error(fallback)
}

pub(super) async fn capped(
    mut source: impl AsyncRead + Unpin,
    max: usize,
) -> Result<Vec<u8>, DlmanError> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let n = source.read(&mut chunk).await?;
        if n == 0 {
            return Ok(bytes);
        }
        if bytes.len() + n > max {
            return Err(media_error("Media tool output exceeded its limit."));
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
}

pub(super) async fn capture(
    executable: &Path,
    args: &[String],
    cancel: CancellationToken,
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>, DlmanError> {
    let mut process = ProcessTree::spawn(executable, args)?;
    let stdout = process.child.stdout.take().ok_or_else(tool_error)?;
    let stderr = process.child.stderr.take().ok_or_else(tool_error)?;
    let work = async {
        let (out, diagnostics, status) =
            tokio::try_join!(capped(stdout, limit), capped(stderr, 128 * 1024), async {
                process.child.wait().await.map_err(DlmanError::from)
            })?;
        if !status.success() {
            return Err(process_failure(&diagnostics, "This public video could not be processed. It may be unavailable or require an account."));
        }
        Ok(out)
    };
    let result = tokio::select! {
        _ = cancel.cancelled() => Err(DlmanError::Cancelled),
        result = tokio::time::timeout(timeout, work) => result.unwrap_or_else(|_| Err(media_error("Media inspection timed out."))),
    };
    // Reap descendants even when output bounds, cancellation or timeout ends collection.
    process.stop().await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_bot_checks_without_exposing_raw_diagnostics() {
        for diagnostic in [
            "ERROR: [youtube] HLUamwXQ218: Sign in to confirm you're not a bot.",
            "ERROR: [youtube] HLUamwXQ218: Sign in to confirm you’re not a bot.",
        ] {
            let stderr = format!(
                "{diagnostic} Proxy: https://user:secret@example.com; cookies: C:\\private\\cookies.txt"
            );
            let error = process_failure(stderr.as_bytes(), "Inspection failed.").to_string();
            assert!(error.contains("bot check"), "{error}");
            assert!(error.contains("public"), "{error}");
            assert!(!error.contains("secret"));
            assert!(!error.contains("cookies.txt"));
            assert!(!error.contains("Inspection failed."));
        }
    }

    #[test]
    fn unrelated_failures_keep_the_callers_fallback() {
        for stderr in [
            b"".as_slice(),
            b"ERROR: Private video. Sign in if you have access.",
            b"ERROR: Sign in to confirm your age.",
            b"ERROR: network failure\xff",
        ] {
            let error = process_failure(stderr, "Transfer failed; partial files retained.");
            assert!(error
                .to_string()
                .contains("Transfer failed; partial files retained."));
            assert!(!error.to_string().contains("bot check"));
        }
    }

    #[test]
    #[ignore = "subprocess fixture used by the media diagnostics test"]
    fn bot_check_fixture() {
        eprintln!("ERROR: [youtube] HLUamwXQ218: Sign in to confirm you're not a bot.");
        std::process::exit(1);
    }

    #[tokio::test]
    async fn capture_reports_the_bot_check_from_a_failed_child() {
        let executable = std::env::current_exe().unwrap();
        let args = vec![
            "--exact".into(),
            "media::youtube::tools::tests::bot_check_fixture".into(),
            "--ignored".into(),
            "--nocapture".into(),
        ];
        let error = capture(
            &executable,
            &args,
            CancellationToken::new(),
            Duration::from_secs(10),
            8192,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("bot check"), "{error}");
        assert!(!error.to_string().contains("may be unavailable"));
    }
}
