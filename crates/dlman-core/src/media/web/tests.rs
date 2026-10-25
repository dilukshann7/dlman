use super::*;
use crate::media::web::probe::{classify, ResourceKind};

const PAGE: &str = "https://example.com/watch?id=7#video";
const SOURCE: &str = "https://cdn.example.com/media/master?token=first";

fn claim() -> WebMediaCaptureClaim {
    WebMediaCaptureClaim {
        client_id: "test-client".into(),
        tab_id: 12,
    }
}

fn cookie(domain: &str, path: &str) -> YoutubeSessionCookie {
    YoutubeSessionCookie {
        domain: domain.into(),
        path: path.into(),
        name: "session".into(),
        value: "token".into(),
        secure: true,
        http_only: true,
        host_only: false,
        expires: None,
    }
}

fn source(url: &str) -> WebMediaCapturedSource {
    WebMediaCapturedSource {
        url: url.into(),
        title: Some("Example video".into()),
        protocol: "hls".into(),
        referrer: Some(PAGE.into()),
        user_agent: Some("Test browser".into()),
        cookies: Vec::new(),
    }
}

fn submission(sources: Vec<WebMediaCapturedSource>) -> WebMediaCaptureSubmission {
    WebMediaCaptureSubmission {
        client_id: "test-client".into(),
        tab_id: 12,
        page_url: PAGE.into(),
        sources,
    }
}

fn ready_store() -> (capture::CaptureStore, WebMediaCapture) {
    let mut store = capture::CaptureStore::default();
    let ticket = store.begin(PAGE).unwrap();
    store.claim(ticket.id, claim()).unwrap();
    let capture = store
        .submit(ticket.id, submission(vec![source(SOURCE)]))
        .unwrap();
    (store, capture)
}

fn preview() -> WebMediaPreview {
    WebMediaPreview {
        url: PAGE.into(),
        identity: "generic:7".into(),
        title: "Example".into(),
        variants: vec![YoutubeVariant {
            id: "v1+a1".into(),
            quality: "1080p".into(),
            container: "mp4".into(),
            audio: "AAC".into(),
            size: Some(1024),
            fps: Some(30),
            output_mode: YoutubeOutput::Video,
        }],
    }
}

#[tokio::test]
async fn automatic_inspection_reuses_pending_work_and_reserves_the_other_slot_for_explicit_work() {
    let service = Arc::new(WebMediaService::new(Arc::new(youtube::YoutubeService::default())));
    let id = Uuid::new_v4();
    service.inspections.lock().await.insert(id, InspectionEntry {
        page: PAGE.into(), source_key: None, revision: None, fingerprint: "test".into(),
        created: Instant::now(),
        result: WebMediaInspection { id, status: "pending".into(), preview: None, error: None },
    });
    *service.automatic_inspection.lock().await = Some(id);
    let same = service.inspect_automatic(PAGE, ProxySettings::default()).await.unwrap();
    assert_eq!(same.id, id);
    assert!(service.inspect_automatic("https://example.com/another", ProxySettings::default()).await.is_err());
    assert_eq!(service.slots.available_permits(), 2);
}

fn selection() -> WebMediaSelection {
    WebMediaSelection {
        identity: "generic:7".into(),
        format_id: "v1+a1".into(),
        quality: "1080p".into(),
        container: "mp4".into(),
        output_mode: YoutubeOutput::Video,
    }
}

#[test]
fn kvs_session_tokens_rotate_without_changing_the_selected_source() {
    let original = capture::source_key(
        PAGE,
        "https://cdn.example.com/get_file/42_720p.mp4/?v-acctoken=old&quality=720",
    )
    .unwrap();
    let refreshed = capture::source_key(
        PAGE,
        "https://cdn.example.com/get_file/42_720p.mp4/?v-acctoken=new&quality=720",
    )
    .unwrap();
    let other = capture::source_key(
        PAGE,
        "https://cdn.example.com/get_file/42_480p.mp4/?v-acctoken=new&quality=480",
    )
    .unwrap();
    assert_eq!(original, refreshed);
    assert_ne!(original, other);
}

