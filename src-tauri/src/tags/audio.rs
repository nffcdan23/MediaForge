//! Audio tags (ID3, MP4, Vorbis comments, APE, RIFF INFO) and the tags in MP4 and QuickTime video, through
//! lofty.
//!
//! Every tag a file already has is updated, not just the main one, so an MP3's old ID3v1 tag can't
//! contradict its ID3v2 tag. MP3s are written as ID3v2.3: Spotify, Windows and most car stereos read it,
//! while some still don't read ID3v2.4. Frames MediaForge doesn't show (lyrics, ReplayGain, private data) are
//! kept as they were.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, FileType, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::probe::Probe;
use lofty::tag::{Tag, TagType};

use super::{Contents, Cover, CoverChange, Field, FileKind, TagError, detail, files, picture_summary, signature};

const AUDIO_FIELDS: &[Field] = &[
    Field::Title,
    Field::Artist,
    Field::Album,
    Field::AlbumArtist,
    Field::Track,
    Field::TrackTotal,
    Field::Disc,
    Field::DiscTotal,
    Field::Year,
    Field::Genre,
    Field::Composer,
    Field::Comment,
];

const VIDEO_FIELDS: &[Field] =
    &[Field::Title, Field::Artist, Field::Album, Field::Year, Field::Genre, Field::Comment, Field::Description, Field::Copyright];

/// Text fields stored under one key. Track and disc numbers and the year are handled separately.
fn key(field: Field) -> Option<ItemKey> {
    Some(match field {
        Field::Title => ItemKey::TrackTitle,
        Field::Artist => ItemKey::TrackArtist,
        Field::Album => ItemKey::AlbumTitle,
        Field::AlbumArtist => ItemKey::AlbumArtist,
        Field::Genre => ItemKey::Genre,
        Field::Composer => ItemKey::Composer,
        Field::Comment => ItemKey::Comment,
        Field::Description => ItemKey::Description,
        Field::Copyright => ItemKey::CopyrightMessage,
        _ => return None,
    })
}

fn parse_options() -> ParseOptions {
    ParseOptions::new().read_cover_art(true).read_properties(true)
}

fn open(path: &Path) -> Result<TaggedFile, String> {
    let probe = Probe::open(path).map_err(|e| e.to_string())?.options(parse_options()).guess_file_type().map_err(|e| e.to_string())?;
    probe.read().map_err(|e| e.to_string())
}

fn format_name(file_type: FileType, kind: FileKind) -> String {
    let name = match file_type {
        FileType::Aac => "AAC",
        FileType::Aiff => "AIFF",
        FileType::Ape => "Monkey's Audio",
        FileType::Flac => "FLAC",
        FileType::Mpeg => "MP3",
        FileType::Mp4 => "MP4",
        FileType::Mpc => "Musepack",
        FileType::Opus => "Opus",
        FileType::Vorbis => "Ogg Vorbis",
        FileType::Speex => "Speex",
        FileType::Wav => "WAV",
        FileType::WavPack => "WavPack",
        _ => "Audio",
    };
    match (kind, file_type) {
        (FileKind::Video, _) => "MP4 video".to_string(),
        (_, FileType::Mp4) => "MP4 audio".to_string(),
        _ => format!("{name} audio"),
    }
}

fn tag_type_name(tag_type: TagType) -> &'static str {
    match tag_type {
        TagType::Ape => "APE",
        TagType::Id3v1 => "ID3v1",
        TagType::Id3v2 => "ID3v2",
        TagType::Mp4Ilst => "iTunes tags",
        TagType::VorbisComments => "Vorbis comments",
        TagType::RiffInfo => "RIFF INFO",
        TagType::AiffText => "AIFF text",
        _ => "Tags",
    }
}

/// "ID3v2.3" rather than "ID3v2" for MP3s, read from the tag's own header.
fn id3v2_version(path: &Path) -> Option<u8> {
    let mut header = [0u8; 4];
    std::fs::File::open(path).ok()?.read_exact(&mut header).ok()?;
    (&header[..3] == b"ID3").then_some(header[3])
}

fn year(tag: &Tag) -> Option<String> {
    [ItemKey::RecordingDate, ItemKey::Year, ItemKey::ReleaseDate, ItemKey::OriginalReleaseDate]
        .into_iter()
        .find_map(|key| tag.get_string(key).map(str::to_string))
        .or_else(|| tag.date().map(|d| d.to_string()))
}

