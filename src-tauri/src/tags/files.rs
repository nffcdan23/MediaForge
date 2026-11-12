//! What every file has, whatever is inside it: a name, dates and a read-only flag. Also the safe way to
//! change a file's contents: write a hidden copy beside it, then swap the copy in with one rename, so an
//! interrupted save leaves either the old file or the new one, never half of each.

use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Utc};

use super::TagError;
use crate::support::filename;

/// Most files a folder is allowed to add, so dropping a whole drive can't stall the window.
pub const MAX_FILES: usize = 2_000;

fn is_hidden(path: &Path) -> bool {
    let hidden_name = path.file_name().is_some_and(|n| {
        let name = n.to_string_lossy();
        name.starts_with('.')
            || name.starts_with("~$")
            || name.eq_ignore_ascii_case("desktop.ini")
            || name.eq_ignore_ascii_case("thumbs.db")
    });
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        let attributes = fs::symlink_metadata(path).map(|m| m.file_attributes()).unwrap_or(0);
        hidden_name || attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
    }
    #[cfg(not(windows))]
    {
        hidden_name
    }
}

/// Flattens files and folders into the files they contain, folders read in name order, hidden files left out.
pub fn collect(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if result.len() >= MAX_FILES {
            break;
        }
        let Ok(metadata) = fs::metadata(path) else { continue };
        if metadata.is_dir() {
            let mut found = Vec::new();
            walk(path, &mut found, 0);
            found.sort_by(|a, b| crate::engine::media_files::natural_compare(&a.to_string_lossy(), &b.to_string_lossy()));
            for file in found {
                if result.len() >= MAX_FILES {
                    break;
                }
                if seen.insert(file.clone()) {
                    result.push(file);
                }
            }
        } else if metadata.is_file() {
            let file = std::path::absolute(path).unwrap_or_else(|_| path.clone());
            if seen.insert(file.clone()) {
                result.push(file);
            }
        }
    }
    result
}