#[test]
fn classification_uses_mime_and_extensionless_manifest_prefixes() {
    assert_eq!(
        classify(
            "application/octet-stream",
            b"\xef\xbb\xbf  #EXTM3U\n#EXT-X-VERSION:3"
        ),
        ResourceKind::Manifest
    );
    assert_eq!(
        classify(
            "application/octet-stream",
            br#"<?xml version="1.0"?><MPD xmlns="urn:mpeg:dash:schema:mpd:2011">"#
        ),
        ResourceKind::Manifest
    );
    assert_eq!(
        classify("application/dash+xml; charset=utf-8", b""),
        ResourceKind::Manifest
    );
    assert_eq!(
        classify("text/plain", b"<!DOCTYPE html><html>"),
        ResourceKind::Page
    );
    assert_eq!(
        classify("text/html; charset=utf-8", b""),
        ResourceKind::Page
    );
    assert_eq!(
        classify("video/mp4", b"\0\0\0\x18ftypmp42"),
        ResourceKind::File
    );
    assert_eq!(
        classify("application/zip", b"PK\x03\x04"),
        ResourceKind::File
    );
    assert_eq!(
        classify("application/xml", b"<?xml version=\"1.0\"?><html>"),
        ResourceKind::Page
    );
    assert_eq!(
        classify("text/x-unknown", b"page document"),
        ResourceKind::Page
    );
    assert_eq!(
        classify("text/plain", b"plain document"),
        ResourceKind::File
    );
    assert_eq!(
        classify("text/csv", b"name,count\nexample,1"),
        ResourceKind::File
    );
}

#[test]
fn urls_preserve_queries_and_fragments_and_reject_credentials_and_youtube() {
    assert_eq!(canonical_url(PAGE).unwrap(), PAGE);
    for value in [
        "file:///secret",
        "ftp://example.com/x",
        "https://user:pass@example.com/x",
        "https://example.com/\r\nx",
        "https://music.youtube.com/watch?v=123",
        "https://youtu.be/123",
        "https://www.youtube-nocookie.com/embed/123",
    ] {
        assert!(canonical_url(value).is_err(), "accepted invalid URL");
    }
    assert!(canonical_url("https://youtube.com.example.com/video").is_ok());
}

#[test]
fn captures_bind_owner_page_and_keep_multiple_sources() {
    let mut store = capture::CaptureStore::default();
    let ticket = store.begin(PAGE).unwrap();
    assert!(store
        .submit(ticket.id, submission(vec![source(SOURCE)]))
        .is_err());
    store.claim(ticket.id, claim()).unwrap();
    assert!(store
        .claim(
            ticket.id,
            WebMediaCaptureClaim {
                client_id: "other".into(),
                tab_id: 12
            }
        )
        .is_err());
    let mut moved = submission(vec![source(SOURCE)]);
    moved.page_url = "https://example.com/other".into();
    assert!(store.submit(ticket.id, moved).is_err());
    let first = store
        .submit(ticket.id, submission(vec![source(SOURCE)]))
        .unwrap();
    let second = store
        .submit(
            ticket.id,
            submission(vec![source("https://cdn.example.com/other/master")]),
        )
        .unwrap();
    assert_eq!(second.sources.len(), 2);
    assert!(second
        .sources
        .iter()
        .any(|source| source.id == first.sources[0].id));
    assert!(store
        .cancel_claim(
            ticket.id,
            WebMediaCaptureClaim {
                client_id: "other".into(),
                tab_id: 12
            }
        )
        .is_err());
}

