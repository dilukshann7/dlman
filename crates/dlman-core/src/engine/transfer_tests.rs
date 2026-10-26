//! Authored regression fixtures; running them requires explicit test authorization.

use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Interrupted,
    MissingEtag,
    WeakEtag,
    MissingResponseEtag,
    CleanTruncated,
    ChangedSample,
    ChangedEtag,
    ChangedTotal,
    IgnoredRange,
    UnknownSize,
    UnknownLength,
    DiscoveredSizeChanged,
    AlwaysInterrupted,
    StalledHeaders,
    StalledBody,
    StalledSample,
    Continuous,
    InvalidFinalChunk,
    FullPendingEof,
}

struct Fixture {
    url: String,
    body: Arc<Vec<u8>>,
    requests: Arc<Mutex<Vec<String>>>,
    interrupted: Arc<Notify>,
    sample_started: Arc<Notify>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn serve(mode: Mode) -> Fixture {
    let body: Arc<Vec<u8>> = Arc::new((0..65536).map(|n| (n % 251) as u8).collect());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let interrupted = Arc::new(Notify::new());
    let sample_started = Arc::new(Notify::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = {
        let body = body.clone();
        let requests = requests.clone();
        let interrupted = interrupted.clone();
        let sample_started = sample_started.clone();
        tokio::spawn(async move {
            let attempts = Arc::new(AtomicUsize::new(0));
            let mut handlers = JoinSet::new();
            loop {
                let (mut socket, _) = tokio::select! {
                    accepted = listener.accept() => accepted.unwrap(),
                    _ = handlers.join_next(), if !handlers.is_empty() => continue,
                };
                let body = body.clone();
                let requests = requests.clone();
                let interrupted = interrupted.clone();
                let sample_started = sample_started.clone();
                let attempts = attempts.clone();
                handlers.spawn(async move {
                    let mut bytes = Vec::new();
                    while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                        let mut buffer = [0; 1024];
                        let count = socket.read(&mut buffer).await.unwrap();
                        if count == 0 { return; }
                        bytes.extend_from_slice(&buffer[..count]);
                        assert!(bytes.len() < 16384);
                    }
                    let request = String::from_utf8(bytes).unwrap();
                    requests.lock().await.push(request.clone());
                    let header = |name: &str| request.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case(name).then_some(value.trim())
                    });
                    let range = header("range");
                    let (start, end) = range.map(|value| {
                        let (start, end) = value.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
                        (start.parse::<usize>().unwrap(), if end.is_empty() { body.len() - 1 } else { end.parse().unwrap() })
                    }).unwrap_or((0, body.len() - 1));
                    let sample = end - start + 1 <= 4096;
                    let attempt = if sample { attempts.load(Ordering::Acquire) } else { attempts.fetch_add(1, Ordering::AcqRel) };
                    if mode == Mode::StalledHeaders {
                        interrupted.notify_one();
                        std::future::pending::<()>().await;
                    }
                    if sample && mode == Mode::StalledSample {
                        sample_started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    let etag = match mode {
                        Mode::MissingEtag | Mode::MissingResponseEtag | Mode::ChangedSample | Mode::StalledSample => "",
                        Mode::WeakEtag => "ETag: W/\"stable\"\r\n",
                        Mode::ChangedEtag if attempt > 0 => "ETag: \"changed\"\r\n",
                        _ => "ETag: \"stable\"\r\n",
                    };
                    let unknown = mode == Mode::UnknownSize && attempt == 0;
                    let ignored = mode == Mode::IgnoredRange && attempt > 0;
                    let partial = range.is_some() && !ignored;
                    let total = body.len() + usize::from(matches!(mode, Mode::ChangedTotal | Mode::DiscoveredSizeChanged) && attempt > 0);
                    let range_header = if partial { format!("Content-Range: bytes {start}-{end}/{total}\r\n") } else { String::new() };
                    let chunked = unknown || matches!(mode, Mode::InvalidFinalChunk | Mode::FullPendingEof);
                    let length = if chunked { "Transfer-Encoding: chunked\r\n".to_string() }
                        else if mode == Mode::CleanTruncated { String::new() }
                        else { format!("Content-Length: {}\r\n", end - start + 1) };
                    let status = if partial { "206 Partial Content" } else { "200 OK" };
                    let headers = format!("HTTP/1.1 {status}\r\n{range_header}{length}{etag}Connection: close\r\n\r\n");
                    if socket.write_all(headers.as_bytes()).await.is_err() { return; }
                    if mode == Mode::StalledBody {
                        interrupted.notify_one();
                        std::future::pending::<()>().await;
                    }
                    let payload = &body[start..=end];
                    if mode == Mode::Continuous {
                        for chunk in payload.chunks(4096) {
                            if socket.write_all(chunk).await.is_err() { return; }
                            tokio::time::sleep(Duration::from_millis(30)).await;
                        }
                        return;
                    }
                    if matches!(mode, Mode::InvalidFinalChunk | Mode::FullPendingEof) {
                        let _ = socket.write_all(format!("{:x}\r\n", payload.len()).as_bytes()).await;
                        let _ = socket.write_all(payload).await;
                        let _ = socket.write_all(b"\r\n").await;
                        if mode == Mode::FullPendingEof {
                            std::future::pending::<()>().await;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        let _ = socket.write_all(b"invalid\r\n").await;
                        return;
                    }
                    let should_interrupt = !sample && (attempt == 0 || mode == Mode::AlwaysInterrupted);
                    if should_interrupt {
                        let prefix = &payload[..8192.min(payload.len())];
                        if unknown {
                            let _ = socket.write_all(format!("{:x}\r\n", prefix.len()).as_bytes()).await;
                        }
                        let _ = socket.write_all(prefix).await;
                        if unknown { let _ = socket.write_all(b"\r\n").await; }
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        interrupted.notify_one();
                        return;
                    }
                    if sample && mode == Mode::ChangedSample {
                        let _ = socket.write_all(&vec![255; payload.len()]).await;
                    } else {
                        let _ = socket.write_all(payload).await;
                    }
                });
            }
        })
    };
    Fixture {
        url: format!("http://{address}/file"),
        body,
        requests,
        interrupted,
        sample_started,
        server,
    }
}

async fn task(root: &Path, fixture: &Fixture, mode: Mode) -> DownloadTask {
    let mut download = Download::new(fixture.url.clone(), root.join("output"), Uuid::nil());
    download.filename = "payload.bin".into();
    let unknown = matches!(
        mode,
        Mode::UnknownSize | Mode::UnknownLength | Mode::DiscoveredSizeChanged
    );
    download.size = (!unknown).then_some(fixture.body.len() as u64);
    download.etag = match mode {
        Mode::MissingEtag | Mode::ChangedSample | Mode::StalledSample => None,
        Mode::WeakEtag => Some("W/\"stable\"".into()),
        _ => Some("\"stable\"".into()),
    };
    download.supports_range = true;
    download.segments = vec![Segment::new(
        0,
        0,
        if unknown {
            u64::MAX
        } else {
            fixture.body.len() as u64 - 1
        },
    )];
    let db = DownloadDatabase::new(root.join("downloads.db"))
        .await
        .unwrap();
    db.upsert_download(&download).await.unwrap();
    let (events, _) = broadcast::channel(1024);
    DownloadTask::new(
        download,
        root.join("cache"),
        super::super::manager::http_client_builder(
            Duration::from_millis(150),
            Duration::from_millis(150),
        )
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap(),
        crate::engine::RateLimiter::unlimited(),
        db,
        events,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        1,
        2,
        0,
    )
}

#[tokio::test]
async fn interrupted_single_stream_resumes_exact_prefix_with_strong_weak_or_missing_etag() {
    for mode in [
        Mode::Interrupted,
        Mode::MissingEtag,
        Mode::WeakEtag,
        Mode::MissingResponseEtag,
        Mode::CleanTruncated,
        Mode::UnknownSize,
        Mode::UnknownLength,
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = serve(mode).await;
        let task = task(root.path(), &fixture, mode).await;
        let id = task.download.id;
        let db = task.db.clone();
        timeout(Duration::from_secs(5), task.run())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tokio::fs::read(root.path().join("output/payload.bin"))
                .await
                .unwrap(),
            *fixture.body
        );
        let saved = db.load_download(id).await.unwrap().unwrap();
        assert_eq!(saved.downloaded, fixture.body.len() as u64);
        assert_eq!(saved.size, Some(fixture.body.len() as u64));
        assert!(saved.segments[0].complete);
        let requests = fixture.requests.lock().await;
        assert!(requests
            .iter()
            .any(|request| request.to_ascii_lowercase().contains("range: bytes=8192-")));
        if matches!(
            mode,
            Mode::MissingEtag | Mode::WeakEtag | Mode::MissingResponseEtag
        ) {
            assert!(requests
                .iter()
                .any(|request| request.to_ascii_lowercase().contains("range: bytes=0-4095")));
        } else {
            assert!(requests.iter().any(|request| request
                .to_ascii_lowercase()
                .contains("if-range: \"stable\"")));
        }
    }
}

