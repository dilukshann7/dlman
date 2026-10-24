//! Download task - coordinates multiple segment workers
//!
//! This is the main orchestrator for a single download.
//! It spawns segment workers, monitors their progress, and merges temp files on completion.

use crate::engine::{DownloadDatabase, RateLimiter, SegmentWorker};
use crate::error::DlmanError;
use dlman_types::{CoreEvent, Download, DownloadStatus, Segment, TempStorageSettings};
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tokio::task::JoinSet;

#[path = "segmented.rs"]
mod segmented;

#[cfg(test)]
#[path = "dynamic_tests.rs"]
mod dynamic_tests;
use tracing::{error, info, warn};

/// Resolve the scratch directory for a download's partial segment files,
/// honoring the user's temp-storage policy.
///
/// This is the **single source of truth** for "where do in-progress parts
/// live" — both the active task and the delete/cleanup path go through here, so
/// changing the policy changes behavior everywhere at once. See
/// [`TempStorageSettings`] for the trade-offs behind each mode.
///
/// - `destination` is the download's target folder.
/// - `app_data_dir` is DLMan's per-user data directory (always on the system
///   disk).
/// - `size` is the total download size when known; the `auto` policy uses it to
///   decide whether the system disk can safely hold the scratch data.
///
/// Segment files inside the returned directory are namespaced by download id, so
/// a directory shared across downloads (appdata/custom) is safe for concurrent
/// use.
pub(crate) fn resolve_segment_cache_dir(
    policy: &TempStorageSettings,
    destination: &Path,
    app_data_dir: &Path,
    size: Option<u64>,
) -> PathBuf {
    let appdata_scratch = || app_data_dir.join("temp");
    let destination_scratch = || destination.join(".dlman-cache");

    match policy.mode.as_str() {
        // Keep partial data next to the final file, on the destination
        // filesystem. Best for huge files / external drives — it never fills the
        // system disk and keeps the merge on one volume. (Issue #7)
        "destination" => destination_scratch(),
        // Fast, fixed scratch on the system disk (the pre-1.11 behavior). Best
        // when the destination is a slow drive, e.g. an external HDD. (Issue #10)
        "appdata" => appdata_scratch(),
        // A user-picked folder (e.g. a fast SSD). Falls back to appdata if unset.
        "custom" => match &policy.custom_path {
            Some(p) if !p.as_os_str().is_empty() => p.clone(),
            _ => appdata_scratch(),
        },
        // "auto" (default): prefer the fast system-disk scratch, but fall back to
        // the destination when the system disk can't safely hold the download.
        // This fixes the slow-HDD regression (#10) for typical downloads without
        // reintroducing the system-disk-full failure on huge ones (#7).
        _ => {
            if appdata_has_room_for(app_data_dir, size) {
                appdata_scratch()
            } else {
                destination_scratch()
            }
        }
    }
}

/// Every directory a download's scratch files could plausibly live in under the
/// current policy. Used by the delete path to remove orphaned parts even if the
/// policy (or available free space, for `auto`) changed since the download
/// started. Order/dedup is best-effort; callers treat each as best-effort.
pub(crate) fn candidate_cache_dirs(
    policy: &TempStorageSettings,
    destination: &Path,
    app_data_dir: &Path,
) -> Vec<PathBuf> {
    let mut dirs = vec![
        destination.join(".dlman-cache"),
        app_data_dir.join("temp"),
    ];
    if let Some(p) = &policy.custom_path {
        if !p.as_os_str().is_empty() {
            dirs.push(p.clone());
        }
    }
    dirs.dedup();
    dirs
}

/// Whether the app-data (system disk) volume can comfortably hold `size` bytes
/// of scratch data. Unknown size is treated as "fits" (typical small/medium
/// files); if free space can't be determined we also err toward the system
/// disk, matching the long-standing pre-1.11 default. The `auto` destination
/// fallback exists for the genuinely-too-big case.
fn appdata_has_room_for(app_data_dir: &Path, size: Option<u64>) -> bool {
    let Some(size) = size else {
        return true;
    };
    // Keep ~1 GiB of headroom so a download never wedges the system disk.
    const HEADROOM: u64 = 1024 * 1024 * 1024;
    match fs2::available_space(app_data_dir) {
        Ok(available) => available >= size.saturating_add(HEADROOM),
        Err(_) => true,
    }
}

