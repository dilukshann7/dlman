//! Webpage inspection and browser-captured sources. Credentials never enter persisted jobs.
mod capture;
mod context;
mod probe;

use crate::{media::youtube, DlmanError};
pub use context::WebMediaContext;
use dlman_types::*;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn error(message: &str) -> DlmanError {
    DlmanError::InvalidOperation(message.to_owned())
}

/// Canonicalize syntax without dropping query parameters or client-side page fragments.
pub fn canonical_url(value: &str) -> Result<String, DlmanError> {
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err(error("Invalid webpage URL."));
    }
    let url = url::Url::parse(value).map_err(|_| error("Invalid webpage URL."))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err(error(
            "Use an HTTP or HTTPS webpage URL without embedded credentials.",
        ));
    }
    let host = url
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .to_ascii_lowercase();
    if ["youtube.com", "youtu.be", "youtube-nocookie.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
    {
        return Err(error(
            "Use the dedicated YouTube download flow for YouTube links.",
        ));
    }
    Ok(url.into())
}

/// Convert legacy extension headers into a validated, memory-only request context.
pub fn legacy_context(
    url: &str,
    referrer: Option<String>,
    cookies: Option<String>,
) -> Result<WebMediaContext, DlmanError> {
    WebMediaContext {
        url: url.to_owned(),
        referrer,
        user_agent: None,
        cookies: Vec::new(),
        cookie_header: cookies,
    }
    .checked()
}

struct InspectionEntry {
    page: String,
    source_key: Option<String>,
    revision: Option<Uuid>,
    fingerprint: String,
    created: Instant,
    result: WebMediaInspection,
}

impl InspectionEntry {
    fn fresh(&self) -> bool {
        self.created.elapsed()
            < Duration::from_secs(if self.result.status == "pending" {
                // Bounded HTTP probing (35 seconds) may precede extraction (150 seconds).
                210
            } else {
                120
            })
    }
}

pub struct WebMediaService {
    youtube: Arc<youtube::YoutubeService>,
    captures: Mutex<capture::CaptureStore>,
    inspections: Mutex<HashMap<Uuid, InspectionEntry>>,
    inspection_tasks: Mutex<HashMap<Uuid, tokio::task::JoinHandle<()>>>,
    // Native ownership keeps the automatic limit intact across MV3 worker restarts.
    automatic_inspection: Mutex<Option<Uuid>>,
    slots: Arc<Semaphore>,
    shutdown: CancellationToken,
}

impl WebMediaService {
    pub fn new(youtube: Arc<youtube::YoutubeService>) -> Self {
        Self {
            youtube,
            captures: Mutex::new(capture::CaptureStore::default()),
            inspections: Mutex::new(HashMap::new()),
            inspection_tasks: Mutex::new(HashMap::new()),
            automatic_inspection: Mutex::new(None),
            slots: Arc::new(Semaphore::new(2)),
            shutdown: CancellationToken::new(),
        }
    }

    pub async fn inspect_automatic(
        self: &Arc<Self>,
        value: &str,
        proxy: ProxySettings,
    ) -> Result<WebMediaInspection, DlmanError> {
        let page = canonical_url(value)?;
        let mut automatic = self.automatic_inspection.lock().await;
        if let Some(id) = *automatic {
            if let Some(entry) = self.inspections.lock().await.get(&id).filter(|entry| entry.fresh()) {
                if entry.result.status == "pending" {
                    if entry.page == page {
                        return Ok(entry.result.clone());
                    }
                    return Err(error("Automatic video inspection is busy. Retry shortly."));
                }
            }
        }
        let result = self.inspect(&page, proxy, None, None, None).await?;
        *automatic = Some(result.id);
        Ok(result)
    }

    pub async fn inspect(
        self: &Arc<Self>,
        value: &str,
        proxy: ProxySettings,
        capture_id: Option<Uuid>,
        source_id: Option<String>,
        legacy_context: Option<WebMediaContext>,
    ) -> Result<WebMediaInspection, DlmanError> {
        self.inspect_with_probe_credentials(
            value,
            proxy,
            capture_id,
            source_id,
            legacy_context,
            None,
        )
        .await
    }

