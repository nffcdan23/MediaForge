//! The metadata inside files, for the Metadata screen: tags in audio and video, EXIF and XMP in photos,
//! document properties in PDF and Office files, and, for every file, its name, dates and read-only flag.
//!
//! Each kind of file has its own reader and writer. They all write through `files`, which saves to a hidden
//! copy and swaps it in, so an interrupted save never leaves a damaged file. Nothing here knows about the
//! window; `app::commands` decides which files may be touched.

mod audio;
mod container;
mod document;
pub mod files;
mod photo;
mod xml;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagError {
    pub title: String,
    pub message: String,
}

impl TagError {
    pub fn new(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self { title: title.into(), message: message.into() }
    }
}

/// A piece of metadata the editor shows. Which ones a file has depends on its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Field {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Track,
    TrackTotal,
    Disc,
    DiscTotal,
    Year,
    Genre,
    Composer,
    Comment,
    Description,
    Copyright,
    Authors,
    Subject,
    Keywords,
    Category,
    Rating,
    DateTaken,
}

impl Field {
    fn is_number(self) -> bool {
        matches!(self, Field::Track | Field::TrackTotal | Field::Disc | Field::DiscTotal)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    Audio,
    Video,
    Image,
    Document,
    Other,
}

/// Embedded artwork.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cover {
    pub present: bool,
    /// Short hash of the picture, so the window can tell when several files share the same one.
    pub signature: Option<String>,
    /// "JPEG · 600 × 600".
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detail {
    pub label: String,
    pub value: String,
}

fn detail(label: &str, value: impl Into<String>) -> Detail {
    Detail { label: label.into(), value: value.into() }
}

/// A file as the Metadata screen shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagFile {
    pub path: String,
    pub name: String,
    pub kind: FileKind,
    /// "MP3 audio", "JPEG image", "Word document"…
    pub format: String,
    /// Where the fields live: "ID3v2.3", "EXIF and XMP", "Document properties". None when there's nothing yet.
    pub storage: Option<String>,
    pub size: u64,
    pub modified: Option<String>,
    pub created: Option<String>,
    pub read_only: bool,
    pub fields: BTreeMap<Field, String>,
    /// The fields this file can hold, in the order the editor shows them. Empty when only the name and dates
    /// can change.
    pub editable: Vec<Field>,
    /// Present when the file can hold artwork.
    pub cover: Option<Cover>,
    /// Facts about the content: length, bit rate, dimensions.
    pub details: Vec<Detail>,
    /// Why the fields can't be changed, when they can't.
    pub note: Option<String>,
    /// Changes whenever the file does, so cached artwork can be refreshed.
    pub revision: String,
}

/// What a reader found inside a file.
#[derive(Debug, Clone)]
struct Contents {
    kind: FileKind,
    format: String,
    storage: Option<String>,
    fields: BTreeMap<Field, String>,
    editable: Vec<Field>,
    cover: Option<Cover>,
    details: Vec<Detail>,
    note: Option<String>,
}

impl Contents {
    fn new(kind: FileKind, format: impl Into<String>) -> Self {
        Self {
            kind,
            format: format.into(),
            storage: None,
            fields: BTreeMap::new(),
            editable: Vec::new(),
            cover: None,
            details: Vec::new(),
            note: None,
        }
    }

    fn set(&mut self, field: Field, value: Option<impl Into<String>>) {
        if let Some(value) = value.map(Into::into).map(|v: String| v.trim_end_matches('\0').trim().to_string()).filter(|v| !v.is_empty()) {
            self.fields.insert(field, value);
        }
    }

    /// A file whose fields couldn't be read keeps its name and dates editable.
    fn unreadable(kind: FileKind, format: impl Into<String>, why: impl Into<String>) -> Self {
        let mut contents = Self::new(kind, format);
        contents.note = Some(why.into());
        contents
    }
}

/// The helper programs some formats need.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
}

