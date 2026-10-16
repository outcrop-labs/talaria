// READING an OOXML document: open the zip, walk the one or two parts that hold
// the words, and hand back plain text.
//
// WHY A HAND-ROLLED WALK. The crates that read .docx/.xlsx for you either pull
// a whole DOM (and a dependency tree with it) or model the format faithfully —
// styles, themes, calc chains. None of that survives the trip: the caller
// turns this text into retrieval chunks and model context, where a paragraph
// break matters and a font does not. A pull parser over the three parts that
// carry text is a couple of hundred lines, keeps the dependency surface at two
// crates, and never has to be right about anything an Office app would render.
//
// WHAT IS PRESERVED, ON PURPOSE. Paragraph and row boundaries, tabs, line
// breaks, table rows as tab-separated cells, sheet and slide order with a
// heading per sheet/slide. That is the structure a reader of the text can act
// on. Everything else — numbering, merges, formatting, drawings, comments — is
// dropped silently rather than approximated.
//
// WHY NOTHING HERE PANICS OR TRUSTS A HEADER. Every byte reaching this file
// came off an upload, so a part can lie about its size, claim to be XML and
// not be, or nest tags to a depth nothing wrote by hand. Each part is refused
// above a cap BEFORE it is inflated (the central directory's declared size)
// and again after (a header that lied), malformed XML comes back as an error
// rather than an unwrap, and the one real amplifier — a sheet whose cell
// references are sparse but far apart, which pads into megabytes of tabs — is
// bounded in both directions.

use std::io::{Cursor, Read, Seek};

use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, BytesText, Event};
use quick_xml::{Reader, XmlVersion};
use zip::ZipArchive;

use crate::{OfficeError, OfficeKind};

/// Refusal threshold for a single part, inflated. A real document's text parts
/// run in the low megabytes even when they are long; past this the file is
/// either a zip bomb or something no one is going to read as text.
const MAX_PART: u64 = 20 * 1024 * 1024;

/// Ceiling on the text one spreadsheet may produce, because column padding
/// (below) turns a small part into a big string. The caller clips too; this
/// only keeps the clip from having to happen on gigabytes.
const MAX_SHEET_TEXT: usize = 4 * 1024 * 1024;

/// Excel's own last column (XFD). A reference past it is a crafted file, not a
/// cell, and padding out to it would be the whole attack.
const MAX_COLS: usize = 16_384;

/// Extract the readable text of an Office document.
///
/// `kind` says which family the bytes belong to — [`crate::office_kind`]
/// decides that from the mime and filename, since the zip itself does not say.
pub fn extract_text(bytes: &[u8], kind: OfficeKind) -> Result<String, OfficeError> {
    // Not a zip at all is the common case for a mislabelled upload (a .doc
    // renamed, a PDF with the wrong mime), and it is not a malformed document
    // — it is not a document.
    let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|_| OfficeError::NotOffice)?;
    match kind {
        OfficeKind::Docx => docx_text(&mut zip),
        OfficeKind::Xlsx => xlsx_text(&mut zip),
        OfficeKind::Pptx => pptx_text(&mut zip),
    }
}

/// Read one part as a string, or `None` when the document does not carry it.
///
/// A missing part is not an error here: a deck without speaker notes, a
/// workbook with no shared strings and a slide listed in the rels but absent
/// from the zip all answer `None`, and each caller decides whether its own
/// main part going missing means the file was not a document after all.
fn part<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, OfficeError> {
    let Ok(entry) = zip.by_name(name) else {
        return Ok(None);
    };
    let declared = entry.size();
    // The cheap refusal: the central directory's claim, checked before a single
    // byte is inflated or allocated.
    if declared > MAX_PART {
        return Err(OfficeError::TooLarge);
    }
    let mut raw = Vec::with_capacity(declared as usize);
    // The second refusal, for a header that lied: read one byte past the cap
    // and refuse if it arrives.
    entry
        .take(MAX_PART + 1)
        .read_to_end(&mut raw)
        .map_err(|e| OfficeError::Malformed(e.to_string()))?;
    if raw.len() as u64 > MAX_PART {
        return Err(OfficeError::TooLarge);
    }
    String::from_utf8(raw)
        .map(Some)
        .map_err(|e| OfficeError::Malformed(e.to_string()))
}

