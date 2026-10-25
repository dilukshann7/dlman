use super::{canonical_url, error};
use crate::DlmanError;
use dlman_types::YoutubeSessionCookie;
use std::{
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};
use url::Url;

/// Credentials deliberately implement neither Debug nor Serialize.
#[derive(Clone)]
pub struct WebMediaContext {
    pub(crate) url: String,
    pub(crate) referrer: Option<String>,
    pub(crate) user_agent: Option<String>,
    pub(crate) cookies: Vec<YoutubeSessionCookie>,
    pub(crate) cookie_header: Option<String>,
}

pub(super) fn header_value(value: &str, limit: usize) -> bool {
    value.len() <= limit
        && !value.chars().any(char::is_control)
        && reqwest::header::HeaderValue::from_str(value).is_ok()
}

fn cookie_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1024
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn valid_cookie(cookie: &YoutubeSessionCookie) -> bool {
    let domain = cookie.domain.strip_prefix('.').unwrap_or(&cookie.domain);
    !domain.is_empty()
        && domain.len() <= 253
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.:[]".contains(&b))
        && cookie.path.starts_with('/')
        && cookie.path.len() <= 4096
        && !cookie
            .path
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        && cookie_name(&cookie.name)
        && cookie.value.len() <= 8192
        && cookie
            .value
            .bytes()
            .all(|b| b >= 0x21 && b <= 0x7e && !b";\"\\".contains(&b))
}

fn path_matches(path: &str, scope: &str) -> bool {
    path == scope
        || (path.starts_with(scope)
            && (scope.ends_with('/') || path.as_bytes().get(scope.len()) == Some(&b'/')))
}

pub(super) fn cookie_matches(cookie: &YoutubeSessionCookie, url: &Url) -> bool {
    let domain = cookie.domain.trim_start_matches('.').to_ascii_lowercase();
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let domain_match =
        host == domain || (!cookie.host_only && host.ends_with(&format!(".{domain}")));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    domain_match
        && path_matches(url.path(), &cookie.path)
        && (!cookie.secure || url.scheme() == "https")
        && !cookie
            .expires
            .is_some_and(|expiry| expiry != 0 && expiry <= now)
}

impl WebMediaContext {
    pub(crate) fn anonymous(url: &str) -> Result<Self, DlmanError> {
        Ok(Self {
            url: canonical_url(url)?,
            referrer: None,
            user_agent: None,
            cookies: Vec::new(),
            cookie_header: None,
        })
    }

    /// Normalize once at the trust boundary and discard unrelated browser cookies.
    pub(super) fn checked(mut self) -> Result<Self, DlmanError> {
        self.url = canonical_url(&self.url)?;
        let source = Url::parse(&self.url).map_err(|_| error("Invalid media source URL."))?;
        if let Some(referrer) = &self.referrer {
            canonical_url(referrer)?;
            if !header_value(referrer, 8192) {
                return Err(error("Invalid media referrer."));
            }
        }
        if self
            .user_agent
            .as_ref()
            .is_some_and(|v| !header_value(v, 2048))
        {
            return Err(error("Invalid browser user agent."));
        }
        if self.cookies.len() > 256 || self.cookies.iter().any(|cookie| !valid_cookie(cookie)) {
            return Err(error(
                "Invalid browser cookies. Detect this video in the browser again.",
            ));
        }
        self.cookies
            .retain(|cookie| cookie_matches(cookie, &source));
        // A captured request authorizes only this source host. Narrow browser domain
        // cookies to that host before the extractor follows redirects or embeds.
        for cookie in &mut self.cookies {
            cookie.domain = source.host_str().unwrap_or("").to_owned();
            cookie.host_only = true;
        }
        if let Some(raw) = self.cookie_header.take() {
            if !header_value(&raw, 65536) {
                return Err(error("Invalid browser Cookie header."));
            }
            for pair in raw.split(';').filter(|pair| !pair.trim().is_empty()) {
                let (name, value) = pair
                    .trim()
                    .split_once('=')
                    .ok_or_else(|| error("Invalid browser Cookie header."))?;
                let cookie = YoutubeSessionCookie {
                    domain: source.host_str().unwrap_or("").to_owned(),
                    path: "/".into(),
                    name: name.trim().into(),
                    value: value.trim().into(),
                    secure: source.scheme() == "https",
                    http_only: true,
                    host_only: true,
                    expires: None,
                };
                if !valid_cookie(&cookie) {
                    return Err(error("Invalid browser Cookie header."));
                }
                if !self.cookies.iter().any(|existing| {
                    existing.name == cookie.name
                        && existing.path == cookie.path
                        && existing.domain == cookie.domain
                }) {
                    self.cookies.push(cookie);
                }
            }
        }
        if self.cookies.len() > 256
            || self
                .cookies
                .iter()
                .map(|c| c.name.len() + c.value.len() + c.domain.len() + c.path.len())
                .sum::<usize>()
                > 65536
        {
            return Err(error("Browser session exceeds the supported size."));
        }
        Ok(self)
    }

