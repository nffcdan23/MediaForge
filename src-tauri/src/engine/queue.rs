//! Owns every job in the current session and decides when each one runs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::job::{Job, JobKind, JobProgress, JobSnapshot, JobState};
use crate::engine::pipeline;
use crate::error::{Failure, MediaForgeError};
use crate::media::video::EncoderCache;
use crate::models::{DownloadLimits, VideoCompatibility};
use crate::tools::Toolchain;

/// Settings captured at the moment a job starts, so changing preferences mid-run can't affect it.
#[derive(Debug, Clone)]
pub struct JobEnvironment {
    pub toolchain: Toolchain,
    pub destination: PathBuf,
    pub limits: DownloadLimits,
    pub embed_metadata: bool,
    /// What to do with video most players and editors can't open.
    pub video_compatibility: VideoCompatibility,
    pub work_root: PathBuf,
}

/// Receives the queue's news. Implemented by the app (events, history, notifications) and by tests.
pub trait QueueObserver: Send + Sync {
    fn job_changed(&self, job: &JobSnapshot);
    fn jobs_removed(&self, ids: &[Uuid]);
    /// Called once a job reaches a finished state (completed, failed or cancelled).
    fn job_finished(&self, job: &Job);
}

/// How often a running job's progress may be sent to the UI.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

pub(crate) struct Inner {
    state: Mutex<State>,
    environment: Box<dyn Fn() -> JobEnvironment + Send + Sync>,
    observer: Arc<dyn QueueObserver>,
    pub(crate) encoders: EncoderCache,
    idle: tokio::sync::Notify,
}

struct State {
    /// In the order they were added. The UI shows the newest first; scheduling takes the oldest first.
    jobs: Vec<Job>,
    running: HashMap<Uuid, CancellationToken>,
    max_concurrent: usize,
    last_emit: HashMap<Uuid, Instant>,
}

#[derive(Clone)]
pub struct JobQueue {
    inner: Arc<Inner>,
}

