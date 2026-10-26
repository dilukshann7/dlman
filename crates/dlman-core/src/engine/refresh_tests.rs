use super::*;

#[test]
fn partial_samples_cover_start_middle_and_end_without_duplicates() {
    let mut segment = Segment::new(7, 1000, 100000);
    segment.downloaded = 20000;
    assert_eq!(partial_samples(&[segment.clone()]).unwrap(), vec![
        (7, 1000, 0, 4096), (7, 1000, 10000, 4096), (7, 1000, 15904, 4096),
    ]);
    for length in [1, 4096] {
        segment.downloaded = length;
        assert_eq!(partial_samples(&[segment.clone()]).unwrap(), vec![(7, 1000, 0, length)]);
    }
    segment.downloaded = 0;
    assert!(partial_samples(&[segment]).unwrap().is_empty());
}

#[test]
fn content_ranges_are_exact_and_totals_are_required() {
    assert_eq!(parse_content_range("bytes 5-9/10").unwrap(), (5, 9, 10));
    for value in [
        "bytes 5-9/*",
        "bytes 9-5/10",
        "bytes 0-10/10",
        "items 0-1/2",
        "bytes 0-1/0",
        "bytes 0-1/2 extra",
        "bytes +0-0/1",
    ] {
        assert!(parse_content_range(value).is_err(), "{value}");
    }
}

#[test]
fn refresh_requires_an_ordinary_inactive_job() {
    let mut download = Download::new(
        "https://example.com/file".into(),
        "/tmp".into(),
        Uuid::nil(),
    );
    assert!(!eligible(&download));
    download.status = DownloadStatus::Paused;
    assert!(eligible(&download));
    for url in [
        "ftp://example.com/file",
        "https://youtu.be/123",
        "https://example.com/video.mpd",
        "https://example.com/video.m3u8?token=1",
    ] {
        download.url = url.into();
        assert!(!eligible(&download));
    }
}

#[test]
fn weak_etags_do_not_establish_identity() {
    assert_eq!(strong_etag(Some("W/\"abc\"")), None);
    assert_eq!(strong_etag(Some("\"abc\"")), Some("\"abc\""));
    assert_eq!(strong_etag(Some("abc")), None);
}

#[test]
fn url_validation_rejects_credentials_and_non_http_urls() {
    assert!(http_url("https://example.com/file?token=abc").is_ok());
    assert!(http_url("https://user:secret@example.com/file").is_err());
    assert!(http_url("file:///tmp/file").is_err());
}

#[test]
fn session_expires_and_verifying_sessions_do_not_capture() {
    let mut session = RefreshSession::new(Uuid::new_v4());
    assert!(session.live());
    assert!(session.can_capture());
    session.verifying = true;
    assert!(!session.can_capture());
    session.started = std::time::Instant::now() - SESSION_TTL;
    assert!(!session.live());
}

#[test]
fn candidate_display_redacts_query_and_fragment() {
    assert_eq!(
        display_url("https://example.com/file?token=secret#secret"),
        "https://example.com/file"
    );
}

#[test]
fn intended_html_files_are_distinguished_from_unexpected_html_pages() {
    assert!(expects_html("page.HTML"));
    assert!(expects_html("page.xhtml"));
    assert!(!expects_html("archive.zip"));
}

// These fixtures run only inside explicitly requested Rust tests. They are not
// started by normal application execution or the source verification workflow.
#[derive(Clone, Copy)]
enum Mode {
    Range,
    Ignored,
    Malformed,
    WrongTotal,
    Html,
    Encoded,
    Unauthorized,
    Truncated,
    Overlong,
    Slow,
}

