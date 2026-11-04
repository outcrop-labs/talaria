// A SESSION'S FILES — what the Work pane lists as tabs, derived from what the
// conversation already records rather than from a list of its own.
//
// Every Talaria document and every Google file the agent created, read or
// edited in a turn left a chip on that turn's message (`chips_from_tool`, and
// the edit route's document chip), so the chips ARE the session's file
// history: the order they first appeared in is the order the files joined the
// session, and the newest message that names a file is when it was last
// touched. To those the teammate's own pins are added (`pinned_files` — the
// pane's memory of what they opened), and, for a work session from before the
// files pane, the living document it was created with.
//
// PURE, IN TWO STEPS, because the middle needs the database and this crate
// should not decide who may read an artifact: `candidates` reads the rows the
// route fetched into a deduplicated list; the route looks up the artifacts it
// names and keeps the ones the viewer can read (the same check the artifact
// GET makes); `session_files` turns what is left into the wire shape. A
// deleted artifact simply comes back from no lookup and is dropped there.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::Value;

use crate::classify_href;
use crate::google_file::{GoogleFile, GoogleType, google_id_ok, parse_google_file_url};
use talaria_params::uuid_canonical;

/// How many files one session answers with.
pub const MAX_SESSION_FILES: usize = 100;

/// One message's chips, in conversation order, with the message's own time
/// (ISO 8601) — what `lastTouchedAt` reports.
#[derive(Debug, Clone)]
pub struct TrailMessage {
    pub created_at: String,
    pub chips: Value,
}

/// Which file a candidate is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRef {
    /// A Talaria artifact, by its (lowercased) uuid.
    Artifact(String),
    Google(GoogleFile),
}

impl FileRef {
    fn key(&self) -> String {
        match self {
            Self::Artifact(id) => format!("artifact:{id}"),
            Self::Google(f) => f.key(),
        }
    }
}

/// Why a file is in the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSource {
    /// A chip on one of the session's messages names it.
    Touched,
    /// The teammate opened it in the pane (`pinned_files`).
    Pinned,
    /// A legacy work session's living document.
    Living,
}

impl FileSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Touched => "touched",
            Self::Pinned => "pinned",
            Self::Living => "living",
        }
    }
}

/// One file before the read check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub file: FileRef,
    /// A Google file's title, when a chip or pin knew it. An artifact's title
    /// comes from its row, never from here.
    pub title: Option<String>,
    pub source: FileSource,
    /// The created_at of the NEWEST message whose chips name it; None for a
    /// file that is only pinned or only the living document.
    pub last_touched_at: Option<String>,
    /// The position of that newest message — what the cap ranks by.
    touched_rank: Option<usize>,
}

/// What the route found for one readable artifact.
#[derive(Debug, Clone)]
pub struct ArtifactFacts {
    /// `artifacts.kind` — doc | sheet | page | file | …
    pub kind: String,
    pub title: String,
}

/// One file on the wire — `SessionFile` in the client. Key order is the
/// interface's; the optional keys are absent, not null, when they do not
/// apply, while `title` and `lastTouchedAt` are always present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFile {
    pub key: String,
    pub kind: &'static str,
    pub id: String,
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub source: &'static str,
    pub last_touched_at: Option<String>,
}

fn str_of<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The file one URL names: a Talaria document by the platform-href
/// classifier's own reading (`/artifacts?a=<uuid>` in any of its spellings —
/// relative, absolute, with other parameters), else a Google file.
fn file_named_by(url: &str) -> Option<FileRef> {
    if let Some(link) = classify_href(url) {
        if link.entity != "document" {
            return None;
        }
        // Keyed by the id's canonical text, the one the artifacts read
        // answers with: every spelling of one document is one file here.
        return uuid_canonical(&link.id).map(FileRef::Artifact);
    }
    parse_google_file_url(url).map(FileRef::Google)
}

