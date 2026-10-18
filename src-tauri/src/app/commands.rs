//! Typed commands the UI can call. Each one validates what it is given: the webview is never trusted to
//! supply safe paths, links or options.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::app::state::{AppState, ToolsReport};
use crate::engine::job::{Job, JobKind, JobSnapshot, JobSource};
use crate::engine::media_files;
use crate::error::{ErrorPayload, Failure, MediaForgeError};
use crate::licensing::LicensingSummary;
use crate::media::metadata::PlaylistEntry;
use crate::media::video;
use crate::media::ytdlp::YtDlp;
use crate::models::{AudioOptions, OutputSpec, Section, VideoQuality};
use crate::store::history::HistoryItem;
use crate::store::settings::Settings;
use crate::support::url_validator::{self, ValidatedUrl};
use crate::tools::Tool;

type AppStateRef<'a> = State<'a, Arc<AppState>>;
pub type CommandResult<T> = Result<T, ErrorPayload>;

fn failure(error: MediaForgeError) -> ErrorPayload {
    error.payload()
}

fn cancelled() -> ErrorPayload {
    ErrorPayload { code: "cancelled", title: "Cancelled".into(), message: String::new(), retryable: true }
}

fn simple(title: &str, message: impl Into<String>) -> ErrorPayload {
    ErrorPayload { code: "message", title: title.into(), message: message.into(), retryable: false }
}

// MARK: - Start-up

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap {
    pub platform: &'static str,
    pub version: String,
    pub build: String,
    pub is_debug: bool,
    pub settings: Settings,
    pub default_destination: String,
    pub destination: String,
    pub jobs: Vec<JobSnapshot>,
    pub history: Vec<HistoryItem>,
    pub tools: ToolsReport,
    pub has_third_party_notices: bool,
    pub pending_section: Option<Section>,
}

pub fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

/// The build number shown next to the version: "1.1.0 (4)". Kept in `bundle.macOS.bundleVersion`.
pub fn build_number(_app: &AppHandle) -> String {
    env!("MEDIAFORGE_BUILD_NUMBER").to_string()
}

#[tauri::command]
pub fn bootstrap(app: AppHandle, state: AppStateRef<'_>) -> Bootstrap {
    state.frontend_ready.store(true, Ordering::SeqCst);
    Bootstrap {
        platform: platform(),
        version: app.package_info().version.to_string(),
        build: build_number(&app),
        is_debug: cfg!(debug_assertions),
        settings: state.settings(),
        default_destination: state.paths.default_destination.display().to_string(),
        destination: state.destination().display().to_string(),
        jobs: state.queue.snapshots(),
        history: state.history_items(),
        tools: state.tools_report(),
        has_third_party_notices: third_party_notices(&state).is_some(),
        pending_section: state.pending_section.lock().unwrap().take(),
    }
}

// MARK: - Settings

#[tauri::command]
pub fn update_settings(change: Map<String, Value>, state: AppStateRef<'_>) -> Settings {
    let before = state.settings();
    let after = state.settings.lock().unwrap().update(&change).clone();
    apply_side_effects(&state, &before, &after);
    after
}

fn apply_side_effects(state: &AppState, before: &Settings, after: &Settings) {
    if before.max_concurrent_jobs != after.max_concurrent_jobs {
        state.queue.set_max_concurrent(after.max_concurrent_jobs as usize);
    }
    if before.tool_paths != after.tool_paths {
        state.tools.refresh(Some(after.tool_overrides()));
    }
    if before.appearance != after.appearance {
        crate::app::apply_appearance(&state.handle, after.appearance);
    }
    state.emit_settings();
}

#[tauri::command]
pub async fn choose_destination(app: AppHandle, state: AppStateRef<'_>) -> CommandResult<Option<Settings>> {
    let current = state.destination();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog().file().set_title("Choose where MediaForge saves finished files").set_directory(current).blocking_pick_folder()
    })
    .await
    .map_err(|e| simple("Couldn't open the folder picker", e.to_string()))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let before = state.settings();
    let after = state.settings.lock().unwrap().modify(|s| s.destination_path = path.display().to_string()).clone();
    apply_side_effects(&state, &before, &after);
    Ok(Some(after))
}

