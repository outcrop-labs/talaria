// WRITING an OOXML document: a markdown document out as .docx, a grid out as
// .xlsx, both as the smallest file Word and Excel will open without a repair
// prompt.
//
// WHY WRITE THE XML BY HAND. The export's whole job is to hand someone a file
// they can open, edit and send on. That needs five short parts and a style
// sheet — not a document model. Writing the parts as strings keeps the
// dependency surface at the zip crate, keeps the output byte-stable (same
// input, same bytes, so a caller may hash or cache it), and means the one
// thing that can go wrong — unescaped text breaking the XML — is a single
// function away from every call site.
//
// WHAT "MINIMAL BUT VALID" MEANS HERE. Content types, package relationships,
// the part relationships each main part needs, and core properties carrying
// the title. Numbering definitions are deliberately absent: a list is a
// `ListParagraph` with its bullet or number written into the text, because
// real numbering (numbering.xml, abstract definitions, level overrides) is
// several times this file's size and buys a renumbering behaviour nobody
// asked an export for.
//
// WHY THESE RETURN BYTES AND NOT A RESULT. Everything is built in memory, so
// the only failures the zip writer can report are the ones that cannot happen
// to a `Cursor<Vec<u8>>`. A `Result` there would be a lie every caller has to
// unwrap anyway.

use std::io::{Cursor, Write};

use quick_xml::escape::escape;
use zip::write::SimpleFileOptions;
use zip::{DateTime, ZipWriter};

const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";
const CONTENT_TYPES_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const PACKAGE_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const WORD_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const SHEET_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const CORE_PROPS_TYPE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
const OFFICE_DOC_TYPE: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";

/// The monospace face code runs and code blocks ask for. Present on Windows
/// Word and mapped to something monospaced everywhere else, which is all a
/// code run needs.
const MONO: &str = "Consolas";

/// Text on its way into a part: the characters XML cannot carry dropped, then
/// the five predefined entities escaped.
///
/// `escape` alone is the whole job for text a person typed. It is not the whole
/// job for text that came out of a program, and an artifact body routinely did:
/// XML 1.0 has no spelling at all for a C0 control other than tab, newline and
/// return — not a raw byte, not a character reference — so a stray ANSI escape
/// or NUL from pasted terminal output would write a part no parser will read,
/// and Word answers that with a repair prompt rather than a document. Dropping
/// the character loses nothing a reader could have seen on the page; keeping it
/// loses the file.
fn xml_text(raw: &str) -> String {
    if raw.chars().all(xml_char) {
        return escape(raw).into_owned();
    }
    let kept: String = raw.chars().filter(|c| xml_char(*c)).collect();
    escape(kept.as_str()).into_owned()
}

/// XML 1.0's `Char` production, which is narrower than Rust's `char`.
const fn xml_char(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\r'
            | ' '..='\u{d7ff}'
            | '\u{e000}'..='\u{fffd}'
            | '\u{10000}'..='\u{10ffff}'
    )
}

/// Pack parts into a zip in memory.
///
/// `[Content_Types].xml` goes first by convention — some readers look for it
/// at the head of the archive before falling back to a scan.
fn pack(parts: &[(&str, String)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    // A fixed 1980 timestamp rather than "now": the same document exported
    // twice should be the same bytes, which is what makes the output safe to
    // hash, cache or compare in a test.
    let options = SimpleFileOptions::default().last_modified_time(DateTime::default());
    for (name, body) in parts {
        zip.start_file(*name, options)
            .expect("an in-memory zip entry cannot fail to start");
        zip.write_all(body.as_bytes())
            .expect("an in-memory zip entry cannot fail to write");
    }
    zip.finish()
        .expect("an in-memory zip cannot fail to finish")
        .into_inner()
}

fn core_props(title: &str) -> String {
    format!(
        "{DECL}\n<cp:coreProperties \
         xmlns:cp=\"http://schemas.openxmlformats.org/package/2006/metadata/core-properties\" \
         xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:dcterms=\"http://purl.org/dc/terms/\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\">\
         <dc:title>{}</dc:title></cp:coreProperties>",
        xml_text(title)
    )
}

fn package_rels(main_part: &str) -> String {
    format!(
        "{DECL}\n<Relationships xmlns=\"{PACKAGE_REL_NS}\">\
         <Relationship Id=\"rId1\" Type=\"{OFFICE_DOC_TYPE}\" Target=\"{main_part}\"/>\
         <Relationship Id=\"rId2\" Type=\"{CORE_PROPS_TYPE}\" Target=\"docProps/core.xml\"/>\
         </Relationships>"
    )
}

// ---------------------------------------------------------------- docx

/// What a run of text looks like. Nothing else is expressible: the markdown
/// subset below produces bold, italic and code, and a heading or list is a
/// property of the paragraph rather than the run.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
}

