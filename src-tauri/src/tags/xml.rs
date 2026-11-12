//! Reads and changes a few properties in an XML document (a photo's XMP packet, an Office file's core
//! properties) without disturbing anything else in it. Elements are matched by namespace, not by prefix, so
//! a document that spells "dc:" differently is still understood.

use std::collections::HashMap;

use quick_xml::escape::escape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};
use quick_xml::{NsReader, Writer};

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const DC: &str = "http://purl.org/dc/elements/1.1/";

/// An element or attribute name: its namespace, the prefix to use if the document hasn't declared one, and
/// its local name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Name {
    pub namespace: &'static str,
    pub prefix: &'static str,
    pub local: &'static str,
}

pub const fn name(namespace: &'static str, prefix: &'static str, local: &'static str) -> Name {
    Name { namespace, prefix, local }
}

pub const RDF_DESCRIPTION: Name = name(RDF, "rdf", "Description");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Text(String),
    /// XMP's language alternative: one default-language entry.
    LangAlt(String),
    /// XMP's ordered list (authors).
    Seq(Vec<String>),
    /// XMP's unordered list (keywords).
    Bag(Vec<String>),
}

/// One property to set, or to remove when `value` is None.
#[derive(Debug, Clone)]
pub struct Update {
    pub name: Name,
    pub value: Option<Value>,
}

fn bound(resolved: &ResolveResult) -> Option<Vec<u8>> {
    match resolved {
        ResolveResult::Bound(Namespace(ns)) => Some(ns.to_vec()),
        _ => None,
    }
}

fn matches(namespace: &Option<Vec<u8>>, local: &[u8], name: &Name) -> bool {
    namespace.as_deref() == Some(name.namespace.as_bytes()) && local == name.local.as_bytes()
}

