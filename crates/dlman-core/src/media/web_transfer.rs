//! Generic webpage and manifest jobs share the managed media process lifecycle.
use super::web::WebMediaContext;
use crate::{DlmanCore, DlmanError};
use dlman_types::*;
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

fn invalid(message: &str) -> DlmanError {
    DlmanError::InvalidOperation(message.into())
}

impl DlmanCore {
    pub async fn add_web_media_download(
        &self,
        url: &str,
        inspection_id: Uuid,
        selection: WebMediaSelection,
        destination: PathBuf,
        queue_id: Uuid,
        category_id: Option<Uuid>,
        filename: Option<String>,
        auto_start: bool,
    ) -> Result<Download, DlmanError> {
        if selection.output_mode != YoutubeOutput::Video {
            return Err(invalid(
                "Choose a video format. Audio conversion is unsupported for webpages.",
            ));
        }
        let (preview, source_key) = self
            .web_media
            .validate_preview(inspection_id, url, &selection)
            .await?;
        if !destination.is_absolute() {
            return Err(invalid("Choose an absolute destination folder."));
        }
        if self.queue_manager.get_queue(queue_id).await.is_none() {
            return Err(invalid("The selected queue no longer exists."));
        }
        let mut download = Download::new(preview.url.clone(), destination, queue_id);
        download.source_page_url = Some(preview.url);
        let chosen =
            filename.unwrap_or_else(|| format!("{}.{}", preview.title, selection.container));
        let chosen = super::youtube::safe_filename(&chosen, &selection.container)?;
        download.filename = self
            .unique_filename(&download.destination, &chosen, None)
            .await;
        download.category_id = category_id;
        download.size = preview
            .variants
            .iter()
            .find(|v| v.id == selection.format_id)
            .and_then(|v| v.size);
        download.status = DownloadStatus::Queued;
        let settings = self.get_settings().await;
        let stage_dir = crate::engine::resolve_segment_cache_dir(
            &settings.temp_storage,
            &download.destination,
            &self.youtube_data_dir,
            download.size,
        )
        .join(format!("web-media-{}", download.id));
        download.web_media = Some(
            serde_json::to_value(WebMediaJob {
                selection,
                stage_dir,
                source_key,
            })
            .map_err(|_| invalid("Cannot persist video selection."))?,
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

    /// Compatibility intake for manifest URLs received outside the quality dialog.
    /// New desktop submissions always supply an explicit, freshly inspected selection.
    pub(crate) async fn add_web_stream_download(
        &self,
        url: &str,
        destination: PathBuf,
        queue_id: Uuid,
        category_id: Option<Uuid>,
        filename: Option<String>,
        context: Option<WebMediaContext>,
        auto_start: bool,
    ) -> Result<Download, DlmanError> {
        let proxy = self.get_settings().await.proxy;
        let mut inspection = self
            .web_media
            .inspect(url, proxy, None, None, context)
            .await?;
        let id = inspection.id;
        let until = tokio::time::Instant::now() + Duration::from_secs(210);
        while inspection.status == "pending" {
            if tokio::time::Instant::now() >= until {
                return Err(invalid("Video inspection timed out."));
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
            inspection = self.web_media.inspection(id).await?;
        }
        let preview = inspection.preview.ok_or_else(|| {
            invalid(
                inspection
                    .error
                    .as_deref()
                    .unwrap_or("No downloadable video was detected."),
            )
        })?;
        let variant = preview
            .variants
            .first()
            .ok_or_else(|| invalid("No video format is available."))?;
        let selection = WebMediaSelection {
            identity: preview.identity.clone(),
            format_id: variant.id.clone(),
            container: variant.container.clone(),
            quality: variant.quality.clone(),
            output_mode: YoutubeOutput::Video,
        };
        self.add_web_media_download(
            url,
            id,
            selection,
            destination,
            queue_id,
            category_id,
            filename,
            auto_start,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_web_job_has_no_playback_credentials_or_stream_url() {
        let job = WebMediaJob {
            selection: WebMediaSelection {
                identity: "Generic:clip".into(),
                format_id: "v+a".into(),
                container: "mp4".into(),
                quality: "1080p".into(),
                output_mode: YoutubeOutput::Video,
            },
            stage_dir: std::env::temp_dir().join("web-media-test"),
            source_key: Some("opaque-source".into()),
        };
        let value = serde_json::to_value(&job).unwrap();
        assert!(value.get("cookies").is_none());
        assert!(value.get("url").is_none());
        let restored: WebMediaJob = serde_json::from_value(value).unwrap();
        assert_eq!(restored.selection, job.selection);
        assert_eq!(restored.source_key, job.source_key);
    }

    #[tokio::test]
    async fn additive_migration_preserves_legacy_rows_and_round_trips_web_jobs() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("downloads.db");
        let db = crate::DownloadDatabase::new(&path).await.unwrap();
        let legacy = Download::new(
            "https://example.com/archive.zip".into(),
            root.path().into(),
            Uuid::nil(),
        );
        db.upsert_download(&legacy).await.unwrap();
        drop(db);
        // Model the pre-feature database rather than an empty database only.
        let options = sqlx::sqlite::SqliteConnectOptions::new().filename(&path);
        let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
        sqlx::query("ALTER TABLE downloads DROP COLUMN web_media")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;
        let db = crate::DownloadDatabase::new(&path).await.unwrap();
        let restored = db.load_download(legacy.id).await.unwrap().unwrap();
        assert_eq!(restored.url, legacy.url);
        assert!(restored.web_media.is_none());
        let mut video = Download::new(
            "https://example.com/watch/1".into(),
            root.path().into(),
            Uuid::nil(),
        );
        video.web_media = Some(serde_json::json!({
            "selection": {"identity":"Generic:1", "format_id":"v+a", "container":"mp4",
                "quality":"1080p", "output_mode":"video"},
            "stage_dir": root.path().join(format!("web-media-{}", video.id)), "source_key":null,
        }));
        db.upsert_download(&video).await.unwrap();
        drop(db);
        // Running migration a second time must retain both rows and the exact selection.
        let reopened = crate::DownloadDatabase::new(&path).await.unwrap();
        assert_eq!(
            reopened
                .load_download(video.id)
                .await
                .unwrap()
                .unwrap()
                .web_media,
            video.web_media
        );
        assert_eq!(
            reopened
                .load_download(legacy.id)
                .await
                .unwrap()
                .unwrap()
                .url,
            legacy.url
        );
    }
}
