//! Stable YouTube contracts. Signed stream URLs never enter persisted jobs.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum YoutubeOutput {
    Video,
    Mp3,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct YoutubeSelection {
    pub identity: String,
    pub format_id: String,
    pub container: String,
    pub quality: String,
    pub output_mode: YoutubeOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubeVariant {
    pub id: String,
    pub quality: String,
    pub container: String,
    pub audio: String,
    pub size: Option<u64>,
    pub fps: Option<u32>,
    pub output_mode: YoutubeOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubePreview {
    pub url: String,
    pub identity: String,
    pub title: String,
    pub variants: Vec<YoutubeVariant>,
}

impl YoutubeSelection {
    pub fn matches(&self, preview: &YoutubePreview) -> bool {
        self.identity == preview.identity
            && preview.variants.iter().any(|v| {
                self.format_id == v.id
                    && self.container == v.container
                    && self.quality == v.quality
                    && self.output_mode == v.output_mode
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubeJob {
    pub selection: YoutubeSelection,
    pub stage_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubeInspection {
    pub id: Uuid,
    pub status: String,
    pub preview: Option<YoutubePreview>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YoutubeInspectRequest {
    pub url: String,
}