struct Fixture {
    url: String,
    requests: std::sync::Arc<tokio::sync::Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(body: Vec<u8>, mode: Mode) -> Fixture {
    serve_with_etags(body, mode, Vec::new()).await
}

async fn serve_with_etags(body: Vec<u8>, mode: Mode, etags: Vec<&'static str>) -> Fixture {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let length = stream.read(&mut buffer).await.unwrap();
                if length == 0 || request.len() > 16384 {
                    break;
                }
                request.extend_from_slice(&buffer[..length]);
            }
            let request = String::from_utf8(request).unwrap();
            let request_index = captured.lock().await.len();
            captured.lock().await.push(request.clone());
            if matches!(mode, Mode::Slow) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let range = request
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("range").then_some(value.trim())
                })
                .unwrap_or("bytes=0-");
            let (start, end) = range
                .strip_prefix("bytes=")
                .unwrap()
                .split_once('-')
                .unwrap();
            let start: usize = start.parse().unwrap();
            let end: usize = if end.is_empty() {
                body.len() - 1
            } else {
                end.parse().unwrap()
            };
            let mut payload = body[start..=end].to_vec();
            let mut status = "206 Partial Content";
            let etag = etags.get(request_index).copied().unwrap_or("\"stable\"");
            let mut headers = format!(
                "Content-Range: bytes {start}-{end}/{}\r\nETag: {etag}\r\n",
                body.len()
            );
            match mode {
                Mode::Ignored => {
                    status = "200 OK";
                    payload = body.clone();
                    headers.clear();
                }
                Mode::Malformed => headers = "Content-Range: bytes nonsense\r\n".into(),
                Mode::WrongTotal => {
                    headers = format!("Content-Range: bytes {start}-{end}/{}\r\n", body.len() + 1)
                }
                Mode::Html => headers.push_str("Content-Type: text/html; charset=UTF-8\r\n"),
                Mode::Encoded => headers.push_str("Content-Encoding: gzip\r\n"),
                Mode::Unauthorized => status = "403 Forbidden",
                Mode::Overlong => payload.push(42),
                _ => {}
            }
            let advertised = payload.len();
            if matches!(mode, Mode::Truncated) {
                payload.pop();
            }
            let response = format!("HTTP/1.1 {status}\r\n{headers}Content-Length: {advertised}\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.write_all(&payload).await;
            let _ = stream.shutdown().await;
        }
    });
    Fixture {
        url: format!("http://{address}/file"),
        requests,
        task,
    }
}

