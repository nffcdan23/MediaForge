//! Links that point straight at an image file.
//!
//! yt-dlp treats any file that isn't a web page as a "video", so MediaForge recognises images itself and
//! fetches them with its own HTTP client: no cookies, an honest user agent, certificates checked, and the
//! size, speed and time limits from Settings. The file is identified from its contents, not from what the
//! server claims.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use reqwest::header::{ACCEPT, CONTENT_DISPOSITION, CONTENT_TYPE, HeaderName};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::engine::media_files::IMAGE_EXTENSIONS;
use crate::engine::output_files::is_out_of_space;
use crate::error::{Failure, JobResult, MediaForgeError};
use crate::media::image::{self, Sniffed, SourceKind};
use crate::models::DownloadLimits;
use crate::support::url_validator::{ValidatedUrl, percent_decode};

/// Largest image MediaForge downloads, whatever the size limit in Settings.
pub const MAX_IMAGE_BYTES: u64 = 200 * 1024 * 1024;
/// How much of an image a lookup reads at most, to learn its size.
const PROBE_BYTES: usize = 1024 * 1024;
/// Enough of the start of a file to recognise it.
const HEAD_BYTES: usize = 32;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(45);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Lossless and original formats first, so a server that can choose sends the best copy it has.
const ACCEPTED: &str =
    "image/png,image/jpeg;q=0.9,image/webp;q=0.8,image/gif;q=0.7,image/bmp;q=0.6,image/tiff;q=0.6,image/*;q=0.5,*/*;q=0.1";

/// A fresh client for each request: no cookies or connections shared between jobs.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("MediaForge/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default()
}

/// What an image link holds, from the start of the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageLink {
    pub title: String,
    pub kind: SourceKind,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The file's size, when the server says.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    Image(ImageLink),
    NotImage,
}

/// Whether a link's address names an image file, so it's worth checking before asking yt-dlp.
pub fn looks_like_image(url: &ValidatedUrl) -> bool {
    let path = url.path().to_ascii_lowercase();
    let name = path.rsplit('/').next().unwrap_or("");
    name.rsplit_once('.').is_some_and(|(_, extension)| IMAGE_EXTENSIONS.contains(&extension))
}

/// Looks a link up: whether it's an image, which kind, and how big, reading only the start of it.
pub async fn probe(url: &ValidatedUrl, cancel: &CancellationToken) -> JobResult<Probe> {
    let mut response = request(url, cancel).await?;
    let content_type = header(&response, CONTENT_TYPE).to_ascii_lowercase();
    let title = title_for(url, &header(&response, CONTENT_DISPOSITION));
    let size = response.content_length().filter(|s| *s > 0);

    let mut head: Vec<u8> = Vec::new();
    while head.len() < HEAD_BYTES {
        match next_chunk(&mut response, cancel).await? {
            Some(chunk) => head.extend_from_slice(&chunk),
            None => break,
        }
    }
    let kind = match image::sniff(&head) {
        Sniffed::Image(kind) => kind,
        Sniffed::Unsupported(name) if content_type.starts_with("image/") => return Err(MediaForgeError::UnsupportedImage(name).into()),
        _ => match unsupported_type(&content_type) {
            Some(name) => return Err(MediaForgeError::UnsupportedImage(name.into()).into()),
            None => return Ok(Probe::NotImage),
        },
    };
    // Most formats state their size in the first few hundred bytes; a JPEG with a large preview inside may
    // need more. The rest of the file is never fetched.
    let mut dimensions = image::dimensions(&head);
    while dimensions.is_none() && head.len() < PROBE_BYTES {
        match next_chunk(&mut response, cancel).await? {
            Some(chunk) => {
                head.extend_from_slice(&chunk);
                dimensions = image::dimensions(&head);
            }
            None => break,
        }
    }
    Ok(Probe::Image(ImageLink { title, kind, width: dimensions.map(|d| d.0), height: dimensions.map(|d| d.1), size }))
}

/// Image types servers name that MediaForge can't convert, for a clear message instead of a generic failure.
fn unsupported_type(content_type: &str) -> Option<&'static str> {
    let subtype = content_type.strip_prefix("image/")?.split(';').next().unwrap_or("").trim();
    Some(match subtype {
        "svg+xml" => "SVG",
        "avif" => "AVIF",
        "heic" | "heif" | "heic-sequence" | "heif-sequence" => "HEIC",
        "jxl" => "JPEG XL",
        "x-icon" | "vnd.microsoft.icon" => "ICO",
        _ => return None,
    })
}