/// Which code handles a file, decided by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handler {
    /// Audio tags through lofty: ID3, MP4, Vorbis comments, APE, RIFF INFO.
    Audio,
    /// MP4 and QuickTime video, whose tags lofty also reads and writes.
    Mp4Video,
    /// Matroska, WebM, AVI and ASF, rewritten by FFmpeg.
    Container(container::Muxer),
    Photo(photo::Format),
    Pdf,
    Office(document::OfficeKind),
    None,
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn handler(path: &Path) -> Handler {
    match extension(path).as_str() {
        "mp3" | "mp2" | "m4a" | "m4b" | "m4r" | "aac" | "flac" | "ogg" | "oga" | "opus" | "spx" | "wav" | "wave" | "aif" | "aiff"
        | "aifc" | "wv" | "ape" | "mpc" => Handler::Audio,
        "mp4" | "m4v" | "mov" | "3gp" | "3g2" => Handler::Mp4Video,
        "mkv" | "mka" => Handler::Container(container::Muxer::Matroska),
        "webm" => Handler::Container(container::Muxer::WebM),
        "avi" => Handler::Container(container::Muxer::Avi),
        "wmv" | "wma" | "asf" => Handler::Container(container::Muxer::Asf),
        "jpg" | "jpeg" | "jfif" | "jpe" => Handler::Photo(photo::Format::Jpeg),
        "png" => Handler::Photo(photo::Format::Png),
        "webp" => Handler::Photo(photo::Format::WebP),
        "tif" | "tiff" => Handler::Photo(photo::Format::Tiff),
        "heic" | "heif" | "hif" => Handler::Photo(photo::Format::Heif),
        "pdf" => Handler::Pdf,
        "docx" | "docm" | "dotx" | "dotm" => Handler::Office(document::OfficeKind::Word),
        "xlsx" | "xlsm" | "xltx" | "xltm" => Handler::Office(document::OfficeKind::Excel),
        "pptx" | "pptm" | "potx" | "potm" | "ppsx" => Handler::Office(document::OfficeKind::PowerPoint),
        _ => Handler::None,
    }
}

/// A name for files MediaForge has no reader for: "ZIP file", "File".
fn generic_format(path: &Path) -> (FileKind, String) {
    let ext = extension(path);
    let kind = if crate::engine::media_files::AUDIO_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::Audio
    } else if crate::engine::media_files::VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        FileKind::Video
    } else if crate::engine::media_files::IMAGE_EXTENSIONS.contains(&ext.as_str())
        || ["gif", "bmp", "svg", "avif", "ico"].contains(&ext.as_str())
    {
        FileKind::Image
    } else if ["doc", "xls", "ppt", "odt", "ods", "odp", "rtf", "txt", "md", "csv", "pages", "numbers", "key", "epub"]
        .contains(&ext.as_str())
    {
        FileKind::Document
    } else {
        FileKind::Other
    };
    let format = if ext.is_empty() { "File".to_string() } else { format!("{} file", ext.to_uppercase()) };
    (kind, format)
}

fn contents_blocking(path: &Path, handler: Handler) -> Contents {
    match handler {
        Handler::Audio => audio::read(path, FileKind::Audio),
        Handler::Mp4Video => audio::read(path, FileKind::Video),
        Handler::Photo(format) => photo::read(path, format),
        Handler::Pdf => document::read_pdf(path),
        Handler::Office(kind) => document::read_office(path, kind),
        Handler::Container(_) | Handler::None => {
            let (kind, format) = generic_format(path);
            Contents::new(kind, format)
        }
    }
}