    pub(super) fn request(
        &self,
        request: reqwest::RequestBuilder,
        target: &Url,
    ) -> Result<reqwest::RequestBuilder, DlmanError> {
        let mut request = request;
        if let Some(value) = &self.referrer {
            request = request.header(reqwest::header::REFERER, value);
        }
        if let Some(value) = &self.user_agent {
            request = request.header(reqwest::header::USER_AGENT, value);
        }
        let header = self
            .cookies
            .iter()
            .filter(|cookie| cookie_matches(cookie, target))
            .map(|cookie| format!("{}={}", cookie.name, cookie.value))
            .collect::<Vec<_>>()
            .join("; ");
        if !header.is_empty() {
            request = request.header(reqwest::header::COOKIE, header);
        }
        Ok(request)
    }

    /// A Netscape cookie file lets yt-dlp scope credentials on redirects and fragment requests.
    pub(crate) fn tool_args(
        &self,
    ) -> Result<(Vec<String>, Option<tempfile::NamedTempFile>), DlmanError> {
        let context = self.clone().checked()?;
        let mut args = Vec::new();
        if let Some(value) = &context.referrer {
            args.extend(["--referer".into(), value.clone()]);
        }
        if let Some(value) = &context.user_agent {
            args.extend(["--user-agent".into(), value.clone()]);
        }
        if context.cookies.is_empty() {
            return Ok((args, None));
        }
        let failure =
            || error("Cannot prepare a protected browser session. Detect this video again.");
        let mut file = tempfile::Builder::new()
            .prefix("dlman-web-session-")
            .tempfile()
            .map_err(|_| failure())?;
        #[cfg(windows)]
        protect_windows_file(file.path()).map_err(|_| failure())?;
        let mut text = String::from("# Netscape HTTP Cookie File\n");
        for cookie in &context.cookies {
            let domain = cookie.domain.trim_start_matches('.');
            let domain = if cookie.host_only {
                domain.to_owned()
            } else {
                format!(".{domain}")
            };
            text.push_str(&format!(
                "{}{domain}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                if cookie.http_only { "#HttpOnly_" } else { "" },
                if cookie.host_only { "FALSE" } else { "TRUE" },
                cookie.path,
                if cookie.secure { "TRUE" } else { "FALSE" },
                cookie.expires.unwrap_or(0),
                cookie.name,
                cookie.value
            ));
        }
        file.write_all(text.as_bytes()).map_err(|_| failure())?;
        file.flush().map_err(|_| failure())?;
        args.extend([
            "--cookies".into(),
            file.path().to_string_lossy().into_owned(),
        ]);
        Ok((args, Some(file)))
    }
}

#[cfg(windows)]
fn protect_windows_file(path: &std::path::Path) -> std::io::Result<()> {
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