/// A Google chip's title. `title` when the producer set one; otherwise a
/// `source` chip's label — unless that label is only the fallback a chip
/// carries when nobody knew the title (the URL's host, the URL itself, or
/// what the file is: "Google Doc").
fn google_title(chip: &Value, url: &str) -> Option<String> {
    if let Some(t) = str_of(chip, "title") {
        return Some(t.to_string());
    }
    let label = str_of(chip, "label")?;
    let host = url
        .split("://")
        .nth(1)
        .and_then(|r| r.split(['/', '?', '#']).next())
        .unwrap_or("");
    let generic = label == url
        || label.eq_ignore_ascii_case(host)
        || [
            GoogleType::Document,
            GoogleType::Spreadsheet,
            GoogleType::Presentation,
            GoogleType::Drive,
        ]
        .iter()
        .any(|t| label == t.noun());
    (!generic).then(|| label.to_string())
}

/// Every file a chip names — its `href`, and a `url` if it carries one.
fn files_on(chip: &Value) -> Vec<(FileRef, Option<String>)> {
    let mut out = Vec::new();
    for field in ["href", "url"] {
        let Some(url) = str_of(chip, field) else {
            continue;
        };
        if let Some(file) = file_named_by(url) {
            let title = match &file {
                FileRef::Google(_) => google_title(chip, url),
                FileRef::Artifact(_) => None,
            };
            if !out.iter().any(|(f, _)| f == &file) {
                out.push((file, title));
            }
        }
    }
    out
}

/// Merge a later sighting of a Google file into the one already listed: a
/// specific type outranks "some Drive file" (an `open?id=` link and the
/// editor's own URL are the same file), and a title fills one that was
/// missing.
fn absorb(existing: &mut Candidate, file: &FileRef, title: Option<String>) {
    if let (FileRef::Google(have), FileRef::Google(seen)) = (&mut existing.file, file)
        && have.kind == GoogleType::Drive
        && seen.kind != GoogleType::Drive
    {
        have.kind = seen.kind;
    }
    if title.is_some() && matches!(existing.file, FileRef::Google(_)) {
        existing.title = title;
    }
}

/// The session's files before the read check: every file the messages' chips
/// name (oldest first appearance first), then the pins not already listed,
/// then the living document if it is not listed either. Deduplicated by key.
///
/// A touched Google file keeps the NEWEST title a chip gave it — a file
/// renamed mid-session reads as it is called now.
pub fn candidates(
    messages: &[TrailMessage],
    pinned: &Value,
    living: Option<&str>,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();

    for (rank, m) in messages.iter().enumerate() {
        let Some(chips) = m.chips.as_array() else {
            continue;
        };
        for chip in chips {
            for (file, title) in files_on(chip) {
                let key = file.key();
                match at.get(&key) {
                    Some(&i) => {
                        let c = &mut out[i];
                        absorb(c, &file, title);
                        c.last_touched_at = Some(m.created_at.clone());
                        c.touched_rank = Some(rank);
                    }
                    None => {
                        at.insert(key, out.len());
                        out.push(Candidate {
                            file,
                            title,
                            source: FileSource::Touched,
                            last_touched_at: Some(m.created_at.clone()),
                            touched_rank: Some(rank),
                        });
                    }
                }
            }
        }
    }

    for pin in pinned.as_array().into_iter().flatten() {
        let Some(id) = str_of(pin, "id") else {
            continue;
        };
        let title = str_of(pin, "title").map(str::to_string);
        let file = match (str_of(pin, "kind"), uuid_canonical(id)) {
            (Some("artifact"), Some(id)) => FileRef::Artifact(id),
            (Some("google"), _) if google_id_ok(id) => FileRef::Google(GoogleFile {
                id: id.to_string(),
                kind: GoogleType::from_mime(str_of(pin, "mime")),
            }),
            _ => continue,
        };
        let key = file.key();
        match at.get(&key) {
            // Already listed as touched: it keeps that place and source, and
            // a pin only fills in what the chips did not know.
            Some(&i) => {
                let c = &mut out[i];
                let fill = if c.title.is_none() { title } else { None };
                absorb(c, &file, fill);
            }
            None => {
                at.insert(key, out.len());
                out.push(Candidate {
                    title: match file {
                        FileRef::Google(_) => title,
                        FileRef::Artifact(_) => None,
                    },
                    file,
                    source: FileSource::Pinned,
                    last_touched_at: None,
                    touched_rank: None,
                });
            }
        }
    }

    if let Some(id) = living.and_then(uuid_canonical) {
        let file = FileRef::Artifact(id);
        if !at.contains_key(&file.key()) {
            out.push(Candidate {
                file,
                title: None,
                source: FileSource::Living,
                last_touched_at: None,
                touched_rank: None,
            });
        }
    }
    out
}

