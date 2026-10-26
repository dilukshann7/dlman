//! Native update ownership. Webviews never provide installer URLs or executable bytes.
use crate::state::AppState;
use base64::Engine;
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

const RELEASES: &str = "https://github.com/dilukshann7/dlman/releases/latest";

#[derive(Default)]
pub struct AppUpdates {
    operation: Mutex<()>,
    payload: Mutex<Payload>,
}

#[derive(Default)]
struct Payload {
    checked: Option<Update>,
    bytes: Option<Arc<Vec<u8>>>,
}

fn verify_payload(bytes: &[u8], signature: &str, public_key: &str) -> Result<(), String> {
    let decode = |value: &str| -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value.trim())
            .map_err(|error| error.to_string())?;
        String::from_utf8(bytes).map_err(|error| error.to_string())
    };
    let key = minisign_verify::PublicKey::decode(&decode(public_key)?)
        .map_err(|error| error.to_string())?;
    let signature = minisign_verify::Signature::decode(&decode(signature)?)
        .map_err(|error| error.to_string())?;
    key.verify(bytes, &signature, true)
        .map_err(|error| format!("Update signature verification failed: {error}"))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    has_update: bool,
    latest_version: Option<String>,
    current_version: String,
    release_url: String,
    release_notes: Option<String>,
    published_at: Option<String>,
    supported: bool,
    unsupported_reason: Option<&'static str>,
}

fn main_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Application updates are available only from the main window.".into())
    }
}

fn unsupported_reason() -> Option<&'static str> {
    if tauri::is_dev() {
        return Some("development");
    }
    if !cfg!(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(
            target_os = "macos",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(target_os = "linux", target_arch = "x86_64"),
    )) {
        return Some("unsupported-platform");
    }
    if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
        return Some("package-manager");
    }
    None
}

fn info(app: &AppHandle, update: Option<&Update>, reason: Option<&'static str>) -> UpdateInfo {
    UpdateInfo {
        has_update: update.is_some(),
        latest_version: update.map(|update| update.version.clone()),
        current_version: app.package_info().version.to_string(),
        release_url: RELEASES.into(),
        release_notes: update.and_then(|update| update.body.clone()),
        published_at: None,
        supported: reason.is_none(),
        unsupported_reason: reason,
    }
}

#[tauri::command]
pub async fn check_app_update(
    app: AppHandle,
    window: WebviewWindow,
    updates: State<'_, AppUpdates>,
) -> Result<UpdateInfo, String> {
    main_window(&window)?;
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    let reason = unsupported_reason();
    // Preserve a verified download if a manual check is requested while it is ready.
    {
        let payload = updates.payload.lock().await;
        if payload.bytes.is_some() {
            return Ok(info(&app, payload.checked.as_ref(), reason));
        }
    }
    if let Some(reason) = reason {
        let response = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| error.to_string())?
            .get("https://api.github.com/repos/dilukshann7/dlman/releases/latest")
            .header("User-Agent", "DLMan-Update-Checker")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .map_err(|error| format!("Could not check for updates: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Could not check for updates: {error}"))?
            .json::<serde_json::Value>()
            .await
            .map_err(|error| error.to_string())?;
        let version = response["tag_name"]
            .as_str()
            .ok_or("Release is missing its version.")?;
        let latest = semver::Version::parse(version.trim_start_matches('v'))
            .map_err(|error| error.to_string())?;
        let mut result = info(&app, None, Some(reason));
        result.has_update = latest > app.package_info().version
            && latest.pre.is_empty()
            && response["draft"] == false
            && response["prerelease"] == false;
        result.latest_version = Some(latest.to_string());
        result.release_notes = response["body"].as_str().map(str::to_owned);
        result.published_at = response["published_at"].as_str().map(str::to_owned);
        return Ok(result);
    }
    let updater = app
        .updater_builder()
        .timeout(Duration::from_secs(20))
        .version_comparator(|current, release| {
            release.version.pre.is_empty() && release.version > current
        })
        .on_before_exit({
            let app = app.clone();
            move || {
                // Windows exits directly from the plugin; its normal Tauri Exit event is bypassed.
                tauri::async_runtime::block_on(async {
                    if let Some(state) = app.try_state::<AppState>() {
                        if let Some(core) = state.core.read().await.as_ref() {
                            core.shutdown_for_update().await;
                        }
                    }
                });
            }
        })
        .build()
        .map_err(|error| error.to_string())?;
    let mut update = updater
        .check()
        .await
        .map_err(|error| format!("Could not check for updates: {error}"))?;
    if let Some(update) = update.as_mut() {
        // Large bundled media tools can take minutes to transfer on a slow connection.
        update.timeout = Some(Duration::from_secs(30 * 60));
        if update.download_url.scheme() != "https" {
            return Err("Update download requires HTTPS.".into());
        }
    }
    let result = info(&app, update.as_ref(), None);
    updates.payload.lock().await.checked = update;
    Ok(result)
}