#[tauri::command]
pub fn open_destination(app: AppHandle, state: AppStateRef<'_>) -> CommandResult<()> {
    let destination = state.destination();
    let _ = std::fs::create_dir_all(&destination);
    app.opener().open_path(destination.display().to_string(), None::<&str>).map_err(|_| {
        simple("Folder Unavailable", format!("“{}” couldn't be opened. Choose a different destination in Settings.", destination.display()))
    })
}

#[tauri::command]
pub async fn choose_tool_path(tool: String, app: AppHandle, state: AppStateRef<'_>) -> CommandResult<Option<Settings>> {
    let tool = Tool::parse(&tool).ok_or_else(|| simple("Unknown tool", tool.clone()))?;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app.dialog().file().set_title(format!("Choose the {} program", tool.display_name()));
        if cfg!(windows) {
            dialog = dialog.add_filter("Programs", &["exe"]);
        } else if Path::new("/opt/homebrew/bin").is_dir() {
            dialog = dialog.set_directory("/opt/homebrew/bin");
        }
        dialog.blocking_pick_file()
    })
    .await
    .map_err(|e| simple("Couldn't open the file picker", e.to_string()))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let before = state.settings();
    let after = state
        .settings
        .lock()
        .unwrap()
        .modify(|s| {
            s.tool_paths.insert(tool.id().to_string(), path.display().to_string());
        })
        .clone();
    apply_side_effects(&state, &before, &after);
    Ok(Some(after))
}

#[tauri::command]
pub fn reset_tool_path(tool: String, state: AppStateRef<'_>) -> Settings {
    let before = state.settings();
    let after = state
        .settings
        .lock()
        .unwrap()
        .modify(|s| {
            s.tool_paths.remove(&tool);
        })
        .clone();
    apply_side_effects(&state, &before, &after);
    after
}

#[tauri::command]
pub fn refresh_tools(state: AppStateRef<'_>) -> ToolsReport {
    let overrides = state.settings.lock().unwrap().get().tool_overrides();
    state.tools.refresh(Some(overrides));
    state.tools_report()
}

#[tauri::command]
pub fn acknowledge_terms(state: AppStateRef<'_>) -> Settings {
    let settings = state.settings.lock().unwrap().modify(|s| s.has_acknowledged_terms = true).clone();
    state.emit_settings();
    settings
}

