//! Turns untrusted text (remote titles, mostly) into names that are safe to write to disk on macOS and Windows.

use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Both APFS and NTFS allow 255 units per name; leave room for an extension and a " 99" suffix.
pub const MAX_BASE_NAME_BYTES: usize = 200;
pub const FALLBACK_NAME: &str = "Untitled";

const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4",
    "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters that are illegal or troublesome on macOS, Windows or in shells.
fn replacement(c: char) -> Option<&'static str> {
    Some(match c {
        '/' | '\\' | '|' => "-",
        ':' => " -",
        '?' | '*' => "",
        '"' => "'",
        '<' => "(",
        '>' => ")",
        _ => return None,
    })
}

/// Returns a single path component with no separators, control characters or leading dots.
pub fn sanitize(name: &str) -> String {
    sanitize_with_fallback(name, FALLBACK_NAME)
}

pub fn sanitize_with_fallback(name: &str, fallback: &str) -> String {
    let mut output = String::with_capacity(name.len());
    for c in name.nfc() {
        if let Some(replacement) = replacement(c) {
            output.push_str(replacement);
        } else if c == '\n' || c == '\r' || c == '\t' || c == '\u{2028}' || c == '\u{2029}' {
            output.push(' ');
        } else if c == '\u{200D}' && output.chars().last().is_some_and(|last| !last.is_ascii()) {
            // A zero-width joiner inside an emoji sequence (👨‍👩‍👧) is part of the character, not a disguise.
            output.push(c);
        } else if is_unsafe_char(c) {
            continue;
        } else {
            output.push(c);
        }
    }

    // Collapse runs of whitespace so "A  /  B" doesn't become "A  -  B".
    let mut output = output.split_whitespace().collect::<Vec<_>>().join(" ");
    output = trim_edges(&output);
    output = truncate(&output, MAX_BASE_NAME_BYTES);
    output = trim_edges(&output);

    if output.is_empty() {
        return fallback.to_string();
    }
    let stem = output.split('.').next().unwrap_or(&output).to_uppercase();
    if RESERVED_DEVICE_NAMES.contains(&stem.as_str()) {
        output.insert(0, '_');
    }
    output
}

pub fn sanitize_extension(ext: &str) -> String {
    ext.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).take(10).collect()
}

/// Picks a name in `directory` that doesn't exist yet, Finder style: "Name.mp3", "Name 2.mp3", …
pub fn unique_path(directory: &Path, base_name: &str, extension: &str) -> PathBuf {
    let base = sanitize(base_name);
    let ext = sanitize_extension(extension);
    let candidate = |index: usize| {
        let name = if index <= 1 { base.clone() } else { format!("{base} {index}") };
        directory.join(if ext.is_empty() { name } else { format!("{name}.{ext}") })
    };
    let mut index = 1;
    while candidate(index).symlink_metadata().is_ok() && index < 10_000 {
        index += 1;
    }
    candidate(index)
}

fn is_unsafe_char(c: char) -> bool {
    let value = c as u32;
    value < 0x20
        || value == 0x7F
        || (0x80..=0x9F).contains(&value)
        || (0x200B..=0x200F).contains(&value) // zero-width and direction marks
        || (0x202A..=0x202E).contains(&value) // bidirectional overrides
        || (0x2066..=0x2069).contains(&value)
        || value == 0xFEFF
}

/// Leading dots hide files, leading dashes look like options, trailing dots and spaces break on Windows.
fn trim_edges(text: &str) -> String {
    text.trim_start_matches(|c: char| c == '.' || c == '-' || c == '~' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace())
        .to_string()
}

