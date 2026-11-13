//! Document properties: the title, author, subject and keywords of PDFs and Office files.
//!
//! PDFs are changed with an incremental update: the original bytes are kept exactly and the new
//! properties are appended, as Acrobat does, so nothing else in the document can be disturbed. Office files
//! keep their properties in `docProps/core.xml`; every other part is copied across byte for byte.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use lopdf::{Dictionary, Document, IncrementalDocument, Object, StringFormat};

use super::xml::{self, DC, Name, Update, Value};
use super::{Contents, Field, FileKind, TagError, detail, files};

// MARK: - PDF

const PDF_FIELDS: &[Field] = &[Field::Title, Field::Authors, Field::Subject, Field::Keywords];
const PDF_NS: &str = "http://ns.adobe.com/pdf/1.3/";

fn pdf_key(field: Field) -> Option<&'static [u8]> {
    Some(match field {
        Field::Title => b"Title",
        Field::Authors => b"Author",
        Field::Subject => b"Subject",
        Field::Keywords => b"Keywords",
        _ => return None,
    })
}

pub fn read_pdf(path: &Path) -> Contents {
    let metadata = match Document::load_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return Contents::unreadable(FileKind::Document, "PDF document", format!("MediaForge couldn't read this PDF ({error})."));
        }
    };
    let mut contents = Contents::new(FileKind::Document, "PDF document");
    contents.storage = Some("Document properties".into());
    contents.set(Field::Title, metadata.title);
    contents.set(Field::Authors, metadata.author);
    contents.set(Field::Subject, metadata.subject);
    contents.set(Field::Keywords, metadata.keywords);
    if metadata.encrypted {
        contents.note = Some("This PDF is protected, so its properties can't be changed here.".into());
    } else {
        contents.editable = PDF_FIELDS.to_vec();
    }
    if metadata.page_count > 0 {
        contents.details.push(detail("Pages", metadata.page_count.to_string()));
    }
    if let Some(creator) = metadata.creator.or(metadata.producer).filter(|c| !c.trim().is_empty()) {
        contents.details.push(detail("Created with", creator.trim()));
    }
    if !metadata.version.is_empty() {
        contents.details.push(detail("PDF version", metadata.version));
    }
    contents
}

/// A PDF text string: plain for ASCII, UTF-16 with a byte-order mark for anything else.
fn pdf_text(value: &str) -> Object {
    if value.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
        Object::string_literal(value)
    } else {
        let mut bytes = vec![0xFE, 0xFF];
        bytes.extend(value.encode_utf16().flat_map(u16::to_be_bytes));
        Object::String(bytes, StringFormat::Hexadecimal)
    }
}

fn pdf_xmp_updates(fields: &BTreeMap<Field, String>) -> Vec<Update> {
    let mut updates = Vec::new();
    for (field, value) in fields {
        let value = value.trim().to_string();
        let present = !value.is_empty();
        let (name, value) = match field {
            Field::Title => (xml::name(DC, "dc", "title"), present.then_some(Value::LangAlt(value))),
            Field::Authors => (xml::name(DC, "dc", "creator"), present.then(|| Value::Seq(vec![value]))),
            Field::Subject => (xml::name(DC, "dc", "description"), present.then_some(Value::LangAlt(value))),
            Field::Keywords => (xml::name(PDF_NS, "pdf", "Keywords"), present.then_some(Value::Text(value))),
            _ => continue,
        };
        updates.push(Update { name, value });
    }
    updates
}

