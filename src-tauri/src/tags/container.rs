//! Tags in Matroska, WebM, AVI and ASF (WMV, WMA) files. ffprobe reads them; FFmpeg writes a copy of the
//! file with every stream copied unchanged and the new tags, which then replaces the original.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::{Contents, Field, FileKind, TagError, Tools, detail, files};
use crate::media::ffmpeg::{PROTOCOL_WHITELIST, file_argument, probe_arguments};
use crate::process::{self, ProcessRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Muxer {
    Matroska,
    WebM,
    Avi,
    Asf,
}

impl Muxer {
    fn ffmpeg_name(self) -> &'static str {
        match self {
            Muxer::Matroska => "matroska",
            Muxer::WebM => "webm",
            Muxer::Avi => "avi",
            Muxer::Asf => "asf",
        }
    }

    fn fields(self) -> &'static [Field] {
        match self {
            Muxer::Matroska | Muxer::WebM => &[
                Field::Title,
                Field::Artist,
                Field::Album,
                Field::AlbumArtist,
                Field::Track,
                Field::Year,
                Field::Genre,
                Field::Comment,
                Field::Description,
                Field::Copyright,
            ],
            Muxer::Avi => {
                &[Field::Title, Field::Artist, Field::Album, Field::Track, Field::Year, Field::Genre, Field::Comment, Field::Copyright]
            }
            Muxer::Asf => &[
                Field::Title,
                Field::Artist,
                Field::Album,
                Field::AlbumArtist,
                Field::Track,
                Field::Year,
                Field::Genre,
                Field::Composer,
                Field::Comment,
                Field::Copyright,
            ],
        }
    }
}

/// FFmpeg's name for a field. Keys are matched without regard to case, so a Matroska "ARTIST" is "artist".
fn key(field: Field) -> Option<&'static str> {
    Some(match field {
        Field::Title => "title",
        Field::Artist => "artist",
        Field::Album => "album",
        Field::AlbumArtist => "album_artist",
        Field::Track => "track",
        Field::Year => "date",
        Field::Genre => "genre",
        Field::Composer => "composer",
        Field::Comment => "comment",
        Field::Description => "description",
        Field::Copyright => "copyright",
        _ => return None,
    })
}

fn kind_of(path: &Path, has_video: bool) -> FileKind {
    let audio_only = ["mka", "wma"].contains(&super::extension(path).as_str());
    if audio_only || !has_video { FileKind::Audio } else { FileKind::Video }
}

fn format_name(muxer: Muxer, kind: FileKind) -> String {
    let container = match muxer {
        Muxer::Matroska => "Matroska",
        Muxer::WebM => "WebM",
        Muxer::Avi => "AVI",
        Muxer::Asf => "Windows Media",
    };
    format!("{container} {}", if kind == FileKind::Audio { "audio" } else { "video" })
}

pub async fn read(path: &Path, muxer: Muxer, tools: &Tools) -> Contents {
    let guessed_kind = kind_of(path, true);
    let Some(ffprobe) = tools.ffprobe.as_ref() else {
        return Contents::unreadable(
            guessed_kind,
            format_name(muxer, guessed_kind),
            "FFmpeg is needed to read and change the tags in this kind of file.",
        );
    };
    let mut request = ProcessRequest::new(ffprobe, probe_arguments(path));
    request.timeout = Some(Duration::from_secs(30));
    let output = match process::run_collect(&request, &CancellationToken::new()).await {
        Ok(output) if output.exit.success() => output,
        _ => return Contents::unreadable(guessed_kind, format_name(muxer, guessed_kind), "MediaForge couldn't read this file's tags."),
    };
    let json: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let has_video = streams.iter().any(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0);
    let kind = kind_of(path, has_video);
    let mut contents = Contents::new(kind, format_name(muxer, kind));
    contents.storage = Some("Container tags".into());
    if tools.ffmpeg.is_some() {
        contents.editable = muxer.fields().to_vec();
    } else {
        contents.note = Some("FFmpeg is needed to change the tags in this kind of file.".into());
    }
    let tags: BTreeMap<String, String> = json["format"]["tags"]
        .as_object()
        .map(|o| o.iter().filter_map(|(k, v)| Some((k.to_lowercase(), v.as_str()?.to_string()))).collect())
        .unwrap_or_default();
    for field in muxer.fields() {
        let value = key(*field).and_then(|k| tags.get(k)).cloned();
        // "3/12" in a track tag is the third of twelve.
        let value = if *field == Field::Track { value.map(|v| v.split('/').next().unwrap_or_default().trim().to_string()) } else { value };
        contents.set(*field, value);
    }
    if !contents.fields.contains_key(&Field::Year) {
        contents.set(Field::Year, tags.get("year").cloned());
    }
    if let Some(seconds) = json["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).filter(|d| *d > 0.0) {
        contents.details.push(detail("Length", crate::support::format::duration(seconds)));
    }
    if let Some(video) = streams.iter().find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64().unwrap_or(0) == 0)
    {
        if let (Some(w), Some(h)) = (video["width"].as_u64(), video["height"].as_u64()) {
            contents.details.push(detail("Dimensions", format!("{w} × {h}")));
        }
        if let Some(codec) = video["codec_name"].as_str() {
            contents.details.push(detail("Video", codec.to_uppercase()));
        }
    }
    if let Some(audio) = streams.iter().find(|s| s["codec_type"] == "audio")
        && let Some(codec) = audio["codec_name"].as_str()
    {
        contents.details.push(detail("Audio", codec.to_uppercase()));
    }
    contents
}

