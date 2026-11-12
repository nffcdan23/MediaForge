//! Photo details: the fields File Explorer shows on a picture's Details tab.
//!
//! They live in EXIF, which little_exif reads and writes, and Windows' own fields (XPTitle, XPKeywords and
//! so on) live there too. Many JPEGs also carry an XMP packet, which Windows and photo apps read before EXIF;
//! when a JPEG has one, the same fields are updated in it so the two never disagree.

use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::HashMap;
use std::path::Path;

use chrono::NaiveDateTime;
use little_exif::exif_tag::ExifTag;
use little_exif::filetype::FileExtension;
use little_exif::ifd::ExifTagGroup;
use little_exif::metadata::Metadata;

use super::xml::{self, DC, Name, Update, Value};
use super::{Contents, Field, FileKind, TagError, detail, files};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
    Tiff,
    Heif,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG image",
            Format::Png => "PNG image",
            Format::WebP => "WebP image",
            Format::Tiff => "TIFF image",
            Format::Heif => "HEIC image",
        }
    }

    fn extension(self) -> FileExtension {
        match self {
            Format::Jpeg => FileExtension::JPEG,
            Format::Png => FileExtension::PNG { as_zTXt_chunk: false },
            Format::WebP => FileExtension::WEBP,
            Format::Tiff => FileExtension::TIFF,
            Format::Heif => FileExtension::HEIF,
        }
    }
}

const FIELDS: &[Field] =
    &[Field::Title, Field::Subject, Field::Rating, Field::Keywords, Field::Comment, Field::Authors, Field::Copyright, Field::DateTaken];

// EXIF and TIFF tags, and Windows' own (stored as UTF-16 in byte arrays).
const IMAGE_DESCRIPTION: u16 = 0x010E;
const MAKE: u16 = 0x010F;
const MODEL: u16 = 0x0110;
const ARTIST: u16 = 0x013B;
const COPYRIGHT: u16 = 0x8298;
const DATE_TIME_ORIGINAL: u16 = 0x9003;
const RATING: u16 = 0x4746;
const RATING_PERCENT: u16 = 0x4749;
const XP_TITLE: u16 = 0x9C9B;
const XP_COMMENT: u16 = 0x9C9C;
const XP_AUTHOR: u16 = 0x9C9D;
const XP_KEYWORDS: u16 = 0x9C9E;
const XP_SUBJECT: u16 = 0x9C9F;

const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";
const MICROSOFT_PHOTO: &str = "http://ns.microsoft.com/photo/1.0/";
const XMP_TITLE: Name = xml::name(DC, "dc", "title");
const XMP_CREATOR: Name = xml::name(DC, "dc", "creator");
const XMP_RIGHTS: Name = xml::name(DC, "dc", "rights");
const XMP_SUBJECT: Name = xml::name(DC, "dc", "subject");
const XMP_RATING: Name = xml::name(XMP_NS, "xmp", "Rating");
const XMP_RATING_PERCENT: Name = xml::name(MICROSOFT_PHOTO, "MicrosoftPhoto", "Rating");

fn group(tag: u16) -> ExifTagGroup {
    if tag == DATE_TIME_ORIGINAL { ExifTagGroup::EXIF } else { ExifTagGroup::GENERIC }
}

fn utf16(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|u| *u != 0).collect();
    String::from_utf16_lossy(&units)
}

fn to_utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

fn text_of(tag: &ExifTag) -> Option<String> {
    let text = match tag {
        ExifTag::ImageDescription(s)
        | ExifTag::Artist(s)
        | ExifTag::Copyright(s)
        | ExifTag::DateTimeOriginal(s)
        | ExifTag::Make(s)
        | ExifTag::Model(s)
        | ExifTag::UnknownSTRING(s, _, _) => s.clone(),
        ExifTag::UnknownINT8U(bytes, _, _) | ExifTag::UnknownUNDEF(bytes, _, _) => utf16(bytes),
        ExifTag::UnknownINT16U(values, _, _) => values.first()?.to_string(),
        _ => return None,
    };
    let text = text.trim_end_matches('\0').trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn get(metadata: &Metadata, tag: u16) -> Option<String> {
    metadata.get_tag_by_hex(tag, Some(group(tag))).find_map(text_of)
}

/// "2024:05:01 10:20:30" (EXIF) to "2024-05-01T10:20:30" (what the editor shows).
fn exif_date_to_field(text: &str) -> Option<String> {
    NaiveDateTime::parse_from_str(text.trim(), "%Y:%m:%d %H:%M:%S").ok().map(|d| d.format("%Y-%m-%dT%H:%M:%S").to_string())
}

pub fn parse_date_taken(text: &str) -> Option<NaiveDateTime> {
    let text = text.trim();
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").or_else(|_| NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M")).ok()
}