/// Progress of a download.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transfer {
    pub received: u64,
    pub total: Option<u64>,
    pub bytes_per_second: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Downloaded {
    pub path: PathBuf,
    pub kind: SourceKind,
    /// A title from the file's name, for links queued without one.
    pub title: String,
}

/// Downloads an image into `folder` (a job's private working folder), reporting progress as it arrives.
pub async fn download(
    url: &ValidatedUrl,
    folder: &Path,
    limits: DownloadLimits,
    cancel: &CancellationToken,
    mut progress: impl FnMut(Transfer),
) -> JobResult<Downloaded> {
    let started = Instant::now();
    let mut response = request(url, cancel).await?;
    let title = title_for(url, &header(&response, CONTENT_DISPOSITION));
    let cap = limits.max_file_size.map_or(MAX_IMAGE_BYTES, |limit| limit.min(MAX_IMAGE_BYTES));
    let too_big = || -> Failure {
        if limits.max_file_size.is_some_and(|limit| limit <= MAX_IMAGE_BYTES) {
            MediaForgeError::ExceedsSizeLimit.into()
        } else {
            MediaForgeError::DownloadFailed(format!("MediaForge downloads images of up to {} MB.", MAX_IMAGE_BYTES / 1024 / 1024)).into()
        }
    };
    let total = response.content_length().filter(|t| *t > 0);
    if total.is_some_and(|t| t > cap) {
        return Err(too_big());
    }

    let partial = folder.join("image.part");
    let mut file = tokio::fs::File::create(&partial).await.map_err(write_error)?;
    let mut received: u64 = 0;
    let mut head: Vec<u8> = Vec::new();
    let mut first_byte: Option<Instant> = None;
    while let Some(chunk) = next_chunk(&mut response, cancel).await? {
        if started.elapsed() > TOTAL_TIMEOUT {
            return Err(MediaForgeError::TimedOut.into());
        }
        received += chunk.len() as u64;
        if received > cap {
            return Err(too_big());
        }
        if head.len() < HEAD_BYTES {
            let wanted = (HEAD_BYTES - head.len()).min(chunk.len());
            head.extend_from_slice(&chunk[..wanted]);
            // Don't fetch a whole web page or video before noticing it isn't a picture.
            if head.len() == HEAD_BYTES && !matches!(image::sniff(&head), Sniffed::Image(_)) {
                return Err(not_an_image(&head));
            }
        }
        file.write_all(&chunk).await.map_err(write_error)?;
        let since = *first_byte.get_or_insert_with(Instant::now);
        let elapsed = since.elapsed().as_secs_f64();
        progress(Transfer { received, total, bytes_per_second: (elapsed > 0.25).then(|| received as f64 / elapsed) });

        if let Some(rate) = limits.rate_limit.filter(|r| *r > 0) {
            let due = Duration::from_secs_f64(received as f64 / rate as f64);
            let spent = since.elapsed();
            if due > spent {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(Failure::Cancelled),
                    _ = tokio::time::sleep(due - spent) => {}
                }
            }
        }
    }
    file.flush().await.map_err(write_error)?;
    drop(file);
    if received == 0 || total.is_some_and(|t| received < t) {
        return Err(MediaForgeError::DownloadFailed("The download stopped before the whole image arrived.".into()).into());
    }
    let kind = match image::sniff(&head) {
        Sniffed::Image(kind) => kind,
        _ => return Err(not_an_image(&head)),
    };
    let path = folder.join(format!("image.{}", kind.extension()));
    tokio::fs::rename(&partial, &path).await.map_err(write_error)?;
    Ok(Downloaded { path, kind, title })
}

fn not_an_image(head: &[u8]) -> Failure {
    match image::sniff(head) {
        Sniffed::Unsupported(name) => MediaForgeError::UnsupportedImage(name).into(),
        _ => MediaForgeError::DownloadFailed("The link didn't lead to an image.".into()).into(),
    }
}

fn write_error(error: std::io::Error) -> Failure {
    if is_out_of_space(&error) { MediaForgeError::InsufficientDiskSpace.into() } else { MediaForgeError::Unknown(error.to_string()).into() }
}

async fn request(url: &ValidatedUrl, cancel: &CancellationToken) -> JobResult<reqwest::Response> {
    let send = client().get(url.as_str()).header(ACCEPT, ACCEPTED).send();
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(Failure::Cancelled),
        result = tokio::time::timeout(RESPONSE_TIMEOUT, send) => match result {
            Err(_) => return Err(MediaForgeError::TimedOut.into()),
            Ok(Err(error)) => return Err(request_error(&error).into()),
            Ok(Ok(response)) => response,
        },
    };
    match status_error(response.status()) {
        Some(error) => Err(error.into()),
        None => Ok(response),
    }
}