fn client() -> Client {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
fn data() -> Vec<u8> {
    (0..16384).map(|n| (n % 251) as u8).collect()
}
fn candidate(url: String) -> RefreshCandidate {
    RefreshCandidate {
        info: RefreshCandidateInfo {
            candidate_id: Uuid::new_v4(),
            display_url: display_url(&url),
            filename: None,
        },
        url,
        source_page_url: None,
        cookies: None,
    }
}

async fn partial(root: &Path) -> Download {
    let body = data();
    let mut download = Download::new(
        "https://old.example/file".into(),
        root.into(),
        Uuid::new_v4(),
    );
    download.status = DownloadStatus::Paused;
    download.size = Some(body.len() as u64);
    download.etag = Some("\"stable\"".into());
    download.segment_cache_dir = Some(root.into());
    download.segments = vec![
        dlman_types::Segment::new(0, 0, 8191),
        dlman_types::Segment::new(1, 8192, 16383),
    ];
    for segment in &mut download.segments {
        segment.downloaded = 4096;
        tokio::fs::write(
            root.join(format!("{}_segment_{}.part", download.id, segment.index)),
            &body[segment.start as usize..segment.start as usize + 4096],
        )
        .await
        .unwrap();
    }
    download.downloaded = 8192;
    download
}

#[tokio::test]
async fn verifies_segmented_partial_without_changing_identity_or_files() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    let fixture = serve(data(), Mode::Range).await;
    let result = verify_candidate(
        &client(),
        download.clone(),
        candidate(fixture.url.clone()),
        directory.path(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.id, download.id);
    assert_eq!(result.downloaded, 8192);
    assert_eq!(result.filename, download.filename);
    assert_eq!(result.queue_id, download.queue_id);
    assert_eq!(result.created_at, download.created_at);
    assert_eq!(result.segments.len(), 2);
    assert_eq!(result.url, fixture.url);
    assert!(fixture.requests.lock().await.len() <= 3);
}

#[tokio::test]
async fn rejects_unsafe_responses_and_preserves_partial_bytes() {
    for mode in [
        Mode::Ignored,
        Mode::Malformed,
        Mode::WrongTotal,
        Mode::Html,
        Mode::Encoded,
        Mode::Unauthorized,
        Mode::Truncated,
        Mode::Overlong,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let download = partial(directory.path()).await;
        let path = directory
            .path()
            .join(format!("{}_segment_0.part", download.id));
        let before = tokio::fs::read(&path).await.unwrap();
        let fixture = serve(data(), mode).await;
        assert!(verify_candidate(
            &client(),
            download,
            candidate(fixture.url.clone()),
            directory.path(),
            None
        )
        .await
        .is_err());
        assert_eq!(tokio::fs::read(path).await.unwrap(), before);
    }
}

#[tokio::test]
async fn rejects_same_size_changed_content_and_conflicting_strong_etag() {
    let directory = tempfile::tempdir().unwrap();
    let mut download = partial(directory.path()).await;
    let fixture = serve(data().into_iter().map(|n| n ^ 255).collect(), Mode::Range).await;
    assert!(verify_candidate(
        &client(),
        download.clone(),
        candidate(fixture.url.clone()),
        directory.path(),
        None
    )
    .await
    .is_err());
    download.etag = Some("\"different\"".into());
    let fixture = serve(data(), Mode::Range).await;
    assert!(verify_candidate(
        &client(),
        download,
        candidate(fixture.url.clone()),
        directory.path(),
        None
    )
    .await
    .is_err());
}

#[tokio::test]
async fn missing_partial_does_not_become_a_fresh_download() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    tokio::fs::remove_file(
        directory
            .path()
            .join(format!("{}_segment_1.part", download.id)),
    )
    .await
    .unwrap();
    let fixture = serve(data(), Mode::Range).await;
    assert!(verify_candidate(
        &client(),
        download,
        candidate(fixture.url.clone()),
        directory.path(),
        None
    )
    .await
    .is_err());
    assert!(fixture.requests.lock().await.is_empty());
}

