use super::{
    media_error, parse_preview,
    process::ProcessTree,
    tools::{capped, capture, process_failure},
    verification::CookieFile,
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
    mut job: YoutubeJob,
    cancel: CancellationToken,
) -> Result<(), DlmanError> {
    let tools = core.youtube.tools()?;
    let settings = core.get_settings().await;
    let web_job: Option<WebMediaJob> = download
        .web_media
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|_| media_error("Invalid stored webpage video selection."))?;
    let web_context = if let Some(web) = &web_job {
        Some(
            core.web_media
                .context_for_job(&download.url, web.source_key.as_deref())
                .await?,
        )
    } else {
        None
    };
    let session = if web_job.is_some() {
        None
    } else {
        core.youtube
            .browser_session(&download.url, job.requires_browser_session)
            .await?
    };
    if session.is_some() && !job.requires_browser_session {
        // A failed anonymous job can be retried after explicit verification of this video.
        // Persist the requirement before using credentials so restart cannot silently downgrade it.
        job.requires_browser_session = true;
        download.youtube = Some(
            serde_json::to_value(&job)
                .map_err(|_| media_error("Cannot persist YouTube verification requirement."))?,
        );
        core.download_manager
            .db()
            .upsert_download(&download)
            .await?;
    }
    // A queued job outlives its preview. Re-extract and validate the exact stored selection.
    let inspection_slot = tokio::select! {
        _ = cancel.cancelled() => return Err(DlmanError::Cancelled),
        permit = core.youtube.slots.acquire() => permit.map_err(|_| media_error("Video inspection service stopped."))?,
    };
    let value = if let Some(context) = &web_context {
        tools
            .metadata_context(context, &settings.proxy, cancel.clone())
            .await?
    } else {
        tools
            .metadata(
                &download.url,
                &settings.proxy,
                cancel.clone(),
                session.as_deref(),
            )
            .await?
    };
    let preview = if web_job.is_some() {
        super::parse_web_preview(&download.url, &value)?
    } else {
        parse_preview(&download.url, &value)?
    };
    drop(inspection_slot);
    if !job.selection.matches(&preview) {
        return Err(media_error("The selected quality is no longer available. Add this video again to choose another format."));
    }
    if cancel.is_cancelled() {
        return Err(DlmanError::Cancelled);
    }
    // Inspection can discover a size that was unavailable when the job was queued.
    let refreshed_size = preview
        .variants
        .iter()
        .find(|variant| {
            variant.id == job.selection.format_id
                && variant.output_mode == job.selection.output_mode
        })
        .and_then(|variant| variant.size)
        .filter(|size| *size > 0);
    if refreshed_size.is_some() && refreshed_size != download.size {
        download.size = refreshed_size;
        core.download_manager
            .db()
            .update_media_progress(download.id, download.downloaded, download.size)
            .await?;
        core.emit(CoreEvent::DownloadUpdated {
            download: download.clone(),
        });
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
    let cookies = session.as_deref().map(CookieFile::create).transpose()?;
    if let Some(cookies) = &cookies {
        args.extend(["--cookies".into(), cookies.path().display().to_string()]);
    }
    let _web_cookie_file = if let Some(context) = &web_context {
        let (headers, cookie_file) = context.tool_args()?;
        args.extend(headers);
        cookie_file
    } else {
        None
    };
    args.extend(download_args(&job.selection, &job.stage_dir, limit));
    if web_job.is_some() {
        // A single HLS/TS format also needs a container conversion; merging alone
        // only changes the container when multiple audio/video inputs are selected.
        args.extend(["--remux-video".into(), job.selection.container.clone()]);
    }
    args.extend([
        "--".into(),
        web_context
            .as_ref()
            .map_or_else(|| download.url.clone(), |context| context.url.clone()),
    ]);
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
        let (_, diagnostics, status) = tokio::try_join!(
            progress(stdout, core, &download),
            capped(stderr, 128 * 1024),
            async { process.child.wait().await.map_err(DlmanError::from) }
        )?;
        if !status.success() {
            return Err(process_failure(
                &diagnostics,
                "Video transfer failed. Partial files were retained; use Retry.",
            ));
        }
        Ok(())
    };
    let result = tokio::select! { _ = cancel.cancelled() => Err(DlmanError::Cancelled), result = work => result };
    process.stop().await?;
    drop(cookies);
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
    let probe: Value = serde_json::from_slice(&probe)
        .map_err(|_| media_error("Invalid media verification output."))?;
    let silent = web_job.is_some()
        && preview
            .variants
            .iter()
            .any(|variant| variant.id == job.selection.format_id && variant.audio == "none");
    if silent {
        if !probe["streams"].as_array().is_some_and(|streams| {
            streams.len() <= 128 && streams.iter().any(|stream| stream["codec_type"] == "video")
        }) {
            return Err(media_error(
                "Completed media does not contain the selected video stream.",
            ));
        }
    } else {
        verify_streams(&probe, job.selection.output_mode)?;
    }
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

