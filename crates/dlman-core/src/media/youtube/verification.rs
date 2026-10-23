//! Explicit, video-scoped browser sessions. Secrets never enter persisted jobs.
use super::{canonical_url, media_error};
use crate::DlmanError;
use dlman_types::{YoutubeSessionCookie, YoutubeVerification};
use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

const CHALLENGE_TTL: Duration = Duration::from_secs(10 * 60);
const SESSION_TTL: Duration = Duration::from_secs(30 * 60);

pub(super) struct BrowserSession {
    pub id: Uuid,
    created: Instant,
    cookies: Vec<YoutubeSessionCookie>,
}

struct Pending {
    url: String,
    created: Instant,
    completed: bool,
}

#[derive(Default)]
pub(super) struct VerificationStore {
    pending: HashMap<Uuid, Pending>,
    sessions: HashMap<String, Arc<BrowserSession>>,
}

impl VerificationStore {
    fn prune(&mut self) {
        self.pending
            .retain(|_, entry| entry.created.elapsed() < CHALLENGE_TTL);
        self.sessions
            .retain(|_, session| session.created.elapsed() < SESSION_TTL);
    }

    pub fn begin(&mut self, value: &str) -> Result<YoutubeVerification, DlmanError> {
        self.prune();
        let url = canonical_url(value)?;
        if self.pending.len() >= 16 || self.sessions.len() >= 32 {
            return Err(media_error(
                "Too many browser verifications. Try again later.",
            ));
        }
        let id = Uuid::new_v4();
        self.pending.insert(
            id,
            Pending {
                url: url.clone(),
                created: Instant::now(),
                completed: false,
            },
        );
        Ok(YoutubeVerification {
            id,
            url: format!("{url}#dlman-verify={id}"),
        })
    }

    pub fn ready(&mut self, id: Uuid) -> Result<bool, DlmanError> {
        self.prune();
        self.pending
            .get(&id)
            .map(|entry| entry.completed)
            .ok_or_else(|| {
                media_error("Browser verification expired or was cancelled. Start again.")
            })
    }

    pub fn complete(
        &mut self,
        token: Uuid,
        value: &str,
        cookies: Vec<YoutubeSessionCookie>,
    ) -> Result<(), DlmanError> {
        self.prune();
        let url = canonical_url(value)?;
        let pending = self
            .pending
            .get_mut(&token)
            .filter(|entry| !entry.completed && entry.url == url)
            .ok_or_else(|| {
                media_error(
                    "Browser verification expired, was already used, or belongs to another video.",
                )
            })?;
        cookie_text(&cookies)?;
        pending.completed = true;
        self.sessions.insert(
            url,
            Arc::new(BrowserSession {
                id: token,
                created: Instant::now(),
                cookies,
            }),
        );
        Ok(())
    }

    pub fn cancel(&mut self, id: Uuid) {
        self.pending.remove(&id);
        self.sessions.retain(|_, session| session.id != id);
    }

    pub fn session(&mut self, url: &str) -> Option<Arc<BrowserSession>> {
        self.prune();
        self.sessions.get(url).cloned()
    }

    pub fn clear(&mut self) {
        self.pending.clear();
        self.sessions.clear();
    }
}

fn cookie_text(cookies: &[YoutubeSessionCookie]) -> Result<String, DlmanError> {
    let invalid = || media_error("Invalid YouTube browser session. Verify the video again.");
    if cookies.is_empty() || cookies.len() > 256 {
        return Err(invalid());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid())?
        .as_secs();
    let mut text = String::from("# Netscape HTTP Cookie File\n");
    let mut count = 0;
    for cookie in cookies {
        if !matches!(
            cookie.domain.as_str(),
            "youtube.com" | ".youtube.com" | "www.youtube.com"
        ) || !cookie.path.starts_with('/')
            || cookie.name.is_empty()
            || cookie.name.len() > 1024
            || cookie.value.len() > 8192
            || [&cookie.name, &cookie.value, &cookie.path]
                .iter()
                .any(|value| value.chars().any(|c| c.is_control() || c.is_whitespace()))
        {
            return Err(invalid());
        }
        if cookie.expires.is_some_and(|expiry| expiry <= now) {
            continue;
        }
        let prefix = if cookie.http_only { "#HttpOnly_" } else { "" };
        let domain = cookie.domain.trim_start_matches('.');
        let domain = if cookie.host_only {
            domain.to_owned()
        } else {
            format!(".{domain}")
        };
        text.push_str(&format!(
            "{prefix}{domain}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            if cookie.host_only { "FALSE" } else { "TRUE" },
            cookie.path,
            if cookie.secure { "TRUE" } else { "FALSE" },
            cookie.expires.unwrap_or(0),
            cookie.name,
            cookie.value
        ));
        if text.len() > 64 * 1024 {
            return Err(invalid());
        }
        count += 1;
    }
    if count == 0 {
        return Err(invalid());
    }
    Ok(text)
}

pub(super) struct CookieFile(tempfile::NamedTempFile);