struct Run {
    text: String,
    style: Style,
}

/// Turn a markdown document into a .docx.
///
/// The subset is line-based: `#`/`##`/`###` headings, blank-line-separated
/// paragraphs, `-`/`*` bullets, `1.` numbered items, fenced code blocks, and
/// `**bold**` / `*italic*` / `` `code` `` inside a line. Anything else is
/// carried through as the plain text it looks like, which is the right answer
/// for a format people write by hand — a table or a link that came out as its
/// source text is readable; a paragraph silently dropped is not.
pub fn markdown_to_docx(title: &str, markdown: &str) -> Vec<u8> {
    let mut body = String::new();
    // Lines of the paragraph being accumulated. A hard-wrapped markdown
    // paragraph is one Word paragraph, so these join rather than break.
    let mut pending: Vec<&str> = Vec::new();
    let mut fenced = false;
    for line in markdown.lines() {
        let trimmed = line.trim_end();
        let starts_fence = trimmed.trim_start().starts_with("```");
        if fenced {
            if starts_fence {
                fenced = false;
            } else {
                // Verbatim: indentation and markers inside a code block are
                // the content, not markup.
                paragraph(
                    &mut body,
                    None,
                    &[Run {
                        text: trimmed.to_string(),
                        style: Style {
                            code: true,
                            ..Style::default()
                        },
                    }],
                );
            }
            continue;
        }
        if starts_fence {
            flush(&mut body, &mut pending);
            fenced = true;
            continue;
        }
        if trimmed.trim().is_empty() {
            flush(&mut body, &mut pending);
            continue;
        }
        if let Some((level, rest)) = heading(trimmed) {
            flush(&mut body, &mut pending);
            paragraph(&mut body, Some(&format!("Heading{level}")), &runs(rest));
            continue;
        }
        if let Some((prefix, rest)) = list_item(trimmed) {
            flush(&mut body, &mut pending);
            let mut item = vec![Run {
                text: prefix,
                style: Style::default(),
            }];
            item.extend(runs(rest));
            paragraph(&mut body, Some("ListParagraph"), &item);
            continue;
        }
        pending.push(trimmed);
    }
    flush(&mut body, &mut pending);

    let document = format!(
        "{DECL}\n<w:document xmlns:w=\"{WORD_NS}\"><w:body>{body}\
         <w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/>\
         <w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/>\
         </w:sectPr></w:body></w:document>"
    );
    pack(&[
        ("[Content_Types].xml", docx_content_types()),
        ("_rels/.rels", package_rels("word/document.xml")),
        ("word/document.xml", document),
        ("word/_rels/document.xml.rels", document_rels()),
        ("word/styles.xml", styles()),
        ("docProps/core.xml", core_props(title)),
    ])
}

fn docx_content_types() -> String {
    format!(
        "{DECL}\n<Types xmlns=\"{CONTENT_TYPES_NS}\">\
         <Default Extension=\"rels\" \
         ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.\
         openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>\
         <Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.\
         openxmlformats-officedocument.wordprocessingml.styles+xml\"/>\
         <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.\
         openxmlformats-package.core-properties+xml\"/></Types>"
    )
}

