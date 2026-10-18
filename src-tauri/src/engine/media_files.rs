//! Decides which local files can be offered for conversion.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const MAX_IMPORT_COUNT: usize = 500;

pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "wav", "wave", "aac", "m4a", "m4b", "flac", "aiff", "aif", "aifc", "caf", "ogg", "oga", "opus", "wma", "mka", "ac3", "eac3",
    "amr", "alac", "ape", "wv", "dsf",
];

pub const VIDEO_EXTENSIONS: &[&str] =
    &["mp4", "m4v", "mov", "mkv", "webm", "avi", "flv", "wmv", "ts", "mts", "m2ts", "3gp", "mpg", "mpeg", "mxf", "ogv"];

/// Folders macOS presents as single documents; their insides are never media to convert.
const PACKAGE_EXTENSIONS: &[&str] = &[
    "app",
    "bundle",
    "framework",
    "plugin",
    "kext",
    "photoslibrary",
    "fcpbundle",
    "logicx",
    "band",
    "musiclibrary",
    "imovielibrary",
    "tvlibrary",
    "rtfd",
    "pages",
    "numbers",
    "key",
];

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub fn is_supported(path: &Path) -> bool {
    let ext = extension(path);
    AUDIO_EXTENSIONS.contains(&ext.as_str()) || VIDEO_EXTENSIONS.contains(&ext.as_str())
}

pub fn is_audio(path: &Path) -> bool {
    AUDIO_EXTENSIONS.contains(&extension(path).as_str())
}

fn is_hidden(path: &Path) -> bool {
    let hidden_name = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        let attributes = std::fs::symlink_metadata(path).map(|m| m.file_attributes()).unwrap_or(0);
        hidden_name || attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0
    }
    #[cfg(not(windows))]
    {
        hidden_name
    }
}

fn is_package(path: &Path) -> bool {
    cfg!(target_os = "macos") && PACKAGE_EXTENSIONS.contains(&extension(path).as_str())
}

/// Flattens a mix of files and folders into the media files they contain, in a stable order.
pub fn collect(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut add = |path: &Path, result: &mut Vec<PathBuf>| {
        if result.len() >= MAX_IMPORT_COUNT || !is_supported(path) {
            return;
        }
        let normalized = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if seen.insert(normalized.clone()) {
            result.push(normalized);
        }
    };

    for path in paths {
        let Ok(metadata) = std::fs::metadata(path) else { continue };
        if metadata.is_dir() {
            let mut children = Vec::new();
            walk(path, &mut children, MAX_IMPORT_COUNT * 4);
            children.sort_by(|a, b| natural_compare(&a.to_string_lossy(), &b.to_string_lossy()));
            for child in children {
                add(&child, &mut result);
            }
        } else if metadata.is_file() {
            add(path, &mut result);
        }
    }
    result
}

fn walk(directory: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        if is_hidden(&path) {
            continue;
        }
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if !is_package(&path) {
                walk(&path, out, limit);
            }
        } else if kind.is_file() {
            out.push(path);
        }
    }
}

/// Orders names the way Finder and Explorer do: case-insensitively, with runs of digits compared as numbers.
pub fn natural_compare(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();
    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) if l.is_ascii_digit() && r.is_ascii_digit() => {
                let mut ln = String::new();
                while let Some(c) = left.peek().copied().filter(char::is_ascii_digit) {
                    ln.push(c);
                    left.next();
                }
                let mut rn = String::new();
                while let Some(c) = right.peek().copied().filter(char::is_ascii_digit) {
                    rn.push(c);
                    right.next();
                }
                let lt = ln.trim_start_matches('0');
                let rt = rn.trim_start_matches('0');
                let ordering = lt.len().cmp(&rt.len()).then_with(|| lt.cmp(rt));
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            (Some(l), Some(r)) => {
                let ordering = l.to_lowercase().cmp(r.to_lowercase());
                if ordering != Ordering::Equal {
                    return ordering;
                }
                left.next();
                right.next();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_media_ignores_everything_else_and_walks_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("Album").join("Disc 2")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        for name in [
            "Album/Track 10.flac",
            "Album/Track 2.flac",
            "Album/Disc 2/Track 1.wav",
            "Album/cover.jpg",
            ".hidden/secret.mp3",
            "notes.txt",
            "clip.MOV",
        ] {
            std::fs::write(root.join(name), "x").unwrap();
        }
        let found = collect(&[root.join("Album"), root.join("notes.txt"), root.join("clip.MOV"), root.join("clip.MOV")]);
        let names: Vec<String> = found.iter().map(|p| p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(names, ["Album/Disc 2/Track 1.wav", "Album/Track 2.flac", "Album/Track 10.flac", "clip.MOV"]);
    }

    #[test]
    fn recognises_supported_extensions_case_insensitively() {
        assert!(is_supported(Path::new("/a/b.MP3")));
        assert!(is_supported(Path::new("/a/b.webm")));
        assert!(!is_supported(Path::new("/a/b.exe")));
        assert!(!is_supported(Path::new("/a/mp3")));
        assert!(is_audio(Path::new("x.Flac")));
        assert!(!is_audio(Path::new("x.mp4")));
    }

    #[test]
    fn natural_order() {
        let mut names = vec!["Track 10", "track 2", "Track 1", "Track 02b", "Intro"];
        names.sort_by(|a, b| natural_compare(a, b));
        assert_eq!(names, ["Intro", "Track 1", "track 2", "Track 02b", "Track 10"]);
    }
}
