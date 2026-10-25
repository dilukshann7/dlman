use super::{canonical_url, error, WebMediaContext};
use crate::DlmanError;
use dlman_types::ProxySettings;
use futures::StreamExt;
use reqwest::{header, Client};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ResourceKind {
    Page,
    Manifest,
    File,
}

fn known_file_url(url: &url::Url) -> bool {
    let extension = url
        .path()
        .rsplit('/')
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    extension.is_some_and(|extension| {
        matches!(
            extension.as_str(),
            "zip"
                | "rar"
                | "7z"
                | "tar"
                | "gz"
                | "bz2"
                | "xz"
                | "zst"
                | "iso"
                | "exe"
                | "msi"
                | "msix"
                | "msixbundle"
                | "appx"
                | "apk"
                | "dmg"
                | "pkg"
                | "deb"
                | "rpm"
                | "pdf"
                | "doc"
                | "docx"
                | "xls"
                | "xlsx"
                | "ppt"
                | "pptx"
                | "odt"
                | "ods"
                | "epub"
                | "rtf"
                | "txt"
                | "csv"
                | "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "bmp"
                | "tif"
                | "tiff"
                | "svg"
                | "ico"
                | "avif"
                | "mp4"
                | "webm"
                | "mkv"
                | "avi"
                | "mov"
                | "wmv"
                | "m4v"
                | "mpeg"
                | "mpg"
                | "mp3"
                | "m4a"
                | "aac"
                | "flac"
                | "wav"
                | "ogg"
                | "opus"
        )
    })
}

/// MIME and a small prefix take precedence over suffixes, including signed extensionless manifests.
pub(super) fn classify(mime: &str, body: &[u8]) -> ResourceKind {
    let mime = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let text = String::from_utf8_lossy(body);
    let text = text
        .trim_start_matches(|c: char| c == '\u{feff}' || c.is_whitespace())
        .to_ascii_lowercase();
    if text.starts_with("#extm3u")
        || text.starts_with("<mpd")
        || (text.starts_with('<') && text.contains("<mpd"))
        || matches!(
            mime.as_str(),
            "application/vnd.apple.mpegurl"
                | "application/x-mpegurl"
                | "audio/mpegurl"
                | "audio/x-mpegurl"
                | "application/dash+xml"
        )
    {
        ResourceKind::Manifest
    } else if matches!(mime.as_str(), "text/html" | "application/xhtml+xml")
        || text.starts_with("<!doctype html")
        || text.starts_with("<html")
        || text.starts_with("<head")
        || text.starts_with("<body")
        || (text.starts_with("<!--") && (text.contains("<html") || text.contains("<!doctype html")))
        || (text.starts_with("<?xml") && text.contains("<html"))
        || (mime.starts_with("text/")
            && !matches!(
                mime.as_str(),
                "text/plain"
                    | "text/csv"
                    | "text/tab-separated-values"
                    | "text/css"
                    | "text/javascript"
                    | "text/calendar"
                    | "text/vcard"
                    | "text/xml"
            ))
    {
        ResourceKind::Page
    } else {
        ResourceKind::File
    }
}

fn client(settings: &ProxySettings) -> Result<Client, DlmanError> {
    let mut builder = Client::builder()
        .user_agent(crate::USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(25))
        .redirect(reqwest::redirect::Policy::none());
    match settings.mode.as_str() {
        "none" => builder = builder.no_proxy(),
        "manual" => {
            builder = builder.no_proxy();
            let bypass = reqwest::NoProxy::from_string(settings.no_proxy.as_deref().unwrap_or(""));
            for (scheme, address) in [
                ("http", &settings.http_proxy),
                ("https", &settings.https_proxy),
            ] {
                let Some(address) = address.as_ref().filter(|value| !value.is_empty()) else {
                    continue;
                };
                let mut proxy = if scheme == "https" {
                    reqwest::Proxy::https(address)
                } else {
                    reqwest::Proxy::http(address)
                }
                .map_err(|_| error("Invalid media inspection proxy configuration."))?
                .no_proxy(bypass.clone());
                if let Some(user) = settings.username.as_ref().filter(|value| !value.is_empty()) {
                    proxy = proxy.basic_auth(user, settings.password.as_deref().unwrap_or(""));
                }
                builder = builder.proxy(proxy);
            }
        }
        "system" | "" => {}
        _ => return Err(error("Invalid media inspection proxy mode.")),
    }
    builder
        .build()
        .map_err(|_| error("Cannot create the media inspection HTTP client."))
}

pub(super) async fn probe(
    context: &WebMediaContext,
    settings: &ProxySettings,
    cancel: CancellationToken,
) -> Result<ResourceKind, DlmanError> {
    probe_with_credentials(context, settings, cancel, None).await
}