#[cfg(test)]
mod size_tests {
    use super::*;
    use serde_json::json;

    async fn observed_progress(
        format_id: &str,
        initial_size: Option<u64>,
        records: &[Value],
    ) -> (Vec<(u64, Option<u64>)>, Download) {
        let root = std::env::temp_dir().join(format!("dlman-video-size-{}", uuid::Uuid::new_v4()));
        let core = DlmanCore::new(root.clone()).await.unwrap();
        let mut download = Download::new(
            "https://www.youtube.com/watch?v=abcdefghijk".into(),
            root.join("downloads"),
            uuid::Uuid::nil(),
        );
        download.size = initial_size;
        download.youtube = Some(json!({
            "selection": { "identity": "Youtube:abcdefghijk", "format_id": format_id,
                "container": "mp4", "quality": "1080p", "output_mode": "video" },
            "stage_dir": root.join("staging")
        }));
        core.download_manager
            .db()
            .upsert_download(&download)
            .await
            .unwrap();
        let mut events = core.subscribe();
        let input = records
            .iter()
            .map(|value| format!("DLMAN_PROGRESS:{value}\n"))
            .collect::<String>();
        progress(input.as_bytes(), &core, &download).await.unwrap();
        let mut observed = Vec::new();
        while let Ok(event) = events.try_recv() {
            if let CoreEvent::DownloadProgress {
                downloaded, total, ..
            } = event
            {
                observed.push((downloaded, total));
            }
        }
        let stored = core.get_download(download.id).await.unwrap();
        core.youtube.shutdown().await;
        core.scheduler.stop().await;
        (observed, stored)
    }

    #[tokio::test]
    async fn video_size_uses_runtime_estimates_then_exact_totals_and_persists_them() {
        let (observed, stored) = observed_progress("18", None, &[
            json!({"filename":"media.mp4", "downloaded_bytes":100, "total_bytes_estimate":1000.5}),
            json!({"filename":"media.mp4", "downloaded_bytes":200}),
            json!({"filename":"media.mp4", "downloaded_bytes":1200, "total_bytes":1200, "status":"finished"}),
        ]).await;
        assert_eq!(
            observed,
            vec![(100, Some(1001)), (200, Some(1001)), (1199, Some(1200))]
        );
        assert_eq!(stored.size, Some(1200));
        assert_eq!(stored.downloaded, 1199);
    }

    #[tokio::test]
    async fn video_size_never_treats_one_stream_as_the_whole_video() {
        let (observed, stored) = observed_progress("137+140", None, &[
            json!({"filename":"media.f137.mp4", "downloaded_bytes":1000, "total_bytes":1000, "status":"finished"}),
            json!({"filename":"media.f140.m4a", "downloaded_bytes":50, "total_bytes_estimate":100}),
            json!({"filename":"media.f140.m4a", "downloaded_bytes":100, "total_bytes":100, "status":"finished"}),
        ]).await;
        assert_eq!(
            observed,
            vec![(1000, None), (1050, Some(1100)), (1099, Some(1100))]
        );
        assert_eq!(stored.size, Some(1100));
    }

