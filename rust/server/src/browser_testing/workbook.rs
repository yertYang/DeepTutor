//! Change only requested result cells. Every other ZIP part is copied intact,
//! preserving formulas, styles, drawings and worksheet structure in the source.
use super::*;
use quick_xml::{Reader, events::Event};
use std::{
    collections::BTreeMap,
    io::{Cursor, Read, Write},
};

fn invalid() -> Error {
    Error::invalid("Invalid XLSX workbook structure.")
}
fn local(name: &[u8]) -> &[u8] {
    name.rsplit(|b| *b == b':').next().unwrap_or(name)
}
fn attrs(e: &quick_xml::events::BytesStart<'_>) -> Result<HashMap<String, String>> {
    e.attributes()
        .map(|a| {
            let a = a.map_err(|_| invalid())?;
            Ok((
                String::from_utf8_lossy(a.key.as_ref()).to_string(),
                a.unescape_value().map_err(|_| invalid())?.into_owned(),
            ))
        })
        .collect()
}
pub fn column(value: &str) -> Result<u32> {
    if value.is_empty() || value.len() > 3 || !value.bytes().all(|b| b.is_ascii_uppercase()) {
        return Err(Error::invalid("Use an Excel column such as G or AA."));
    }
    let n = value
        .bytes()
        .fold(0u32, |n, b| n * 26 + u32::from(b - b'A' + 1));
    if n > 16384 {
        return Err(Error::invalid("Excel column exceeds XFD."));
    }
    Ok(n)
}
pub fn column_name(mut n: u32) -> String {
    let mut out = Vec::new();
    while n > 0 {
        n -= 1;
        out.push(b'A' + (n % 26) as u8);
        n /= 26;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}
fn cell(value: &str) -> Result<(u32, u32)> {
    let split = value
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(invalid)?;
    let c = column(&value[..split])?;
    let row = value[split..].parse::<u32>().map_err(|_| invalid())?;
    if !(1..=1048576).contains(&row) {
        return Err(invalid());
    }
    Ok((c, row))
}
fn part(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<String> {
    let mut f = archive.by_name(name).map_err(|_| invalid())?;
    if f.size() > 8 * 1024 * 1024 {
        return Err(invalid());
    }
    let mut out = String::new();
    f.read_to_string(&mut out).map_err(|_| invalid())?;
    Ok(out)
}
fn sheet_path(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, sheet: &str) -> Result<String> {
    let workbook = part(archive, "xl/workbook.xml")?;
    let mut reader = Reader::from_str(&workbook);
    let mut relationship = None;
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"sheet" => {
                let a = attrs(&e)?;
                if a.get("name").is_some_and(|v| v == sheet) {
                    relationship = a
                        .iter()
                        .find(|(k, _)| k.ends_with(":id"))
                        .map(|(_, v)| v.clone());
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let relationship = relationship
        .ok_or_else(|| Error::invalid("Worksheet was not found. Use its exact imported name."))?;
    let relations = part(archive, "xl/_rels/workbook.xml.rels")?;
    let mut reader = Reader::from_str(&relations);
    loop {
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"Relationship" => {
                let a = attrs(&e)?;
                if a.get("Id") == Some(&relationship)
                    && a.get("TargetMode").is_none_or(|v| v != "External")
                {
                    let t = a.get("Target").ok_or_else(invalid)?;
                    let path = if t.starts_with('/') {
                        t.trim_start_matches('/').to_owned()
                    } else {
                        format!("xl/{t}")
                    };
                    if !path.starts_with("xl/worksheets/") || path.split('/').any(|p| p == "..") {
                        return Err(invalid());
                    }
                    return Ok(path);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Err(invalid())
}
fn value_cell(reference: &str, style: Option<&String>, value: &str, prefix: &str) -> String {
    let value = value
        .chars()
        .filter(|c| {
            matches!(*c, '\t' | '\n' | '\r')
                || (*c >= ' ' && !matches!(*c, '\u{fffe}' | '\u{ffff}'))
        })
        .collect::<String>();
    let style = style
        .map(|s| format!(" s=\"{}\"", quick_xml::escape::escape(s)))
        .unwrap_or_default();
    format!(
        "<{prefix}c r=\"{reference}\"{style} t=\"inlineStr\"><{prefix}is><{prefix}t xml:space=\"preserve\">{}</{prefix}t></{prefix}is></{prefix}c>",
        quick_xml::escape::escape(&value)
    )
}
fn replace(mut source: String, mut edits: Vec<(usize, usize, String)>) -> String {
    // Replace the original cell before inserting new cells at the same offset.
    edits.sort_by_key(|&(start, end, _)| std::cmp::Reverse((start, end)));
    for (start, end, value) in edits {
        source.replace_range(start..end, &value);
    }
    source
}
fn patch_row(source: &str, values: &BTreeMap<u32, String>) -> Result<String> {
    let mut reader = Reader::from_str(source);
    let mut cells = Vec::new();
    let mut row = 0;
    let mut close = None;
    let mut prefix = String::new();
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(e) if local(e.name().as_ref()) == b"row" => {
                row = attrs(&e)?
                    .get("r")
                    .ok_or_else(invalid)?
                    .parse::<u32>()
                    .map_err(|_| invalid())?;
                let q = String::from_utf8_lossy(e.name().as_ref()).to_string();
                prefix = q.strip_suffix("row").unwrap_or("").to_owned();
            }
            Event::Empty(e) if local(e.name().as_ref()) == b"row" => {
                let a = attrs(&e)?;
                let r = a.get("r").ok_or_else(invalid)?;
                let q = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let p = q.strip_suffix("row").unwrap_or("");
                let tag = source.trim_end().strip_suffix("/>").ok_or_else(invalid)?;
                let body = values
                    .iter()
                    .map(|(c, v)| value_cell(&format!("{}{r}", column_name(*c)), None, v, p))
                    .collect::<String>();
                return Ok(format!("{tag}>{body}</{q}>"));
            }
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"c" => {
                let a = attrs(&e)?;
                let (c, r) = cell(a.get("r").ok_or_else(invalid)?)?;
                if r != row {
                    return Err(invalid());
                }
                // Start tags end in '>'; empty tags end in '/>'.
                let event_end = reader.buffer_position() as usize;
                if !source[start..event_end].trim_end().ends_with("/>") {
                    reader.read_to_end(e.name()).map_err(|_| invalid())?;
                }
                cells.push((
                    c,
                    start,
                    reader.buffer_position() as usize,
                    a.get("s").cloned(),
                ));
            }
            Event::Empty(e) | Event::Start(e) if local(e.name().as_ref()) == b"extLst" => {
                close.get_or_insert(start);
                if !source[start..reader.buffer_position() as usize].ends_with("/>") {
                    reader.read_to_end(e.name()).map_err(|_| invalid())?;
                }
            }
            Event::End(e) if local(e.name().as_ref()) == b"row" => {
                close.get_or_insert(start);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let close = close.ok_or_else(invalid)?;
    if row == 0 {
        return Err(invalid());
    }
    let mut edits = Vec::new();
    let mut inserts: BTreeMap<usize, String> = BTreeMap::new();
    for (c, v) in values {
        if let Some((_, start, end, style)) = cells.iter().find(|(n, _, _, _)| n == c) {
            let mut cell_reader = Reader::from_str(&source[*start..*end]);
            loop {
                match cell_reader.read_event().map_err(|_| invalid())? {
                    Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"f" => {
                        return Err(Error::invalid(
                            "A result cell contains a formula. Choose empty result columns instead.",
                        ));
                    }
                    Event::Eof => break,
                    _ => {}
                }
            }
            edits.push((
                *start,
                *end,
                value_cell(
                    &format!("{}{row}", column_name(*c)),
                    style.as_ref(),
                    v,
                    &prefix,
                ),
            ));
        } else {
            let pos = cells
                .iter()
                .find(|(n, _, _, _)| n > c)
                .map(|(_, s, _, _)| *s)
                .unwrap_or(close);
            inserts.entry(pos).or_default().push_str(&value_cell(
                &format!("{}{row}", column_name(*c)),
                None,
                v,
                &prefix,
            ));
        }
    }
    edits.extend(inserts.into_iter().map(|(p, v)| (p, p, v)));
    Ok(replace(source.to_owned(), edits))
}
pub fn update(
    bytes: &[u8],
    sheet: &str,
    changes: &BTreeMap<u32, BTreeMap<u32, String>>,
) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid())?;
    let target = sheet_path(&mut archive, sheet)?;
    let source = part(&mut archive, &target)?;
    let mut reader = Reader::from_str(&source);
    let mut edits = Vec::new();
    let mut found = Vec::new();
    let max_col = changes
        .values()
        .flat_map(|c| c.keys())
        .copied()
        .max()
        .unwrap_or(1);
    let max_row = changes.keys().copied().max().unwrap_or(1);
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event().map_err(|_| invalid())? {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"row" => {
                let row = attrs(&e)?
                    .get("r")
                    .ok_or_else(invalid)?
                    .parse::<u32>()
                    .map_err(|_| invalid())?;
                let event_end = reader.buffer_position() as usize;
                if !source[start..event_end].trim_end().ends_with("/>") {
                    reader.read_to_end(e.name()).map_err(|_| invalid())?;
                }
                if let Some(values) = changes.get(&row) {
                    let end = reader.buffer_position() as usize;
                    edits.push((start, end, patch_row(&source[start..end], values)?));
                    found.push(row);
                }
            }
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"dimension" => {
                let a = attrs(&e)?;
                let reference = a.get("ref").ok_or_else(invalid)?;
                let end = reference.rsplit(':').next().unwrap_or(reference);
                let (c, r) = cell(end)?;
                let mut node = e.into_owned();
                node.clear_attributes();
                let dimension = format!("A1:{}{}", column_name(c.max(max_col)), r.max(max_row));
                for (k, v) in &a {
                    if k != "ref" {
                        node.push_attribute((k.as_str(), v.as_str()));
                    }
                }
                node.push_attribute(("ref", dimension.as_str()));
                edits.push((
                    start,
                    reader.buffer_position() as usize,
                    format!(
                        "<{}{}>",
                        String::from_utf8_lossy(node.as_ref()),
                        if source[start..reader.buffer_position() as usize].ends_with("/>") {
                            "/"
                        } else {
                            ""
                        }
                    ),
                ));
            }
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == b"mergeCell" => {
                let a = attrs(&e)?;
                let reference = a.get("ref").ok_or_else(invalid)?;
                let mut range = reference.split(':');
                let (c1, r1) = cell(range.next().ok_or_else(invalid)?)?;
                let (c2, r2) = cell(range.next().unwrap_or(reference))?;
                if changes.iter().any(|(r, cells)| {
                    *r >= r1
                        && *r <= r2
                        && cells
                            .keys()
                            .any(|c| *c >= c1 && *c <= c2 && (*c != c1 || *r != r1))
                }) {
                    return Err(Error::invalid(
                        "A result cell is inside a merged range. Choose separate result columns.",
                    ));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if found.len() != changes.len() {
        return Err(Error::invalid(
            "A selected Excel row does not exist. Use the row numbers returned by read_source.",
        ));
    }
    let patched = replace(source, edits);
    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let f = archive.by_index(index).map_err(|_| invalid())?;
        if f.name() == target {
            output
                .start_file(
                    &target,
                    zip::write::SimpleFileOptions::default().compression_method(f.compression()),
                )
                .map_err(|_| invalid())?;
            output.write_all(patched.as_bytes())?;
        } else {
            output.raw_copy_file(f).map_err(|_| invalid())?;
        }
    }
    Ok(output.finish().map_err(|_| invalid())?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inserted_cells_precede_row_extensions_and_replaced_cells() {
        let source = r#"<row r="2"><c r="C2"/><extLst><ext uri="test"/></extLst></row>"#;
        let values = BTreeMap::from([(2, "new".into()), (3, "updated".into()), (4, "last".into())]);
        let xml = patch_row(source, &values).unwrap();
        for (left, right) in [("B2", "C2"), ("C2", "D2"), ("D2", "<extLst>")] {
            assert!(xml.find(left).unwrap() < xml.find(right).unwrap());
        }
    }

    #[test]
    fn preserves_formula_style_merges_and_unmodified_zip_parts() {
        let original = include_bytes!("../../tests/fixtures/browser-cases.xlsx");
        let changes = BTreeMap::from([
            (1, BTreeMap::from([(7, "执行状态".into())])),
            (
                2,
                BTreeMap::from([(7, "通过 & <verified>".into()), (8, "Alice".into())]),
            ),
        ]);
        let output = update(original, "Cases", &changes).unwrap();
        let text = crate::document_parser::parse("cases.xlsx", &output).unwrap();
        assert!(text.contains("G2\t通过 & <verified>"));
        assert!(text.contains("F2\t2"));
        let mut source = zip::ZipArchive::new(Cursor::new(original.as_slice())).unwrap();
        let mut result = zip::ZipArchive::new(Cursor::new(output.as_slice())).unwrap();
        let xml = part(&mut result, "xl/worksheets/sheet1.xml").unwrap();
        assert!(xml.contains("<f>1+1</f>"));
        assert!(xml.contains("s=\"1\""));
        assert!(xml.contains("A3:B3"));
        assert!(xml.contains("A1:H3"));
        for name in source.file_names().map(str::to_owned).collect::<Vec<_>>() {
            if name != "xl/worksheets/sheet1.xml" {
                assert_eq!(
                    part(&mut source, &name).unwrap(),
                    part(&mut result, &name).unwrap()
                );
            }
        }
        assert!(
            update(
                original,
                "Cases",
                &BTreeMap::from([(2, BTreeMap::from([(6, "overwrite formula".into())]))])
            )
            .is_err()
        );
        assert!(
            update(
                original,
                "Cases",
                &BTreeMap::from([(3, BTreeMap::from([(2, "overwrite merge".into())]))])
            )
            .is_err()
        );
        assert!(
            update(
                original,
                "Cases",
                &BTreeMap::from([(999, BTreeMap::from([(7, "missing row".into())]))])
            )
            .is_err()
        );
    }
}