/// Windows writes keywords separated by semicolons; people type commas too.
fn keywords(text: &str) -> Vec<String> {
    text.split([';', ',']).map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect()
}

fn authors(text: &str) -> Vec<String> {
    text.split(';').map(str::trim).filter(|k| !k.is_empty()).map(String::from).collect()
}

/// The XMP packet in a JPEG: the segment's start and end, and where the packet itself starts.
const XMP_SIGNATURE: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

fn find_jpeg_xmp(data: &[u8]) -> Option<(usize, usize)> {
    if !data.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut index = 2;
    while index + 4 <= data.len() && data[index] == 0xFF {
        let marker = data[index + 1];
        if marker == 0xFF {
            index += 1;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            index += 2;
            continue;
        }
        let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
        let end = index + 2 + length;
        if length < 2 || end > data.len() {
            return None;
        }
        if marker == 0xE1 && data[index + 4..end].starts_with(XMP_SIGNATURE) {
            return Some((index, end));
        }
        index = end;
    }
    None
}

fn jpeg_xmp(data: &[u8]) -> Option<&[u8]> {
    let (start, end) = find_jpeg_xmp(data)?;
    Some(&data[start + 4 + XMP_SIGNATURE.len()..end])
}

pub fn read(path: &Path, format: Format) -> Contents {
    let mut contents = Contents::new(FileKind::Image, format.name());
    let data = match std::fs::read(path) {
        Ok(data) => data,
        Err(error) => return Contents::unreadable(FileKind::Image, format.name(), files::io_error("Unreadable", path, &error).message),
    };
    contents.editable = FIELDS.to_vec();
    let exif = Metadata::new_from_vec(&data, format.extension()).ok();
    let mut storage = Vec::new();
    if let Some(exif) = &exif {
        let any = [IMAGE_DESCRIPTION, ARTIST, COPYRIGHT, DATE_TIME_ORIGINAL, MAKE, MODEL, XP_TITLE, XP_KEYWORDS, RATING]
            .iter()
            .any(|t| get(exif, *t).is_some());
        if any || exif.into_iter().next().is_some() {
            storage.push("EXIF");
        }
        contents.set(Field::Title, get(exif, XP_TITLE).or_else(|| get(exif, IMAGE_DESCRIPTION)));
        contents.set(Field::Subject, get(exif, XP_SUBJECT));
        contents.set(Field::Comment, get(exif, XP_COMMENT));
        contents.set(Field::Authors, get(exif, XP_AUTHOR).or_else(|| get(exif, ARTIST)));
        contents.set(Field::Keywords, get(exif, XP_KEYWORDS).map(|k| keywords(&k).join("; ")));
        contents.set(Field::Copyright, get(exif, COPYRIGHT));
        contents.set(Field::Rating, get(exif, RATING).filter(|r| r != "0"));
        contents.set(Field::DateTaken, get(exif, DATE_TIME_ORIGINAL).and_then(|d| exif_date_to_field(&d)));
        let camera: Vec<String> = [get(exif, MAKE), get(exif, MODEL)].into_iter().flatten().collect();
        if !camera.is_empty() {
            contents.details.push(detail("Camera", camera.join(" ")));
        }
    }
    if format == Format::Jpeg
        && let Some(packet) = jpeg_xmp(&data)
    {
        storage.push("XMP");
        // XMP wins where both have a value, as it does in Windows and photo apps.
        let found = xml::read(packet, &[XMP_TITLE, XMP_CREATOR, XMP_RIGHTS, XMP_SUBJECT, XMP_RATING]);
        let first = |name: &Name| found.get(name).and_then(|v| v.first().cloned());
        let joined = |name: &Name| found.get(name).map(|v| v.join("; "));
        contents.set(Field::Title, first(&XMP_TITLE));
        contents.set(Field::Authors, joined(&XMP_CREATOR));
        contents.set(Field::Copyright, first(&XMP_RIGHTS));
        contents.set(Field::Keywords, joined(&XMP_SUBJECT));
        contents.set(Field::Rating, first(&XMP_RATING).filter(|r| r.parse::<i32>().is_ok_and(|n| (1..=5).contains(&n))));
    }
    contents.storage = (!storage.is_empty()).then(|| storage.join(" and "));
    if let Ok((width, height)) = image::ImageReader::new(std::io::Cursor::new(&data))
        .with_guessed_format()
        .map_err(|_| ())
        .and_then(|r| r.into_dimensions().map_err(|_| ()))
    {
        contents.details.insert(0, detail("Dimensions", format!("{width} × {height}")));
    }
    contents
}

