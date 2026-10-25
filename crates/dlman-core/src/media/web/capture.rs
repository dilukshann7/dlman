use super::{canonical_url, error, WebMediaContext};
use crate::DlmanError;
use dlman_types::*;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use uuid::Uuid;

const CAPTURE_TTL: Duration = Duration::from_secs(600);
const CONTEXT_TTL: Duration = Duration::from_secs(1800);

struct CaptureEntry {
    result: WebMediaCapture,
    created: Instant,
    owner: Option<(String, i64)>,
}

struct ContextEntry {
    page: String,
    context: WebMediaContext,
    updated: Instant,
    revision: Uuid,
}

#[derive(Default)]
pub(super) struct CaptureStore {
    captures: HashMap<Uuid, CaptureEntry>,
    contexts: HashMap<String, ContextEntry>,
}

pub(super) fn source_key(page: &str, source: &str) -> Result<String, DlmanError> {
    let mut source =
        url::Url::parse(&canonical_url(source)?).map_err(|_| error("Invalid media source URL."))?;
    // Retain resource selectors such as video/id/quality. Only recognized rotating
    // authorization parameters are omitted from the stable source identity.
    let mut query: Vec<(String, String)> = source
        .query_pairs()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            !matches!(
                name.as_str(),
                "token"
                    | "expires"
                    | "exp"
                    | "signature"
                    | "sig"
                    | "policy"
                    | "key-pair-id"
                    | "hmac"
                    | "hdnea"
                    | "hdntl"
                    | "auth"
                    | "auth_key"
                    | "v-acctoken"
            ) && !name.starts_with("x-amz-")
                && !name.starts_with("x-goog-")
        })
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    query.sort();
    source.set_query(None);
    if !query.is_empty() {
        source.query_pairs_mut().extend_pairs(query);
    }
    source.set_fragment(None);
    let mut hash = Sha256::new();
    hash.update(page.as_bytes());
    hash.update([0]);
    hash.update(source.as_str().as_bytes());
    Ok(format!("{:x}", hash.finalize()))
}

fn valid_claim(client: &str, tab: i64) -> Result<(), DlmanError> {
    if tab < 0
        || client.is_empty()
        || client.len() > 128
        || !client
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
    {
        return Err(error("Invalid browser capture owner."));
    }
    Ok(())
}

impl CaptureStore {
    fn clean(&mut self) {
        self.contexts
            .retain(|_, entry| entry.updated.elapsed() < CONTEXT_TTL);
        for entry in self.captures.values_mut() {
            if entry.created.elapsed() >= CAPTURE_TTL {
                entry.result.status = "expired".into();
                entry.result.sources.clear();
                entry.owner = None;
            }
        }
    }

    pub(super) fn begin(&mut self, value: &str) -> Result<WebMediaCapture, DlmanError> {
        self.clean();
        self.captures
            .retain(|_, entry| !matches!(entry.result.status.as_str(), "expired" | "cancelled"));
        if self.captures.len() >= 16 {
            return Err(error(
                "Too many browser capture requests. Close a capture and Retry.",
            ));
        }
        let result = WebMediaCapture {
            id: Uuid::new_v4(),
            url: canonical_url(value)?,
            status: "waiting".into(),
            sources: Vec::new(),
            error: None,
            bridge_url: None,
        };
        self.captures.insert(
            result.id,
            CaptureEntry {
                result: result.clone(),
                created: Instant::now(),
                owner: None,
            },
        );
        Ok(result)
    }

    pub(super) fn capture(&mut self, id: Uuid) -> Result<WebMediaCapture, DlmanError> {
        self.clean();
        self.captures
            .get(&id)
            .map(|entry| entry.result.clone())
            .ok_or_else(|| error("Browser capture no longer exists. Use Detect in browser again."))
    }

    fn active(&mut self, id: Uuid) -> Result<&mut CaptureEntry, DlmanError> {
        self.clean();
        self.captures
            .get_mut(&id)
            .filter(|entry| !matches!(entry.result.status.as_str(), "expired" | "cancelled"))
            .ok_or_else(|| {
                error("Browser capture expired or was cancelled. Use Detect in browser again.")
            })
    }

    pub(super) fn claim(
        &mut self,
        id: Uuid,
        claim: WebMediaCaptureClaim,
    ) -> Result<WebMediaCapture, DlmanError> {
        valid_claim(&claim.client_id, claim.tab_id)?;
        let entry = self.active(id)?;
        let owner = (claim.client_id, claim.tab_id);
        if entry
            .owner
            .as_ref()
            .is_some_and(|existing| existing != &owner)
        {
            return Err(error("This browser capture belongs to another tab."));
        }
        entry.owner = Some(owner);
        if entry.result.sources.is_empty() {
            entry.result.status = "watching".into();
        }
        Ok(entry.result.clone())
    }

