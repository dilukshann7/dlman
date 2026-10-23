//! Public, individual YouTube downloads using pinned bundled executables.
mod metadata;
mod process;
mod tools;
mod transfer;

use crate::{DlmanCore, DlmanError};
use dlman_types::*;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use metadata::parse_preview;
use tools::MediaTools;

pub(super) fn media_error(message: &str) -> DlmanError {
    DlmanError::InvalidOperation(message.to_owned())
}

pub fn is_youtube_host(value: &str) -> bool {
    url::Url::parse(value)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .is_some_and(|host| {
            matches!(
                host.as_str(),
                "youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be" | "www.youtu.be"
            )
        })
}

pub fn canonical_url(value: &str) -> Result<String, DlmanError> {
    let url = url::Url::parse(value).map_err(|_| media_error("Invalid YouTube URL."))?;
    if value.len() > 8192
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !is_youtube_host(value)
    {
        return Err(media_error(
            "Only public YouTube video and Shorts links are supported.",
        ));
    }
    let video = if matches!(url.host_str(), Some("youtu.be" | "www.youtu.be")) {
        Some(url.path().trim_start_matches('/').to_owned())
    } else if url.path() == "/watch" {
        url.query_pairs().find(|(key, _)| key == "v").map(|(_, v)| v.into_owned())
    } else {
        url.path().strip_prefix("/shorts/").map(|id| id.trim_end_matches('/').to_owned())
    }.filter(|id| id.len() == 11 && id.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')))
        .ok_or_else(|| media_error("Only individual YouTube videos and Shorts are supported; playlists and live links are unsupported."))?;
    Ok(format!("https://www.youtube.com/watch?v={video}"))
}

struct InspectionEntry {
    url: String,
    created: Instant,
    result: YoutubeInspection,
}

pub(crate) struct YoutubeTask {
    cancel: CancellationToken,
    join: tokio::task::JoinHandle<Result<(), DlmanError>>,
}

pub struct YoutubeService {
    root: RwLock<Option<PathBuf>>,
    inspections: Mutex<HashMap<Uuid, InspectionEntry>>,
    slots: Semaphore,
    inspection_tasks: Mutex<HashMap<Uuid, tokio::task::JoinHandle<()>>>,
    pub(crate) tasks: Mutex<HashMap<Uuid, YoutubeTask>>,
    pub(crate) controls: Mutex<()>,
    shutdown: CancellationToken,
}

impl Default for YoutubeService {
    fn default() -> Self {
        Self {
            root: RwLock::new(None),
            inspections: Mutex::new(HashMap::new()),
            slots: Semaphore::new(2),
            inspection_tasks: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            controls: Mutex::new(()),
            shutdown: CancellationToken::new(),
        }
    }
}

impl YoutubeService {
    /// Called once by the desktop host with Tauri's resolved resource directory.
    pub fn configure(&self, root: PathBuf) {
        *self.root.write().expect("media tools lock") = Some(root);
    }

    fn tools(&self) -> Result<MediaTools, DlmanError> {
        let root = self.root.read().map_err(|_| media_error("Media tools unavailable."))?
            .clone().ok_or_else(|| media_error("Bundled YouTube tools are unavailable. Install a complete DLMan desktop package."))?;
        MediaTools::load(&root)
    }

    pub async fn inspect(
        self: &Arc<Self>,
        value: &str,
        proxy: ProxySettings,
    ) -> Result<YoutubeInspection, DlmanError> {
        let url = canonical_url(value)?;
        let mut records = self.inspections.lock().await;
        records.retain(|_, entry| {
            entry.created.elapsed()
                < Duration::from_secs(if entry.result.status == "pending" {
                    180
                } else {
                    120
                })
        });
        if let Some(record) = records
            .values()
            .find(|entry| entry.url == url && entry.result.status != "failed")
        {
            return Ok(record.result.clone());
        }
        if records.len() >= 64 {
            return Err(media_error(
                "Too many video inspections. Try again shortly.",
            ));
        }
        // Acquire before spawning: waiting HTTP requests must not create an unbounded queue.
        let permit = self.clone().slots_available()?;
        let id = Uuid::new_v4();
        let result = YoutubeInspection {
            id,
            status: "pending".into(),
            preview: None,
            error: None,
        };
        records.insert(
            id,
            InspectionEntry {
                url: url.clone(),
                created: Instant::now(),
                result: result.clone(),
            },
        );
        let service = self.clone();
        let task = tokio::spawn(async move {
            let _permit = permit;
            let inspected = async {
                let tools = service.tools()?;
                let value = tools
                    .metadata(&url, &proxy, service.shutdown.child_token())
                    .await?;
                parse_preview(&url, &value)
            }
            .await;
            if let Some(entry) = service.inspections.lock().await.get_mut(&id) {
                entry.created = Instant::now();
                match inspected {
                    Ok(preview) => {
                        entry.result.status = "ready".into();
                        entry.result.preview = Some(preview);
                    }
                    Err(error) => {
                        entry.result.status = "failed".into();
                        entry.result.error = Some(error.to_string());
                    }
                }
            }
        });
        self.inspection_tasks
            .lock()
            .await
            .retain(|_, task| !task.is_finished());
        self.inspection_tasks.lock().await.insert(id, task);
        Ok(result)
    }

    fn slots_available(self: &Arc<Self>) -> Result<InspectionPermit, DlmanError> {
        // The permit owns the service; this keeps the semaphore alive for the task.
        if self
            .slots
            .try_acquire()
            .map(|permit| permit.forget())
            .is_err()
        {
            return Err(media_error(
                "Two video inspections are already running. Try again shortly.",
            ));
        }
        Ok(InspectionPermit(self.clone()))
    }

    pub async fn inspection(&self, id: Uuid) -> Result<YoutubeInspection, DlmanError> {
        self.inspections
            .lock()
            .await
            .get(&id)
            .filter(|entry| {
                entry.created.elapsed()
                    < Duration::from_secs(if entry.result.status == "pending" {
                        180
                    } else {
                        120
                    })
            })
            .map(|entry| entry.result.clone())
            .ok_or_else(|| media_error("Video preview expired. Inspect the video again."))
    }

    pub async fn validate_preview(
        &self,
        id: Uuid,
        value: &str,
        selection: &YoutubeSelection,
    ) -> Result<YoutubePreview, DlmanError> {
        let preview = self
            .inspection(id)
            .await?
            .preview
            .ok_or_else(|| media_error("Video inspection is not ready."))?;
        if preview.url != canonical_url(value)? || !selection.matches(&preview) {
            return Err(media_error(
                "The selected video format is unavailable. Refresh the quality list.",
            ));
        }
        Ok(preview)
    }

    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let inspections: Vec<_> = self
            .inspection_tasks
            .lock()
            .await
            .drain()
            .map(|(_, task)| task)
            .collect();
        for task in inspections {
            let _ = task.await;
        }
        let tasks: Vec<_> = self
            .tasks
            .lock()
            .await
            .drain()
            .map(|(_, task)| task)
            .collect();
        for task in &tasks {
            task.cancel.cancel();
        }
        for task in tasks {
            let _ = task.join.await;
        }
    }
}