pub fn write_pdf(path: &Path, fields: &BTreeMap<Field, String>) -> Result<(), TagError> {
    let failed = |e: &dyn std::fmt::Display| TagError::new("Couldn't save the properties", format!("{e}"));
    let mut document = IncrementalDocument::load(path).map_err(|e| failed(&e))?;
    let previous = document.get_prev_documents();
    if previous.is_encrypted() || previous.trailer.has(b"Encrypt") {
        return Err(TagError::new("Can't change this PDF", "It's protected, so its properties can't be changed here."));
    }
    let info_id = previous.trailer.get(b"Info").and_then(Object::as_reference).ok().filter(|id| previous.get_dictionary(*id).is_ok());
    let metadata_id = previous
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .and_then(|root| previous.get_dictionary(root))
        .and_then(|catalog| catalog.get(b"Metadata"))
        .and_then(Object::as_reference)
        .ok();

    let info_id = match info_id {
        Some(id) => {
            document.opt_clone_object_to_new_document(id).map_err(|e| failed(&e))?;
            id
        }
        None => {
            let id = document.new_document.add_object(Dictionary::new());
            document.new_document.trailer.set("Info", Object::Reference(id));
            id
        }
    };
    let info = document.new_document.get_dictionary_mut(info_id).map_err(|e| failed(&e))?;
    for (field, value) in fields {
        let Some(key) = pdf_key(*field) else { continue };
        let value = value.trim();
        if value.is_empty() {
            info.remove(key);
        } else {
            info.set(key, pdf_text(value));
        }
    }
    info.set("ModDate", Object::string_literal(chrono::Utc::now().format("D:%Y%m%d%H%M%SZ").to_string()));

    // Keep the document's XMP (which newer readers prefer) in step with the Info dictionary.
    if let Some(metadata_id) = metadata_id
        && document.opt_clone_object_to_new_document(metadata_id).is_ok()
        && let Ok(stream) = document.new_document.get_object_mut(metadata_id).and_then(Object::as_stream_mut)
    {
        let packet = if stream.dict.has(b"Filter") { stream.decompressed_content().ok() } else { Some(stream.content.clone()) };
        if let Some(packet) = packet
            && let Ok(Some(updated)) = xml::rewrite(&packet, xml::RDF_DESCRIPTION, &pdf_xmp_updates(fields))
        {
            stream.dict.remove(b"Filter");
            stream.dict.remove(b"DecodeParms");
            stream.set_content(updated);
        }
    }

    let mut out = Vec::new();
    document.save_to(&mut out).map_err(|e| failed(&e))?;
    // Only a PDF that reads back replaces the original.
    Document::load_mem(&out)
        .map_err(|_| TagError::new("Couldn't save the properties", "The PDF came out unreadable, so it wasn't changed."))?;
    files::replace_with_bytes(path, &out)
}

// MARK: - Office

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    Word,
    Excel,
    PowerPoint,
}

impl OfficeKind {
    fn name(self) -> &'static str {
        match self {
            OfficeKind::Word => "Word document",
            OfficeKind::Excel => "Excel workbook",
            OfficeKind::PowerPoint => "PowerPoint presentation",
        }
    }
}

const CORE_PART: &str = "docProps/core.xml";
const APP_PART: &str = "docProps/app.xml";
const CORE_NS: &str = "http://schemas.openxmlformats.org/package/2006/metadata/core-properties";
const APP_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/extended-properties";
const CORE_PROPERTIES: Name = xml::name(CORE_NS, "cp", "coreProperties");
const OFFICE_FIELDS: &[Field] = &[Field::Title, Field::Subject, Field::Authors, Field::Keywords, Field::Category, Field::Comment];

fn office_name(field: Field) -> Option<Name> {
    Some(match field {
        Field::Title => xml::name(DC, "dc", "title"),
        Field::Subject => xml::name(DC, "dc", "subject"),
        Field::Authors => xml::name(DC, "dc", "creator"),
        Field::Keywords => xml::name(CORE_NS, "cp", "keywords"),
        Field::Category => xml::name(CORE_NS, "cp", "category"),
        Field::Comment => xml::name(DC, "dc", "description"),
        _ => return None,
    })
}

fn read_part(archive: &mut zip::ZipArchive<File>, name: &str) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    if entry.size() > 4 * 1024 * 1024 {
        return None;
    }
    let mut data = Vec::new();
    entry.read_to_end(&mut data).ok()?;
    Some(data)
}

fn open_office(path: &Path) -> Result<zip::ZipArchive<File>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(file).map_err(|_| "it may be password-protected".to_string())
}

