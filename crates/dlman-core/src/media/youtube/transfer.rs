use super::{
    media_error, parse_preview,
    process::ProcessTree,
    tools::{capped, capture},
};
use crate::{DlmanCore, DlmanError};
use dlman_types::*;
use serde_json::Value;
use std::{path::Path, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

pub(super) async fn run(
    core: &DlmanCore,
    mut download: Download,
    job: YoutubeJob,
    cancel: CancellationToken,
) -> Result<(), DlmanError> {
    let tools = core.youtube.tools()?;
    let settings = core.get_settings().await;
    // A queued job outlives its preview. Re-extract and validate the exact stored selection.
    let inspection_slot = tokio::select! {
        _ = cancel.cancelled() => return Err(DlmanError::Cancelled),
        permit = core.youtube.slots.acquire() => permit.map_err(|_| media_error("Video inspection service stopped."))?,
    };
    let value = tools
        .metadata(&download.url, &settings.proxy, cancel.clone())
        .await?;
    let preview = parse_preview(&download.url, &value)?;
    drop(inspection_slot);
    if !job.selection.matches(&preview) {
        return Err(media_error("The selected quality is no longer available. Add this video again to choose another format."));
    }
    if cancel.is_cancelled() {
        return Err(DlmanError::Cancelled);
    }
    tokio::fs::create_dir_all(&job.stage_dir).await?;
    let info = tokio::fs::symlink_metadata(&job.stage_dir).await?;
    if !info.is_dir() || info.file_type().is_symlink() {
        return Err(media_error("Unsafe YouTube staging directory."));
    }
    let queue = core.queue_manager.get_queue(download.queue_id).await;
    let limit = download
        .speed_limit
        .or_else(|| queue.and_then(|q| q.speed_limit));
    let mut args = tools.args(&settings.proxy)?;
    args.extend(download_args(&job.selection, &job.stage_dir, limit));
    args.extend(["--".into(), download.url.clone()]);
    let mut process = ProcessTree::spawn(&tools.yt_dlp, &args)?;
    let stdout = process
        .child
        .stdout
        .take()
        .ok_or_else(|| media_error("Cannot read media progress."))?;
    let stderr = process
        .child
        .stderr
        .take()
        .ok_or_else(|| media_error("Cannot read media diagnostics."))?;
    let work = async {
        let (_, _, status) = tokio::try_join!(
            progress(stdout, core, &download),
            capped(stderr, 128 * 1024),
            async { process.child.wait().await.map_err(DlmanError::from) }
        )?;
        if !status.success() {
            return Err(media_error(
                "YouTube transfer failed. Partial files were retained; use Retry.",
            ));
        }
        Ok(())
    };
    let result = tokio::select! { _ = cancel.cancelled() => Err(DlmanError::Cancelled), result = work => result };
    process.stop().await?;
    result?;
    // Do not display completion when only the first audio/video component has finished.
    download = core.get_download(download.id).await?;
    core.emit(CoreEvent::DownloadProgress {
        id: download.id,
        downloaded: download.downloaded,
        total: None,
        speed: 0,
        eta: None,
    });
    let output = job
        .stage_dir
        .join(format!("media.{}", job.selection.container));
    verify_file(&output).await?;
    let probe_args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-show_entries",
        "stream=codec_type,codec_name",
        "-of",
        "json",
        "-i",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain(std::iter::once(output.display().to_string()))
    .collect::<Vec<_>>();
    let probe = capture(
        &tools.ffprobe,
        &probe_args,
        cancel.clone(),
        Duration::from_secs(30),
        1024 * 1024,
    )
    .await?;
    verify_streams(
        &serde_json::from_slice(&probe)
            .map_err(|_| media_error("Invalid media verification output."))?,
        job.selection.output_mode,
    )?;
    // Reload destination: paused jobs may be redirected while retaining their original stage.
    download = core.get_download(download.id).await?;
    publish(core, &mut download, &output, cancel).await?;
    // Completed data is now durable. Remove only this UUID's owned stage.
    let _ = tokio::fs::remove_dir_all(&job.stage_dir).await;
    Ok(())
}

pub(super) fn download_args(
    selection: &YoutubeSelection,
    stage: &Path,
    limit: Option<u64>,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "--newline",
        "--progress",
        "--progress-delta",
        "0.5",
        "--progress-template",
        "download:DLMAN_PROGRESS:%(progress)j",
        "--no-overwrites",
        "--no-post-overwrites",
        "--continue",
        "--part",
        "--retries",
        "3",
        "--fragment-retries",
        "3",
        "--format",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    args.push(selection.format_id.clone());
    if selection.output_mode == YoutubeOutput::Mp3 {
        args.extend(
            [
                "--extract-audio",
                "--audio-format",
                "mp3",
                "--audio-quality",
                "0",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    } else {
        args.extend(["--merge-output-format".into(), selection.container.clone()]);
    }
    if let Some(limit) = limit.filter(|v| *v > 0) {
        args.extend(["--limit-rate".into(), limit.to_string()]);
    }
    args.extend([
        "--paths".into(),
        format!("home:{}", stage.display()),
        "--paths".into(),
        format!("temp:{}", stage.display()),
        "--output".into(),
        "media.%(ext)s".into(),
    ]);
    args
}

async fn progress(
    source: impl tokio::io::AsyncRead + Unpin,
    core: &DlmanCore,
    download: &Download,
) -> Result<(), DlmanError> {
    // Bound each line, not total output: a long download must not exhaust a lifetime quota.
    let mut source = BufReader::new(source);
    let mut line = Vec::new();
    let mut components = std::collections::HashMap::<String, u64>::new();
    loop {
        let buffer = source.fill_buf().await?;
        if buffer.is_empty() {
            return Ok(());
        }
        let end = buffer
            .iter()
            .position(|c| *c == b'\n')
            .map(|n| n + 1)
            .unwrap_or(buffer.len());
        if line.len() + end > 65536 {
            return Err(media_error("Media progress line exceeded its limit."));
        }
        line.extend_from_slice(&buffer[..end]);
        let complete = buffer[end - 1] == b'\n';
        source.consume(end);
        if !complete {
            continue;
        }
        if let Some(record) = line.strip_prefix(b"DLMAN_PROGRESS:") {
            if let Ok(value) = serde_json::from_slice::<Value>(record) {
                let key = value["filename"].as_str().unwrap_or("current").to_owned();
                let received = value["downloaded_bytes"].as_u64().unwrap_or(0);
                let previous = components.entry(key).or_default();
                *previous = (*previous).max(received);
                let total_received = components
                    .values()
                    .copied()
                    .fold(0u64, u64::saturating_add)
                    .max(download.downloaded);
                // The selected size includes both streams, but conversion/publish remain outstanding.
                let total = download.size.filter(|size| *size > 0);
                let total_received = total
                    .map(|size| total_received.min(size.saturating_sub(1)))
                    .unwrap_or(total_received);
                core.download_manager
                    .db()
                    .update_download_progress(download.id, total_received)
                    .await?;
                core.emit(CoreEvent::DownloadProgress {
                    id: download.id,
                    downloaded: total_received,
                    total,
                    speed: value["speed"]
                        .as_f64()
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .unwrap_or(0.0) as u64,
                    eta: value["eta"].as_u64(),
                });
            }
        }
        line.clear();
    }
}

async fn verify_file(path: &Path) -> Result<(), DlmanError> {
    let info = tokio::fs::symlink_metadata(path).await?;
    if !info.is_file() || info.file_type().is_symlink() || info.len() == 0 {
        return Err(media_error("Completed media is missing, empty or unsafe."));
    }
    Ok(())
}

pub(super) fn verify_streams(value: &Value, mode: YoutubeOutput) -> Result<(), DlmanError> {
    let streams = value["streams"]
        .as_array()
        .filter(|s| s.len() <= 128)
        .ok_or_else(|| media_error("Cannot verify output streams."))?;
    let video = streams.iter().any(|s| s["codec_type"] == "video");
    let audio = streams.iter().any(|s| s["codec_type"] == "audio");
    let valid = match mode {
        YoutubeOutput::Video => video && audio,
        YoutubeOutput::Mp3 => {
            !video
                && audio
                && streams
                    .iter()
                    .filter(|s| s["codec_type"] == "audio")
                    .all(|s| s["codec_name"] == "mp3")
        }
    };
    if valid {
        Ok(())
    } else {
        Err(media_error(
            "Completed media does not contain the selected audio/video streams.",
        ))
    }
}

async fn publish(
    core: &DlmanCore,
    download: &mut Download,
    source: &Path,
    cancel: CancellationToken,
) -> Result<(), DlmanError> {
    tokio::fs::create_dir_all(&download.destination).await?;
    let final_path = download.destination.join(&download.filename);
    if final_path.exists() {
        return Err(media_error(
            "The output filename already exists. Partial files were retained.",
        ));
    }
    let temp = download
        .destination
        .join(format!(".dlman-{}.publishing", download.id));
    // A prior interrupted copy is owned by the same UUID. Reject links before removing it.
    if let Ok(info) = tokio::fs::symlink_metadata(&temp).await {
        if !info.is_file() || info.file_type().is_symlink() {
            return Err(media_error("Unsafe publication file."));
        }
        tokio::fs::remove_file(&temp).await?;
    }
    let result = async {
        let mut input = tokio::fs::File::open(source).await?;
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .await?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let bytes = loop {
            if cancel.is_cancelled() {
                return Err(DlmanError::Cancelled);
            }
            let n = input.read(&mut buffer).await?;
            if n == 0 {
                break input.metadata().await?.len();
            }
            output.write_all(&buffer[..n]).await?;
        };
        output.sync_all().await?;
        drop(output);
        if cancel.is_cancelled() {
            return Err(DlmanError::Cancelled);
        }
        // The native rename operation fails if the destination already exists.
        // This also works on destination filesystems that do not support hard links.
        publish_noclobber(&temp, &final_path)?;
        download.downloaded = bytes;
        download.size = Some(bytes);
        download.status = DownloadStatus::Completed;
        download.completed_at = Some(chrono::Utc::now());
        core.download_manager.db().upsert_download(download).await?;
        core.emit(CoreEvent::DownloadUpdated {
            download: download.clone(),
        });
        core.emit(CoreEvent::DownloadStatusChanged {
            id: download.id,
            status: DownloadStatus::Completed,
            error: None,
        });
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_file(temp).await;
    result
}

/// Atomically publish within the destination directory without replacing a file.
fn publish_noclobber(source: &Path, destination: &Path) -> Result<(), DlmanError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let moved = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            return Err(media_error(
                "Cannot publish media without overwriting an existing file. Staging was retained.",
            ));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let source = std::ffi::CString::new(source.as_os_str().as_bytes())
            .map_err(|_| media_error("Invalid publication path."))?;
        let destination = std::ffi::CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| media_error("Invalid publication path."))?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let result = -1;
        if result != 0 {
            return Err(media_error(
                "Cannot publish media without overwriting an existing file. Staging was retained.",
            ));
        }
    }
    Ok(())
}
