use super::*;
use crate::engine::segmentation::MIN_SPLIT_SIZE;
use dlman_types::Download;
use tokio::sync::oneshot;

async fn worker(root: &std::path::Path) -> (SegmentWorker, SegmentFile) {
    let mut download = Download::new("https://example.com/file".into(), root.into(), Uuid::nil());
    download.size = Some(8 * MIN_SPLIT_SIZE);
    let mut segment = Segment::new(0, 0, 8 * MIN_SPLIT_SIZE - 1);
    segment.downloaded = 65536;
    download.segments = vec![segment.clone()];
    let database_path = root.join("downloads.db");
    let migrated = DownloadDatabase::new(&database_path).await.unwrap();
    drop(migrated);
    // SQLx 0.7 can retain pre-migration SELECT metadata on another pooled
    // connection. Reopen the completed schema to isolate these file-I/O tests.
    let db = DownloadDatabase::new(database_path).await.unwrap();
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
    file.flush().await.unwrap();
    (worker, buffered_segment_file(file))
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
        .await
        .unwrap();
    let (donor, child) = receiver.await.unwrap().unwrap().unwrap();
    assert_eq!(rollback, 65536);
    assert_eq!(donor.end + 1, child.start);
    file.write_all(&[7; 1024]).await.unwrap();
    worker.segment.downloaded += 1024;
    worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
    worker.rollback_attempt(&mut file, rollback).await.unwrap();
    assert_eq!(file.get_ref().metadata().await.unwrap().len(), 65536);
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
    file.get_mut().set_len(worker.segment.size()).await.unwrap();
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
        .await
        .unwrap();
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

#[tokio::test]
async fn interruption_checkpoints_new_bytes_without_rolling_back_the_prefix() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    file.write_all(&[7; 1024]).await.unwrap();
    worker.segment.downloaded += 1024;
    worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
    worker
        .checkpoint_interrupted(&mut file, 65536)
        .await
        .unwrap();
    assert_eq!(file.get_ref().metadata().await.unwrap().len(), 66560);
    assert_eq!(worker.downloaded_bytes.load(Ordering::Acquire), 66560);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments[0].downloaded, 66560);
    assert!(!saved.segments[0].complete);
}

#[tokio::test]
async fn interrupted_full_range_cannot_become_complete_on_reconciliation() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    let size = worker.segment.size();
    file.get_mut().set_len(size).await.unwrap();
    worker.downloaded_bytes.store(size, Ordering::Release);
    worker.segment.downloaded = size;
    worker
        .checkpoint_interrupted(&mut file, 65536)
        .await
        .unwrap();
    assert_eq!(file.get_ref().metadata().await.unwrap().len(), 65536);
    assert_eq!(worker.downloaded_bytes.load(Ordering::Acquire), 65536);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments[0].downloaded, 65536);
    assert!(!saved.segments[0].complete);
}

#[test]
fn premature_eof_is_retryable_but_invalid_content_is_not() {
    assert!(DlmanError::IncompleteTransfer.is_retryable());
    assert!(!invalid("Changed content").is_retryable());
}

#[tokio::test]
async fn rollback_restores_cursor_before_any_later_write() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    file.write_all(&[7; 1024]).await.unwrap();
    worker.segment.downloaded += 1024;
    worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
    worker.rollback_attempt(&mut file, 65536).await.unwrap();
    file.write_all(&[9; 16]).await.unwrap();
    file.flush().await.unwrap();
    let bytes = tokio::fs::read(&worker.temp_file_path).await.unwrap();
    assert_eq!(bytes.len(), 65552);
    assert_eq!(&bytes[..65536], vec![42; 65536]);
    assert_eq!(&bytes[65536..], &[9; 16]);
}

#[tokio::test]
async fn small_chunks_remain_batched_until_a_checkpoint() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    for _ in 0..4 {
        file.write_all(&[7; 16384]).await.unwrap();
        worker.segment.downloaded += 16384;
        worker.downloaded_bytes.fetch_add(16384, Ordering::AcqRel);
    }
    assert_eq!(file.buffer().len(), 65536);
    assert_eq!(file.get_ref().metadata().await.unwrap().len(), 65536);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments[0].downloaded, 65536);

    worker.checkpoint(&mut file).await.unwrap();
    assert!(file.buffer().is_empty());
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    let bytes = tokio::fs::read(&worker.temp_file_path).await.unwrap();
    assert_eq!(saved.segments[0].downloaded, 131072);
    assert_eq!(&bytes[..65536], vec![42; 65536]);
    assert_eq!(&bytes[65536..], vec![7; 65536]);
}