/// Reads properties anywhere in the document, as element text, list items (`rdf:li`) or attributes. Each
/// property's first occurrence wins.
pub fn read(document: &[u8], names: &[Name]) -> HashMap<Name, Vec<String>> {
    struct Capture {
        name: Name,
        depth: usize,
        text: String,
        items: Vec<String>,
        item: Option<String>,
    }
    let mut found: HashMap<Name, Vec<String>> = HashMap::new();
    let mut reader = NsReader::from_reader(document);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut capture: Option<Capture> = None;
    while let Ok((resolved, event)) = reader.read_resolved_event_into(&mut buffer) {
        let namespace = bound(&resolved);
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let is_start = matches!(event, Event::Start(_));
                if is_start {
                    depth += 1;
                }
                let local = start.local_name().as_ref().to_vec();
                for attribute in start.attributes().flatten() {
                    let (attribute_resolved, attribute_local) = reader.resolve_attribute(attribute.key);
                    let attribute_namespace = bound(&attribute_resolved);
                    for name in names {
                        if matches(&attribute_namespace, attribute_local.as_ref(), name)
                            && !found.contains_key(name)
                            && let Ok(value) = attribute.unescape_value()
                            && !value.trim().is_empty()
                        {
                            found.insert(*name, vec![value.trim().to_string()]);
                        }
                    }
                }
                if let Some(capture) = capture.as_mut() {
                    if is_start && namespace.as_deref() == Some(RDF.as_bytes()) && local == b"li" {
                        capture.item = Some(String::new());
                    }
                } else if is_start && let Some(name) = names.iter().find(|n| matches(&namespace, &local, n) && !found.contains_key(n)) {
                    capture = Some(Capture { name: *name, depth, text: String::new(), items: Vec::new(), item: None });
                }
            }
            Event::Text(ref text) => {
                if let Some(capture) = capture.as_mut()
                    && let Ok(text) = text.unescape()
                {
                    match capture.item.as_mut() {
                        Some(item) => item.push_str(&text),
                        None => capture.text.push_str(&text),
                    }
                }
            }
            Event::CData(ref data) => {
                if let Some(capture) = capture.as_mut() {
                    let text = String::from_utf8_lossy(data.as_ref()).into_owned();
                    match capture.item.as_mut() {
                        Some(item) => item.push_str(&text),
                        None => capture.text.push_str(&text),
                    }
                }
            }
            Event::End(ref end) => {
                let local = end.local_name().as_ref().to_vec();
                if let Some(current) = capture.as_mut() {
                    if namespace.as_deref() == Some(RDF.as_bytes()) && local == b"li" {
                        if let Some(item) = current.item.take().map(|i| i.trim().to_string()).filter(|i| !i.is_empty()) {
                            current.items.push(item);
                        }
                    } else if depth == current.depth {
                        let finished = capture.take().expect("capture is present");
                        let values = if finished.items.is_empty() {
                            let text = finished.text.trim().to_string();
                            if text.is_empty() { Vec::new() } else { vec![text] }
                        } else {
                            finished.items
                        };
                        if !values.is_empty() {
                            found.insert(finished.name, values);
                        }
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    found
}

/// The prefix the document already uses for a namespace, or the default one plus a declaration to add.
fn prefix_for(namespace: &str, default: &str, declared: &HashMap<String, String>) -> (String, String) {
    match declared.get(namespace) {
        Some(prefix) => (prefix.clone(), String::new()),
        None => (default.to_string(), format!(" xmlns:{default}=\"{}\"", escape(namespace))),
    }
}

/// The XML for a list of updates, using the document's own prefixes where it has them.
pub fn render(updates: &[Update], declared: &HashMap<String, String>) -> String {
    let mut out = String::new();
    for update in updates {
        let Some(value) = &update.value else { continue };
        let (prefix, declaration) = prefix_for(update.name.namespace, update.name.prefix, declared);
        let tag = format!("{prefix}:{}", update.name.local);
        let (rdf, rdf_declaration) = prefix_for(RDF, "rdf", declared);
        let list = |kind: &str, items: &[String]| {
            let items: String = items.iter().map(|i| format!("<{rdf}:li>{}</{rdf}:li>", escape(i.as_str()))).collect();
            format!("<{tag}{declaration}{rdf_declaration}><{rdf}:{kind}>{items}</{rdf}:{kind}></{tag}>")
        };
        out.push_str(&match value {
            Value::Text(text) => format!("<{tag}{declaration}>{}</{tag}>", escape(text.as_str())),
            Value::LangAlt(text) => format!(
                "<{tag}{declaration}{rdf_declaration}><{rdf}:Alt><{rdf}:li xml:lang=\"x-default\">{}</{rdf}:li></{rdf}:Alt></{tag}>",
                escape(text.as_str())
            ),
            Value::Seq(items) => list("Seq", items),
            Value::Bag(items) => list("Bag", items),
        });
    }
    out
}

/// Rewrites `document`, removing each updated property from inside every `container` element (as a child
/// element or an attribute), then adding the new values to the first container. Returns None when the
/// document has no container.
pub fn rewrite(document: &[u8], container: Name, updates: &[Update]) -> Result<Option<Vec<u8>>, String> {
    let managed: Vec<Name> = updates.iter().map(|u| u.name).collect();
    let mut reader = NsReader::from_reader(document);
    let mut writer = Writer::new(Vec::with_capacity(document.len() + 1024));
    let mut buffer = Vec::new();
    // For each open element, whether it's a container.
    let mut open: Vec<bool> = Vec::new();
    let mut skipping = 0usize;
    let mut inserted = false;
    let mut declared: HashMap<String, String> = HashMap::new();
    loop {
        let (resolved, event) = reader.read_resolved_event_into(&mut buffer).map_err(|e| e.to_string())?;
        let namespace = bound(&resolved);
        match event {
            Event::Start(ref start) | Event::Empty(ref start) => {
                let is_start = matches!(event, Event::Start(_));
                if skipping > 0 {
                    if is_start {
                        skipping += 1;
                    }
                    buffer.clear();
                    continue;
                }
                let local = start.local_name().as_ref().to_vec();
                if open.last() == Some(&true) && managed.iter().any(|n| matches(&namespace, &local, n)) {
                    if is_start {
                        skipping = 1;
                    }
                    buffer.clear();
                    continue;
                }
                for attribute in start.attributes().flatten() {
                    let key = attribute.key.as_ref();
                    if let Some(prefix) = key.strip_prefix(b"xmlns:")
                        && let Ok(value) = attribute.unescape_value()
                    {
                        declared.entry(value.into_owned()).or_insert_with(|| String::from_utf8_lossy(prefix).into_owned());
                    }
                }
                let is_container = matches(&namespace, &local, &container);
                if is_container {
                    let mut kept = BytesStart::new(String::from_utf8_lossy(start.name().as_ref()).into_owned());
                    for attribute in start.attributes().flatten() {
                        let (attribute_resolved, attribute_local) = reader.resolve_attribute(attribute.key);
                        let attribute_namespace = bound(&attribute_resolved);
                        if !managed.iter().any(|n| matches(&attribute_namespace, attribute_local.as_ref(), n)) {
                            kept.push_attribute(attribute);
                        }
                    }
                    writer.write_event(Event::Start(kept.borrow())).map_err(|e| e.to_string())?;
                    if !inserted {
                        writer.get_mut().extend_from_slice(render(updates, &declared).as_bytes());
                        inserted = true;
                    }
                    if is_start {
                        open.push(true);
                    } else {
                        writer.write_event(Event::End(kept.to_end())).map_err(|e| e.to_string())?;
                    }
                } else {
                    writer.write_event(event.borrow()).map_err(|e| e.to_string())?;
                    if is_start {
                        open.push(false);
                    }
                }
            }
            Event::End(_) => {
                if skipping > 0 {
                    skipping -= 1;
                    buffer.clear();
                    continue;
                }
                open.pop();
                writer.write_event(event.borrow()).map_err(|e| e.to_string())?;
            }
            Event::Eof => break,
            _ => {
                if skipping == 0 {
                    writer.write_event(event.borrow()).map_err(|e| e.to_string())?;
                }
            }
        }
        buffer.clear();
    }
    Ok(inserted.then(|| writer.into_inner()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XMP: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="3" xmp:CreatorTool="Camera">
<dc:title xmlns:dc="http://purl.org/dc/elements/1.1/"><rdf:Alt><rdf:li xml:lang="x-default">Old &amp; tired</rdf:li></rdf:Alt></dc:title>
<dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/"><rdf:Seq><rdf:li>Ann</rdf:li><rdf:li>Bob</rdf:li></rdf:Seq></dc:creator>
</rdf:Description>
</rdf:RDF></x:xmpmeta>
<?xpacket end="w"?>"#;

    const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";
    const TITLE: Name = name(DC, "dc", "title");
    const CREATOR: Name = name(DC, "dc", "creator");
    const RATING: Name = name(XMP_NS, "xmp", "Rating");
    const SUBJECT: Name = name(DC, "dc", "subject");

    #[test]
    fn reads_text_lists_and_attributes() {
        let found = read(XMP.as_bytes(), &[TITLE, CREATOR, RATING, SUBJECT]);
        assert_eq!(found[&TITLE], ["Old & tired"]);
        assert_eq!(found[&CREATOR], ["Ann", "Bob"]);
        assert_eq!(found[&RATING], ["3"]);
        assert!(!found.contains_key(&SUBJECT));
    }

    #[test]
    fn replaces_elements_and_attributes_and_keeps_the_rest() {
        let updates = [
            Update { name: TITLE, value: Some(Value::LangAlt("New <title>".into())) },
            Update { name: RATING, value: Some(Value::Text("5".into())) },
            Update { name: CREATOR, value: None },
            Update { name: SUBJECT, value: Some(Value::Bag(vec!["sea".into(), "sky".into()])) },
        ];
        let rewritten = rewrite(XMP.as_bytes(), RDF_DESCRIPTION, &updates).unwrap().unwrap();
        let text = String::from_utf8(rewritten.clone()).unwrap();
        assert!(text.contains("xmp:CreatorTool=\"Camera\""), "{text}");
        assert!(text.starts_with("<?xpacket begin="), "{text}");
        assert!(!text.contains("Old"), "{text}");
        assert!(!text.contains("Ann"), "{text}");
        let found = read(&rewritten, &[TITLE, CREATOR, RATING, SUBJECT]);
        assert_eq!(found[&TITLE], ["New <title>"]);
        assert_eq!(found[&RATING], ["5"]);
        assert_eq!(found[&SUBJECT], ["sea", "sky"]);
        assert!(!found.contains_key(&CREATOR));
    }

    #[test]
    fn uses_the_documents_own_prefixes() {
        let core = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:d="http://purl.org/dc/elements/1.1/"><d:title>Draft</d:title><cp:revision>4</cp:revision></cp:coreProperties>"#;
        let container = name("http://schemas.openxmlformats.org/package/2006/metadata/core-properties", "cp", "coreProperties");
        let updates = [Update { name: TITLE, value: Some(Value::Text("Final".into())) }];
        let text = String::from_utf8(rewrite(core.as_bytes(), container, &updates).unwrap().unwrap()).unwrap();
        assert!(text.contains("<d:title>Final</d:title>"), "{text}");
        assert!(text.contains("<cp:revision>4</cp:revision>"), "{text}");
        assert!(!text.contains("Draft"));
    }

    #[test]
    fn reports_a_missing_container() {
        assert_eq!(rewrite(b"<a><b/></a>", RDF_DESCRIPTION, &[]).unwrap(), None);
    }
}