#[tokio::test]
async fn local_throttling_longer_than_the_idle_timeout_does_not_fail_the_transfer() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(Mode::Continuous).await;
    let mut task = task(root.path(), &fixture, Mode::Continuous).await;
    // A 4096-byte frame can fit in this bucket. The second acquisition waits
    // much longer than the client's 150ms read timeout, without pending a read.
    task.rate_limiter = crate::engine::RateLimiter::new(16384);
    timeout(Duration::from_secs(12), task.run())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        tokio::fs::read(root.path().join("output/payload.bin"))
            .await
            .unwrap(),
        *fixture.body
    );
}

#[tokio::test]
async fn resume_rejects_changed_identity_size_or_ignored_ranges_without_erasing_prefix() {
    for mode in [
        Mode::ChangedSample,
        Mode::ChangedEtag,
        Mode::ChangedTotal,
        Mode::IgnoredRange,
        Mode::DiscoveredSizeChanged,
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = serve(mode).await;
        let task = task(root.path(), &fixture, mode).await;
        let id = task.download.id;
        let db = task.db.clone();
        assert!(timeout(Duration::from_secs(5), task.run())
            .await
            .unwrap()
            .is_err());
        assert!(!root.path().join("output/payload.bin").exists());
        let saved = db.load_download(id).await.unwrap().unwrap();
        assert_eq!(saved.downloaded, 8192);
        assert_eq!(saved.size, Some(fixture.body.len() as u64));
        assert_eq!(saved.segments[0].downloaded, 8192);
        assert!(!saved.segments[0].complete);
        assert_eq!(
            tokio::fs::read(
                root.path()
                    .join("cache")
                    .join(format!("{id}_segment_0.part"))
            )
            .await
            .unwrap(),
            fixture.body[..8192]
        );
    }
}

