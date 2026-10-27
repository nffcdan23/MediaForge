//! User preferences, stored as JSON in the app's data folder on this computer only.
//!
//! Values are read leniently: a missing, mistyped or out-of-range entry falls back to its default without
//! affecting the others, so a hand-edited or older file never stops the app from starting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::models::{
    Appearance, AudioBitrate, AudioFormat, AudioOptions, BitDepth, DownloadLimits, DownloadMode, ImageFormat, ImageOptions, ImageQuality,
    RateLimitOption, SampleRate, Section, SizeLimitOption, VideoCompatibility, VideoQuality,
};
use crate::tools::Tool;

pub const CONCURRENCY_RANGE: std::ops::RangeInclusive<u32> = 1..=6;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub destination_path: String,
    pub download_mode: DownloadMode,
    pub audio_format: AudioFormat,
    pub audio_bitrate: AudioBitrate,
    pub sample_rate: SampleRate,
    pub bit_depth: BitDepth,
    pub video_quality: VideoQuality,
    pub video_compatibility: VideoCompatibility,
    pub image_format: ImageFormat,
    pub image_quality: ImageQuality,
    pub max_concurrent_jobs: u32,
    pub notifications_enabled: bool,
    pub embed_metadata: bool,
    pub rate_limit: RateLimitOption,
    pub size_limit: SizeLimitOption,
    pub appearance: Appearance,
    pub has_acknowledged_terms: bool,
    /// Explicit tool locations keyed by tool id. Empty means "find it automatically".
    pub tool_paths: BTreeMap<String, String>,
    pub selected_section: Section,
}

impl Settings {
    pub fn defaults(default_destination: &Path) -> Self {
        Self {
            destination_path: default_destination.display().to_string(),
            download_mode: DownloadMode::Audio,
            audio_format: AudioFormat::Mp3,
            audio_bitrate: AudioBitrate::Kbps320,
            sample_rate: SampleRate::Source,
            bit_depth: BitDepth::Pcm16,
            video_quality: VideoQuality::P1080,
            video_compatibility: VideoCompatibility::H264,
            image_format: ImageFormat::Jpg,
            image_quality: ImageQuality::High,
            max_concurrent_jobs: 3,
            notifications_enabled: true,
            embed_metadata: true,
            rate_limit: RateLimitOption::Unlimited,
            size_limit: SizeLimitOption::NoLimit,
            appearance: Appearance::Dark,
            has_acknowledged_terms: false,
            tool_paths: BTreeMap::new(),
            selected_section: Section::Downloads,
        }
    }

    /// Builds settings from any JSON object, keeping each recognisable value and defaulting the rest.
    pub fn from_value(value: &Value, default_destination: &Path) -> Self {
        let mut settings = Self::defaults(default_destination);
        let Some(object) = value.as_object() else { return settings };
        settings.apply_object(object);
        settings
    }

    /// Applies whichever recognisable values `object` contains. Used for loading and for changes from the UI.
    pub fn apply_object(&mut self, object: &Map<String, Value>) {
        fn pick<T: serde::de::DeserializeOwned>(object: &Map<String, Value>, key: &str) -> Option<T> {
            let value = object.get(key)?;
            serde_json::from_value(value.clone())
                .ok()
                // Numbers saved as text (by hand, or by older builds) still count.
                .or_else(|| value.as_str().and_then(|text| serde_json::from_str(text.trim()).ok()))
        }
        if let Some(path) = pick::<String>(object, "destinationPath").filter(|p| !p.trim().is_empty()) {
            self.destination_path = path;
        }
        if let Some(v) = pick(object, "downloadMode") {
            self.download_mode = v;
        }
        if let Some(v) = pick::<String>(object, "audioFormat").and_then(|f| AudioFormat::parse(&f)) {
            self.audio_format = v;
        }
        if let Some(v) = pick(object, "audioBitrate") {
            self.audio_bitrate = v;
        }
        if let Some(v) = pick(object, "sampleRate") {
            self.sample_rate = v;
        }
        if let Some(v) = pick(object, "bitDepth") {
            self.bit_depth = v;
        }
        if let Some(v) = pick(object, "videoQuality") {
            self.video_quality = v;
        }
        if let Some(v) = pick(object, "videoCompatibility") {
            self.video_compatibility = v;
        }
        if let Some(v) = pick::<String>(object, "imageFormat").and_then(|f| ImageFormat::parse(&f)) {
            self.image_format = v;
        }
        if let Some(v) = pick(object, "imageQuality") {
            self.image_quality = v;
        }
        if let Some(v) = pick::<i64>(object, "maxConcurrentJobs") {
            self.max_concurrent_jobs = v.clamp(*CONCURRENCY_RANGE.start() as i64, *CONCURRENCY_RANGE.end() as i64) as u32;
        }
        if let Some(v) = pick_bool(object, "notificationsEnabled") {
            self.notifications_enabled = v;
        }
        if let Some(v) = pick_bool(object, "embedMetadata") {
            self.embed_metadata = v;
        }
        if let Some(v) = pick(object, "rateLimit") {
            self.rate_limit = v;
        }
        if let Some(v) = pick(object, "sizeLimit") {
            self.size_limit = v;
        }
        if let Some(v) = pick(object, "appearance") {
            self.appearance = v;
        }
        if let Some(v) = pick_bool(object, "hasAcknowledgedTerms") {
            self.has_acknowledged_terms = v;
        }
        if let Some(paths) = object.get("toolPaths").and_then(Value::as_object) {
            self.tool_paths = paths
                .iter()
                .filter(|(k, _)| Tool::parse(k).is_some())
                .filter_map(|(k, v)| v.as_str().filter(|p| !p.trim().is_empty()).map(|p| (k.clone(), p.to_string())))
                .collect();
        }
        if let Some(v) = pick(object, "selectedSection") {
            self.selected_section = v;
        }
    }

