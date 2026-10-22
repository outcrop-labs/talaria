// WHAT A GOOGLE URL NAMES — the Rust port of `ui/src/lib/google-embed.ts`'s
// `parseGoogleFileUrl`, so the server can tell which chips on a conversation
// point at a Google file (the Work pane lists them as the session's files)
// without a round trip to Drive.
//
// THE TYPE COMES FROM THE URL'S SHAPE, never from a fetch: /document/,
// /spreadsheets/ and /presentation/ each name exactly one native editor, and a
// Drive /file/ link (or the old `open?id=`) is "some file in Drive", which the
// pane previews. The id after the `d` segment is the Drive file id, and it is
// the identity: two spellings of one file are one file.
//
// WHERE THIS IS STRICTER THAN THE TS. The TS reads only `parts[0]` and answers
// "a Drive file" for any other editor that has a `/d/<id>` (Forms, Drawings);
// here those are not files the pane can show, so they are not files at all.
// It also refuses the publish-to-web form (`/d/e/<published id>/…`), whose id
// is not a Drive file id, and it understands the `/u/0/` account segment and
// the `open?id=` form, which the paste box never needed and chips do.

/// The four kinds of Google file the Work pane knows how to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleType {
    Document,
    Spreadsheet,
    Presentation,
    /// Anything else in Drive — a PDF, an image, an upload — previewed.
    Drive,
}

impl GoogleType {
    /// The wire spelling (`SessionFile.googleType`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Spreadsheet => "spreadsheet",
            Self::Presentation => "presentation",
            Self::Drive => "drive",
        }
    }

    /// From a Drive mime type, the way `pinned_files` records one. Anything
    /// that is not one of the three native editors is a Drive file.
    pub fn from_mime(mime: Option<&str>) -> Self {
        match mime {
            Some("application/vnd.google-apps.document") => Self::Document,
            Some("application/vnd.google-apps.spreadsheet") => Self::Spreadsheet,
            Some("application/vnd.google-apps.presentation") => Self::Presentation,
            _ => Self::Drive,
        }
    }

    /// What a person calls one — the label when the file's title is unknown.
    pub fn noun(self) -> &'static str {
        match self {
            Self::Document => "Google Doc",
            Self::Spreadsheet => "Google Sheet",
            Self::Presentation => "Google Slides",
            Self::Drive => "Drive file",
        }
    }

    /// The editor path segment that names this type in a URL.
    fn from_segment(seg: &str) -> Option<Self> {
        match seg {
            "document" => Some(Self::Document),
            "spreadsheets" => Some(Self::Spreadsheet),
            "presentation" => Some(Self::Presentation),
            "file" => Some(Self::Drive),
            _ => None,
        }
    }
}

/// One Google file: its Drive id and what kind of file the URL said it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleFile {
    pub id: String,
    pub kind: GoogleType,
}

impl GoogleFile {
    /// The canonical web URL — where "Open in Google" goes, the same shapes as
    /// `openUrl` in `google-embed.ts`. The id alphabet needs no escaping.
    pub fn url(&self) -> String {
        let id = &self.id;
        match self.kind {
            GoogleType::Document => format!("https://docs.google.com/document/d/{id}/edit"),
            GoogleType::Spreadsheet => format!("https://docs.google.com/spreadsheets/d/{id}/edit"),
            GoogleType::Presentation => {
                format!("https://docs.google.com/presentation/d/{id}/edit")
            }
            GoogleType::Drive => format!("https://drive.google.com/file/d/{id}/view"),
        }
    }

    /// The key a file is deduplicated by, wherever it was seen.
    pub fn key(&self) -> String {
        format!("google:{}", self.id)
    }
}

