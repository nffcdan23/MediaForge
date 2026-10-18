//! Details about a remote item, decoded from `yt-dlp --dump-single-json` or a `.info.json` sidecar.
//!
//! yt-dlp's JSON is loosely typed: numbers arrive as integers, floats or strings depending on the extractor,
//! so every field is read leniently and a missing or odd value never fails the whole document.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::error::MediaForgeError;

/// Hard ceiling on metadata we're willing to parse. Real documents are a few hundred KB.
pub const MAX_JSON_BYTES: usize = 64 * 1024 * 1024;

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mov", "webm", "mkv", "flv", "avi", "ts", "3gp"];

/// One downloadable rendition as described by yt-dlp.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaFormat {
    pub id: String,
    pub ext: Option<String>,
    pub note: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    /// Raw yt-dlp value: a codec string, `"none"`, or `None` when unknown.
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_bitrate: Option<f64>,
    pub total_bitrate: Option<f64>,
    pub file_size: Option<u64>,
    pub has_drm: bool,
}

impl MediaFormat {
    pub fn has_video(&self) -> bool {
        if let Some(codec) = &self.video_codec {
            return codec != "none";
        }
        if self.height.is_some() {
            return true;
        }
        self.ext.as_deref().is_some_and(|ext| VIDEO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
    }

    pub fn has_audio(&self) -> bool {
        self.audio_codec.as_deref().map(|c| c != "none").unwrap_or(true)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub id: String,
    pub title: String,
    pub url: String,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MediaMetadata {
    pub id: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub webpage_url: Option<String>,
    pub extractor: Option<String>,
    pub is_live: bool,
    pub formats: Vec<MediaFormat>,
    /// The renditions yt-dlp actually chose. Only present in a download's `.info.json`.
    pub requested_formats: Vec<MediaFormat>,
    /// Non-empty when the link resolved to a collection rather than a single item.
    pub playlist_entries: Vec<PlaylistEntry>,
    pub is_playlist: bool,
    pub playlist_count: Option<u64>,
}

impl MediaMetadata {
    pub fn has_video(&self) -> bool {
        self.formats.is_empty() || self.formats.iter().any(MediaFormat::has_video)
    }

    pub fn max_video_height(&self) -> Option<u32> {
        self.formats.iter().filter(|f| f.has_video()).filter_map(|f| f.height).max()
    }

    pub fn is_entirely_drm(&self) -> bool {
        !self.formats.is_empty() && self.formats.iter().all(|f| f.has_drm)
    }

    pub fn source_name(&self) -> Option<String> {
        let extractor = self.extractor.as_deref().filter(|e| !e.is_empty())?;
        if extractor.eq_ignore_ascii_case("generic") {
            return Some("Direct link".into());
        }
        let first = extractor.split(':').next().unwrap_or(extractor);
        let mut chars = first.chars();
        Some(match chars.next() {
            Some(c) => c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
            None => extractor.to_string(),
        })
    }

    pub fn decode(data: &[u8]) -> Result<Self, MediaForgeError> {
        if data.len() > MAX_JSON_BYTES {
            return Err(MediaForgeError::InvalidMetadata);
        }
        let value: Value = serde_json::from_slice(data).map_err(|_| MediaForgeError::InvalidMetadata)?;
        if !value.is_object() {
            return Err(MediaForgeError::InvalidMetadata);
        }
        Ok(Self::from_value(&value))
    }

    /// Reads a `.info.json` sidecar.
    pub fn load(path: &Path) -> Result<Self, MediaForgeError> {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if size as usize > MAX_JSON_BYTES {
            return Err(MediaForgeError::InvalidMetadata);
        }
        let data = std::fs::read(path).map_err(|_| MediaForgeError::InvalidMetadata)?;
        Self::decode(&data)
    }

    fn from_value(raw: &Value) -> Self {
        let kind = lenient_string(raw, "_type");
        let is_playlist = matches!(kind.as_deref(), Some("playlist") | Some("multi_video"));
        let title = ["title", "fulltitle", "id"]
            .iter()
            .filter_map(|key| lenient_string(raw, key))
            .map(|t| t.trim().to_string())
            .find(|t| !t.is_empty())
            .unwrap_or_else(|| "Untitled".into());
        let uploader = ["uploader", "channel", "artist", "creator"]
            .iter()
            .filter_map(|key| lenient_string(raw, key))
            .map(|t| t.trim().to_string())
            .find(|t| !t.is_empty());
        let live_status = lenient_string(raw, "live_status");
        let formats = raw.get("formats").and_then(Value::as_array).map(|a| a.iter().map(format_from).collect()).unwrap_or_default();
        let requested_formats =
            raw.get("requested_formats").and_then(Value::as_array).map(|a| a.iter().map(format_from).collect()).unwrap_or_default();
        let playlist_entries = raw
            .get("entries")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        let url =
                            lenient_string(entry, "webpage_url").or_else(|| lenient_string(entry, "url")).filter(|u| !u.is_empty())?;
                        let title = lenient_string(entry, "title").map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                        Some(PlaylistEntry {
                            id: format!("{index}-{}", lenient_string(entry, "id").unwrap_or_else(|| url.clone())),
                            title: title.unwrap_or_else(|| format!("Item {}", index + 1)),
                            url,
                            duration: lenient_f64(entry, "duration").filter(|d| *d > 0.0),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            id: lenient_string(raw, "id").unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            title,
            uploader,
            duration: lenient_f64(raw, "duration").filter(|d| *d > 0.0),
            thumbnail_url: best_thumbnail(raw),
            webpage_url: lenient_string(raw, "webpage_url").or_else(|| lenient_string(raw, "original_url")),
            extractor: lenient_string(raw, "extractor_key").or_else(|| lenient_string(raw, "extractor")),
            is_live: lenient_bool(raw, "is_live") == Some(true) || matches!(live_status.as_deref(), Some("is_live") | Some("is_upcoming")),
            formats,
            requested_formats,
            playlist_entries,
            is_playlist,
            playlist_count: lenient_f64(raw, "playlist_count").map(|c| c.max(0.0) as u64),
        }
    }
}

fn format_from(raw: &Value) -> MediaFormat {
    let dimension = |key| lenient_f64(raw, key).filter(|v| *v > 0.0 && *v < 100_000.0).map(|v| v as u32);
    MediaFormat {
        id: lenient_string(raw, "format_id").unwrap_or_else(|| "unknown".into()),
        ext: lenient_string(raw, "ext"),
        note: lenient_string(raw, "format_note"),
        width: dimension("width"),
        height: dimension("height"),
        fps: lenient_f64(raw, "fps"),
        video_codec: lenient_string(raw, "vcodec"),
        audio_codec: lenient_string(raw, "acodec"),
        audio_bitrate: lenient_f64(raw, "abr"),
        total_bitrate: lenient_f64(raw, "tbr"),
        file_size: lenient_f64(raw, "filesize")
            .or_else(|| lenient_f64(raw, "filesize_approx"))
            .filter(|s| *s > 0.0)
            .map(|s| s.min(9.0e15) as u64),
        has_drm: lenient_bool(raw, "has_drm").unwrap_or(false),
    }
}

fn best_thumbnail(raw: &Value) -> Option<String> {
    fn web_url(text: Option<String>) -> Option<String> {
        let text = text?;
        let parsed = url::Url::parse(&text).ok()?;
        (matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some()).then_some(text)
    }
    if let Some(url) = web_url(lenient_string(raw, "thumbnail")) {
        return Some(url);
    }
    let mut candidates: Vec<&Value> = raw.get("thumbnails").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    candidates.sort_by(|a, b| {
        let key = |v: &Value| (lenient_f64(v, "preference").unwrap_or(0.0), lenient_f64(v, "width").unwrap_or(0.0));
        key(b).partial_cmp(&key(a)).unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.into_iter().find_map(|c| web_url(lenient_string(c, "url")))
}

pub(crate) fn lenient_string(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

pub(crate) fn lenient_f64(value: &Value, key: &str) -> Option<f64> {
    let number = match value.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    number.is_finite().then_some(number)
}

fn lenient_bool(value: &Value, key: &str) -> Option<bool> {
    match value.get(key)? {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) => Some(!text.is_empty() && text != "false"),
        Value::Number(number) => Some(number.as_f64().unwrap_or(0.0) != 0.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_typical_video_document_tolerating_loosely_typed_numbers() {
        let json = br#"{
            "id": "abc", "title": "  Spring Campaign  ", "uploader": "C2", "duration": "187.4",
            "thumbnail": "https://img.example.com/a.jpg", "extractor_key": "Youtube", "webpage_url": "https://example.com/w",
            "formats": [
                {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "abr": 129.5, "filesize": 3000000},
                {"format_id": 137, "ext": "mp4", "vcodec": "avc1.640028", "acodec": "none", "height": "1080", "width": 1920, "fps": 30, "filesize_approx": "50000000"},
                {"format_id": "313", "ext": "webm", "vcodec": "vp9", "acodec": "none", "height": 2160.0}
            ]
        }"#;
        let metadata = MediaMetadata::decode(json).unwrap();
        assert_eq!(metadata.title, "Spring Campaign");
        assert_eq!(metadata.uploader.as_deref(), Some("C2"));
        assert_eq!(metadata.duration, Some(187.4));
        assert_eq!(metadata.formats.len(), 3);
        assert_eq!(metadata.formats[1].id, "137");
        assert_eq!(metadata.formats[1].file_size, Some(50_000_000));
        assert_eq!(metadata.max_video_height(), Some(2160));
        assert!(metadata.has_video());
        assert_eq!(metadata.source_name().as_deref(), Some("Youtube"));
    }

    #[test]
    fn recognises_audio_only_sources_live_streams_and_drm() {
        let audio =
            MediaMetadata::decode(br#"{"title": "Sting", "formats": [{"format_id": "a", "vcodec": "none", "acodec": "opus"}]}"#).unwrap();
        assert!(!audio.has_video());
        assert_eq!(audio.max_video_height(), None);
        let live = MediaMetadata::decode(br#"{"title": "Now", "live_status": "is_live"}"#).unwrap();
        assert!(live.is_live);
        let drm = MediaMetadata::decode(
            br#"{"title": "Film", "formats": [{"format_id": "a", "has_drm": true}, {"format_id": "b", "has_drm": "yes"}]}"#,
        )
        .unwrap();
        assert!(drm.is_entirely_drm());
        let generic = MediaMetadata::decode(br#"{"title": "clip", "extractor": "generic"}"#).unwrap();
        assert_eq!(generic.source_name().as_deref(), Some("Direct link"));
    }

    #[test]
    fn decodes_playlists_and_ignores_entries_without_a_link() {
        let json = br#"{"_type": "playlist", "title": "Feed", "playlist_count": 3, "entries": [
            {"id": "1", "title": "One", "url": "https://example.com/1", "duration": 60},
            {"id": "2", "title": "", "webpage_url": "https://example.com/2"},
            {"id": "3", "title": "No link"}
        ]}"#;
        let metadata = MediaMetadata::decode(json).unwrap();
        assert!(metadata.is_playlist);
        assert_eq!(metadata.playlist_entries.len(), 2);
        assert_eq!(metadata.playlist_entries[0].duration, Some(60.0));
        assert_eq!(metadata.playlist_entries[1].title, "Item 2");
    }

    #[test]
    fn falls_back_gracefully_on_sparse_or_hostile_documents() {
        let sparse = MediaMetadata::decode(br#"{"formats": "nope", "duration": -5, "thumbnail": "javascript:alert(1)", "thumbnails": [{"url": "file:///etc/passwd"}, {"url": "https://ok.example/t.jpg", "preference": 1}]}"#).unwrap();
        assert_eq!(sparse.title, "Untitled");
        assert_eq!(sparse.duration, None);
        assert!(sparse.formats.is_empty());
        assert_eq!(sparse.thumbnail_url.as_deref(), Some("https://ok.example/t.jpg"));
        assert!(MediaMetadata::decode(b"[]").is_err());
        assert!(MediaMetadata::decode(b"not json").is_err());
    }
}