#[tokio::test]
async fn unknown_saved_size_is_resolved_by_exact_range_total() {
    let directory = tempfile::tempdir().unwrap();
    let mut download = partial(directory.path()).await;
    download.size = None;
    download.segments.truncate(1);
    download.segments[0].end = u64::MAX;
    download.downloaded = 4096;
    let fixture = serve(data(), Mode::Range).await;
    let manager = manager(directory.path(), &download).await;
    let result = verify_candidate(
        &client(),
        download,
        candidate(fixture.url.clone()),
        directory.path(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.size, Some(16384));
    assert_eq!(result.segments[0].end, 16383);
    manager.db().rebind_download(&result).await.unwrap();
    assert_eq!(
        manager
            .db()
            .load_download(result.id)
            .await
            .unwrap()
            .unwrap()
            .segments[0]
            .end,
        16383
    );
}

#[tokio::test]
async fn zero_bytes_can_probe_a_server_without_range_support() {
    let directory = tempfile::tempdir().unwrap();
    let mut download = Download::new(
        "https://old.example/file".into(),
        directory.path().into(),
        Uuid::nil(),
    );
    download.status = DownloadStatus::Failed;
    let fixture = serve(data(), Mode::Ignored).await;
    let result = verify_candidate(
        &client(),
        download,
        candidate(fixture.url.clone()),
        directory.path(),
        None,
    )
    .await
    .unwrap();
    assert!(!result.supports_range);
    assert_eq!(result.downloaded, 0);
    assert_eq!(result.size, Some(16384));
}

async fn manager(
    root: &Path,
    download: &Download,
) -> std::sync::Arc<super::super::DownloadManager> {
    let (events, _) = tokio::sync::broadcast::channel(100);
    let manager = std::sync::Arc::new(
        super::super::DownloadManager::new(root.into(), events)
            .await
            .unwrap(),
    );
    manager.db().upsert_download(download).await.unwrap();
    manager
}

#[tokio::test]
async fn reservations_block_competing_controls_and_require_confirmation() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    let manager = manager(directory.path(), &download).await;
    let session = manager.begin_refresh(download.id).await.unwrap();
    assert!(manager.begin_refresh(download.id).await.is_err());
    assert!(manager.edit_guard(download.id).await.is_err());
    assert!(manager
        .resume(download.id, None, 2, 0, 0, None)
        .await
        .is_err());
    let captured = manager
        .capture_refresh_candidate(
            "https://new.example/file?secret=1".into(),
            None,
            None,
            Some("session=secret".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        manager
            .db()
            .load_download(download.id)
            .await
            .unwrap()
            .unwrap()
            .url,
        download.url
    );
    assert!(manager
        .capture_refresh_candidate("https://other.example/file".into(), None, None, None)
        .await
        .is_none());
    let released = manager
        .release_refresh_candidate(session.session_id, captured.candidate.unwrap().candidate_id)
        .await
        .unwrap();
    assert_eq!(released.cookies.as_deref(), Some("session=secret"));
    manager.cancel_refresh(session.session_id).await;
    assert!(manager.edit_guard(download.id).await.is_ok());
}

#[tokio::test]
async fn expiry_releases_reservation_and_stale_session_cannot_apply() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    let manager = manager(directory.path(), &download).await;
    let session = manager.begin_refresh(download.id).await.unwrap();
    manager.refresh.lock().await.as_mut().unwrap().started = Instant::now() - SESSION_TTL;
    assert!(manager.edit_guard(download.id).await.is_ok());
    assert!(manager
        .refresh_address(
            session.session_id,
            Some("https://example.com/file".into()),
            None
        )
        .await
        .is_err());
}

#[tokio::test]
async fn cancel_during_verification_and_duplicate_submission_cannot_commit() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    let manager = manager(directory.path(), &download).await;
    let session = manager.begin_refresh(download.id).await.unwrap();
    let fixture = serve(data(), Mode::Slow).await;
    let task_manager = manager.clone();
    let url = fixture.url.clone();
    let work = tokio::spawn(async move {
        task_manager
            .refresh_address(session.session_id, Some(url), None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.requests.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(manager
        .refresh_address(session.session_id, Some(fixture.url.clone()), None)
        .await
        .is_err());
    manager.cancel_refresh(session.session_id).await;
    assert!(work.await.unwrap().is_err());
    assert_eq!(
        manager
            .db()
            .load_download(download.id)
            .await
            .unwrap()
            .unwrap()
            .url,
        download.url
    );
}

#[tokio::test]
async fn rebind_preserves_metadata_and_segment_rows() {
    use sqlx::Row;
    let directory = tempfile::tempdir().unwrap();
    let mut download = partial(directory.path()).await;
    download.category_id = Some(Uuid::new_v4());
    download.color = Some("#123456".into());
    download.speed_limit = Some(12345);
    download.source_page_url = Some("https://example.com/page".into());
    let manager = manager(directory.path(), &download).await;
    let options =
        sqlx::sqlite::SqliteConnectOptions::new().filename(directory.path().join("downloads.db"));
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("UPDATE segments SET temp_file_path = 'preserve-this-value'")
        .execute(&pool)
        .await
        .unwrap();
    let mut replacement = download.clone();
    replacement.url = "https://replacement.example/file".into();
    manager.db().rebind_download(&replacement).await.unwrap();
    let loaded = manager
        .db()
        .load_download(download.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.url, replacement.url);
    assert_eq!(loaded.category_id, download.category_id);
    assert_eq!(loaded.color, download.color);
    assert_eq!(loaded.speed_limit, download.speed_limit);
    assert_eq!(loaded.destination, download.destination);
    assert_eq!(loaded.filename, download.filename);
    assert_eq!(loaded.created_at, download.created_at);
    assert_eq!(loaded.source_page_url, download.source_page_url);
    let rows = sqlx::query("SELECT temp_file_path FROM segments")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|row| row.get::<String, _>("temp_file_path") == "preserve-this-value"));
}

