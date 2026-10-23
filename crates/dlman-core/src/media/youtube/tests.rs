use super::*;
use serde_json::json;

fn fixture() -> serde_json::Value {
    json!({"id":"abcdefghijk", "extractor_key":"Youtube", "title":"Example", "formats":[
        {"format_id":"137", "ext":"mp4", "height":1080, "width":1920, "fps":59.94,
         "format_note":"1080p60", "vcodec":"avc1", "acodec":"none", "filesize":1000},
        {"format_id":"140", "ext":"m4a", "vcodec":"none", "acodec":"mp4a.40.2", "abr":128, "filesize":100},
        {"format_id":"251", "ext":"webm", "vcodec":"none", "acodec":"opus", "abr":160, "filesize":120}
    ]})
}

#[test]
fn canonicalizes_single_video_and_rejects_unsupported_urls() {
    for url in [
        "https://youtu.be/abcdefghijk",
        "https://m.youtube.com/shorts/abcdefghijk",
        "https://youtube.com/watch?v=abcdefghijk&list=x",
    ] {
        assert_eq!(
            canonical_url(url).unwrap(),
            "https://www.youtube.com/watch?v=abcdefghijk"
        );
    }
    for url in [
        "https://evil.youtube.com/watch?v=abcdefghijk",
        "https://youtube.com/live/abcdefghijk",
        "https://youtube.com/playlist?list=x",
        "https://user@youtube.com/watch?v=abcdefghijk",
    ] {
        assert!(canonical_url(url).is_err());
    }
}

#[test]
fn pairs_compatible_audio_and_adds_explicit_mp3_conversion() {
    let preview = parse_preview("https://youtu.be/abcdefghijk", &fixture()).unwrap();
    assert_eq!(preview.variants.len(), 2);
    assert_eq!(preview.variants[0].id, "137+140");
    assert_eq!(preview.variants[0].quality, "1080p60");
    assert_eq!(preview.variants[0].fps, Some(60));
    assert_eq!(preview.variants[1].id, "251");
    assert_eq!(preview.variants[1].output_mode, YoutubeOutput::Mp3);
    assert_eq!(preview.variants[1].size, None);
}

#[test]
fn rejects_live_drm_auth_playlist_and_changed_identity() {
    for (key, value) in [
        ("is_live", json!(true)),
        ("has_drm", json!(true)),
        ("availability", json!("needs_auth")),
        ("entries", json!([])),
        ("id", json!("otherxxxxxx")),
    ] {
        let mut data = fixture();
        data[key] = value;
        assert!(parse_preview("https://youtu.be/abcdefghijk", &data).is_err());
    }
}

#[test]
fn mp3_args_convert_only_audio_and_keep_speed_limit() {
    let selection = YoutubeSelection {
        identity: "Youtube:abcdefghijk".into(),
        format_id: "251".into(),
        container: "mp3".into(),
        quality: "Audio only · MP3".into(),
        output_mode: YoutubeOutput::Mp3,
    };
    let args = transfer::download_args(&selection, std::path::Path::new("/staging"), Some(1000));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--audio-format", "mp3"]));
    assert!(args.windows(2).any(|pair| pair == ["--audio-quality", "0"]));
    assert!(args.windows(2).any(|pair| pair == ["--limit-rate", "1000"]));
    assert!(!args.iter().any(|arg| arg == "--merge-output-format"));
    assert!(transfer::verify_streams(
        &json!({"streams":[{"codec_type":"audio", "codec_name":"mp3"}]}),
        YoutubeOutput::Mp3
    )
    .is_ok());
    assert!(transfer::verify_streams(
        &json!({"streams":[{"codec_type":"video"},{"codec_type":"audio"}]}),
        YoutubeOutput::Mp3
    )
    .is_err());
}

#[tokio::test]
async fn inspection_tokens_expire_without_invalidating_stored_selections() {
    let service = YoutubeService::default();
    let id = Uuid::new_v4();
    service.inspections.lock().await.insert(
        id,
        InspectionEntry {
            url: "x".into(),
            session_id: None,
            created: Instant::now() - Duration::from_secs(121),
            result: YoutubeInspection {
                id,
                status: "ready".into(),
                preview: None,
                error: None,
            },
        },
    );
    assert!(service.inspection(id).await.is_err());
    let mut download = Download::new(
        "https://www.youtube.com/watch?v=abcdefghijk".into(),
        PathBuf::from("/downloads"),
        Uuid::nil(),
    );
    download.youtube = Some(json!({"unexpected":"metadata"}));
    let encoded = serde_json::to_string(&download).unwrap();
    let decoded: Download = serde_json::from_str(&encoded).unwrap();
    assert!(serde_json::from_value::<YoutubeJob>(decoded.youtube.unwrap()).is_err());
}