pub fn arguments(source: &Path, target: &Path, muxer: Muxer, fields: &BTreeMap<Field, String>) -> Vec<OsString> {
    let mut arguments: Vec<OsString> =
        ["-hide_banner", "-nostdin", "-loglevel", "error", "-n", "-protocol_whitelist", PROTOCOL_WHITELIST, "-i"]
            .iter()
            .map(Into::into)
            .collect();
    arguments.push(file_argument(source));
    // Every stream, chapter and attachment, copied bit for bit; the file's own tags carried over.
    arguments.extend(["-map", "0", "-c", "copy", "-map_metadata", "0", "-map_chapters", "0"].iter().map(Into::into));
    for (field, value) in fields {
        if let Some(key) = key(*field) {
            let value: String = value.trim().chars().filter(|c| !c.is_control() || *c == '\n').collect();
            arguments.push("-metadata".into());
            arguments.push(format!("{key}={value}").into());
        }
    }
    arguments.extend(["-f", muxer.ffmpeg_name()].iter().map(Into::into));
    arguments.push(file_argument(target));
    arguments
}

pub async fn write(path: &Path, muxer: Muxer, fields: &BTreeMap<Field, String>, tools: &Tools) -> Result<(), TagError> {
    let ffmpeg = tools
        .ffmpeg
        .as_ref()
        .ok_or_else(|| TagError::new("FFmpeg not found", "FFmpeg is needed to change the tags in this kind of file."))?;
    let fields: BTreeMap<Field, String> = fields.iter().filter(|(f, _)| muxer.fields().contains(f)).map(|(f, v)| (*f, v.clone())).collect();
    if fields.is_empty() {
        return Ok(());
    }
    let temporary = files::TemporaryFile::beside(path);
    let mut request = ProcessRequest::new(ffmpeg, arguments(path, &temporary.path, muxer, &fields));
    // Copying streams runs at disk speed; allow for very large files.
    request.timeout = Some(Duration::from_secs(60 * 60));
    request.idle_timeout = Some(Duration::from_secs(300));
    request.working_directory = path.parent().map(Path::to_path_buf);
    let output = process::run_collect(&request, &CancellationToken::new())
        .await
        .map_err(|e| TagError::new("Couldn't save the tags", format!("FFmpeg couldn't run ({e:?}).")))?;
    if !output.exit.success() {
        let reason = output.stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("FFmpeg reported an error").trim().to_string();
        return Err(TagError::new("Couldn't save the tags", reason));
    }
    let written = std::fs::metadata(&temporary.path).map(|m| m.len()).unwrap_or(0);
    if written == 0 {
        return Err(TagError::new("Couldn't save the tags", "FFmpeg produced an empty file, so the original wasn't changed."));
    }
    temporary.replace(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffmpeg_copies_every_stream_and_sets_only_the_changed_tags() {
        let fields = BTreeMap::from([(Field::Title, "Holiday\u{0007} film".to_string()), (Field::Comment, String::new())]);
        let arguments: Vec<String> = arguments(Path::new("/in/a.mkv"), Path::new("/in/~$a.tmp"), Muxer::Matroska, &fields)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let joined = arguments.join(" ");
        assert!(joined.contains("-map 0 -c copy -map_metadata 0"), "{joined}");
        assert!(arguments.contains(&"title=Holiday film".to_string()), "{joined}");
        assert!(arguments.contains(&"comment=".to_string()), "{joined}");
        assert!(joined.ends_with("-f matroska file:/in/~$a.tmp"), "{joined}");
        assert!(arguments.contains(&"-n".to_string()));
    }
}
