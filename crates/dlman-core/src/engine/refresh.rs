//! Conservative address replacement for an existing ordinary HTTP download.
//! Sampling checks compatibility; it is not a complete file identity proof.

use crate::error::DlmanError;
use dlman_types::{Download, DownloadStatus};
use futures::StreamExt;
use reqwest::{header, Client, Method, Response};
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use uuid::Uuid;

pub const SESSION_TTL: Duration = Duration::from_secs(600);

pub(crate) fn invalid(message: &str) -> DlmanError {
    DlmanError::InvalidOperation(message.into())
}

pub fn http_url(value: &str) -> Result<url::Url, DlmanError> {
    let url = url::Url::parse(value).map_err(|_| invalid("Enter a valid HTTP(S) download URL."))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("Use an HTTP(S) URL without embedded credentials."));
    }
    Ok(url)
}

pub fn eligible(download: &Download) -> bool {
    matches!(
        download.status,
        DownloadStatus::Paused | DownloadStatus::Failed
    ) && download.youtube.is_none()
        && ordinary_url(&download.url)
}

pub fn ordinary_url(value: &str) -> bool {
    http_url(value)
        .map(|url| {
            !crate::media::youtube::is_youtube_host(value)
                && !["googlevideo.com", "youtube-nocookie.com"]
                    .iter()
                    .any(|domain| {
                        let host = url.host_str().unwrap_or("");
                        host == *domain || host.ends_with(&format!(".{domain}"))
                    })
                && !crate::is_streaming_url(value)
        })
        .unwrap_or(false)
}

pub fn display_url(value: &str) -> String {
    http_url(value)
        .map(|mut url| {
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        })
        .unwrap_or_default()
}

#[derive(Clone, Serialize)]
pub struct RefreshCandidateInfo {
    pub candidate_id: Uuid,
    pub display_url: String,
    pub filename: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct RefreshSessionInfo {
    pub session_id: Uuid,
    pub download_id: Uuid,
    /// Unix milliseconds, for the desktop countdown only. Core uses Instant.
    pub expires_at: i64,
    pub candidate: Option<RefreshCandidateInfo>,
}

#[derive(Clone)]
pub(crate) struct RefreshCandidate {
    pub info: RefreshCandidateInfo,
    pub url: String,
    pub cookies: Option<String>,
    pub source_page_url: Option<String>,
}

pub(crate) struct RefreshSession {
    pub info: RefreshSessionInfo,
    pub started: Instant,
    pub verifying: bool,
    pub candidate: Option<RefreshCandidate>,
}

impl RefreshSession {
    pub fn new(id: Uuid) -> Self {
        Self {
            info: RefreshSessionInfo {
                session_id: Uuid::new_v4(),
                download_id: id,
                expires_at: chrono::Utc::now().timestamp_millis() + SESSION_TTL.as_millis() as i64,
                candidate: None,
            },
            started: Instant::now(),
            verifying: false,
            candidate: None,
        }
    }
    pub fn live(&self) -> bool {
        self.started.elapsed() < SESSION_TTL
    }
    pub fn can_capture(&self) -> bool {
        self.live() && !self.verifying && self.candidate.is_none()
    }
}

pub fn strong_etag(value: Option<&str>) -> Option<&str> {
    value.filter(|v| v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
}

pub fn parse_content_range(value: &str) -> Result<(u64, u64, u64), DlmanError> {
    let parsed = (|| {
        let (range, total) = value.strip_prefix("bytes ")?.split_once('/')?;
        let (start, end) = range.split_once('-')?;
        if ![start, end, total]
            .iter()
            .all(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        let (start, end, total) = (
            start.parse::<u64>().ok()?,
            end.parse::<u64>().ok()?,
            total.parse::<u64>().ok()?,
        );
        (start <= end && end < total).then_some((start, end, total))
    })();
    parsed.ok_or_else(|| {
        invalid("Server returned an invalid Content-Range; partial data was preserved.")
    })
}

/// The client must have automatic redirects disabled. Secrets are applied only
/// to the original origin, and never restored after crossing an origin boundary.
pub async fn send_scoped(
    client: &Client,
    method: Method,
    value: &str,
    cookies: Option<&str>,
    credentials: Option<&(String, String)>,
    referrer: Option<&str>,
    range: Option<&str>,
    if_range: Option<&str>,
) -> Result<Response, DlmanError> {
    let mut url = http_url(value)?;
    let origin = url.origin();
    let mut secrets_allowed = true;
    for hop in 0..=10 {
        let mut request = client
            .request(method.clone(), url.clone())
            .header(header::ACCEPT_ENCODING, "identity");
        if secrets_allowed {
            if let Some(cookies) = cookies {
                request = request.header(header::COOKIE, cookies);
            }
            if let Some((username, password)) = credentials {
                request = request.basic_auth(username, Some(password));
            }
        }
        if let Some(source) = referrer.and_then(|source| http_url(source).ok()) {
            if !(source.scheme() == "https" && url.scheme() == "http") {
                let referer = if source.origin() == url.origin() {
                    source.to_string()
                } else {
                    format!("{}/", source.origin().ascii_serialization())
                };
                request = request.header(header::REFERER, referer);
            }
        }
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        if let Some(validator) = if_range {
            request = request.header(header::IF_RANGE, validator);
        }
        let response = request
            .send()
            .await
            .map_err(|error| DlmanError::Network(error.without_url()))?;
        if !response.status().is_redirection() {
            return Ok(response);
        }
        if hop == 10 {
            return Err(invalid("Too many download redirects."));
        }
        let location = response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| invalid("Download redirect has no valid Location."))?;
        let next = url
            .join(location)
            .map_err(|_| invalid("Download redirect has an invalid URL."))?;
        http_url(next.as_str())?;
        if url.scheme() == "https" && next.scheme() == "http" {
            return Err(invalid("Download redirected from HTTPS to HTTP."));
        }
        secrets_allowed &= next.origin() == origin;
        url = next;
    }
    Err(invalid("Too many download redirects."))
}

pub fn validate_response(response: &Response) -> Result<(), DlmanError> {
    validate_response_for_file(response, false)
}

pub fn expects_html(filename: &str) -> bool {
    let filename = filename.to_ascii_lowercase();
    filename.ends_with(".html") || filename.ends_with(".htm") || filename.ends_with(".xhtml")
}

pub fn validate_response_for_file(response: &Response, allow_html: bool) -> Result<(), DlmanError> {
    let status = response.status();
    if !status.is_success() {
        return Err(DlmanError::ServerError {
            status: status.as_u16(),
            message: "Download address returned an error; partial data was preserved.".into(),
        });
    }
    if response
        .headers()
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
    {
        return Err(invalid(
            "Encoded download responses cannot safely resume partial data.",
        ));
    }
    if !allow_html
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                let mime = v.split(';').next().unwrap_or("").trim();
                mime.eq_ignore_ascii_case("text/html")
                    || mime.eq_ignore_ascii_case("application/xhtml+xml")
            })
    {
        return Err(invalid(
            "The address returned an HTML page. Obtain a fresh file download link.",
        ));
    }
    Ok(())
}