impl JobQueue {
    pub fn new(
        max_concurrent: usize,
        environment: impl Fn() -> JobEnvironment + Send + Sync + 'static,
        observer: Arc<dyn QueueObserver>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    jobs: Vec::new(),
                    running: HashMap::new(),
                    max_concurrent: max_concurrent.max(1),
                    last_emit: HashMap::new(),
                }),
                environment: Box::new(environment),
                observer,
                encoders: EncoderCache::default(),
                idle: tokio::sync::Notify::new(),
            }),
        }
    }

    // MARK: Queries

    pub fn snapshots(&self) -> Vec<JobSnapshot> {
        self.inner.state.lock().unwrap().jobs.iter().map(Job::snapshot).collect()
    }

    pub fn job(&self, id: Uuid) -> Option<Job> {
        self.inner.state.lock().unwrap().jobs.iter().find(|j| j.id == id).cloned()
    }

    pub fn log(&self, id: Uuid) -> Vec<String> {
        self.job(id).map(|j| j.log.into_iter().collect()).unwrap_or_default()
    }

    pub fn pending_count(&self, kind: Option<JobKind>) -> usize {
        let state = self.inner.state.lock().unwrap();
        state.jobs.iter().filter(|j| j.state.is_pending() && kind.is_none_or(|k| j.kind() == k)).count()
    }

    /// Overall progress of everything pending, for the taskbar and Dock: `None` when nothing is pending,
    /// `Some(None)` when the amount of work is unknown.
    pub fn aggregate_progress(&self) -> Option<Option<f64>> {
        let state = self.inner.state.lock().unwrap();
        let pending: Vec<&Job> = state.jobs.iter().filter(|j| j.state.is_pending()).collect();
        if pending.is_empty() {
            return None;
        }
        let fractions: Vec<f64> = pending
            .iter()
            .map(|j| if j.state == JobState::Queued { Some(0.0) } else { j.progress.fraction })
            .collect::<Option<Vec<f64>>>()
            .unwrap_or_default();
        if fractions.is_empty() { Some(None) } else { Some(Some(fractions.iter().sum::<f64>() / fractions.len() as f64)) }
    }

    /// Every finished file the queue knows about, for checking requests to open or reveal one.
    pub fn output_paths(&self) -> Vec<PathBuf> {
        self.inner.state.lock().unwrap().jobs.iter().filter_map(|j| j.output_path.clone()).collect()
    }

    // MARK: Mutations

    pub fn enqueue(&self, jobs: Vec<Job>) {
        if jobs.is_empty() {
            return;
        }
        let snapshots: Vec<JobSnapshot> = jobs.iter().map(Job::snapshot).collect();
        self.inner.state.lock().unwrap().jobs.extend(jobs);
        for snapshot in &snapshots {
            self.inner.observer.job_changed(snapshot);
        }
        self.schedule();
    }

    pub fn set_max_concurrent(&self, count: usize) {
        self.inner.state.lock().unwrap().max_concurrent = count.max(1);
        self.schedule();
    }

    /// Stops a running job, or pulls a waiting one out of line.
    pub fn cancel(&self, id: Uuid) {
        let finished = {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(token) = state.running.get(&id) {
                token.cancel();
                None
            } else if let Some(job) = state.jobs.iter_mut().find(|j| j.id == id && j.state == JobState::Queued) {
                job.state = JobState::Cancelled;
                job.finished_at = Some(Utc::now());
                Some(job.clone())
            } else {
                None
            }
        };
        if let Some(job) = finished {
            self.inner.observer.job_changed(&job.snapshot());
            self.inner.observer.job_finished(&job);
        }
    }

    pub fn cancel_all(&self, kind: Option<JobKind>) {
        let ids: Vec<Uuid> = {
            let state = self.inner.state.lock().unwrap();
            state.jobs.iter().filter(|j| j.state.is_pending() && kind.is_none_or(|k| j.kind() == k)).map(|j| j.id).collect()
        };
        for id in ids {
            self.cancel(id);
        }
    }

    /// Runs a failed or cancelled job again from the beginning.
    pub fn retry(&self, id: Uuid) {
        let snapshot = {
            let mut state = self.inner.state.lock().unwrap();
            if state.running.contains_key(&id) {
                return;
            }
            let Some(job) = state.jobs.iter_mut().find(|j| j.id == id) else { return };
            if !matches!(job.state, JobState::Failed(_) | JobState::Cancelled) {
                return;
            }
            job.attempt += 1;
            job.state = JobState::Queued;
            job.progress = JobProgress::default();
            job.output_path = None;
            job.output_size = None;
            job.finished_at = None;
            job.log.clear();
            job.snapshot()
        };
        self.inner.observer.job_changed(&snapshot);
        self.schedule();
    }

    pub fn remove(&self, id: Uuid) {
        {
            let mut state = self.inner.state.lock().unwrap();
            if let Some(token) = state.running.get(&id) {
                token.cancel();
            }
            state.jobs.retain(|j| j.id != id);
            state.last_emit.remove(&id);
        }
        self.inner.observer.jobs_removed(&[id]);
    }

    pub fn clear_finished(&self, kind: Option<JobKind>) {
        let removed: Vec<Uuid> = {
            let mut state = self.inner.state.lock().unwrap();
            let removed: Vec<Uuid> =
                state.jobs.iter().filter(|j| j.state.is_finished() && kind.is_none_or(|k| j.kind() == k)).map(|j| j.id).collect();
            state.jobs.retain(|j| !removed.contains(&j.id));
            removed
        };
        if !removed.is_empty() {
            self.inner.observer.jobs_removed(&removed);
        }
    }

    /// Suspends until nothing is running or waiting to run. Used on quit and by tests.
    pub async fn wait_until_idle(&self) {
        loop {
            let notified = self.inner.idle.notified();
            {
                let state = self.inner.state.lock().unwrap();
                let waiting = state.jobs.iter().any(|j| j.state == JobState::Queued);
                if state.running.is_empty() && !waiting {
                    return;
                }
            }
            notified.await;
        }
    }

    // MARK: Scheduling

    fn schedule(&self) {
        let started: Vec<(Uuid, CancellationToken, JobSnapshot)> = {
            let mut guard = self.inner.state.lock().unwrap();
            let state = &mut *guard;
            let mut started = Vec::new();
            let limit = state.max_concurrent.max(1);
            for job in state.jobs.iter_mut() {
                if state.running.len() >= limit {
                    break;
                }
                if job.state != JobState::Queued {
                    continue;
                }
                let token = CancellationToken::new();
                job.state = JobState::Preparing;
                state.running.insert(job.id, token.clone());
                started.push((job.id, token, job.snapshot()));
            }
            started
        };
        for (id, token, snapshot) in started {
            self.inner.observer.job_changed(&snapshot);
            let environment = (self.inner.environment)();
            let handle = JobHandle { inner: Arc::clone(&self.inner), id };
            let queue = self.clone();
            tauri::async_runtime::spawn(async move {
                let result = pipeline::run(&handle, &environment, &token).await;
                queue.finish(id, result, token.is_cancelled());
            });
        }
    }

    fn finish(&self, id: Uuid, result: Result<pipeline::Delivered, Failure>, was_cancelled: bool) {
        let finished = {
            let mut state = self.inner.state.lock().unwrap();
            state.running.remove(&id);
            state.last_emit.remove(&id);
            state.jobs.iter_mut().find(|j| j.id == id).map(|job| {
                match result {
                    Ok(delivered) => {
                        job.state = JobState::Completed;
                        job.progress = JobProgress { fraction: Some(1.0), ..Default::default() };
                        job.output_path = Some(delivered.path);
                        job.output_size = Some(delivered.size);
                    }
                    Err(Failure::Cancelled) => job.state = JobState::Cancelled,
                    Err(Failure::Error(_)) if was_cancelled => job.state = JobState::Cancelled,
                    Err(Failure::Error(error)) => job.state = JobState::Failed(error),
                }
                job.finished_at = Some(Utc::now());
                job.clone()
            })
        };
        if let Some(job) = finished {
            self.inner.observer.job_changed(&job.snapshot());
            self.inner.observer.job_finished(&job);
        }
        self.inner.idle.notify_waiters();
        self.schedule();
        self.inner.idle.notify_waiters();
    }
}