    /// Saved site credentials authorize only the bounded classification request.
    /// They are never attached to browser contexts or passed to the media extractor.
    pub async fn inspect_with_probe_credentials(
        self: &Arc<Self>,
        value: &str,
        proxy: ProxySettings,
        capture_id: Option<Uuid>,
        source_id: Option<String>,
        legacy_context: Option<WebMediaContext>,
        probe_credentials: Option<(String, String)>,
    ) -> Result<WebMediaInspection, DlmanError> {
        if self.shutdown.is_cancelled() {
            return Err(error("Media inspection is shutting down."));
        }
        let page = canonical_url(value)?;
        if capture_id.is_some() != source_id.is_some()
            || (capture_id.is_some() && legacy_context.is_some())
        {
            return Err(error("Select one browser source from the active capture."));
        }
        // Ordinary extension file requests also carry legacy headers. Only an
        // explicitly selected browser source bypasses ordinary-file classification.
        let captured = capture_id.is_some();
        let (context, source_key, revision) = if let (Some(id), Some(source)) =
            (capture_id, source_id)
        {
            let (context, revision) = self.captures.lock().await.selected(id, &page, &source)?;
            (context, Some(source), Some(revision))
        } else if let Some(context) = legacy_context {
            let context = context.checked()?;
            if context.url != page
                && context
                    .referrer
                    .as_deref()
                    .and_then(|value| canonical_url(value).ok())
                    .as_deref()
                    != Some(page.as_str())
            {
                return Err(error(
                    "The browser context does not match the requested webpage or its referrer.",
                ));
            }
            let (key, revision) = self.captures.lock().await.legacy(&page, context.clone())?;
            (context, Some(key), Some(revision))
        } else {
            (WebMediaContext::anonymous(&page)?, None, None)
        };
        let legacy_video_source = context.url != page;
        // Saved credentials were resolved for the requested page, which may have
        // provided a manifest on a different CDN origin. Never authorize that CDN
        // with the page's saved credentials.
        let probe_credentials = probe_credentials.filter(|_| {
            url::Url::parse(&page).ok().map(|url| url.origin())
                == url::Url::parse(&context.url).ok().map(|url| url.origin())
        });
        let fingerprint =
            inspection_fingerprint_with_credentials(&context, &proxy, probe_credentials.as_ref());
        let mut records = self.inspections.lock().await;
        records.retain(|_, entry| entry.fresh());
        if let Some(entry) = records.values().find(|entry| {
            entry.page == page
                && entry.source_key == source_key
                && entry.revision == revision
                && entry.fingerprint == fingerprint
                && entry.result.status != "failed"
        }) {
            return Ok(entry.result.clone());
        }
        if records.len() >= 64 {
            return Err(error("Too many video inspections. Try again shortly."));
        }
        let permit =
            self.slots.clone().try_acquire_owned().map_err(|_| {
                error("Two video inspections are already running. Try again shortly.")
            })?;
        let id = Uuid::new_v4();
        let result = WebMediaInspection {
            id,
            status: "pending".into(),
            preview: None,
            error: None,
        };
        records.insert(
            id,
            InspectionEntry {
                page: page.clone(),
                source_key,
                revision,
                fingerprint,
                created: Instant::now(),
                result: result.clone(),
            },
        );
        drop(records);
        let service = self.clone();
        // Hold the task registry while spawning so shutdown cannot miss a new task.
        let mut tasks = self.inspection_tasks.lock().await;
        if self.shutdown.is_cancelled() {
            self.inspections.lock().await.remove(&id);
            return Err(error("Media inspection is shutting down."));
        }
        tasks.retain(|_, task| !task.is_finished());
        let task = tokio::spawn(async move {
            let _permit = permit;
            let inspected = async {
                let resource = if captured { None } else if probe_credentials.is_some() {
                    Some(probe::probe_with_credentials(&context, &proxy, service.shutdown.child_token(), probe_credentials.as_ref()).await?)
                } else {
                    Some(probe::probe(&context, &proxy, service.shutdown.child_token()).await?)
                };
                drop(probe_credentials);
                if !legacy_video_source && resource == Some(probe::ResourceKind::File) {
                    return Ok(None);
                }
                let value = service.youtube.extract_web_metadata(&context, &proxy, service.shutdown.child_token()).await?;
                let mut preview = youtube::parse_web_preview(&page, &value)?;
                preview.variants.retain(|variant| variant.output_mode == YoutubeOutput::Video);
                if preview.variants.is_empty() { return Err(error("No supported on-demand video formats were found. Try Detect in browser while playing the video.")); }
                // The persisted download URL always stays the page supplied by the user.
                preview.url = page.clone();
                Ok(Some(preview))
            }.await;
            if let Some(entry) = service.inspections.lock().await.get_mut(&id) {
                entry.created = Instant::now();
                match inspected {
                    Ok(Some(preview)) => {
                        entry.result.status = "ready".into();
                        entry.result.preview = Some(preview);
                    }
                    Ok(None) => entry.result.status = "file".into(),
                    Err(failure) => {
                        entry.result.status = "failed".into();
                        entry.result.error = Some(failure.to_string());
                    }
                }
            }
        });
        tasks.insert(id, task);
        Ok(result)
    }

    pub async fn inspection(&self, id: Uuid) -> Result<WebMediaInspection, DlmanError> {
        self.inspections
            .lock()
            .await
            .get(&id)
            .filter(|entry| entry.fresh())
            .map(|entry| entry.result.clone())
            .ok_or_else(|| error("Video preview expired. Inspect the webpage again."))
    }