fn header_text(response: &Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

async fn bounded_body(response: Response, expected: usize) -> Result<Vec<u8>, DlmanError> {
    let mut body = Vec::with_capacity(expected);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| DlmanError::Network(error.without_url()))?;
        if chunk.len() > expected - body.len() {
            return Err(invalid(
                "Server sent more bytes than the requested verification range.",
            ));
        }
        body.extend_from_slice(&chunk);
    }
    if body.len() != expected {
        return Err(invalid("Server truncated the verification range."));
    }
    Ok(body)
}

pub(crate) async fn verify_candidate(
    client: &Client,
    mut download: Download,
    candidate: RefreshCandidate,
    cache_dir: &Path,
    credentials: Option<&(String, String)>,
) -> Result<Download, DlmanError> {
    if !ordinary_url(&candidate.url) {
        return Err(invalid("Refresh supports ordinary HTTP(S) file URLs only."));
    }
    // Reconcile files without writing to disk or changing persistent progress.
    let mut retained = 0u64;
    for segment in &mut download.segments {
        if segment.end < segment.start {
            return Err(invalid("Saved segment boundaries are inconsistent."));
        }
        let path = cache_dir.join(format!("{}_segment_{}.part", download.id, segment.index));
        let length = match tokio::fs::metadata(&path).await {
            Ok(meta) if meta.is_file() => meta.len(),
            Ok(_) => return Err(invalid("A partial segment is not a regular file.")),
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && segment.downloaded == 0
                    && !segment.complete =>
            {
                0
            }
            Err(_) => {
                return Err(invalid(
                    "Required partial segment files are missing or unreadable.",
                ))
            }
        };
        if length < segment.downloaded || (!segment.is_unknown_size() && length > segment.size()) {
            return Err(invalid(
                "Partial segment lengths are inconsistent; files were preserved.",
            ));
        }
        segment.downloaded = length;
        segment.complete = !segment.is_unknown_size() && length == segment.size();
        retained = retained
            .checked_add(length)
            .ok_or_else(|| invalid("Invalid partial segment size."))?;
    }
    if retained == 0 && download.downloaded > 0 {
        return Err(invalid(
            "Required partial data is missing; the download was not restarted.",
        ));
    }
    let mut samples = Vec::new();
    if retained > 0 {
        for position in [0, retained / 2, retained - 1] {
            let mut offset = position;
            for segment in &download.segments {
                if offset < segment.downloaded {
                    let length = segment.downloaded.min(4096);
                    let local = offset.min(segment.downloaded - length);
                    if !samples.contains(&(segment.index, segment.start, local, length)) {
                        samples.push((segment.index, segment.start, local, length));
                    }
                    break;
                }
                offset -= segment.downloaded;
            }
        }
    } else {
        samples.push((0, 0, 0, 1));
    }
    let mut total = None;
    let mut etag: Option<String> = None;
    let mut last_modified = None;
    let mut final_url = None;
    let mut supports_range = false;
    for (index, start, local, length) in samples {
        let absolute = start
            .checked_add(local)
            .ok_or_else(|| invalid("Invalid segment offset."))?;
        let end = absolute
            .checked_add(length - 1)
            .ok_or_else(|| invalid("Invalid segment offset."))?;
        let range = format!("bytes={absolute}-{end}");
        let response = send_scoped(
            client,
            Method::GET,
            &candidate.url,
            candidate.cookies.as_deref(),
            credentials,
            candidate.source_page_url.as_deref(),
            Some(&range),
            None,
        )
        .await?;
        validate_response_for_file(&response, expects_html(&download.filename))?;
        let response_etag = header_text(&response, header::ETAG);
        for previous in [
            if retained > 0 {
                download.etag.as_deref()
            } else {
                None
            },
            etag.as_deref(),
        ] {
            if let (Some(old), Some(new)) =
                (strong_etag(previous), strong_etag(response_etag.as_deref()))
            {
                if old != new {
                    return Err(invalid(
                        "The replacement file has a different ETag; partial data was preserved.",
                    ));
                }
            }
        }
        let found_total;
        if response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let value = header_text(&response, header::CONTENT_RANGE)
                .ok_or_else(|| invalid("Missing Content-Range in verification response."))?;
            let (actual_start, actual_end, actual_total) = parse_content_range(&value)?;
            if (actual_start, actual_end) != (absolute, end) {
                return Err(invalid("Server returned a different verification range."));
            }
            found_total = Some(actual_total);
            supports_range = true;
        } else if retained == 0 && response.status() == reqwest::StatusCode::OK {
            found_total = response.content_length();
        } else {
            return Err(invalid(
                "Server ignored the byte range; partial data was preserved.",
            ));
        }
        if retained > 0
            && (found_total.is_none()
                || download.size.is_some_and(|size| Some(size) != found_total)
                || total.is_some_and(|size| Some(size) != found_total))
        {
            return Err(invalid(
                "Replacement file size differs from the saved download.",
            ));
        }
        total = found_total;
        final_url = Some(response.url().to_string());
        // A weak/missing validator must never erase a previously observed
        // strong validator: later samples must remain consistent with it.
        if strong_etag(etag.as_deref()).is_none() || strong_etag(response_etag.as_deref()).is_some()
        {
            etag = response_etag.or(etag);
        }
        last_modified = header_text(&response, header::LAST_MODIFIED).or(last_modified);
        if retained == 0 && supports_range {
            bounded_body(response, 1).await?;
        } else if retained > 0 {
            let remote = bounded_body(response, length as usize).await?;
            let path = cache_dir.join(format!("{}_segment_{}.part", download.id, index));
            let mut file = tokio::fs::File::open(path).await?;
            file.seek(std::io::SeekFrom::Start(local)).await?;
            let mut local_bytes = vec![0; length as usize];
            file.read_exact(&mut local_bytes).await?;
            if local_bytes != remote {
                return Err(invalid(
                    "Replacement bytes do not match the saved file. Partial data was preserved.",
                ));
            }
        }
    }
    if let Some(total) = total {
        let mut expected_start = 0;
        for segment in &mut download.segments {
            if segment.start != expected_start {
                return Err(invalid("Saved segment layout is inconsistent."));
            }
            if segment.is_unknown_size() {
                if segment.start != 0 || retained > total {
                    return Err(invalid(
                        "Saved segment layout does not match the replacement.",
                    ));
                }
                segment.end = total.saturating_sub(1);
                segment.complete = segment.downloaded == total;
            }
            expected_start = segment
                .end
                .checked_add(1)
                .ok_or_else(|| invalid("Invalid segment boundary."))?;
        }
        if !download.segments.is_empty() && expected_start != total {
            return Err(invalid(
                "Saved segment layout does not match the replacement size.",
            ));
        }
    }
    download.url = candidate.url;
    download.final_url = final_url;
    download.source_page_url = candidate.source_page_url.or(download.source_page_url);
    download.cookies = candidate.cookies;
    download.etag = etag;
    download.last_modified = last_modified;
    download.supports_range = supports_range;
    download.size = total;
    download.downloaded = retained;
    download.segment_cache_dir = Some(cache_dir.to_path_buf());
    download.status = DownloadStatus::Paused;
    download.error = None;
    download.retry_count = 0;
    Ok(download)
}

#[cfg(test)]
#[path = "refresh_tests.rs"]
mod tests;