#[derive(Clone, Serialize)]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

#[tauri::command]
pub async fn download_app_update(
    app: AppHandle,
    window: WebviewWindow,
    updates: State<'_, AppUpdates>,
) -> Result<(), String> {
    main_window(&window)?;
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    let update = {
        let payload = updates.payload.lock().await;
        if payload.bytes.is_some() {
            return Ok(());
        }
        payload
            .checked
            .clone()
            .ok_or("Check for an update before downloading.")?
    };
    let mut downloaded = 0u64;
    let bytes = update
        .download(
            |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                let _ = app.emit_to(
                    "main",
                    "app-update-progress",
                    Progress { downloaded, total },
                );
            },
            || {},
        )
        .await
        .map_err(|error| format!("Could not download or verify the update: {error}"))?;
    // Tauri verifies the signature before returning. A failure never replaces the stored bytes.
    updates.payload.lock().await.bytes = Some(Arc::new(bytes));
    Ok(())
}

#[tauri::command]
pub async fn can_install_app_update(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    main_window(&window)?;
    state
        .with_core(|core| core.download_manager.admission.can_install())
        .await
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    window: WebviewWindow,
    updates: State<'_, AppUpdates>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    main_window(&window)?;
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "An update operation is already running.")?;
    let core = state
        .core
        .read()
        .await
        .clone()
        .ok_or("Core not initialized")?;
    let admission = core
        .download_manager
        .admission
        .try_install()
        .map_err(|error| error.to_string())?;
    let (update, bytes) = {
        let payload = updates.payload.lock().await;
        (
            payload.checked.clone().ok_or("No update is available.")?,
            payload
                .bytes
                .clone()
                .ok_or("Download the update before installing.")?,
        )
    };
    // Reverify immediately before install; the plugin's install() expects already-verified bytes.
    let config: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
        .map_err(|error| error.to_string())?;
    let public_key = config["plugins"]["updater"]["pubkey"]
        .as_str()
        .ok_or("Missing updater public key.")?;
    verify_payload(bytes.as_slice(), &update.signature, public_key)?;
    // macOS installation must be allowed to dispatch work to the main thread.
    let result =
        tauri::async_runtime::spawn_blocking(move || update.install(bytes.as_slice())).await;
    match result {
        Ok(Ok(())) => {
            // Windows never returns on success; its installer relaunches the app.
            core.shutdown_for_update().await;
            // Hold admission until process exit, including the restart handoff.
            std::mem::forget(admission);
            app.restart();
        }
        result => {
            // Windows' pre-exit hook can run before an installer spawn failure.
            core.recover_after_update_failure(admission).await;
            match result {
                Ok(Err(error)) => Err(format!("Could not install the update: {error}")),
                Err(error) => Err(format!("Update installer stopped unexpectedly: {error}")),
                Ok(Ok(())) => unreachable!(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_payload_is_accepted_and_modified_payload_is_rejected() {
        let bytes = include_bytes!("../tests/fixtures/update-payload.txt");
        let signature = include_str!("../tests/fixtures/update-payload.txt.sig");
        let key = include_str!("../tests/fixtures/update-test.pub");
        verify_payload(bytes, signature, key).unwrap();
        let mut changed = bytes.to_vec();
        changed.push(0);
        assert!(verify_payload(&changed, signature, key).is_err());
    }

    #[test]
    fn wrong_key_and_malformed_signature_cannot_reach_installation() {
        let bytes = include_bytes!("../tests/fixtures/update-payload.txt");
        let signature = include_str!("../tests/fixtures/update-payload.txt.sig");
        let key = include_str!("../tests/fixtures/update-test.pub");
        assert!(verify_payload(bytes, "not a signature", key).is_err());
        let mut decoded = base64::engine::general_purpose::STANDARD
            .decode(key.trim())
            .unwrap();
        let index = decoded.len() - 3;
        decoded[index] = if decoded[index] == b'A' { b'B' } else { b'A' };
        let changed_key = base64::engine::general_purpose::STANDARD.encode(decoded);
        assert!(verify_payload(bytes, signature, &changed_key).is_err());
    }
}