#[tokio::test]
async fn continuously_reading_transfer_outlives_the_read_timeout() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(Mode::Continuous).await;
    let task = task(root.path(), &fixture, Mode::Continuous).await;
    let started = tokio::time::Instant::now();
    timeout(Duration::from_secs(5), task.run())
        .await
        .unwrap()
        .unwrap();
    assert!(started.elapsed() > Duration::from_millis(150));
    assert_eq!(
        tokio::fs::read(root.path().join("output/payload.bin"))
            .await
            .unwrap(),
        *fixture.body
    );
}

#[tokio::test]
async fn stalled_headers_and_body_exhaust_the_configured_retry_budget() {
    for mode in [Mode::StalledHeaders, Mode::StalledBody] {
        let root = tempfile::tempdir().unwrap();
        let fixture = serve(mode).await;
        let task = task(root.path(), &fixture, mode).await;
        assert!(timeout(Duration::from_secs(5), task.run())
            .await
            .unwrap()
            .is_err());
        assert_eq!(fixture.requests.lock().await.len(), 3);
        assert!(!root.path().join("output/payload.bin").exists());
    }
}

#[tokio::test]
async fn retry_exhaustion_preserves_progress_across_database_reopen() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(Mode::AlwaysInterrupted).await;
    let task = task(root.path(), &fixture, Mode::AlwaysInterrupted).await;
    let id = task.download.id;
    assert!(timeout(Duration::from_secs(5), task.run())
        .await
        .unwrap()
        .is_err());
    let db = DownloadDatabase::new(root.path().join("downloads.db"))
        .await
        .unwrap();
    let saved = db.load_download(id).await.unwrap().unwrap();
    assert_eq!(saved.downloaded, 3 * 8192);
    assert_eq!(saved.segments[0].downloaded, saved.downloaded);
    assert!(!saved.segments[0].complete);
    assert_eq!(fixture.requests.lock().await.len(), 3);
}