    pub fn destination(&self, default_destination: &Path) -> PathBuf {
        let expanded = expand_home(&self.destination_path);
        if expanded.is_absolute() { expanded } else { default_destination.to_path_buf() }
    }

    pub fn audio_options(&self) -> AudioOptions {
        AudioOptions { format: self.audio_format, bitrate: self.audio_bitrate, sample_rate: self.sample_rate, bit_depth: self.bit_depth }
    }

    pub fn image_options(&self) -> ImageOptions {
        ImageOptions { format: self.image_format, quality: self.image_quality }
    }

    pub fn download_limits(&self) -> DownloadLimits {
        DownloadLimits { rate_limit: self.rate_limit.bytes_per_second(), max_file_size: self.size_limit.bytes() }
    }

    pub fn tool_overrides(&self) -> BTreeMap<Tool, String> {
        self.tool_paths.iter().filter_map(|(k, v)| Some((Tool::parse(k)?, v.clone()))).filter(|(_, v)| !v.is_empty()).collect()
    }
}

fn pick_bool(object: &Map<String, Value>, key: &str) -> Option<bool> {
    match object.get(key)? {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => Some(number.as_f64().unwrap_or(0.0) != 0.0),
        Value::String(text) => match text.trim().to_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn expand_home(path: &str) -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if path == "~" => PathBuf::from(std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default()),
        _ => PathBuf::from(path),
    }
}

/// Loads and saves `settings.json`.
pub struct SettingsStore {
    path: PathBuf,
    default_destination: PathBuf,
    settings: Settings,
}

impl SettingsStore {
    /// Reads the saved settings. `seed` supplies values for a first launch (settings carried over from the
    /// Swift app) and is ignored once a settings file exists.
    pub fn load(path: PathBuf, default_destination: PathBuf, seed: impl FnOnce() -> Option<Map<String, Value>>) -> Self {
        let settings = match std::fs::read(&path) {
            Ok(data) => match serde_json::from_slice::<Value>(&data) {
                Ok(value) => Settings::from_value(&value, &default_destination),
                Err(_) => {
                    // Keep the unreadable file for diagnosis instead of silently overwriting it.
                    let _ = std::fs::rename(&path, path.with_extension("unreadable.json"));
                    Settings::defaults(&default_destination)
                }
            },
            Err(_) => {
                let mut settings = Settings::defaults(&default_destination);
                if let Some(seed) = seed() {
                    settings.apply_object(&seed);
                }
                settings
            }
        };
        let store = Self { path, default_destination, settings };
        store.save();
        store
    }

    pub fn get(&self) -> &Settings {
        &self.settings
    }

    pub fn default_destination(&self) -> &Path {
        &self.default_destination
    }

    pub fn destination(&self) -> PathBuf {
        self.settings.destination(&self.default_destination)
    }

    /// Applies a partial change from the UI and saves it.
    pub fn update(&mut self, change: &Map<String, Value>) -> &Settings {
        self.settings.apply_object(change);
        self.save();
        &self.settings
    }

