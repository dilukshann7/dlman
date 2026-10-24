use super::*;
use crate::engine::segmentation::MIN_SPLIT_SIZE;
use dlman_types::Download;
use tokio::sync::oneshot;

async fn worker(root: &std::path::Path) -> (SegmentWorker, tokio::fs::File) {
    let mut download = Download::new("https://example.com/file".into(), root.into(), Uuid::nil());
    download.size = Some(8 * MIN_SPLIT_SIZE);
    let mut segment = Segment::new(0, 0, 8 * MIN_SPLIT_SIZE - 1);
    segment.downloaded = 65536;
    download.segments = vec![segment.clone()];
    let db = DownloadDatabase::new(root.join("downloads.db"))
        .await
        .unwrap();
    db.upsert_download(&download).await.unwrap();
    let (events, _) = broadcast::channel(100);
    let mut worker = SegmentWorker::new(
        download.id,
        segment,
        download.url,
        root.into(),
        Client::new(),
        RateLimiter::unlimited(),
        db,
        events,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicU64::new(65536)),
    );
    worker.ready = true;
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .read(true)
        .open(&worker.temp_file_path)
        .await
        .unwrap();
    file.write_all(&vec![42; 65536]).await.unwrap();
    (worker, file)
}

#[tokio::test]
async fn split_advances_rollback_boundary_and_preserves_its_durable_prefix() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    let (reply, receiver) = oneshot::channel();
    let mut rollback = 0;
    worker
        .handle_split(
            &mut file,
            SplitCommand {
                child_index: 4,
                reply,
            },
            &mut rollback,
        )
        .await;
    let (donor, child) = receiver.await.unwrap().unwrap().unwrap();
    assert_eq!(rollback, 65536);
    assert_eq!(donor.end + 1, child.start);
    file.write_all(&[7; 1024]).await.unwrap();
    worker.segment.downloaded += 1024;
    worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
    worker.rollback_attempt(&mut file, rollback).await.unwrap();
    assert_eq!(file.metadata().await.unwrap().len(), 65536);
    assert_eq!(worker.downloaded_bytes.load(Ordering::Acquire), 65536);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments[0].downloaded, 65536);
    assert_eq!(saved.segments[1].index, 4);
    assert_eq!(saved.segments[1].downloaded, 0);
}

#[tokio::test]
async fn completion_racing_a_split_does_not_create_a_child() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    file.set_len(worker.segment.size()).await.unwrap();
    worker.segment.downloaded = worker.segment.size();
    let original_end = worker.segment.end;
    let (reply, receiver) = oneshot::channel();
    let mut rollback = 65536;
    worker
        .handle_split(
            &mut file,
            SplitCommand {
                child_index: 4,
                reply,
            },
            &mut rollback,
        )
        .await;
    assert!(receiver.await.unwrap().unwrap().is_none());
    assert_eq!(worker.segment.end, original_end);
    assert_eq!(
        worker
            .db
            .load_download(worker.download_id)
            .await
            .unwrap()
            .unwrap()
            .segments
            .len(),
        1
    );
}

#[tokio::test]
async fn observing_pause_marks_the_shared_manager_handle_as_stopping() {
    let root = tempfile::tempdir().unwrap();
    let (worker, _) = worker(root.path()).await;
    worker.paused.store(true, Ordering::Release);
    assert!(matches!(worker.stop_error(), Some(DlmanError::Paused)));
    assert!(worker.cancelled.load(Ordering::Acquire));
}
