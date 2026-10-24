//! DPI-matched Windows title-bar and taskbar icons from the multi-size ICO.

use std::{io, sync::Mutex};
use tauri::{image::Image, Manager, WebviewWindow, WindowEvent};
use windows_sys::Win32::UI::{
    HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi},
    WindowsAndMessaging::{
        CreateIconFromResourceEx, DestroyIcon, FindWindowW, SendMessageW, ICON_BIG, ICON_SMALL,
        SM_CXICON, SM_CXSMICON, WM_SETICON,
    },
};

const ICO: &[u8] = include_bytes!("../icons/icon.ico");

pub fn tray_icon_size() -> i32 {
    let taskbar_class: Vec<u16> = "Shell_TrayWnd\0".encode_utf16().collect();
    // SAFETY: the class name is terminated and the optional title is null.
    // GetDpiForWindow targets Explorer's primary taskbar rather than our window.
    unsafe {
        let taskbar = FindWindowW(taskbar_class.as_ptr(), std::ptr::null());
        let dpi = if taskbar.is_null() {
            GetDpiForSystem()
        } else {
            GetDpiForWindow(taskbar)
        };
        GetSystemMetricsForDpi(SM_CXSMICON, if dpi == 0 { 96 } else { dpi }).max(16)
    }
}

pub fn tray_image(size: i32) -> Result<Image<'static>, String> {
    let frame = frame_for_size(size as u32).ok_or("Missing tray icon frame")?;
    Image::from_bytes(frame).map_err(|error| error.to_string())
}

// Keep native handles alive for as long as Windows references them. Storing the
// opaque value as an integer permits ownership in the Send window-event callback.
struct IconHandle(isize);

impl Drop for IconHandle {
    fn drop(&mut self) {
        // SAFETY: this handle was created by CreateIconFromResourceEx without
        // LR_SHARED; only this owner destroys it after Windows stops using it.
        unsafe { DestroyIcon(self.0 as _) };
    }
}

fn frame_for_size(size: u32) -> Option<&'static [u8]> {
    let count = u16::from_le_bytes(ICO.get(4..6)?.try_into().ok()?) as usize;
    let directory = ICO.get(6..6 + count * 16)?;
    let entry = directory.chunks_exact(16).min_by_key(|entry| {
        let width = if entry[0] == 0 { 256 } else { entry[0] as u32 };
        (width.abs_diff(size), std::cmp::Reverse(width))
    })?;
    let length = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
    let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
    ICO.get(offset..offset.checked_add(length)?)
}

fn load_icon(size: i32) -> Result<IconHandle, String> {
    let frame = frame_for_size(size as u32).ok_or("Missing Windows icon frame")?;
    // SAFETY: the checked slice contains a complete ICO image, is immutable,
    // and stays alive throughout the call. Windows owns the returned copy.
    let handle = unsafe {
        CreateIconFromResourceEx(
            frame.as_ptr(),
            frame.len() as u32,
            1,
            0x0003_0000,
            size,
            size,
            0,
        )
    };
    if handle.is_null() {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(IconHandle(handle as isize))
    }
}

fn dpi_for_scale(scale_factor: f64) -> u32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        (96.0 * scale_factor).round().clamp(96.0, 768.0) as u32
    } else {
        96
    }
}

struct WindowIcons {
    _small: IconHandle,
    _large: IconHandle,
}

impl WindowIcons {
    fn apply(window: &WebviewWindow, scale_factor: f64) -> Result<Self, String> {
        let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
        let dpi = dpi_for_scale(scale_factor);
        // SAFETY: DPI and metric identifiers are valid. Both new handles are
        // retained below; callers release the old pair only after replacement.
        unsafe {
            let small = load_icon(GetSystemMetricsForDpi(SM_CXSMICON, dpi))?;
            let large = load_icon(GetSystemMetricsForDpi(SM_CXICON, dpi))?;
            SendMessageW(hwnd as _, WM_SETICON, ICON_SMALL as usize, small.0);
            SendMessageW(hwnd as _, WM_SETICON, ICON_BIG as usize, large.0);
            Ok(Self {
                _small: small,
                _large: large,
            })
        }
    }
}

pub fn install(window: &WebviewWindow) {
    let initial = WindowIcons::apply(window, window.scale_factor().unwrap_or(1.0));
    if let Err(ref error) = initial {
        tracing::warn!(%error, "Could not set Windows title-bar and taskbar icons");
    }
    let icons = Mutex::new(initial.ok());
    let app = window.app_handle().clone();
    let label = window.label().to_owned();
    window.on_window_event(move |event| match event {
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            crate::tray::refresh_dpi(&app);
            if let Some(window) = app.get_webview_window(&label) {
                match WindowIcons::apply(&window, *scale_factor) {
                    Ok(updated) => {
                        *icons.lock().unwrap_or_else(|error| error.into_inner()) = Some(updated)
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Could not refresh Windows icons after DPI change")
                    }
                }
            }
        }
        WindowEvent::Destroyed => {
            icons
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::{dpi_for_scale, load_icon, tray_image};
    use windows_sys::Win32::{
        Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP},
        UI::WindowsAndMessaging::{GetIconInfo, ICONINFO},
    };

    #[test]
    fn matches_common_dpi_scales_and_handles_invalid_values() {
        for (scale, expected) in [(1.0, 96), (1.25, 120), (1.5, 144), (2.0, 192), (3.0, 288)] {
            assert_eq!(dpi_for_scale(scale), expected);
        }
        for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(dpi_for_scale(scale), 96);
        }
    }

    #[test]
    fn windows_decodes_title_bar_and_taskbar_frames_at_native_sizes() {
        for size in [16, 20, 24, 28, 32, 40, 48, 64, 96, 128, 256] {
            let icon = load_icon(size).expect("Windows must decode each embedded ICO frame");
            // SAFETY: structures are zero-initialized output buffers. GetIconInfo
            // returns owned bitmap copies, which are released after inspection.
            unsafe {
                let mut info: ICONINFO = std::mem::zeroed();
                assert_ne!(GetIconInfo(icon.0 as _, &mut info), 0);
                let mut bitmap: BITMAP = std::mem::zeroed();
                let length = GetObjectW(
                    info.hbmColor,
                    std::mem::size_of::<BITMAP>() as i32,
                    &mut bitmap as *mut _ as _,
                );
                DeleteObject(info.hbmColor);
                DeleteObject(info.hbmMask);
                assert_ne!(length, 0);
                assert_eq!((bitmap.bmWidth, bitmap.bmHeight), (size, size));
            }
        }
    }

    #[test]
    fn tray_images_keep_native_dimensions_for_common_taskbar_dpi() {
        for size in [16, 20, 24, 28, 32, 40, 48, 64] {
            let image = tray_image(size).expect("tray PNG must decode");
            assert_eq!((image.width(), image.height()), (size as u32, size as u32));
        }
    }
}
