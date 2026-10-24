//! Continuously scheduled ranges for a single DownloadTask.

use super::*;
use crate::engine::segmentation::{largest_candidate, SplitCommand, WorkerSnapshot};
use std::collections::{HashMap, HashSet};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

struct ActiveWorker {
    commands: mpsc::Sender<SplitCommand>,
    snapshot: watch::Receiver<WorkerSnapshot>,
}

impl DownloadTask {
    pub(super) async fn publish_range_map(&self) -> Result<(), DlmanError> {
        let mut snapshot = self
            .db
            .load_download(self.download.id)
            .await?
            .ok_or(DlmanError::NotFound(self.download.id))?;
        snapshot.segments = self.download.segments.clone();
        snapshot.downloaded = self.total_downloaded.load(Ordering::Acquire);
        if self.paused.load(Ordering::Acquire) {
            snapshot.status = DownloadStatus::Paused;
        } else if self.download.status == DownloadStatus::Cancelled {
            snapshot.status = DownloadStatus::Cancelled;
        }
        let _ = self
            .event_tx
            .send(CoreEvent::DownloadUpdated { download: snapshot });
        Ok(())
    }

    pub(super) fn can_rebalance(&self) -> bool {
        self.segment_count > 1
            && self.download.supports_range
            && self.download.size.is_some_and(|size| size > 1024 * 1024)
            && url::Url::parse(&self.download.url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
    }

    fn refresh_worker_snapshots(&mut self, active: &HashMap<u32, ActiveWorker>) {
        for (index, worker) in active {
            if let Some(segment) = self
                .download
                .segments
                .iter_mut()
                .find(|s| s.index == *index)
            {
                *segment = worker.snapshot.borrow().segment.clone();
            }
        }
        self.download.downloaded = self.total_downloaded.load(Ordering::Acquire);
    }

    fn spawn_range(
        &self,
        segment: Segment,
        stop: &CancellationToken,
        active: &mut HashMap<u32, ActiveWorker>,
        tasks: &mut JoinSet<(u32, Result<crate::engine::SegmentResult, DlmanError>)>,
    ) {
        let index = segment.index;
        let (commands, receiver) = mpsc::channel(1);
        let (snapshot, updates) = watch::channel(WorkerSnapshot {
            segment: segment.clone(),
            ready: false,
        });
        let worker = SegmentWorker::new_with_credentials(
            self.download.id,
            segment,
            self.effective_url().to_string(),
            self.temp_dir.clone(),
            self.client.clone(),
            self.rate_limiter.clone(),
            self.db.clone(),
            self.event_tx.clone(),
            self.paused.clone(),
            self.cancelled.clone(),
            self.total_downloaded.clone(),
            self.credentials.clone(),
            self.download.cookies.clone(),
        )
        .with_http_context(
            self.download.size,
            self.download.etag.clone(),
            self.download.source_page_url.clone(),
            super::super::refresh::expects_html(&self.download.filename),
        )
        .with_control(receiver, snapshot, stop.clone());
        active.insert(
            index,
            ActiveWorker {
                commands,
                snapshot: updates,
            },
        );
        tasks.spawn(async move { (index, worker.run().await) });
    }

    pub(super) async fn download_multi_segment(&mut self) -> Result<(), DlmanError> {
        let progress = self.spawn_progress_reporter();
        let result = self.run_range_pool().await;
        progress.abort();
        let _ = progress.await;
        result
    }

    async fn run_range_pool(&mut self) -> Result<(), DlmanError> {
        let limit = self.segment_count.max(1) as usize;
        let rebalance = self.can_rebalance();
        let stop = CancellationToken::new();
        let mut tasks = JoinSet::new();
        let mut active = HashMap::<u32, ActiveWorker>::new();
        let mut retries = HashMap::<u32, u32>::new();
        let mut retry_at = HashMap::<u32, Instant>::new();
        let mut split_disabled = HashSet::<u32>::new();
        let mut next_index = self
            .download
            .segments
            .iter()
            .map(|s| s.index)
            .max()
            .and_then(|i| i.checked_add(1));
        let mut pending: Option<oneshot::Receiver<Result<Option<(Segment, Segment)>, DlmanError>>> =
            None;
        let mut pending_donor = None;
        let mut failure = None;
        let mut tick = tokio::time::interval(Duration::from_millis(100));

        loop {
            self.refresh_worker_snapshots(&active);
            if failure.is_none() {
                if self.paused.load(Ordering::Acquire) {
                    failure = Some(DlmanError::Paused);
                } else if self.cancelled.load(Ordering::Acquire) {
                    failure = Some(DlmanError::Cancelled);
                }
            }
            if failure.is_some() {
                self.cancelled.store(true, Ordering::Release);
                stop.cancel();
                if tasks.is_empty() {
                    break;
                }
            } else {
                // A split in flight reserves one slot until its acknowledgement.
                while active.len() + usize::from(pending.is_some()) < limit {
                    let next = self
                        .download
                        .segments
                        .iter()
                        .find(|segment| {
                            !segment.complete
                                && !active.contains_key(&segment.index)
                                && retry_at
                                    .get(&segment.index)
                                    .map_or(true, |time| *time <= Instant::now())
                        })
                        .cloned();
                    let Some(segment) = next else {
                        break;
                    };
                    retry_at.remove(&segment.index);
                    self.spawn_range(segment, &stop, &mut active, &mut tasks);
                }
                if rebalance && pending.is_none() && active.len() < limit {
                    let snapshots: Vec<_> = active
                        .iter()
                        .filter(|(index, _)| !split_disabled.contains(*index))
                        .map(|(_, worker)| worker.snapshot.borrow().clone())
                        .collect();
                    if let (Some(donor), Some(child_index)) =
                        (largest_candidate(snapshots.iter()), next_index)
                    {
                        let (reply, receiver) = oneshot::channel();
                        if active[&donor]
                            .commands
                            .try_send(SplitCommand { child_index, reply })
                            .is_ok()
                        {
                            pending = Some(receiver);
                            pending_donor = Some(donor);
                            next_index = child_index.checked_add(1);
                        }
                    }
                }
                if tasks.is_empty()
                    && pending.is_none()
                    && self
                        .download
                        .segments
                        .iter()
                        .all(|segment| segment.complete)
                {
                    break;
                }
            }

            tokio::select! {
                acknowledgement = async { pending.as_mut().expect("guarded split receiver").await }, if pending.is_some() => {
                    pending = None;
                    let donor_index = pending_donor.take();
                    match acknowledgement {
                        Ok(Ok(Some((donor, child)))) => {
                            if let Some(segment) = self.download.segments.iter_mut().find(|s| s.index == donor.index) {
                                if segment.end != donor.end || segment.downloaded < donor.downloaded {
                                    *segment = donor;
                                }
                            }
                            self.download.segments.push(child);
                            self.refresh_worker_snapshots(&active);
                            self.download.segments.sort_by_key(|segment| segment.start);
                            if let Err(error) = self.publish_range_map().await {
                                failure = Some(error);
                            }
                        }
                        Ok(Err(error)) => {
                            // A rejected transaction leaves ownership unchanged.
                            // Finish this donor without further split attempts.
                            warn!("Segment split rejected: {}", error);
                            if let Some(index) = donor_index { split_disabled.insert(index); }
                        }
                        Ok(Ok(None)) | Err(_) => {}
                    }
                }
                joined = tasks.join_next(), if !tasks.is_empty() => {
                    match joined {
                        Some(Ok((index, result))) => {
                            if let Some(worker) = active.remove(&index) {
                                if let Some(segment) = self.download.segments.iter_mut().find(|s| s.index == index) {
                                    *segment = worker.snapshot.borrow().segment.clone();
                                }
                            }
                            match result {
                                Ok(_) => {}
                                Err(error) if failure.is_some() => { tracing::debug!("Worker drained: {}", error); }
                                Err(DlmanError::Paused) => failure = Some(DlmanError::Paused),
                                Err(DlmanError::Cancelled) => failure = Some(DlmanError::Cancelled),
                                Err(error) => {
                                    let attempts = retries.entry(index).or_default();
                                    *attempts += 1;
                                    if error.is_retryable() && *attempts <= self.max_retries {
                                        retry_at.insert(index, Instant::now() + Duration::from_secs(self.retry_delay_secs as u64));
                                    } else {
                                        failure = Some(error);
                                    }
                                }
                            }
                        }
                        Some(Err(error)) => {
                            failure = Some(DlmanError::Unknown(format!("Segment task panicked: {}", error)));
                        }
                        None => {}
                    }
                }
                _ = tick.tick() => {}
            }
        }

        // Every worker is now stopped; loading only the range map avoids stale
        // metadata/status writes and recovers splits whose receiver disappeared.
        if let Some(saved) = self.db.load_download(self.download.id).await? {
            self.download.segments = saved.segments;
        }
        self.reconcile_segments().await?;
        if matches!(&failure, Some(DlmanError::Paused))
            && !self.paused.load(Ordering::Acquire)
            && self.cancelled.load(Ordering::Acquire)
        {
            failure = Some(DlmanError::Cancelled);
        }
        if matches!(&failure, Some(DlmanError::Paused)) {
            self.download.status = DownloadStatus::Paused;
        } else if matches!(&failure, Some(DlmanError::Cancelled)) {
            self.download.status = DownloadStatus::Cancelled;
        }
        self.publish_range_map().await?;
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