/// Cuts on a grapheme boundary so an accented letter or an emoji is never split.
fn truncate(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut result = String::new();
    for grapheme in text.graphemes(true) {
        if result.len() + grapheme.len() > limit {
            break;
        }
        result.push_str(grapheme);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_path_separators_so_a_title_can_never_escape_its_folder() {
        for (input, expected) in [
            ("../../etc/passwd", "etc-passwd"),
            ("a/b\\c", "a-b-c"),
            ("/absolute/path", "absolute-path"),
            ("..", "Untitled"),
            ("...hidden", "hidden"),
            ("C:\\Windows\\System32", "C --Windows-System32"),
        ] {
            let result = sanitize(input);
            assert_eq!(result, expected, "{input}");
            assert!(!result.contains('/') && !result.contains('\\') && !result.starts_with('.'));
        }
    }

    #[test]
    fn replaces_characters_that_are_illegal_on_macos_or_windows() {
        assert_eq!(sanitize("Episode 1: The \"Start\" <HD> | Part?*"), "Episode 1 - The 'Start' (HD) - Part");
        assert_eq!(sanitize("Line one\nLine two\tTabbed"), "Line one Line two Tabbed");
    }

    #[test]
    fn strips_control_and_invisible_direction_characters() {
        assert_eq!(sanitize("Bad\u{0}Name\u{7}"), "BadName");
        assert_eq!(sanitize("invoice\u{202E}3pm.exe"), "invoice3pm.exe");
        assert_eq!(sanitize("zero\u{200B}width"), "zerowidth");
        assert_eq!(sanitize("join\u{200D}ed"), "joined");
        assert_eq!(sanitize("👨\u{200D}👩"), "👨\u{200D}👩");
    }

    #[test]
    fn never_returns_an_empty_hidden_or_option_like_name() {
        assert_eq!(sanitize(""), "Untitled");
        assert_eq!(sanitize("   "), "Untitled");
        assert_eq!(sanitize("///"), "Untitled");
        assert_eq!(sanitize("-rf"), "rf");
        assert_eq!(sanitize("~root"), "root");
        assert_eq!(sanitize("trailing dots...  "), "trailing dots");
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("nul.mp3"), "_nul.mp3");
        assert_eq!(sanitize("Lpt1"), "_Lpt1");
    }

    #[test]
    fn keeps_international_text_and_emoji_intact() {
        assert_eq!(sanitize("Café Müller – 東京 🎧"), "Café Müller – 東京 🎧");
        // Decomposed input is composed, so the same title always gives the same name.
        assert_eq!(sanitize("Cafe\u{301}"), "Café");
    }

    #[test]
    fn truncates_long_names_on_a_character_boundary() {
        let long = format!("{}🎧", "é".repeat(400));
        let result = sanitize(&long);
        assert!(result.len() <= MAX_BASE_NAME_BYTES);
        assert!(result.chars().all(|c| c == 'é'));
        let family = "👨‍👩‍👧".repeat(60);
        let result = sanitize(&family);
        assert!(result.len() <= MAX_BASE_NAME_BYTES);
        assert_eq!(result.graphemes(true).count() * "👨‍👩‍👧".len(), result.len());
    }

    #[test]
    fn sanitises_extensions() {
        assert_eq!(sanitize_extension("MP3"), "mp3");
        assert_eq!(sanitize_extension("m4a/../x"), "m4ax");
        assert_eq!(sanitize_extension(""), "");
    }

    #[test]
    fn picks_a_free_name_instead_of_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let first = unique_path(dir.path(), "Track", "mp3");
        assert_eq!(first.file_name().unwrap(), "Track.mp3");
        std::fs::write(&first, "x").unwrap();
        let second = unique_path(dir.path(), "Track", "mp3");
        assert_eq!(second.file_name().unwrap(), "Track 2.mp3");
        std::fs::write(&second, "x").unwrap();
        let third = unique_path(dir.path(), "Track", "mp3");
        assert_eq!(third.file_name().unwrap(), "Track 3.mp3");
        assert_eq!(third.parent().unwrap(), dir.path());
    }

    #[test]
    fn a_hostile_title_still_lands_inside_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let path = unique_path(dir.path(), "../../../../tmp/evil", "mp3");
        assert_eq!(path.parent().unwrap(), dir.path());
        assert_eq!(path.file_name().unwrap(), "tmp-evil.mp3");
    }
}