    #[tokio::test]
    async fn video_size_keeps_the_combined_preview_until_all_stream_totals_arrive() {
        let (observed, _) = observed_progress(
            "137+140",
            Some(1300),
            &[
                json!({"filename":"media.f137.mp4", "downloaded_bytes":500, "total_bytes":1000}),
                json!({"filename":"media.f137.mp4", "downloaded_bytes":1000, "total_bytes":1000}),
                json!({"filename":"media.f140.m4a", "downloaded_bytes":50, "total_bytes":100}),
            ],
        )
        .await;
        assert_eq!(
            observed,
            vec![(500, Some(1300)), (1000, Some(1300)), (1050, Some(1100))]
        );
    }

    #[tokio::test]
    async fn video_size_counts_reused_components_without_downloaded_bytes() {
        let (observed, stored) = observed_progress(
            "137+140",
            None,
            &[
                json!({"filename":"media.f137.mp4", "total_bytes":1000, "status":"finished"}),
                json!({"filename":"media.f140.m4a", "total_bytes":100, "status":"finished"}),
            ],
        )
        .await;
        assert_eq!(observed, vec![(1000, None), (1099, Some(1100))]);
        assert_eq!(stored.size, Some(1100));
    }

    #[tokio::test]
    async fn video_size_rejects_invalid_runtime_totals() {
        let (observed, stored) = observed_progress("18", None, &[
            json!({"filename":"media.mp4", "downloaded_bytes":100, "total_bytes":0}),
            json!({"filename":"media.mp4", "downloaded_bytes":200, "total_bytes":-1, "total_bytes_estimate":1e308}),
        ]).await;
        assert_eq!(observed, vec![(100, None), (200, None)]);
        assert_eq!(stored.size, None);
    }
}

async fn progress(
    source: impl tokio::io::AsyncRead + Unpin,
    core: &DlmanCore,
    download: &Download,
) -> Result<(), DlmanError> {
    // Bound each line, not total output: a long download must not exhaust a lifetime quota.
    let mut source = BufReader::new(source);
    let mut line = Vec::new();
    let mut components = std::collections::HashMap::<String, (u64, Option<u64>)>::new();
    let job = download
        .youtube
        .clone()
        .and_then(|value| serde_json::from_value::<YoutubeJob>(value).ok());
    let web_job = download
        .web_media
        .clone()
        .and_then(|value| serde_json::from_value::<WebMediaJob>(value).ok());
    let selection = job
        .as_ref()
        .map(|job| &job.selection)
        .or_else(|| web_job.as_ref().map(|job| &job.selection));
    let expected_components = selection
        .as_ref()
        .map_or(1, |selection| selection.format_id.split('+').count());
    let video_output = selection.as_ref().map_or(true, |selection| {
        selection.output_mode == YoutubeOutput::Video
    });
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
                let reported_total = super::metadata::positive_bytes(&value["total_bytes"])
                    .or_else(|| super::metadata::positive_bytes(&value["total_bytes_estimate"]));
                let received = value["downloaded_bytes"]
                    .as_u64()
                    .or_else(|| {
                        (value["status"] == "finished")
                            .then_some(reported_total)
                            .flatten()
                    })
                    .unwrap_or(0);
                let previous = components.entry(key).or_default();
                previous.0 = previous.0.max(received);
                previous.1 = reported_total.or(previous.1);
                let total_received = components
                    .values()
                    .map(|(downloaded, _)| *downloaded)
                    .fold(0u64, u64::saturating_add)
                    .max(download.downloaded);
                // The selected size includes both streams, but conversion/publish remain outstanding.
                // A component total is not the full video size until every selected stream is known.
                let runtime_total = if video_output && components.len() == expected_components {
                    components
                        .values()
                        .try_fold(0u64, |sum, (_, total)| sum.checked_add((*total)?))
                        .filter(|total| *total > 0 && *total <= i64::MAX as u64)
                } else {
                    None
                };
                let total = runtime_total.or(download.size.filter(|size| *size > 0));
                let total_received = total
                    .map(|size| total_received.min(size.saturating_sub(1)))
                    .unwrap_or(total_received);
                core.download_manager
                    .db()
                    .update_media_progress(download.id, total_received, total)
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