async fn next_chunk(response: &mut reqwest::Response, cancel: &CancellationToken) -> JobResult<Option<Vec<u8>>> {
    tokio::select! {
        _ = cancel.cancelled() => Err(Failure::Cancelled),
        result = tokio::time::timeout(IDLE_TIMEOUT, response.chunk()) => match result {
            Err(_) => Err(MediaForgeError::TimedOut.into()),
            Ok(Err(error)) => Err(request_error(&error).into()),
            Ok(Ok(chunk)) => Ok(chunk.map(|c| c.to_vec())),
        },
    }
}

fn request_error(error: &reqwest::Error) -> MediaForgeError {
    if error.is_timeout() {
        MediaForgeError::TimedOut
    } else if error.is_redirect() {
        MediaForgeError::DownloadFailed("The link redirects too many times.".into())
    } else {
        MediaForgeError::Network
    }
}

fn status_error(status: reqwest::StatusCode) -> Option<MediaForgeError> {
    use reqwest::StatusCode;
    if status.is_success() {
        return None;
    }
    Some(match status {
        StatusCode::NOT_FOUND | StatusCode::GONE => MediaForgeError::Unavailable,
        StatusCode::UNAUTHORIZED | StatusCode::PROXY_AUTHENTICATION_REQUIRED => MediaForgeError::AuthenticationRequired,
        StatusCode::FORBIDDEN | StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS => MediaForgeError::AccessDenied,
        StatusCode::TOO_MANY_REQUESTS => MediaForgeError::RateLimited,
        other if other.is_server_error() => {
            MediaForgeError::DownloadFailed(format!("The server had a problem (error {}). Try again later.", other.as_u16()))
        }
        other => MediaForgeError::DownloadFailed(format!("The server refused the request (error {}).", other.as_u16())),
    })
}

fn header(response: &reqwest::Response, name: HeaderName) -> String {
    response.headers().get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

/// A title from the file name the server suggests, or else the last part of the address: "Sunset" for
/// `…/photos/Sunset.webp`.
fn title_for(url: &ValidatedUrl, disposition: &str) -> String {
    let name = disposition_filename(disposition)
        .unwrap_or_else(|| percent_decode(url.path().rsplit('/').find(|part| !part.is_empty()).unwrap_or("")));
    let name = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let stem = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.trim().is_empty() && extension.len() <= 5 => stem.trim(),
        _ => name,
    };
    let title: String = stem.chars().filter(|c| !c.is_control()).take(200).collect();
    if title.trim().is_empty() { url.host().trim_start_matches("www.").to_string() } else { title }
}