fn document_rels() -> String {
    format!(
        "{DECL}\n<Relationships xmlns=\"{PACKAGE_REL_NS}\">\
         <Relationship Id=\"rId1\" Type=\"{REL_NS}/styles\" Target=\"styles.xml\"/>\
         </Relationships>"
    )
}

/// The style sheet the paragraphs reference. Four styles, because four is what
/// the markdown subset can produce; sizes are in half-points, Word's unit.
fn styles() -> String {
    let mut out = format!("{DECL}\n<w:styles xmlns:w=\"{WORD_NS}\">");
    out.push_str(
        "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\">\
         <w:name w:val=\"Normal\"/><w:rPr>\
         <w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\"/><w:sz w:val=\"22\"/>\
         </w:rPr></w:style>",
    );
    for (level, size) in [(1u8, 32u8), (2, 28), (3, 24)] {
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\">\
             <w:name w:val=\"heading {level}\"/><w:basedOn w:val=\"Normal\"/>\
             <w:pPr><w:keepNext/><w:spacing w:before=\"240\" w:after=\"120\"/>\
             <w:outlineLvl w:val=\"{outline}\"/></w:pPr>\
             <w:rPr><w:b/><w:sz w:val=\"{size}\"/></w:rPr></w:style>",
            outline = level - 1
        ));
    }
    out.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\">\
         <w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/>\
         <w:pPr><w:ind w:left=\"720\"/><w:contextualSpacing/></w:pPr></w:style>",
    );
    out.push_str("</w:styles>");
    out
}

/// Emit one `w:p`, optionally carrying a paragraph style.
fn paragraph(out: &mut String, style: Option<&str>, runs: &[Run]) {
    out.push_str("<w:p>");
    if let Some(style) = style {
        out.push_str(&format!("<w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr>"));
    }
    for run in runs {
        out.push_str("<w:r>");
        if run.style != Style::default() {
            out.push_str("<w:rPr>");
            if run.style.bold {
                out.push_str("<w:b/>");
            }
            if run.style.italic {
                out.push_str("<w:i/>");
            }
            if run.style.code {
                out.push_str(&format!(
                    "<w:rFonts w:ascii=\"{MONO}\" w:hAnsi=\"{MONO}\" w:cs=\"{MONO}\"/>"
                ));
            }
            out.push_str("</w:rPr>");
        }
        // xml:space="preserve" because a run very often ends in the space
        // before the next run, and Word would otherwise collapse it away.
        out.push_str(&format!(
            "<w:t xml:space=\"preserve\">{}</w:t>",
            xml_text(run.text.as_str())
        ));
        out.push_str("</w:r>");
    }
    out.push_str("</w:p>");
}

/// Close off the paragraph being accumulated, if any.
fn flush(out: &mut String, pending: &mut Vec<&str>) {
    if pending.is_empty() {
        return;
    }
    let text = pending.join(" ");
    pending.clear();
    paragraph(out, None, &runs(&text));
}

/// `#`, `##` or `###` and the text after it. Deeper hashes are not headings in
/// this subset; they fall through to plain text with the hashes intact, which
/// at least shows what the source said.
fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 3 {
        return None;
    }
    let rest = line[hashes..].strip_prefix(' ')?;
    Some((hashes as u8, rest.trim()))
}