fn malformed(e: quick_xml::Error) -> OfficeError {
    OfficeError::Malformed(e.to_string())
}

/// The text of one text node, line endings normalized as XML 1.0 (which is
/// what every OOXML part declares itself to be).
///
/// Entities are NOT in here: this parser reports `&amp;` and `&#8212;` as
/// their own events, which is what `ref_text` is for — a walk that handles
/// `Text` and forgets `GeneralRef` silently drops every ampersand, quote and
/// dash a document contains.
fn text_of(t: &BytesText<'_>) -> String {
    t.xml10_content().to_string()
}

/// The text one entity reference stands for.
///
/// Character references and the five XML predefined entities resolve. Anything
/// else (a `&nbsp;` from some converter, which is not even well-formed without
/// a DTD) is written back as the reference it was, rather than guessed at or
/// dropped.
fn ref_text(r: &BytesRef<'_>) -> String {
    match r.resolve_char_ref() {
        Ok(Some(c)) => c.to_string(),
        _ => match resolve_predefined_entity(r) {
            Some(resolved) => resolved.to_string(),
            None => format!("&{};", r.xml10_content()),
        },
    }
}

/// The value of an attribute, by qualified key, entities resolved.
///
/// Normalized as XML 1.0, which every OOXML part declares itself to be; the
/// 1.1 rules differ only over line endings inside an attribute value, which no
/// part this crate reads has one of.
fn attr(e: &BytesStart<'_>, key: &str) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == key)
        .map(|a| match a.normalized_value(XmlVersion::Explicit1_0) {
            Ok(value) => value.to_string(),
            Err(_) => a.value.to_string(),
        })
}

/// The relationship id attribute (`r:id`).
///
/// It cannot be found by local name: `<p:sldId id="256" r:id="rId2"/>` carries
/// two attributes whose local name is `id`, and the one that matters is the
/// namespace-prefixed one.
fn rel_id(e: &BytesStart<'_>) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == "id" && a.key.prefix().is_some())
        .map(|a| a.value.to_string())
}

/// Resolve a relationship target against the part directory it is relative to.
///
/// Targets come either absolute (`/xl/worksheets/sheet1.xml`) or relative to
/// the rels file's own part (`worksheets/sheet1.xml`); both spellings are in
/// the wild from Word, Excel, LibreOffice and every converter in between.
fn resolve(target: &str, base: &str) -> String {
    let target = target.trim();
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut rest = target;
    let mut dir = base.trim_end_matches('/').to_string();
    while let Some(up) = rest.strip_prefix("../") {
        // One level up out of the part directory; at the top it just stays.
        dir = dir
            .rsplit_once('/')
            .map_or(String::new(), |(head, _)| head.to_string());
        rest = up;
    }
    let rest = rest.strip_prefix("./").unwrap_or(rest);
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{dir}/{rest}")
    }
}