/// The `filename` (or RFC 5987 `filename*`) in a Content-Disposition header.
fn disposition_filename(header: &str) -> Option<String> {
    let mut plain = None;
    for part in header.split(';').map(str::trim) {
        let Some((key, value)) = part.split_once('=') else { continue };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                // charset'language'percent-encoded
                let encoded = value.splitn(3, '\'').nth(2)?;
                let decoded = percent_decode(encoded);
                if !decoded.trim().is_empty() {
                    return Some(decoded);
                }
            }
            "filename" => plain = Some(value.trim_matches('"').to_string()).filter(|v| !v.trim().is_empty()),
            _ => {}
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::local_server::LocalServer;
    use crate::support::url_validator::validate;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/images").join(name)).unwrap()
    }

    #[test]
    fn recognises_image_addresses() {
        assert!(looks_like_image(&validate("https://example.com/photos/Sunset.WEBP").unwrap()));
        assert!(looks_like_image(&validate("https://example.com/a/b.jpg?size=large").unwrap()));
        assert!(!looks_like_image(&validate("https://example.com/watch?v=abc.jpg").unwrap()));
        assert!(!looks_like_image(&validate("https://example.com/clip.mp4").unwrap()));
        assert!(!looks_like_image(&validate("https://example.com/images.jpg/page").unwrap()));
    }

    #[test]
    fn titles_come_from_the_file_name() {
        let url = validate("https://www.example.com/photos/Summer%20Fair.webp").unwrap();
        assert_eq!(title_for(&url, ""), "Summer Fair");
        assert_eq!(title_for(&url, "inline; filename=\"Poster.png\""), "Poster");
        assert_eq!(title_for(&url, "attachment; filename*=UTF-8''Caf%C3%A9.jpg"), "Café");
        assert_eq!(title_for(&url, "attachment; filename=\"../../etc/passwd.png\""), "passwd");
        assert_eq!(title_for(&validate("https://www.example.com/").unwrap(), ""), "example.com");
    }

    #[test]
    fn server_answers_map_to_clear_errors() {
        use reqwest::StatusCode;
        assert_eq!(status_error(StatusCode::OK), None);
        assert_eq!(status_error(StatusCode::NOT_FOUND), Some(MediaForgeError::Unavailable));
        assert_eq!(status_error(StatusCode::FORBIDDEN), Some(MediaForgeError::AccessDenied));
        assert_eq!(status_error(StatusCode::UNAUTHORIZED), Some(MediaForgeError::AuthenticationRequired));
        assert_eq!(status_error(StatusCode::TOO_MANY_REQUESTS), Some(MediaForgeError::RateLimited));
        assert!(matches!(status_error(StatusCode::BAD_GATEWAY), Some(MediaForgeError::DownloadFailed(_))));
        assert_eq!(unsupported_type("image/svg+xml; charset=utf-8"), Some("SVG"));
        assert_eq!(unsupported_type("image/webp"), None);
        assert_eq!(unsupported_type("text/html"), None);
    }

    #[tokio::test]
    async fn looks_up_and_downloads_an_image_link() {
        let served = tempfile::tempdir().unwrap();
        std::fs::write(served.path().join("Harbour.webp"), fixture("photo-lossy.webp")).unwrap();
        // A server that doesn't know what WebP is still gets recognised from the contents.
        std::fs::write(served.path().join("picture"), fixture("logo-alpha.webp")).unwrap();
        std::fs::write(served.path().join("page.html"), "<!DOCTYPE html><html><body>Hello there, not a picture</body></html>").unwrap();
        let server = LocalServer::start(served.path(), None).unwrap();
        let cancel = CancellationToken::new();

        let url = validate(&server.url("Harbour.webp")).unwrap();
        let Probe::Image(link) = probe(&url, &cancel).await.unwrap() else { panic!("expected an image") };
        assert_eq!((link.title.as_str(), link.kind, link.width, link.height), ("Harbour", SourceKind::Webp, Some(64), Some(48)));
        assert_eq!(link.size, Some(fixture("photo-lossy.webp").len() as u64));

        let Probe::Image(unnamed) = probe(&validate(&server.url("picture")).unwrap(), &cancel).await.unwrap() else { panic!() };
        assert_eq!((unnamed.kind, unnamed.width), (SourceKind::Webp, Some(40)));
        assert_eq!(probe(&validate(&server.url("page.html")).unwrap(), &cancel).await.unwrap(), Probe::NotImage);
        assert_eq!(probe(&validate(&server.url("missing.png")).unwrap(), &cancel).await, Err(MediaForgeError::Unavailable.into()));

        let work = tempfile::tempdir().unwrap();
        let mut updates = Vec::new();
        let downloaded = download(&url, work.path(), DownloadLimits::default(), &cancel, |t| updates.push(t)).await.unwrap();
        assert_eq!(downloaded.kind, SourceKind::Webp);
        assert_eq!(downloaded.path, work.path().join("image.webp"));
        assert_eq!(std::fs::read(&downloaded.path).unwrap(), fixture("photo-lossy.webp"));
        assert_eq!(updates.last().map(|t| t.received), Some(fixture("photo-lossy.webp").len() as u64));

        let page = download(&validate(&server.url("page.html")).unwrap(), work.path(), DownloadLimits::default(), &cancel, |_| {}).await;
        assert_eq!(page, Err(MediaForgeError::DownloadFailed("The link didn't lead to an image.".into()).into()));
        let limited = DownloadLimits { rate_limit: None, max_file_size: Some(50) };
        assert_eq!(download(&url, work.path(), limited, &cancel, |_| {}).await, Err(MediaForgeError::ExceedsSizeLimit.into()));
    }

    #[tokio::test]
    async fn a_slow_download_can_be_cancelled() {
        let served = tempfile::tempdir().unwrap();
        let mut big = fixture("photo-lossy.webp");
        big.resize(400_000, 0);
        std::fs::write(served.path().join("big.webp"), big).unwrap();
        let server = LocalServer::start(served.path(), Some(50_000)).unwrap();
        let work = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            trigger.cancel();
        });
        let started = Instant::now();
        let url = validate(&server.url("big.webp")).unwrap();
        assert_eq!(download(&url, work.path(), DownloadLimits::default(), &cancel, |_| {}).await, Err(Failure::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