fn set_text(metadata: &mut Metadata, tag: u16, value: &str, make: impl Fn(String) -> ExifTag) {
    metadata.remove_tag_by_hex_group(tag, group(tag));
    if !value.is_empty() {
        metadata.set_tag(make(value.to_string()));
    }
}

fn set_windows_text(metadata: &mut Metadata, tag: u16, value: &str) {
    metadata.remove_tag_by_hex_group(tag, group(tag));
    if !value.is_empty() {
        metadata.set_tag(ExifTag::UnknownINT8U(to_utf16(value), tag, group(tag)));
    }
}

fn apply_exif(metadata: &mut Metadata, fields: &BTreeMap<Field, String>) {
    for (field, value) in fields {
        let value = value.trim();
        match field {
            Field::Title => {
                set_text(metadata, IMAGE_DESCRIPTION, value, ExifTag::ImageDescription);
                set_windows_text(metadata, XP_TITLE, value);
            }
            Field::Subject => set_windows_text(metadata, XP_SUBJECT, value),
            Field::Comment => set_windows_text(metadata, XP_COMMENT, value),
            Field::Authors => {
                set_text(metadata, ARTIST, value, ExifTag::Artist);
                set_windows_text(metadata, XP_AUTHOR, &authors(value).join("; "));
            }
            Field::Keywords => set_windows_text(metadata, XP_KEYWORDS, &keywords(value).join(";")),
            Field::Copyright => set_text(metadata, COPYRIGHT, value, ExifTag::Copyright),
            Field::Rating => {
                let stars: u16 = value.parse().unwrap_or(0).min(5);
                for tag in [RATING, RATING_PERCENT] {
                    metadata.remove_tag_by_hex_group(tag, group(tag));
                }
                if stars > 0 {
                    // Windows' own mapping of stars to percent.
                    let percent = [0, 1, 25, 50, 75, 99][stars as usize];
                    metadata.set_tag(ExifTag::UnknownINT16U(vec![stars], RATING, group(RATING)));
                    metadata.set_tag(ExifTag::UnknownINT16U(vec![percent], RATING_PERCENT, group(RATING_PERCENT)));
                }
            }
            Field::DateTaken => {
                let exif = parse_date_taken(value).map(|d| d.format("%Y:%m:%d %H:%M:%S").to_string()).unwrap_or_default();
                set_text(metadata, DATE_TIME_ORIGINAL, &exif, ExifTag::DateTimeOriginal);
            }
            _ => {}
        }
    }
}

fn xmp_updates(fields: &BTreeMap<Field, String>) -> Vec<Update> {
    let mut updates = Vec::new();
    let none_if_empty = |value: &str, make: &dyn Fn(String) -> Value| (!value.is_empty()).then(|| make(value.to_string()));
    for (field, value) in fields {
        let value = value.trim();
        match field {
            Field::Title => updates.push(Update { name: XMP_TITLE, value: none_if_empty(value, &Value::LangAlt) }),
            Field::Copyright => updates.push(Update { name: XMP_RIGHTS, value: none_if_empty(value, &Value::LangAlt) }),
            Field::Authors => {
                let list = authors(value);
                updates.push(Update { name: XMP_CREATOR, value: (!list.is_empty()).then_some(Value::Seq(list)) });
            }
            Field::Keywords => {
                let list = keywords(value);
                updates.push(Update { name: XMP_SUBJECT, value: (!list.is_empty()).then_some(Value::Bag(list)) });
            }
            Field::Rating => {
                let stars: usize = value.parse().unwrap_or(0).min(5);
                let percent = [0, 1, 25, 50, 75, 99][stars];
                updates.push(Update { name: XMP_RATING, value: (stars > 0).then(|| Value::Text(stars.to_string())) });
                updates.push(Update { name: XMP_RATING_PERCENT, value: (stars > 0).then(|| Value::Text(percent.to_string())) });
            }
            _ => {}
        }
    }
    updates
}