#[tokio::test]
async fn worker_downloads_the_last_byte_and_rejects_bad_responses_without_appending() {
    use super::super::{DownloadDatabase, RateLimiter, SegmentWorker};
    use std::sync::{
        atomic::{AtomicBool, AtomicU64},
        Arc,
    };
    for mode in [Mode::Range, Mode::Ignored, Mode::Truncated, Mode::Overlong] {
        let directory = tempfile::tempdir().unwrap();
        let fixture = serve(data(), mode).await;
        let mut download = Download::new(fixture.url.clone(), directory.path().into(), Uuid::nil());
        download.size = Some(16384);
        download.segments = vec![dlman_types::Segment::new(0, 0, 16383)];
        download.segments[0].downloaded = 16383;
        download.downloaded = 16383;
        let path = directory
            .path()
            .join(format!("{}_segment_0.part", download.id));
        tokio::fs::write(&path, &data()[..16383]).await.unwrap();
        let db = DownloadDatabase::new(directory.path().join("downloads.db"))
            .await
            .unwrap();
        db.upsert_download(&download).await.unwrap();
        let (events, _) = tokio::sync::broadcast::channel(100);
        let worker = SegmentWorker::new(
            download.id,
            download.segments[0].clone(),
            fixture.url.clone(),
            directory.path().into(),
            client(),
            RateLimiter::unlimited(),
            db.clone(),
            events,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(16383)),
        )
        .with_http_context(download.size, Some("\"stable\"".into()), None, false);
        let result = worker.run().await;
        if matches!(mode, Mode::Range) {
            assert!(result.is_ok());
            assert_eq!(tokio::fs::read(&path).await.unwrap(), data());
            assert!(
                db.load_download(download.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .segments[0]
                    .complete
            );
            assert!(fixture.requests.lock().await[0].contains("16383-16383"));
        } else {
            assert!(result.is_err());
            assert_eq!(tokio::fs::read(path).await.unwrap(), data()[..16383]);
            assert!(
                !db.load_download(download.id)
                    .await
                    .unwrap()
                    .unwrap()
                    .segments[0]
                    .complete
            );
        }
    }
}

async fn redirector(location: String) -> Fixture {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 2048];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let length = stream.read(&mut buffer).await.unwrap();
                if length == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..length]);
            }
            captured
                .lock()
                .await
                .push(String::from_utf8(request).unwrap());
            let response = format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    Fixture {
        url: format!("http://{address}/file"),
        requests,
        task,
    }
}