    pub fn modify(&mut self, change: impl FnOnce(&mut Settings)) -> &Settings {
        change(&mut self.settings);
        self.save();
        &self.settings
    }

    fn save(&self) {
        let Some(parent) = self.path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(data) = serde_json::to_vec_pretty(&self.settings) else { return };
        // Write then rename, so a crash mid-write can't leave a half-written file.
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn has_sensible_defaults_and_persists_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let destination = dir.path().join("Out");
        let mut store = SettingsStore::load(path.clone(), destination.clone(), || None);
        let defaults = store.get().clone();
        assert_eq!(defaults.audio_format, AudioFormat::Mp3);
        assert_eq!(defaults.audio_bitrate, AudioBitrate::Kbps320);
        assert_eq!(defaults.video_quality, VideoQuality::P1080);
        assert_eq!(defaults.video_compatibility, VideoCompatibility::H264);
        assert_eq!(defaults.image_options(), ImageOptions { format: ImageFormat::Jpg, quality: ImageQuality::High });
        assert_eq!(defaults.max_concurrent_jobs, 3);
        assert_eq!(defaults.appearance, Appearance::Dark);
        assert!(!defaults.has_acknowledged_terms);
        assert_eq!(store.destination(), destination);

        store.update(
            json!({"audioFormat": "flac", "bitDepth": 24, "maxConcurrentJobs": 5, "appearance": "system", "imageFormat": "png", "imageQuality": 80})
                .as_object()
                .unwrap(),
        );
        let reloaded = SettingsStore::load(path, destination, || None);
        assert_eq!(reloaded.get().audio_format, AudioFormat::Flac);
        assert_eq!(reloaded.get().bit_depth, BitDepth::Pcm24);
        assert_eq!(reloaded.get().max_concurrent_jobs, 5);
        assert_eq!(reloaded.get().appearance, Appearance::System);
        assert_eq!(reloaded.get().image_options(), ImageOptions { format: ImageFormat::Png, quality: ImageQuality::Medium });
    }

    #[test]
    fn out_of_range_and_mistyped_values_fall_back_safely() {
        let destination = PathBuf::from("/fallback");
        let settings = Settings::from_value(
            &json!({
                "audioFormat": "wma", "audioBitrate": 999, "sampleRate": "48000", "maxConcurrentJobs": 99,
                "notificationsEnabled": "no", "videoQuality": "1440", "rateLimit": -1, "destinationPath": "",
                "imageFormat": "webp", "imageQuality": 100,
                "toolPaths": {"yt-dlp": "/opt/x/yt-dlp", "evil": "/bin/sh", "ffmpeg": ""}
            }),
            &destination,
        );
        assert_eq!(settings.audio_format, AudioFormat::Mp3);
        assert_eq!(settings.audio_bitrate, AudioBitrate::Kbps320);
        assert_eq!(settings.sample_rate, SampleRate::Hz48000);
        assert_eq!(settings.max_concurrent_jobs, 6);
        assert!(!settings.notifications_enabled);
        assert_eq!(settings.video_quality, VideoQuality::P1440);
        assert_eq!(settings.rate_limit, RateLimitOption::Unlimited);
        assert_eq!(settings.image_options(), ImageOptions::default());
        assert_eq!(settings.destination_path, "/fallback");
        assert_eq!(settings.tool_paths.len(), 1);
        assert_eq!(settings.tool_overrides().get(&Tool::YtDlp).map(String::as_str), Some("/opt/x/yt-dlp"));
    }

    #[test]
    fn a_corrupt_file_is_set_aside_and_a_seed_only_applies_on_first_launch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let seed = || Some(json!({"audioFormat": "wav"}).as_object().unwrap().clone());
        let first = SettingsStore::load(path.clone(), dir.path().into(), seed);
        assert_eq!(first.get().audio_format, AudioFormat::Wav);
        let second =
            SettingsStore::load(path.clone(), dir.path().into(), || Some(json!({"audioFormat": "aiff"}).as_object().unwrap().clone()));
        assert_eq!(second.get().audio_format, AudioFormat::Wav);

        std::fs::write(&path, "{ not json").unwrap();
        let recovered = SettingsStore::load(path.clone(), dir.path().into(), || None);
        assert_eq!(recovered.get().audio_format, AudioFormat::Mp3);
        assert!(dir.path().join("settings.unreadable.json").exists());
    }
}