#[tauri::command]
pub fn set_section(section: Section, state: AppStateRef<'_>) {
    state.settings.lock().unwrap().modify(|s| s.selected_section = section);
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

// MARK: - Links

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkSummary {
    pub url: String,
    pub display_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInspection {
    pub urls: Vec<LinkSummary>,
    pub rejected: usize,
    pub truncated: bool,
    /// Why the text isn't a usable link, when nothing in it is.
    pub invalid_message: Option<String>,
}

/// Validates what is in the link field. Fast and local: nothing is contacted.
#[tauri::command]
pub fn inspect_links(text: String) -> LinkInspection {
    let text: String = text.chars().take(100_000).collect();
    let extraction = url_validator::extract(&text);
    let invalid_message = if extraction.urls.is_empty() && !text.trim().is_empty() {
        Some(match url_validator::validate(&text) {
            Err(error) => error.message(),
            Ok(_) => url_validator::UrlValidationError::Malformed.message(),
        })
    } else {
        None
    };
    LinkInspection {
        urls: extraction.urls.iter().map(|u| LinkSummary { url: u.as_str().into(), display_name: u.display_name() }).collect(),
        rejected: extraction.rejected.len(),
        truncated: extraction.truncated,
        invalid_message,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    pub is_playlist: bool,
    pub url: String,
    pub title: String,
    pub uploader: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail_url: Option<String>,
    pub source_name: Option<String>,
    pub has_video: bool,
    pub max_video_height: Option<u32>,
    /// Qualities worth offering for this source.
    pub quality_options: Vec<VideoQuality>,
    /// For each offered quality, whether the download will need converting to open in most apps.
    pub conversion_expected: HashMap<String, bool>,
    pub entries: Vec<PlaylistEntry>,
}

/// Looks a link up with yt-dlp. A newer lookup cancels an older one still running.
#[tauri::command]
pub async fn resolve_link(url: String, state: AppStateRef<'_>) -> CommandResult<Resolution> {
    let url = url_validator::validate(&url).map_err(|e| simple("Invalid link", e.message()))?;
    let toolchain = state.tools.toolchain();
    let executable = toolchain.ytdlp.clone().ok_or_else(|| failure(MediaForgeError::ToolMissing(Tool::YtDlp)))?;
    let token = CancellationToken::new();
    if let Some(previous) = state.resolver.lock().unwrap().replace(token.clone()) {
        previous.cancel();
    }
    let service = YtDlp {
        executable,
        ffmpeg_directory: toolchain.ffmpeg.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)),
        support_directories: toolchain.support_directories.clone(),
    };
    let result = service.fetch_metadata(&url, &token).await;
    let metadata = match result {
        Ok(metadata) => metadata,
        Err(Failure::Cancelled) => return Err(cancelled()),
        Err(Failure::Error(error)) => return Err(failure(error)),
    };
    if metadata.is_playlist && metadata.playlist_entries.is_empty() {
        return Err(failure(MediaForgeError::Unavailable));
    }
    let max_height = metadata.max_video_height();
    let quality_options = VideoQuality::options_for_source_height(max_height);
    let conversion_expected =
        quality_options.iter().map(|q| (q.value().to_string(), video::expects_conversion(&metadata.formats, q.max_height()))).collect();
    Ok(Resolution {
        is_playlist: metadata.is_playlist,
        url: url.as_str().into(),
        title: metadata.title.clone(),
        uploader: metadata.uploader.clone(),
        duration: metadata.duration,
        thumbnail_url: metadata.thumbnail_url.clone(),
        source_name: metadata.source_name(),
        has_video: metadata.has_video(),
        max_video_height: max_height,
        quality_options,
        conversion_expected,
        entries: metadata.playlist_entries.clone(),
    })
}

#[tauri::command]
pub fn cancel_resolve(state: AppStateRef<'_>) {
    if let Some(token) = state.resolver.lock().unwrap().take() {
        token.cancel();
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
    pub url: String,
    pub title: Option<String>,
    pub detail: Option<String>,
    pub duration: Option<f64>,
    pub thumbnail_url: Option<String>,
}

/// Queues downloads. Every link is validated again here.
#[tauri::command]
pub fn enqueue_downloads(items: Vec<DownloadItem>, output: OutputSpec, state: AppStateRef<'_>) -> CommandResult<usize> {
    let jobs: Vec<Job> = items
        .into_iter()
        .take(url_validator::MAX_BATCH_SIZE)
        .filter_map(|item| {
            let url: ValidatedUrl = url_validator::validate(&item.url).ok()?;
            let title = item
                .title
                .map(|t| t.trim().chars().take(500).collect::<String>())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| url.display_name());
            let mut job = Job::new(JobSource::Remote(url), output, title);
            job.detail = item.detail.map(|d| d.chars().take(300).collect());
            job.duration = item.duration.filter(|d| d.is_finite() && *d > 0.0);
            job.thumbnail_url = item.thumbnail_url.filter(|t| url::Url::parse(t).is_ok_and(|u| matches!(u.scheme(), "http" | "https")));
            Some(job)
        })
        .collect();
    if jobs.is_empty() {
        return Err(simple("Nothing to download", "None of the links could be used."));
    }
    let count = jobs.len();
    crate::app::prepare_notifications(&state);
    state.queue.enqueue(jobs);
    Ok(count)
}

// MARK: - Conversions

/// Queues conversions for local files and folders.
pub fn import_paths(state: &AppState, paths: &[PathBuf], options: Option<AudioOptions>) -> CommandResult<usize> {
    if !state.tools.can_convert() {
        return Err(simple("FFmpeg Is Required", MediaForgeError::ToolMissing(Tool::Ffmpeg).message()));
    }
    let files = media_files::collect(paths);
    if files.is_empty() {
        return Err(simple("No Supported Media", "MediaForge couldn't find any audio or video files in what you added."));
    }
    let options = options.unwrap_or_else(|| state.settings().audio_options());
    let jobs: Vec<Job> = files
        .into_iter()
        .map(|file| {
            let title = file.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into());
            let detail = file.extension().map(|e| e.to_string_lossy().to_uppercase());
            let mut job = Job::new(JobSource::File(file), OutputSpec::Audio(options), title);
            job.detail = detail;
            job
        })
        .collect();
    let count = jobs.len();
    crate::app::prepare_notifications(state);
    state.queue.enqueue(jobs);
    Ok(count)
}

