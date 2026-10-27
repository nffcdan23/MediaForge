//! Artwork for the UI: thumbnails supplied by a media source, frames from video files, cover art embedded in
//! audio files, small copies of images, and waveform peaks for the preview player.
//!
//! Everything is handed to the webview as data, so the webview itself never fetches from the network or
//! reads files.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::media::ffmpeg::{self, PROTOCOL_WHITELIST};
use crate::process::{self, ProcessRequest};

pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const MEMORY_LIMIT: usize = 300;

/// Recognises the image formats thumbnails come in, from their first bytes rather than a server's say-so.
pub fn image_mime_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("image/webp")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

pub fn data_url(data: &[u8]) -> Option<String> {
    let mime = image_mime_type(data)?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(data)))
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Recently used artwork by key, with the order keys were added so the oldest can be dropped.
type MemoryCache = (HashMap<String, Option<String>>, VecDeque<String>);

/// Loads and caches artwork. Remote images are cached on disk by a hash of their address; frames from local
/// files by a hash of the path, size and modification time.
pub struct ArtworkCache {
    directory: PathBuf,
    memory: Mutex<MemoryCache>,
    client: reqwest::Client,
}

impl ArtworkCache {
    pub fn new(directory: PathBuf) -> Self {
        // A private client: no cookies, no shared cache, nothing that identifies the user.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .https_only(false)
            .user_agent("MediaForge")
            .build()
            .unwrap_or_default();
        Self { directory, memory: Mutex::new((HashMap::new(), VecDeque::new())), client }
    }

    fn remember(&self, key: String, value: Option<String>) -> Option<String> {
        let mut guard = self.memory.lock().unwrap();
        let (map, order) = &mut *guard;
        if map.insert(key.clone(), value.clone()).is_none() {
            order.push_back(key);
        }
        while order.len() > MEMORY_LIMIT {
            if let Some(oldest) = order.pop_front() {
                map.remove(&oldest);
            }
        }
        value
    }

    fn recall(&self, key: &str) -> Option<Option<String>> {
        self.memory.lock().unwrap().0.get(key).cloned()
    }

    /// A thumbnail published by a media source, as a data URL.
    pub async fn remote(&self, address: &str) -> Option<String> {
        let parsed = url::Url::parse(address).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return None;
        }
        let key = format!("remote:{address}");
        if let Some(cached) = self.recall(&key) {
            return cached;
        }
        let file = self.directory.join(digest(address));
        if let Ok(data) = std::fs::read(&file)
            && let Some(url) = data_url(&data)
        {
            return self.remember(key, Some(url));
        }
        let data = self.fetch(parsed).await;
        let url = data.as_deref().and_then(data_url);
        if let (Some(data), Some(_)) = (&data, &url)
            && std::fs::create_dir_all(&self.directory).is_ok()
        {
            let _ = std::fs::write(&file, data);
        }
        // Failures aren't remembered, so a thumbnail that was briefly unreachable loads next time.
        if url.is_some() { self.remember(key, url) } else { None }
    }

    async fn fetch(&self, url: url::Url) -> Option<Vec<u8>> {
        let mut response = self.client.get(url).header("Accept", "image/*").send().await.ok()?;
        if response.status() != reqwest::StatusCode::OK {
            return None;
        }
        if response.content_length().is_some_and(|length| length as usize > MAX_IMAGE_BYTES) {
            return None;
        }
        let mut data = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            data.extend_from_slice(&chunk);
            if data.len() > MAX_IMAGE_BYTES {
                return None;
            }
        }
        (!data.is_empty()).then_some(data)
    }

    /// A frame from a video file, an audio file's embedded cover art, or a small copy of an image, as a data
    /// URL. `None` when there is nothing specific to show, so the UI uses its own placeholder.
    pub async fn local(&self, path: &Path, ffmpeg_path: Option<&Path>) -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        if !metadata.is_file() {
            return None;
        }
        let modified =
            metadata.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
        let identity = format!("{}|{}|{}", path.display(), metadata.len(), modified);
        let key = format!("file:{identity}");
        if let Some(cached) = self.recall(&key) {
            return cached;
        }
        let file = self.directory.join(format!("frame-{}", digest(&identity)));
        if let Ok(data) = std::fs::read(&file) {
            return self.remember(key, data_url(&data));
        }
        let image = if crate::engine::media_files::is_image(path) {
            let source = path.to_path_buf();
            tokio::task::spawn_blocking(move || crate::media::image::thumbnail(&source, 480)).await.ok().flatten()
        } else {
            let ffmpeg_path = ffmpeg_path?;
            let is_audio = crate::engine::media_files::is_audio(path);
            let mut frame = extract_frame(ffmpeg_path, path, if is_audio { None } else { Some(1.0) }).await;
            if frame.is_none() && !is_audio {
                // Shorter than a second, or no keyframe there: take the very first frame.
                frame = extract_frame(ffmpeg_path, path, None).await;
            }
            frame
        };
        if let Some(data) = &image
            && std::fs::create_dir_all(&self.directory).is_ok()
        {
            let _ = std::fs::write(&file, data);
        }
        self.remember(key, image.as_deref().and_then(data_url))
    }
}