impl CookieFile {
    pub fn create(session: &BrowserSession) -> Result<Self, DlmanError> {
        let failure =
            || media_error("Cannot prepare a protected YouTube session. Verify the video again.");
        let text = cookie_text(&session.cookies)?;
        let mut file = tempfile::Builder::new()
            .prefix("dlman-youtube-")
            .tempfile()
            .map_err(|_| failure())?;
        #[cfg(windows)]
        protect_windows_file(file.path()).map_err(|_| failure())?;
        // tempfile creates Unix files with mode 0600; Windows ACLs are restricted before writing.
        file.write_all(text.as_bytes()).map_err(|_| failure())?;
        file.flush().map_err(|_| failure())?;
        Ok(Self(file))
    }

    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

#[cfg(windows)]
fn protect_windows_file(path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::{
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        },
    };
    let sddl = "D:P(A;;FA;;;OW)(A;;FA;;;SY)\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let path = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut descriptor = std::ptr::null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let result = SetFileSecurityW(
            path.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor,
        );
        let error = std::io::Error::last_os_error();
        LocalFree(descriptor);
        if result == 0 {
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str) -> YoutubeSessionCookie {
        YoutubeSessionCookie {
            domain: domain.into(),
            path: "/".into(),
            name: "SID".into(),
            value: "secret".into(),
            secure: true,
            http_only: true,
            host_only: false,
            expires: None,
        }
    }

    #[test]
    fn challenges_are_single_use_expiring_and_bound_to_one_video() {
        let mut store = VerificationStore::default();
        let challenge = store.begin("https://youtu.be/HLUamwXQ218").unwrap();
        assert!(store
            .complete(
                challenge.id,
                "https://youtu.be/abcdefghijk",
                vec![cookie(".youtube.com")]
            )
            .is_err());
        assert!(!store.ready(challenge.id).unwrap());
        store
            .complete(
                challenge.id,
                "https://youtu.be/HLUamwXQ218",
                vec![cookie(".youtube.com")],
            )
            .unwrap();
        assert!(store.ready(challenge.id).unwrap());
        assert!(store
            .complete(
                challenge.id,
                "https://youtu.be/HLUamwXQ218",
                vec![cookie(".youtube.com")]
            )
            .is_err());
        assert!(store
            .session("https://www.youtube.com/watch?v=HLUamwXQ218")
            .is_some());
        assert!(store
            .session("https://www.youtube.com/watch?v=abcdefghijk")
            .is_none());
        store.cancel(challenge.id);
        assert!(store
            .session("https://www.youtube.com/watch?v=HLUamwXQ218")
            .is_none());
        let expired = store.begin("https://youtu.be/HLUamwXQ218").unwrap();
        store.pending.get_mut(&expired.id).unwrap().created -= CHALLENGE_TTL;
        assert!(store
            .complete(
                expired.id,
                "https://youtu.be/HLUamwXQ218",
                vec![cookie(".youtube.com")]
            )
            .is_err());
    }

    #[test]
    fn cookie_jar_rejects_other_domains_and_injected_rows() {
        assert!(cookie_text(&[cookie(".google.com")]).is_err());
        let mut injected = cookie(".youtube.com");
        injected.value = "secret\ninjected".into();
        assert!(cookie_text(&[injected]).is_err());
        assert!(cookie_text(&[]).is_err());
        let text = cookie_text(&[cookie(".youtube.com")]).unwrap();
        assert!(text.contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tsecret\n"));
    }

    #[test]
    fn sessions_expire_before_they_can_be_reused() {
        let mut store = VerificationStore::default();
        let url = "https://www.youtube.com/watch?v=HLUamwXQ218";
        let challenge = store.begin(url).unwrap();
        store
            .complete(challenge.id, url, vec![cookie(".youtube.com")])
            .unwrap();
        let session = store.sessions.get_mut(url).unwrap();
        Arc::get_mut(session).unwrap().created -= SESSION_TTL;
        assert!(store.session(url).is_none());
        assert!(store.sessions.is_empty());
    }

    #[test]
    fn cookie_files_are_removed_when_the_guard_is_dropped() {
        let session = BrowserSession {
            id: Uuid::new_v4(),
            created: Instant::now(),
            cookies: vec![cookie(".youtube.com")],
        };
        let jar = CookieFile::create(&session).unwrap();
        let path = jar.path().to_path_buf();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("SID\tsecret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(jar);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_fresh_session_can_recover_an_initially_anonymous_job() {
        let service = super::super::YoutubeService::default();
        let url = "https://www.youtube.com/watch?v=HLUamwXQ218";
        assert!(service.browser_session(url, false).await.unwrap().is_none());
        assert!(service.browser_session(url, true).await.is_err());
        let verification = service.begin_verification(url).await.unwrap();
        service
            .complete_verification(dlman_types::YoutubeVerificationSubmission {
                token: verification.id,
                url: url.into(),
                cookies: vec![cookie(".youtube.com")],
            })
            .await
            .unwrap();
        assert!(service.browser_session(url, false).await.unwrap().is_some());
        assert!(service.browser_session(url, true).await.unwrap().is_some());
    }

    #[test]
    fn verified_job_metadata_never_serializes_browser_secrets() {
        let job: dlman_types::YoutubeJob = serde_json::from_value(serde_json::json!({
            "selection": { "identity": "Youtube:HLUamwXQ218", "format_id": "140", "container": "mp3", "quality": "Audio", "output_mode": "mp3" },
            "stage_dir": "/temporary/stage", "requires_browser_session": true
        })).unwrap();
        let persisted = serde_json::to_value(job).unwrap();
        assert_eq!(persisted["requires_browser_session"], true);
        assert!(persisted.get("cookies").is_none());
        assert!(persisted.get("token").is_none());
    }
}