/// Reads one file. Fails only when the file itself is gone; unreadable contents become a note.
pub async fn read(path: &Path, tools: &Tools) -> Result<TagFile, TagError> {
    let metadata = fs::metadata(path).map_err(|e| files::io_error("File not found", path, &e))?;
    if !metadata.is_file() {
        return Err(TagError::new("Not a file", format!("“{}” is a folder.", path.display())));
    }
    let handler = handler(path);
    let contents = match handler {
        Handler::Container(muxer) => container::read(path, muxer, tools).await,
        _ => {
            let owned = path.to_path_buf();
            tokio::task::spawn_blocking(move || contents_blocking(&owned, handler))
                .await
                .unwrap_or_else(|_| Contents::unreadable(FileKind::Other, "File", "MediaForge couldn't read this file."))
        }
    };
    let modified = metadata.modified().ok();
    let revision =
        format!("{}-{}", metadata.len(), modified.and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos()));
    Ok(TagFile {
        path: path.display().to_string(),
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        kind: contents.kind,
        format: contents.format,
        storage: contents.storage,
        size: metadata.len(),
        modified: modified.map(files::timestamp),
        created: files::created(&metadata).map(files::timestamp),
        read_only: metadata.permissions().readonly(),
        fields: contents.fields,
        editable: contents.editable,
        cover: contents.cover,
        details: contents.details,
        note: contents.note,
        revision,
    })
}

/// Reads many files, several at a time, in the order given. Files that have gone are left out.
pub async fn read_many(paths: Vec<PathBuf>, tools: &Tools) -> Vec<TagFile> {
    in_order(paths, 8, |path| {
        let tools = tools.clone();
        async move { read(&path, &tools).await.ok() }
    })
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// Runs `work` over `items` with at most `limit` running at once, keeping the results in order.
async fn in_order<T, F, Fut, R>(items: Vec<T>, limit: usize, work: F) -> Vec<R>
where
    T: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(limit.max(1)));
    let mut handles = Vec::with_capacity(items.len());
    for item in items {
        let permit = semaphore.clone();
        let future = work(item);
        handles.push(tokio::spawn(async move {
            let _permit = permit.acquire_owned().await;
            future.await
        }));
    }
    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        if let Ok(result) = handle.await {
            results.push(result);
        }
    }
    results
}

/// The embedded artwork of a file, for display or export.
pub fn cover_data(path: &Path) -> Option<Vec<u8>> {
    match handler(path) {
        Handler::Audio | Handler::Mp4Video => audio::cover(path),
        _ => None,
    }
}

/// New artwork, already checked and in a format players accept.
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub data: Vec<u8>,
    pub mime: &'static str,
}

#[derive(Debug, Clone)]
pub enum CoverChange {
    Set(CoverImage),
    Remove,
}

/// Everything to change in one file. Fields with an empty value are removed.
#[derive(Debug, Clone, Default)]
pub struct Change {
    pub fields: BTreeMap<Field, String>,
    pub cover: Option<CoverChange>,
    /// New name without the extension.
    pub name: Option<String>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub read_only: Option<bool>,
}

impl Change {
    fn touches_contents(&self) -> bool {
        !self.fields.is_empty() || self.cover.is_some()
    }
}

/// Checks values before anything is written.
pub fn validate(fields: &BTreeMap<Field, String>) -> Result<(), TagError> {
    for (field, value) in fields {
        let value = value.trim();
        if value.chars().count() > 20_000 {
            return Err(TagError::new("Too long", "A field can hold at most 20,000 characters."));
        }
        if value.is_empty() {
            continue;
        }
        if field.is_number() && !value.parse::<u32>().is_ok_and(|n| n > 0 && n < 100_000) {
            return Err(TagError::new("Not a number", format!("Track and disc numbers must be whole numbers, not “{value}”.")));
        }
        if *field == Field::Rating && !value.parse::<u8>().is_ok_and(|n| n <= 5) {
            return Err(TagError::new("Invalid rating", "A rating is from 0 to 5 stars."));
        }
        if *field == Field::DateTaken && photo::parse_date_taken(value).is_none() {
            return Err(TagError::new("Invalid date", format!("“{value}” isn't a date and time.")));
        }
    }
    Ok(())
}