/// A download task that manages multiple segment workers
pub struct DownloadTask {
    pub download: Download,
    temp_dir: PathBuf,
    client: Client,
    rate_limiter: RateLimiter,
    db: DownloadDatabase,
    event_tx: broadcast::Sender<CoreEvent>,
    paused: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    total_downloaded: Arc<AtomicU64>,
    /// Number of segments to use for multi-segment downloads
    segment_count: u32,
    /// Maximum number of retries for failed segments
    max_retries: u32,
    /// Delay between retries in seconds
    retry_delay_secs: u32,
    /// Optional credentials for authenticated downloads
    credentials: Option<(String, String)>,
}

impl DownloadTask {
    /// Create a new download task.
    ///
    /// `temp_dir` is the resolved scratch directory for this download's partial
    /// segment files (see [`resolve_segment_cache_dir`]). The caller owns the
    /// policy decision so the task itself stays agnostic about *where* scratch
    /// lives.
    pub fn new(
        download: Download,
        temp_dir: PathBuf,
        client: Client,
        rate_limiter: RateLimiter,
        db: DownloadDatabase,
        event_tx: broadcast::Sender<CoreEvent>,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        segment_count: u32,
        max_retries: u32,
        retry_delay_secs: u32,
    ) -> Self {
        Self::new_with_credentials(
            download, temp_dir, client, rate_limiter, db, event_tx,
            paused, cancelled, segment_count, max_retries, retry_delay_secs, None,
        )
    }

    /// Create a new download task with credentials. See [`Self::new`] for the
    /// meaning of `temp_dir`.
    pub fn new_with_credentials(
        download: Download,
        temp_dir: PathBuf,
        client: Client,
        rate_limiter: RateLimiter,
        db: DownloadDatabase,
        event_tx: broadcast::Sender<CoreEvent>,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        segment_count: u32,
        max_retries: u32,
        retry_delay_secs: u32,
        credentials: Option<(String, String)>,
    ) -> Self {
        // Calculate total downloaded from segments if available, otherwise use download.downloaded
        let total_from_segments: u64 = download.segments.iter().map(|s| s.downloaded).sum();
        let initial_downloaded = if total_from_segments > 0 {
            total_from_segments
        } else {
            download.downloaded
        };
        let total_downloaded = Arc::new(AtomicU64::new(initial_downloaded));
        
        Self {
            download,
            temp_dir,
            client,
            rate_limiter,
            db,
            event_tx,
            paused,
            cancelled,
            total_downloaded,
            segment_count,
            max_retries,
            retry_delay_secs,
            credentials,
        }
    }
    
    /// Get the effective URL for downloading segments
    /// Uses the original URL so redirect handling scopes cookies and credentials to the request origin.
    fn effective_url(&self) -> &str {
        &self.download.url
    }
    
    /// Get the paused flag for external control
    pub fn paused(&self) -> Arc<AtomicBool> {
        self.paused.clone()
    }
    
