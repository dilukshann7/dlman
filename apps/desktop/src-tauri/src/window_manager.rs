//! Window management for DLMan
//!
//! Handles creation and management of windows for browser integration

use tauri::{AppHandle, Emitter, Manager};
use std::sync::Mutex;
use std::collections::HashMap;

/// Tauri deep-link events carry a JSON array; retain legacy newline payloads.
pub fn deep_link_urls(payload: &str) -> Vec<String> {
    serde_json::from_str(payload).unwrap_or_else(|_| payload.lines().map(str::to_owned).collect())
}

/// Window manager for tracking windows
pub struct WindowManager {
    windows: Mutex<HashMap<String, String>>,
}

impl WindowManager {
    pub fn new() -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
        }
    }

    /// Show the main window and trigger the add download dialog
    /// This focuses the existing main window instead of creating a popup
    pub fn show_add_download_popup(
        &self,
        app_handle: &AppHandle,
        url: Option<String>,
    ) -> Result<(), String> {
        // Get the main window (first window)
        if let Some(window) = app_handle.get_webview_window("main") {
            // Show and focus the main window
            window.show().map_err(|e| e.to_string())?;
            window.unminimize().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;

            // Emit event to show the new download dialog with the URL
            window.emit("show-new-download-dialog", url).map_err(|e| e.to_string())?;
            
            return Ok(());
        }

        // Fallback: try any webview window
        let windows = app_handle.webview_windows();
        if let Some((_, window)) = windows.into_iter().next() {
            window.show().map_err(|e| e.to_string())?;
            window.unminimize().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            window.emit("show-new-download-dialog", url).map_err(|e| e.to_string())?;
            return Ok(());
        }

        Err("No windows available".to_string())
    }

    /// Close a popup window by label
    pub fn close_popup(&self, app_handle: &AppHandle, label: &str) -> Result<(), String> {
        if let Some(window) = app_handle.get_webview_window(label) {
            window.close().map_err(|e| e.to_string())?;
            self.windows.lock().unwrap().remove(label);
        }
        Ok(())
    }
}

impl Default for WindowManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::deep_link_urls;

    #[test]
    fn accepts_forwarded_json_deep_links_without_changing_urls() {
        assert_eq!(deep_link_urls(r#"["dlman://add-download?url=https%3A%2F%2Fexample.com%2Ffile"]"#),
                   vec!["dlman://add-download?url=https%3A%2F%2Fexample.com%2Ffile"]);
    }

    #[test]
    fn retains_legacy_newline_deep_links() {
        assert_eq!(deep_link_urls("dlman://first\ndlman://second"), vec!["dlman://first", "dlman://second"]);
    }
}
