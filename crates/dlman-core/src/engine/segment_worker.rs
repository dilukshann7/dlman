//! Segment worker - downloads a single segment to a temporary file
//!
//! Each segment worker is independent and writes to its own temp file.
//! On completion, all segment files are merged into the final file.

use crate::engine::rate_limiter::RateLimiter;
use crate::engine::persistence::DownloadDatabase;
use crate::error::DlmanError;
use super::refresh::{invalid, parse_content_range, send_scoped, strong_etag, validate_response_for_file};
use super::segmentation::{split_tail, SplitCommand, WorkerSnapshot};
use dlman_types::{CoreEvent, Segment};
use futures::StreamExt;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt, BufWriter};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

#[cfg(test)]
#[path = "segment_worker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "segment_write_bench.rs"]
mod write_bench;

// Tuned by the explicitly ignored disk benchmark in segment_write_bench.rs.
const SEGMENT_WRITE_BUFFER_SIZE: usize = 512 * 1024;
type SegmentFile = BufWriter<tokio::fs::File>;

fn buffered_segment_file(file: tokio::fs::File) -> SegmentFile {
    BufWriter::with_capacity(SEGMENT_WRITE_BUFFER_SIZE, file)
}

/// Result of a segment download
pub struct SegmentResult {
    /// Final ownership and progress, including any acknowledged splits.
    pub segment: Segment,
    /// Path to the downloaded segment file
    pub path: PathBuf,
    /// Total size discovered during download (if previously unknown)
    pub discovered_size: Option<u64>,
}

/// A segment worker that downloads a byte range to a temporary file
pub struct SegmentWorker {
    download_id: Uuid,
    segment: Segment,
    url: String,
    temp_file_path: PathBuf,
    client: Client,
    rate_limiter: RateLimiter,
    db: DownloadDatabase,
    event_tx: broadcast::Sender<CoreEvent>,
    paused: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    downloaded_bytes: Arc<AtomicU64>,
    /// Optional credentials for authenticated downloads
    credentials: Option<(String, String)>, // (username, password)
    /// Optional browser cookies for session-based authentication
    cookies: Option<String>,
    expected_total: Option<u64>,
    etag: Option<String>,
    referrer: Option<String>,
    allow_html: bool,
    commands: Option<mpsc::Receiver<SplitCommand>>,
    snapshot: Option<watch::Sender<WorkerSnapshot>>,
    stop: CancellationToken,
    ready: bool,
}

impl SegmentWorker {
    /// Create a new segment worker
    pub fn new(
        download_id: Uuid,
        segment: Segment,
        url: String,
        temp_dir: PathBuf,
        client: Client,
        rate_limiter: RateLimiter,
        db: DownloadDatabase,
        event_tx: broadcast::Sender<CoreEvent>,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        downloaded_bytes: Arc<AtomicU64>,
    ) -> Self {
        let temp_file_path = temp_dir.join(format!(
            "{}_segment_{}.part",
            download_id, segment.index
        ));
        
        Self {
            download_id,
            segment,
            url,
            temp_file_path,
            client,
            rate_limiter,
            db,
            event_tx,
            paused,
            cancelled,
            downloaded_bytes,
            credentials: None,
            cookies: None,
            expected_total: None,
            etag: None,
            referrer: None,
            allow_html: false,
            commands: None,
            snapshot: None,
            stop: CancellationToken::new(),
            ready: false,
        }
    }
    
    /// Create a new segment worker with credentials and cookies
    pub fn new_with_credentials(
        download_id: Uuid,
        segment: Segment,
        url: String,
        temp_dir: PathBuf,
        client: Client,
        rate_limiter: RateLimiter,
        db: DownloadDatabase,
        event_tx: broadcast::Sender<CoreEvent>,
        paused: Arc<AtomicBool>,
        cancelled: Arc<AtomicBool>,
        downloaded_bytes: Arc<AtomicU64>,
        credentials: Option<(String, String)>,
        cookies: Option<String>,
    ) -> Self {
        let temp_file_path = temp_dir.join(format!(
            "{}_segment_{}.part",
            download_id, segment.index
        ));
        
        Self {
            download_id,
            segment,
            url,
            temp_file_path,
            client,
            rate_limiter,
            db,
            event_tx,
            paused,
            cancelled,
            downloaded_bytes,
            credentials,
            cookies,
            expected_total: None,
            etag: None,
            referrer: None,
            allow_html: false,
            commands: None,
            snapshot: None,
            stop: CancellationToken::new(),
            ready: false,
        }
    }
    