struct InspectionPermit(Arc<YoutubeService>);
impl Drop for InspectionPermit {
    fn drop(&mut self) {
        self.0.slots.add_permits(1);
    }
}

impl DlmanCore {
    pub async fn add_youtube_download(
        &self,
        url: &str,
        inspection_id: Uuid,
        selection: YoutubeSelection,
        destination: PathBuf,
        queue_id: Uuid,
        category_id: Option<Uuid>,
        filename: Option<String>,
        auto_start: bool,
    ) -> Result<Download, DlmanError> {
        let preview = self
            .youtube
            .validate_preview(inspection_id, url, &selection)
            .await?;
        if !destination.is_absolute() {
            return Err(media_error("Choose an absolute destination folder."));
        }
        if self.queue_manager.get_queue(queue_id).await.is_none() {
            return Err(media_error("The selected queue no longer exists."));
        }
        let mut download = Download::new(preview.url, destination, queue_id);
        let chosen =
            filename.unwrap_or_else(|| format!("{}.{}", preview.title, selection.container));
        let chosen = safe_filename(&chosen, &selection.container)?;
        download.filename = self
            .unique_filename(&download.destination, &chosen, None)
            .await;
        download.category_id = category_id;
        download.size = preview
            .variants
            .iter()
            .find(|v| v.id == selection.format_id && v.output_mode == selection.output_mode)
            .and_then(|v| v.size);
        download.status = DownloadStatus::Queued;
        let settings = self.get_settings().await;
        let stage = crate::engine::resolve_segment_cache_dir(
            &settings.temp_storage,
            &download.destination,
            &self.youtube_data_dir,
            download.size,
        )
        .join(format!("youtube-{}", download.id));
        download.youtube = Some(
            serde_json::to_value(YoutubeJob {
                selection,
                stage_dir: stage,
            })
            .map_err(|_| media_error("Cannot persist YouTube selection."))?,
        );
        self.download_manager
            .db()
            .upsert_download(&download)
            .await?;
        self.emit(CoreEvent::DownloadAdded {
            download: download.clone(),
        });
        if auto_start {
            self.start_youtube(download.id).await?;
        }
        Ok(download)
    }

