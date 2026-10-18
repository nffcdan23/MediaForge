//! Finished jobs, persisted as JSON in the app's data folder. Nothing here ever leaves the computer.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::engine::job::{Job, JobKind, JobSource, JobState};
use crate::models::OutputSpec;

pub const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Completed,
    Failed,
}

/// A finished job, as persisted between launches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: Uuid,
    pub date: DateTime<Utc>,
    pub kind: JobKind,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file_path: Option<String>,
    pub output: OutputSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl HistoryEntry {
    /// The record of a finished job, or `None` for one that was cancelled.
    pub fn from_job(job: &Job, date: DateTime<Utc>) -> Option<Self> {
        let (outcome, failure) = match &job.state {
            JobState::Completed => (Outcome::Completed, None),
            JobState::Failed(error) => (Outcome::Failed, Some(error)),
            _ => return None,
        };
        let (source_url, source_file_path) = match &job.source {
            JobSource::Remote(url) => (Some(url.as_str().to_string()), None),
            JobSource::File(path) => (None, Some(path.display().to_string())),
        };
        Some(Self {
            id: Uuid::new_v4(),
            date,
            kind: job.kind(),
            title: job.title.clone(),
            detail: job.detail.clone(),
            source_url,
            source_file_path,
            output: job.output,
            output_path: job.output_path.as_ref().map(|p| p.display().to_string()),
            file_size: job.output_size,
            duration: job.duration,
            thumbnail_url: job.thumbnail_url.clone(),
            outcome,
            error_title: failure.map(|e| e.title()),
            error_message: failure.map(|e| e.message()),
        })
    }
}

/// History entry plus what the UI needs to know about the file right now.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    #[serde(flatten)]
    pub entry: HistoryEntry,
    pub file_exists: bool,
}

pub struct HistoryStore {
    path: PathBuf,
    /// Newest first.
    entries: Vec<HistoryEntry>,
}

impl HistoryStore {
    /// Reads saved history. `seed` supplies entries for a first launch (carried over from the Swift app) and
    /// is ignored once a history file exists.
    pub fn load(path: PathBuf, seed: impl FnOnce() -> Vec<HistoryEntry>) -> Self {
        let mut entries = Vec::new();
        let mut needs_save = false;
        match std::fs::read(&path) {
            Ok(data) => match serde_json::from_slice::<Vec<HistoryEntry>>(&data) {
                Ok(decoded) => entries = decoded,
                Err(_) => {
                    // Keep the unreadable file for diagnosis instead of silently overwriting it.
                    let backup = path.with_extension("unreadable.json");
                    let _ = std::fs::remove_file(&backup);
                    let _ = std::fs::rename(&path, &backup);
                }
            },
            Err(_) => {
                entries = seed();
                needs_save = !entries.is_empty();
            }
        }
        entries.truncate(MAX_ENTRIES);
        let store = Self { path, entries };
        if needs_save {
            store.save();
        }
        store
    }

    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    pub fn items(&self) -> Vec<HistoryItem> {
        self.entries
            .iter()
            .map(|entry| HistoryItem {
                file_exists: entry.output_path.as_deref().is_some_and(|p| Path::new(p).is_file()),
                entry: entry.clone(),
            })
            .collect()
    }

    pub fn find(&self, id: Uuid) -> Option<&HistoryEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn record(&mut self, entry: HistoryEntry) {
        self.entries.insert(0, entry);
        self.entries.truncate(MAX_ENTRIES);
        self.save();
    }

    pub fn remove(&mut self, ids: &[Uuid]) {
        if ids.is_empty() {
            return;
        }
        self.entries.retain(|e| !ids.contains(&e.id));
        self.save();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }

    fn save(&self) {
        // History is a convenience; failing to persist it must never interrupt a download.
        let Some(parent) = self.path.parent() else { return };
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
        let Ok(data) = serde_json::to_vec_pretty(&self.entries) else { return };
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, data).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MediaForgeError;
    use crate::models::AudioOptions;

    fn finished(title: &str, state: JobState) -> Job {
        let mut job = Job::new(JobSource::File(PathBuf::from("/music/in.wav")), OutputSpec::Audio(AudioOptions::default()), title);
        job.state = state;
        job
    }

    #[test]
    fn survives_a_relaunch_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History.json");
        let mut store = HistoryStore::load(path.clone(), Vec::new);
        store.record(HistoryEntry::from_job(&finished("First", JobState::Completed), Utc::now()).unwrap());
        store.record(HistoryEntry::from_job(&finished("Second", JobState::Failed(MediaForgeError::NoAudioStream)), Utc::now()).unwrap());
        assert!(HistoryEntry::from_job(&finished("Cancelled", JobState::Cancelled), Utc::now()).is_none());

        let reloaded = HistoryStore::load(path, Vec::new);
        let titles: Vec<&str> = reloaded.entries().iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Second", "First"]);
        assert_eq!(reloaded.entries()[0].outcome, Outcome::Failed);
        assert_eq!(reloaded.entries()[0].error_title.as_deref(), Some("No audio track"));
        assert_eq!(reloaded.entries()[1].source_file_path.as_deref(), Some(PathBuf::from("/music/in.wav").display().to_string().as_str()));
    }

    #[test]
    fn is_capped_and_a_corrupt_file_is_set_aside_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("History.json");
        let mut store = HistoryStore::load(path.clone(), Vec::new);
        for index in 0..(MAX_ENTRIES + 5) {
            store.record(HistoryEntry::from_job(&finished(&format!("{index}"), JobState::Completed), Utc::now()).unwrap());
        }
        assert_eq!(store.entries().len(), MAX_ENTRIES);
        assert_eq!(store.entries()[0].title, format!("{}", MAX_ENTRIES + 4));

        std::fs::write(&path, "{ broken").unwrap();
        let recovered = HistoryStore::load(path.clone(), Vec::new);
        assert!(recovered.entries().is_empty());
        assert!(dir.path().join("History.unreadable.json").exists());
    }

    #[test]
    fn reports_whether_each_file_still_exists() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.mp3");
        std::fs::write(&output, "x").unwrap();
        let mut job = finished("Here", JobState::Completed);
        job.output_path = Some(output.clone());
        let mut store = HistoryStore::load(dir.path().join("History.json"), Vec::new);
        store.record(HistoryEntry::from_job(&job, Utc::now()).unwrap());
        assert!(store.items()[0].file_exists);
        std::fs::remove_file(&output).unwrap();
        assert!(!store.items()[0].file_exists);
        let id = store.entries()[0].id;
        store.remove(&[id]);
        assert!(store.entries().is_empty());
    }
}