/// The pipeline's way of updating its job.
pub struct JobHandle {
    inner: Arc<Inner>,
    id: Uuid,
}

impl JobHandle {
    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn encoders(&self) -> &EncoderCache {
        &self.inner.encoders
    }

    /// Reads a value from the job. Returns `None` if the job has been removed.
    pub fn read<T>(&self, read: impl FnOnce(&Job) -> T) -> Option<T> {
        let state = self.inner.state.lock().unwrap();
        state.jobs.iter().find(|j| j.id == self.id).map(read)
    }

    /// Changes the job and tells the UI straight away.
    pub fn update(&self, change: impl FnOnce(&mut Job)) {
        self.apply(change, true);
    }

    /// Changes the job's progress; the UI hears about it at most a few times a second.
    pub fn progress(&self, change: impl FnOnce(&mut Job)) {
        self.apply(change, false);
    }

    pub fn set_state(&self, new_state: JobState) {
        self.update(|job| {
            job.state = new_state;
            job.progress = JobProgress::default();
        });
    }

    pub fn log(&self, line: &str) {
        let mut state = self.inner.state.lock().unwrap();
        if let Some(job) = state.jobs.iter_mut().find(|j| j.id == self.id) {
            job.append_log(line);
        }
    }

    fn apply(&self, change: impl FnOnce(&mut Job), immediate: bool) {
        let snapshot = {
            let mut guard = self.inner.state.lock().unwrap();
            let state = &mut *guard;
            let Some(job) = state.jobs.iter_mut().find(|j| j.id == self.id) else { return };
            change(job);
            let now = Instant::now();
            let due = state.last_emit.get(&self.id).is_none_or(|last| now.duration_since(*last) >= PROGRESS_INTERVAL);
            if !(immediate || due) {
                return;
            }
            state.last_emit.insert(self.id, now);
            job.snapshot()
        };
        self.inner.observer.job_changed(&snapshot);
    }
}

/// An error a pipeline step can raise when the job it belongs to has disappeared.
pub fn removed() -> Failure {
    Failure::Error(MediaForgeError::Unknown("The job was removed.".into()))
}
