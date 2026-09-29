// Google Slides, read only.
//
// WHY READ ONLY. A deck's text is what an agent can actually use — summarise
// it, check it against a plan, draft the next slide's copy for a person to
// paste. Writing one back is a different order of problem: the Slides API
// edits through batched requests against shape and placeholder ids, and a
// "replace this deck's text" tool that did not understand layouts would
// produce decks nobody wants. Reading is the whole of the useful half, so it
// is the whole of this crate; `import_drive_file` still exists for anyone who
// wants the deck as a PDF artifact.
//
// WHY NOT THE PDF EXPORT. Drive can export a deck to PDF (talaria-google-drive
// does exactly that on import), but a PDF is bytes an agent cannot read
// without OCR. presentations.get gives the text already separated per slide,
// with speaker notes, which is what the agent actually wanted.
//
// SCOPE. presentations.get accepts `https://www.googleapis.com/auth/drive`,
// which every Talaria connection already carries — no reconnect.

use talaria_body::percent_encode;
use talaria_gateway::provider::http;
use talaria_google_errors::GoogleError;

const SLIDES_ENDPOINT: &str = "https://slides.googleapis.com/v1/presentations";

/// One slide's readable content, in deck order.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Slide {
    /// 1-based, so "slide 3" in the answer means slide 3 in the deck.
    pub number: usize,
    pub id: String,
    /// Every text run on the slide, joined — titles, bullets, text boxes, in
    /// the order Slides reports the shapes.
    pub text: String,
    /// The speaker notes, when the slide has any.
    pub notes: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Deck {
    pub id: String,
    pub title: String,
    pub url: String,
    pub slides: Vec<Slide>,
}

/// Walk a page element tree and collect its text runs. Slides nests: a group
/// holds elements, a table holds cells, and each cell holds its own text — so
/// this recurses rather than reading one level and missing the rest.
fn collect_text(node: &serde_json::Value, out: &mut String) {
    if let Some(content) = node
        .get("textRun")
        .and_then(|r| r.get("content"))
        .and_then(|c| c.as_str())
    {
        out.push_str(content);
    }
    match node {
        serde_json::Value::Object(map) => {
            for (_, v) in map {
                collect_text(v, out);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                collect_text(v, out);
            }
        }
        _ => {}
    }
}

/// Collapse the runs into something a person would recognise: Slides emits a
/// vertical-tab for a soft line break and splits a sentence across runs, so
/// the raw join is full of stray whitespace.
fn tidy(raw: &str) -> String {
    raw.replace('\u{000b}', "\n")
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn notes_text(slide: &serde_json::Value) -> String {
    let mut raw = String::new();
    if let Some(notes) = slide
        .get("slideProperties")
        .and_then(|p| p.get("notesPage"))
    {
        collect_text(notes, &mut raw);
    }
    tidy(&raw)
}

/// Read a deck's text, slide by slide.
#[tracing::instrument(skip(token))]
pub async fn read_deck_with_token(token: &str, id: &str) -> Result<Deck, GoogleError> {
    let res = http()
        .get(format!("{SLIDES_ENDPOINT}/{}", percent_encode(id)))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("slides get request: {e}")))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "slides read failed: {status} {body}"
        )));
    }
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("slides get body: {e}")))?;
    let slides = body
        .get("slides")
        .and_then(|s| s.as_array())
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(i, slide)| {
                    // The notes live under the slide too, so they are gathered
                    // FIRST and the page elements walked separately — walking
                    // the whole slide would fold the notes into the body text.
                    let notes = notes_text(slide);
                    let mut raw = String::new();
                    if let Some(elements) = slide.get("pageElements") {
                        collect_text(elements, &mut raw);
                    }
                    Slide {
                        number: i + 1,
                        id: slide
                            .get("objectId")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        text: tidy(&raw),
                        notes,
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Deck {
        id: id.to_string(),
        title: body
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        url: format!("https://docs.google.com/presentation/d/{id}/edit"),
        slides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_gathered_from_nested_shapes_and_tables() {
        let page = serde_json::json!([
            { "shape": { "text": { "textElements": [ { "textRun": { "content": "Title\n" } } ] } } },
            { "elementGroup": { "children": [
                { "shape": { "text": { "textElements": [ { "textRun": { "content": "Nested bullet\n" } } ] } } }
            ] } },
            { "table": { "tableRows": [ { "tableCells": [
                { "text": { "textElements": [ { "textRun": { "content": "Cell\n" } } ] } }
            ] } ] } }
        ]);
        let mut raw = String::new();
        collect_text(&page, &mut raw);
        let out = tidy(&raw);
        assert!(out.contains("Title"), "{out}");
        assert!(out.contains("Nested bullet"), "{out}");
        assert!(out.contains("Cell"), "{out}");
    }

    #[test]
    fn a_soft_break_becomes_a_line_and_blank_runs_vanish() {
        assert_eq!(tidy("One\u{000b}Two\n\n\n  \nThree  "), "One\nTwo\nThree");
    }

    #[test]
    fn notes_do_not_leak_into_the_slide_body() {
        let slide = serde_json::json!({
            "objectId": "s1",
            "pageElements": [
                { "shape": { "text": { "textElements": [ { "textRun": { "content": "On the slide\n" } } ] } } }
            ],
            "slideProperties": { "notesPage": { "pageElements": [
                { "shape": { "text": { "textElements": [ { "textRun": { "content": "For the speaker\n" } } ] } } }
            ] } }
        });
        let notes = notes_text(&slide);
        let mut raw = String::new();
        collect_text(slide.get("pageElements").unwrap(), &mut raw);
        let body = tidy(&raw);
        assert_eq!(notes, "For the speaker");
        assert_eq!(body, "On the slide");
    }
}