/// A bullet or numbered item, as `(prefix to write, the item's text)`.
///
/// The prefix is text because there is no numbering.xml: `• ` and `3. ` are
/// characters in the paragraph, so what the author wrote is what the reader
/// sees — including a list that starts at 3.
fn list_item(line: &str) -> Option<(String, &str)> {
    let line = line.trim_start();
    for marker in ["- ", "* "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(("• ".to_string(), rest.trim()));
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = line[digits..].strip_prefix(". ")?;
    Some((format!("{}. ", &line[..digits]), rest.trim()))
}

/// Split a line into styled runs.
fn runs(line: &str) -> Vec<Run> {
    styled_runs(line, Style::default(), 0)
}

fn styled_runs(line: &str, base: Style, depth: u8) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    let mut plain = String::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A code span binds tightest and takes its content verbatim, so
        // `**` inside backticks stays two asterisks.
        if bytes[i] == b'`'
            && let Some(end) = line[i + 1..].find('`')
        {
            let inner = &line[i + 1..i + 1 + end];
            push_plain(&mut out, &mut plain, base);
            out.push(Run {
                text: inner.to_string(),
                style: Style { code: true, ..base },
            });
            i += end + 2;
            continue;
        }
        if bytes[i] == b'*' {
            let marker = if line[i..].starts_with("**") {
                "**"
            } else {
                "*"
            };
            let after = i + marker.len();
            // An unmatched marker is just an asterisk someone typed.
            if let Some(end) = line[after..].find(marker).filter(|end| *end > 0) {
                let inner = &line[after..after + end];
                let mut style = base;
                if marker == "**" {
                    style.bold = true;
                } else {
                    style.italic = true;
                }
                push_plain(&mut out, &mut plain, base);
                // One more level in, so `**bold with *emphasis*** ` works;
                // past that the markers stay literal rather than recursing on
                // whatever a generator produced.
                if depth < 2 {
                    out.extend(styled_runs(inner, style, depth + 1));
                } else {
                    out.push(Run {
                        text: inner.to_string(),
                        style,
                    });
                }
                i = after + end + marker.len();
                continue;
            }
        }
        // Byte indexing is safe only because every branch above advances past
        // ASCII markers; this one steps a whole character.
        let Some(c) = line[i..].chars().next() else {
            break;
        };
        plain.push(c);
        i += c.len_utf8();
    }
    push_plain(&mut out, &mut plain, base);
    out
}

fn push_plain(out: &mut Vec<Run>, plain: &mut String, style: Style) {
    if !plain.is_empty() {
        out.push(Run {
            text: std::mem::take(plain),
            style,
        });
    }
}

// ---------------------------------------------------------------- xlsx

/// Turn a grid of strings into a single-sheet .xlsx.
///
/// Cells that parse as a finite number are written as numbers so the sheet
/// sums and sorts; everything else is an inline string, which keeps the file
/// to one part per sheet instead of a shared string table that a one-shot
/// export would never reuse.
pub fn grid_to_xlsx(title: &str, grid: &[Vec<String>]) -> Vec<u8> {
    let name = sheet_name(title);
    let mut sheet = format!("{DECL}\n<worksheet xmlns=\"{SHEET_NS}\"><sheetData>");
    for (row_index, row) in grid.iter().enumerate() {
        let number = row_index + 1;
        sheet.push_str(&format!("<row r=\"{number}\">"));
        for (column_index, cell) in row.iter().enumerate() {
            if cell.is_empty() {
                // An absent `c` element IS the empty cell; writing one would
                // only make the file bigger.
                continue;
            }
            let reference = format!("{}{number}", column_name(column_index));
            match numeric(cell) {
                Some(value) => {
                    sheet.push_str(&format!("<c r=\"{reference}\"><v>{value}</v></c>"));
                }
                None => sheet.push_str(&format!(
                    "<c r=\"{reference}\" t=\"inlineStr\"><is>\
                     <t xml:space=\"preserve\">{}</t></is></c>",
                    xml_text(cell.as_str())
                )),
            }
        }
        sheet.push_str("</row>");
    }
    sheet.push_str("</sheetData></worksheet>");

    let workbook = format!(
        "{DECL}\n<workbook xmlns=\"{SHEET_NS}\" xmlns:r=\"{REL_NS}\"><sheets>\
         <sheet name=\"{}\" sheetId=\"1\" r:id=\"rId1\"/></sheets></workbook>",
        xml_text(name.as_str())
    );
    pack(&[
        ("[Content_Types].xml", xlsx_content_types()),
        ("_rels/.rels", package_rels("xl/workbook.xml")),
        ("xl/workbook.xml", workbook),
        ("xl/_rels/workbook.xml.rels", workbook_rels()),
        ("xl/worksheets/sheet1.xml", sheet),
        ("xl/styles.xml", sheet_styles()),
        ("docProps/core.xml", core_props(title)),
    ])
}

