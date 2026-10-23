//! Segment worker - downloads a single segment to a temporary file
//!
//! Each segment worker is independent and writes to its own temp file.
//! On completion, all segment files are merged into the final file.

use crate::engine::rate_limiter::RateLimiter;
use crate::engine::persistence::DownloadDatabase;
use crate::error::DlmanError;
use super::refresh::{invalid, parse_content_range, send_scoped, strong_etag, validate_response_for_file};
use dlman_types::{CoreEvent, Segment};
use futures::StreamExt;
use reqwest::Client;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::broadcast;
use tracing::{debug, info};
use uuid::Uuid;

/// Result of a segment download
pub struct SegmentResult {
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
        }
    }
    
    pub fn with_http_context(mut self, total: Option<u64>, etag: Option<String>, referrer: Option<String>, allow_html: bool) -> Self {
        self.expected_total = total;
        self.etag = etag;
        self.referrer = referrer;
        self.allow_html = allow_html;
        self
    }

    async fn rollback_attempt(&mut self, file: &mut tokio::fs::File, original: u64) -> Result<(), DlmanError> {
        file.set_len(original).await?;
        self.downloaded_bytes.fetch_sub(self.segment.downloaded - original, Ordering::AcqRel);
        self.segment.downloaded = original;
        self.save_progress().await
    }

    /// Run the segment download
    pub async fn run(mut self) -> Result<SegmentResult, DlmanError> {
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
                path: self.temp_file_path,
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
                path: self.temp_file_path,
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
        let response = send_scoped(&self.client, reqwest::Method::GET, &self.url,
            self.cookies.as_deref(), self.credentials.as_ref(), self.referrer.as_deref(),
            range.as_deref(), strong_etag(self.etag.as_deref())).await?;

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
        if let (Some(expected), Some(actual)) = (strong_etag(self.etag.as_deref()),
            strong_etag(response.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()))) {
            if expected != actual { return Err(invalid("Download resource changed; partial data was preserved.")); }
        }
        let expected_body;
        if status == reqwest::StatusCode::PARTIAL_CONTENT {
            // Try Content-Range first (for 206 responses): "bytes 0-X/12345".
            let value = response.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok())
                .ok_or_else(|| invalid("Missing Content-Range; partial data was preserved."))?;
            let (actual_start, actual_end, total) = parse_content_range(value)?;
            if actual_start != start_byte || (!unknown_size && actual_end != end_byte)
                || (unknown_size && actual_end != total - 1)
                || self.expected_total.is_some_and(|expected| expected != total) {
                return Err(invalid("Server returned a different byte range or file size."));
            }
            expected_body = Some(actual_end - actual_start + 1);
            if unknown_size {
                self.segment.end = total - 1;
                discovered_size = Some(total);
            }
        } else if status == reqwest::StatusCode::OK && start_byte == 0 && self.segment.start == 0 {
            // Try Content-Length for 200 OK responses (full content).
            // A full response is safe only for a fresh, whole-file segment.
            let length = response.content_length();
            if !unknown_size && (length != Some(self.segment.size()) || self.expected_total.is_some_and(|total| total != self.segment.size())) {
                return Err(invalid("Server ignored a segmented range; partial data was preserved."));
            }
            expected_body = length;
            if unknown_size {
                if let Some(total) = length.filter(|n| *n > 0) {
                    self.segment.end = total - 1;
                    discovered_size = Some(total);
                }
            }
        } else { return Err(invalid("Server ignored a resume range; partial data was preserved.")); }
        let mut received = 0u64;

        // Stream and write chunks
        let mut stream = response.bytes_stream();
        let mut last_db_update = tokio::time::Instant::now();
        let mut last_event_emit = tokio::time::Instant::now();
        
        while let Some(chunk_result) = stream.next().await {
            // Check cancellation first
            if self.cancelled.load(Ordering::Acquire) {
                info!("Segment {} cancelled", self.segment.index);
                self.save_progress().await?;
                return Err(DlmanError::Cancelled);
            }
            
            // Check pause - save progress and return Paused error
            // This allows the HTTP connection to be closed and resumed later
            if self.paused.load(Ordering::Acquire) {
                info!("Segment {} paused", self.segment.index);
                self.save_progress().await?;
                return Err(DlmanError::Paused);
            }
            
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(error) => {
                    self.rollback_attempt(&mut file, existing_size).await?;
                    return Err(DlmanError::Network(error.without_url()));
                }
            };
            let chunk_len = chunk.len() as u64;
            if expected_body.is_some_and(|expected| chunk_len > expected.saturating_sub(received)) {
                self.rollback_attempt(&mut file, existing_size).await?;
                return Err(invalid("Server sent more bytes than the requested segment."));
            }
            received += chunk_len;
            
            // Apply rate limiting
            self.rate_limiter.acquire(chunk_len).await;
            
            // Write to temp file
            file.write_all(&chunk).await?;
            
            // Update progress atomically
            self.segment.downloaded += chunk_len;
            self.downloaded_bytes.fetch_add(chunk_len, Ordering::AcqRel);
            
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
                self.save_progress().await?;
                last_db_update = tokio::time::Instant::now();
            }
        }
        
        if expected_body.is_some_and(|expected| received != expected) {
            self.rollback_attempt(&mut file, existing_size).await?;
            return Err(invalid("Server truncated the requested segment."));
        }

        // Flush and sync to disk
        file.flush().await?;
        file.sync_all().await?;
        
        // Mark segment as complete
        self.segment.complete = true;
        self.save_progress().await?;
        
        info!(
            "Segment {} complete ({} bytes)",
            self.segment.index, self.segment.downloaded
        );
        
        Ok(SegmentResult {
            path: self.temp_file_path,
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