#[test]
fn signed_source_refreshes_same_job_key_and_cancellation_preserves_context() {
    let (mut store, capture) = ready_store();
    let key = &capture.sources[0].id;
    let (_, old_revision) = store.context(PAGE, key).unwrap();
    store
        .submit(capture.id, submission(vec![source(SOURCE)]))
        .unwrap();
    assert_eq!(store.context(PAGE, key).unwrap().1, old_revision);
    let refreshed = "https://cdn.example.com/media/master?token=refreshed";
    let capture = store
        .submit(capture.id, submission(vec![source(refreshed)]))
        .unwrap();
    assert_eq!(capture.sources.len(), 1);
    assert_eq!(&capture.sources[0].id, key);
    let (context, new_revision) = store.context(PAGE, key).unwrap();
    assert_eq!(context.url, refreshed);
    assert_ne!(old_revision, new_revision);
    store.cancel(capture.id);
    assert_eq!(store.capture(capture.id).unwrap().status, "cancelled");
    assert_eq!(store.context(PAGE, key).unwrap().0.url, refreshed);
    assert!(store.context("https://example.com/other", key).is_err());
    assert!(store.selected(capture.id, PAGE, key).is_err());
}

#[test]
fn source_identity_preserves_video_and_quality_selectors() {
    let a = capture::source_key(
        PAGE,
        "https://cdn.example.com/manifest?video=1&quality=720&token=old",
    )
    .unwrap();
    let refreshed = capture::source_key(
        PAGE,
        "https://cdn.example.com/manifest?token=new&quality=720&video=1",
    )
    .unwrap();
    let other_video = capture::source_key(
        PAGE,
        "https://cdn.example.com/manifest?video=2&quality=720&token=new",
    )
    .unwrap();
    let other_quality = capture::source_key(
        PAGE,
        "https://cdn.example.com/manifest?video=1&quality=1080&token=new",
    )
    .unwrap();
    assert_eq!(a, refreshed);
    assert_ne!(a, other_video);
    assert_ne!(a, other_quality);
    assert_ne!(
        capture::source_key(PAGE, "https://cdn.example.com/manifest?unknown_auth=old").unwrap(),
        capture::source_key(PAGE, "https://cdn.example.com/manifest?unknown_auth=new").unwrap()
    );
}

#[test]
fn cookies_are_filtered_for_domain_path_secure_and_header_injection() {
    let context = WebMediaContext {
        url: SOURCE.into(),
        referrer: Some(PAGE.into()),
        user_agent: None,
        cookies: vec![
            cookie(".example.com", "/media"),
            cookie("other.example.org", "/"),
            cookie(".example.com", "/unrelated"),
        ],
        cookie_header: None,
    }
    .checked()
    .unwrap();
    assert_eq!(context.cookies.len(), 1);
    assert_eq!(context.cookies[0].domain, "cdn.example.com");
    assert!(context.cookies[0].host_only);
    let mut injection = context.clone();
    injection.cookies[0].value = "token\r\nInjected: true".into();
    assert!(injection.checked().is_err());
    let mut injection = context.clone();
    injection.user_agent = Some("browser\nCookie: stolen".into());
    assert!(injection.checked().is_err());
    let mut insecure = context;
    insecure.url = "http://cdn.example.com/media/master".into();
    assert!(insecure.checked().unwrap().cookies.is_empty());
}

#[test]
fn legacy_cookie_header_uses_host_scoped_file_and_never_global_header_argument() {
    let context = legacy_context(
        SOURCE,
        Some(PAGE.into()),
        Some("sid=secret; token=second".into()),
    )
    .unwrap();
    assert!(context.cookie_header.is_none());
    let (args, file) = context.tool_args().unwrap();
    assert!(!args
        .iter()
        .any(|arg| arg.contains("secret") || arg.contains("--add-header")));
    let file = file.unwrap();
    let path = file.path().to_owned();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("#HttpOnly_cdn.example.com\tFALSE\t/\tTRUE\t0\tsid\tsecret"));
    assert!(!context::cookie_matches(
        &context.cookies[0],
        &url::Url::parse("https://different.example.com/").unwrap()
    ));
    drop(file);
    assert!(!path.exists());
    assert!(legacy_context(SOURCE, None, Some("sid=secret\r\nInjected: true".into())).is_err());
}

