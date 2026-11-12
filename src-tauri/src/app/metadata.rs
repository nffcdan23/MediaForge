//! Commands for the Metadata screen. The window can only touch files the user added to it through the
//! system's file dialog or by dropping them on the window: paths it sends are checked against that list.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;

use super::commands::CommandResult;
use super::state::{AppState, events};
use crate::error::ErrorPayload;
use crate::tags::{self, Change, CoverChange, Field, TagError, TagFile, Tools};
use crate::tools::Tool;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

fn payload(error: TagError) -> ErrorPayload {
    ErrorPayload { code: "message", title: error.title, message: error.message, retryable: false }
}

fn simple(title: &str, message: impl Into<String>) -> ErrorPayload {
    ErrorPayload { code: "message", title: title.into(), message: message.into(), retryable: false }
}

pub fn tools(state: &AppState) -> Tools {
    Tools { ffmpeg: state.tools.path(Tool::Ffmpeg), ffprobe: state.tools.path(Tool::Ffprobe) }
}

/// A path the window sent, if it's one of the files the user added.
fn known(state: &AppState, path: &str) -> CommandResult<PathBuf> {
    let path = PathBuf::from(path);
    if path.is_absolute() && state.metadata_files.lock().unwrap().contains(&path) {
        Ok(path)
    } else {
        Err(simple("File Not Found", "Add the file to the Metadata screen again."))
    }
}

/// Adds files and folders to the Metadata screen and reads them.
pub async fn add_paths(state: &AppState, paths: Vec<PathBuf>) -> Vec<TagFile> {
    let files = tauri::async_runtime::spawn_blocking(move || tags::files::collect(&paths)).await.unwrap_or_default();
    state.metadata_files.lock().unwrap().extend(files.iter().cloned());
    tags::read_many(files, &tools(state)).await
}

/// Files dropped on the window while the Metadata screen is showing.
pub fn add_dropped(state: Arc<AppState>, paths: Vec<PathBuf>) {
    tauri::async_runtime::spawn(async move {
        let files = add_paths(&state, paths).await;
        if files.is_empty() {
            state.alert("Nothing to Add", "Those were empty folders or hidden files.");
        } else {
            let _ = state.handle.emit(events::METADATA_ADDED, files);
        }
    });
}

#[tauri::command]
pub async fn metadata_choose_files(folders: bool, app: AppHandle, state: AppStateRef<'_>) -> CommandResult<Vec<TagFile>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let dialog = app.dialog().file();
        if folders {
            dialog.set_title("Choose folders whose files you want to edit").blocking_pick_folders()
        } else {
            dialog.set_title("Choose files to edit").blocking_pick_files()
        }
    })
    .await
    .map_err(|e| simple("Couldn't open the file picker", e.to_string()))?;
    let Some(paths) = picked else { return Ok(Vec::new()) };
    let paths: Vec<PathBuf> = paths.into_iter().filter_map(|p| p.into_path().ok()).collect();
    Ok(add_paths(&state, paths).await)
}

/// Reads files again, for Revert or after something else changed them.
#[tauri::command]
pub async fn metadata_reload(paths: Vec<String>, state: AppStateRef<'_>) -> CommandResult<Vec<TagFile>> {
    let paths: Vec<PathBuf> = paths.iter().take(tags::files::MAX_FILES).filter_map(|p| known(&state, p).ok()).collect();
    Ok(tags::read_many(paths, &tools(&state)).await)
}