#[tokio::test]
async fn completion_flushes_empty_exact_oversized_and_partial_buffers() {
    for length in [
        0,
        17,
        SEGMENT_WRITE_BUFFER_SIZE,
        SEGMENT_WRITE_BUFFER_SIZE + 19,
    ] {
        let root = tempfile::tempdir().unwrap();
        let (mut worker, mut file) = worker(root.path()).await;
        let payload: Vec<u8> = (0..length).map(|i| (i % 251) as u8).collect();
        file.write_all(&payload).await.unwrap();
        worker.segment.downloaded += length as u64;
        worker
            .downloaded_bytes
            .fetch_add(length as u64, Ordering::AcqRel);
        worker.segment.end = worker.segment.downloaded - 1;
        worker.complete_segment(&mut file).await.unwrap();
        let saved = worker
            .db
            .load_download(worker.download_id)
            .await
            .unwrap()
            .unwrap();
        assert!(saved.segments[0].complete);
        assert_eq!(saved.segments[0].downloaded, 65536 + length as u64);
        let bytes = tokio::fs::read(&worker.temp_file_path).await.unwrap();
        assert_eq!(&bytes[..65536], vec![42; 65536]);
        assert_eq!(&bytes[65536..], payload);
        assert!(file.buffer().is_empty());
    }
}

#[tokio::test]
async fn split_acknowledges_only_after_pending_bytes_are_durable() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    file.write_all(&[7; 1024]).await.unwrap();
    worker.segment.downloaded += 1024;
    worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
    assert_eq!(file.buffer().len(), 1024);
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
        .await
        .unwrap();
    let (donor, child) = receiver.await.unwrap().unwrap().unwrap();
    assert_eq!(rollback, 66560);
    assert_eq!(donor.downloaded, 66560);
    assert_eq!(donor.end + 1, child.start);
    assert!(file.buffer().is_empty());
    let bytes = tokio::fs::read(&worker.temp_file_path).await.unwrap();
    assert_eq!(&bytes[..65536], vec![42; 65536]);
    assert_eq!(&bytes[65536..], &[7; 1024]);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments[0].downloaded, 66560);
    assert_eq!(saved.segments[1].downloaded, 0);
}

#[tokio::test]
async fn rollback_removes_pending_and_previously_checkpointed_rejected_bytes() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, mut file) = worker(root.path()).await;
    for value in [7, 8] {
        file.write_all(&[value; 1024]).await.unwrap();
        worker.segment.downloaded += 1024;
        worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
        if value == 7 {
            worker.checkpoint(&mut file).await.unwrap();
        }
    }
    assert_eq!(file.buffer().len(), 1024);
    worker.rollback_attempt(&mut file, 65536).await.unwrap();
    file.flush().await.unwrap();
    assert!(file.buffer().is_empty());
    assert_eq!(
        tokio::fs::read(&worker.temp_file_path).await.unwrap(),
        vec![42; 65536]
    );
    assert_eq!(worker.downloaded_bytes.load(Ordering::Acquire), 65536);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments[0].downloaded, 65536);
    assert!(!saved.segments[0].complete);
}

#[tokio::test]
async fn pause_and_cancel_flush_pending_prefix_before_returning() {
    for pause in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut worker, mut file) = worker(root.path()).await;
        file.write_all(&[7; 1024]).await.unwrap();
        worker.segment.downloaded += 1024;
        worker.downloaded_bytes.fetch_add(1024, Ordering::AcqRel);
        if pause {
            worker.paused.store(true, Ordering::Release);
        } else {
            worker.cancelled.store(true, Ordering::Release);
        }
        assert!(matches!(
            (pause, worker.stop_error()),
            (true, Some(DlmanError::Paused)) | (false, Some(DlmanError::Cancelled))
        ));
        worker
            .checkpoint_interrupted(&mut file, 65536)
            .await
            .unwrap();
        drop(file);
        let saved = worker
            .db
            .load_download(worker.download_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.segments[0].downloaded, 66560);
        assert!(!saved.segments[0].complete);
        let bytes = tokio::fs::read(&worker.temp_file_path).await.unwrap();
        assert_eq!(&bytes[..65536], vec![42; 65536]);
        assert_eq!(&bytes[65536..], &[7; 1024]);
    }
}

