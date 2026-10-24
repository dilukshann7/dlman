//! These HTTP fixtures are authored regression tests only. They must not be
//! started by source verification, normal startup, or a development server.

use super::*;
use crate::engine::segmentation::{split_tail, validate_map, MIN_SPLIT_SIZE};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Mutex, Notify};
use tokio::time::{timeout, Duration};
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    GatedDonor,
    RetryChild,
    WrongChildRange,
    ChangedChildEtag,
    TruncatedChild,
    NoRanges,
    UnknownSize,
    StalledHeaders,
    LateOverlong,
}

struct Fixture {
    url: String,
    body: Arc<Vec<u8>>,
    ranges: Arc<Mutex<Vec<(u64, u64)>>>,
    helper: Arc<Notify>,
    release: Arc<Notify>,
    max_active: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct RequestGuard(Arc<AtomicUsize>);
impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

async fn serve(size: usize, mode: Mode) -> Fixture {
    let body: Arc<Vec<u8>> = Arc::new((0..size).map(|i| (i % 251) as u8).collect());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let helper = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let active = Arc::new(AtomicUsize::new(0));
    let max_active = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = {
        let body = body.clone();
        let ranges = ranges.clone();
        let helper = helper.clone();
        let release = release.clone();
        let max_active = max_active.clone();
        let child_failed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            loop {
                let (mut socket, _) = tokio::select! {
                    accepted = listener.accept() => accepted.unwrap(),
                    _ = handlers.join_next(), if !handlers.is_empty() => continue,
                };
                let body = body.clone();
                let ranges = ranges.clone();
                let helper = helper.clone();
                let release = release.clone();
                let active = active.clone();
                let max_active = max_active.clone();
                let child_failed = child_failed.clone();
                handlers.spawn(async move {
                    let mut bytes = Vec::new();
                    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        let mut buffer = [0; 1024];
                        let length = socket.read(&mut buffer).await.unwrap();
                        if length == 0 { return; }
                        bytes.extend_from_slice(&buffer[..length]);
                        assert!(bytes.len() < 16384);
                    }
                    let request = String::from_utf8(bytes).unwrap();
                    if request.starts_with("HEAD ") {
                        let headers = if mode == Mode::UnknownSize {
                            String::new()
                        } else {
                            format!("Content-Length: {}\r\n{}", body.len(),
                                if mode == Mode::NoRanges { "" } else { "Accept-Ranges: bytes\r\n" })
                        };
                        let response = format!("HTTP/1.1 200 OK\r\n{headers}ETag: \"stable\"\r\nConnection: close\r\n\r\n");
                        let _ = socket.write_all(response.as_bytes()).await;
                        return;
                    }
                    let count = active.fetch_add(1, Ordering::AcqRel) + 1;
                    let _guard = RequestGuard(active);
                    max_active.fetch_max(count, Ordering::AcqRel);
                    let range = request.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("range").then_some(value.trim())
                    });
                    let (start, end) = range.map(|value| {
                        let (start, end) = value.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
                        (start.parse::<u64>().unwrap(), if end.is_empty() { body.len() as u64 - 1 } else { end.parse().unwrap() })
                    }).unwrap_or((0, body.len() as u64 - 1));
                    ranges.lock().await.push((start, end));
                    if mode == Mode::StalledHeaders {
                        std::future::pending::<()>().await;
                    }
                    let initial_end = body.len() as u64 / 2 - 1;
                    let child = start > 0 && start <= initial_end && end <= initial_end;
                    if child { helper.notify_one(); }
                    let entire = matches!(mode, Mode::NoRanges | Mode::UnknownSize);
                    let (start, end) = if entire { (0, body.len() as u64 - 1) } else { (start, end) };
                    let length = end - start + 1;
                    let status = if entire { "200 OK" } else { "206 Partial Content" };
                    let etag = if child && mode == Mode::ChangedChildEtag { "\"changed\"" } else { "\"stable\"" };
                    let range_header = if entire { String::new() } else {
                        let actual_start = start + u64::from(child && mode == Mode::WrongChildRange);
                        format!("Content-Range: bytes {actual_start}-{end}/{}\r\n", body.len())
                    };
                    let length_header = if mode == Mode::UnknownSize { String::new() } else {
                        format!("Content-Length: {}\r\n", length + u64::from(mode == Mode::LateOverlong))
                    };
                    let headers = format!("HTTP/1.1 {status}\r\n{range_header}{length_header}ETag: {etag}\r\nConnection: close\r\n\r\n");
                    if socket.write_all(headers.as_bytes()).await.is_err() { return; }
                    if child && mode == Mode::RetryChild && !child_failed.swap(true, Ordering::AcqRel) {
                        return;
                    }
                    let payload = &body[start as usize..=end as usize];
                    let gated = !matches!(mode, Mode::Normal | Mode::NoRanges | Mode::UnknownSize | Mode::StalledHeaders | Mode::LateOverlong);
                    if mode == Mode::LateOverlong {
                        if socket.write_all(&payload[..1]).await.is_err() { return; }
                        tokio::time::sleep(Duration::from_millis(2100)).await;
                        if socket.write_all(&payload[1..]).await.is_err() { return; }
                        // Ensure the complete advertised range can be checkpointed
                        // before invalid extra framing arrives in a later chunk.
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let _ = socket.write_all(&[255]).await;
                    } else if gated && start == 0 && end == initial_end {
                        let prefix = 65536.min(payload.len());
                        if socket.write_all(&payload[..prefix]).await.is_err() { return; }
                        release.notified().await;
                        let _ = socket.write_all(&payload[prefix..]).await;
                    } else if child && mode == Mode::TruncatedChild {
                        let _ = socket.write_all(&payload[..payload.len() - 1]).await;
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
        ranges,
        helper,
        release,
        max_active,
        server,
    }
}

async fn make_task(
    root: &Path,
    download: Download,
    connections: u32,
) -> (DownloadTask, broadcast::Receiver<CoreEvent>) {
    let db = DownloadDatabase::new(root.join("downloads.db"))
        .await
        .unwrap();
    db.upsert_download(&download).await.unwrap();
    let (events, receiver) = broadcast::channel(1024);
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    (
        DownloadTask::new(
            download,
            root.join("cache"),
            client,
            RateLimiter::unlimited(),
            db,
            events,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            connections,
            2,
            0,
        ),
        receiver,
    )
}

fn download(root: &Path, url: &str) -> Download {
    let mut download = Download::new(url.into(), root.join("output"), Uuid::nil());
    download.filename = "payload.bin".into();
    download
}

#[tokio::test]
async fn fast_worker_helps_a_live_donor_and_publishes_exact_bytes() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(12 * MIN_SPLIT_SIZE as usize + 37, Mode::GatedDonor).await;
    let download = download(root.path(), &fixture.url);
    let id = download.id;
    let output = download.destination.join(&download.filename);
    let (task, mut events) = make_task(root.path(), download, 2).await;
    let db = task.db.clone();
    let work = tokio::spawn(task.run());
    timeout(Duration::from_secs(10), fixture.helper.notified())
        .await
        .unwrap();
    assert!(
        !work.is_finished(),
        "donor must still be running when help starts"
    );
    let saved = db.load_download(id).await.unwrap().unwrap();
    assert!(saved.segments.len() > 2);
    validate_map(&saved.segments, saved.size, false).unwrap();
    let mut saw_split = false;
    while let Ok(event) = events.try_recv() {
        if let CoreEvent::DownloadUpdated { download } = event {
            saw_split |= download.segments.len() > 2;
        }
    }
    assert!(saw_split, "map must be published before the child starts");
    fixture.release.notify_one();
    timeout(Duration::from_secs(10), work)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(tokio::fs::read(output).await.unwrap(), *fixture.body);
    assert!(fixture.max_active.load(Ordering::Acquire) <= 2);
    let saved = db.load_download(id).await.unwrap().unwrap();
    assert_eq!(saved.downloaded, fixture.body.len() as u64);
    assert_eq!(saved.status, DownloadStatus::Completed);
    validate_map(&saved.segments, saved.size, true).unwrap();
}

#[tokio::test]
async fn split_transaction_collision_rolls_back_donor_and_keeps_other_rows() {
    let root = tempfile::tempdir().unwrap();
    let mut download = download(root.path(), "https://example.com/file");
    download.size = Some(16 * MIN_SPLIT_SIZE);
    download.segments = vec![
        Segment::new(0, 0, 8 * MIN_SPLIT_SIZE - 1),
        Segment::new(1, 8 * MIN_SPLIT_SIZE, 16 * MIN_SPLIT_SIZE - 1),
    ];
    let db = DownloadDatabase::new(root.path().join("downloads.db"))
        .await
        .unwrap();
    db.upsert_download(&download).await.unwrap();
    let (donor, child) = split_tail(&download.segments[0], 1).unwrap();
    assert!(db
        .split_segment(download.id, &download.segments[0], &donor, &child)
        .await
        .is_err());
    let saved = db.load_download(download.id).await.unwrap().unwrap();
    assert_eq!(saved.segments.len(), 2);
    assert_eq!(saved.segments[0].end, 8 * MIN_SPLIT_SIZE - 1);
    assert_eq!(saved.segments[1].start, 8 * MIN_SPLIT_SIZE);
}

#[tokio::test]
async fn committed_split_survives_reopen_before_child_file_creation() {
    let root = tempfile::tempdir().unwrap();
    let mut download = download(root.path(), "https://example.com/file");
    download.size = Some(8 * MIN_SPLIT_SIZE);
    download.supports_range = true;
    download.segments = vec![Segment::new(4, 0, 8 * MIN_SPLIT_SIZE - 1)];
    download.segments[0].downloaded = 65536;
    let (mut task, _) = make_task(root.path(), download.clone(), 2).await;
    tokio::fs::create_dir_all(&task.temp_dir).await.unwrap();
    let prefix = vec![42; 65536];
    tokio::fs::write(
        task.temp_dir
            .join(format!("{}_segment_4.part", download.id)),
        &prefix,
    )
    .await
    .unwrap();
    let (donor, child) = split_tail(&download.segments[0], 9).unwrap();
    task.db
        .split_segment(download.id, &download.segments[0], &donor, &child)
        .await
        .unwrap();
    let reopened = DownloadDatabase::new(root.path().join("downloads.db"))
        .await
        .unwrap();
    task.download = reopened.load_download(download.id).await.unwrap().unwrap();
    task.reconcile_segments().await.unwrap();
    assert_eq!(task.total_downloaded.load(Ordering::Acquire), 65536);
    assert_eq!(task.download.segments.len(), 2);
    assert_eq!(task.download.segments[1].index, 9);
    assert!(!task
        .temp_dir
        .join(format!("{}_segment_9.part", download.id))
        .exists());
    validate_map(&task.download.segments, task.download.size, false).unwrap();
}

#[tokio::test]
async fn merge_orders_by_offsets_and_preserves_parts_if_publication_fails() {
    let root = tempfile::tempdir().unwrap();
    let mut download = download(root.path(), "https://example.com/file");
    download.size = Some(6);
    download.segments = vec![
        Segment::new(0, 0, 1),
        Segment::new(1, 4, 5),
        Segment::new(9, 2, 3),
    ];
    for segment in &mut download.segments {
        segment.downloaded = 2;
        segment.complete = true;
    }
    let output = download.destination.join(&download.filename);
    let (mut task, _) = make_task(root.path(), download.clone(), 2).await;
    tokio::fs::create_dir_all(&task.temp_dir).await.unwrap();
    for (id, bytes) in [(0, b"ab"), (1, b"ef"), (9, b"cd")] {
        tokio::fs::write(
            task.temp_dir
                .join(format!("{}_segment_{id}.part", download.id)),
            bytes,
        )
        .await
        .unwrap();
    }
    tokio::fs::create_dir_all(&output).await.unwrap();
    assert!(task.merge_segments().await.is_err());
    assert!(!task
        .download
        .destination
        .join(format!(".{}.assembling", download.id))
        .exists());
    for id in [0, 1, 9] {
        assert!(task
            .temp_dir
            .join(format!("{}_segment_{id}.part", download.id))
            .exists());
    }
    tokio::fs::remove_dir(&output).await.unwrap();
    assert_eq!(task.merge_segments().await.unwrap(), vec![2, 2, 2]);
    assert_eq!(tokio::fs::read(output).await.unwrap(), b"abcdef");
}

#[tokio::test]
async fn resumed_range_count_does_not_increase_the_connection_limit() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(65536, Mode::Normal).await;
    let mut download = download(root.path(), &fixture.url);
    download.size = Some(65536);
    download.supports_range = true;
    download.segments = (0..8)
        .map(|index| Segment::new(index, index as u64 * 8192, (index as u64 + 1) * 8192 - 1))
        .collect();
    let output = download.destination.join(&download.filename);
    let (task, _) = make_task(root.path(), download, 2).await;
    timeout(Duration::from_secs(10), task.run())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fixture.ranges.lock().await.len(), 8);
    assert!(fixture.max_active.load(Ordering::Acquire) <= 2);
    assert_eq!(tokio::fs::read(output).await.unwrap(), *fixture.body);
}