/// Applies a change to one file and returns where it is now (renaming moves it).
pub async fn save(path: &Path, change: Change, tools: &Tools, keep_dates: bool) -> Result<PathBuf, TagError> {
    validate(&change.fields)?;
    let metadata = fs::metadata(path).map_err(|e| files::io_error("File not found", path, &e))?;
    let was_read_only = metadata.permissions().readonly();
    if change.touches_contents() {
        if was_read_only && change.read_only != Some(false) {
            return Err(TagError::new("The file is read-only", "Untick Read-only to change what's inside it, then save again."));
        }
        if was_read_only {
            files::set_read_only(path, false)?;
        }
        let modified_before = metadata.modified().ok();
        let handler = handler(path);
        match handler {
            Handler::Container(muxer) => container::write(path, muxer, &change.fields, tools).await?,
            Handler::None => return Err(TagError::new("Nothing to change", "This kind of file has no tags of its own.")),
            _ => {
                let owned = path.to_path_buf();
                let fields = change.fields.clone();
                let cover = change.cover.clone();
                tokio::task::spawn_blocking(move || write_blocking(&owned, handler, &fields, cover.as_ref()))
                    .await
                    .map_err(|_| TagError::new("Couldn't save the changes", "Something went wrong while writing the file."))??;
            }
        }
        if keep_dates && change.modified.is_none() {
            files::set_times(path, modified_before, None)?;
        }
    } else if was_read_only && change.read_only == Some(false) {
        files::set_read_only(path, false)?;
    }

    if change.modified.is_some() || change.created.is_some() {
        if was_read_only && change.read_only != Some(false) {
            return Err(TagError::new("The file is read-only", "Untick Read-only to change its dates, then save again."));
        }
        files::set_times(path, change.modified, change.created)?;
    }
    let path = match change.name.as_deref() {
        Some(name) => files::rename(path, name)?,
        None => path.to_path_buf(),
    };
    if change.read_only == Some(true) {
        files::set_read_only(&path, true)?;
    }
    Ok(path)
}

fn write_blocking(path: &Path, handler: Handler, fields: &BTreeMap<Field, String>, cover: Option<&CoverChange>) -> Result<(), TagError> {
    match handler {
        Handler::Audio | Handler::Mp4Video => audio::write(path, fields, cover),
        Handler::Photo(format) => photo::write(path, format, fields),
        Handler::Pdf => document::write_pdf(path, fields),
        Handler::Office(_) => document::write_office(path, fields),
        Handler::Container(_) | Handler::None => Ok(()),
    }
}

/// Prepares a picture the user chose as artwork. JPEG and PNG pictures of a sensible size are used as they
/// are; anything else (WebP, a huge photo, a progressive JPEG some players can't show) becomes a baseline
/// JPEG at most 1,500 pixels across.
pub fn prepare_cover(data: &[u8]) -> Result<CoverImage, TagError> {
    use image::ImageReader;
    let unreadable = || TagError::new("Can't use this picture", "Choose a JPEG, PNG or WebP picture.");
    let reader = ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().map_err(|_| unreadable())?;
    let format = reader.format().ok_or_else(unreadable)?;
    let (width, height) = reader.into_dimensions().map_err(|_| unreadable())?;
    const LIMIT: u32 = 3_000;
    let keep = match format {
        image::ImageFormat::Jpeg => width <= LIMIT && height <= LIMIT && !is_progressive_jpeg(data),
        image::ImageFormat::Png => width <= LIMIT && height <= LIMIT && data.len() <= 4 * 1024 * 1024,
        _ => false,
    };
    if keep {
        let mime = if format == image::ImageFormat::Png { "image/png" } else { "image/jpeg" };
        return Ok(CoverImage { data: data.to_vec(), mime });
    }
    let mut decoder = ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().map_err(|_| unreadable())?;
    decoder.limits(image::Limits::default());
    let picture = decoder.decode().map_err(|_| unreadable())?;
    let picture =
        if width > 1_500 || height > 1_500 { picture.resize(1_500, 1_500, image::imageops::FilterType::Lanczos3) } else { picture };
    let rgb = image::DynamicImage::ImageRgb8(flatten_on_white(&picture));
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode_image(&rgb).map_err(|_| unreadable())?;
    Ok(CoverImage { data: out, mime: "image/jpeg" })
}