/// The artifact ids the candidates name — what the route looks up.
pub fn artifact_ids(candidates: &[Candidate]) -> Vec<String> {
    candidates
        .iter()
        .filter_map(|c| match &c.file {
            FileRef::Artifact(id) => Some(id.clone()),
            FileRef::Google(_) => None,
        })
        .collect()
}

/// The wire list. An artifact missing from `readable` — deleted, or not one
/// this viewer may read — is dropped. Over `cap`, the files kept are the
/// teammate's pins and the most recently touched (the living document last),
/// and they keep their first-appearance order: the newest file the agent
/// made is the one the pane must not lose.
pub fn session_files(
    candidates: Vec<Candidate>,
    readable: &HashMap<String, ArtifactFacts>,
    cap: usize,
) -> Vec<SessionFile> {
    let kept: Vec<Candidate> = candidates
        .into_iter()
        .filter(|c| match &c.file {
            FileRef::Artifact(id) => readable.contains_key(id),
            FileRef::Google(_) => true,
        })
        .collect();
    let kept = if kept.len() > cap {
        let mut order: Vec<usize> = (0..kept.len()).collect();
        // Pins first, then touched newest-first, then the living document.
        order.sort_by_key(|&i| {
            let c = &kept[i];
            match c.source {
                FileSource::Pinned => (0, 0),
                FileSource::Touched => (1, usize::MAX - c.touched_rank.unwrap_or(0)),
                FileSource::Living => (2, 0),
            }
        });
        let mut survivors: Vec<usize> = order.into_iter().take(cap).collect();
        survivors.sort_unstable();
        let mut slots: Vec<Option<Candidate>> = kept.into_iter().map(Some).collect();
        survivors
            .into_iter()
            .filter_map(|i| slots[i].take())
            .collect()
    } else {
        kept
    };

    kept.into_iter()
        .map(|c| {
            let key = c.file.key();
            let source = c.source.as_str();
            match c.file {
                FileRef::Artifact(id) => {
                    let facts = &readable[&id];
                    let title = Some(facts.title.trim().to_string()).filter(|t| !t.is_empty());
                    SessionFile {
                        key,
                        kind: "artifact",
                        id,
                        title,
                        artifact_kind: Some(facts.kind.clone()),
                        google_type: None,
                        url: None,
                        source,
                        last_touched_at: c.last_touched_at,
                    }
                }
                FileRef::Google(file) => SessionFile {
                    key,
                    kind: "google",
                    url: Some(file.url()),
                    google_type: Some(file.kind.as_str()),
                    id: file.id,
                    title: c.title,
                    artifact_kind: None,
                    source,
                    last_touched_at: c.last_touched_at,
                },
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const A1: &str = "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2";
    const A2: &str = "0b5c6d1e-1111-4222-8333-444455556666";
    const A3: &str = "7e7e7e7e-aaaa-4bbb-8ccc-dddddddddddd";

    fn msg(at: &str, chips: Value) -> TrailMessage {
        TrailMessage {
            created_at: at.to_string(),
            chips,
        }
    }

    fn doc_chip(id: &str, title: &str) -> Value {
        json!({ "id": format!("link:/artifacts?a={id}"), "kind": "link",
                "href": format!("/artifacts?a={id}"), "entity": "document", "title": title })
    }

    fn readable(ids: &[(&str, &str, &str)]) -> HashMap<String, ArtifactFacts> {
        ids.iter()
            .map(|(id, kind, title)| {
                (
                    id.to_string(),
                    ArtifactFacts {
                        kind: kind.to_string(),
                        title: title.to_string(),
                    },
                )
            })
            .collect()
    }

    fn keys(files: &[SessionFile]) -> Vec<&str> {
        files.iter().map(|f| f.key.as_str()).collect()
    }

    #[test]
    fn files_are_listed_in_first_appearance_order_and_touched_at_the_newest_mention() {
        let messages = vec![
            msg("2026-10-21T10:00:00.000Z", json!([doc_chip(A1, "Brief")])),
            msg("2026-10-21T10:01:00.000Z", json!([])),
            msg(
                "2026-10-21T10:02:00.000Z",
                json!([
                    { "id": "source:aa", "kind": "source", "label": "Budget",
                      "href": "https://docs.google.com/spreadsheets/d/1budget/edit#gid=0" },
                    doc_chip(A2, "Notes"),
                ]),
            ),
            msg("2026-10-21T10:03:00.000Z", json!([doc_chip(A1, "Brief")])),
        ];
        let c = candidates(&messages, &json!([]), None);
        let files = session_files(
            c,
            &readable(&[(A1, "doc", "Launch brief"), (A2, "sheet", "Numbers")]),
            MAX_SESSION_FILES,
        );
        assert_eq!(
            keys(&files),
            vec![
                format!("artifact:{A1}").as_str(),
                "google:1budget",
                format!("artifact:{A2}").as_str()
            ]
        );
        // Re-mentioned in the last message: its touch moved, its place did not.
        assert_eq!(
            files[0].last_touched_at.as_deref(),
            Some("2026-10-21T10:03:00.000Z")
        );
        // The ROW's title, not the chip's.
        assert_eq!(files[0].title.as_deref(), Some("Launch brief"));
        assert_eq!(files[0].artifact_kind.as_deref(), Some("doc"));
        assert_eq!(files[1].google_type, Some("spreadsheet"));
        assert_eq!(files[1].title.as_deref(), Some("Budget"));
        assert_eq!(
            files[1].url.as_deref(),
            Some("https://docs.google.com/spreadsheets/d/1budget/edit")
        );
        assert_eq!(
            files[1].last_touched_at.as_deref(),
            Some("2026-10-21T10:02:00.000Z")
        );
        assert!(files.iter().all(|f| f.source == "touched"));
    }

    #[test]
    fn every_spelling_of_a_document_link_is_one_file() {
        let messages = vec![msg(
            "t1",
            json!([
                { "kind": "link", "href": format!("https://talaria.example.com/artifacts?x=1&a={A1}") },
                { "kind": "link", "href": format!("/artifacts?a={}", A1.to_uppercase()) },
                { "kind": "link", "href": format!("/artifacts/?a={A1}") },
                // The hyphen-less spelling the cast takes is the same row.
                { "kind": "link", "href": format!("/artifacts?a={}", A1.replace('-', "")) },
            ]),
        )];
        // A pin and the living document in other spellings join it too.
        let pinned = json!([{ "kind": "artifact", "id": format!("{{{}}}", A1.to_uppercase()) }]);
        let living = A1.replace('-', "");
        let c = candidates(&messages, &pinned, Some(&living));
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].file, FileRef::Artifact(A1.to_string()));
    }

    #[test]
    fn only_documents_and_google_files_count() {
        let messages = vec![msg(
            "t1",
            json!([
                { "kind": "link", "href": "/boards/b-1/t-2", "entity": "ticket" },
                { "kind": "link", "href": "/knowledge?doc=k-1", "entity": "kb" },
                { "kind": "link", "href": "/artifacts?a=doc-1", "entity": "document" },
                { "kind": "source", "href": "https://example.com/page", "label": "A page" },
                { "kind": "approval", "actionId": "p-1" },
                { "kind": "source", "href": "https://docs.google.com/forms/d/1form/viewform" },
                "not a chip",
            ]),
        )];
        assert!(candidates(&messages, &json!([]), None).is_empty());
    }

    #[test]
    fn a_google_file_is_one_file_whatever_the_chip_and_the_url_spelling() {
        let messages = vec![
            msg(
                "t1",
                json!([{ "kind": "link", "url": "https://drive.google.com/open?id=1doc" }]),
            ),
            msg(
                "t2",
                json!([{ "kind": "source", "label": "docs.google.com",
                         "href": "https://docs.google.com/document/u/0/d/1doc/edit" }]),
            ),
            msg(
                "t3",
                json!([{ "kind": "source", "label": "Google Doc", "title": "Spec v2",
                         "href": "https://docs.google.com/document/d/1doc/edit" }]),
            ),
        ];
        let files = session_files(
            candidates(&messages, &json!([]), None),
            &HashMap::new(),
            MAX_SESSION_FILES,
        );
        assert_eq!(files.len(), 1, "{files:?}");
        let f = &files[0];
        assert_eq!(f.key, "google:1doc");
        // An open?id= link said "some Drive file"; the editor URL said what.
        assert_eq!(f.google_type, Some("document"));
        assert_eq!(
            f.url.as_deref(),
            Some("https://docs.google.com/document/d/1doc/edit")
        );
        // The newest title wins; a host label is no title at all.
        assert_eq!(f.title.as_deref(), Some("Spec v2"));
        assert_eq!(f.last_touched_at.as_deref(), Some("t3"));
    }

    #[test]
    fn a_fallback_label_is_not_a_title() {
        let host_only = vec![msg(
            "t1",
            json!([{ "kind": "source", "label": "docs.google.com",
                     "href": "https://docs.google.com/document/d/1x/edit" }]),
        )];
        let noun_only = vec![msg(
            "t1",
            json!([{ "kind": "source", "label": "Google Sheet",
                     "href": "https://docs.google.com/spreadsheets/d/1y/edit" }]),
        )];
        for m in [host_only, noun_only] {
            let c = candidates(&m, &json!([]), None);
            assert_eq!(c[0].title, None, "{c:?}");
        }
    }

    #[test]
    fn pins_follow_the_touched_files_and_never_duplicate_one() {
        let messages = vec![msg("t1", json!([doc_chip(A1, "Brief")]))];
        let pinned = json!([
            { "kind": "artifact", "id": A1 },
            { "kind": "google", "id": "1pinned", "title": "Roadmap",
              "mime": "application/vnd.google-apps.presentation" },
            { "kind": "artifact", "id": A2 },
            { "kind": "artifact", "id": "not-a-uuid" },
            { "kind": "google", "id": "bad id" },
            { "kind": "folder", "id": "x" },
            "junk",
        ]);
        let files = session_files(
            candidates(&messages, &pinned, None),
            &readable(&[(A1, "doc", "Brief"), (A2, "page", "")]),
            MAX_SESSION_FILES,
        );
        assert_eq!(
            keys(&files),
            vec![
                format!("artifact:{A1}").as_str(),
                "google:1pinned",
                format!("artifact:{A2}").as_str()
            ]
        );
        assert_eq!(files[0].source, "touched");
        assert_eq!(files[1].source, "pinned");
        assert_eq!(files[1].title.as_deref(), Some("Roadmap"));
        assert_eq!(files[1].google_type, Some("presentation"));
        assert_eq!(files[1].last_touched_at, None);
        // An empty row title is no title — the client names it by its type.
        assert_eq!(files[2].title, None);
        assert_eq!(files[2].artifact_kind.as_deref(), Some("page"));
    }

    #[test]
    fn the_living_document_comes_last_and_only_once() {
        let pinned = json!([{ "kind": "google", "id": "1g" }]);
        let files = session_files(
            candidates(&[], &pinned, Some(A3)),
            &readable(&[(A3, "doc", "Plan — Q3")]),
            MAX_SESSION_FILES,
        );
        assert_eq!(
            keys(&files),
            vec!["google:1g", format!("artifact:{A3}").as_str()]
        );
        assert_eq!(files[1].source, "living");
        assert_eq!(files[1].last_touched_at, None);

        // Already named by a chip: it stays where the chip put it.
        let messages = vec![msg("t1", json!([doc_chip(A3, "Plan")]))];
        let files = session_files(
            candidates(&messages, &json!([]), Some(A3)),
            &readable(&[(A3, "doc", "Plan — Q3")]),
            MAX_SESSION_FILES,
        );
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].source, "touched");
    }

    #[test]
    fn an_unreadable_or_deleted_artifact_is_dropped() {
        let messages = vec![msg(
            "t1",
            json!([doc_chip(A1, "Mine"), doc_chip(A2, "Theirs")]),
        )];
        let files = session_files(
            candidates(&messages, &json!([{ "kind": "artifact", "id": A3 }]), None),
            &readable(&[(A1, "doc", "Mine")]),
            MAX_SESSION_FILES,
        );
        assert_eq!(keys(&files), vec![format!("artifact:{A1}").as_str()]);
    }

    #[test]
    fn over_the_cap_the_newest_touches_and_the_pins_survive_in_their_order() {
        // Five touched Google files, one per message, then a pin.
        let messages: Vec<TrailMessage> = (0..5)
            .map(|i| {
                msg(
                    &format!("t{i}"),
                    json!([{ "kind": "source",
                             "href": format!("https://docs.google.com/document/d/1f{i}/edit") }]),
                )
            })
            .collect();
        // The oldest file is touched again last, so it is among the newest.
        let mut messages = messages;
        messages.push(msg(
            "t5",
            json!([{ "kind": "source", "href": "https://docs.google.com/document/d/1f0/edit" }]),
        ));
        let pinned = json!([{ "kind": "google", "id": "1pin" }]);
        let files = session_files(candidates(&messages, &pinned, None), &HashMap::new(), 3);
        assert_eq!(
            keys(&files),
            vec!["google:1f0", "google:1f4", "google:1pin"]
        );
    }

    #[test]
    fn the_wire_shape_has_the_contracts_keys_in_order() {
        let messages = vec![msg(
            "2026-10-21T10:00:00.000Z",
            json!([
                doc_chip(A1, "Brief"),
                { "kind": "source", "href": "https://drive.google.com/file/d/1pdf/view" },
            ]),
        )];
        let files = session_files(
            candidates(&messages, &json!([]), None),
            &readable(&[(A1, "doc", "Brief")]),
            MAX_SESSION_FILES,
        );
        assert_eq!(
            serde_json::to_string(&files[0]).unwrap(),
            format!(
                r#"{{"key":"artifact:{A1}","kind":"artifact","id":"{A1}","title":"Brief","artifactKind":"doc","source":"touched","lastTouchedAt":"2026-10-21T10:00:00.000Z"}}"#
            )
        );
        assert_eq!(
            serde_json::to_string(&files[1]).unwrap(),
            r#"{"key":"google:1pdf","kind":"google","id":"1pdf","title":null,"googleType":"drive","url":"https://drive.google.com/file/d/1pdf/view","source":"touched","lastTouchedAt":"2026-10-21T10:00:00.000Z"}"#
        );
    }

    #[test]
    fn artifact_ids_are_what_the_route_looks_up() {
        let messages = vec![msg(
            "t1",
            json!([
                doc_chip(A1, "x"),
                { "kind": "source", "href": "https://docs.google.com/document/d/1g/edit" },
            ]),
        )];
        let c = candidates(
            &messages,
            &json!([{ "kind": "artifact", "id": A2 }]),
            Some(A3),
        );
        assert_eq!(artifact_ids(&c), vec![A1, A2, A3]);
    }
}
