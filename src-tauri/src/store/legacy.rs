//! Carries settings and history over from the earlier, macOS-only MediaForge (the Swift app) the first time
//! this version runs on a Mac that has it. Nothing is changed or removed in the old app's files.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::engine::job::JobKind;
use crate::models::{AudioOptions, OutputSpec, VideoOptions};
use crate::store::history::{HistoryEntry, Outcome};

/// The Swift app's identifiers: its current one, then the placeholder used by the earliest builds.
pub const LEGACY_DOMAINS: &[&str] = &["uk.co.c-2.MediaForge", "com.mediaforge.MediaForge"];

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Preferences saved by the Swift app, as a JSON object of the keys this version understands.
pub fn settings(home: &Path) -> Option<Map<String, Value>> {
    let preferences = home.join("Library").join("Preferences");
    let mut merged = Map::new();
    // The newer identifier wins; the placeholder only fills gaps, as the Swift app itself did.
    for domain in LEGACY_DOMAINS {
        let Some(values) = read_plist(&preferences.join(format!("{domain}.plist"))) else { continue };
        for (key, value) in values {
            merged.entry(key).or_insert(value);
        }
    }
    (!merged.is_empty()).then_some(merged)
}

pub fn read_plist(path: &Path) -> Option<Map<String, Value>> {
    #[cfg(target_os = "macos")]
    {
        let value: plist::Value = plist::Value::from_file(path).ok()?;
        let dictionary = value.into_dictionary()?;
        let mut object = Map::new();
        for (key, value) in dictionary {
            if let Some(json) = to_json(value) {
                object.insert(key, json);
            }
        }
        Some(object)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        None
    }
}

#[cfg(target_os = "macos")]
fn to_json(value: plist::Value) -> Option<Value> {
    Some(match value {
        plist::Value::String(text) => Value::String(text),
        plist::Value::Boolean(flag) => Value::Bool(flag),
        plist::Value::Integer(number) => number.as_signed().map(Value::from).or_else(|| number.as_unsigned().map(Value::from))?,
        plist::Value::Real(number) => serde_json::Number::from_f64(number).map(Value::Number)?,
        plist::Value::Dictionary(dictionary) => Value::Object(dictionary.into_iter().filter_map(|(k, v)| Some((k, to_json(v)?))).collect()),
        plist::Value::Array(items) => Value::Array(items.into_iter().filter_map(to_json).collect()),
        _ => return None,
    })
}

/// The Swift app's history file.
pub fn history_path(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support").join("MediaForge").join("History.json")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwiftEntry {
    id: Uuid,
    date: DateTime<Utc>,
    kind: JobKind,
    title: String,
    detail: Option<String>,
    #[serde(rename = "sourceURL")]
    source_url: Option<String>,
    source_file_path: Option<String>,
    output: SwiftOutput,
    output_path: Option<String>,
    file_size: Option<u64>,
    duration: Option<f64>,
    #[serde(rename = "thumbnailURL")]
    thumbnail_url: Option<String>,
    outcome: Outcome,
    error_title: Option<String>,
    error_message: Option<String>,
}

/// Swift encodes `case audio(AudioOptions)` as `{"audio": {"_0": {…}}}`.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum SwiftOutput {
    Audio {
        #[serde(rename = "_0")]
        options: AudioOptions,
    },
    Video {
        #[serde(rename = "_0")]
        options: VideoOptions,
    },
}

/// History entries from the Swift app's file. Entries this version can't read are skipped.
pub fn history(path: &Path) -> Vec<HistoryEntry> {
    let Ok(data) = std::fs::read(path) else { return Vec::new() };
    let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&data) else { return Vec::new() };
    items
        .into_iter()
        .filter_map(|item| serde_json::from_value::<SwiftEntry>(item).ok())
        .map(|entry| HistoryEntry {
            id: entry.id,
            date: entry.date,
            kind: entry.kind,
            title: entry.title,
            detail: entry.detail,
            source_url: entry.source_url,
            source_file_path: entry.source_file_path,
            output: match entry.output {
                SwiftOutput::Audio { options } => OutputSpec::Audio(options),
                SwiftOutput::Video { options } => OutputSpec::Video(options),
            },
            output_path: entry.output_path,
            file_size: entry.file_size,
            duration: entry.duration,
            thumbnail_url: entry.thumbnail_url,
            outcome: entry.outcome,
            error_title: entry.error_title,
            error_message: entry.error_message,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AudioFormat, VideoQuality};

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join(name)
    }

    #[test]
    fn reads_history_written_by_the_swift_app() {
        let entries = history(&fixture("swift-history.json"));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "Clip");
        assert_eq!(entries[0].source_url.as_deref(), Some("https://example.com/x"));
        assert_eq!(entries[0].thumbnail_url.as_deref(), Some("https://example.com/t.jpg"));
        assert!(matches!(entries[0].output, OutputSpec::Audio(AudioOptions { format: AudioFormat::Mp3, .. })));
        assert!(matches!(entries[1].output, OutputSpec::Video(VideoOptions { quality: VideoQuality::P1080 })));
        assert_eq!(entries[1].outcome, Outcome::Failed);
        assert_eq!(entries[1].error_title.as_deref(), Some("Network problem"));
        assert!(history(Path::new("/nonexistent/History.json")).is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn carries_preferences_over_from_the_swift_app() {
        let home = tempfile::tempdir().unwrap();
        let preferences = home.path().join("Library").join("Preferences");
        std::fs::create_dir_all(&preferences).unwrap();
        std::fs::copy(fixture("swift-preferences.plist"), preferences.join("uk.co.c-2.MediaForge.plist")).unwrap();
        let values = settings(home.path()).unwrap();
        let settings = crate::store::settings::Settings::from_value(&Value::Object(values), Path::new("/default"));
        assert_eq!(settings.destination_path, "/Users/example/Music/Out");
        assert_eq!(settings.audio_format, AudioFormat::Flac);
        assert_eq!(settings.sample_rate, crate::models::SampleRate::Hz48000);
        assert_eq!(settings.video_quality, VideoQuality::P2160);
        assert_eq!(settings.video_compatibility, crate::models::VideoCompatibility::Hevc);
        assert_eq!(settings.max_concurrent_jobs, 4);
        assert!(!settings.notifications_enabled);
        assert!(settings.has_acknowledged_terms);
        assert_eq!(settings.selected_section, crate::models::Section::History);
        assert_eq!(settings.tool_paths.get("yt-dlp").map(String::as_str), Some("/opt/homebrew/bin/yt-dlp"));
        assert!(super::settings(&home.path().join("nobody")).is_none());
    }
}