    pub fn with_http_context(mut self, total: Option<u64>, etag: Option<String>, referrer: Option<String>, allow_html: bool) -> Self {
        self.expected_total = total;
        self.etag = etag;
        self.referrer = referrer;
        self.allow_html = allow_html;
        self
    }

    pub(super) fn with_control(
        mut self,
        commands: mpsc::Receiver<SplitCommand>,
        snapshot: watch::Sender<WorkerSnapshot>,
        stop: CancellationToken,
    ) -> Self {
        self.commands = Some(commands);
        self.snapshot = Some(snapshot);
        self.stop = stop;
        self
    }

    fn publish_snapshot(&self) {
        if let Some(snapshot) = &self.snapshot {
            snapshot.send_replace(WorkerSnapshot { segment: self.segment.clone(), ready: self.ready });
        }
    }

    fn stop_error(&self) -> Option<DlmanError> {
        if self.paused.load(Ordering::Acquire) {
            // The manager must reject resume once this worker begins draining.
            // Before this point clearing `paused` still resumes the live request.
            self.cancelled.store(true, Ordering::Release);
            Some(DlmanError::Paused)
        } else if self.cancelled.load(Ordering::Acquire) || self.stop.is_cancelled() {
            Some(DlmanError::Cancelled)
        } else {
            None
        }
    }

    async fn checkpoint(&self, file: &mut SegmentFile) -> Result<(), DlmanError> {
        file.flush().await?;
        file.get_ref().sync_all().await?;
        self.save_progress().await
    }

    async fn complete_segment(&mut self, file: &mut SegmentFile) -> Result<(), DlmanError> {
        // Completion becomes visible only after all accepted bytes are durable.
        file.flush().await?;
        file.get_ref().sync_all().await?;
        self.segment.complete = true;
        self.save_progress().await
    }

    async fn handle_split(
        &mut self,
        file: &mut SegmentFile,
        command: SplitCommand,
        rollback_checkpoint: &mut u64,
    ) -> Result<(), DlmanError> {
        let result = async {
            if !self.ready || self.stop_error().is_some() {
                return Ok(None);
            }
            let Some((donor, child)) = split_tail(&self.segment, command.child_index) else {
                return Ok(None);
            };
            // This is a write boundary: no bytes can cross into the child while
            // the donor checkpoint and its new ownership map are committed.
            file.flush().await?;
            file.get_ref().sync_all().await?;
            self.db.split_segment(self.download_id, &self.segment, &donor, &child).await?;
            self.segment = donor.clone();
            *rollback_checkpoint = donor.downloaded;
            self.publish_snapshot();
            Ok(Some((donor, child)))
        }.await;
        // A lost acknowledgement cannot undo a committed split. The coordinator
        // drains workers and reloads the persistent map on every terminal path.
        match result {
            // Disk failures must terminate the worker. Dropping the reply lets
            // the coordinator drain it through the ordinary worker-error path.
            Err(error @ DlmanError::Io(_)) => Err(error),
            result => {
                let _ = command.reply.send(result);
                Ok(())
            }
        }
    }

    async fn rollback_attempt(&mut self, file: &mut SegmentFile, original: u64) -> Result<(), DlmanError> {
        // Drain before truncating so rejected bytes cannot be flushed back later.
        file.flush().await?;
        file.get_mut().set_len(original).await?;
        file.seek(std::io::SeekFrom::Start(original)).await?;
        self.downloaded_bytes.fetch_sub(self.segment.downloaded - original, Ordering::AcqRel);
        self.segment.downloaded = original;
        self.checkpoint(file).await?;
        self.publish_snapshot();
        Ok(())
    }

    async fn checkpoint_interrupted(
        &mut self,
        file: &mut SegmentFile,
        original: u64,
    ) -> Result<(), DlmanError> {
        // Reconciliation treats a full-length file as complete. Do not let a
        // response that failed before validated EOF enter that path.
        if !self.segment.is_unknown_size() && self.segment.downloaded == self.segment.size() {
            self.rollback_attempt(file, original).await
        } else {
            self.checkpoint(file).await?;
            self.publish_snapshot();
            Ok(())
        }
    }

    /// Run the segment download
    pub async fn run(mut self) -> Result<SegmentResult, DlmanError> {
        let result = self.run_inner().await;
        self.ready = false;
        self.publish_snapshot();
        result
    }

