//! The app's composition root: owns the engine and the stores, and connects them to the window.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::artwork::ArtworkCache;
use crate::engine::job::{Job, JobSnapshot, JobState};
use crate::engine::output_files;
use crate::engine::queue::{JobEnvironment, JobQueue, QueueObserver};
use crate::licensing::LicensingServices;
use crate::models::Section;
use crate::store::history::{HistoryEntry, HistoryItem, HistoryStore};
use crate::store::settings::{Settings, SettingsStore};
use crate::support::format;
use crate::tools::{self, BundledTools, Tool, ToolLocator, ToolManager, ToolStatus};

pub mod events {
    pub const JOB_CHANGED: &str = "jobs:changed";
    pub const JOBS_REMOVED: &str = "jobs:removed";
    pub const HISTORY_CHANGED: &str = "history:changed";
    pub const SETTINGS_CHANGED: &str = "settings:changed";
    pub const TOOLS_CHANGED: &str = "tools:changed";
    pub const NAVIGATE: &str = "app:navigate";
    pub const MENU: &str = "app:menu";
    pub const ALERT: &str = "app:alert";
    pub const DEBUG: &str = "app:debug";
}

pub const NOTIFICATION_ACTION_TYPE: &str = "job-finished";
pub const NOTIFICATION_REVEAL: &str = "reveal";