#[tokio::test]
async fn flush_failure_cannot_persist_progress_or_publish_completion() {
    for complete in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut worker, file) = worker(root.path()).await;
        drop(file);
        let readonly = tokio::fs::File::open(&worker.temp_file_path).await.unwrap();
        let mut file = buffered_segment_file(readonly);
        file.write_all(&[7; 1024]).await.unwrap();
        worker.segment.downloaded += 1024;
        let result = if complete {
            worker.complete_segment(&mut file).await
        } else {
            worker.checkpoint(&mut file).await
        };
        assert!(matches!(result, Err(DlmanError::Io(_))));
        assert!(!worker.segment.complete);
        let saved = worker
            .db
            .load_download(worker.download_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.segments[0].downloaded, 65536);
        assert!(!saved.segments[0].complete);
        assert_eq!(
            tokio::fs::read(&worker.temp_file_path).await.unwrap(),
            vec![42; 65536]
        );
    }
}

#[tokio::test]
async fn failed_split_flush_stops_the_worker_without_acknowledging_ownership() {
    let root = tempfile::tempdir().unwrap();
    let (mut worker, file) = worker(root.path()).await;
    drop(file);
    let readonly = tokio::fs::File::open(&worker.temp_file_path).await.unwrap();
    let mut file = buffered_segment_file(readonly);
    file.write_all(&[7; 1024]).await.unwrap();
    worker.segment.downloaded += 1024;
    let original_end = worker.segment.end;
    let (reply, receiver) = oneshot::channel();
    let mut rollback = 65536;
    let result = worker
        .handle_split(
            &mut file,
            SplitCommand {
                child_index: 4,
                reply,
            },
            &mut rollback,
        )
        .await;
    assert!(matches!(result, Err(DlmanError::Io(_))));
    assert!(receiver.await.is_err());
    assert_eq!(worker.segment.end, original_end);
    assert_eq!(rollback, 65536);
    let saved = worker
        .db
        .load_download(worker.download_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.segments.len(), 1);
    assert_eq!(saved.segments[0].downloaded, 65536);
    assert!(!saved.segments[0].complete);
}

