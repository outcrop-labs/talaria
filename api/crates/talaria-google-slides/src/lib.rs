// Google Slides: read a deck, and replace text in one.
//
// WHAT IS DELIBERATELY ABSENT. There is no authoring here — nothing creates a
// slide, moves a box or adds a bullet. The Slides API edits through batched
// requests against shape and placeholder ids, so inserting content means
// knowing which placeholder on which layout it belongs to, and a tool that
// guessed would produce decks with text off the edge of the slide and boxes
// stacked on each other. The two operations below need none of that: reading
// walks what is there, and replacing swaps strings inside boxes a designer
// already placed, so the deck keeps the shape a person gave it.
//
// That is not a stage on the way to authoring. It is the line: the deck's
// LAYOUT belongs to whoever made it, and an agent changing its words is a
// different act from an agent building it. `import_drive_file` still exists for
// anyone who wants the deck as a PDF artifact.
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
    fn occurrences_sum_across_replies_and_absent_counts_read_as_zero() {
        // Two pairs, one of which matched nothing: Google omits the count
        // rather than sending 0, and a caller that read the first reply only
        // would report a partial success as a whole one.
        let body = serde_json::json!({ "replies": [
            { "replaceAllText": { "occurrencesChanged": 3 } },
            { "replaceAllText": {} }
        ]});
        assert_eq!(occurrences_changed(&body), 3);
    }

    #[test]
    fn nothing_matched_is_zero_not_a_failure() {
        // The honest answer when the text was not found. A caller must report
        // it rather than call the write a success.
        assert_eq!(
            occurrences_changed(&serde_json::json!({ "replies": [ { "replaceAllText": {} } ] })),
            0
        );
        assert_eq!(occurrences_changed(&serde_json::json!({})), 0);
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

// ── The write half ───────────────────────────────────────────────────────────
//
// TEXT REPLACEMENT AND NOTHING ELSE, and the narrowness is the design rather
// than a stage on the way to more. The Slides API edits through batched
// requests against shape and placeholder ids: inserting a bullet means knowing
// which placeholder on which layout it belongs to, and a tool that guessed
// would produce decks with text off the edge of the slide and boxes stacked on
// each other. `replaceAllText` needs none of that — it swaps strings wherever
// they already appear, inside boxes a designer already placed, so the deck
// stays the shape a person made it.
//
// That covers the ask people actually have: the number changed, the date moved,
// the client's name is spelled wrong on nine slides. What it deliberately does
// not cover is authoring a deck, and the tool description says so rather than
// letting an agent discover it by producing something unusable.

/// What a replacement pass changed, from Google's own reply.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeckUpdate {
    pub id: String,
    pub url: String,
    /// Total matches replaced across every requested pair. Zero is a real
    /// answer and an important one — it means the text was not found, which a
    /// caller must report rather than call success.
    pub occurrences_changed: i64,
}

/// One find/replace pair.
#[derive(Debug, Clone)]
pub struct TextReplacement<'a> {
    pub find: &'a str,
    pub replace: &'a str,
}

/// Total matches across every reply. A batch of N replacements answers with N
/// replies, and a reply for a pair that matched nothing simply omits the count
/// rather than reporting zero — so this sums what is there and treats an absent
/// count as the zero it means. Separated from the request so it can be tested
/// against the shapes Google actually returns.
fn occurrences_changed(body: &serde_json::Value) -> i64 {
    body.get("replies")
        .and_then(|r| r.as_array())
        .map(|replies| {
            replies
                .iter()
                .filter_map(|r| {
                    r.get("replaceAllText")
                        .and_then(|v| v.get("occurrencesChanged"))
                        .and_then(serde_json::Value::as_i64)
                })
                .sum()
        })
        .unwrap_or(0)
}

/// Replace text across a deck. `match_case` is passed through rather than
/// assumed: "Q3" and "q3" are usually the same intent, and a name usually is
/// not, so the caller decides.
#[tracing::instrument(skip(token, pairs))]
pub async fn replace_text_with_token(
    token: &str,
    id: &str,
    pairs: &[TextReplacement<'_>],
    match_case: bool,
) -> Result<DeckUpdate, GoogleError> {
    if pairs.is_empty() {
        return Err(GoogleError::Failed("no replacements given".into()));
    }
    let requests: Vec<serde_json::Value> = pairs
        .iter()
        .map(|p| {
            serde_json::json!({
                "replaceAllText": {
                    "containsText": { "text": p.find, "matchCase": match_case },
                    "replaceText": p.replace,
                }
            })
        })
        .collect();
    let res = http()
        .post(format!(
            "{SLIDES_ENDPOINT}/{}:batchUpdate",
            percent_encode(id)
        ))
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(serde_json::json!({ "requests": requests }).to_string())
        .send()
        .await
        .map_err(|e| GoogleError::Failed(format!("slides update request: {e}")))?;
    let status = res.status();
    if !status.is_success() {
        let body = res.text().await.unwrap_or_default();
        return Err(GoogleError::Failed(format!(
            "slides update failed: {status} {body}"
        )));
    }
    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|e| GoogleError::Failed(format!("slides update body: {e}")))?;
    Ok(DeckUpdate {
        id: id.to_string(),
        url: format!("https://docs.google.com/presentation/d/{id}/edit"),
        occurrences_changed: occurrences_changed(&body),
    })
}