/// `Id` → `Target` pairs of a `.rels` part, in document order.
fn relationships(xml: &str) -> Result<Vec<(String, String)>, OfficeError> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    loop {
        let event = reader.read_event().map_err(malformed)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e)
                if e.local_name().as_ref() == "Relationship" =>
            {
                if let (Some(id), Some(target)) = (attr(e, "Id"), attr(e, "Target")) {
                    out.push((id, target));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn target_for(rels: &[(String, String)], id: &str) -> Option<String> {
    rels.iter()
        .find(|(rel_id, _)| rel_id == id)
        .map(|(_, target)| target.clone())
}

/// Join collected lines, dropping the trailing blank the walks leave behind.
fn joined(lines: Vec<String>) -> String {
    let mut out = lines.join("\n");
    out.truncate(out.trim_end().len());
    out
}

// ---------------------------------------------------------------- docx

/// `word/document.xml`: one line per `w:p`, tables as tab-separated rows.
fn docx_text<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<String, OfficeError> {
    let xml = part(zip, "word/document.xml")?.ok_or(OfficeError::NotOffice)?;
    let mut out: Vec<String> = Vec::new();
    let mut para = String::new();
    // A cell collects whole paragraphs and a row collects finished cells, so a
    // paragraph inside a table lands in its row instead of on its own line.
    // Nested tables (rare enough that Word hides the feature) flatten into the
    // row that is already open rather than replacing it.
    let mut cell: Option<Vec<String>> = None;
    let mut row: Option<Vec<String>> = None;
    // How deep in tables the walk is. Only the OUTER table's rows and cells
    // shape the output; a table nested in a cell contributes its words to
    // that cell, and its own `</w:tc>` must not close the outer cell early
    // — which is what would push half the outer cell's paragraphs out as
    // lines of their own.
    let mut tbl_depth = 0usize;
    // Only `w:t` carries words. `w:instrText` (field codes) and `w:delText`
    // (tracked deletions) are text nodes too and must not be collected.
    let mut in_text = 0usize;
    let mut reader = Reader::from_str(&xml);
    loop {
        match reader.read_event().map_err(malformed)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "p" => para.clear(),
                "t" => in_text += 1,
                "tab" => para.push('\t'),
                "br" | "cr" => para.push('\n'),
                "tbl" => tbl_depth += 1,
                "tr" if tbl_depth == 1 && row.is_none() => row = Some(Vec::new()),
                "tc" if tbl_depth == 1 && cell.is_none() => cell = Some(Vec::new()),
                _ => {}
            },
            // `<w:tab/>` and `<w:br/>` are self-closing in everything Word
            // writes; the Start arm above is for the converters that do not.
            Event::Empty(e) => match e.local_name().as_ref() {
                "tab" => para.push('\t'),
                "br" | "cr" => para.push('\n'),
                _ => {}
            },
            Event::Text(t) if in_text > 0 => para.push_str(&text_of(&t)),
            Event::GeneralRef(r) if in_text > 0 => para.push_str(&ref_text(&r)),
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = in_text.saturating_sub(1),
                "p" => {
                    let line = std::mem::take(&mut para);
                    match cell.as_mut() {
                        Some(cell) => cell.push(line),
                        None => out.push(line),
                    }
                }
                "tbl" => tbl_depth = tbl_depth.saturating_sub(1),
                "tc" if tbl_depth == 1 => {
                    if let (Some(done), Some(row)) = (cell.take(), row.as_mut()) {
                        // A cell holding several paragraphs is still one field
                        // of the row, so its paragraphs join with a space — a
                        // newline there would break the row apart.
                        row.push(done.join(" "));
                    }
                }
                "tr" if tbl_depth == 1 => {
                    if let Some(done) = row.take() {
                        out.push(done.join("\t"));
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(joined(out))
}

// ---------------------------------------------------------------- xlsx

/// `xl/sharedStrings.xml`: the string table cells point into by index.
fn shared_strings(xml: &str) -> Result<Vec<String>, OfficeError> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_item = false;
    let mut in_text = 0usize;
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(malformed)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "si" => {
                    in_item = true;
                    current.clear();
                }
                // A shared string split across formatting runs is still one
                // string; every `t` under the item concatenates.
                "t" if in_item => in_text += 1,
                _ => {}
            },
            Event::Text(t) if in_text > 0 => current.push_str(&text_of(&t)),
            Event::GeneralRef(r) if in_text > 0 => current.push_str(&ref_text(&r)),
            Event::End(e) => match e.local_name().as_ref() {
                "t" if in_item => in_text = in_text.saturating_sub(1),
                "si" => {
                    in_item = false;
                    out.push(std::mem::take(&mut current));
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// `xl/workbook.xml`: `(sheet name, relationship id)` in workbook order, which
/// is the order the tabs appear in and the only order worth reading them in.
fn workbook_sheets(xml: &str) -> Result<Vec<(String, String)>, OfficeError> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    loop {
        let event = reader.read_event().map_err(malformed)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) if e.local_name().as_ref() == "sheet" => {
                if let Some(id) = rel_id(e) {
                    let name = attr(e, "name").unwrap_or_else(|| "Sheet".to_string());
                    out.push((name, id));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// The 0-based column of a cell reference (`A1` → 0, `AA1` → 26).
fn column_of(reference: &str) -> Option<usize> {
    let letters: String = reference
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if letters.is_empty() {
        return None;
    }
    let mut column = 0usize;
    for c in letters.chars() {
        let digit = usize::from(c.to_ascii_uppercase() as u8 - b'A') + 1;
        // Bijective base 26. A crafted reference with twenty letters would
        // overflow, so the arithmetic is checked and a nonsense column reads
        // as no column at all.
        column = column.checked_mul(26)?.checked_add(digit)?;
        if column > MAX_COLS {
            return None;
        }
    }
    Some(column - 1)
}

/// One worksheet's rows, cells tab-separated, empty rows dropped.
fn sheet_rows(xml: &str, shared: &[String]) -> Result<Vec<String>, OfficeError> {
    let mut rows: Vec<String> = Vec::new();
    let mut size = 0usize;
    let mut cells: Vec<String> = Vec::new();
    let mut column = 0usize;
    let mut cell_type = String::new();
    let mut value = String::new();
    let mut inline = String::new();
    let mut in_value = false;
    let mut in_text = 0usize;
    let mut reader = Reader::from_str(xml);
    loop {
        let event = reader.read_event().map_err(malformed)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) if e.local_name().as_ref() == "c" => {
                // A cell without a usable reference lands in the next free
                // column, which is what a reader of a sparse sheet expects.
                column = attr(e, "r")
                    .and_then(|r| column_of(&r))
                    .unwrap_or(cells.len());
                cell_type = attr(e, "t").unwrap_or_default();
                value.clear();
                inline.clear();
            }
            Event::Start(ref e) => match e.local_name().as_ref() {
                "row" => cells.clear(),
                "v" => in_value = true,
                "t" => in_text += 1,
                _ => {}
            },
            Event::Text(ref t) if in_value => value.push_str(&text_of(t)),
            Event::Text(ref t) if in_text > 0 => inline.push_str(&text_of(t)),
            Event::GeneralRef(ref r) if in_value => value.push_str(&ref_text(r)),
            Event::GeneralRef(ref r) if in_text > 0 => inline.push_str(&ref_text(r)),
            Event::End(ref e) => match e.local_name().as_ref() {
                "v" => in_value = false,
                "t" => in_text = in_text.saturating_sub(1),
                "c" => {
                    let text = cell_text(&cell_type, &value, &inline, shared);
                    if !text.is_empty() && column < MAX_COLS {
                        if cells.len() <= column {
                            cells.resize(column + 1, String::new());
                        }
                        cells[column] = text;
                    }
                }
                "row" => {
                    // Trailing empties carry nothing; an interior one does,
                    // because dropping it would shift every later column.
                    while cells.last().is_some_and(String::is_empty) {
                        cells.pop();
                    }
                    if !cells.is_empty() {
                        let line = cells.join("\t");
                        size += line.len() + 1;
                        rows.push(line);
                        if size > MAX_SHEET_TEXT {
                            // Padding is the amplifier: two cells far apart
                            // pad to kilobytes of tabs per row. Stop reading
                            // rather than hand back a sheet nobody asked for
                            // — and SAY so, or a cut sheet reads as a whole
                            // one.
                            rows.push("[truncated]".to_string());
                            break;
                        }
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(rows)
}

/// One cell's text, by its declared type.
fn cell_text(cell_type: &str, value: &str, inline: &str, shared: &[String]) -> String {
    match cell_type {
        // An index into the shared string table.
        "s" => value
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|i| shared.get(i))
            .cloned()
            .unwrap_or_default(),
        // Written in the cell itself, either as an inline string or as a
        // formula's cached string result.
        "inlineStr" | "str" => {
            if inline.is_empty() {
                value.to_string()
            } else {
                inline.to_string()
            }
        }
        // A boolean is 0 or 1 on the wire. The words are what a reader of the
        // text expects, and what the sheet shows them anyway.
        "b" => if value.trim() == "1" { "TRUE" } else { "FALSE" }.to_string(),
        // Numbers (`n`, or no type at all) and error values read as written:
        // the digits a spreadsheet stores are what a reader of the text wants,
        // not a re-rendering of them through some number format.
        _ => {
            if inline.is_empty() {
                value.trim().to_string()
            } else {
                inline.to_string()
            }
        }
    }
}

fn xlsx_text<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<String, OfficeError> {
    let book = part(zip, "xl/workbook.xml")?.ok_or(OfficeError::NotOffice)?;
    let sheets = workbook_sheets(&book)?;
    let shared = match part(zip, "xl/sharedStrings.xml")? {
        Some(xml) => shared_strings(&xml)?,
        None => Vec::new(),
    };
    let rels = match part(zip, "xl/_rels/workbook.xml.rels")? {
        Some(xml) => relationships(&xml)?,
        None => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for (name, id) in sheets {
        let Some(target) = target_for(&rels, &id) else {
            continue;
        };
        let Some(xml) = part(zip, &resolve(&target, "xl/"))? else {
            continue;
        };
        if !out.is_empty() {
            out.push(String::new());
        }
        // The sheet name is a heading rather than a prefix per row: a reader
        // (human or model) needs to know which tab a row came from, and once
        // per sheet is enough to carry that.
        out.push(format!("## {name}"));
        out.extend(sheet_rows(&xml, &shared)?);
    }
    Ok(joined(out))
}

// ---------------------------------------------------------------- pptx

/// `ppt/presentation.xml`: the relationship ids of `p:sldIdLst`, in deck order.
fn slide_ids(xml: &str) -> Result<Vec<String>, OfficeError> {
    let mut out = Vec::new();
    let mut reader = Reader::from_str(xml);
    loop {
        let event = reader.read_event().map_err(malformed)?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) if e.local_name().as_ref() == "sldId" => {
                if let Some(id) = rel_id(e) {
                    out.push(id);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

/// One slide's lines: every `a:t` run, paragraphs on their own lines.
fn slide_lines(xml: &str) -> Result<Vec<String>, OfficeError> {
    let mut out = Vec::new();
    let mut para = String::new();
    let mut in_text = 0usize;
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event().map_err(malformed)? {
            Event::Start(e) => match e.local_name().as_ref() {
                "p" => para.clear(),
                "t" => in_text += 1,
                "br" => para.push('\n'),
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == "br" => para.push('\n'),
            Event::Text(t) if in_text > 0 => para.push_str(&text_of(&t)),
            Event::GeneralRef(r) if in_text > 0 => para.push_str(&ref_text(&r)),
            Event::End(e) => match e.local_name().as_ref() {
                "t" => in_text = in_text.saturating_sub(1),
                "p" => {
                    let line = std::mem::take(&mut para);
                    // A deck is mostly empty placeholders; a blank line per
                    // empty one would bury the slides that say something.
                    if !line.trim().is_empty() {
                        out.push(line);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out)
}

fn pptx_text<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<String, OfficeError> {
    let presentation = part(zip, "ppt/presentation.xml")?.ok_or(OfficeError::NotOffice)?;
    let ids = slide_ids(&presentation)?;
    let rels = match part(zip, "ppt/_rels/presentation.xml.rels")? {
        Some(xml) => relationships(&xml)?,
        None => Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let Some(target) = target_for(&rels, id) else {
            continue;
        };
        let Some(xml) = part(zip, &resolve(&target, "ppt/"))? else {
            continue;
        };
        if !out.is_empty() {
            out.push(String::new());
        }
        // Numbered by position in the deck, not by the order parts happen to
        // sit in the zip — "Slide 4" has to mean the fourth slide.
        out.push(format!("## Slide {}", index + 1));
        out.extend(slide_lines(&xml)?);
    }
    Ok(joined(out))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::fixtures::zip_of;

    const DOCUMENT: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
        "<w:body>",
        // Two runs of one paragraph are one line.
        "<w:p><w:r><w:t>Hello</w:t></w:r>",
        "<w:r><w:t xml:space=\"preserve\"> world</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>left</w:t><w:tab/><w:t>right</w:t></w:r></w:p>",
        "<w:p><w:r><w:t>first</w:t><w:br/><w:t>second</w:t></w:r></w:p>",
        "<w:tbl>",
        "<w:tr><w:tc><w:p><w:r><w:t>A1</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc></w:tr>",
        "<w:tr><w:tc><w:p><w:r><w:t>A2</w:t></w:r></w:p></w:tc>",
        "<w:tc><w:p><w:r><w:t>B2</w:t></w:r></w:p></w:tc></w:tr>",
        "</w:tbl>",
        // Field codes are text nodes too, and are not words anyone wrote.
        "<w:p><w:r><w:instrText>PAGE \\* MERGEFORMAT</w:instrText>",
        "<w:t>after &amp; done</w:t></w:r></w:p>",
        "</w:body></w:document>"
    );

    #[test]
    fn docx_paragraphs_tabs_breaks_and_tables() {
        let bytes = zip_of(&[("word/document.xml", DOCUMENT)]);
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        assert_eq!(
            text,
            "Hello world\nleft\tright\nfirst\nsecond\nA1\tB1\nA2\tB2\nafter & done"
        );
    }

    /// A table nested inside a cell belongs to that cell: its own cell and row
    /// ends must not close the OUTER cell early and spill the rest of that
    /// cell's paragraphs out as lines of their own.
    #[test]
    fn a_nested_table_stays_inside_its_cell() {
        const NESTED: &str = concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
            "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">",
            "<w:body><w:tbl><w:tr>",
            "<w:tc><w:p><w:r><w:t>outer start</w:t></w:r></w:p>",
            "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:tc></w:tr></w:tbl>",
            "<w:p><w:r><w:t>outer end</w:t></w:r></w:p></w:tc>",
            "<w:tc><w:p><w:r><w:t>B1</w:t></w:r></w:p></w:tc>",
            "</w:tr></w:tbl>",
            "<w:p><w:r><w:t>after</w:t></w:r></w:p>",
            "</w:body></w:document>"
        );
        let bytes = zip_of(&[("word/document.xml", NESTED)]);
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        assert_eq!(text, "outer start inner outer end\tB1\nafter");
    }

    /// Entity and character references are separate events in this parser, so
    /// a walk that only collected text would quietly drop every ampersand,
    /// quote and em dash in the document. This is that regression.
    #[test]
    fn entities_and_character_references_come_through() {
        let bytes = zip_of(&[(
            "word/document.xml",
            concat!(
                "<w:document><w:body><w:p><w:r><w:t>",
                "Tom &amp; Jerry &#8212; &quot;quoted&quot; &lt;tag&gt; &nbsp;",
                "</w:t></w:r></w:p></w:body></w:document>"
            ),
        )]);
        assert_eq!(
            extract_text(&bytes, OfficeKind::Docx).unwrap(),
            // The last one is nothing this parser can resolve without a DTD,
            // so it stays the reference it was rather than vanishing.
            "Tom & Jerry \u{2014} \"quoted\" <tag> &nbsp;"
        );
    }

    #[test]
    fn docx_without_its_main_part_is_not_a_document() {
        let bytes = zip_of(&[("word/footnotes.xml", "<w:footnotes/>")]);
        assert_eq!(
            extract_text(&bytes, OfficeKind::Docx),
            Err(OfficeError::NotOffice)
        );
    }

    fn workbook() -> Vec<u8> {
        zip_of(&[
            (
                "xl/workbook.xml",
                concat!(
                    "<workbook ",
                    "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\" ",
                    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/",
                    "relationships\"><sheets>",
                    "<sheet name=\"Summary\" sheetId=\"1\" r:id=\"rId1\"/>",
                    "<sheet name=\"Detail\" sheetId=\"2\" r:id=\"rId2\"/>",
                    "</sheets></workbook>"
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels",
                concat!(
                    "<Relationships ",
                    "xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
                    "<Relationship Id=\"rId1\" Target=\"worksheets/sheet1.xml\"/>",
                    // Absolute targets turn up too, from converters.
                    "<Relationship Id=\"rId2\" Target=\"/xl/worksheets/sheet2.xml\"/>",
                    "</Relationships>"
                ),
            ),
            (
                "xl/sharedStrings.xml",
                concat!(
                    "<sst xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
                    // A string split across formatting runs is still one string.
                    "<si><r><t>Re</t></r><r><t>gion</t></r></si>",
                    "<si><t>West</t></si>",
                    "</sst>"
                ),
            ),
            (
                "xl/worksheets/sheet1.xml",
                concat!(
                    "<worksheet ",
                    "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
                    "<sheetData>",
                    "<row r=\"1\"><c r=\"A1\" t=\"s\"><v>0</v></c>",
                    "<c r=\"B1\" t=\"inlineStr\"><is><t>Revenue</t></is></c></row>",
                    "<row r=\"2\"><c r=\"A2\" t=\"s\"><v>1</v></c>",
                    "<c r=\"B2\"><v>1200.5</v></c></row>",
                    "<row r=\"3\"/>",
                    "<row r=\"4\"><c r=\"A4\" t=\"inlineStr\"><is><t>Total</t></is></c>",
                    "<c r=\"C4\"><v>980</v></c></row>",
                    "</sheetData></worksheet>"
                ),
            ),
            (
                "xl/worksheets/sheet2.xml",
                concat!(
                    "<worksheet ",
                    "xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\">",
                    "<sheetData><row r=\"1\">",
                    "<c r=\"A1\" t=\"inlineStr\"><is><t>Detail sheet</t></is></c>",
                    "<c r=\"B1\" t=\"b\"><v>1</v></c>",
                    "</row></sheetData></worksheet>"
                ),
            ),
        ])
    }

    #[test]
    fn xlsx_sheets_in_workbook_order_with_shared_inline_and_numeric_cells() {
        let text = extract_text(&workbook(), OfficeKind::Xlsx).unwrap();
        assert_eq!(
            text,
            concat!(
                "## Summary\n",
                "Region\tRevenue\n",
                "West\t1200.5\n",
                // Row 3 held nothing and is gone; the gap at B4 is still a
                // column, so the 980 stays under C.
                "Total\t\t980\n",
                "\n",
                "## Detail\n",
                "Detail sheet\tTRUE"
            )
        );
    }

    #[test]
    fn xlsx_without_a_workbook_is_not_a_document() {
        let bytes = zip_of(&[("xl/worksheets/sheet1.xml", "<worksheet/>")]);
        assert_eq!(
            extract_text(&bytes, OfficeKind::Xlsx),
            Err(OfficeError::NotOffice)
        );
    }

    fn deck() -> Vec<u8> {
        zip_of(&[
            // Deliberately out of deck order in the zip: the order has to come
            // from sldIdLst, not from where the parts happen to sit.
            (
                "ppt/slides/slide2.xml",
                concat!(
                    "<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" ",
                    "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">",
                    "<p:cSld><p:spTree><p:sp><p:txBody>",
                    "<a:p><a:r><a:t>Second slide</a:t></a:r></a:p>",
                    "</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
                ),
            ),
            (
                "ppt/slides/slide1.xml",
                concat!(
                    "<p:sld xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" ",
                    "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">",
                    "<p:cSld><p:spTree><p:sp><p:txBody>",
                    "<a:p><a:r><a:t>Title</a:t></a:r><a:r><a:t> one</a:t></a:r></a:p>",
                    // An empty placeholder paragraph carries nothing.
                    "<a:p/>",
                    "<a:p><a:r><a:t>Bullet</a:t></a:r></a:p>",
                    "</p:txBody></p:sp></p:spTree></p:cSld></p:sld>"
                ),
            ),
            (
                "ppt/presentation.xml",
                concat!(
                    "<p:presentation ",
                    "xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\" ",
                    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/",
                    "relationships\"><p:sldIdLst>",
                    "<p:sldId id=\"256\" r:id=\"rId2\"/>",
                    "<p:sldId id=\"257\" r:id=\"rId3\"/>",
                    "</p:sldIdLst></p:presentation>"
                ),
            ),
            (
                "ppt/_rels/presentation.xml.rels",
                concat!(
                    "<Relationships ",
                    "xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
                    "<Relationship Id=\"rId2\" Target=\"slides/slide1.xml\"/>",
                    "<Relationship Id=\"rId3\" Target=\"slides/slide2.xml\"/>",
                    "</Relationships>"
                ),
            ),
        ])
    }

    #[test]
    fn pptx_slides_in_deck_order() {
        let text = extract_text(&deck(), OfficeKind::Pptx).unwrap();
        assert_eq!(
            text,
            "## Slide 1\nTitle one\nBullet\n\n## Slide 2\nSecond slide"
        );
    }

    #[test]
    fn a_part_over_the_cap_is_refused_on_its_declared_size() {
        // Written in chunks, so neither the fixture nor the refusal needs 21
        // MiB in memory; the entry deflates to nothing because it is spaces.
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        let chunk = vec![b' '; 64 * 1024];
        for _ in 0..(21 * 16) {
            zip.write_all(&chunk).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        // The proof that nothing inflated it: the archive itself is tiny, and
        // the refusal comes back anyway.
        assert!(bytes.len() < 200_000, "fixture was {} bytes", bytes.len());
        assert_eq!(
            extract_text(&bytes, OfficeKind::Docx),
            Err(OfficeError::TooLarge)
        );
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        let bytes = zip_of(&[(
            "word/document.xml",
            "<w:document><w:body><w:p><w:t>hi</w:x></w:p></w:body></w:document>",
        )]);
        let error = extract_text(&bytes, OfficeKind::Docx).unwrap_err();
        assert!(
            matches!(error, OfficeError::Malformed(ref detail) if !detail.is_empty()),
            "{error:?}"
        );
    }

    #[test]
    fn a_truncated_part_is_an_error_not_a_panic() {
        let bytes = zip_of(&[("ppt/presentation.xml", "<p:sldIdLst><p:sldId")]);
        assert!(matches!(
            extract_text(&bytes, OfficeKind::Pptx),
            Err(OfficeError::Malformed(_))
        ));
    }

    #[test]
    fn bytes_that_are_not_a_zip_are_not_a_document() {
        assert_eq!(
            extract_text(b"%PDF-1.7 not a zip at all", OfficeKind::Docx),
            Err(OfficeError::NotOffice)
        );
        assert_eq!(
            extract_text(&[], OfficeKind::Xlsx),
            Err(OfficeError::NotOffice)
        );
    }

    #[test]
    fn column_references_past_z_and_nonsense_ones() {
        assert_eq!(column_of("A1"), Some(0));
        assert_eq!(column_of("Z9"), Some(25));
        assert_eq!(column_of("AA1"), Some(26));
        assert_eq!(column_of("AB2"), Some(27));
        assert_eq!(column_of("XFD1048576"), Some(16_383));
        // Past Excel's last column, and with no column at all: a crafted
        // reference must not become a resize of that many cells.
        assert_eq!(column_of("XFE1"), None);
        assert_eq!(column_of("ZZZZZZZZZZ1"), None);
        assert_eq!(column_of("1"), None);
    }

    #[test]
    fn relationship_targets_resolve_both_spellings() {
        assert_eq!(
            resolve("worksheets/sheet1.xml", "xl/"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve("/xl/worksheets/sheet1.xml", "xl/"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve("./slides/slide1.xml", "ppt/"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(
            resolve("../media/image1.png", "ppt/slides/"),
            "ppt/media/image1.png"
        );
    }
}