fn walk(directory: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 || out.len() >= MAX_FILES * 2 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_hidden(&path) {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_symlink() {
            continue;
        } else if kind.is_dir() {
            if !cfg!(target_os = "macos") || !crate::engine::media_files::is_package_directory(&path) {
                walk(&path, out, depth + 1);
            }
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

pub fn timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn parse_timestamp(text: &str) -> Result<SystemTime, TagError> {
    DateTime::parse_from_rfc3339(text.trim())
        .map(|d| SystemTime::from(d.with_timezone(&Utc)))
        .map_err(|_| TagError::new("Invalid date", format!("“{text}” isn't a date and time MediaForge understands.")))
}

pub fn created(metadata: &fs::Metadata) -> Option<SystemTime> {
    metadata.created().ok()
}

/// Sets a file's modified and created dates. Created dates exist on Windows and macOS only.
pub fn set_times(path: &Path, modified: Option<SystemTime>, created: Option<SystemTime>) -> Result<(), TagError> {
    if modified.is_none() && created.is_none() {
        return Ok(());
    }
    let file = fs::OpenOptions::new().write(true).open(path).map_err(|e| io_error("Couldn't change the dates", path, &e))?;
    let mut times = FileTimes::new();
    if let Some(modified) = modified {
        times = times.set_modified(modified);
    }
    if let Some(created) = created {
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTimesExt;
            times = times.set_created(created);
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::macos::fs::FileTimesExt;
            times = times.set_created(created);
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        let _ = created;
    }
    file.set_times(times).map_err(|e| io_error("Couldn't change the dates", path, &e))
}

pub fn set_read_only(path: &Path, read_only: bool) -> Result<(), TagError> {
    let mut permissions = fs::metadata(path).map_err(|e| io_error("Couldn't change Read-only", path, &e))?.permissions();
    if permissions.readonly() == read_only {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = permissions.mode();
        // Read-only means nobody may write; making it writable again gives the owner write access only.
        permissions.set_mode(if read_only { mode & !0o222 } else { mode | 0o200 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(read_only);
    fs::set_permissions(path, permissions).map_err(|e| io_error("Couldn't change Read-only", path, &e))
}

/// Renames a file in its folder, keeping its extension. `base_name` is cleaned of characters no file system
/// allows. Returns the new path; an existing file of that name is never replaced.
pub fn rename(path: &Path, base_name: &str) -> Result<PathBuf, TagError> {
    let wanted = base_name.trim();
    if wanted.is_empty() {
        return Err(TagError::new("Name needed", "A file name can't be empty."));
    }
    let base = filename::sanitize(wanted);
    let extension = path.extension().map(|e| e.to_string_lossy().into_owned());
    let name = match &extension {
        Some(extension) if !extension.is_empty() => format!("{base}.{extension}"),
        _ => base,
    };
    let parent = path.parent().ok_or_else(|| TagError::new("Can't rename", "The file's folder couldn't be found."))?;
    let target = parent.join(&name);
    if target == path {
        return Ok(target);
    }
    // A change of capitals only ("song.mp3" to "Song.mp3") is the same file on Windows and macOS.
    let same_file = target.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase();
    if target.exists() && !same_file {
        return Err(TagError::new("Name already used", format!("There's already a file called “{name}” in that folder.")));
    }
    fs::rename(path, &target).map_err(|e| io_error("Couldn't rename the file", path, &e))?;
    Ok(target)
}

/// A hidden name beside `original` for its replacement. OneDrive and similar services skip names starting
/// "~$", so the half-written copy is never uploaded.
pub fn temporary_sibling(original: &Path) -> PathBuf {
    let stem: String = original.file_name().map(|n| n.to_string_lossy().chars().take(60).collect()).unwrap_or_default();
    let unique = uuid::Uuid::new_v4().simple().to_string();
    original.with_file_name(format!("~${stem}.{}.mediaforge.tmp", &unique[..8]))
}

/// Removes a temporary file when dropped, unless it has been used.
pub struct TemporaryFile {
    pub path: PathBuf,
    keep: bool,
}

impl TemporaryFile {
    pub fn beside(original: &Path) -> Self {
        Self { path: temporary_sibling(original), keep: false }
    }

    /// Swaps this file in for `original`.
    pub fn replace(mut self, original: &Path) -> Result<(), TagError> {
        replace(original, &self.path)?;
        self.keep = true;
        Ok(())
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// A copy of `original` beside it, for libraries that edit a file in place.
pub fn copy_beside(original: &Path) -> Result<TemporaryFile, TagError> {
    let temporary = TemporaryFile::beside(original);
    fs::copy(original, &temporary.path).map_err(|e| io_error("Couldn't save the changes", original, &e))?;
    Ok(temporary)
}

/// Writes `bytes` as the new contents of `original`.
pub fn replace_with_bytes(original: &Path, bytes: &[u8]) -> Result<(), TagError> {
    let temporary = TemporaryFile::beside(original);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary.path)
        .map_err(|e| io_error("Couldn't save the changes", original, &e))?;
    file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|e| io_error("Couldn't save the changes", original, &e))?;
    drop(file);
    #[cfg(unix)]
    if let Ok(metadata) = fs::metadata(original) {
        let _ = fs::set_permissions(&temporary.path, metadata.permissions());
    }
    temporary.replace(original)
}

/// Puts `replacement` in the place of `original`, keeping the original's creation date (and on Windows its
/// security settings and other streams, which `ReplaceFileW` carries over).
fn replace(original: &Path, replacement: &Path) -> Result<(), TagError> {
    if let Ok(file) = File::open(replacement) {
        let _ = file.sync_all();
    }
    let created = fs::metadata(original).ok().and_then(|m| created(&m));
    #[cfg(windows)]
    {
        if replace_file_windows(original, replacement) {
            return Ok(());
        }
    }
    fs::rename(replacement, original).map_err(|e| io_error("Couldn't save the changes", original, &e))?;
    if cfg!(target_os = "macos")
        && let Some(created) = created
    {
        let _ = set_times(original, None, Some(created));
    }
    #[cfg(not(target_os = "macos"))]
    let _ = created;
    Ok(())
}

#[cfg(windows)]
fn replace_file_windows(original: &Path, replacement: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{REPLACEFILE_IGNORE_ACL_ERRORS, REPLACEFILE_IGNORE_MERGE_ERRORS, ReplaceFileW};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let original = wide(original);
    let replacement = wide(replacement);
    unsafe {
        ReplaceFileW(
            original.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
            std::ptr::null(),
            std::ptr::null(),
        ) != 0
    }
}

pub fn io_error(title: &str, path: &Path, error: &std::io::Error) -> TagError {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let message = match error.kind() {
        std::io::ErrorKind::NotFound => format!("“{name}” has been moved or deleted."),
        std::io::ErrorKind::PermissionDenied => {
            format!("MediaForge isn't allowed to change “{name}”. It may be read-only, open in another app, or in a protected folder.")
        }
        std::io::ErrorKind::StorageFull => "There isn't enough free space on the disk.".to_string(),
        _ => format!("“{name}”: {error}"),
    };
    TagError::new(title, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_keeps_the_name_and_leaves_no_temporary_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.txt");
        fs::write(&path, b"old").unwrap();
        replace_with_bytes(&path, b"new contents").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new contents");
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_edit_leaves_the_original_alone() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("song.txt");
        fs::write(&path, b"old").unwrap();
        {
            let copy = copy_beside(&path).unwrap();
            fs::write(&copy.path, b"half written").unwrap();
            // Dropped without replace(): the edit failed.
        }
        assert_eq!(fs::read(&path).unwrap(), b"old");
        assert_eq!(fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn renaming_keeps_the_extension_and_refuses_to_overwrite() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("track01.mp3");
        fs::write(&path, b"a").unwrap();
        fs::write(folder.path().join("Taken.mp3"), b"b").unwrap();
        let renamed = rename(&path, "Artist - Title: Live?").unwrap();
        assert_eq!(renamed.file_name().unwrap(), "Artist - Title - Live.mp3");
        assert!(rename(&renamed, "Taken").is_err());
        let recased = rename(&renamed, "ARTIST - Title - Live").unwrap();
        assert_eq!(recased.file_name().unwrap(), "ARTIST - Title - Live.mp3");
    }

    #[test]
    fn dates_and_read_only_can_be_set() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("file.bin");
        fs::write(&path, b"x").unwrap();
        let when = parse_timestamp("2001-02-03T04:05:06Z").unwrap();
        set_times(&path, Some(when), None).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), when);
        assert_eq!(timestamp(when), "2001-02-03T04:05:06Z");
        set_read_only(&path, true).unwrap();
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        set_read_only(&path, false).unwrap();
        assert!(!fs::metadata(&path).unwrap().permissions().readonly());
    }

    #[test]
    fn folders_are_read_in_name_order_without_hidden_files() {
        let folder = tempfile::tempdir().unwrap();
        for name in ["10 b.txt", "2 a.txt", ".hidden", "~$lock.docx"] {
            fs::write(folder.path().join(name), b"x").unwrap();
        }
        fs::create_dir(folder.path().join("inner")).unwrap();
        fs::write(folder.path().join("inner").join("c.txt"), b"x").unwrap();
        let names: Vec<String> =
            collect(&[folder.path().to_path_buf()]).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["2 a.txt", "10 b.txt", "c.txt"]);
    }
}