    pub(super) fn submit(
        &mut self,
        id: Uuid,
        submission: WebMediaCaptureSubmission,
    ) -> Result<WebMediaCapture, DlmanError> {
        valid_claim(&submission.client_id, submission.tab_id)?;
        let page = canonical_url(&submission.page_url)?;
        if submission.sources.is_empty() || submission.sources.len() > 64 {
            return Err(error(
                "A capture must contain between 1 and 64 media sources.",
            ));
        }
        {
            let entry = self.active(id)?;
            if entry.owner.as_ref() != Some(&(submission.client_id, submission.tab_id)) {
                return Err(error("This browser capture belongs to another tab. Claim it before submitting sources."));
            }
            if entry.result.url != page {
                return Err(error("The browser tab navigated away from the requested page. Start Detect in browser again."));
            }
        }
        // Validate the complete submission before mutating either tickets or job contexts.
        let mut prepared = Vec::new();
        for source in submission.sources {
            if source
                .title
                .as_ref()
                .is_some_and(|value| value.len() > 8192)
            {
                return Err(error(
                    "The captured video title exceeds the supported size.",
                ));
            }
            let protocol = source.protocol.to_ascii_lowercase();
            let protocol = match protocol.as_str() {
                "hls" | "m3u8" => "hls",
                "dash" | "mpd" => "dash",
                "direct" | "mp4" | "webm" | "http" | "https" | "file" | "progressive" => "direct",
                _ => return Err(error("Unsupported captured media protocol.")),
            }
            .to_owned();
            let title = source
                .title
                .map(|value| {
                    value
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(512)
                        .collect::<String>()
                })
                .filter(|value| !value.is_empty());
            let context = WebMediaContext {
                url: source.url,
                referrer: source.referrer.or_else(|| Some(page.clone())),
                user_agent: source.user_agent,
                cookies: source.cookies,
                cookie_header: None,
            }
            .checked()?;
            let key = source_key(&page, &context.url)?;
            prepared.push((key, protocol, title, context));
        }
        let entry = self
            .captures
            .get(&id)
            .ok_or_else(|| error("Browser capture expired."))?;
        let mut ids: std::collections::HashSet<_> = entry
            .result
            .sources
            .iter()
            .map(|source| source.id.clone())
            .collect();
        ids.extend(prepared.iter().map(|(key, _, _, _)| key.clone()));
        if ids.len() > 64 {
            return Err(error("This page has reached the 64-source capture limit."));
        }
        let new_contexts = prepared
            .iter()
            .filter(|(key, _, _, _)| !self.contexts.contains_key(key))
            .map(|(key, _, _, _)| key)
            .collect::<std::collections::HashSet<_>>()
            .len();
        if self.contexts.len() + new_contexts > 1024 {
            return Err(error(
                "Browser session capacity reached. Wait a few minutes and Retry.",
            ));
        }
        for (key, protocol, title, context) in prepared {
            let visible = WebMediaSource {
                id: key.clone(),
                url: context.url.clone(),
                title,
                protocol,
            };
            let revision = self.revision_for(&key, &context);
            self.contexts.insert(
                key.clone(),
                ContextEntry {
                    page: page.clone(),
                    context,
                    updated: Instant::now(),
                    revision,
                },
            );
            let entry = self
                .captures
                .get_mut(&id)
                .ok_or_else(|| error("Browser capture expired."))?;
            if let Some(existing) = entry
                .result
                .sources
                .iter_mut()
                .find(|source| source.id == key)
            {
                *existing = visible;
            } else {
                entry.result.sources.push(visible);
            }
            entry.result.status = "ready".into();
        }
        self.capture(id)
    }

    pub(super) fn cancel(&mut self, id: Uuid) {
        if let Some(entry) = self.captures.get_mut(&id) {
            entry.result.status = "cancelled".into();
            entry.result.sources.clear();
            entry.owner = None;
        }
        // Jobs keep their temporary source context after the dialog stops watching.
        self.clean();
    }

    pub(super) fn cancel_claim(
        &mut self,
        id: Uuid,
        claim: WebMediaCaptureClaim,
    ) -> Result<(), DlmanError> {
        valid_claim(&claim.client_id, claim.tab_id)?;
        let entry = self.active(id)?;
        if entry.owner.as_ref() != Some(&(claim.client_id, claim.tab_id)) {
            return Err(error("This browser capture belongs to another tab."));
        }
        self.cancel(id);
        Ok(())
    }

    pub(super) fn selected(
        &mut self,
        id: Uuid,
        page: &str,
        source: &str,
    ) -> Result<(WebMediaContext, Uuid), DlmanError> {
        let entry = self.active(id)?;
        if entry.result.url != page || !entry.result.sources.iter().any(|item| item.id == source) {
            return Err(error(
                "The selected browser source does not belong to this page.",
            ));
        }
        self.context(page, source)
    }