#[tokio::test]
async fn migrated_download_history_round_trips_provider_data_independently() {
    let path = std::env::temp_dir().join(format!("dlman-youtube-{}.sqlite", Uuid::new_v4()));
    let db = crate::DownloadDatabase::new(&path).await.unwrap();
    let ordinary = Download::new(
        "https://example.com/file.zip".into(),
        PathBuf::from("/downloads"),
        Uuid::nil(),
    );
    db.upsert_download(&ordinary).await.unwrap();
    let mut provider = ordinary.clone();
    provider.id = Uuid::new_v4();
    provider.youtube =
        Some(json!({"selection": {"identity":"Youtube:abcdefghijk"}, "stage_dir":"/retained"}));
    db.upsert_download(&provider).await.unwrap();
    assert_eq!(
        db.load_download(ordinary.id)
            .await
            .unwrap()
            .unwrap()
            .youtube,
        None
    );
    let recovered = db.load_download(provider.id).await.unwrap().unwrap();
    assert_eq!(recovered.youtube, provider.youtube);
    // Invalid provider fields do not stop normal history enumeration.
    assert_eq!(db.load_all_downloads().await.unwrap().len(), 2);
    assert!(serde_json::from_value::<YoutubeJob>(recovered.youtube.unwrap()).is_err());
    drop(db);
    let _ = std::fs::remove_file(path);
}

#[test]
fn collision_suffix_on_a_long_title_remains_resumable() {
    let generated = format!("{} (1).mp4", "a".repeat(160));
    assert!(validate_stored_filename(&generated, "mp4").is_ok());
    assert!(validate_stored_filename("../escape.mp4", "mp4").is_err());
}

#[test]
fn progressive_only_videos_still_offer_mp3() {
    let data = json!({"id":"abcdefghijk", "title":"Example", "formats":[
        {"format_id":"18", "ext":"mp4", "width":640, "height":360, "vcodec":"avc1", "acodec":"mp4a", "abr":96}
    ]});
    let preview = parse_preview("https://youtu.be/abcdefghijk", &data).unwrap();
    assert!(preview
        .variants
        .iter()
        .any(|v| v.output_mode == YoutubeOutput::Mp3 && v.id == "18"));
}

#[tokio::test]
async fn malformed_provider_job_fails_without_becoming_an_http_transfer() {
    let root = std::env::temp_dir().join(format!("dlman-youtube-routing-{}", Uuid::new_v4()));
    let core = DlmanCore::new(root.clone()).await.unwrap();
    let mut download = Download::new(
        "https://www.youtube.com/watch?v=abcdefghijk".into(),
        root.join("downloads"),
        Uuid::nil(),
    );
    download.youtube = Some(json!({"invalid":"selection"}));
    core.download_manager
        .db()
        .upsert_download(&download)
        .await
        .unwrap();
    assert!(core.resume_download(download.id).await.is_err());
    assert_eq!(
        core.get_download(download.id).await.unwrap().status,
        DownloadStatus::Failed
    );
    assert!(!core.download_manager.is_active(download.id).await);
    core.youtube.shutdown().await;
    core.scheduler.stop().await;
}

#[tokio::test]
async fn pause_all_waits_for_the_managed_task_and_preserves_its_identity() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let root = std::env::temp_dir().join(format!("dlman-youtube-controls-{}", Uuid::new_v4()));
    let core = DlmanCore::new(root.clone()).await.unwrap();
    let mut download = Download::new(
        "https://www.youtube.com/watch?v=abcdefghijk".into(),
        root.join("downloads"),
        Uuid::nil(),
    );
    download.status = DownloadStatus::Downloading;
    download.youtube = Some(json!({"retained":"metadata"}));
    core.download_manager
        .db()
        .upsert_download(&download)
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let observed = stopped.clone();
    let join = tokio::spawn(async move {
        token.cancelled().await;
        observed.store(true, Ordering::Release);
        Ok(())
    });
    core.youtube
        .tasks
        .lock()
        .await
        .insert(download.id, YoutubeTask { cancel, join });
    core.pause_all_downloads().await.unwrap();
    assert!(stopped.load(Ordering::Acquire));
    let paused = core.get_download(download.id).await.unwrap();
    assert_eq!(paused.status, DownloadStatus::Paused);
    assert_eq!(paused.id, download.id);
    assert_eq!(paused.youtube, download.youtube);
    assert!(!core.youtube.tasks.lock().await.contains_key(&download.id));
    core.youtube.shutdown().await;
    core.scheduler.stop().await;
}
