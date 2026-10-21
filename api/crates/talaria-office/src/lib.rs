//! OFFICE FORMATS, BOTH DIRECTIONS — .docx/.xlsx/.pptx in as text, markdown
//! and grids out as files people can open.
//!
//! WHY THIS EXISTS. Two surfaces needed the same thing from opposite ends. An
//! upload of a Word document used to reach the agent as a mime type and a byte
//! count: the attachment pipeline could only read what `TEXT_MIME` admitted,
//! so the one format most of the business world writes in arrived as nothing
//! at all. And an artifact the agents had written could be read in the app but
//! not sent anywhere — no file to attach to a mail, hand to a lawyer, or open
//! in Word and edit. Both ends are the same small problem: OOXML is a zip of
//! XML parts, and nobody had written the hundred lines that walk it.
//!
//! WHY ITS OWN CRATE, WITH NO `talaria-*` DEPENDENCIES. Nothing here knows
//! about a request, a database, an artifact or a user. It is bytes in, text
//! out and text in, bytes out — which is what lets the uploads path and the
//! export route both depend on it without depending on each other, and what
//! makes the whole surface testable from hand-built fixtures with no stack
//! running. The crate stays a leaf; it must not grow a `talaria-` dep.
//!
//! WHAT IT DELIBERATELY IS NOT. Not a document model, and not a converter.
//! Reading keeps the structure a reader can act on (paragraphs, table rows,
//! sheet and slide order) and drops everything a renderer would need. Writing
//! emits the smallest package Word and Excel open without complaint. A
//! round-trip through both is lossy on purpose, and the tests pin exactly what
//! survives it: every heading, paragraph and cell.
//!
//! WHERE THE LIMITS LIVE. Reading refuses any part over 20 MiB and never
//! panics on malformed XML; clipping the text to whatever a caller can afford
//! is the caller's business, not this crate's.

mod read;
mod write;

pub use read::extract_text;
pub use write::{grid_to_xlsx, markdown_to_docx};

/// The three OOXML documents this crate handles.
///
/// Deliberately not the pre-2007 binary formats: `.doc`/`.xls`/`.ppt` are a
/// different problem (OLE compound files), and a file named `.doc` that is
/// really OOXML is rare enough to let fail honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfficeKind {
    Docx,
    Xlsx,
    Pptx,
}

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const XLSX_MIME: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
const PPTX_MIME: &str = "application/vnd.openxmlformats-officedocument.presentationml.presentation";

impl OfficeKind {
    /// The mime type a response serving this kind should carry.
    pub const fn mime(self) -> &'static str {
        match self {
            Self::Docx => DOCX_MIME,
            Self::Xlsx => XLSX_MIME,
            Self::Pptx => PPTX_MIME,
        }
    }

    /// The filename extension, without the dot.
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Docx => "docx",
            Self::Xlsx => "xlsx",
            Self::Pptx => "pptx",
        }
    }
}

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficeError {
    /// The container opened but a part inside it is not the XML it claims to
    /// be. Carries the parser's own complaint, because "malformed" alone tells
    /// an operator staring at a log nothing.
    Malformed(String),
    /// A part exceeds the size this crate will inflate.
    TooLarge,
    /// The bytes are not an Office document: not a zip, or a zip with none of
    /// the parts the kind requires.
    NotOffice,
}

impl std::fmt::Display for OfficeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Malformed(detail) => write!(f, "malformed Office document: {detail}"),
            Self::TooLarge => write!(f, "this Office document has a part too large to read"),
            Self::NotOffice => write!(f, "these bytes are not an Office document"),
        }
    }
}

impl std::error::Error for OfficeError {}

/// Which Office document, if any, these bytes are supposed to be.
///
/// By mime when the mime says something, by extension when it does not.
/// Uploads routinely arrive as `application/octet-stream` — a browser that
/// cannot guess, a curl without `-H`, an integration streaming bytes it never
/// sniffed — and the filename is then the only evidence there is. The reverse
/// case is NOT symmetric: a mime that specifically says `text/plain` wins over
/// a `.docx` on the end of the name, because a specific mime is a claim and an
/// extension is a guess.
pub fn office_kind(mime: &str, filename: &str) -> Option<OfficeKind> {
    // A mime arrives with parameters (`; charset=utf-8`) and in any case.
    let essence = mime
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match essence.as_str() {
        DOCX_MIME => Some(OfficeKind::Docx),
        XLSX_MIME => Some(OfficeKind::Xlsx),
        PPTX_MIME => Some(OfficeKind::Pptx),
        // The generic claims, in the spellings that actually turn up. A zip
        // mime is generic here for the same reason: every one of these
        // documents IS a zip, so a sniffer that got that far got it right.
        "" | "application/octet-stream" | "application/zip" | "application/x-zip-compressed" => {
            by_extension(filename)
        }
        _ => None,
    }
}

fn by_extension(filename: &str) -> Option<OfficeKind> {
    let (_, extension) = filename.trim().rsplit_once('.')?;
    match extension.to_ascii_lowercase().as_str() {
        "docx" => Some(OfficeKind::Docx),
        "xlsx" => Some(OfficeKind::Xlsx),
        "pptx" => Some(OfficeKind::Pptx),
        _ => None,
    }
}