fn xlsx_content_types() -> String {
    format!(
        "{DECL}\n<Types xmlns=\"{CONTENT_TYPES_NS}\">\
         <Default Extension=\"rels\" \
         ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.\
         openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/>\
         <Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.\
         openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/>\
         <Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.\
         openxmlformats-officedocument.spreadsheetml.styles+xml\"/>\
         <Override PartName=\"/docProps/core.xml\" ContentType=\"application/vnd.\
         openxmlformats-package.core-properties+xml\"/></Types>"
    )
}

fn workbook_rels() -> String {
    format!(
        "{DECL}\n<Relationships xmlns=\"{PACKAGE_REL_NS}\">\
         <Relationship Id=\"rId1\" Type=\"{REL_NS}/worksheet\" \
         Target=\"worksheets/sheet1.xml\"/>\
         <Relationship Id=\"rId2\" Type=\"{REL_NS}/styles\" Target=\"styles.xml\"/>\
         </Relationships>"
    )
}

/// The style sheet Excel insists on finding once it is referenced: one font,
/// one fill, one border, one cell format. Nothing in the export styles a cell,
/// but the part has to exist and be well-formed.
fn sheet_styles() -> String {
    format!(
        "{DECL}\n<styleSheet xmlns=\"{SHEET_NS}\">\
         <fonts count=\"1\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts>\
         <fills count=\"1\"><fill><patternFill patternType=\"none\"/></fill></fills>\
         <borders count=\"1\"><border/></borders>\
         <cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" \
         borderId=\"0\"/></cellStyleXfs>\
         <cellXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" \
         xfId=\"0\"/></cellXfs>\
         <cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles>\
         </styleSheet>"
    )
}

/// A sheet name Excel will accept: at most 31 characters, none of the seven it
/// reserves for references, never edged with an apostrophe, and never empty.
fn sheet_name(title: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| match c {
            ':' | '\\' | '/' | '?' | '*' | '[' | ']' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .take(31)
        .collect();
    // Truncation can land just after a space; a sheet tab that ends in
    // whitespace is nobody's intention.
    let cleaned = cleaned.trim_end();
    // A name may not BEGIN or end with an apostrophe either: a sheet
    // reference quotes the name with those, so Excel refuses the file rather
    // than guess where the quoting stops. A title like "’Til next quarter"
    // written with a straight quote is an ordinary title, not a crafted one.
    let cleaned = cleaned.trim_matches('\'');
    if cleaned.is_empty() {
        "Sheet1".to_string()
    } else {
        cleaned.to_string()
    }
}

/// The cell's value if it is a plain finite number, as written.
///
/// Written as given rather than reformatted through `f64`, so `007` and `1.50`
/// keep the digits the caller had — and the character filter is what keeps
/// `inf`, `NaN` and `1_0` (all of which `f64` parsing would take, or take and
/// then refuse) out of a `v` element.
fn numeric(cell: &str) -> Option<&str> {
    let trimmed = cell.trim();
    if trimmed.is_empty()
        || !trimmed
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e' | b'E'))
    {
        return None;
    }
    trimmed
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|_| trimmed)
}