/// Replaces the XMP packet in a JPEG, if it has one.
fn update_jpeg_xmp(data: Vec<u8>, fields: &BTreeMap<Field, String>) -> Result<Vec<u8>, TagError> {
    let updates = xmp_updates(fields);
    let Some((start, end)) = find_jpeg_xmp(&data) else { return Ok(data) };
    if updates.is_empty() {
        return Ok(data);
    }
    let packet = &data[start + 4 + XMP_SIGNATURE.len()..end];
    let Some(packet) = xml::rewrite(packet, xml::RDF_DESCRIPTION, &updates)
        .map_err(|e| TagError::new("Couldn't save the details", format!("The picture's XMP data couldn't be read ({e}).")))?
    else {
        return Ok(data);
    };
    let length = 2 + XMP_SIGNATURE.len() + packet.len();
    if length > u16::MAX as usize {
        return Err(TagError::new("Couldn't save the details", "The details are too long to fit in this picture."));
    }
    let mut out = Vec::with_capacity(data.len() + 512);
    out.extend_from_slice(&data[..start]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&(length as u16).to_be_bytes());
    out.extend_from_slice(XMP_SIGNATURE);
    out.extend_from_slice(&packet);
    out.extend_from_slice(&data[end..]);
    Ok(out)
}

pub fn write(path: &Path, format: Format, fields: &BTreeMap<Field, String>) -> Result<(), TagError> {
    let failed = |e: &dyn std::fmt::Display| TagError::new("Couldn't save the details", format!("{e}"));
    let original = std::fs::read(path).map_err(|e| files::io_error("Couldn't save the details", path, &e))?;
    let mut metadata = match Metadata::new_from_vec(&original, format.extension()) {
        Ok(metadata) => metadata,
        Err(_) if !has_exif(&original, format) => Metadata::new(),
        Err(error) => {
            return Err(TagError::new("Can't change this picture", format!("Its EXIF data couldn't be read safely ({error}).")));
        }
    };
    apply_exif(&mut metadata, fields);
    let mut data = original.clone();
    metadata.write_to_vec(&mut data, format.extension()).map_err(|e| failed(&e))?;
    if format == Format::Jpeg {
        data = update_jpeg_xmp(data, fields)?;
    }
    // The result must still be a picture of the same kind before it replaces the original.
    if format != Format::Heif
        && image::ImageReader::new(std::io::Cursor::new(&data)).with_guessed_format().ok().and_then(|r| r.format()).is_none()
    {
        return Err(TagError::new("Couldn't save the details", "The picture came out unreadable, so it wasn't changed."));
    }
    files::replace_with_bytes(path, &data)
}

/// Whether a picture has any EXIF at all, so a failure to read it means damage rather than absence.
fn has_exif(data: &[u8], format: Format) -> bool {
    match format {
        Format::Jpeg => {
            let mut index = 2;
            while index + 10 <= data.len() && data[index] == 0xFF {
                let marker = data[index + 1];
                if marker == 0xDA {
                    return false;
                }
                let length = u16::from_be_bytes([data[index + 2], data[index + 3]]) as usize;
                if marker == 0xE1 && data[index + 4..].starts_with(b"Exif\0") {
                    return true;
                }
                index += 2 + length;
            }
            false
        }
        Format::Png => data.windows(4).any(|w| w == b"eXIf"),
        Format::WebP => data.windows(4).any(|w| w == b"EXIF"),
        Format::Tiff | Format::Heif => true,
    }
}