/// Zip building and part reading for the tests, which build every fixture by
/// hand rather than checking binary documents into the repo — a fixture you
/// can read in the diff is worth more than one you have to open in Word.
#[cfg(test)]
mod fixtures {
    use std::io::{Cursor, Read, Write};

    use zip::write::SimpleFileOptions;
    use zip::{ZipArchive, ZipWriter};

    pub fn zip_of(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    pub fn part_of(bytes: &[u8], name: &str) -> String {
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut entry = zip.by_name(name).unwrap();
        let mut out = String::new();
        entry.read_to_string(&mut out).unwrap();
        out
    }

    pub fn names_in(bytes: &[u8]) -> Vec<String> {
        let zip = ZipArchive::new(Cursor::new(bytes)).unwrap();
        zip.file_names()
            .flatten()
            .map(|name| name.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_by_mime() {
        assert_eq!(office_kind(DOCX_MIME, "x.bin"), Some(OfficeKind::Docx));
        assert_eq!(office_kind(XLSX_MIME, "x.bin"), Some(OfficeKind::Xlsx));
        assert_eq!(office_kind(PPTX_MIME, "x.bin"), Some(OfficeKind::Pptx));
        // Parameters and case are noise on the way to the essence.
        assert_eq!(
            office_kind(&format!("{}; charset=binary", DOCX_MIME.to_uppercase()), ""),
            Some(OfficeKind::Docx)
        );
    }

    #[test]
    fn kind_by_extension_when_the_mime_is_generic() {
        for mime in [
            "",
            "application/octet-stream",
            "application/zip",
            "application/x-zip-compressed",
        ] {
            assert_eq!(
                office_kind(mime, "notes.docx"),
                Some(OfficeKind::Docx),
                "{mime}"
            );
            assert_eq!(
                office_kind(mime, "Q3 numbers.XLSX"),
                Some(OfficeKind::Xlsx),
                "{mime}"
            );
            assert_eq!(
                office_kind(mime, "deck.pptx"),
                Some(OfficeKind::Pptx),
                "{mime}"
            );
        }
    }

    #[test]
    fn not_office() {
        // A generic mime with an extension that says nothing.
        assert_eq!(office_kind("application/octet-stream", "notes.txt"), None);
        assert_eq!(office_kind("", "README"), None);
        // A specific mime outranks the name.
        assert_eq!(office_kind("text/plain", "notes.docx"), None);
        // The binary predecessors are a different format, not this one.
        assert_eq!(office_kind("application/octet-stream", "old.doc"), None);
    }

    #[test]
    fn kind_carries_its_mime_and_extension() {
        assert_eq!(OfficeKind::Xlsx.mime(), XLSX_MIME);
        assert_eq!(OfficeKind::Pptx.extension(), "pptx");
    }

    #[test]
    fn errors_say_what_went_wrong() {
        assert_eq!(
            OfficeError::Malformed("mismatched end tag".into()).to_string(),
            "malformed Office document: mismatched end tag"
        );
        assert!(OfficeError::TooLarge.to_string().contains("too large"));
        assert!(OfficeError::NotOffice.to_string().contains("not an Office"));
    }

    /// The round trip the two halves of this crate owe each other: a document
    /// written here reads back with every heading and every paragraph intact.
    /// Formatting is not part of the deal — the bold markers are gone from the
    /// text, which is why the assertion is on the words.
    #[test]
    fn docx_round_trip_keeps_every_line() {
        let markdown = "# Quarterly review\n\nRevenue grew by a third.\n\n\
                        ## What changed\n\nThe pipeline doubled in March.\n\n\
                        ### Detail\n\nMargins held at 42%.\n";
        let bytes = markdown_to_docx("Quarterly review", markdown);
        let text = extract_text(&bytes, OfficeKind::Docx).unwrap();
        for line in [
            "Quarterly review",
            "Revenue grew by a third.",
            "What changed",
            "The pipeline doubled in March.",
            "Detail",
            "Margins held at 42%.",
        ] {
            assert!(text.contains(line), "{line:?} missing from {text:?}");
        }
    }

    #[test]
    fn xlsx_round_trip_keeps_every_cell_in_row_order() {
        let grid = vec![
            vec![
                "Region".to_string(),
                "Revenue".to_string(),
                "Note".to_string(),
            ],
            vec![
                "West".to_string(),
                "1200.5".to_string(),
                "steady".to_string(),
            ],
            vec![
                "East".to_string(),
                "980".to_string(),
                "new logo & co".to_string(),
            ],
        ];
        let bytes = grid_to_xlsx("Revenue by region", &grid);
        let text = extract_text(&bytes, OfficeKind::Xlsx).unwrap();
        assert!(text.starts_with("## Revenue by region"), "{text:?}");
        let mut cursor = 0;
        for row in [
            "Region\tRevenue\tNote",
            "West\t1200.5\tsteady",
            "East\t980\tnew logo & co",
        ] {
            let at = text[cursor..]
                .find(row)
                .unwrap_or_else(|| panic!("{row:?} missing from {text:?}"));
            cursor += at + row.len();
        }
    }
}