#[tokio::test]
async fn single_stream_fallbacks_keep_exact_output() {
    for (mode, size, limit) in [
        (Mode::NoRanges, 3 * MIN_SPLIT_SIZE as usize, 4),
        (Mode::UnknownSize, 65536, 4),
        (Mode::Normal, 65536, 4),
        (Mode::Normal, 3 * MIN_SPLIT_SIZE as usize, 1),
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = serve(size, mode).await;
        let download = download(root.path(), &fixture.url);
        let output = download.destination.join(&download.filename);
        let id = download.id;
        let (task, _) = make_task(root.path(), download, limit).await;
        let db = task.db.clone();
        timeout(Duration::from_secs(10), task.run())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tokio::fs::read(output).await.unwrap(), *fixture.body);
        assert_eq!(
            db.load_download(id).await.unwrap().unwrap().segments.len(),
            1
        );
    }
}

#[tokio::test]
async fn pause_after_split_drains_workers_and_resumes_the_saved_map() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(12 * MIN_SPLIT_SIZE as usize + 37, Mode::GatedDonor).await;
    let download = download(root.path(), &fixture.url);
    let output = download.destination.join(&download.filename);
    let id = download.id;
    let (task, _) = make_task(root.path(), download, 2).await;
    let paused = task.paused();
    let db = task.db.clone();
    let work = tokio::spawn(task.run());
    timeout(Duration::from_secs(10), fixture.helper.notified())
        .await
        .unwrap();
    paused.store(true, Ordering::Release);
    timeout(Duration::from_secs(10), work)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!output.exists());
    let saved = db.load_download(id).await.unwrap().unwrap();
    let donor = saved
        .segments
        .iter()
        .find(|segment| segment.index == 0)
        .unwrap();
    assert!(donor.downloaded > 0);
    assert!(saved.segments.len() > 2);
    validate_map(&saved.segments, saved.size, false).unwrap();
    // The old fixture's handler may remain blocked after its client disconnects;
    // releasing it ensures it cannot affect the resumed concurrency observation.
    fixture.release.notify_one();
    let (task, _) = make_task(root.path(), saved, 1).await;
    timeout(Duration::from_secs(10), task.run())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tokio::fs::read(output).await.unwrap(), *fixture.body);
}