/// Takes files off the screen; the window can no longer change them.
#[tauri::command]
pub fn metadata_forget(paths: Vec<String>, state: AppStateRef<'_>) {
    let mut known = state.metadata_files.lock().unwrap();
    for path in paths {
        known.remove(Path::new(&path));
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CoverEdit {
    Set { id: String },
    Remove,
}

/// Everything to change in one file, as the window sends it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataEdit {
    pub path: String,
    #[serde(default)]
    pub fields: BTreeMap<Field, String>,
    #[serde(default)]
    pub cover: Option<CoverEdit>,
    /// New name without the extension.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub modified: Option<String>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub read_only: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveOutcome {
    /// The path the edit was for.
    pub path: String,
    /// The file as it is now (renaming changes its path).
    pub file: Option<TagFile>,
    pub error: Option<ErrorPayload>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProgress {
    pub done: usize,
    pub total: usize,
}

fn change_for(state: &AppState, edit: &MetadataEdit) -> CommandResult<Change> {
    let cover = match &edit.cover {
        None => None,
        Some(CoverEdit::Remove) => Some(CoverChange::Remove),
        Some(CoverEdit::Set { id }) => {
            let image =
                state.covers.lock().unwrap().get(id).cloned().ok_or_else(|| simple("Artwork Missing", "Choose the artwork again."))?;
            Some(CoverChange::Set(image))
        }
    };
    let time = |text: &Option<String>| {
        text.as_deref().filter(|t| !t.trim().is_empty()).map(tags::files::parse_timestamp).transpose().map_err(payload)
    };
    Ok(Change {
        fields: edit.fields.clone(),
        cover,
        name: edit.name.clone().filter(|n| !n.trim().is_empty()),
        modified: time(&edit.modified)?,
        created: time(&edit.created)?,
        read_only: edit.read_only,
    })
}

/// Saves edits one file at a time, reporting progress. A file that fails doesn't stop the others.
#[tauri::command]
pub async fn metadata_save(edits: Vec<MetadataEdit>, state: AppStateRef<'_>) -> CommandResult<Vec<SaveOutcome>> {
    let keep_dates = state.settings().keep_file_dates;
    let tools = tools(&state);
    let total = edits.len();
    let mut outcomes = Vec::with_capacity(total);
    for (index, edit) in edits.into_iter().enumerate() {
        let _ = state.handle.emit(events::METADATA_PROGRESS, SaveProgress { done: index, total });
        let result = async {
            let path = known(&state, &edit.path)?;
            let change = change_for(&state, &edit)?;
            let saved = tags::save(&path, change, &tools, keep_dates).await.map_err(payload)?;
            if saved != path {
                let mut known = state.metadata_files.lock().unwrap();
                known.remove(&path);
                known.insert(saved.clone());
            }
            tags::read(&saved, &tools).await.map_err(payload)
        }
        .await;
        outcomes.push(match result {
            Ok(file) => SaveOutcome { path: edit.path, file: Some(file), error: None },
            Err(error) => {
                // Show the file as it really is, so the window doesn't keep a value that wasn't written.
                let file = match PathBuf::from(&edit.path) {
                    p if p.is_file() => tags::read(&p, &tools).await.ok(),
                    _ => None,
                };
                SaveOutcome { path: edit.path, file, error: Some(error) }
            }
        });
    }
    let _ = state.handle.emit(events::METADATA_PROGRESS, SaveProgress { done: total, total });
    Ok(outcomes)
}

/// A small copy of a picture for the window, as a data URL.
fn preview(data: &[u8], size: u32) -> Option<String> {
    let picture = image::load_from_memory(data).ok()?;
    let small = picture.thumbnail(size, size);
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(small.to_rgb8()).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg).ok()?;
    crate::artwork::data_url(&out)
}

/// A file's artwork, scaled down for display.
#[tauri::command]
pub async fn metadata_cover(path: String, state: AppStateRef<'_>) -> CommandResult<Option<String>> {
    let path = known(&state, &path)?;
    Ok(tauri::async_runtime::spawn_blocking(move || tags::cover_data(&path).and_then(|data| preview(&data, 360))).await.unwrap_or(None))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverChoice {
    pub id: String,
    pub preview: Option<String>,
    pub summary: Option<String>,
}

/// Lets the user pick a picture to use as artwork. It's kept here until saved; the window only gets an ID.
#[tauri::command]
pub async fn metadata_choose_cover(app: AppHandle, state: AppStateRef<'_>) -> CommandResult<Option<CoverChoice>> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Choose artwork")
            .add_filter("Pictures", &["jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| simple("Couldn't open the file picker", e.to_string()))?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let image = tauri::async_runtime::spawn_blocking(move || -> CommandResult<tags::CoverImage> {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if size > 40 * 1024 * 1024 {
            return Err(simple("Picture Too Large", "Choose a picture smaller than 40 MB."));
        }
        let data = std::fs::read(&path).map_err(|e| simple("Couldn't read the picture", e.to_string()))?;
        tags::prepare_cover(&data).map_err(payload)
    })
    .await
    .map_err(|e| simple("Couldn't read the picture", e.to_string()))??;
    let id = uuid::Uuid::new_v4().to_string();
    let choice = CoverChoice { id: id.clone(), preview: preview(&image.data, 360), summary: tags::picture_summary(&image.data) };
    let mut covers = state.covers.lock().unwrap();
    // Only the most recent few are kept: each is a full-size picture.
    if covers.len() >= 20 {
        covers.clear();
    }
    covers.insert(id, image);
    Ok(Some(choice))
}

/// Saves a file's artwork as a picture of its own.
#[tauri::command]
pub async fn metadata_export_cover(path: String, app: AppHandle, state: AppStateRef<'_>) -> CommandResult<bool> {
    let path = known(&state, &path)?;
    let data = tauri::async_runtime::spawn_blocking({
        let path = path.clone();
        move || tags::cover_data(&path)
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| simple("No Artwork", "This file has no artwork to save."))?;
    let extension = if crate::artwork::image_mime_type(&data) == Some("image/png") { "png" } else { "jpg" };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Cover".into());
    let folder = path.parent().map(Path::to_path_buf);
    let target = tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Save artwork")
            .set_file_name(format!("{stem} cover.{extension}"))
            .add_filter("Picture", &[extension]);
        if let Some(folder) = folder {
            dialog = dialog.set_directory(folder);
        }
        dialog.blocking_save_file()
    })
    .await
    .map_err(|e| simple("Couldn't open the save dialog", e.to_string()))?;
    let Some(target) = target.and_then(|p| p.into_path().ok()) else { return Ok(false) };
    std::fs::write(&target, data).map_err(|e| simple("Couldn't save the artwork", e.to_string()))?;
    Ok(true)
}