#[tokio::test]
async fn redirect_to_another_origin_drops_cookies_and_authorization() {
    let target = serve(data(), Mode::Range).await;
    let source = redirector(target.url.clone()).await;
    let credentials = ("user".into(), "secret".into());
    let response = send_scoped(
        &client(),
        Method::GET,
        &source.url,
        Some("session=secret"),
        Some(&credentials),
        Some("https://page.example/private?token=secret"),
        Some("bytes=0-0"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(bounded_body(response, 1).await.unwrap(), vec![0]);
    let first = source.requests.lock().await[0].to_lowercase();
    let second = target.requests.lock().await[0].to_lowercase();
    assert!(first.contains("cookie: session=secret"));
    assert!(first.contains("authorization: basic"));
    assert!(!second.contains("cookie:"));
    assert!(!second.contains("authorization:"));
    assert!(!second.contains("token=secret"));
}

#[tokio::test]
async fn migration_defaults_are_nullable_and_existing_validators_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let mut download = partial(directory.path()).await;
    download.last_modified = Some("Wed, 21 Oct 2015 07:28:00 GMT".into());
    download.supports_range = true;
    let manager = manager(directory.path(), &download).await;
    let options =
        sqlx::sqlite::SqliteConnectOptions::new().filename(directory.path().join("downloads.db"));
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    sqlx::query("ALTER TABLE downloads DROP COLUMN source_page_url")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("ALTER TABLE downloads DROP COLUMN segment_cache_dir")
        .execute(&pool)
        .await
        .unwrap();
    let db = super::super::DownloadDatabase::new(directory.path().join("downloads.db"))
        .await
        .unwrap();
    let loaded = db.load_download(download.id).await.unwrap().unwrap();
    assert!(loaded.source_page_url.is_none());
    assert!(loaded.segment_cache_dir.is_none());
    assert_eq!(loaded.etag, download.etag);
    assert_eq!(loaded.last_modified, download.last_modified);
    assert!(loaded.supports_range);
    drop(manager);
}

#[tokio::test]
async fn deletion_during_verification_invalidates_the_pending_commit() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    let manager = manager(directory.path(), &download).await;
    let session = manager.begin_refresh(download.id).await.unwrap();
    let fixture = serve(data(), Mode::Slow).await;
    let task_manager = manager.clone();
    let url = fixture.url.clone();
    let work = tokio::spawn(async move {
        task_manager
            .refresh_address(session.session_id, Some(url), None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.requests.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    manager.delete(download.id, false).await.unwrap();
    assert!(work.await.unwrap().is_err());
    assert!(manager
        .db()
        .load_download(download.id)
        .await
        .unwrap()
        .is_none());
    assert!(manager.refresh.lock().await.is_none());
}

#[tokio::test]
async fn saved_replacement_is_preserved_if_subsequent_start_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let download = partial(directory.path()).await;
    let manager = manager(directory.path(), &download).await;
    let session = manager.begin_refresh(download.id).await.unwrap();
    let fixture = serve(data(), Mode::Range).await;
    manager
        .refresh_address(session.session_id, Some(fixture.url.clone()), None)
        .await
        .unwrap();
    manager.cancel(download.id).await.unwrap();
    assert!(manager
        .resume(download.id, None, 2, 0, 0, None)
        .await
        .is_err());
    assert_eq!(
        manager
            .db()
            .load_download(download.id)
            .await
            .unwrap()
            .unwrap()
            .url,
        fixture.url
    );
}

#[tokio::test]
async fn a_weak_validator_cannot_hide_conflicting_strong_sample_validators() {
    let directory = tempfile::tempdir().unwrap();
    let mut download = partial(directory.path()).await;
    download.etag = None;
    download.downloaded = 16384;
    for segment in &mut download.segments {
        segment.downloaded = 8192;
        tokio::fs::write(
            directory
                .path()
                .join(format!("{}_segment_{}.part", download.id, segment.index)),
            &data()[segment.start as usize..segment.start as usize + 8192],
        )
        .await
        .unwrap();
    }
    let fixture = serve_with_etags(
        data(),
        Mode::Range,
        vec!["\"first\"", "W/\"first\"", "\"different\""],
    )
    .await;
    assert!(verify_candidate(
        &client(),
        download,
        candidate(fixture.url.clone()),
        directory.path(),
        None
    )
    .await
    .is_err());
    assert_eq!(fixture.requests.lock().await.len(), 3);
}

#[tokio::test]
async fn start_does_not_send_credentials_from_a_stale_resume_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let mut download = partial(directory.path()).await;
    let fixture = serve(data(), Mode::Range).await;
    download.url = fixture.url.clone();
    let manager = manager(directory.path(), &download).await;
    manager
        .start(
            download.clone(),
            None,
            2,
            0,
            0,
            Some(("old-origin-user".into(), "old-origin-secret".into())),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.requests.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!fixture.requests.lock().await[0]
        .to_lowercase()
        .contains("authorization:"));
    tokio::time::timeout(Duration::from_secs(2), async {
        while manager.is_active(download.id).await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