#[tokio::test]
async fn invalid_framing_after_full_payload_never_publishes_a_complete_file() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(Mode::InvalidFinalChunk).await;
    let mut task = task(root.path(), &fixture, Mode::InvalidFinalChunk).await;
    task.max_retries = 0;
    let id = task.download.id;
    let db = task.db.clone();
    assert!(timeout(Duration::from_secs(5), task.run())
        .await
        .unwrap()
        .is_err());
    let saved = db.load_download(id).await.unwrap().unwrap();
    assert_eq!(saved.segments[0].downloaded, 0);
    assert!(!saved.segments[0].complete);
    assert!(!root.path().join("output/payload.bin").exists());
}

#[tokio::test]
async fn pause_and_cancel_interrupt_single_stream_retry_waits_and_sample_verification() {
    for sampling in [false, true] {
        for pause in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mode = if sampling {
                Mode::StalledSample
            } else {
                Mode::Interrupted
            };
            let fixture = serve(mode).await;
            let mut task = task(root.path(), &fixture, mode).await;
            task.retry_delay_secs = if sampling { 0 } else { 30 };
            let paused = task.paused.clone();
            let cancelled = task.cancelled.clone();
            let id = task.download.id;
            let db = task.db.clone();
            let work = tokio::spawn(task.run());
            timeout(Duration::from_secs(3), fixture.interrupted.notified())
                .await
                .unwrap();
            if sampling {
                timeout(Duration::from_secs(3), fixture.sample_started.notified())
                    .await
                    .unwrap();
            }
            if pause {
                paused.store(true, Ordering::Release);
            } else {
                cancelled.store(true, Ordering::Release);
            }
            timeout(Duration::from_secs(2), work)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let saved = db.load_download(id).await.unwrap().unwrap();
            assert_eq!(
                saved.status,
                if pause {
                    DownloadStatus::Paused
                } else {
                    DownloadStatus::Cancelled
                }
            );
            assert!(saved.segments[0].downloaded > 0);
            assert!(!root.path().join("output/payload.bin").exists());
        }
    }
}

#[tokio::test]
async fn pause_or_cancel_after_full_payload_does_not_accept_unvalidated_eof() {
    for pause in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let fixture = serve(Mode::FullPendingEof).await;
        let task = task(root.path(), &fixture, Mode::FullPendingEof).await;
        let total = task.total_downloaded.clone();
        let paused = task.paused.clone();
        let cancelled = task.cancelled.clone();
        let id = task.download.id;
        let db = task.db.clone();
        let work = tokio::spawn(task.run());
        timeout(Duration::from_secs(3), async {
            while total.load(Ordering::Acquire) != fixture.body.len() as u64 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if pause {
            paused.store(true, Ordering::Release);
        } else {
            cancelled.store(true, Ordering::Release);
        }
        timeout(Duration::from_secs(2), work)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let saved = db.load_download(id).await.unwrap().unwrap();
        assert_eq!(saved.segments[0].downloaded, 0);
        assert!(!saved.segments[0].complete);
        assert_eq!(total.load(Ordering::Acquire), 0);
        assert!(!root.path().join("output/payload.bin").exists());
    }
}