fn flatten_on_white(picture: &image::DynamicImage) -> image::RgbImage {
    let rgba = picture.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend = |c: u8| ((c as u16 * a as u16 + 255 * (255 - a as u16)) / 255) as u8;
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

/// True when a JPEG's frame is progressive (SOF2), which some car stereos and older players can't show.
fn is_progressive_jpeg(data: &[u8]) -> bool {
    let mut index = 2;
    while index + 4 <= data.len() {
        if data[index] != 0xFF {
            return false;
        }
        let marker = data[index + 1];
        if marker == 0xFF {
            index += 1;
            continue;
        }
        if matches!(marker, 0xC2 | 0xC6 | 0xCA | 0xCE) {
            return true;
        }
        if matches!(marker, 0xC0 | 0xC1 | 0xC3 | 0xDA | 0xD9) {
            return false;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        index += 2 + length;
    }
    false
}

/// Short identity for a picture, so identical artwork across files can be recognised.
fn signature(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// "JPEG · 600 × 600".
pub fn picture_summary(data: &[u8]) -> Option<String> {
    let reader = image::ImageReader::new(std::io::Cursor::new(data)).with_guessed_format().ok()?;
    let format = reader.format().map(|f| match f {
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::WebP => "WebP",
        image::ImageFormat::Gif => "GIF",
        image::ImageFormat::Bmp => "BMP",
        _ => "Picture",
    })?;
    match reader.into_dimensions() {
        Ok((w, h)) => Some(format!("{format} · {w} × {h}")),
        Err(_) => Some(format.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_serialise_by_name() {
        let mut fields = BTreeMap::new();
        fields.insert(Field::AlbumArtist, "Someone".to_string());
        assert_eq!(serde_json::to_string(&fields).unwrap(), r#"{"albumArtist":"Someone"}"#);
        let parsed: BTreeMap<Field, String> = serde_json::from_str(r#"{"trackTotal":"12","dateTaken":""}"#).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn numbers_ratings_and_dates_are_checked() {
        let check = |field, value: &str| validate(&BTreeMap::from([(field, value.to_string())]));
        assert!(check(Field::Track, "3").is_ok());
        assert!(check(Field::Track, "").is_ok());
        assert!(check(Field::Track, "three").is_err());
        assert!(check(Field::Track, "0").is_err());
        assert!(check(Field::Rating, "5").is_ok());
        assert!(check(Field::Rating, "6").is_err());
        assert!(check(Field::DateTaken, "2024-05-01T10:20:30").is_ok());
        assert!(check(Field::DateTaken, "yesterday").is_err());
        assert!(check(Field::Year, "1999").is_ok());
    }

    #[test]
    fn handlers_follow_the_extension() {
        assert_eq!(handler(Path::new("a/Song.MP3")), Handler::Audio);
        assert_eq!(handler(Path::new("clip.mov")), Handler::Mp4Video);
        assert_eq!(handler(Path::new("film.mkv")), Handler::Container(container::Muxer::Matroska));
        assert_eq!(handler(Path::new("photo.jpeg")), Handler::Photo(photo::Format::Jpeg));
        assert_eq!(handler(Path::new("report.docx")), Handler::Office(document::OfficeKind::Word));
        assert_eq!(handler(Path::new("notes.txt")), Handler::None);
        assert_eq!(generic_format(Path::new("archive.zip")), (FileKind::Other, "ZIP file".to_string()));
    }

    #[test]
    fn covers_are_converted_only_when_needed() {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(40, 30, image::Rgba([200, 10, 10, 128])))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let kept = prepare_cover(&png).unwrap();
        assert_eq!((kept.mime, kept.data.len()), ("image/png", png.len()));

        let mut webp = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(40, 30, image::Rgb([1, 2, 3])))
            .write_to(&mut std::io::Cursor::new(&mut webp), image::ImageFormat::WebP)
            .unwrap();
        let converted = prepare_cover(&webp).unwrap();
        assert_eq!(converted.mime, "image/jpeg");
        assert!(!is_progressive_jpeg(&converted.data));
        assert_eq!(picture_summary(&converted.data).as_deref(), Some("JPEG · 40 × 30"));
        assert!(prepare_cover(b"not a picture").is_err());
    }
}