pub fn read_office(path: &Path, kind: OfficeKind) -> Contents {
    let mut archive = match open_office(path) {
        Ok(archive) => archive,
        Err(why) => {
            return Contents::unreadable(
                FileKind::Document,
                kind.name(),
                format!("MediaForge couldn't read this document's properties ({why})."),
            );
        }
    };
    let mut contents = Contents::new(FileKind::Document, kind.name());
    let Some(core) = read_part(&mut archive, CORE_PART) else {
        contents.note = Some("This document has no properties section. Open it in Office and save it once to add one.".into());
        return contents;
    };
    contents.storage = Some("Document properties".into());
    contents.editable = OFFICE_FIELDS.to_vec();
    let names: Vec<Name> = OFFICE_FIELDS.iter().filter_map(|f| office_name(*f)).collect();
    let last_saved_by = xml::name(CORE_NS, "cp", "lastModifiedBy");
    let found = xml::read(&core, &[names.as_slice(), &[last_saved_by]].concat());
    for field in OFFICE_FIELDS {
        let value = office_name(*field).and_then(|n| found.get(&n)).map(|v| v.join("; "));
        contents.set(*field, value);
    }
    if let Some(app) = read_part(&mut archive, APP_PART) {
        let [application, pages, words, slides] = [
            xml::name(APP_NS, "", "Application"),
            xml::name(APP_NS, "", "Pages"),
            xml::name(APP_NS, "", "Words"),
            xml::name(APP_NS, "", "Slides"),
        ];
        let found = xml::read(&app, &[application, pages, words, slides]);
        for (label, name) in [("Pages", pages), ("Words", words), ("Slides", slides), ("Created with", application)] {
            if let Some(value) = found.get(&name).and_then(|v| v.first()).filter(|v| v.as_str() != "0") {
                contents.details.push(detail(label, value.clone()));
            }
        }
    }
    if let Some(who) = found.get(&last_saved_by).and_then(|v| v.first()) {
        contents.details.push(detail("Last saved by", who.clone()));
    }
    contents
}