#[tauri::command]
pub fn import_files(paths: Vec<String>, state: AppStateRef<'_>) -> CommandResult<usize> {
    let paths: Vec<PathBuf> =
        paths.into_iter().take(media_files::MAX_IMPORT_COUNT).map(PathBuf::from).filter(|p| p.is_absolute()).collect();
    import_paths(&state, &paths, None)
}

#[tauri::command]
pub async fn choose_files_to_convert(folders: bool, app: AppHandle, state: AppStateRef<'_>) -> CommandResult<usize> {
    let extensions: Vec<&str> = media_files::AUDIO_EXTENSIONS.iter().chain(media_files::VIDEO_EXTENSIONS).copied().collect();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        if folders {
            dialog.set_title("Choose folders of audio or video to convert").blocking_pick_folders()
        } else {
            dialog.set_title("Choose audio or video files to convert").add_filter("Audio and video", &extensions).blocking_pick_files()
        }
    })
    .await
    .map_err(|e| simple("Couldn't open the file picker", e.to_string()))?;
    let Some(paths) = picked else { return Ok(0) };
    let paths: Vec<PathBuf> = paths.into_iter().filter_map(|p| p.into_path().ok()).collect();
    if paths.is_empty() {
        return Ok(0);
    }
    import_paths(&state, &paths, None)
}

// MARK: - Jobs

fn parse_id(id: &str) -> CommandResult<Uuid> {
    Uuid::parse_str(id).map_err(|_| simple("Unknown job", id.to_string()))
}

#[tauri::command]
pub fn cancel_job(id: String, state: AppStateRef<'_>) -> CommandResult<()> {
    state.queue.cancel(parse_id(&id)?);
    Ok(())
}

#[tauri::command]
pub fn retry_job(id: String, state: AppStateRef<'_>) -> CommandResult<()> {
    state.queue.retry(parse_id(&id)?);
    Ok(())
}

#[tauri::command]
pub fn remove_job(id: String, state: AppStateRef<'_>) -> CommandResult<()> {
    state.queue.remove(parse_id(&id)?);
    Ok(())
}

#[tauri::command]
pub fn cancel_all(kind: Option<JobKind>, state: AppStateRef<'_>) {
    state.queue.cancel_all(kind);
}

#[tauri::command]
pub fn clear_finished(kind: Option<JobKind>, state: AppStateRef<'_>) {
    state.queue.clear_finished(kind);
}

#[tauri::command]
pub fn job_log(id: String, state: AppStateRef<'_>) -> CommandResult<Vec<String>> {
    Ok(state.queue.log(parse_id(&id)?))
}

// MARK: - Files

fn known_file(state: &AppState, path: &str) -> CommandResult<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !state.is_known_output(&path) {
        return Err(simple("File Not Found", "The file has been moved or deleted."));
    }
    if !path.is_file() {
        return Err(simple("File Not Found", "The file has been moved or deleted."));
    }
    Ok(path)
}

/// Shows a finished file (or a tool, or the destination) in Finder or File Explorer.
#[tauri::command]
pub fn reveal_path(path: String, app: AppHandle, state: AppStateRef<'_>) -> CommandResult<()> {
    let candidate = PathBuf::from(&path);
    let is_tool = state.tools.statuses().iter().any(|s| s.path.as_deref() == Some(path.as_str()));
    let target = if is_tool && candidate.is_file() { candidate } else { known_file(&state, &path)? };
    app.opener().reveal_item_in_dir(&target).map_err(|e| simple("Couldn't show the file", e.to_string()))
}