/// Tags present in a set of found values, for tests.
#[cfg(test)]
fn exif_values(data: &[u8], format: Format) -> HashMap<u16, String> {
    let metadata = Metadata::new_from_vec(&data.to_vec(), format.extension()).unwrap();
    [
        IMAGE_DESCRIPTION,
        ARTIST,
        COPYRIGHT,
        DATE_TIME_ORIGINAL,
        RATING,
        RATING_PERCENT,
        XP_TITLE,
        XP_COMMENT,
        XP_AUTHOR,
        XP_KEYWORDS,
        XP_SUBJECT,
    ]
    .into_iter()
    .filter_map(|t| get(&metadata, t).map(|v| (t, v)))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg() -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(16, 12, image::Rgb([90, 120, 200])))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Jpeg)
            .unwrap();
        out
    }

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(16, 12, image::Rgb([90, 120, 200])))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn all_fields() -> BTreeMap<Field, String> {
        BTreeMap::from([
            (Field::Title, "Harbour at dawn".to_string()),
            (Field::Subject, "Boats".to_string()),
            (Field::Comment, "Shot from the pier — café side".to_string()),
            (Field::Authors, "Ann Lee; Bob Ray".to_string()),
            (Field::Keywords, "sea, boats; morning".to_string()),
            (Field::Copyright, "© 2026 Ann Lee".to_string()),
            (Field::Rating, "4".to_string()),
            (Field::DateTaken, "2026-05-01T06:12:30".to_string()),
        ])
    }

    #[test]
    fn jpeg_details_round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("photo.jpg");
        std::fs::write(&path, jpeg()).unwrap();
        assert!(read(&path, Format::Jpeg).fields.is_empty());

        write(&path, Format::Jpeg, &all_fields()).unwrap();
        let contents = read(&path, Format::Jpeg);
        assert_eq!(contents.fields[&Field::Title], "Harbour at dawn");
        assert_eq!(contents.fields[&Field::Comment], "Shot from the pier — café side");
        assert_eq!(contents.fields[&Field::Authors], "Ann Lee; Bob Ray");
        assert_eq!(contents.fields[&Field::Keywords], "sea; boats; morning");
        assert_eq!(contents.fields[&Field::Rating], "4");
        assert_eq!(contents.fields[&Field::DateTaken], "2026-05-01T06:12:30");
        let raw = exif_values(&std::fs::read(&path).unwrap(), Format::Jpeg);
        assert_eq!(raw[&RATING_PERCENT], "75");
        assert_eq!(raw[&IMAGE_DESCRIPTION], "Harbour at dawn");
        assert!(image::load_from_memory(&std::fs::read(&path).unwrap()).is_ok());

        // Clearing a field removes it everywhere.
        write(&path, Format::Jpeg, &BTreeMap::from([(Field::Title, String::new()), (Field::Rating, "0".into())])).unwrap();
        let contents = read(&path, Format::Jpeg);
        assert!(!contents.fields.contains_key(&Field::Title));
        assert!(!contents.fields.contains_key(&Field::Rating));
        assert_eq!(contents.fields[&Field::Copyright], "© 2026 Ann Lee");
    }

    #[test]
    fn a_jpegs_xmp_packet_is_kept_in_step() {
        let mut data = jpeg();
        let packet = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/" photoshop:City="Leeds"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Old title</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&((2 + XMP_SIGNATURE.len() + packet.len()) as u16).to_be_bytes());
        segment.extend_from_slice(XMP_SIGNATURE);
        segment.extend_from_slice(packet);
        data.splice(2..2, segment);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("edited.jpg");
        std::fs::write(&path, &data).unwrap();
        assert_eq!(read(&path, Format::Jpeg).fields[&Field::Title], "Old title");

        write(&path, Format::Jpeg, &BTreeMap::from([(Field::Title, "New title".to_string())])).unwrap();
        let saved = std::fs::read(&path).unwrap();
        let packet = String::from_utf8_lossy(jpeg_xmp(&saved).unwrap()).into_owned();
        assert!(packet.contains("New title") && !packet.contains("Old title"), "{packet}");
        assert!(packet.contains("photoshop:City=\"Leeds\""), "{packet}");
        assert_eq!(read(&path, Format::Jpeg).fields[&Field::Title], "New title");
    }

    #[test]
    fn png_details_round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("logo.png");
        std::fs::write(&path, png()).unwrap();
        write(&path, Format::Png, &all_fields()).unwrap();
        let contents = read(&path, Format::Png);
        assert_eq!(contents.fields[&Field::Title], "Harbour at dawn");
        assert_eq!(contents.fields[&Field::Authors], "Ann Lee; Bob Ray");
        assert!(image::load_from_memory(&std::fs::read(&path).unwrap()).is_ok());
    }
}