#[tokio::test]
async fn previews_bind_page_selection_output_and_expiry() {
    let service = WebMediaService::new(Arc::new(youtube::YoutubeService::default()));
    let id = Uuid::new_v4();
    service.inspections.lock().await.insert(
        id,
        InspectionEntry {
            page: PAGE.into(),
            source_key: None,
            revision: None,
            fingerprint: "test".into(),
            created: Instant::now(),
            result: WebMediaInspection {
                id,
                status: "ready".into(),
                preview: Some(preview()),
                error: None,
            },
        },
    );
    assert!(service
        .validate_preview(id, PAGE, &selection())
        .await
        .is_ok());
    assert!(service
        .validate_preview(id, "https://example.com/watch?id=8", &selection())
        .await
        .is_err());
    let mut wrong = selection();
    wrong.format_id = "another-format".into();
    assert!(service.validate_preview(id, PAGE, &wrong).await.is_err());
    let mut wrong = selection();
    wrong.output_mode = YoutubeOutput::Mp3;
    assert!(service.validate_preview(id, PAGE, &wrong).await.is_err());
    service
        .inspections
        .lock()
        .await
        .get_mut(&id)
        .unwrap()
        .created = Instant::now() - Duration::from_secs(121);
    assert!(service
        .validate_preview(id, PAGE, &selection())
        .await
        .is_err());
}

#[tokio::test]
async fn refreshed_context_invalidates_preview_but_job_reads_latest_source() {
    let service = WebMediaService::new(Arc::new(youtube::YoutubeService::default()));
    let (store, capture) = ready_store();
    *service.captures.lock().await = store;
    let key = capture.sources[0].id.clone();
    let revision = service.captures.lock().await.context(PAGE, &key).unwrap().1;
    let id = Uuid::new_v4();
    service.inspections.lock().await.insert(
        id,
        InspectionEntry {
            page: PAGE.into(),
            source_key: Some(key.clone()),
            revision: Some(revision),
            fingerprint: "test".into(),
            created: Instant::now(),
            result: WebMediaInspection {
                id,
                status: "ready".into(),
                preview: Some(preview()),
                error: None,
            },
        },
    );
    assert!(service
        .validate_preview(id, PAGE, &selection())
        .await
        .is_ok());
    service
        .submit_capture(
            capture.id,
            submission(vec![source(
                "https://cdn.example.com/media/master?token=new",
            )]),
        )
        .await
        .unwrap();
    assert!(service
        .validate_preview(id, PAGE, &selection())
        .await
        .is_err());
    service.cancel_capture(capture.id).await;
    let context = service.context_for_job(PAGE, Some(&key)).await.unwrap();
    assert!(context.url.ends_with("token=new"));
    assert!(service
        .context_for_job(PAGE, Some("missing"))
        .await
        .is_err());
}

#[test]
fn persisted_jobs_contain_only_selection_stage_and_opaque_key() {
    let value = serde_json::to_value(WebMediaJob {
        selection: selection(),
        stage_dir: std::path::PathBuf::from("staging"),
        source_key: Some("opaque-key".into()),
    })
    .unwrap();
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 3);
    for secret in ["url", "cookies", "referrer", "user_agent", "headers"] {
        assert!(!object.contains_key(secret));
    }
    let mut legacy = object.clone();
    legacy.remove("source_key");
    assert!(
        serde_json::from_value::<WebMediaJob>(serde_json::Value::Object(legacy))
            .unwrap()
            .source_key
            .is_none()
    );
}

