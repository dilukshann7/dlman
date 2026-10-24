//! Keep the operating system's login entry in sync with the saved preference.

use dlman_core::{DlmanCore, DlmanError};
use dlman_types::Settings;
use tauri::{AppHandle, Manager};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use tauri_plugin_autostart::ManagerExt;

#[derive(Default)]
pub struct SettingsLock(tokio::sync::Mutex<()>);

pub fn apply(app: &AppHandle, enabled: bool) -> Result<(), String> {
    // Every entry point must leave the installed login entry alone during development.
    if tauri::is_dev() {
        return Ok(());
    }
    #[cfg(windows)]
    {
        let _ = app;
        windows_registration(enabled)
            .map_err(|error| format!("Could not update launch at login: {error}"))
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let manager = app.autolaunch();
        let result = if enabled {
            // Refresh the executable path as well, including after upgrades or portable moves.
            manager.enable()
        } else if manager.is_enabled().map_err(|error| error.to_string())? {
            manager.disable()
        } else {
            Ok(())
        };
        result.map_err(|error| format!("Could not update launch at login: {error}"))
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = (app, enabled);
        Ok(())
    }
}

#[cfg(windows)]
fn windows_startup_command(executable: &std::path::Path) -> String {
    format!("\"{}\"", executable.display())
}

#[cfg(windows)]
fn windows_registration(enabled: bool) -> std::io::Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, enums::REG_BINARY, RegKey, RegValue};

    let user = RegKey::predef(HKEY_CURRENT_USER);
    let (run, _) = user.create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")?;
    if enabled {
        // Store an absolute, quoted command without a shell or administrator privileges.
        // Refresh the executable path as well, including after upgrades or portable moves.
        let executable = std::env::current_exe()?;
        run.set_value("DLMan", &windows_startup_command(&executable))?;
        let (approved, _) = user.create_subkey(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run",
        )?;
        approved.set_raw_value(
            "DLMan",
            &RegValue {
                vtype: REG_BINARY,
                bytes: vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            },
        )?;
    } else {
        match run.delete_value("DLMan") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub async fn save(app: &AppHandle, core: &DlmanCore, settings: Settings) -> Result<(), DlmanError> {
    // Serialize native registration and persistence so concurrent saves cannot leave them mismatched.
    let lock = app.state::<SettingsLock>();
    let _guard = lock.0.lock().await;
    let previous = core.get_settings().await;
    if let Err(error) = apply(app, settings.start_on_boot) {
        // Registration may have failed after writing one of the platform's startup entries.
        if let Err(rollback_error) = apply(app, previous.start_on_boot) {
            tracing::warn!(%rollback_error, "Could not restore launch at login after registration failed");
        }
        return Err(DlmanError::InvalidOperation(error));
    }
    if let Err(error) = core.update_settings(settings).await {
        if let Err(rollback_error) = apply(app, previous.start_on_boot) {
            tracing::warn!(%rollback_error, "Could not restore launch-at-login registration after a settings save failed");
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::windows_startup_command;
    use std::path::Path;

    #[test]
    fn windows_login_command_quotes_executable_paths_with_spaces() {
        assert_eq!(
            windows_startup_command(Path::new(r"C:\Program Files\DLMan\dlman-desktop.exe")),
            r#""C:\Program Files\DLMan\dlman-desktop.exe""#,
        );
    }
}