async fn extract_frame(ffmpeg_path: &Path, input: &Path, seek: Option<f64>) -> Option<Vec<u8>> {
    let mut arguments: Vec<OsString> =
        ["-hide_banner", "-nostdin", "-loglevel", "error", "-protocol_whitelist", PROTOCOL_WHITELIST].iter().map(OsString::from).collect();
    if let Some(seconds) = seek {
        arguments.push("-ss".into());
        arguments.push(seconds.to_string().into());
    }
    arguments.push("-i".into());
    arguments.push(ffmpeg::file_argument(input));
    arguments.extend(
        ["-map", "0:v:0", "-frames:v", "1", "-vf", "scale=480:-2", "-c:v", "mjpeg", "-q:v", "4", "-f", "image2pipe", "pipe:1"]
            .iter()
            .map(OsString::from),
    );
    let mut request = ProcessRequest::new(ffmpeg_path, arguments);
    request.timeout = Some(Duration::from_secs(20));
    request.max_output_bytes = MAX_IMAGE_BYTES;
    request.working_directory = Some(std::env::temp_dir());
    let output = process::run_collect(&request, &CancellationToken::new()).await.ok()?;
    (output.exit.success() && image_mime_type(&output.stdout).is_some()).then_some(output.stdout)
}

/// Reduces an audio file to `buckets` peak levels between 0 and 1, for drawing a scrubber.
///
/// FFmpeg decodes to low-rate mono samples that are read as they arrive, so memory use doesn't depend on how
/// long the file is.
pub async fn waveform(ffmpeg_path: &Path, input: &Path, duration: Option<f64>, buckets: usize) -> Vec<f32> {
    const RATE: f64 = 2000.0;
    let mut arguments: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-protocol_whitelist", PROTOCOL_WHITELIST, "-i"]
        .iter()
        .map(OsString::from)
        .collect();
    arguments.push(ffmpeg::file_argument(input));
    arguments.extend(["-map", "0:a:0", "-ac", "1", "-ar", "2000", "-f", "s16le", "-c:a", "pcm_s16le", "pipe:1"].iter().map(OsString::from));
    let mut request = ProcessRequest::new(ffmpeg_path, arguments);
    request.timeout = Some(Duration::from_secs(120));
    request.max_output_bytes = 512 * 1024 * 1024;
    request.working_directory = Some(std::env::temp_dir());

    // With a known length each sample goes straight into its bucket; otherwise collect fine-grained peaks
    // and fold them down at the end.
    let expected = duration.filter(|d| *d > 0.0).map(|d| (d * RATE) as usize);
    let mut peaks: Vec<f32> = vec![0.0; if expected.is_some() { buckets } else { 0 }];
    let mut fine: Vec<f32> = Vec::new();
    let mut index = 0usize;
    let mut carry: Option<u8> = None;
    let mut accumulate = |bytes: &[u8]| {
        let mut push = |sample: i16| {
            let level = (sample as f32 / 32768.0).abs();
            match expected {
                Some(total) => {
                    let bucket = ((index as f64 / total.max(1) as f64) * buckets as f64) as usize;
                    let slot = bucket.min(buckets - 1);
                    peaks[slot] = peaks[slot].max(level);
                }
                None => {
                    let slot = index / 200;
                    if fine.len() <= slot {
                        fine.push(0.0);
                    }
                    fine[slot] = fine[slot].max(level);
                }
            }
            index += 1;
        };
        let mut start = 0;
        if let Some(low) = carry.take()
            && let Some(&high) = bytes.first()
        {
            push(i16::from_le_bytes([low, high]));
            start = 1;
        }
        let rest = &bytes[start..];
        for pair in rest.chunks(2) {
            if pair.len() == 2 {
                push(i16::from_le_bytes([pair[0], pair[1]]));
            } else {
                carry = Some(pair[0]);
            }
        }
    };
    let succeeded = matches!(
        process::run_bytes(&request, &CancellationToken::new(), &mut accumulate).await,
        Ok(exit) if exit.success()
    );
    if !succeeded {
        return Vec::new();
    }
    if expected.is_none() {
        if fine.is_empty() {
            return Vec::new();
        }
        peaks = (0..buckets)
            .map(|b| {
                let from = b * fine.len() / buckets;
                let to = ((b + 1) * fine.len() / buckets).max(from + 1).min(fine.len());
                fine[from..to].iter().copied().fold(0.0, f32::max)
            })
            .collect();
    }
    let loudest = peaks.iter().copied().fold(0.0, f32::max);
    if loudest > 0.0 { peaks.iter().map(|p| p / loudest).collect() } else { peaks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_images_from_their_contents() {
        assert_eq!(image_mime_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_mime_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_mime_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_mime_type(b"<html>"), None);
        assert!(data_url(b"<svg onload=alert(1)>").is_none());
        assert!(data_url(&[0xFF, 0xD8, 0xFF, 0xDB]).unwrap().starts_with("data:image/jpeg;base64,"));
    }

    #[tokio::test]
    async fn refuses_non_web_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ArtworkCache::new(dir.path().to_path_buf());
        assert_eq!(cache.remote("file:///etc/passwd").await, None);
        assert_eq!(cache.remote("javascript:alert(1)").await, None);
    }
}