    /// Get the cancelled flag for external control
    pub fn cancelled(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
    
    /// Run the download task
    pub async fn run(mut self) -> Result<(), DlmanError> {
        info!("Starting download task for {}: {} with segment_count={}", 
              self.download.id, self.download.filename, self.segment_count);
        
        // Check for early pause/cancel
        if self.cancelled.load(Ordering::Acquire) {
            self.download.status = DownloadStatus::Cancelled;
            self.db.update_download_status(self.download.id, DownloadStatus::Cancelled, None).await?;
            self.emit_status_change(DownloadStatus::Cancelled, None).await;
            return Ok(());
        }
        if self.paused.load(Ordering::Acquire) {
            self.download.status = DownloadStatus::Paused;
            self.db.update_download_status(self.download.id, DownloadStatus::Paused, None).await?;
            self.emit_status_change(DownloadStatus::Paused, None).await;
            return Ok(());
        }
        
        // Update status to downloading
        self.download.segment_cache_dir = Some(self.temp_dir.clone());
        self.download.status = DownloadStatus::Downloading;
        self.db.update_download_status(self.download.id, DownloadStatus::Downloading, None).await?;
        self.emit_status_change(DownloadStatus::Downloading, None).await;

        // Ensure the scratch directory exists before any segment worker writes
        // to it. Its location is chosen by the user's temp-storage policy (see
        // resolve_segment_cache_dir) and threaded in as self.temp_dir.
        if let Err(e) = tokio::fs::create_dir_all(&self.temp_dir).await {
            error!("Failed to create scratch directory {:?}: {}", self.temp_dir, e);
            return Err(DlmanError::Io(e));
        }

        // Emit initial progress so UI shows current state immediately
        let initial_downloaded = self.total_downloaded.load(Ordering::Acquire);
        let _ = self.event_tx.send(CoreEvent::DownloadProgress {
            id: self.download.id,
            downloaded: initial_downloaded,
            total: self.download.size,
            speed: 0,
            eta: None,
        });
        
        // If no segments, probe URL and initialize them
        if self.download.segments.is_empty() {
            info!("No segments found, initializing...");
            let supports_range = self.probe_url().await?;
            
            // Check for pause/cancel after probe (which might have taken time)
            if self.cancelled.load(Ordering::Acquire) {
                self.download.status = DownloadStatus::Cancelled;
                self.db.update_download_status(self.download.id, DownloadStatus::Cancelled, None).await?;
                self.emit_status_change(DownloadStatus::Cancelled, None).await;
                return Ok(());
            }
            if self.paused.load(Ordering::Acquire) {
                self.download.status = DownloadStatus::Paused;
                self.db.update_download_status(self.download.id, DownloadStatus::Paused, None).await?;
                self.emit_status_change(DownloadStatus::Paused, None).await;
                return Ok(());
            }
            
            if supports_range && self.download.size.unwrap_or(0) > 1024 * 1024 && self.segment_count > 1 {
                // Multi-segment download
                let num_segments = self.segment_count as usize;
                self.download.segments = self.calculate_segments(num_segments);
                info!("Initialized {} segments", num_segments);
            } else {
                // Single segment download (no range support, small file, or segment_count=1)
                let size = self.download.size.unwrap_or(u64::MAX);
                self.download.segments = vec![Segment {
                    index: 0,
                    start: 0,
                    end: if size == u64::MAX || size == 0 { u64::MAX } else { size - 1 },
                    downloaded: 0,
                    complete: false,
                }];
                info!("Initialized single segment (no range support or small file)");
            }
            
            // Save segments to DB
            self.db.save_transfer(&self.download).await?;
            
            // Emit update event so UI gets size and final URL.
            self.publish_range_map().await?;
        }
        
        self.reconcile_segments().await?;

        // Check if all segments are already complete (resuming a finished-but-not-merged download)
        let all_complete = self.download.segments.iter().all(|s| s.complete);
        
        if all_complete {
            info!("All segments already complete, skipping to merge");
        } else {
            // Spawn segment workers for incomplete segments
            let result = if self.download.segments.len() == 1 && !self.can_rebalance() {
                self.download_single_segment().await
            } else {
                self.download_multi_segment().await
            };
            
            // Handle download result (pause/cancel)
            if let Err(e) = result {
                let e = if matches!(e, DlmanError::Paused) && !self.paused.load(Ordering::Acquire)
                    && self.cancelled.load(Ordering::Acquire) { DlmanError::Cancelled } else { e };
                if matches!(e, DlmanError::Paused) {
                    info!("Download paused: {}", self.download.filename);
                    // Status already updated by pause command; persist it here too
                    // for callers that control the task flags directly.
                    self.db.update_download_status(self.download.id, DownloadStatus::Paused, None).await?;
                    self.emit_status_change(DownloadStatus::Paused, None).await;
                    return Ok(());
                } else if matches!(e, DlmanError::Cancelled) {
                    info!("Download cancelled: {}", self.download.filename);
                    self.download.status = DownloadStatus::Cancelled;
                    self.db.update_download_status(self.download.id, DownloadStatus::Cancelled, None).await?;
                    self.emit_status_change(DownloadStatus::Cancelled, None).await;
                    return Ok(());
                } else {
                    // Check if this is an authentication error — emit credential required event
                    if let DlmanError::AuthenticationRequired { ref domain, ref url, status } = e {
                        info!("Download requires authentication for domain: {}", domain);
                        let _ = self.event_tx.send(CoreEvent::CredentialRequired {
                            download_id: self.download.id,
                            domain: domain.clone(),
                            url: url.clone(),
                            status_code: status,
                        });
                    }
                    
                    error!("Download failed: {} - {}", self.download.filename, e);
                    self.download.status = DownloadStatus::Failed;
                    let error_msg = e.to_string();
                    self.download.error = Some(error_msg.clone());
                    self.db.update_download_status(self.download.id, DownloadStatus::Failed, Some(error_msg.clone())).await?;
                    self.emit_status_change(DownloadStatus::Failed, Some(error_msg)).await;
                    return Err(e);
                }
            }
        }
        
        // All segments complete - merge into final file
        info!("All segments complete, merging...");
        let segment_sizes = match self.merge_segments().await {
            Ok(sizes) => sizes,
            Err(DlmanError::Paused) => {
                self.cancelled.store(true, Ordering::Release);
                self.db.update_download_status(self.download.id, DownloadStatus::Paused, None).await?;
                self.emit_status_change(DownloadStatus::Paused, None).await;
                return Ok(());
            }
            Err(DlmanError::Cancelled) => {
                self.db.update_download_status(self.download.id, DownloadStatus::Cancelled, None).await?;
                self.emit_status_change(DownloadStatus::Cancelled, None).await;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        
        // Update segments with actual file sizes and calculate total downloaded
        let mut total_downloaded: u64 = 0;
        for (i, segment) in self.download.segments.iter_mut().enumerate() {
            segment.complete = true;
            // Use the actual file size from merge (this is authoritative)
            if let Some(&actual_size) = segment_sizes.get(i) {
                segment.downloaded = actual_size;
                // For unknown size segments, also fix the end value
                if segment.is_unknown_size() && actual_size > 0 {
                    segment.end = segment.start.saturating_add(actual_size).saturating_sub(1);
                }
            }
            total_downloaded = total_downloaded.saturating_add(segment.downloaded);
        }
        
        // Update download size if it was unknown (discovered during download)
        if self.download.size.is_none() && total_downloaded > 0 {
            self.download.size = Some(total_downloaded);
            info!("Final download size determined: {} bytes", total_downloaded);
        }
        
        // Update status to completed
        self.download.status = DownloadStatus::Completed;
        self.download.completed_at = Some(chrono::Utc::now());
        self.download.downloaded = total_downloaded.max(self.download.size.unwrap_or(0));
        
        // Save the updated download with correct size info
        self.db.save_transfer(&self.download).await?;
        self.db.update_download_status(self.download.id, DownloadStatus::Completed, None).await?;
        self.cleanup_segments().await;
        // Reload metadata edited during transfer before publishing the final snapshot.
        if let Some(saved) = self.db.load_download(self.download.id).await? { self.download = saved; }
        self.emit_status_change(DownloadStatus::Completed, None).await;
        
        // Emit final download update so UI shows all segments complete
        let _ = self.event_tx.send(CoreEvent::DownloadUpdated {
            download: self.download.clone(),
        });
        
        info!("Download completed: {}", self.download.filename);
        Ok(())
    }
    
    /// Download with a single segment
    async fn download_single_segment(&mut self) -> Result<(), DlmanError> {
        let segment = self.download.segments[0].clone();
        
        // Check if already complete
        if segment.complete {
            return Ok(());
        }
        
        // Use final_url if available (after redirects), otherwise use original url
        let url = self.effective_url().to_string();
        
        let worker = SegmentWorker::new_with_credentials(
            self.download.id,
            segment,
            url,
            self.temp_dir.clone(),
            self.client.clone(),
            self.rate_limiter.clone(),
            self.db.clone(),
            self.event_tx.clone(),
            self.paused.clone(),
            self.cancelled.clone(),
            self.total_downloaded.clone(),
            self.credentials.clone(),
            self.download.cookies.clone(),
        ).with_http_context(self.download.size, self.download.etag.clone(), self.download.source_page_url.clone(), super::refresh::expects_html(&self.download.filename));
        
        // Start progress reporter
        let progress_handle = self.spawn_progress_reporter();
        
        // Run segment worker
        let result = worker.run().await;
        
        // Stop progress reporter
        progress_handle.abort();
        let _ = progress_handle.await;
        
        // Handle result - update size if discovered
        match result {
            Ok(segment_result) => {
                self.download.segments = vec![segment_result.segment];
                if let Some(size) = segment_result.discovered_size {
                    self.download.size = Some(size);
                    // Also update the segment end value so it's no longer u64::MAX
                    if !self.download.segments.is_empty() && self.download.segments[0].end == u64::MAX {
                        self.download.segments[0].end = size.saturating_sub(1);
                        self.download.segments[0].downloaded = size;
                    }
                    self.db.save_transfer(&self.download).await?;
                    let _ = self.event_tx.send(CoreEvent::DownloadUpdated {
                        download: self.download.clone(),
                    });
                    info!("Updated download size to {} bytes (discovered during download)", size);
                }
                self.download.downloaded = self.total_downloaded.load(Ordering::Acquire);
                self.db.save_transfer(&self.download).await?;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
    
    /// Spawn a background task to report progress periodically
    fn spawn_progress_reporter(&self) -> tokio::task::JoinHandle<()> {
        let download_id = self.download.id;
        let total_size = self.download.size;
        let total_downloaded = self.total_downloaded.clone();
        let cancelled = self.cancelled.clone();
        let paused = self.paused.clone();
        let event_tx = self.event_tx.clone();
        let db = self.db.clone();
        
        tokio::spawn(async move {
            // Rolling speed calculation with exponential moving average
            // Use a sliding window for more stable measurements
            let mut speed_samples: Vec<f64> = Vec::with_capacity(10);
            let mut last_downloaded = total_downloaded.load(Ordering::Acquire);
            let mut last_time = std::time::Instant::now();
            let mut smoothed_speed: f64 = 0.0;
            let alpha = 0.15; // Lower alpha = smoother speed display (was 0.3)
            let mut last_db_save = std::time::Instant::now();
            
            while !cancelled.load(Ordering::Acquire) {
                // Update every 500ms for smooth UI without flooding
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                
                // Skip if paused
                if paused.load(Ordering::Acquire) {
                    last_time = std::time::Instant::now();
                    last_downloaded = total_downloaded.load(Ordering::Acquire);
                    speed_samples.clear();
                    smoothed_speed = 0.0;
                    continue;
                }
                
                let now = std::time::Instant::now();
                let downloaded = total_downloaded.load(Ordering::Acquire);
                let elapsed = now.duration_since(last_time).as_secs_f64();
                
                // Calculate instant speed
                let instant_speed = if elapsed > 0.0 {
                    (downloaded.saturating_sub(last_downloaded)) as f64 / elapsed
                } else {
                    0.0
                };
                
                // Store sample in sliding window (last 10 samples = 5 seconds)
                speed_samples.push(instant_speed);
                if speed_samples.len() > 10 {
                    speed_samples.remove(0);
                }
                
                // Use windowed average combined with EMA for stability
                let window_avg = if !speed_samples.is_empty() {
                    speed_samples.iter().sum::<f64>() / speed_samples.len() as f64
                } else {
                    instant_speed
                };
                
                // Apply exponential moving average to windowed average for smooth speed
                smoothed_speed = alpha * window_avg + (1.0 - alpha) * smoothed_speed;
                let speed = smoothed_speed as u64;
                
                // Calculate ETA
                let eta = if speed > 0 && total_size.is_some() {
                    let remaining = total_size.unwrap().saturating_sub(downloaded);
                    Some(remaining / speed)
                } else {
                    None
                };
                
                // Emit progress event
                let _ = event_tx.send(CoreEvent::DownloadProgress {
                    id: download_id,
                    downloaded,
                    total: total_size,
                    speed,
                    eta,
                });
                
                // Save to DB every 5 seconds
                if last_db_save.elapsed().as_secs() >= 5 {
                    let _ = db.update_download_progress(download_id, downloaded).await;
                    last_db_save = std::time::Instant::now();
                }
                
                last_downloaded = downloaded;
                last_time = now;
            }
        })
    }
    
    /// Probe URL to determine if range requests are supported
    /// Uses HEAD first, then falls back to partial GET for size/resumability detection
    async fn probe_url(&mut self) -> Result<bool, DlmanError> {
        // Apply credentials if available.
        // Apply browser cookies if available (session-based auth).
        let response = super::refresh::send_scoped(&self.client, reqwest::Method::HEAD,
            &self.download.url, self.download.cookies.as_deref(), self.credentials.as_ref(),
            self.download.source_page_url.as_deref(), None, None).await?;

        // Check for authentication required
        let status = response.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            let domain = url::Url::parse(&self.download.url)
                .ok()
                .and_then(|u| u.host_str().map(|h| h.to_string()))
                .unwrap_or_else(|| "unknown".to_string());
            
            return Err(DlmanError::AuthenticationRequired {
                domain,
                url: self.download.url.clone(),
                status: status.as_u16(),
            });
        }
        
        if response.status().is_success() { super::refresh::validate_response_for_file(&response, super::refresh::expects_html(&self.download.filename))?; }
        self.download.etag = response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
        self.download.last_modified = response.headers().get(reqwest::header::LAST_MODIFIED).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let mut supports_range = response
            .headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .and_then(|v| v.to_str().ok())
            .map(|s| s == "bytes")
            .unwrap_or(false);
        
        // Update size if we don't have it
        if self.download.size.is_none() && response.status().is_success() {
            self.download.size = response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok());
        }
        
        // Update final URL if redirected
        let final_url = response.url().to_string();
        if final_url != self.download.url {
            self.download.final_url = Some(final_url.clone());
        }
        
        // If HEAD didn't give us size, try a partial GET request
        // This is critical for GitHub releases and similar CDNs
        if self.download.size.is_none() || !response.status().is_success() {
            let probe_url = &self.download.url;
            info!("HEAD didn't return Content-Length, trying partial GET on {}", probe_url);
            
            // Apply cookies to the fallback probe too, with redirect scoping.
            match super::refresh::send_scoped(&self.client, reqwest::Method::GET, probe_url,
                self.download.cookies.as_deref(), self.credentials.as_ref(),
                self.download.source_page_url.as_deref(), Some("bytes=0-0"), None).await {
                Ok(range_response) => {
                    super::refresh::validate_response_for_file(&range_response, super::refresh::expects_html(&self.download.filename))?;
                    self.download.final_url = Some(range_response.url().to_string());
                    self.download.etag = range_response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
                    self.download.last_modified = range_response.headers().get(reqwest::header::LAST_MODIFIED).and_then(|v| v.to_str().ok()).map(str::to_owned);
                    let status = range_response.status();
                    info!("Partial GET status: {}", status);
                    
                    // Check for 206 Partial Content - means range is supported
                    if status == reqwest::StatusCode::PARTIAL_CONTENT {
                        supports_range = true;
                        
                        // Parse Content-Range header for total size: "bytes 0-0/12345"
                        if let Some(content_range) = range_response.headers().get(reqwest::header::CONTENT_RANGE) {
                            if let Ok(range_str) = content_range.to_str() {
                                info!("Content-Range: {}", range_str);
                                if let Some(total) = range_str.split('/').last() {
                                    if total != "*" { // "*" means unknown size
                                        if let Ok(total_size) = total.parse::<u64>() {
                                            self.download.size = Some(total_size);
                                            info!("Got size from Content-Range: {} bytes", total_size);
                                        }
                                    }
                                }
                            }
                        }
                    } else if status == reqwest::StatusCode::OK {
                        // Server ignored range and sent full content - get size from Content-Length
                        if let Some(size) = range_response.headers()
                            .get(reqwest::header::CONTENT_LENGTH)
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse().ok())
                        {
                            self.download.size = Some(size);
                            info!("Got size from full GET response: {} bytes", size);
                        }
                        // Range not supported, we'll do single-segment
                        supports_range = false;
                    }
                }
                Err(e) => {
                    warn!("Partial GET probe failed: {} - continuing without size info", e);
                }
            }
        }
        
        self.download.supports_range = supports_range;
        Ok(supports_range)
    }
    
    /// Calculate segments for multi-segment download
    fn calculate_segments(&self, num_segments: usize) -> Vec<Segment> {
        let total_size = self.download.size.unwrap_or(0);
        
        if num_segments <= 1 || total_size < 1024 * 1024 {
            return vec![Segment {
                index: 0,
                start: 0,
                end: total_size.saturating_sub(1),
                downloaded: 0,
                complete: false,
            }];
        }
        
        let segment_size = total_size / num_segments as u64;
        let mut segments = Vec::new();
        
        for i in 0..num_segments {
            let start = i as u64 * segment_size;
            let end = if i == num_segments - 1 {
                total_size.saturating_sub(1)
            } else {
                (i as u64 + 1) * segment_size - 1
            };
            
            segments.push(Segment {
                index: i as u32,
                start,
                end,
                downloaded: 0,
                complete: false,
            });
        }
        
        segments
    }
    
    /// Reconcile retained files before counters, scheduling, or merge use them.
    async fn reconcile_segments(&mut self) -> Result<(), DlmanError> {
        super::segmentation::validate_map(&self.download.segments, self.download.size, false)?;
        let mut total = 0u64;
        for segment in &mut self.download.segments {
            let path = self.temp_dir.join(format!("{}_segment_{}.part", self.download.id, segment.index));
            let length = match tokio::fs::metadata(&path).await {
                Ok(metadata) if metadata.is_file() => metadata.len(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
                    && segment.downloaded == 0 && !segment.complete => 0,
                _ => return Err(super::refresh::invalid("Required partial segment files are missing or unreadable.")),
            };
            if length < segment.downloaded || (!segment.is_unknown_size() && length > segment.size()) {
                return Err(super::refresh::invalid("Partial segment length is inconsistent; file was preserved."));
            }
            segment.downloaded = length;
            if !segment.is_unknown_size() {
                segment.complete = length == segment.size();
            }
            self.db.update_segment_progress(self.download.id, segment.index, length, segment.complete).await?;
            total = total.checked_add(length).ok_or_else(|| super::refresh::invalid("Invalid partial segment size."))?;
        }
        self.download.downloaded = total;
        self.total_downloaded.store(total, Ordering::Release);
        self.db.update_download_progress(self.download.id, total).await?;
        Ok(())
    }

    /// Merge all segment temp files into the final file.
    /// Returns actual sizes in the same byte order as self.download.segments.
    async fn merge_segments(&mut self) -> Result<Vec<u64>, DlmanError> {
        let result = self.assemble_segments().await;
        if result.is_err() {
            let staging = self.download.destination.join(format!(".{}.assembling", self.download.id));
            let _ = tokio::fs::remove_file(staging).await;
        }
        result
    }

    async fn assemble_segments(&mut self) -> Result<Vec<u64>, DlmanError> {
        self.download.segments.sort_by_key(|segment| segment.start);
        super::segmentation::validate_map(&self.download.segments, self.download.size, true)?;
        let final_path = self.download.destination.join(&self.download.filename);
        let mut segment_sizes = Vec::with_capacity(self.download.segments.len());
        for segment in &self.download.segments {
            let path = self.temp_dir.join(format!("{}_segment_{}.part", self.download.id, segment.index));
            let length = tokio::fs::metadata(path).await?.len();
            if length != segment.downloaded || (!segment.is_unknown_size() && length != segment.size()) {
                return Err(super::refresh::invalid("Partial segment length is inconsistent; file was preserved."));
            }
            segment_sizes.push(length);
        }
        tokio::fs::create_dir_all(&self.download.destination).await?;
        // Staging is on the destination volume. Its unique name cannot collide
        // with another download, and no partial is removed until publication.
        let staging = self.download.destination.join(format!(".{}.assembling", self.download.id));
        let mut output = OpenOptions::new().create(true).write(true).truncate(true).open(&staging).await?;
        for segment in &self.download.segments {
            self.check_assembly_control()?;
            let path = self.temp_dir.join(format!("{}_segment_{}.part", self.download.id, segment.index));
            let mut input = File::open(path).await?;
            let mut buffer = vec![0u8; 1024 * 1024]; // 1MB buffer
            let mut copied = 0u64;
            loop {
                self.check_assembly_control()?;
                let length = input.read(&mut buffer).await?;
                if length == 0 { break; }
                output.write_all(&buffer[..length]).await?;
                copied += length as u64;
            }
            if copied != segment.downloaded {
                return Err(super::refresh::invalid("Partial segment changed during assembly; files were preserved."));
            }
        }
        output.flush().await?;
        output.sync_all().await?;
        drop(output);
        self.check_assembly_control()?;
        tokio::fs::rename(&staging, &final_path).await?;
        Ok(segment_sizes)
    }

    async fn cleanup_segments(&self) {
        for segment in &self.download.segments {
            let path = self.temp_dir.join(format!("{}_segment_{}.part", self.download.id, segment.index));
            if let Err(error) = tokio::fs::remove_file(&path).await {
                warn!("Failed to remove temp file {:?}: {}", path, error);
            }
        }
        // Best-effort: remove the scratch dir now that this download's parts are
        // merged. remove_dir only succeeds if empty, so concurrent downloads
        // sharing the destination's cache are left untouched.
        let _ = tokio::fs::remove_dir(&self.temp_dir).await;
    }

    fn check_assembly_control(&self) -> Result<(), DlmanError> {
        if self.paused.load(Ordering::Acquire) {
            self.cancelled.store(true, Ordering::Release);
            return Err(DlmanError::Paused);
        }
        if self.cancelled.load(Ordering::Acquire) { return Err(DlmanError::Cancelled); }
        Ok(())
    }

    /// Pause the download
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
        info!("Download {} paused", self.download.id);
    }
    
    /// Resume the download
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
        info!("Download {} resumed", self.download.id);
    }
    
    /// Cancel the download
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        info!("Download {} cancelled", self.download.id);
    }
    
    /// Check if paused
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
    
    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    
    /// Emit status change event
    async fn emit_status_change(&self, status: DownloadStatus, error: Option<String>) {
        let _ = self.event_tx.send(CoreEvent::DownloadStatusChanged {
            id: self.download.id,
            status,
            error,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: &str, custom: Option<&str>) -> TempStorageSettings {
        TempStorageSettings {
            mode: mode.to_string(),
            custom_path: custom.map(PathBuf::from),
        }
    }

    #[test]
    fn destination_mode_uses_hidden_dir_beside_file() {
        let dir = resolve_segment_cache_dir(
            &policy("destination", None),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
            Some(10),
        );
        assert_eq!(dir, PathBuf::from("/mnt/usb/Movies/.dlman-cache"));
    }

    #[test]
    fn appdata_mode_uses_system_temp() {
        let dir = resolve_segment_cache_dir(
            &policy("appdata", None),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
            Some(10),
        );
        assert_eq!(dir, PathBuf::from("/home/u/.local/share/dlman/temp"));
    }

    #[test]
    fn custom_mode_uses_chosen_dir_but_falls_back_when_empty() {
        let dir = resolve_segment_cache_dir(
            &policy("custom", Some("/fast/ssd/scratch")),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
            None,
        );
        assert_eq!(dir, PathBuf::from("/fast/ssd/scratch"));

        let fallback = resolve_segment_cache_dir(
            &policy("custom", None),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
            None,
        );
        assert_eq!(fallback, PathBuf::from("/home/u/.local/share/dlman/temp"));
    }

    #[test]
    fn auto_uses_appdata_when_size_unknown() {
        // Unknown size is treated as "fits" → fast system-disk scratch.
        let dir = resolve_segment_cache_dir(
            &policy("auto", None),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
            None,
        );
        assert_eq!(dir, PathBuf::from("/home/u/.local/share/dlman/temp"));
    }

    #[test]
    fn unknown_mode_is_treated_as_auto() {
        let dir = resolve_segment_cache_dir(
            &policy("something-else", None),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
            None,
        );
        assert_eq!(dir, PathBuf::from("/home/u/.local/share/dlman/temp"));
    }

    #[test]
    fn candidate_dirs_cover_destination_appdata_and_custom() {
        let dirs = candidate_cache_dirs(
            &policy("custom", Some("/fast/ssd/scratch")),
            Path::new("/mnt/usb/Movies"),
            Path::new("/home/u/.local/share/dlman"),
        );
        assert!(dirs.contains(&PathBuf::from("/mnt/usb/Movies/.dlman-cache")));
        assert!(dirs.contains(&PathBuf::from("/home/u/.local/share/dlman/temp")));
        assert!(dirs.contains(&PathBuf::from("/fast/ssd/scratch")));
    }
}