#[tokio::test]
async fn ordinary_cookie_context_still_classifies_as_file() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/archive.zip", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let count = stream.read(&mut request).await.unwrap();
        assert!(String::from_utf8_lossy(&request[..count])
            .to_ascii_lowercase()
            .contains("cookie: sid=secret"));
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: 4\r\n\r\nPK\x03\x04").await.unwrap();
    });
    let service = Arc::new(WebMediaService::new(Arc::new(
        youtube::YoutubeService::default(),
    )));
    let context = legacy_context(&url, None, Some("sid=secret".into())).unwrap();
    let pending = service
        .inspect(
            &url,
            ProxySettings {
                mode: "none".into(),
                ..ProxySettings::default()
            },
            None,
            None,
            Some(context),
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = service.inspection(pending.id).await.unwrap();
            if result.status != "pending" {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result.status, "file");
    server.await.unwrap();
    service.shutdown().await;
}

#[test]
fn changing_saved_probe_credentials_invalidates_cached_inspection_identity() {
    let context = WebMediaContext::anonymous(PAGE).unwrap();
    let proxy = ProxySettings::default();
    let first = ("user".into(), "first-password".into());
    let second = ("user".into(), "second-password".into());
    let first_key = inspection_fingerprint_with_credentials(&context, &proxy, Some(&first));
    assert_eq!(first_key.len(), 64);
    assert!(!first_key.contains("password"));
    assert_ne!(
        first_key,
        inspection_fingerprint_with_credentials(&context, &proxy, Some(&second))
    );
    assert_ne!(
        first_key,
        inspection_fingerprint_with_credentials(&context, &proxy, None)
    );
}

#[tokio::test]
async fn legacy_manifest_binds_to_its_referrer_page_without_forwarding_page_credentials() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source_url = format!(
        "http://{}/master?token=private",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        assert!(!String::from_utf8_lossy(&request)
            .to_ascii_lowercase()
            .contains("authorization:"));
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/vnd.apple.mpegurl\r\nContent-Length: 8\r\n\r\n#EXTM3U\n").await.unwrap();
    });
    let service = Arc::new(WebMediaService::new(Arc::new(
        youtube::YoutubeService::default(),
    )));
    let context = legacy_context(&source_url, Some(PAGE.into()), None).unwrap();
    let pending = service
        .inspect_with_probe_credentials(
            PAGE,
            ProxySettings {
                mode: "none".into(),
                ..ProxySettings::default()
            },
            None,
            None,
            Some(context),
            Some(("user".into(), "pass".into())),
        )
        .await
        .unwrap();
    let key = service
        .inspections
        .lock()
        .await
        .get(&pending.id)
        .unwrap()
        .source_key
        .clone()
        .unwrap();
    let cached = service.context_for_job(PAGE, Some(&key)).await.unwrap();
    assert_eq!(cached.url, source_url);
    assert_eq!(cached.referrer.as_deref(), Some(PAGE));
    assert!(cached.cookies.is_empty());
    assert_eq!(key, capture::source_key(PAGE, &source_url).unwrap());
    assert!(service
        .inspect(
            PAGE,
            ProxySettings::default(),
            None,
            None,
            Some(
                legacy_context(&source_url, Some("https://other.example.com/".into()), None)
                    .unwrap()
            )
        )
        .await
        .is_err());
    server.await.unwrap();
    service.shutdown().await;
}

#[tokio::test]
async fn distinct_legacy_binary_video_never_returns_webpage_file_fallback() {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = format!("http://{}/video.mp4", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 8\r\n\r\nftypmp42",
            )
            .await
            .unwrap();
    });
    // Bundled tools intentionally remain unconfigured: the expected failure proves
    // this distinct source reached extraction instead of returning the webpage as a file.
    let service = Arc::new(WebMediaService::new(Arc::new(
        youtube::YoutubeService::default(),
    )));
    let context = legacy_context(&source, Some(PAGE.into()), None).unwrap();
    let pending = service
        .inspect(
            PAGE,
            ProxySettings {
                mode: "none".into(),
                ..ProxySettings::default()
            },
            None,
            None,
            Some(context),
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let result = service.inspection(pending.id).await.unwrap();
            if result.status != "pending" {
                break result;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result.status, "failed");
    assert!(result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Bundled YouTube tools are unavailable")));
    server.await.unwrap();
    service.shutdown().await;
}