fn front_cover(tag: &Tag) -> Option<&Picture> {
    tag.pictures().iter().find(|p| p.pic_type() == PictureType::CoverFront).or_else(|| tag.pictures().first())
}

pub fn read(path: &Path, kind: FileKind) -> Contents {
    let tagged = match open(path) {
        Ok(tagged) => tagged,
        Err(error) => {
            let format = if kind == FileKind::Video { "Video" } else { "Audio" };
            return Contents::unreadable(kind, format, format!("MediaForge couldn't read the tags in this file ({error})."));
        }
    };
    let file_type = tagged.file_type();
    let mut contents = Contents::new(kind, format_name(file_type, kind));
    let primary = tagged.primary_tag_type();

    let storage: Vec<String> = tagged
        .tags()
        .iter()
        .map(|t| match (t.tag_type(), file_type, id3v2_version(path)) {
            (TagType::Id3v2, FileType::Mpeg | FileType::Aac, Some(version @ 2..=4)) => format!("ID3v2.{version}"),
            (tag_type, _, _) => tag_type_name(tag_type).to_string(),
        })
        .collect();
    contents.storage = (!storage.is_empty()).then(|| storage.join(" · "));

    let candidates = if kind == FileKind::Video { VIDEO_FIELDS } else { AUDIO_FIELDS };
    contents.editable = candidates.iter().copied().filter(|f| supports(primary, *f)).collect();

    let tag = tagged.tag(primary).or_else(|| tagged.first_tag());
    if let Some(tag) = tag {
        for field in contents.editable.clone().iter() {
            let value = match field {
                Field::Track => tag.track().filter(|n| *n > 0).map(|n| n.to_string()),
                Field::TrackTotal => tag.track_total().filter(|n| *n > 0).map(|n| n.to_string()),
                Field::Disc => tag.disk().filter(|n| *n > 0).map(|n| n.to_string()),
                Field::DiscTotal => tag.disk_total().filter(|n| *n > 0).map(|n| n.to_string()),
                Field::Year => year(tag),
                other => key(*other).and_then(|k| tag.get_string(k)).map(str::to_string),
            };
            contents.set(*field, value);
        }
    }
    let picture = tagged.tags().iter().find_map(front_cover);
    contents.cover = Some(match picture {
        Some(picture) => Cover { present: true, signature: Some(signature(picture.data())), summary: picture_summary(picture.data()) },
        None => Cover::default(),
    });

    let properties = tagged.properties();
    let seconds = properties.duration().as_secs_f64();
    if seconds > 0.0 {
        contents.details.push(detail("Length", crate::support::format::duration(seconds)));
    }
    if let Some(bitrate) = properties.audio_bitrate().filter(|b| *b > 0) {
        contents.details.push(detail("Bit rate", format!("{bitrate} kbps")));
    }
    if let Some(rate) = properties.sample_rate().filter(|r| *r > 0) {
        let khz = rate as f64 / 1000.0;
        contents.details.push(detail("Sample rate", if khz.fract() == 0.0 { format!("{khz:.0} kHz") } else { format!("{khz:.1} kHz") }));
    }
    if let Some(channels) = properties.channels() {
        let name = match channels {
            1 => "Mono".to_string(),
            2 => "Stereo".to_string(),
            n => format!("{n} channels"),
        };
        contents.details.push(detail("Channels", name));
    }
    contents
}

/// Whether a tag format has somewhere to keep a field.
fn supports(tag_type: TagType, field: Field) -> bool {
    match field {
        Field::Track => ItemKey::TrackNumber.map_key(tag_type).is_some(),
        Field::TrackTotal => ItemKey::TrackTotal.map_key(tag_type).is_some() || tag_type == TagType::Id3v2 || tag_type == TagType::Mp4Ilst,
        Field::Disc => ItemKey::DiscNumber.map_key(tag_type).is_some(),
        Field::DiscTotal => ItemKey::DiscTotal.map_key(tag_type).is_some() || tag_type == TagType::Id3v2 || tag_type == TagType::Mp4Ilst,
        Field::Year => ItemKey::RecordingDate.map_key(tag_type).is_some() || ItemKey::Year.map_key(tag_type).is_some(),
        other => key(other).is_some_and(|k| k.map_key(tag_type).is_some()),
    }
}