/// A Drive file id: 1–200 characters of `[A-Za-z0-9_-]` — the TS regex.
pub fn google_id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 200
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// What a URL names, or None when it is not a Google file. `https` only, on
/// `docs.google.com` or `drive.google.com`; the id is the segment after `d`
/// (`/document/d/<id>/edit`, `/document/u/0/d/<id>`, `/file/d/<id>/view`), or
/// the `id` parameter of `open?id=<id>`.
pub fn parse_google_file_url(raw: &str) -> Option<GoogleFile> {
    let t = raw.trim();
    let scheme = t.get(..8)?;
    if !scheme.eq_ignore_ascii_case("https://") {
        return None;
    }
    let rest = &t[8..];
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // `user@host` → `host`, then the port off — what `URL.hostname` answers.
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    if host != "docs.google.com" && host != "drive.google.com" {
        return None;
    }
    let after = &rest[end..];
    let after = after.split('#').next().unwrap_or(after);
    let (path, query) = after.split_once('?').unwrap_or((after, ""));
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if let Some(d) = parts.iter().position(|p| *p == "d") {
        let id = *parts.get(d + 1)?;
        // `/d/e/<published id>/pubhtml` is publish-to-web: the id after it is
        // not a Drive file id, and nothing opens it as a file.
        if id == "e" || !google_id_ok(id) {
            return None;
        }
        let kind = parts[..d]
            .iter()
            .find_map(|seg| GoogleType::from_segment(seg))?;
        return Some(GoogleFile {
            id: id.to_string(),
            kind,
        });
    }
    // The pre-`/d/` share link: `drive.google.com/open?id=<id>`.
    if parts == ["open"] {
        let id = query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == "id").then_some(v)
        })?;
        if google_id_ok(id) {
            return Some(GoogleFile {
                id: id.to_string(),
                kind: GoogleType::Drive,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(url: &str) -> Option<(String, &'static str)> {
        parse_google_file_url(url).map(|f| (f.id, f.kind.as_str()))
    }

    fn some(id: &str, kind: &'static str) -> Option<(String, &'static str)> {
        Some((id.to_string(), kind))
    }

    // The shapes `google-embed.ts` reads: one per editor, each with its id.
    #[test]
    fn each_editor_url_names_its_type_and_id() {
        assert_eq!(
            parsed("https://docs.google.com/document/d/1AbC_d-9/edit"),
            some("1AbC_d-9", "document")
        );
        assert_eq!(
            parsed("https://docs.google.com/spreadsheets/d/1sheetID/edit#gid=0"),
            some("1sheetID", "spreadsheet")
        );
        assert_eq!(
            parsed("https://docs.google.com/presentation/d/1deck/edit?usp=sharing"),
            some("1deck", "presentation")
        );
        assert_eq!(
            parsed("https://drive.google.com/file/d/1pdf/view"),
            some("1pdf", "drive")
        );
        // No trailing segment at all.
        assert_eq!(
            parsed("https://docs.google.com/document/d/1bare"),
            some("1bare", "document")
        );
    }

    #[test]
    fn the_old_open_link_is_a_drive_file() {
        assert_eq!(
            parsed("https://drive.google.com/open?id=1old_link"),
            some("1old_link", "drive")
        );
        assert_eq!(
            parsed("https://drive.google.com/open?usp=sharing&id=1x"),
            some("1x", "drive")
        );
        assert_eq!(parsed("https://drive.google.com/open?id="), None);
        assert_eq!(parsed("https://drive.google.com/open?id=bad%20id"), None);
    }

    #[test]
    fn an_account_segment_and_a_port_do_not_change_the_file() {
        assert_eq!(
            parsed("https://docs.google.com/document/u/0/d/1acct/edit"),
            some("1acct", "document")
        );
        assert_eq!(
            parsed("https://DOCS.Google.com:443/spreadsheets/d/1port/edit"),
            some("1port", "spreadsheet")
        );
        assert_eq!(
            parsed("https://drive.google.com/a/example.com/file/d/1dom/view"),
            some("1dom", "drive")
        );
        assert_eq!(
            parsed("  https://docs.google.com/document/d/1ws/edit  "),
            some("1ws", "document")
        );
    }

    #[test]
    fn anything_that_is_not_a_google_file_is_none() {
        for url in [
            // http, other hosts, look-alikes
            "http://docs.google.com/document/d/1x/edit",
            "https://docs.google.com.evil.example/document/d/1x/edit",
            "https://evil.example/docs.google.com/document/d/1x/edit",
            "https://sheets.google.com/spreadsheets/d/1x",
            // not a file: a folder, the Drive home, no id
            "https://drive.google.com/drive/folders/1folder",
            "https://drive.google.com/drive/my-drive",
            "https://docs.google.com/document/d/",
            "https://docs.google.com/document/",
            // an editor the pane cannot show, and publish-to-web
            "https://docs.google.com/forms/d/1form/viewform",
            "https://docs.google.com/spreadsheets/d/e/2PACX-1vQ/pubhtml",
            // an id outside the alphabet or the length cap
            "https://docs.google.com/document/d/bad.id/edit",
            "/document/d/1x/edit",
            "",
            "not a url",
        ] {
            assert_eq!(parsed(url), None, "{url} parsed as a Google file");
        }
        let long = format!(
            "https://docs.google.com/document/d/{}/edit",
            "a".repeat(201)
        );
        assert_eq!(parsed(&long), None);
        let max = format!(
            "https://docs.google.com/document/d/{}/edit",
            "a".repeat(200)
        );
        assert!(parse_google_file_url(&max).is_some());
    }

    #[test]
    fn credentials_in_the_authority_do_not_hide_the_host() {
        assert_eq!(
            parsed("https://someone@docs.google.com/document/d/1u/edit"),
            some("1u", "document")
        );
        assert_eq!(
            parsed("https://docs.google.com@evil.example/document/d/1u/edit"),
            None
        );
    }

    #[test]
    fn the_canonical_url_is_the_editor_for_its_type() {
        let url = |id: &str, kind| {
            GoogleFile {
                id: id.into(),
                kind,
            }
            .url()
        };
        assert_eq!(
            url("1d", GoogleType::Document),
            "https://docs.google.com/document/d/1d/edit"
        );
        assert_eq!(
            url("1s", GoogleType::Spreadsheet),
            "https://docs.google.com/spreadsheets/d/1s/edit"
        );
        assert_eq!(
            url("1p", GoogleType::Presentation),
            "https://docs.google.com/presentation/d/1p/edit"
        );
        assert_eq!(
            url("1f", GoogleType::Drive),
            "https://drive.google.com/file/d/1f/view"
        );
        // And a canonical URL parses back to the file it came from.
        for kind in [
            GoogleType::Document,
            GoogleType::Spreadsheet,
            GoogleType::Presentation,
            GoogleType::Drive,
        ] {
            let f = GoogleFile {
                id: "1round".into(),
                kind,
            };
            assert_eq!(parse_google_file_url(&f.url()), Some(f));
        }
    }

    #[test]
    fn a_mime_type_names_the_same_four_types() {
        assert_eq!(
            GoogleType::from_mime(Some("application/vnd.google-apps.document")),
            GoogleType::Document
        );
        assert_eq!(
            GoogleType::from_mime(Some("application/vnd.google-apps.spreadsheet")),
            GoogleType::Spreadsheet
        );
        assert_eq!(
            GoogleType::from_mime(Some("application/vnd.google-apps.presentation")),
            GoogleType::Presentation
        );
        assert_eq!(
            GoogleType::from_mime(Some("application/pdf")),
            GoogleType::Drive
        );
        assert_eq!(GoogleType::from_mime(None), GoogleType::Drive);
    }
}