/// Opens a finished file in its default app.
#[tauri::command]
pub fn open_path(path: String, app: AppHandle, state: AppStateRef<'_>) -> CommandResult<()> {
    let target = known_file(&state, &path)?;
    if !media_files::is_supported(&target) {
        return Err(simple("Can't Open", "Only finished audio and video files can be opened from MediaForge."));
    }
    app.opener().open_path(target.display().to_string(), None::<&str>).map_err(|e| simple("Couldn't open the file", e.to_string()))
}

#[tauri::command]
pub fn copy_text(text: String, app: AppHandle) -> CommandResult<()> {
    let text: String = text.chars().take(10_000).collect();
    app.clipboard().write_text(text).map_err(|e| simple("Couldn't copy", e.to_string()))
}

/// The clipboard's text, for Paste Link. Read here rather than by the webview, which would need its own permission.
#[tauri::command]
pub fn read_clipboard(app: AppHandle) -> Option<String> {
    app.clipboard().read_text().ok().map(|t| t.chars().take(100_000).collect::<String>()).filter(|t| !t.trim().is_empty())
}

fn third_party_notices(state: &AppState) -> Option<PathBuf> {
    let folder = state.paths.resource_directory.as_ref()?.join("ThirdPartyNotices");
    folder.join("THIRD-PARTY-NOTICES.txt").is_file().then_some(folder)
}

#[tauri::command]
pub fn open_third_party_notices(app: AppHandle, state: AppStateRef<'_>) -> CommandResult<()> {
    let folder = third_party_notices(&state).ok_or_else(|| simple("No Notices", "This build doesn't include third-party tools."))?;
    app.opener().reveal_item_in_dir(folder.join("THIRD-PARTY-NOTICES.txt")).map_err(|e| simple("Couldn't show the notices", e.to_string()))
}

// MARK: - History

#[tauri::command]
pub fn remove_history(ids: Vec<String>, state: AppStateRef<'_>) {
    let ids: Vec<Uuid> = ids.iter().filter_map(|id| Uuid::parse_str(id).ok()).collect();
    state.history.lock().unwrap().remove(&ids);
    state.emit_history();
}

#[tauri::command]
pub fn clear_history(state: AppStateRef<'_>) {
    state.history.lock().unwrap().clear();
    state.emit_history();
}

/// Queues a history item's source again with the same output settings. Returns the screen to show.
#[tauri::command]
pub fn repeat_history(id: String, state: AppStateRef<'_>) -> CommandResult<Section> {
    let id = Uuid::parse_str(&id).map_err(|_| simple("Unknown item", id.clone()))?;
    let entry =
        state.history.lock().unwrap().find(id).cloned().ok_or_else(|| simple("Unknown item", "It has been removed from History."))?;
    if let Some(url) = entry.source_url.as_deref().and_then(|u| url_validator::validate(u).ok()) {
        let mut job = Job::new(JobSource::Remote(url), entry.output, entry.title.clone());
        job.detail = entry.detail.clone();
        job.duration = entry.duration;
        job.thumbnail_url = entry.thumbnail_url.clone();
        crate::app::prepare_notifications(&state);
        state.queue.enqueue(vec![job]);
        return Ok(Section::Downloads);
    }
    if let (Some(path), OutputSpec::Audio(options)) = (entry.source_file_path.as_deref(), entry.output) {
        import_paths(&state, &[PathBuf::from(path)], Some(options))?;
        return Ok(Section::Conversions);
    }
    Err(simple("Can't Repeat This Item", "The original source is no longer available."))
}

// MARK: - Artwork and preview

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ThumbnailRequest {
    Remote { url: String },
    File { path: String },
}