pub fn write_office(path: &Path, fields: &BTreeMap<Field, String>) -> Result<(), TagError> {
    let failed = |e: &dyn std::fmt::Display| TagError::new("Couldn't save the properties", format!("{e}"));
    let mut archive =
        open_office(path).map_err(|why| TagError::new("Can't change this document", format!("Its properties couldn't be read: {why}.")))?;
    let core = read_part(&mut archive, CORE_PART).ok_or_else(|| {
        TagError::new("Can't change this document", "It has no properties section. Open it in Office and save it once to add one.")
    })?;
    let updates: Vec<Update> = fields
        .iter()
        .filter_map(|(field, value)| {
            let value = value.trim();
            office_name(*field).map(|name| Update { name, value: (!value.is_empty()).then(|| Value::Text(value.to_string())) })
        })
        .collect();
    let core = xml::rewrite(&core, CORE_PROPERTIES, &updates)
        .map_err(|e| failed(&e))?
        .ok_or_else(|| TagError::new("Can't change this document", "Its properties section isn't in the usual form."))?;

    let temporary = files::TemporaryFile::beside(path);
    let out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary.path)
        .map_err(|e| files::io_error("Couldn't save the properties", path, &e))?;
    let mut writer = zip::ZipWriter::new(out);
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(|e| failed(&e))?;
        if entry.name().is_ok_and(|name| name == CORE_PART) {
            drop(entry);
            let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer.start_file(CORE_PART, options).map_err(|e| failed(&e))?;
            writer.write_all(&core).map_err(|e| failed(&e))?;
        } else {
            writer.raw_copy_file(entry).map_err(|e| failed(&e))?;
        }
    }
    let out = writer.finish().map_err(|e| failed(&e))?;
    out.sync_all().map_err(|e| failed(&e))?;
    drop(out);
    // The new file must open as a document before it replaces the original.
    let mut check = open_office(&temporary.path)
        .map_err(|_| TagError::new("Couldn't save the properties", "The document came out unreadable, so it wasn't changed."))?;
    read_part(&mut check, CORE_PART)
        .ok_or_else(|| TagError::new("Couldn't save the properties", "The document came out unreadable, so it wasn't changed."))?;
    // Windows won't swap files that are still open: close the original and the check first.
    drop(check);
    drop(archive);
    temporary.replace(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> BTreeMap<Field, String> {
        BTreeMap::from([
            (Field::Title, "Quarterly report".to_string()),
            (Field::Authors, "Zoë Smith".to_string()),
            (Field::Subject, "Sales".to_string()),
            (Field::Keywords, "q3, revenue".to_string()),
        ])
    }

    #[test]
    fn pdf_properties_are_appended_without_touching_the_original_bytes() {
        let mut document = Document::with_version("1.5");
        let pages = document.new_object_id();
        let page = document.add_object(
            lopdf::dictionary! { "Type" => "Page", "Parent" => pages, "MediaBox" => vec![0.into(), 0.into(), 200.into(), 200.into()] },
        );
        document
            .objects
            .insert(pages, Object::Dictionary(lopdf::dictionary! { "Type" => "Pages", "Kids" => vec![page.into()], "Count" => 1 }));
        let catalog = document.add_object(lopdf::dictionary! { "Type" => "Catalog", "Pages" => pages });
        document.trailer.set("Root", catalog);
        let info = document
            .add_object(lopdf::dictionary! { "Title" => Object::string_literal("Draft"), "Producer" => Object::string_literal("Test") });
        document.trailer.set("Info", info);
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("report.pdf");
        document.save(&path).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert_eq!(read_pdf(&path).fields[&Field::Title], "Draft");

        write_pdf(&path, &fields()).unwrap();
        let saved = std::fs::read(&path).unwrap();
        assert!(saved.starts_with(&original), "the original bytes are kept");
        let contents = read_pdf(&path);
        assert_eq!(contents.fields[&Field::Title], "Quarterly report");
        assert_eq!(contents.fields[&Field::Authors], "Zoë Smith");
        assert_eq!(contents.fields[&Field::Keywords], "q3, revenue");
        assert_eq!(contents.details[0].value, "1");
        assert_eq!(Document::load(&path).unwrap().get_pages().len(), 1);

        write_pdf(&path, &BTreeMap::from([(Field::Subject, String::new())])).unwrap();
        assert!(!read_pdf(&path).fields.contains_key(&Field::Subject));
    }

    fn docx(path: &Path, core: &str) {
        let file = File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("[Content_Types].xml", options).unwrap();
        writer.write_all(b"<Types/>").unwrap();
        writer.start_file("word/document.xml", options).unwrap();
        writer.write_all(b"<w:document>Body text</w:document>").unwrap();
        writer.start_file(CORE_PART, options).unwrap();
        writer.write_all(core.as_bytes()).unwrap();
        writer.finish().unwrap();
    }

    #[test]
    fn office_properties_round_trip_and_other_parts_are_untouched() {
        let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>Draft</dc:title><dc:creator>Old Author</dc:creator><cp:lastModifiedBy>Someone</cp:lastModifiedBy><dcterms:created xsi:type="dcterms:W3CDTF">2024-01-01T00:00:00Z</dcterms:created></cp:coreProperties>"#;
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("report.docx");
        docx(&path, core);
        let contents = read_office(&path, OfficeKind::Word);
        assert_eq!(contents.fields[&Field::Title], "Draft");
        assert_eq!(contents.fields[&Field::Authors], "Old Author");

        write_office(&path, &fields()).unwrap();
        let contents = read_office(&path, OfficeKind::Word);
        assert_eq!(contents.fields[&Field::Title], "Quarterly report");
        assert_eq!(contents.fields[&Field::Authors], "Zoë Smith");
        assert_eq!(contents.fields[&Field::Subject], "Sales");
        let mut archive = open_office(&path).unwrap();
        assert_eq!(read_part(&mut archive, "word/document.xml").unwrap(), b"<w:document>Body text</w:document>");
        let core = String::from_utf8(read_part(&mut archive, CORE_PART).unwrap()).unwrap();
        assert!(core.contains("<dcterms:created xsi:type=\"dcterms:W3CDTF\">2024-01-01T00:00:00Z</dcterms:created>"), "{core}");
        assert!(core.contains("<cp:lastModifiedBy>Someone</cp:lastModifiedBy>"), "{core}");
    }
}