fn authorize_probe(
    request: reqwest::RequestBuilder,
    target: &url::Url,
    original_origin: &url::Origin,
    credentials: Option<&(String, String)>,
) -> reqwest::RequestBuilder {
    if target.origin() == *original_origin {
        if let Some((username, password)) = credentials {
            return request.basic_auth(username, Some(password));
        }
    }
    request
}

pub(super) async fn probe_with_credentials(
    context: &WebMediaContext,
    settings: &ProxySettings,
    cancel: CancellationToken,
    credentials: Option<&(String, String)>,
) -> Result<ResourceKind, DlmanError> {
    let client = client(settings)?;
    let operation = async {
        let mut target = url::Url::parse(&context.url).map_err(|_| error("Invalid media URL."))?;
        let original_origin = target.origin();
        for _ in 0..=8 {
            // Always use a bounded GET: servers that reject HEAD still classify correctly.
            let request = context.request(
                client
                    .get(target.clone())
                    .header(header::RANGE, "bytes=0-16383")
                    .header(header::ACCEPT_ENCODING, "identity"),
                &target,
            )?;
            let request = authorize_probe(request, &target, &original_origin, credentials);
            let mut response = request.send().await.map_err(|_| {
                error(
                    "Cannot reach this webpage. Check the URL, connection, and proxy, then Retry.",
                )
            })?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| error("The webpage returned an invalid redirect."))?;
                let redirected = target
                    .join(location)
                    .map_err(|_| error("The webpage returned an invalid redirect."))?;
                target = url::Url::parse(&canonical_url(redirected.as_str())?)
                    .map_err(|_| error("Invalid redirect URL."))?;
                continue;
            }
            if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                // A zero-length resource or a server that refuses Range needs an ordinary GET.
                let request = context.request(
                    client
                        .get(target.clone())
                        .header(header::ACCEPT_ENCODING, "identity"),
                    &target,
                )?;
                response = authorize_probe(request, &target, &original_origin, credentials)
                    .send()
                    .await
                    .map_err(|_| {
                        error("Cannot inspect this webpage. Retry after checking the connection.")
                    })?;
            }
            if !response.status().is_success() {
                let mime = response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("");
                if matches!(
                    response.status(),
                    reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
                ) && known_file_url(&target)
                    && classify(mime, &[]) != ResourceKind::Manifest
                {
                    // The regular file probe can use saved site credentials. Do not
                    // divert known authenticated files into webpage extraction.
                    return Ok(ResourceKind::File);
                }
                return Err(error(&format!("Webpage inspection returned HTTP {}. Open the page and use Detect in browser, then Retry.", response.status().as_u16())));
            }
            let mime = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned();
            let attachment = response
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    value
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case("attachment")
                });
            let mut stream = response.bytes_stream();
            let mut prefix = Vec::with_capacity(16384);
            while prefix.len() < 16384 {
                let Some(chunk) = stream.next().await else {
                    break;
                };
                let chunk = chunk.map_err(|_| {
                    error("The webpage response was interrupted. Retry inspection.")
                })?;
                let count = chunk.len().min(16384 - prefix.len());
                prefix.extend_from_slice(&chunk[..count]);
            }
            let kind = classify(&mime, &prefix);
            return Ok(if attachment && kind != ResourceKind::Manifest {
                ResourceKind::File
            } else {
                kind
            });
        }
        Err(error("The webpage redirected too many times."))
    };
    tokio::select! {
        _ = cancel.cancelled() => Err(DlmanError::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(35), operation) => result.map_err(|_| error("Webpage inspection timed out. Check the connection and Retry."))?,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    async fn response_server(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            while request.len() < 16384 && !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            stream.write_all(&response).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (
            format!("http://{address}/extensionless?token=private"),
            task,
        )
    }

    fn direct() -> ProxySettings {
        ProxySettings {
            mode: "none".into(),
            ..ProxySettings::default()
        }
    }

    #[tokio::test]
    async fn http_failure_is_actionable_and_never_a_file_or_credential_disclosure() {
        let (url, server) =
            response_server(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let context = WebMediaContext::anonymous(&url).unwrap();
        let failure = probe(&context, &direct(), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("HTTP 403"));
        assert!(failure.contains("Detect in browser"));
        assert!(!failure.contains("private"));
        let request = server.await.unwrap();
        assert!(request.starts_with("GET "));
        assert!(request
            .to_ascii_lowercase()
            .contains("range: bytes=0-16383"));
    }

    #[tokio::test]
    async fn binary_response_is_classified_without_reading_full_content_length() {
        let mut response =
            b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 104857600\r\n\r\n"
                .to_vec();
        response.extend(vec![0; 16384]);
        let (url, server) = response_server(response).await;
        assert_eq!(
            probe(
                &WebMediaContext::anonymous(&url).unwrap(),
                &direct(),
                CancellationToken::new()
            )
            .await
            .unwrap(),
            ResourceKind::File
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn manual_http_proxy_receives_extensionless_manifest_request() {
        let body = "#EXTM3U\n#EXT-X-VERSION:3\n";
        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\n\r\n{body}", body.len());
        let (proxy, server) = response_server(response.into_bytes()).await;
        let proxy = url::Url::parse(&proxy)
            .unwrap()
            .origin()
            .ascii_serialization();
        let settings = ProxySettings {
            mode: "manual".into(),
            http_proxy: Some(proxy),
            ..ProxySettings::default()
        };
        let context = WebMediaContext::anonymous("http://example.invalid/manifest").unwrap();
        assert_eq!(
            probe(&context, &settings, CancellationToken::new())
                .await
                .unwrap(),
            ResourceKind::Manifest
        );
        assert!(server
            .await
            .unwrap()
            .starts_with("GET http://example.invalid/manifest "));
    }

    #[tokio::test]
    async fn known_authenticated_files_return_to_regular_probe_but_manifests_do_not() {
        for path in [
            "archive.zip",
            "document.pdf",
            "installer.msi",
            "photo.jpg",
            "video.mp4",
        ] {
            let (url, server) =
                response_server(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec())
                    .await;
            let mut url = url::Url::parse(&url).unwrap();
            url.set_path(path);
            assert_eq!(
                probe(
                    &WebMediaContext::anonymous(url.as_str()).unwrap(),
                    &direct(),
                    CancellationToken::new()
                )
                .await
                .unwrap(),
                ResourceKind::File
            );
            server.await.unwrap();
        }
        for path in [
            "master.m3u8",
            "manifest.mpd",
            "watch.html",
            "watch",
            "page.php",
        ] {
            let (url, server) =
                response_server(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n".to_vec())
                    .await;
            let mut url = url::Url::parse(&url).unwrap();
            url.set_path(path);
            assert!(probe(
                &WebMediaContext::anonymous(url.as_str()).unwrap(),
                &direct(),
                CancellationToken::new()
            )
            .await
            .is_err());
            server.await.unwrap();
        }
    }

    #[test]
    fn basic_authorization_is_bound_to_scheme_host_and_port() {
        let client = client(&direct()).unwrap();
        let original = url::Url::parse("http://example.com:8000/download?id=42").unwrap();
        let credentials = ("user".to_owned(), "pass".to_owned());
        for (value, authorized) in [
            ("http://example.com:8000/another", true),
            ("http://example.com:8001/another", false),
            ("https://example.com:8000/another", false),
            ("http://other.example.com:8000/another", false),
        ] {
            let target = url::Url::parse(value).unwrap();
            let request = authorize_probe(
                client.get(target.clone()),
                &target,
                &original.origin(),
                Some(&credentials),
            )
            .build()
            .unwrap();
            assert_eq!(
                request.headers().contains_key(header::AUTHORIZATION),
                authorized
            );
        }
    }

    #[tokio::test]
    async fn saved_basic_credentials_classify_extensionless_downloads_as_files() {
        let (url, server) = response_server(b"HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: 4\r\n\r\nPK\x03\x04".to_vec()).await;
        let mut url = url::Url::parse(&url).unwrap();
        url.set_path("/download");
        url.set_query(Some("id=42"));
        let credentials = ("user".to_owned(), "pass".to_owned());
        let result = probe_with_credentials(
            &WebMediaContext::anonymous(url.as_str()).unwrap(),
            &direct(),
            CancellationToken::new(),
            Some(&credentials),
        )
        .await
        .unwrap();
        assert_eq!(result, ResourceKind::File);
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /download?id=42 "));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: basic dxnlcjpwyxnz\r\n"));
    }

    #[tokio::test]
    async fn redirects_to_another_origin_never_receive_saved_credentials() {
        let (destination, destination_server) = response_server(b"HTTP/1.1 200 OK\r\nContent-Type: application/zip\r\nContent-Length: 4\r\n\r\nPK\x03\x04".to_vec()).await;
        let response =
            format!("HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\n\r\n");
        let (initial, initial_server) = response_server(response.into_bytes()).await;
        let credentials = ("user".to_owned(), "pass".to_owned());
        let context = WebMediaContext::anonymous(&initial).unwrap();
        assert_eq!(
            probe_with_credentials(
                &context,
                &direct(),
                CancellationToken::new(),
                Some(&credentials)
            )
            .await
            .unwrap(),
            ResourceKind::File
        );
        assert!(initial_server
            .await
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization: basic "));
        assert!(!destination_server
            .await
            .unwrap()
            .to_ascii_lowercase()
            .contains("authorization:"));
    }
}