/// Spreadsheet column letters for a 0-based index: `A`, …, `Z`, `AA`, `AB`.
///
/// Bijective base 26 — there is no zero digit, which is why the carry
/// subtracts one on the way out instead of dividing cleanly.
fn column_name(index: usize) -> String {
    let mut letters = Vec::new();
    let mut remaining = index;
    loop {
        letters.push(b'A' + (remaining % 26) as u8);
        if remaining < 26 {
            break;
        }
        remaining = remaining / 26 - 1;
    }
    letters.reverse();
    String::from_utf8(letters).unwrap_or_else(|_| "A".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{names_in, part_of};
    use crate::{OfficeKind, extract_text};

    #[test]
    fn a_docx_is_a_package_with_the_parts_word_looks_for() {
        let bytes = markdown_to_docx("Report", "# Report\n\nBody.\n");
        let names = names_in(&bytes);
        for part in [
            "[Content_Types].xml",
            "_rels/.rels",
            "word/document.xml",
            "word/_rels/document.xml.rels",
            "word/styles.xml",
            "docProps/core.xml",
        ] {
            assert!(
                names.contains(&part.to_string()),
                "{part} missing from {names:?}"
            );
        }
        // The content types part comes first, where a reader expects it.
        assert_eq!(
            names.first().map(String::as_str),
            Some("[Content_Types].xml")
        );
        assert!(part_of(&bytes, "docProps/core.xml").contains("<dc:title>Report</dc:title>"));
        assert!(part_of(&bytes, "word/styles.xml").contains("w:styleId=\"ListParagraph\""));
    }

    #[test]
    fn an_xlsx_is_a_package_with_the_parts_excel_looks_for() {
        let bytes = grid_to_xlsx("Numbers", &[vec!["a".to_string()]]);
        let names = names_in(&bytes);
        for part in [
            "[Content_Types].xml",
            "_rels/.rels",
            "xl/workbook.xml",
            "xl/_rels/workbook.xml.rels",
            "xl/worksheets/sheet1.xml",
            "xl/styles.xml",
            "docProps/core.xml",
        ] {
            assert!(
                names.contains(&part.to_string()),
                "{part} missing from {names:?}"
            );
        }
        assert_eq!(
            names.first().map(String::as_str),
            Some("[Content_Types].xml")
        );
    }

    /// Every part has to be well-formed XML, including the ones this crate
    /// never reads back: a typo in the content types or the style sheet shows
    /// up as Word's repair prompt, which no other test here would see.
    #[test]
    fn every_part_of_both_writers_is_well_formed() {
        let docx = markdown_to_docx(
            "Report & more",
            "# Report\n\n- item\n\nText with `code` and **bold**.\n\n```\nfn main() {}\n```\n",
        );
        let xlsx = grid_to_xlsx(
            "Sheet <1>",
            &[vec!["a & b".to_string(), "1200.5".to_string()]],
        );
        for bytes in [docx, xlsx] {
            for name in names_in(&bytes) {
                let xml = part_of(&bytes, &name);
                let mut reader = quick_xml::Reader::from_str(&xml);
                loop {
                    match reader.read_event() {
                        Ok(quick_xml::events::Event::Eof) => break,
                        Ok(_) => {}
                        Err(error) => panic!("{name} is not well formed: {error}"),
                    }
                }
            }
        }
    }

    #[test]
    fn the_same_input_writes_the_same_bytes() {
        assert_eq!(
            markdown_to_docx("T", "# T\n\nbody\n"),
            markdown_to_docx("T", "# T\n\nbody\n")
        );
    }

    #[test]
    fn every_text_is_escaped() {
        let markdown = "# <Q3> & \"the rest\"\n\nAlice & Bob said \"<ship it>\".\n";
        let bytes = markdown_to_docx("5 < 6 & \"true\"", markdown);
        let document = part_of(&bytes, "word/document.xml");
        assert!(
            document.contains("&lt;Q3&gt; &amp; &quot;the rest&quot;"),
            "{document}"
        );
        assert!(!document.contains("<Q3>"), "{document}");
        assert!(part_of(&bytes, "docProps/core.xml").contains("5 &lt; 6 &amp; &quot;true&quot;"));
        // And it survives the trip back out as the characters the author typed.
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        assert!(text.contains("Alice & Bob said \"<ship it>\"."), "{text:?}");
    }

    /// The characters XML has no spelling for. An agent pasting terminal
    /// output puts ANSI escapes in a body, and a NUL or a form feed gets in
    /// the same way; every one of them is outside XML 1.0's `Char` production,
    /// so a part that carried one would come back from any parser — and from
    /// Word — as not well formed. The assertion is on the re-parse, because
    /// that is the failure a reader of the bytes would never see.
    #[test]
    fn characters_xml_cannot_carry_are_dropped_not_written() {
        let markdown = "# Head\u{1b}[0m\n\nbody\u{0}and\u{c}more\n";
        let bytes = markdown_to_docx("Title\u{7}", markdown);
        for name in ["word/document.xml", "docProps/core.xml"] {
            let xml = part_of(&bytes, name);
            for forbidden in ['\u{0}', '\u{7}', '\u{c}', '\u{1b}'] {
                assert!(
                    !xml.contains(forbidden),
                    "{name} still carries {forbidden:?}"
                );
            }
        }
        // The words either side of the dropped character stay put.
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        assert_eq!(text, "Head[0m\nbodyandmore");

        // And the same gate on the workbook's inline strings.
        let sheet = grid_to_xlsx(
            "Grid\u{1b}",
            &[vec!["a\u{0}b".to_string(), "plain".to_string()]],
        );
        assert_eq!(
            extract_text(&sheet, OfficeKind::Xlsx).unwrap(),
            "## Grid\nab\tplain"
        );
    }

    #[test]
    fn lists_carry_their_own_markers() {
        let markdown = "- first\n* second\n\n1. one\n7. seven\n";
        let bytes = markdown_to_docx("Lists", markdown);
        let document = part_of(&bytes, "word/document.xml");
        // Four list paragraphs, no numbering definitions anywhere.
        assert_eq!(document.matches("w:val=\"ListParagraph\"").count(), 4);
        assert!(!names_in(&bytes).contains(&"word/numbering.xml".to_string()));
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        // The number the author wrote is the number the reader sees.
        assert_eq!(text, "• first\n• second\n1. one\n7. seven");
    }

    #[test]
    fn headings_paragraphs_and_soft_wraps() {
        let markdown = "# One\n## Two\n### Three\n#### Four\n\nA line\nand its wrap\n";
        let bytes = markdown_to_docx("Headings", markdown);
        let document = part_of(&bytes, "word/document.xml");
        for style in ["Heading1", "Heading2", "Heading3"] {
            assert!(document.contains(&format!("w:val=\"{style}\"")), "{style}");
        }
        // Four hashes is not a heading in this subset, so it stays visible as
        // the text it was rather than vanishing.
        assert!(!document.contains("Heading4"));
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        assert_eq!(text, "One\nTwo\nThree\n#### Four\nA line and its wrap");
    }

    #[test]
    fn inline_styles_and_code_fences() {
        let markdown = "**Bold** and *italic* and `code()` and a stray * asterisk.\n\n\
                        ```rust\n    let x = **1**;\n```\n";
        let bytes = markdown_to_docx("Styles", markdown);
        let document = part_of(&bytes, "word/document.xml");
        assert!(document.contains("<w:b/>"), "{document}");
        assert!(document.contains("<w:i/>"), "{document}");
        assert!(document.contains(MONO), "{document}");
        assert!(!document.contains("```"), "{document}");
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        assert_eq!(
            text,
            "Bold and italic and code() and a stray * asterisk.\n    let x = **1**;"
        );
    }

    #[test]
    fn nested_emphasis_keeps_both() {
        let bytes = markdown_to_docx("N", "**bold with *more* inside**\n");
        let document = part_of(&bytes, "word/document.xml");
        assert!(document.contains("<w:b/><w:i/>"), "{document}");
        assert_eq!(
            extract_text(&bytes, OfficeKind::Docx).unwrap(),
            "bold with more inside"
        );
    }

    #[test]
    fn cell_references_run_past_z() {
        assert_eq!(column_name(0), "A");
        assert_eq!(column_name(25), "Z");
        assert_eq!(column_name(26), "AA");
        assert_eq!(column_name(27), "AB");
        assert_eq!(column_name(701), "ZZ");
        assert_eq!(column_name(702), "AAA");
        assert_eq!(column_name(16_383), "XFD");

        // And the sheet writes them the same way: 27 columns on row one, 28 on
        // row two.
        let wide = |count: usize| (0..count).map(|i| format!("c{i}")).collect::<Vec<_>>();
        let bytes = grid_to_xlsx("Wide", &[wide(27), wide(28)]);
        let sheet = part_of(&bytes, "xl/worksheets/sheet1.xml");
        assert!(sheet.contains("r=\"AA1\""), "{sheet}");
        assert!(sheet.contains("r=\"AB2\""), "{sheet}");
        assert!(!sheet.contains("r=\"AC"), "{sheet}");
    }

    #[test]
    fn numbers_are_numbers_and_everything_else_is_a_string() {
        let grid = vec![vec![
            "1200.5".to_string(),
            "-3".to_string(),
            "007".to_string(),
            "1.2e3".to_string(),
            "12 units".to_string(),
            "NaN".to_string(),
            "inf".to_string(),
            String::new(),
        ]];
        let bytes = grid_to_xlsx("Types", &grid);
        let sheet = part_of(&bytes, "xl/worksheets/sheet1.xml");
        // Numeric cells carry a bare value and the digits as written.
        assert!(sheet.contains("<c r=\"A1\"><v>1200.5</v></c>"), "{sheet}");
        assert!(sheet.contains("<c r=\"C1\"><v>007</v></c>"), "{sheet}");
        assert!(sheet.contains("<c r=\"D1\"><v>1.2e3</v></c>"), "{sheet}");
        // The ones f64 would take but a spreadsheet cannot hold stay strings.
        assert!(sheet.contains("r=\"F1\" t=\"inlineStr\""), "{sheet}");
        assert!(sheet.contains("r=\"G1\" t=\"inlineStr\""), "{sheet}");
        // An empty cell is written as no cell at all.
        assert!(!sheet.contains("r=\"H1\""), "{sheet}");
    }

    #[test]
    fn sheet_names_are_what_excel_accepts() {
        assert_eq!(sheet_name("Revenue"), "Revenue");
        // The seven reserved characters go; the length stops at 31.
        let long = sheet_name("Q1/Q2 plan: [draft] *final* ? really long title");
        assert_eq!(long, "Q1-Q2 plan- -draft- -final- - r");
        assert_eq!(long.chars().count(), 31);
        assert!(!long.contains(['/', '\\', ':', '?', '*', '[', ']']));
        // An apostrophe is what a sheet reference quotes a name WITH, so one
        // on either end is a file Excel refuses to open.
        assert_eq!(sheet_name("'Til next quarter'"), "Til next quarter");
        // Never empty, whatever the title was.
        assert_eq!(sheet_name("   "), "Sheet1");
        assert_eq!(sheet_name("'''"), "Sheet1");
        assert_eq!(sheet_name(""), "Sheet1");
        // And the workbook part carries the sanitised name.
        let bytes = grid_to_xlsx("Q1/Q2 plan", &[vec!["x".to_string()]]);
        assert!(
            part_of(&bytes, "xl/workbook.xml").contains("name=\"Q1-Q2 plan\""),
            "{}",
            part_of(&bytes, "xl/workbook.xml")
        );
    }

    #[test]
    fn an_empty_document_is_still_a_document() {
        let bytes = markdown_to_docx("Empty", "");
        assert_eq!(extract_text(&bytes, OfficeKind::Docx).unwrap(), "");
        let bytes = grid_to_xlsx("Empty", &[]);
        assert_eq!(extract_text(&bytes, OfficeKind::Xlsx).unwrap(), "## Empty");
    }
}