fn number(value: &str) -> Option<u32> {
    value.trim().parse().ok().filter(|n| *n > 0)
}

fn apply(tag: &mut Tag, fields: &BTreeMap<Field, String>) {
    for (field, value) in fields {
        let value = value.trim();
        match field {
            // ID3 and MP4 keep "3 of 12" as one value, so a total without a number would be stored as "0/12".
            Field::Track => match number(value) {
                Some(n) => tag.set_track(n),
                None => {
                    tag.remove_track();
                    if fields.get(&Field::TrackTotal).is_none_or(|t| t.trim().is_empty()) {
                        tag.remove_track_total();
                    }
                }
            },
            Field::TrackTotal => match number(value) {
                Some(n) => tag.set_track_total(n),
                None => tag.remove_track_total(),
            },
            Field::Disc => match number(value) {
                Some(n) => tag.set_disk(n),
                None => {
                    tag.remove_disk();
                    if fields.get(&Field::DiscTotal).is_none_or(|t| t.trim().is_empty()) {
                        tag.remove_disk_total();
                    }
                }
            },
            Field::DiscTotal => match number(value) {
                Some(n) => tag.set_disk_total(n),
                None => tag.remove_disk_total(),
            },
            Field::Year => {
                for key in [ItemKey::RecordingDate, ItemKey::Year] {
                    tag.remove_key(key);
                }
                if !value.is_empty() && !tag.insert_text(ItemKey::RecordingDate, value.to_string()) {
                    tag.insert_text(ItemKey::Year, value.to_string());
                }
            }
            other => {
                let Some(key) = key(*other) else { continue };
                tag.remove_key(key);
                if !value.is_empty() {
                    tag.insert_text(key, value.to_string());
                }
            }
        }
    }
}

fn remove_pictures(tag: &mut Tag) {
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }
}

pub fn write(path: &Path, fields: &BTreeMap<Field, String>, cover: Option<&CoverChange>) -> Result<(), TagError> {
    let failed = |e: &dyn std::fmt::Display| TagError::new("Couldn't save the tags", format!("{e}"));
    let copy = files::copy_beside(path)?;
    {
        let mut file =
            OpenOptions::new().read(true).write(true).open(&copy.path).map_err(|e| files::io_error("Couldn't save the tags", path, &e))?;
        let mut tagged = {
            let mut reader = BufReader::new(&mut file);
            Probe::new(&mut reader).options(parse_options()).guess_file_type().map_err(|e| failed(&e))?.read().map_err(|e| failed(&e))?
        };
        let primary = tagged.primary_tag_type();
        if tagged.tag(primary).is_none() {
            tagged.insert_tag(Tag::new(primary));
        }
        let present: Vec<TagType> = tagged.tags().iter().map(Tag::tag_type).collect();
        for tag_type in present {
            let Some(tag) = tagged.tag_mut(tag_type) else { continue };
            apply(tag, fields);
            match cover {
                Some(CoverChange::Remove) => remove_pictures(tag),
                Some(CoverChange::Set(image)) if tag_type == primary => {
                    // Replacing artwork replaces all of it, as other taggers do: players show the first picture
                    // they find, which mustn't be the old one.
                    remove_pictures(tag);
                    let mime = if image.mime == "image/png" { MimeType::Png } else { MimeType::Jpeg };
                    tag.push_picture(Picture::unchecked(image.data.clone()).pic_type(PictureType::CoverFront).mime_type(mime).build());
                }
                Some(CoverChange::Set(_)) => remove_pictures(tag),
                None => {}
            }
        }
        file.rewind().map_err(|e| failed(&e))?;
        let mut options = WriteOptions::new().preferred_padding(4096).respect_read_only(false);
        let options = options.use_id3v23(true);
        tagged.save_to(&mut file, options).map_err(|e| failed(&e))?;
        file.sync_all().map_err(|e| failed(&e))?;
    }
    copy.replace(path)
}

/// The front cover (or first picture) of a file, if it has one.
pub fn cover(path: &Path) -> Option<Vec<u8>> {
    let tagged = open(path).ok()?;
    tagged.tags().iter().find_map(front_cover).map(|p| p.data().to_vec())
}