    async fn run_inner(&mut self) -> Result<SegmentResult, DlmanError> {
        info!(
            "Starting segment {} for download {} (bytes {}-{})",
            self.segment.index, self.download_id, self.segment.start, self.segment.end
        );
        
        // Track if we discover the total size during download
        let mut discovered_size: Option<u64> = None;
        if self.segment.end < self.segment.start { return Err(invalid("Saved segment boundaries are inconsistent.")); }
        
        // Check if segment is already complete
        if self.segment.complete {
            info!("Segment {} already complete", self.segment.index);
            return Ok(SegmentResult {
                segment: self.segment.clone(),
                path: self.temp_file_path.clone(),
                discovered_size: None,
            });
        }
        
        // Open or create temp file
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .read(true)
            .open(&self.temp_file_path)
            .await?;
        
        // Check existing file size for resume
        // For unknown size segments, we always resume from whatever we have
        let existing_size = file.metadata().await?.len();
        if existing_size < self.segment.downloaded || (!self.segment.is_unknown_size() && existing_size > self.segment.size()) {
            return Err(invalid("Partial segment length is inconsistent; file was preserved."));
        }
        if existing_size > self.segment.downloaded {
            self.downloaded_bytes.fetch_add(existing_size - self.segment.downloaded, Ordering::AcqRel);
        }
        if existing_size > 0 {
            // For unknown size, always resume; for known size, only if within bounds
            if self.segment.is_unknown_size() || existing_size <= self.segment.size() {
                self.segment.downloaded = existing_size;
                file.seek(std::io::SeekFrom::Start(existing_size)).await?;
                info!(
                    "Resuming segment {} from byte {}",
                    self.segment.index, existing_size
                );
            }
        }
        let mut file = buffered_segment_file(file);
        // Reconciliation is normally done by the coordinator, but standalone
        // workers also account for bytes written after the last saved checkpoint.
        self.publish_snapshot();
        let mut rollback_checkpoint = existing_size;
        let mut control_tick = tokio::time::interval(std::time::Duration::from_millis(100));
        if let Some(error) = self.stop_error() {
            self.checkpoint(&mut file).await?;
            return Err(error);
        }
        
        // Calculate actual byte range to download
        let start_byte = self.segment.start.checked_add(self.segment.downloaded).ok_or_else(|| invalid("Invalid segment offset."))?;
        let end_byte = self.segment.end;
        let unknown_size = end_byte == u64::MAX;
        
        // For unknown size, we don't know if we're done until the stream ends
        if !unknown_size && start_byte > end_byte {
            // Already complete
            self.segment.complete = true;
            self.db
                .update_segment_progress(
                    self.download_id,
                    self.segment.index,
                    self.segment.downloaded,
                    true,
                )
                .await?;
            return Ok(SegmentResult {
                segment: self.segment.clone(),
                path: self.temp_file_path.clone(),
                discovered_size: None,
            });
        }
        
        // Build HTTP request
        // For unknown size (end = MAX), use open-ended range "bytes=N-"
        // For known size, use "bytes=N-M"
        let range = if unknown_size {
            if start_byte == 0 { None } else { Some(format!("bytes={start_byte}-")) }
        } else { Some(format!("bytes={start_byte}-{end_byte}")) };
        // Apply credentials if available (HTTP Basic Auth).
        // Apply browser cookies if available (session-based auth).
        // Redirect handling scopes both to their original origin.
        let mut verification_etag = self.etag.clone();
        let mut verified_total = None;
        let mut sampled = false;
        let (response, expected_body) = loop {
            let response = {
                let request = send_scoped(
                    &self.client,
                    reqwest::Method::GET,
                    &self.url,
                    self.cookies.as_deref(),
                    self.credentials.as_ref(),
                    self.referrer.as_deref(),
                    range.as_deref(),
                    strong_etag(verification_etag.as_deref()),
                );
                tokio::pin!(request);
                loop {
                    tokio::select! {
                        result = &mut request => break result?,
                        _ = control_tick.tick() => {
                            if let Some(error) = self.stop_error() {
                                self.checkpoint(&mut file).await?;
                                return Err(error);
                            }
                        }
                    }
                }
            };

            // Check response status
            let status = response.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                // Authentication required - extract domain for credential lookup
                let domain = url::Url::parse(&self.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_string()))
                    .unwrap_or_else(|| "unknown".to_string());

                return Err(DlmanError::AuthenticationRequired {
                    domain,
                    url: self.url.clone(),
                    status: status.as_u16(),
                });
            }
            if !status.is_success() && status.as_u16() != 206 {
                return Err(DlmanError::ServerError {
                    status: status.as_u16(),
                    message: format!("Failed to download segment {}", self.segment.index),
                });
            }