#[tokio::test]
async fn cancellation_interrupts_a_worker_waiting_for_response_headers() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(65536, Mode::StalledHeaders).await;
    let download = download(root.path(), &fixture.url);
    let id = download.id;
    let (task, _) = make_task(root.path(), download, 1).await;
    let cancelled = task.cancelled();
    let db = task.db.clone();
    let work = tokio::spawn(task.run());
    timeout(Duration::from_secs(10), async {
        while fixture.ranges.lock().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancelled.store(true, Ordering::Release);
    timeout(Duration::from_secs(10), work)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        db.load_download(id).await.unwrap().unwrap().status,
        DownloadStatus::Cancelled
    );
}

#[tokio::test]
async fn a_failed_child_retries_its_current_range_without_waiting_for_the_donor() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(12 * MIN_SPLIT_SIZE as usize + 37, Mode::RetryChild).await;
    let download = download(root.path(), &fixture.url);
    let output = download.destination.join(&download.filename);
    let (task, _) = make_task(root.path(), download, 2).await;
    let work = tokio::spawn(task.run());
    timeout(Duration::from_secs(10), async {
        loop {
            let ranges = fixture.ranges.lock().await.clone();
            if ranges.iter().any(|range| {
                range.0 > 0
                    && range.0 < fixture.body.len() as u64 / 2
                    && ranges.iter().filter(|other| *other == range).count() >= 2
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!work.is_finished());
    fixture.release.notify_one();
    timeout(Duration::from_secs(10), work)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(tokio::fs::read(output).await.unwrap(), *fixture.body);
}

#[tokio::test]
async fn invalid_child_responses_stop_and_preserve_the_committed_donor_prefix() {
    for mode in [
        Mode::WrongChildRange,
        Mode::ChangedChildEtag,
        Mode::TruncatedChild,
    ] {
        let root = tempfile::tempdir().unwrap();
        let fixture = serve(12 * MIN_SPLIT_SIZE as usize + 37, mode).await;
        let download = download(root.path(), &fixture.url);
        let id = download.id;
        let output = download.destination.join(&download.filename);
        let (mut task, _) = make_task(root.path(), download, 2).await;
        task.max_retries = 0;
        let db = task.db.clone();
        let work = tokio::spawn(task.run());
        timeout(Duration::from_secs(10), fixture.helper.notified())
            .await
            .unwrap();
        assert!(timeout(Duration::from_secs(10), work)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        assert!(!output.exists());
        let saved = db.load_download(id).await.unwrap().unwrap();
        validate_map(&saved.segments, saved.size, false).unwrap();
        let donor = saved
            .segments
            .iter()
            .find(|segment| segment.index == 0)
            .unwrap();
        let bytes = tokio::fs::read(
            root.path()
                .join("cache")
                .join(format!("{id}_segment_0.part")),
        )
        .await
        .unwrap();
        assert_eq!(bytes, fixture.body[..donor.downloaded as usize]);
        assert!(donor.downloaded > 0);
    }
}

#[tokio::test]
async fn ordinary_checkpoint_does_not_accept_a_later_rejected_response() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(65536, Mode::LateOverlong).await;
    let mut download = download(root.path(), &fixture.url);
    download.size = Some(65536);
    download.supports_range = true;
    download.segments = vec![Segment::new(0, 0, 65535)];
    let id = download.id;
    let output = download.destination.join(&download.filename);
    let (task, _) = make_task(root.path(), download, 1).await;
    let db = task.db.clone();
    assert!(timeout(Duration::from_secs(10), task.run())
        .await
        .unwrap()
        .is_err());
    let saved = db.load_download(id).await.unwrap().unwrap();
    assert!(!saved.segments[0].complete);
    assert_eq!(saved.segments[0].downloaded, 0);
    assert!(!output.exists());
}

#[tokio::test]
async fn concurrent_metadata_updates_preserve_splits_and_transfer_completion_preserves_metadata() {
    let root = tempfile::tempdir().unwrap();
    let mut download = download(root.path(), "https://example.com/file");
    download.size = Some(8 * MIN_SPLIT_SIZE);
    download.segments = vec![Segment::new(0, 0, 8 * MIN_SPLIT_SIZE - 1)];
    let db = DownloadDatabase::new(root.path().join("downloads.db"))
        .await
        .unwrap();
    db.upsert_download(&download).await.unwrap();
    let (donor, child) = split_tail(&download.segments[0], 1).unwrap();
    db.split_segment(download.id, &download.segments[0], &donor, &child)
        .await
        .unwrap();
    let queue = Uuid::new_v4();
    let category = Uuid::new_v4();
    db.update_download_speed_limit(download.id, Some(12345))
        .await
        .unwrap();
    db.update_download_queue(download.id, queue, false)
        .await
        .unwrap();
    let new_destination = root.path().join("moved");
    db.update_download_location_and_category(download.id, &new_destination, Some(category))
        .await
        .unwrap();
    let saved = db.load_download(download.id).await.unwrap().unwrap();
    assert_eq!(saved.segments.len(), 2);
    assert_eq!(saved.segments[0].end + 1, saved.segments[1].start);
    download.segments = saved.segments;
    db.save_transfer(&download).await.unwrap();
    let saved = db.load_download(download.id).await.unwrap().unwrap();
    assert_eq!(saved.queue_id, queue);
    assert_eq!(saved.category_id, Some(category));
    assert_eq!(saved.speed_limit, Some(12345));
    assert_eq!(saved.destination, new_destination);
    validate_map(&saved.segments, saved.size, false).unwrap();
}

#[tokio::test]
async fn paused_assembly_removes_staging_but_keeps_every_partial() {
    let root = tempfile::tempdir().unwrap();
    let mut download = download(root.path(), "https://example.com/file");
    download.size = Some(3);
    let mut segment = Segment::new(0, 0, 2);
    segment.downloaded = 3;
    segment.complete = true;
    download.segments = vec![segment];
    let id = download.id;
    let (mut task, _) = make_task(root.path(), download, 1).await;
    tokio::fs::create_dir_all(&task.temp_dir).await.unwrap();
    let partial = task.temp_dir.join(format!("{id}_segment_0.part"));
    tokio::fs::write(&partial, b"abc").await.unwrap();
    task.paused.store(true, Ordering::Release);
    assert!(matches!(
        task.merge_segments().await,
        Err(DlmanError::Paused)
    ));
    assert!(task.cancelled.load(Ordering::Acquire));
    assert_eq!(tokio::fs::read(partial).await.unwrap(), b"abc");
    assert!(!task
        .download
        .destination
        .join(format!(".{id}.assembling"))
        .exists());
}

#[tokio::test]
async fn deletion_waits_for_split_workers_and_removes_all_parts_and_staging() {
    let root = tempfile::tempdir().unwrap();
    let fixture = serve(12 * MIN_SPLIT_SIZE as usize + 37, Mode::GatedDonor).await;
    let mut download = download(root.path(), &fixture.url);
    let id = download.id;
    let cache = root.path().join("cache");
    download.segment_cache_dir = Some(cache.clone());
    let destination = download.destination.clone();
    let (events, _) = broadcast::channel(1024);
    let manager = crate::engine::DownloadManager::new(root.path().into(), events)
        .await
        .unwrap();
    manager.db().upsert_download(&download).await.unwrap();
    manager.start(download, None, 2, 0, 0, None).await.unwrap();
    timeout(Duration::from_secs(10), fixture.helper.notified())
        .await
        .unwrap();
    tokio::fs::create_dir_all(&destination).await.unwrap();
    let staging = destination.join(format!(".{id}.assembling"));
    tokio::fs::write(&staging, b"interrupted assembly")
        .await
        .unwrap();
    timeout(Duration::from_secs(10), manager.delete(id, false))
        .await
        .unwrap()
        .unwrap();
    assert!(manager.db().load_download(id).await.unwrap().is_none());
    assert!(!manager.is_active(id).await);
    assert!(!staging.exists());
    if cache.exists() {
        let mut files = tokio::fs::read_dir(cache).await.unwrap();
        while let Some(file) = files.next_entry().await.unwrap() {
            assert!(!file
                .file_name()
                .to_string_lossy()
                .starts_with(&id.to_string()));
        }
    }
}
