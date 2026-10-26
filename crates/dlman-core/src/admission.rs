//! A transfer owns a shared permit until its workers and post-processing finish.
//! Installation requires exclusive access, and never pauses somebody else's work.
use crate::DlmanError;
use std::sync::Arc;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

#[derive(Clone, Default)]
pub struct TransferAdmission(Arc<RwLock<()>>);

impl TransferAdmission {
    pub fn enter(&self) -> Result<OwnedRwLockReadGuard<()>, DlmanError> {
        self.0.clone().try_read_owned().map_err(|_| {
            DlmanError::InvalidOperation(
                "An application update is being installed. Try again shortly.".into(),
            )
        })
    }

    pub fn try_install(&self) -> Result<OwnedRwLockWriteGuard<()>, DlmanError> {
        self.0.clone().try_write_owned().map_err(|_| {
            DlmanError::InvalidOperation(
                "Wait for downloads and media processing to finish before installing the update."
                    .into(),
            )
        })
    }

    pub fn can_install(&self) -> bool {
        self.try_install().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workers_and_merging_prevent_installation_until_their_permit_drops() {
        let admission = TransferAdmission::default();
        let worker = admission.enter().unwrap();
        assert!(!admission.can_install());
        let merging = tokio::spawn(async move {
            let _worker = worker;
            tokio::task::yield_now().await;
        });
        merging.await.unwrap();
        assert!(admission.can_install());
    }

    #[test]
    fn installation_blocks_browser_queue_and_scheduler_admission_and_failure_releases_it() {
        let admission = TransferAdmission::default();
        let installation = admission.try_install().unwrap();
        for _ in 0..3 {
            assert!(admission.clone().enter().is_err());
        }
        // The installation guard is RAII: extraction, permission and spawn errors all release it.
        drop(installation);
        assert!(admission.enter().is_ok());
    }

    #[test]
    fn concurrent_install_and_new_worker_never_both_win() {
        for _ in 0..100 {
            let admission = TransferAdmission::default();
            let start = Arc::new(std::sync::Barrier::new(3));
            let attempted = Arc::new(std::sync::Barrier::new(3));
            std::thread::scope(|scope| {
                let install = scope.spawn(|| {
                    start.wait();
                    let permit = admission.try_install();
                    attempted.wait();
                    permit.is_ok()
                });
                let worker = scope.spawn(|| {
                    start.wait();
                    let permit = admission.enter();
                    attempted.wait();
                    permit.is_ok()
                });
                start.wait();
                attempted.wait();
                let installed = install.join().unwrap();
                let started = worker.join().unwrap();
                assert!(!installed || !started);
            });
        }
    }

    #[tokio::test]
    async fn removing_a_segment_abort_handle_does_not_release_live_remux_admission() {
        let admission = TransferAdmission::default();
        let transfer = admission.enter().unwrap();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        let remux = tokio::spawn(async move {
            let _transfer = transfer;
            let _ = finished.await;
        });
        // HLS removes its cancellable segment handle before remuxing, retaining its permit.
        let mut segments = std::collections::HashMap::from([(1, remux.abort_handle())]);
        segments.remove(&1);
        assert!(!admission.can_install());
        finish.send(()).unwrap();
        remux.await.unwrap();
        assert!(admission.can_install());
    }

    #[tokio::test]
    async fn segment_children_keep_admission_after_the_parent_releases_it() {
        let admission = TransferAdmission::default();
        let parent = Arc::new(admission.enter().unwrap());
        let child = parent.clone();
        let (finish, finished) = tokio::sync::oneshot::channel::<()>();
        let worker = tokio::spawn(async move {
            let _transfer = child;
            let _ = finished.await;
        });
        drop(parent);
        assert!(!admission.can_install());
        finish.send(()).unwrap();
        worker.await.unwrap();
        assert!(admission.can_install());
    }
}
