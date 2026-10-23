//! Refresh lifecycle and atomic coordination with ordinary transfer controls.

use super::{refresh::*, DownloadManager};
use crate::error::DlmanError;
use dlman_types::{CoreEvent, Download};
use serde::Serialize;
use tokio::sync::MutexGuard;
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct ReleasedRefreshCapture {
    pub url: String,
    pub referrer: Option<String>,
    pub filename: Option<String>,
    pub cookies: Option<String>,
}

impl DownloadManager {
    /// Serialize mutations with refresh reservation and commit. Do not call a
    /// second manager mutation while retaining this guard.
    pub async fn edit_guard(&self, id: Uuid) -> Result<MutexGuard<'_, ()>, DlmanError> {
        let guard = self.operation.lock().await;
        let mut refresh = self.refresh.lock().await;
        if refresh.as_ref().is_some_and(|s| !s.live()) {
            *refresh = None;
        }
        if refresh.as_ref().is_some_and(|s| s.info.download_id == id) {
            return Err(invalid(
                "This download is waiting for a replacement address. Cancel refresh first.",
            ));
        }
        Ok(guard)
    }

    pub async fn begin_refresh(&self, id: Uuid) -> Result<RefreshSessionInfo, DlmanError> {
        let _operation = self.operation.lock().await;
        let mut refresh = self.refresh.lock().await;
        if refresh.as_ref().is_some_and(|s| !s.live()) {
            *refresh = None;
        }
        if refresh.is_some() {
            return Err(invalid("Another address refresh is already open."));
        }
        if self.active_tasks.read().await.contains_key(&id) {
            return Err(invalid(
                "Download workers are still stopping. Try refresh again shortly.",
            ));
        }
        let download = self
            .db
            .load_download(id)
            .await?
            .ok_or(DlmanError::NotFound(id))?;
        if !eligible(&download) {
            return Err(invalid(
                "Only paused or failed ordinary HTTP(S) downloads can refresh their address.",
            ));
        }
        let session = RefreshSession::new(id);
        let info = session.info.clone();
        *refresh = Some(session);
        let state = self.refresh.clone();
        let session_id = info.session_id;
        tokio::spawn(async move {
            tokio::time::sleep(SESSION_TTL).await;
            let mut state = state.lock().await;
            if state
                .as_ref()
                .is_some_and(|s| s.info.session_id == session_id && !s.live())
            {
                *state = None;
            }
        });
        Ok(info)
    }

    pub async fn cancel_refresh(&self, session_id: Uuid) {
        let _operation = self.operation.lock().await;
        let mut state = self.refresh.lock().await;
        if state
            .as_ref()
            .is_some_and(|s| s.info.session_id == session_id)
        {
            *state = None;
        }
    }

    pub async fn capture_refresh_candidate(
        &self,
        url: String,
        referrer: Option<String>,
        filename: Option<String>,
        cookies: Option<String>,
    ) -> Option<RefreshSessionInfo> {
        if !ordinary_url(&url) {
            return None;
        }
        let mut state = self.refresh.lock().await;
        if state.as_ref().is_some_and(|s| !s.live()) {
            *state = None;
        }
        let session = state.as_mut().filter(|s| s.can_capture())?;
        let candidate = RefreshCandidate {
            info: RefreshCandidateInfo {
                candidate_id: Uuid::new_v4(),
                display_url: display_url(&url),
                filename,
            },
            url,
            cookies,
            source_page_url: referrer.filter(|value| http_url(value).is_ok()),
        };
        session.info.candidate = Some(candidate.info.clone());
        session.candidate = Some(candidate);
        Some(session.info.clone())
    }

    pub async fn release_refresh_candidate(
        &self,
        session_id: Uuid,
        candidate_id: Uuid,
    ) -> Result<ReleasedRefreshCapture, DlmanError> {
        let mut state = self.refresh.lock().await;
        let session = state
            .as_mut()
            .filter(|s| s.live() && s.info.session_id == session_id && !s.verifying)
            .ok_or_else(|| invalid("Address refresh expired or is busy."))?;
        if !session
            .candidate
            .as_ref()
            .is_some_and(|c| c.info.candidate_id == candidate_id)
        {
            return Err(invalid("This browser candidate is no longer available."));
        }
        let candidate = session
            .candidate
            .take()
            .ok_or_else(|| invalid("Browser candidate is no longer available."))?;
        session.info.candidate = None;
        Ok(ReleasedRefreshCapture {
            url: candidate.url,
            referrer: candidate.source_page_url,
            filename: candidate.info.filename,
            cookies: candidate.cookies,
        })
    }

    /// Verify outside the mutation lock; cancellation/deletion can invalidate
    /// the session. Recheck it under the lock immediately before the DB commit.
    pub async fn refresh_address(
        &self,
        session_id: Uuid,
        url: Option<String>,
        candidate_id: Option<Uuid>,
    ) -> Result<Download, DlmanError> {
        let (download, candidate) = {
            let _operation = self.operation.lock().await;
            let mut state = self.refresh.lock().await;
            let session = state
                .as_mut()
                .filter(|s| s.live() && s.info.session_id == session_id && !s.verifying)
                .ok_or_else(|| {
                    invalid("Address refresh expired, was cancelled, or is already verifying.")
                })?;
            let download = self
                .db
                .load_download(session.info.download_id)
                .await?
                .ok_or(DlmanError::NotFound(session.info.download_id))?;
            if !eligible(&download) || self.active_tasks.read().await.contains_key(&download.id) {
                return Err(invalid(
                    "Download is no longer available for address refresh.",
                ));
            }
            let candidate = match (url, candidate_id) {
                (Some(url), None) => {
                    let parsed = http_url(url.trim())?;
                    let same_origin =
                        http_url(&download.url).is_ok_and(|old| old.origin() == parsed.origin());
                    RefreshCandidate {
                        info: RefreshCandidateInfo {
                            candidate_id: Uuid::new_v4(),
                            display_url: display_url(parsed.as_str()),
                            filename: None,
                        },
                        url: parsed.to_string(),
                        cookies: if same_origin {
                            download.cookies.clone()
                        } else {
                            None
                        },
                        source_page_url: download.source_page_url.clone(),
                    }
                }
                (None, Some(id)) => session
                    .candidate
                    .as_ref()
                    .filter(|c| c.info.candidate_id == id)
                    .cloned()
                    .ok_or_else(|| invalid("This browser candidate is no longer available."))?,
                _ => {
                    return Err(invalid(
                        "Provide either a replacement URL or a browser candidate.",
                    ))
                }
            };
            session.verifying = true;
            (download, candidate)
        };
        let result: Result<Download, DlmanError> = async {
            let cache_dir = self.resolve_temp_dir(&download).await;
            let credentials = self
                .db
                .find_credentials_for_url(&candidate.url)
                .await?
                .into_iter()
                .find(|credential| credential.enabled)
                .map(|credential| (credential.username, credential.password));
            let updated = tokio::time::timeout(
                std::time::Duration::from_secs(90),
                verify_candidate(
                    &self.scoped_client,
                    download,
                    candidate,
                    &cache_dir,
                    credentials.as_ref(),
                ),
            )
            .await
            .map_err(|_| {
                invalid("Replacement verification timed out. Partial data was preserved.")
            })??;
            let _operation = self.operation.lock().await;
            let mut state = self.refresh.lock().await;
            if !state
                .as_ref()
                .is_some_and(|s| s.live() && s.info.session_id == session_id && s.verifying)
            {
                return Err(invalid(
                    "Address refresh expired or was cancelled. No address was changed.",
                ));
            }
            self.db.rebind_download(&updated).await?;
            *state = None;
            let mut public_download = updated.clone();
            public_download.cookies = None;
            let _ = self.event_tx.send(CoreEvent::DownloadUpdated {
                download: public_download,
            });
            Ok(updated)
        }
        .await;
        if result.is_err() {
            let mut state = self.refresh.lock().await;
            if let Some(session) = state.as_mut().filter(|s| s.info.session_id == session_id) {
                session.verifying = false;
            }
        }
        result
    }
}
