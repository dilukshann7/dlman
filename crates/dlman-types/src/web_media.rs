//! Generic webpage-video contracts. Browser credentials stay in process memory.
use crate::{YoutubePreview, YoutubeSelection, YoutubeSessionCookie};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

pub type WebMediaSelection = YoutubeSelection;
pub type WebMediaPreview = YoutubePreview;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebMediaQualityHint {
    pub label: String,
    pub height: Option<u32>,
    pub fps: Option<u32>,
}

#[derive(Deserialize)]
pub struct WebMediaInspectRequest {
    pub url: String,
}

#[derive(Deserialize)]
pub struct WebMediaDownloadRequest {
    pub url: String,
    pub inspection_id: Uuid,
    pub selection: WebMediaSelection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebMediaInspection {
    pub id: Uuid,
    pub status: String,
    pub preview: Option<WebMediaPreview>,
    pub error: Option<String>,
}

/// Only the stable selection and an opaque context lookup key may be persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebMediaJob {
    pub selection: WebMediaSelection,
    pub stage_dir: PathBuf,
    #[serde(default)]
    pub source_key: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WebMediaSource {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub protocol: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct WebMediaCapture {
    pub id: Uuid,
    pub url: String,
    pub status: String,
    pub sources: Vec<WebMediaSource>,
    pub error: Option<String>,
    pub bridge_url: Option<String>,
}

#[derive(Deserialize)]
pub struct WebMediaCaptureClaim {
    pub client_id: String,
    pub tab_id: i64,
}

// Deliberately omit Debug and Serialize: these values contain browser credentials.
#[derive(Clone, Deserialize)]
pub struct WebMediaCapturedSource {
    pub url: String,
    pub title: Option<String>,
    pub protocol: String,
    pub referrer: Option<String>,
    pub user_agent: Option<String>,
    #[serde(default)]
    pub cookies: Vec<YoutubeSessionCookie>,
}

#[derive(Deserialize)]
pub struct WebMediaCaptureSubmission {
    pub client_id: String,
    pub tab_id: i64,
    pub page_url: String,
    pub sources: Vec<WebMediaCapturedSource>,
}
