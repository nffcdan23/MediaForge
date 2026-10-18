//! A single download or conversion, and how it is described to the UI.

use std::collections::VecDeque;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::error::{ErrorPayload, MediaForgeError};
use crate::media::metadata::MediaMetadata;
use crate::models::OutputSpec;
use crate::support::url_validator::ValidatedUrl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    Download,
    Conversion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobSource {
    Remote(ValidatedUrl),
    File(PathBuf),
}

impl JobSource {
    pub fn kind(&self) -> JobKind {
        match self {
            Self::Remote(_) => JobKind::Download,
            Self::File(_) => JobKind::Conversion,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    Queued,
    /// Resolving the source before any bytes move.
    Preparing,
    Downloading,
    /// yt-dlp post-processing (merging streams, remuxing) with no measurable progress.
    Processing(String),
    Converting,
    Finalizing,
    Completed,
    Failed(MediaForgeError),
    Cancelled,
}

impl JobState {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Preparing | Self::Downloading | Self::Processing(_) | Self::Converting | Self::Finalizing)
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed(_) | Self::Cancelled)
    }

    pub fn is_pending(&self) -> bool {
        *self == Self::Queued || self.is_active()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgress {
    /// 0...1 for the current phase, or `None` when indeterminate.
    pub fraction: Option<f64>,
    pub bytes_per_second: Option<f64>,
    pub seconds_remaining: Option<f64>,
    pub downloaded_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// FFmpeg throughput relative to real time: 42 means 42× faster than playback.
    pub conversion_speed: Option<f64>,
}

pub const MAX_LOG_LINES: usize = 60;

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub source: JobSource,
    pub output: OutputSpec,
    pub created_at: DateTime<Utc>,
    pub title: String,
    pub detail: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub state: JobState,
    pub progress: JobProgress,
    pub output_path: Option<PathBuf>,
    pub output_size: Option<u64>,
    pub finished_at: Option<DateTime<Utc>>,
    /// 1 for the first run, incremented on each retry.
    pub attempt: u32,
    /// Most recent diagnostic lines from the underlying tools, newest last.
    pub log: VecDeque<String>,
}

impl Job {
    pub fn new(source: JobSource, output: OutputSpec, title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            source,
            output,
            created_at: Utc::now(),
            title: title.into(),
            detail: None,
            duration: None,
            thumbnail_url: None,
            state: JobState::Queued,
            progress: JobProgress::default(),
            output_path: None,
            output_size: None,
            finished_at: None,
            attempt: 1,
            log: VecDeque::new(),
        }
    }

    pub fn kind(&self) -> JobKind {
        self.source.kind()
    }

    pub fn error(&self) -> Option<&MediaForgeError> {
        match &self.state {
            JobState::Failed(error) => Some(error),
            _ => None,
        }
    }

    pub fn append_log(&mut self, line: &str) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        self.log.push_back(trimmed.chars().take(600).collect());
        while self.log.len() > MAX_LOG_LINES {
            self.log.pop_front();
        }
    }

    /// Fills in display details once the source has been resolved, without clobbering anything already known.
    pub fn apply(&mut self, metadata: &MediaMetadata) {
        self.title = metadata.title.clone();
        if let Some(uploader) = &metadata.uploader {
            self.detail = Some(uploader.clone());
        }
        if let Some(duration) = metadata.duration {
            self.duration = Some(duration);
        }
        if let Some(thumbnail) = &metadata.thumbnail_url {
            self.thumbnail_url = Some(thumbnail.clone());
        }
    }

    pub fn snapshot(&self) -> JobSnapshot {
        JobSnapshot {
            id: self.id,
            kind: self.kind(),
            source: match &self.source {
                JobSource::Remote(url) => SourceSnapshot::Remote { url: url.as_str().to_string(), display_name: url.display_name() },
                JobSource::File(path) => SourceSnapshot::File {
                    path: path.display().to_string(),
                    name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                },
            },
            output: self.output,
            title: self.title.clone(),
            detail: self.detail.clone(),
            duration: self.duration,
            thumbnail_url: self.thumbnail_url.clone(),
            state: match &self.state {
                JobState::Queued => StateSnapshot::Queued,
                JobState::Preparing => StateSnapshot::Preparing,
                JobState::Downloading => StateSnapshot::Downloading,
                JobState::Processing(stage) => StateSnapshot::Processing { stage: stage.clone() },
                JobState::Converting => StateSnapshot::Converting,
                JobState::Finalizing => StateSnapshot::Finalizing,
                JobState::Completed => StateSnapshot::Completed,
                JobState::Failed(error) => StateSnapshot::Failed { error: error.payload() },
                JobState::Cancelled => StateSnapshot::Cancelled,
            },
            progress: self.progress,
            output_path: self.output_path.as_ref().map(|p| p.display().to_string()),
            output_size: self.output_size,
            created_at: self.created_at,
            finished_at: self.finished_at,
            attempt: self.attempt,
            has_log: !self.log.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SourceSnapshot {
    #[serde(rename_all = "camelCase")]
    Remote { url: String, display_name: String },
    #[serde(rename_all = "camelCase")]
    File { path: String, name: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StateSnapshot {
    Queued,
    Preparing,
    Downloading,
    Processing { stage: String },
    Converting,
    Finalizing,
    Completed,
    Failed { error: ErrorPayload },
    Cancelled,
}

/// A job as the UI sees it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: Uuid,
    pub kind: JobKind,
    pub source: SourceSnapshot,
    pub output: OutputSpec,
    pub title: String,
    pub detail: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub state: StateSnapshot,
    pub progress: JobProgress,
    pub output_path: Option<String>,
    pub output_size: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub attempt: u32,
    pub has_log: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AudioOptions;

    #[test]
    fn keeps_only_the_newest_log_lines() {
        let mut job = Job::new(JobSource::File(PathBuf::from("/a.wav")), OutputSpec::Audio(AudioOptions::default()), "a");
        for index in 0..100 {
            job.append_log(&format!("line {index}"));
        }
        job.append_log("   ");
        assert_eq!(job.log.len(), MAX_LOG_LINES);
        assert_eq!(job.log.back().map(String::as_str), Some("line 99"));
    }

    #[test]
    fn snapshots_serialise_for_the_ui() {
        let mut job = Job::new(JobSource::File(PathBuf::from("/music/a.wav")), OutputSpec::Audio(AudioOptions::default()), "a");
        job.state = JobState::Processing("Merging video and audio".into());
        let json = serde_json::to_value(job.snapshot()).unwrap();
        assert_eq!(json["kind"], "conversion");
        assert_eq!(json["state"]["type"], "processing");
        assert_eq!(json["state"]["stage"], "Merging video and audio");
        assert_eq!(json["source"]["type"], "file");
        assert_eq!(json["source"]["name"], "a.wav");
        assert_eq!(json["output"]["format"], "mp3");
        assert!(JobState::Queued.is_pending() && !JobState::Queued.is_active());
        assert!(JobState::Cancelled.is_finished());
    }
}