    pub(super) fn context(
        &mut self,
        page: &str,
        key: &str,
    ) -> Result<(WebMediaContext, Uuid), DlmanError> {
        self.clean();
        self.contexts.get(key).filter(|entry| entry.page == page)
            .map(|entry| (entry.context.clone(), entry.revision))
            .ok_or_else(|| error("Browser media context expired or the app restarted. Use Detect in browser for this page, then Retry the existing download."))
    }

    pub(super) fn legacy(
        &mut self,
        page: &str,
        context: WebMediaContext,
    ) -> Result<(String, Uuid), DlmanError> {
        self.clean();
        let context = context.checked()?;
        let key = source_key(page, &context.url)?;
        if self.contexts.len() >= 1024 && !self.contexts.contains_key(&key) {
            return Err(error(
                "Browser session capacity reached. Wait a few minutes and Retry.",
            ));
        }
        let revision = self.revision_for(&key, &context);
        self.contexts.insert(
            key.clone(),
            ContextEntry {
                page: page.to_owned(),
                context,
                updated: Instant::now(),
                revision,
            },
        );
        Ok((key, revision))
    }

    pub(super) fn clear(&mut self) {
        self.captures.clear();
        self.contexts.clear();
    }

    fn revision_for(&self, key: &str, context: &WebMediaContext) -> Uuid {
        self.contexts
            .get(key)
            .filter(|entry| {
                super::inspection_fingerprint(&entry.context, &ProxySettings::default())
                    == super::inspection_fingerprint(context, &ProxySettings::default())
            })
            .map(|entry| entry.revision)
            .unwrap_or_else(Uuid::new_v4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = "https://example.com/watch/1";

    #[test]
    fn capture_and_job_context_have_independent_expiry() {
        let mut store = CaptureStore::default();
        let capture = store.begin(PAGE).unwrap();
        let (key, _) = store
            .legacy(
                PAGE,
                WebMediaContext::anonymous("https://cdn.example.com/master?token=private").unwrap(),
            )
            .unwrap();
        store.captures.get_mut(&capture.id).unwrap().created = Instant::now() - CAPTURE_TTL;
        assert_eq!(store.capture(capture.id).unwrap().status, "expired");
        assert!(store.context(PAGE, &key).is_ok());
        store.contexts.get_mut(&key).unwrap().updated = Instant::now() - CONTEXT_TTL;
        assert!(store.context(PAGE, &key).is_err());
    }

    #[test]
    fn ticket_and_source_limits_are_enforced_without_partial_updates() {
        let mut store = CaptureStore::default();
        let first = store.begin(PAGE).unwrap();
        for _ in 1..16 {
            store.begin(PAGE).unwrap();
        }
        assert!(store.begin(PAGE).is_err());
        let claim = WebMediaCaptureClaim {
            client_id: "client".into(),
            tab_id: 1,
        };
        store.claim(first.id, claim).unwrap();
        let source = |index| WebMediaCapturedSource {
            url: format!("https://cdn.example.com/media/{index}"),
            title: None,
            protocol: "hls".into(),
            referrer: None,
            user_agent: None,
            cookies: Vec::new(),
        };
        let request = |sources| WebMediaCaptureSubmission {
            client_id: "client".into(),
            tab_id: 1,
            page_url: PAGE.into(),
            sources,
        };
        store
            .submit(first.id, request((0..64).map(source).collect()))
            .unwrap();
        assert!(store.submit(first.id, request(vec![source(65)])).is_err());
        assert_eq!(store.capture(first.id).unwrap().sources.len(), 64);
        assert_eq!(store.contexts.len(), 64);
        store.cancel(first.id);
        assert!(store.begin(PAGE).is_ok());
    }

    #[test]
    fn invalid_submission_does_not_replace_valid_contexts() {
        let mut store = CaptureStore::default();
        let capture = store.begin(PAGE).unwrap();
        store
            .claim(
                capture.id,
                WebMediaCaptureClaim {
                    client_id: "client".into(),
                    tab_id: 1,
                },
            )
            .unwrap();
        let source = WebMediaCapturedSource {
            url: "https://cdn.example.com/media/master".into(),
            title: None,
            protocol: "hls".into(),
            referrer: None,
            user_agent: None,
            cookies: Vec::new(),
        };
        let mut invalid = source.clone();
        invalid.user_agent = Some("injected\r\nHeader: value".into());
        assert!(store
            .submit(
                capture.id,
                WebMediaCaptureSubmission {
                    client_id: "client".into(),
                    tab_id: 1,
                    page_url: PAGE.into(),
                    sources: vec![source, invalid]
                }
            )
            .is_err());
        assert!(store.contexts.is_empty());
        assert!(store.capture(capture.id).unwrap().sources.is_empty());
    }
}