    pub(crate) async fn start_youtube(&self, id: Uuid) -> Result<(), DlmanError> {
        let _control = self.youtube.controls.lock().await;
        let mut tasks = self.youtube.tasks.lock().await;
        if tasks.get(&id).is_some_and(|task| !task.join.is_finished()) {
            return Ok(());
        }
        tasks.remove(&id);
        let mut download = self.get_download(id).await?;
        if download.status == DownloadStatus::Completed {
            return Err(media_error("This download is already complete."));
        }
        let checked = (|| -> Result<YoutubeJob, DlmanError> {
            let job: YoutubeJob =
                serde_json::from_value(download.youtube.clone().ok_or_else(|| {
                    media_error("Missing YouTube selection. Add the video again.")
                })?)
                .map_err(|_| {
                    media_error("Invalid stored YouTube selection. Add the video again.")
                })?;
            canonical_url(&download.url)?;
            if !job.stage_dir.is_absolute()
                || job.stage_dir.file_name().and_then(|v| v.to_str())
                    != Some(format!("youtube-{id}").as_str())
            {
                return Err(media_error("Invalid YouTube staging location."));
            }
            validate_stored_filename(&download.filename, &job.selection.container)?;
            Ok(job)
        })();
        let job = match checked {
            Ok(job) => job,
            Err(error) => {
                let message = error.to_string();
                self.download_manager
                    .db()
                    .update_download_status(id, DownloadStatus::Failed, Some(message.clone()))
                    .await?;
                self.emit(CoreEvent::DownloadStatusChanged {
                    id,
                    status: DownloadStatus::Failed,
                    error: Some(message),
                });
                return Err(error);
            }
        };
        // Claim before any slow extraction so queue advancement sees an occupied slot.
        download.status = DownloadStatus::Downloading;
        download.error = None;
        self.download_manager
            .db()
            .upsert_download(&download)
            .await?;
        self.emit(CoreEvent::DownloadStatusChanged {
            id,
            status: DownloadStatus::Downloading,
            error: None,
        });
        let core = self.clone();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let join = tokio::spawn(async move {
            let result = transfer::run(&core, download, job, token.clone()).await;
            if !token.is_cancelled() {
                if let Err(error) = result {
                    let message = error.to_string();
                    let _ = core
                        .download_manager
                        .db()
                        .update_download_status(id, DownloadStatus::Failed, Some(message.clone()))
                        .await;
                    core.emit(CoreEvent::DownloadStatusChanged {
                        id,
                        status: DownloadStatus::Failed,
                        error: Some(message),
                    });
                }
            } else if let Err(error) = result {
                if !matches!(error, DlmanError::Cancelled) {
                    return Err(error);
                }
            }
            Ok(())
        });
        tasks.insert(id, YoutubeTask { cancel, join });
        Ok(())
    }

    pub(crate) async fn stop_youtube(
        &self,
        id: Uuid,
        status: DownloadStatus,
    ) -> Result<(), DlmanError> {
        let _control = self.youtube.controls.lock().await;
        if let Some(task) = self.youtube.tasks.lock().await.remove(&id) {
            task.cancel.cancel();
            task.join.await.map_err(|_| {
                media_error("YouTube transfer did not stop cleanly; staging was retained.")
            })??;
        }
        // A completion that won the race must not be changed into a pause/cancel.
        if self.get_download(id).await?.status == DownloadStatus::Completed {
            return Ok(());
        }
        self.download_manager
            .db()
            .update_download_status(id, status, None)
            .await?;
        self.emit(CoreEvent::DownloadStatusChanged {
            id,
            status,
            error: None,
        });
        Ok(())
    }
}

fn safe_filename(value: &str, extension: &str) -> Result<String, DlmanError> {
    if !["mp4", "webm", "mkv", "mp3"].contains(&extension) {
        return Err(media_error("Invalid output container."));
    }
    let clean: String = value
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .take(160)
        .collect();
    let mut clean = clean.trim_matches([' ', '.']).to_owned();
    if clean.is_empty() {
        clean = "YouTube video".into();
    }
    if !clean.to_lowercase().ends_with(&format!(".{extension}")) {
        clean.push_str(&format!(".{extension}"));
    }
    if matches!(
        clean
            .split('.')
            .next()
            .unwrap_or("")
            .to_uppercase()
            .as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        clean.insert(0, '_');
    }
    Ok(clean)
}

#[cfg(test)]
mod tests;

fn validate_stored_filename(value: &str, extension: &str) -> Result<(), DlmanError> {
    // Collision suffixes are generated after title truncation. Validate without
    // truncating again so a second download of a long title stays resumable.
    if value.is_empty()
        || value.len() > 1024
        || value
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        || value.trim_matches([' ', '.']) != value
        || !["mp4", "webm", "mkv", "mp3"].contains(&extension)
        || !value.to_lowercase().ends_with(&format!(".{extension}"))
    {
        return Err(media_error("Invalid stored YouTube output filename."));
    }
    Ok(())
}