#[tauri::command]
pub async fn thumbnail(source: ThumbnailRequest, state: AppStateRef<'_>) -> CommandResult<Option<String>> {
    Ok(match source {
        ThumbnailRequest::Remote { url } => state.artwork.remote(&url).await,
        ThumbnailRequest::File { path } => {
            let path = PathBuf::from(path);
            // Source files the user added, or files MediaForge made: never arbitrary paths.
            let allowed = path.is_absolute()
                && media_files::is_supported(&path)
                && (state.is_known_output(&path)
                    || state.queue.snapshots().iter().any(
                        |j| matches!(&j.source, crate::engine::job::SourceSnapshot::File { path: p, .. } if Path::new(p) == path.as_path()),
                    ));
            if !allowed {
                return Ok(None);
            }
            let ffmpeg = state.tools.path(Tool::Ffmpeg);
            state.artwork.local(&path, ffmpeg.as_deref()).await
        }
    })
}

#[tauri::command]
pub async fn waveform(path: String, state: AppStateRef<'_>) -> CommandResult<Vec<f32>> {
    let path = known_file(&state, &path)?;
    let (Some(ffmpeg), Some(ffprobe)) = (state.tools.path(Tool::Ffmpeg), state.tools.path(Tool::Ffprobe)) else {
        return Ok(Vec::new());
    };
    let service = crate::media::ffmpeg::Ffmpeg { ffmpeg: ffmpeg.clone(), ffprobe };
    let duration = service.probe(&path, &CancellationToken::new()).await.ok().and_then(|p| p.duration);
    Ok(crate::artwork::waveform(&ffmpeg, &path, duration, 180).await)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewSource {
    pub path: String,
    pub transcoded: bool,
}

/// Lets the webview play one finished file through Tauri's asset protocol. With `transcode`, first makes a
/// temporary WAV copy, for formats the platform's web engine can't decode (AIFF on Windows, for example).
#[tauri::command]
pub async fn preview_source(path: String, transcode: bool, state: AppStateRef<'_>) -> CommandResult<PreviewSource> {
    let path = known_file(&state, &path)?;
    let playable = if transcode {
        let ffmpeg = state.tools.path(Tool::Ffmpeg).ok_or_else(|| failure(MediaForgeError::ToolMissing(Tool::Ffmpeg)))?;
        let directory = state.paths.cache.join("Previews");
        std::fs::create_dir_all(&directory).map_err(|e| simple("Can't Preview", e.to_string()))?;
        let identity = format!("{}|{}", path.display(), std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));
        let digest: String = sha2::Digest::finalize(<sha2::Sha256 as sha2::Digest>::new_with_prefix(identity.as_bytes()))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let target = directory.join(format!("{digest}.wav"));
        if !target.is_file() {
            let mut arguments: Vec<std::ffi::OsString> = [
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-protocol_whitelist",
                crate::media::ffmpeg::PROTOCOL_WHITELIST,
                "-i",
            ]
            .iter()
            .map(Into::into)
            .collect();
            arguments.push(crate::media::ffmpeg::file_argument(&path));
            arguments.extend(["-map", "0:a:0", "-vn", "-c:a", "pcm_s16le", "-f", "wav"].iter().map(Into::into));
            arguments.push(crate::media::ffmpeg::file_argument(&target));
            let mut request = crate::process::ProcessRequest::new(&ffmpeg, arguments);
            request.timeout = Some(std::time::Duration::from_secs(300));
            request.working_directory = Some(directory.clone());
            let output = crate::process::run_collect(&request, &CancellationToken::new())
                .await
                .map_err(|_| simple("Can't Preview", "This file can't be previewed."))?;
            if !output.exit.success() {
                let _ = std::fs::remove_file(&target);
                return Err(simple("Can't Preview", "This file can't be previewed."));
            }
        }
        target
    } else {
        path
    };
    state.handle.asset_protocol_scope().allow_file(&playable).map_err(|e| simple("Can't Preview", e.to_string()))?;
    state.previews.lock().unwrap().insert(playable.clone());
    Ok(PreviewSource { path: playable.display().to_string(), transcoded: transcode })
}

#[tauri::command]
pub fn licensing_summary(state: AppStateRef<'_>) -> LicensingSummary {
    state.licensing.summary()
}

/// Lets the window ask Rust to show it once the first frame is ready, avoiding a white flash.
#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main")
        && !window.is_visible().unwrap_or(false)
    {
        crate::app::show_quietly_or_normally(&window);
    }
}