    pub async fn validate_preview(
        &self,
        id: Uuid,
        value: &str,
        selection: &WebMediaSelection,
    ) -> Result<(WebMediaPreview, Option<String>), DlmanError> {
        let page = canonical_url(value)?;
        let (preview, key, revision) = {
            let records = self.inspections.lock().await;
            let entry = records
                .get(&id)
                .filter(|entry| entry.fresh())
                .ok_or_else(|| error("Video preview expired. Inspect the webpage again."))?;
            let preview = entry
                .result
                .preview
                .as_ref()
                .filter(|_| entry.result.status == "ready")
                .ok_or_else(|| error("Video inspection is not ready."))?;
            if entry.page != page
                || preview.url != page
                || selection.output_mode != YoutubeOutput::Video
                || !selection.matches(preview)
            {
                return Err(error(
                    "The selected webpage or video format changed. Refresh the quality list.",
                ));
            }
            (preview.clone(), entry.source_key.clone(), entry.revision)
        };
        if let Some(key) = &key {
            let (_, current) = self.captures.lock().await.context(&page, key)?;
            if Some(current) != revision {
                return Err(error("The browser media source was refreshed. Refresh the quality list before adding it."));
            }
        }
        Ok((preview, key))
    }

    pub(crate) async fn context_for_job(
        &self,
        page_url: &str,
        source_key: Option<&str>,
    ) -> Result<WebMediaContext, DlmanError> {
        let page = canonical_url(page_url)?;
        match source_key {
            Some(key) => self
                .captures
                .lock()
                .await
                .context(&page, key)
                .map(|(context, _)| context),
            None => WebMediaContext::anonymous(&page),
        }
    }

    pub async fn begin_capture(&self, url: &str) -> Result<WebMediaCapture, DlmanError> {
        if self.shutdown.is_cancelled() {
            return Err(error("Media capture is shutting down."));
        }
        self.captures.lock().await.begin(url)
    }
    pub async fn capture(&self, id: Uuid) -> Result<WebMediaCapture, DlmanError> {
        self.captures.lock().await.capture(id)
    }
    pub async fn claim_capture(
        &self,
        id: Uuid,
        claim: WebMediaCaptureClaim,
    ) -> Result<WebMediaCapture, DlmanError> {
        self.captures.lock().await.claim(id, claim)
    }
    pub async fn submit_capture(
        &self,
        id: Uuid,
        submission: WebMediaCaptureSubmission,
    ) -> Result<WebMediaCapture, DlmanError> {
        self.captures.lock().await.submit(id, submission)
    }
    pub async fn cancel_capture(&self, id: Uuid) {
        self.captures.lock().await.cancel(id);
    }
    pub async fn cancel_capture_claim(
        &self,
        id: Uuid,
        claim: WebMediaCaptureClaim,
    ) -> Result<(), DlmanError> {
        self.captures.lock().await.cancel_claim(id, claim)
    }

    pub async fn shutdown(&self) {
        self.shutdown.cancel();
        let tasks: Vec<_> = self
            .inspection_tasks
            .lock()
            .await
            .drain()
            .map(|(_, task)| task)
            .collect();
        for task in tasks {
            let _ = task.await;
        }
        self.inspections.lock().await.clear();
        self.captures.lock().await.clear();
    }
}

fn inspection_fingerprint(context: &WebMediaContext, proxy: &ProxySettings) -> String {
    let mut hash = Sha256::new();
    for part in [
        Some(context.url.as_str()),
        context.referrer.as_deref(),
        context.user_agent.as_deref(),
        context.cookie_header.as_deref(),
        Some(proxy.mode.as_str()),
        proxy.http_proxy.as_deref(),
        proxy.https_proxy.as_deref(),
        proxy.no_proxy.as_deref(),
        proxy.username.as_deref(),
        proxy.password.as_deref(),
    ] {
        if let Some(part) = part {
            hash.update(part.len().to_le_bytes());
            hash.update(part.as_bytes());
        }
        hash.update([0]);
    }
    for cookie in &context.cookies {
        for part in [&cookie.domain, &cookie.path, &cookie.name, &cookie.value] {
            hash.update(part.len().to_le_bytes());
            hash.update(part.as_bytes());
        }
        hash.update([
            cookie.secure as u8,
            cookie.http_only as u8,
            cookie.host_only as u8,
        ]);
        hash.update(cookie.expires.unwrap_or(0).to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn inspection_fingerprint_with_credentials(
    context: &WebMediaContext,
    proxy: &ProxySettings,
    credentials: Option<&(String, String)>,
) -> String {
    let mut hash = Sha256::new();
    hash.update(inspection_fingerprint(context, proxy).as_bytes());
    if let Some((username, password)) = credentials {
        hash.update([1]);
        for part in [username, password] {
            hash.update(part.len().to_le_bytes());
            hash.update(part.as_bytes());
        }
    } else {
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests;
