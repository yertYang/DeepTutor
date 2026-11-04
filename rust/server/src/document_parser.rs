//! Bounded, in-process extraction. No macros, external links or formulas execute.
use crate::{Error, Result};
use quick_xml::{Reader, events::Event};
use std::{
    collections::HashMap,
    io::{Cursor, Read, Write},
};

pub const EXTENSIONS: [&str; 12] = [
    ".txt",
    ".md",
    ".markdown",
    ".csv",
    ".json",
    ".pdf",
    ".docx",
    ".pptx",
    ".xlsx",
    ".png",
    ".jpg",
    ".jpeg",
];
pub const MAX_UPLOAD: usize = 16 * 1024 * 1024;
const MAX_XML: usize = 8 * 1024 * 1024;
const MAX_EXPANDED: u64 = 64 * 1024 * 1024;
const MAX_PARTS: usize = 2000;

fn invalid() -> Error {
    Error::invalid("Invalid or unsupported document structure.")
}
fn append(out: &mut String, value: &str) -> Result<()> {
    if out.len() + value.len() > crate::knowledge::MAX_FILE {
        return Err(Error::invalid(
            "Extracted text exceeds 2 MiB. Split the document before importing.",
        ));
    }
    out.push_str(value);
    Ok(())
}

struct LimitedText(String);
impl Write for LimitedText {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + bytes.len() > crate::knowledge::MAX_FILE {
            return Err(std::io::Error::other("Extracted text limit"));
        }
        self.0
            .push_str(std::str::from_utf8(bytes).map_err(std::io::Error::other)?);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn pdf(bytes: &[u8]) -> Result<String> {
    let doc = pdf_extract::Document::load_mem(bytes).map_err(|_| invalid())?;
    if doc.is_encrypted() {
        return Err(Error::invalid(
            "Encrypted PDFs must be decrypted before importing.",
        ));
    }
    let pages = doc.get_pages();
    if pages.is_empty() || pages.len() > 500 {
        return Err(Error::invalid("PDF must contain 1-500 pages."));
    }
    let mut output = LimitedText(String::new());
    for number in pages.keys() {
        append(&mut output.0, &format!("\n[Page {number}]\n"))?;
        let mut text = pdf_extract::PlainTextOutput::new(&mut output as &mut dyn Write);
        pdf_extract::output_doc_page(&doc, &mut text, *number).map_err(|_| invalid())?;
    }
    // Page headings alone must not make a scanned document look searchable.
    if output
        .0
        .lines()
        .all(|line| line.trim().is_empty() || line.trim().starts_with("[Page "))
    {
        // An empty text-only page is not a scan. Avoid starting the system OCR
        // service for blank pages (and keep malformed/empty imports cheap).
        let raster = doc.objects.values().any(|object| matches!(object,pdf_extract::Object::Stream(stream) if stream.dict.get(b"Subtype").and_then(pdf_extract::Object::as_name).is_ok_and(|kind|kind==b"Image")));
        if !raster {
            return Err(Error::invalid(
                "This PDF has no extractable text or scanned images.",
            ));
        }
        return crate::ocr::pdf(bytes);
    }
    Ok(output.0)
}

#[derive(Debug)]
enum Node {
    Start(String, HashMap<String, String>),
    End(String),
    Text(String),
}
fn local(name: &[u8]) -> Result<String> {
    Ok(std::str::from_utf8(name)
        .map_err(|_| invalid())?
        .rsplit(':')
        .next()
        .unwrap_or("")
        .to_owned())
}
fn xml(input: &str, mut visit: impl FnMut(Node) -> Result<()>) -> Result<()> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0usize;
    let mut count = 0;
    loop {
        count += 1;
        if count > 500_000 {
            return Err(invalid());
        }
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(event) => {
                depth += 1;
                if depth > 128 {
                    return Err(invalid());
                }
                let mut attrs = HashMap::new();
                for attr in event.attributes() {
                    let attr = attr.map_err(|_| invalid())?;
                    let mut key = local(attr.key.as_ref())?;
                    if key == "id" && attr.key.as_ref().contains(&b':') {
                        key = "relationship_id".into();
                    }
                    attrs.insert(
                        key,
                        attr.decode_and_unescape_value(reader.decoder())
                            .map_err(|_| invalid())?
                            .into_owned(),
                    );
                }
                visit(Node::Start(local(event.name().as_ref())?, attrs))?;
            }
            Event::End(event) => {
                depth = depth.checked_sub(1).ok_or_else(invalid)?;
                visit(Node::End(local(event.name().as_ref())?))?;
            }
            Event::Text(event) => visit(Node::Text(
                event.decode().map_err(|_| invalid())?.into_owned(),
            ))?,
            Event::CData(event) => visit(Node::Text(
                event.decode().map_err(|_| invalid())?.into_owned(),
            ))?,
            Event::GeneralRef(event) => {
                let value = event.decode().map_err(|_| invalid())?;
                let value = format!("&{value};");
                visit(Node::Text(
                    quick_xml::escape::unescape(&value)
                        .map_err(|_| invalid())?
                        .into_owned(),
                ))?;
            }
            Event::DocType(_) => {
                return Err(Error::invalid(
                    "Document XML declarations with entities are not supported.",
                ));
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(invalid());
    }
    Ok(())
}

struct Office {
    archive: zip::ZipArchive<Cursor<Vec<u8>>>,
}
impl Office {
    fn open(bytes: &[u8]) -> Result<Self> {
        let mut archive =
            zip::ZipArchive::new(Cursor::new(bytes.to_vec())).map_err(|_| invalid())?;
        if archive.len() > MAX_PARTS {
            return Err(invalid());
        }
        let mut total = 0u64;
        let mut seen = std::collections::HashSet::new();
        for i in 0..archive.len() {
            let file = archive.by_index(i).map_err(|_| invalid())?;
            if file.enclosed_name().is_none() || !seen.insert(file.name().to_owned()) {
                return Err(invalid());
            }
            total = total.checked_add(file.size()).ok_or_else(invalid)?;
            if total > MAX_EXPANDED {
                return Err(Error::invalid("Office archive expands beyond 64 MiB."));
            }
        }
        Ok(Self { archive })
    }
    fn read(&mut self, name: &str) -> Result<String> {
        let file = self.archive.by_name(name).map_err(|_| invalid())?;
        if file.size() > MAX_XML as u64 {
            return Err(invalid());
        }
        let mut out = String::new();
        file.take((MAX_XML + 1) as u64)
            .read_to_string(&mut out)
            .map_err(|_| invalid())?;
        if out.len() > MAX_XML {
            return Err(invalid());
        }
        Ok(out)
    }
    fn contains(&self, name: &str) -> bool {
        self.archive.file_names().any(|n| n == name)
    }
    fn relations(&mut self, name: &str, prefix: &str) -> Result<HashMap<String, String>> {
        let mut result = HashMap::new();
        xml(&self.read(name)?, |node| {
            if let Node::Start(tag, attrs) = node
                && tag == "Relationship"
            {
                let target = attrs.get("Target").ok_or_else(invalid)?;
                if attrs.get("TargetMode").is_some_and(|v| v == "External") {
                    return Ok(());
                }
                let target = target.trim_start_matches('/');
                let path = if target.starts_with(prefix) {
                    target.to_owned()
                } else {
                    format!("{prefix}{target}")
                };
                crate::knowledge::path(&path)?;
                result.insert(attrs.get("Id").ok_or_else(invalid)?.clone(), path);
            }
            Ok(())
        })?;
        Ok(result)
    }
}

fn paragraphs(input: &str) -> Result<String> {
    let mut out = String::new();
    let mut text = false;
    xml(input, |node| {
        match node {
            Node::Start(tag, _) if tag == "t" => text = true,
            Node::End(tag) if tag == "t" => text = false,
            Node::Text(value) if text => append(&mut out, &value)?,
            Node::End(tag) if tag == "p" || tag == "tr" => append(&mut out, "\n")?,
            Node::Start(tag, _) if tag == "tab" || tag == "tc" => append(&mut out, "\t")?,
            Node::Start(tag, _) if tag == "br" => append(&mut out, "\n")?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(out)
}
fn slides(office: &mut Office) -> Result<String> {
    let relations = office.relations("ppt/_rels/presentation.xml.rels", "ppt/")?;
    let mut paths = Vec::new();
    xml(&office.read("ppt/presentation.xml")?, |node| {
        if let Node::Start(tag, attrs) = node
            && tag == "sldId"
        {
            paths.push(
                relations
                    .get(attrs.get("relationship_id").ok_or_else(invalid)?)
                    .ok_or_else(invalid)?
                    .clone(),
            );
        }
        Ok(())
    })?;
    let mut out = String::new();
    for (index, path) in paths.iter().enumerate() {
        if !path.starts_with("ppt/slides/") {
            return Err(invalid());
        }
        append(&mut out, &format!("\n[Slide {}]\n", index + 1))?;
        append(&mut out, &paragraphs(&office.read(path)?)?)?;
    }
    Ok(out)
}
fn spreadsheet(office: &mut Office) -> Result<String> {
    let mut strings = Vec::new();
    if office.contains("xl/sharedStrings.xml") {
        let mut value = String::new();
        let mut text = false;
        xml(&office.read("xl/sharedStrings.xml")?, |node| {
            match node {
                Node::Start(tag, _) if tag == "si" => value.clear(),
                Node::Start(tag, _) if tag == "t" => text = true,
                Node::End(tag) if tag == "t" => text = false,
                Node::Text(v) if text => append(&mut value, &v)?,
                Node::End(tag) if tag == "si" => strings.push(value.clone()),
                _ => {}
            }
            Ok(())
        })?;
    }
    let relations = office.relations("xl/_rels/workbook.xml.rels", "xl/")?;
    let mut sheets = Vec::new();
    xml(&office.read("xl/workbook.xml")?, |node| {
        if let Node::Start(tag, attrs) = node
            && tag == "sheet"
        {
            sheets.push((
                attrs.get("name").ok_or_else(invalid)?.clone(),
                relations
                    .get(attrs.get("relationship_id").ok_or_else(invalid)?)
                    .ok_or_else(invalid)?
                    .clone(),
            ));
        }
        Ok(())
    })?;
    let mut out = String::new();
    for (name, path) in sheets {
        if !path.starts_with("xl/worksheets/") {
            return Err(invalid());
        }
        append(&mut out, &format!("\n[Sheet {name}]\n"))?;
        let mut cell = String::new();
        let mut kind = String::new();
        let mut value = String::new();
        let mut capture = false;
        xml(&office.read(&path)?, |node| {
            match node {
                Node::Start(tag, attrs) if tag == "c" => {
                    cell = attrs.get("r").cloned().unwrap_or_default();
                    kind = attrs.get("t").cloned().unwrap_or_default();
                    value.clear();
                }
                Node::Start(tag, _) if tag == "v" || tag == "t" => capture = true,
                Node::End(tag) if tag == "v" || tag == "t" => capture = false,
                Node::Text(v) if capture => append(&mut value, &v)?,
                Node::End(tag) if tag == "c" => {
                    let v = if kind == "s" {
                        strings
                            .get(value.parse::<usize>().map_err(|_| invalid())?)
                            .ok_or_else(invalid)?
                            .as_str()
                    } else {
                        &value
                    };
                    append(&mut out, &format!("{cell}\t{v}\n"))?;
                }
                _ => {}
            }
            Ok(())
        })?;
    }
    Ok(out)
}

pub fn parse(path: &str, bytes: &[u8]) -> Result<String> {
    crate::knowledge::path(path)?;
    if bytes.len() > MAX_UPLOAD {
        return Err(Error::invalid("Upload exceeds 16 MiB."));
    }
    let extension = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let value = std::panic::catch_unwind(|| match extension.as_str() {
        "pdf" => pdf(bytes),
        "png" | "jpg" | "jpeg" => crate::ocr::image(bytes),
        "docx" => paragraphs(&Office::open(bytes)?.read("word/document.xml")?),
        "pptx" => slides(&mut Office::open(bytes)?),
        "xlsx" => spreadsheet(&mut Office::open(bytes)?),
        "txt" | "md" | "markdown" | "csv" | "json" => String::from_utf8(bytes.to_vec()).map_err(|_| Error::invalid("Text documents must use UTF-8.")),
        _ => Err(Error::unsupported("Supported formats: TXT, Markdown, CSV, JSON, PDF, DOCX, PPTX and XLSX. Scanned images and legacy Office formats are not supported yet.")),
    }).map_err(|_| invalid())??;
    if value.trim().is_empty() {
        return Err(Error::invalid("Document contains no extractable text."));
    }
    if value.len() > crate::knowledge::MAX_FILE || value.contains('\0') {
        return Err(Error::invalid("Invalid or oversized extracted text."));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn archive(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, value) in parts {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(value.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }
    #[test]
    fn office_preserves_unicode_structure_and_presentation_order() {
        let word = archive(&[(
            "word/document.xml",
            "<w:document xmlns:w='word'><w:body><w:p><w:r><w:t>中文 &amp; 引用</w:t></w:r></w:p><w:p><w:r><w:t>第二段</w:t></w:r></w:p></w:body></w:document>",
        )]);
        assert_eq!(parse("资料.docx", &word).unwrap(), "中文 & 引用\n第二段\n");
        let slides = archive(&[
            (
                "ppt/presentation.xml",
                "<p:sldIdLst xmlns:p='presentation' xmlns:r='relations'><p:sldId r:id='second' id='1'/><p:sldId id='2' r:id='first'/></p:sldIdLst>",
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                "<Relationships><Relationship Id='first' Target='slides/slide1.xml'/><Relationship Id='second' Target='slides/slide2.xml'/></Relationships>",
            ),
            ("ppt/slides/slide1.xml", "<p><t>后面</t></p>"),
            ("ppt/slides/slide2.xml", "<p><t>前面</t></p>"),
        ]);
        assert_eq!(
            parse("课件.pptx", &slides).unwrap(),
            "\n[Slide 1]\n前面\n\n[Slide 2]\n后面\n"
        );
    }
    #[test]
    fn spreadsheet_resolves_shared_and_inline_strings_and_cached_formula_values() {
        let book = archive(&[
            (
                "xl/workbook.xml",
                "<sheets xmlns:r='relations'><sheet name='实验数据' r:id='sheet'/></sheets>",
            ),
            (
                "xl/_rels/workbook.xml.rels",
                "<Relationships><Relationship Id='sheet' Target='/xl/worksheets/sheet1.xml'/><Relationship Id='web' TargetMode='External' Target='https://example.invalid'/></Relationships>",
            ),
            (
                "xl/sharedStrings.xml",
                "<sst><si><r><t>质量</t></r><r><t>与能量</t></r></si></sst>",
            ),
            (
                "xl/worksheets/sheet1.xml",
                "<worksheet><row><c r='A1' t='s'><v>0</v></c><c r='B1' t='inlineStr'><is><t>中文</t></is></c><c r='C1'><f>SUM(1,2)</f><v>3</v></c></row></worksheet>",
            ),
        ]);
        assert_eq!(
            parse("表格.xlsx", &book).unwrap(),
            "\n[Sheet 实验数据]\nA1\t质量与能量\nB1\t中文\nC1\t3\n"
        );
    }
    #[test]
    fn rejects_corruption_entities_traversal_and_empty_documents() {
        for (path, bytes) in [
            ("bad.pdf", b"broken".as_slice()),
            ("bad.docx", b"broken".as_slice()),
            ("bad.doc", b"legacy".as_slice()),
            ("blank.txt", b"  ".as_slice()),
        ] {
            assert!(parse(path, bytes).is_err(), "{path}");
        }
        let entity = archive(&[(
            "word/document.xml",
            "<!DOCTYPE root [<!ENTITY secret SYSTEM 'file:///etc/passwd'>]><p><t>&secret;</t></p>",
        )]);
        assert!(parse("bad.docx", &entity).is_err());
        let traversal = archive(&[("../word/document.xml", "<p><t>data</t></p>")]);
        assert!(parse("bad.docx", &traversal).is_err());
        let large = archive(&[("word/document.xml", &"x".repeat(MAX_XML + 1))]);
        assert!(parse("large.docx", &large).is_err());
    }
    pub fn pdf_fixture(text: &str) -> Vec<u8> {
        use pdf_extract::{Dictionary, Object, Stream};
        let mut doc = pdf_extract::Document::with_version("1.5");
        let pages = doc.new_object_id();
        let mut font = Dictionary::new();
        font.set("Type", "Font");
        font.set("Subtype", "Type1");
        font.set("BaseFont", "Helvetica");
        let font = doc.add_object(font);
        let mut fonts = Dictionary::new();
        fonts.set("F1", font);
        let mut resources = Dictionary::new();
        resources.set("Font", fonts);
        let resources = doc.add_object(resources);
        let content = doc.add_object(Stream::new(
            Dictionary::new(),
            format!("BT /F1 12 Tf 40 700 Td ({text}) Tj ET").into_bytes(),
        ));
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages);
        page.set("Contents", content);
        page.set("Resources", resources);
        page.set(
            "MediaBox",
            vec![Object::Integer(0), 0.into(), 612.into(), 792.into()],
        );
        let page = doc.add_object(page);
        let mut tree = Dictionary::new();
        tree.set("Type", "Pages");
        tree.set("Kids", vec![Object::Reference(page)]);
        tree.set("Count", 1);
        doc.objects.insert(pages, Object::Dictionary(tree));
        let mut root = Dictionary::new();
        root.set("Type", "Catalog");
        root.set("Pages", pages);
        let root = doc.add_object(root);
        doc.trailer.set("Root", root);
        let mut output = Vec::new();
        doc.save_to(&mut output).unwrap();
        output
    }
    #[test]
    fn pdf_extracts_page_text_and_rejects_scan_only_content() {
        let text = parse("paper.pdf", &pdf_fixture("Physics source")).unwrap();
        assert!(text.contains("[Page 1]"));
        assert!(text.contains("Physics source"));
        assert!(parse("scan.pdf", &pdf_fixture("")).is_err());
    }
}