#[tokio::test]
async fn live_pause_and_cancel_flush_buffered_bytes_and_resume_exactly() {
    use tokio::io::AsyncReadExt;
    use tokio::time::{timeout, Duration};

    for pause in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut worker, file) = worker(root.path()).await;
        drop(file);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        worker.url = format!("http://{}/file", listener.local_addr().unwrap());
        worker.client = Client::builder().no_proxy().build().unwrap();
        worker.etag = Some("\"stable\"".into());
        let size = worker.segment.size() as usize;
        let mut body = vec![42; size];
        body[65536..].fill(7);
        let body = Arc::new(body);
        let server_body = body.clone();
        let server = tokio::spawn(async move {
            for attempt in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let mut buffer = [0; 1024];
                    let count = socket.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8(request).unwrap();
                let start = if attempt == 0 { 65536 } else { 73728 };
                assert!(request.contains(&format!("bytes={start}-{}", size - 1)));
                let headers = format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{}/{size}\r\nContent-Length: {}\r\nETag: \"stable\"\r\nConnection: close\r\n\r\n", size - 1, size - start);
                socket.write_all(headers.as_bytes()).await.unwrap();
                if attempt == 0 {
                    socket
                        .write_all(&server_body[start..start + 8192])
                        .await
                        .unwrap();
                    let mut byte = [0];
                    // Wait for the paused/cancelled response to close.
                    let _ = socket.read(&mut byte).await;
                } else {
                    socket.write_all(&server_body[start..]).await.unwrap();
                }
            }
        });
        let paused = worker.paused.clone();
        let cancelled = worker.cancelled.clone();
        let path = worker.temp_file_path.clone();
        let db = worker.db.clone();
        let id = worker.download_id;
        let url = worker.url.clone();
        let (commands, receiver) = mpsc::channel(1);
        let (snapshot, mut snapshots) = watch::channel(WorkerSnapshot {
            segment: worker.segment.clone(),
            ready: false,
        });
        let work = tokio::spawn(
            worker
                .with_control(receiver, snapshot, CancellationToken::new())
                .run(),
        );
        drop(
            timeout(
                Duration::from_secs(3),
                snapshots.wait_for(|state| state.segment.downloaded == 73728),
            )
            .await
            .unwrap()
            .unwrap(),
        );
        assert_eq!(tokio::fs::read(&path).await.unwrap(), vec![42; 65536]);
        if pause {
            paused.store(true, Ordering::Release);
        } else {
            cancelled.store(true, Ordering::Release);
        }
        let result = timeout(Duration::from_secs(3), work)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            (pause, result),
            (true, Err(DlmanError::Paused)) | (false, Err(DlmanError::Cancelled))
        ));
        drop(commands);
        let saved = db.load_download(id).await.unwrap().unwrap();
        assert_eq!(saved.segments[0].downloaded, 73728);
        assert!(!saved.segments[0].complete);
        assert_eq!(tokio::fs::read(&path).await.unwrap(), body[..73728]);

        let (events, _) = broadcast::channel(100);
        let resumed = SegmentWorker::new(
            id,
            saved.segments[0].clone(),
            url,
            root.path().into(),
            Client::builder().no_proxy().build().unwrap(),
            RateLimiter::unlimited(),
            db.clone(),
            events,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(73728)),
        )
        .with_http_context(Some(size as u64), Some("\"stable\"".into()), None, false);
        let result = timeout(Duration::from_secs(5), resumed.run())
            .await
            .unwrap()
            .unwrap();
        assert!(result.segment.complete);
        assert_eq!(tokio::fs::read(path).await.unwrap(), *body);
        timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn empty_unknown_size_response_completes_without_pending_bytes() {
    use tokio::io::AsyncReadExt;
    use tokio::time::{timeout, Duration};

    let root = tempfile::tempdir().unwrap();
    let (mut worker, file) = worker(root.path()).await;
    file.get_ref().set_len(0).await.unwrap();
    drop(file);
    worker.segment = Segment::new(0, 0, u64::MAX);
    worker.downloaded_bytes.store(0, Ordering::Release);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    worker.url = format!("http://{}/file", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let mut buffer = [0; 1024];
            let count = socket.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            request.extend_from_slice(&buffer[..count]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
    });
    let result = timeout(Duration::from_secs(3), worker.run())
        .await
        .unwrap()
        .unwrap();
    assert!(result.segment.complete);
    assert_eq!(result.segment.downloaded, 0);
    assert!(tokio::fs::read(result.path).await.unwrap().is_empty());
    server.await.unwrap();
}

#[cfg(windows)]
#[tokio::test]
async fn sync_failure_cannot_acknowledge_a_split_or_publish_completion() {
    for complete in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let (mut worker, file) = worker(root.path()).await;
        drop(file);
        // FlushFileBuffers requires a write-capable Windows handle. With no
        // pending bytes this reaches sync_all, rather than failing in flush.
        let readonly = tokio::fs::File::open(&worker.temp_file_path).await.unwrap();
        let mut file = buffered_segment_file(readonly);
        let result = if complete {
            worker.complete_segment(&mut file).await
        } else {
            let (reply, receiver) = oneshot::channel();
            let mut rollback = 65536;
            let result = worker
                .handle_split(
                    &mut file,
                    SplitCommand {
                        child_index: 4,
                        reply,
                    },
                    &mut rollback,
                )
                .await;
            assert!(receiver.await.is_err());
            result
        };
        assert!(matches!(result, Err(DlmanError::Io(_))));
        assert!(!worker.segment.complete);
        let saved = worker
            .db
            .load_download(worker.download_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.segments.len(), 1);
        assert_eq!(saved.segments[0].downloaded, 65536);
        assert!(!saved.segments[0].complete);
    }
}