            validate_response_for_file(&response, self.allow_html)?;
            let response_etag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            if let (Some(expected), Some(actual)) = (
                strong_etag(verification_etag.as_deref()),
                strong_etag(
                    response
                        .headers()
                        .get(reqwest::header::ETAG)
                        .and_then(|v| v.to_str().ok()),
                ),
            ) {
                if expected != actual {
                    return Err(invalid(
                        "Download resource changed; partial data was preserved.",
                    ));
                }
            }
            let expected_body;
            let mut response_total = None;
            if status == reqwest::StatusCode::PARTIAL_CONTENT {
                // Try Content-Range first (for 206 responses): "bytes 0-X/12345".
                let value = response
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| invalid("Missing Content-Range; partial data was preserved."))?;
                let (actual_start, actual_end, total) = parse_content_range(value)?;
                if actual_start != start_byte
                    || (!unknown_size && actual_end != end_byte)
                    || (unknown_size && actual_end != total - 1)
                    || self
                        .expected_total
                        .is_some_and(|expected| expected != total)
                    || verified_total.is_some_and(|expected| expected != total)
                {
                    return Err(invalid(
                        "Server returned a different byte range or file size.",
                    ));
                }
                expected_body = Some(actual_end - actual_start + 1);
                response_total = Some(total);
                if unknown_size {
                    self.segment.end = total - 1;
                    discovered_size = Some(total);
                }
            } else if status == reqwest::StatusCode::OK
                && start_byte == 0
                && self.segment.start == 0
            {
                // Try Content-Length for 200 OK responses (full content).
                // A full response is safe only for a fresh, whole-file segment.
                let length = response.content_length();
                if !unknown_size
                    && (length != Some(self.segment.size())
                        || self
                            .expected_total
                            .is_some_and(|total| total != self.segment.size()))
                {
                    return Err(invalid(
                        "Server ignored a segmented range; partial data was preserved.",
                    ));
                }
                expected_body = length;
                if unknown_size {
                    if let Some(total) = length.filter(|n| *n > 0) {
                        self.segment.end = total - 1;
                        discovered_size = Some(total);
                    }
                }
            } else {
                return Err(invalid(
                    "Server ignored a resume range; partial data was preserved.",
                ));
            }
            let confirmed_etag = strong_etag(verification_etag.as_deref())
                .zip(strong_etag(response_etag.as_deref()))
                .is_some_and(|(expected, actual)| expected == actual);
            if existing_size > 0 && !sampled && !confirmed_etag {
                let total = response_total
                    .ok_or_else(|| invalid("Cannot verify the resumed file size."))?;
                // Release the transfer response before sample requests so verification
                // uses this worker's connection slot. Reopen only after verification.
                drop(response);
                let context = super::refresh::PartialVerification {
                    client: &self.client,
                    url: &self.url,
                    cookies: self.cookies.as_deref(),
                    credentials: self.credentials.as_ref(),
                    referrer: self.referrer.as_deref(),
                    allow_html: self.allow_html,
                };
                let sample_etag = strong_etag(verification_etag.as_deref())
                    .or_else(|| strong_etag(response_etag.as_deref()))
                    .map(str::to_owned);
                let verification =
                    context.verify(&self.temp_file_path, &self.segment, total, sample_etag);
                tokio::pin!(verification);
                verification_etag = loop {
                    tokio::select! {
                        result = &mut verification => break result?,
                        _ = control_tick.tick() => {
                            if let Some(error) = self.stop_error() {
                                self.checkpoint(&mut file).await?;
                                return Err(error);
                            }
                        }
                    }
                };
                verified_total = Some(total);
                sampled = true;
                continue;
            }
            break (response, expected_body);
        };
        if let Some(total) = discovered_size {
            self.db.record_discovered_size(self.download_id, self.segment.index, total).await?;
            self.expected_total = Some(total);
        }
        self.ready = !unknown_size && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
        self.publish_snapshot();
        let mut received = 0u64;

        // Stream and write chunks
        let mut stream = response.bytes_stream();
        let mut last_db_update = tokio::time::Instant::now();
        let mut last_event_emit = tokio::time::Instant::now();
        
        loop {
            let chunk_result = tokio::select! {
                _ = control_tick.tick() => {
                    if let Some(error) = self.stop_error() {
                        self.checkpoint_interrupted(&mut file, rollback_checkpoint).await?;
                        return Err(error);
                    }
                    continue;
                }
                Some(command) = async {
                    match &mut self.commands {
                        Some(commands) => commands.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.handle_split(&mut file, command, &mut rollback_checkpoint).await?;
                    continue;
                }
                chunk = stream.next() => match chunk {
                    Some(chunk) => chunk,
                    None => break,
                },
            };
            // Check cancellation and pause before processing another chunk.
            // Pausing saves progress and closes the response for a later resume.
            if let Some(error) = self.stop_error() {
                self.checkpoint_interrupted(&mut file, rollback_checkpoint).await?;
                return Err(error);
            }

            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(error) => {
                    self.checkpoint_interrupted(&mut file, rollback_checkpoint).await?;
                    return Err(DlmanError::Network(error.without_url()));
                }
            };
            let chunk_len = chunk.len() as u64;
            if expected_body.is_some_and(|expected| chunk_len > expected.saturating_sub(received)) {
                self.rollback_attempt(&mut file, rollback_checkpoint).await?;
                return Err(invalid("Server sent more bytes than the requested segment."));
            }
            received += chunk_len;
            
            // Apply rate limiting
            let limiter = self.rate_limiter.clone();
            let acquire = limiter.acquire(chunk_len);
            tokio::pin!(acquire);
            loop {
                tokio::select! {
                    _ = &mut acquire => break,
                    _ = control_tick.tick() => {
                        if let Some(error) = self.stop_error() {
                            self.checkpoint_interrupted(&mut file, rollback_checkpoint).await?;
                            return Err(error);
                        }
                    }
                    Some(command) = async {
                        match &mut self.commands {
                            Some(commands) => commands.recv().await,
                            None => std::future::pending().await,
                        }
                    } => self.handle_split(&mut file, command, &mut rollback_checkpoint).await?,
                }
            }
            if let Some(error) = self.stop_error() {
                self.checkpoint_interrupted(&mut file, rollback_checkpoint).await?;
                return Err(error);
            }
            let accepted = if self.segment.is_unknown_size() {
                chunk_len
            } else {
                chunk_len.min(self.segment.size().saturating_sub(self.segment.downloaded))
            };
            
            // Write to temp file
            file.write_all(&chunk[..accepted as usize]).await?;
            
            // Update progress atomically
            self.segment.downloaded += accepted;
            self.downloaded_bytes.fetch_add(accepted, Ordering::AcqRel);
            self.publish_snapshot();
            
            // Emit segment progress event every 300ms (throttled, not per-chunk)
            if last_event_emit.elapsed().as_millis() >= 300 {
                let _ = self.event_tx.send(CoreEvent::SegmentProgress {
                    download_id: self.download_id,
                    segment_index: self.segment.index,
                    downloaded: self.segment.downloaded,
                });
                last_event_emit = tokio::time::Instant::now();
            }
            
            // Periodically save to database (every 2 seconds)
            if last_db_update.elapsed().as_secs() >= 2 {
                self.checkpoint(&mut file).await?;
                last_db_update = tokio::time::Instant::now();
            }
            // Only an acknowledged shortening permits early completion. An
            // unchanged response must still reach EOF for body validation.
            if !unknown_size && self.segment.end < end_byte && self.segment.downloaded == self.segment.size() {
                break;
            }
        }
        
        let shortened_complete = !unknown_size && self.segment.end < end_byte
            && self.segment.downloaded == self.segment.size();
        if !shortened_complete && expected_body.is_some_and(|expected| received != expected) {
            self.checkpoint_interrupted(&mut file, rollback_checkpoint).await?;
            return Err(DlmanError::IncompleteTransfer);
        }

        // Flush and sync to disk
        // Mark segment as complete
        self.complete_segment(&mut file).await?;
        
        info!(
            "Segment {} complete ({} bytes)",
            self.segment.index, self.segment.downloaded
        );
        
        Ok(SegmentResult {
            segment: self.segment.clone(),
            path: self.temp_file_path.clone(),
            discovered_size,
        })
    }
    
    /// Save progress to database
    async fn save_progress(&self) -> Result<(), DlmanError> {
        self.db
            .update_segment_progress(
                self.download_id,
                self.segment.index,
                self.segment.downloaded,
                self.segment.complete,
            )
            .await?;
        
        Ok(())
    }
}