/// Where things live on this computer.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub data: PathBuf,
    pub cache: PathBuf,
    pub default_destination: PathBuf,
    pub executable_directory: Option<PathBuf>,
    pub resource_directory: Option<PathBuf>,
    pub work_root: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsReport {
    pub statuses: Vec<ToolStatus>,
    pub missing: Vec<Tool>,
    pub can_download: bool,
    pub can_convert: bool,
    pub all_bundled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Alert {
    pub title: String,
    pub message: String,
}

pub struct AppState {
    pub handle: AppHandle,
    pub paths: AppPaths,
    pub settings: Mutex<SettingsStore>,
    pub history: Mutex<HistoryStore>,
    pub tools: Arc<ToolManager>,
    pub queue: JobQueue,
    pub artwork: ArtworkCache,
    pub licensing: LicensingServices,
    /// The lookup currently running for the link field; a new one cancels it.
    pub resolver: Mutex<Option<CancellationToken>>,
    /// Files the webview has been allowed to play through the asset protocol.
    pub previews: Mutex<HashSet<PathBuf>>,
    pub quitting: AtomicBool,
    pub frontend_ready: AtomicBool,
    /// Section to show once the window has loaded, for files opened before it was ready.
    pub pending_section: Mutex<Option<Section>>,
}

impl AppState {
    pub fn new(handle: AppHandle, paths: AppPaths, import_legacy: bool) -> Arc<Self> {
        let settings = SettingsStore::load(paths.data.join("settings.json"), paths.default_destination.clone(), || {
            if import_legacy { crate::store::legacy::home().and_then(|home| crate::store::legacy::settings(&home)) } else { None }
        });
        let history = HistoryStore::load(paths.data.join("history.json"), || {
            if import_legacy {
                crate::store::legacy::home()
                    .map(|home| crate::store::legacy::history(&crate::store::legacy::history_path(&home)))
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        });

        let mut locator = ToolLocator {
            search_directories: tools::default_search_directories(),
            overrides: settings.get().tool_overrides(),
            bundled: BundledTools::discover(paths.executable_directory.as_deref(), paths.resource_directory.as_deref()),
        };
        if cfg!(debug_assertions) && std::env::var("MEDIAFORGE_DEBUG_BUNDLED_TOOLS_ONLY").as_deref() == Ok("1") {
            locator.search_directories.clear();
        }
        let tools_handle = handle.clone();
        let tools = ToolManager::new(locator, move || {
            if let Some(state) = tools_handle.try_state::<Arc<AppState>>() {
                let _ = tools_handle.emit(events::TOOLS_CHANGED, state.tools_report());
            }
        });

        let observer: Arc<dyn QueueObserver> = Arc::new(Observer { handle: handle.clone() });
        let environment_handle = handle.clone();
        let work_root = paths.work_root.clone();
        let fallback_destination = paths.default_destination.clone();
        let queue = JobQueue::new(
            settings.get().max_concurrent_jobs as usize,
            move || {
                let state = environment_handle.state::<Arc<AppState>>();
                let settings = state.settings.lock().unwrap();
                JobEnvironment {
                    toolchain: state.tools.toolchain(),
                    destination: settings.get().destination(&fallback_destination),
                    limits: settings.get().download_limits(),
                    embed_metadata: settings.get().embed_metadata,
                    video_compatibility: settings.get().video_compatibility,
                    work_root: work_root.clone(),
                }
            },
            observer,
        );

        Arc::new(Self {
            artwork: ArtworkCache::new(paths.cache.join("Artwork")),
            handle,
            paths,
            settings: Mutex::new(settings),
            history: Mutex::new(history),
            tools,
            queue,
            licensing: LicensingServices::inactive(),
            resolver: Mutex::new(None),
            previews: Mutex::new(HashSet::new()),
            quitting: AtomicBool::new(false),
            frontend_ready: AtomicBool::new(false),
            pending_section: Mutex::new(None),
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().get().clone()
    }

    pub fn destination(&self) -> PathBuf {
        self.settings.lock().unwrap().destination()
    }

    pub fn tools_report(&self) -> ToolsReport {
        let statuses = self.tools.statuses();
        let missing = self.tools.missing();
        ToolsReport {
            all_bundled: statuses.iter().all(|s| s.bundled),
            can_download: missing.is_empty(),
            can_convert: self.tools.can_convert(),
            missing,
            statuses,
        }
    }

    pub fn history_items(&self) -> Vec<HistoryItem> {
        self.history.lock().unwrap().items()
    }

    pub fn emit_settings(&self) {
        let _ = self.handle.emit(events::SETTINGS_CHANGED, self.settings());
    }

    pub fn emit_history(&self) {
        let _ = self.handle.emit(events::HISTORY_CHANGED, self.history_items());
    }

    pub fn navigate(&self, section: Section) {
        if self.frontend_ready.load(Ordering::SeqCst) {
            let _ = self.handle.emit(events::NAVIGATE, section);
        } else {
            *self.pending_section.lock().unwrap() = Some(section);
        }
        self.settings.lock().unwrap().modify(|s| s.selected_section = section);
    }

    pub fn alert(&self, title: impl Into<String>, message: impl Into<String>) {
        let _ = self.handle.emit(events::ALERT, Alert { title: title.into(), message: message.into() });
    }

    /// Files the UI may open, reveal or play: anything the queue or history produced, and nothing else.
    pub fn is_known_output(&self, path: &Path) -> bool {
        let matches = |candidate: &Path| candidate == path;
        self.queue.output_paths().iter().any(|p| matches(p))
            || self.history.lock().unwrap().entries().iter().filter_map(|e| e.output_path.as_deref()).any(|p| matches(Path::new(p)))
    }

    /// Shows the window again, for example after it was closed on macOS or when a second copy was launched.
    pub fn show_window(&self) {
        if let Some(window) = self.handle.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }

    /// The Dock badge on macOS and the taskbar button's progress on Windows.
    pub fn update_activity_indicators(&self) {
        let Some(window) = self.handle.get_webview_window("main") else { return };
        let pending = self.queue.pending_count(None);
        #[cfg(target_os = "macos")]
        {
            let _ = window.set_badge_count(if pending > 0 { Some(pending as i64) } else { None });
        }
        #[cfg(not(target_os = "macos"))]
        {
            use tauri::window::{ProgressBarState, ProgressBarStatus};
            let state = match self.queue.aggregate_progress() {
                None => ProgressBarState { status: Some(ProgressBarStatus::None), progress: None },
                Some(None) => ProgressBarState { status: Some(ProgressBarStatus::Indeterminate), progress: None },
                Some(Some(fraction)) => ProgressBarState {
                    status: Some(ProgressBarStatus::Normal),
                    progress: Some((fraction * 100.0).round().clamp(1.0, 100.0) as u64),
                },
            };
            let _ = window.set_progress_bar(state);
            let _ = pending;
        }
    }

    fn notify(&self, job: &Job) {
        use tauri_plugin_notification::NotificationExt;
        if !self.settings().notifications_enabled {
            return;
        }
        let noun = if job.kind() == crate::engine::job::JobKind::Download { "Download" } else { "Conversion" };
        let title: String = job.title.chars().take(200).collect();
        let mut builder = self.handle.notification().builder();
        match &job.state {
            JobState::Completed => {
                let mut body = vec![job.output.summary()];
                if let Some(size) = job.output_size {
                    body.push(format::bytes(size));
                }
                builder = builder.title(format!("{noun} complete")).body(format!("{title}\n{}", body.join(" · ")));
                if let Some(path) = &job.output_path {
                    builder = builder.action_type_id(NOTIFICATION_ACTION_TYPE).extra("path", path.display().to_string());
                }
            }
            JobState::Failed(error) => {
                builder = builder.title(format!("{noun} failed")).body(format!("{title}\n{}", error.title()));
            }
            _ => return,
        }
        let _ = builder.show();
    }

    /// Cleans up this run's working folders and temporary previews.
    pub fn clean_up(&self) {
        output_files::remove_work_directory(&self.paths.work_root);
        let _ = std::fs::remove_dir_all(self.paths.cache.join("Previews"));
    }
}

struct Observer {
    handle: AppHandle,
}

impl QueueObserver for Observer {
    fn job_changed(&self, job: &JobSnapshot) {
        let _ = self.handle.emit(events::JOB_CHANGED, job);
        if let Some(state) = self.handle.try_state::<Arc<AppState>>() {
            state.update_activity_indicators();
        }
    }

    fn jobs_removed(&self, ids: &[Uuid]) {
        let _ = self.handle.emit(events::JOBS_REMOVED, ids);
        if let Some(state) = self.handle.try_state::<Arc<AppState>>() {
            state.update_activity_indicators();
        }
    }

    fn job_finished(&self, job: &Job) {
        let Some(state) = self.handle.try_state::<Arc<AppState>>() else { return };
        if let Some(entry) = HistoryEntry::from_job(job, Utc::now()) {
            state.history.lock().unwrap().record(entry);
            state.emit_history();
        }
        if matches!(job.state, JobState::Completed | JobState::Failed(_)) && !state.quitting.load(Ordering::SeqCst) {
            state.notify(job);
        }
        state.update_activity_indicators();
    }
}
