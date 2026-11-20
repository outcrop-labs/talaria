// POST /api/agent/open — the agent puts an existing file in front of the
// person, in the pane beside a Work, Plan or Research session.
//
// WHY A DOOR AT ALL. The pane lists the files the conversation's chips name
// (`GET /api/conversations/{id}/files`). Writers land a chip, and so do Google
// reads — but reading a Talaria document with `get_document` lands nothing,
// and nothing else an agent could call put an existing file there. So "open
// the plan here so we can iterate on it" got the plan pasted into chat. This
// door lands THE SAME CHIP a create or an edit lands, on the reply the agent is
// streaming now, and the pane already follows chips: no table, no new event, no
// new pane code path, and nothing special-cased by document type. A finished
// turn is never the target: a cron run or a job has no streaming reply, and
// gets 409 rather than opening the file in whoever's pane it last spoke in.
//
// WHOSE PANE, AND WHOSE ACCESS. The file opens for the person the turn is for:
// the author of the newest person's message before the landing row (a
// collaborator's message queued behind the live reply has a higher seq and
// never counts), else the conversation's owner (`turn_requester`). The open is
// allowed when THEY can read it — with the files route's own check, so what
// opens is what their pane lists — and refused with nothing landed when they
// cannot, which also keeps a title they could not see off the transcript. The
// agent's own access only shapes the answer: an agent that cannot read the
// file is told it opened, and nothing of its title or contents.
//
// WHAT THE PANE RENDERS, and nothing else: Talaria documents, sheets, pages
// and uploaded files, and Google Docs, Sheets, Slides and Drive files. A
// knowledge doc, a run transcript or an ordinary page is refused with a
// sentence that says where it opens instead.
//
// CALLED WITH NO FILE, it lists the session's files — what the person's pane
// lists, less the artifacts this agent cannot read — so an agent that lost its
// earlier tool results can bring one back without crawling list_documents.
//
// A GOOGLE FILE LANDS WITH ITS NAME when one is already known: the title the
// session gives it (a chip a Google read left, or its pin — the pane's own
// list), else the text of a markdown link, `[Q3 numbers](https://docs…)`.
// Nothing asks Google: a file nobody named still reads as "Google Sheet"
// until it is read or opened.
//
// No audit row: opening a file is chatter (API-CONVENTIONS § Audit).

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use talaria_agent_auth::{AgentCaller, require_agent};
use talaria_api_facades::kb::perms::{ITEM_ARTIFACT, can_edit_human, list_editors};
use talaria_artifacts::agent_access::{agent_edits_artifact, agent_readable, agent_reads_artifact};
use talaria_artifacts::{ArtifactHead, artifact_heads, get_artifact, guarded};
use talaria_body::{optional_max_string_member, parse};
use talaria_chips::session_files::{
    FileRef, SessionFile, TrailMessage, file_named_by, generic_google_label, session_google_title,
};
use talaria_chips::{
    GoogleFile, Landed, Target, classify_href, conversation_hint, document_chip, google_file_chip,
    land_chips, live_landing_target,
};
use talaria_conversations::{conversation_file_trail, turn_requester};
use talaria_error::{house_error, internal, object_or_400};
use talaria_state::AppState;
use talaria_work_mode::clean_focus_title;

use super::conversations_id_files::{FilesError, readable_for, session_files_for};

/// The conversation kinds that have a pane. Any other (a Comms chat) still
/// gets the link on the turn; the answer says there is no pane to open it in.
const PANE_KINDS: [&str; 3] = ["work", "plan", "research"];

/// The artifact kinds the pane renders (`ArtifactEditor`): anything else —
/// a run transcript — would open as "coming soon".
const PANE_ARTIFACTS: [&str; 4] = ["doc", "sheet", "microsite", "file"];

const NOT_A_FILE: &str = "that is not a link to a Talaria document or a Google file — the pane \
     shows Talaria documents, sheets, pages and uploaded files, and Google Docs, Sheets, Slides \
     and Drive files";

const NO_CONVERSATION: &str = "no conversation to open it in — this tool works inside a session \
     you are answering, not in a channel or outside a turn";

const THEY_CANNOT_READ: &str = "the person in this session cannot read that document, so it would \
     not appear in their pane — ask its owner to share it with them";

/// What the body asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ask {
    /// No file named: list the session's files.
    List,
    /// A file, and the text of the markdown link that named it — a Google
    /// file's title when the session has none (a Talaria file's title is
    /// its row's, always).
    Open(FileRef, Option<String>),
}

/// A markdown link, `[text](url)` — what an agent copies out of a tool answer
/// or a message — read as (text, url). The url may sit in angle brackets and
/// carry a quoted link title after it. None for anything else, so a bare URL
/// is read as itself.
fn markdown_link(s: &str) -> Option<(String, String)> {
    let rest = s.trim().strip_prefix('[')?;
    let (text, rest) = rest.split_once("](")?;
    let inner = rest.strip_suffix(')')?.trim();
    // `[t](<url> "title")`: the destination is the first token.
    let dest = inner.split_whitespace().next()?;
    let dest = dest
        .strip_prefix('<')
        .and_then(|d| d.strip_suffix('>'))
        .unwrap_or(dest);
    (!dest.is_empty()).then(|| (text.trim().to_string(), dest.to_string()))
}

/// The name a markdown link's text gives a Google file: cleaned to one line,
/// and nothing when the text is only the URL, its host, or what the file is.
fn link_title(text: Option<&str>, url: &str) -> Option<String> {
    text.filter(|t| !generic_google_label(t, url))
        .and_then(clean_focus_title)
}

/// The body, read: exactly one of `documentId` / `url`, or neither to list.
/// Pure, so every refusal sentence is a unit test.
fn read_body(obj: &Map<String, Value>) -> Result<Ask, String> {
    let document_id = optional_max_string_member(obj, "documentId", 200)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let url = optional_max_string_member(obj, "url", 2000)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match (document_id, url) {
        (None, None) => Ok(Ask::List),
        (Some(_), Some(_)) => Err("give documentId or url — one of them, not both".into()),
        (Some(id), None) => talaria_params::uuid_canonical(&id)
            .map(|id| Ask::Open(FileRef::Artifact(id), None))
            .ok_or_else(|| {
                "documentId is a Talaria document id (from list_documents or search_knowledge) — \
                 pass a link to a Google file as url"
                    .to_string()
            }),
        (None, Some(raw)) => {
            let (text, url) = match markdown_link(&raw) {
                Some((text, url)) => (Some(text), url),
                None => (None, raw),
            };
            read_link(&url, text)
        }
    }
}

/// A link the body named, as a file the pane can show.
fn read_link(url: &str, text: Option<String>) -> Result<Ask, String> {
    match file_named_by(url) {
        Some(file) => {
            let title = match &file {
                FileRef::Google(_) => link_title(text.as_deref(), url),
                FileRef::Artifact(_) => None,
            };
            Ok(Ask::Open(file, title))
        }
        None if classify_href(url).is_some_and(|l| l.entity == "kb") => Err(format!(
            "{NOT_A_FILE} — knowledge docs open on the Knowledge page, not in the pane"
        )),
        None => Err(NOT_A_FILE.to_string()),
    }
}

/// The refusal for an artifact the pane does not render, by its kind.
fn unrenderable(kind: &str) -> String {
    let where_it_opens = if kind == "run-transcript" {
        " — session transcripts open from the run view"
    } else {
        ""
    };
    format!(
        "that file is a {kind}, which the pane does not show — it shows documents, sheets, pages \
         and uploaded files{where_it_opens}"
    )
}

/// What the opened file is, for the answer: how the model should describe it.
fn artifact_type(kind: &str) -> &str {
    match kind {
        "doc" => "document",
        "sheet" => "sheet",
        "microsite" => "page",
        "file" => "file",
        other => other,
    }
}

/// The facts the relayed line is built from.
#[derive(Debug, Clone, Default)]
struct Opened<'a> {
    /// The file's title — only when the agent may read it.
    title: Option<&'a str>,
    google: bool,
    /// The conversation's kind has a pane.
    has_pane: bool,
    /// The file was already linked on this turn, so nothing moved.
    already: bool,
    /// The person can edit it (None: Google decides).
    they_can_edit: Option<bool>,
    you_can_read: bool,
    /// The agent can edit it (None: Google decides).
    you_can_edit: Option<bool>,
}

/// THE ONE LINE THE MODEL RELAYS, built from facts so it cannot claim what did
/// not happen: a file already linked on this turn did not come forward, and a
/// person without edit rights got it view-only.
fn opened_message(o: &Opened) -> String {
    let name = o.title.map(|t| format!("“{t}”"));
    let mut out = if !o.has_pane {
        format!(
            "Linked {} in this chat — only Work, Plan and Research sessions have a pane, so it \
             opens from the link.",
            name.as_deref().unwrap_or("the file")
        )
    } else if o.already {
        format!(
            "{} is already linked on this turn, so it did not move — if they closed its tab, it \
             is under All files in their pane.",
            name.as_deref().unwrap_or("That file")
        )
    } else {
        format!(
            "Opened {} in their pane.",
            name.as_deref().unwrap_or("the file")
        )
    };
    if o.they_can_edit == Some(false) {
        out.push_str(" They can read it but not edit it, so it opens view-only.");
    }
    if !o.you_can_read {
        out.push_str(
            " You cannot read it yourself, so its title and contents are not shown to you — ask \
             them what they need from it.",
        );
    } else if o.you_can_edit == Some(false) && o.they_can_edit != Some(false) {
        out.push_str(
            " You can read it but not edit it — they can make you an editor under Share & \
             permissions.",
        );
    }
    if o.google {
        out.push_str(
            " It opens with their own Google account; if the file is not shared with them, \
             Google shows its request-access page.",
        );
    }
    out
}

/// One session file as the agent reads it: enough to name it and to open it
/// again (`documentId` for a Talaria file, `url` for a Google one).
fn file_for_agent(f: &SessionFile) -> Value {
    let mut row = json!({
        "title": f.title,
        "type": match (f.kind, f.artifact_kind.as_deref(), f.google_type) {
            ("artifact", Some(kind), _) => artifact_type(kind).to_string(),
            (_, _, Some(google)) => format!("google {google}"),
            _ => "file".to_string(),
        },
        "lastTouchedAt": f.last_touched_at,
    });
    if let Some(obj) = row.as_object_mut() {
        match &f.url {
            Some(url) => obj.insert("url".into(), json!(url)),
            None => obj.insert("documentId".into(), json!(f.id)),
        };
    }
    row
}

/// The person's address string, the `who` every read check takes beside the
/// id (`who_of`: email, else name) — for the legacy owner fallback.
async fn who_of_user(pg: &sqlx::PgPool, user_id: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select coalesce(email, name) from users where id = $1::uuid")
            .bind(user_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(who,)| who))
}

/// Is a chip with this id already on the landing row? The chip write is
/// first-wins for links, and a file's touch time is its message's, so a
/// second open on the same turn moves nothing — and the answer must say so.
async fn already_on(pg: &sqlx::PgPool, landed: &Landed, chip: &Value) -> Result<bool, sqlx::Error> {
    let Some(id) = chip.get("id").and_then(Value::as_str) else {
        return Ok(false);
    };
    let row: Option<(bool,)> = sqlx::query_as(
        "select chips @> jsonb_build_array(jsonb_build_object('id', $2::text)) \
         from messages where id = $1::uuid",
    )
    .bind(&landed.message_id)
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some_and(|(on,)| on))
}

// doc: Open an existing file in the person's pane beside the session this agent is in — or, with no file named, list the session's files.

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, Response> {
    let caller = require_agent(&state.pg, &headers).await?;
    let parsed = parse(&body);
    let obj = object_or_400(&parsed)?;
    let ask = match read_body(obj) {
        Ok(a) => a,
        Err(msg) => return Ok(house_error(StatusCode::BAD_REQUEST, &msg)),
    };
    let hint = conversation_hint(&parsed);
    // THE TURN STREAMING NOW, never the recent tier: a cron run or a job has
    // no streaming reply, and the recent tier would open the file in the pane
    // of whoever the agent last spoke to (`live_landing_target`).
    let landed = match live_landing_target(&state.pg, &caller.model, hint.as_deref()).await {
        Ok(target @ Target::Ambiguous { .. }) => {
            return Ok(house_error(
                StatusCode::CONFLICT,
                &target.ambiguity().unwrap_or_default(),
            ));
        }
        // A CHANNEL TURN, OR NO TURN STREAMING: a channel reply has no pane,
        // and outside a live turn there is nobody to open it for.
        Ok(target) => match target.landed().filter(|l| l.conversation_id.is_some()) {
            Some(landed) => landed,
            None => return Ok(house_error(StatusCode::CONFLICT, NO_CONVERSATION)),
        },
        Err(e) => return Ok(internal("[open] conversation resolve failed", e)),
    };
    let conversation_id = landed.conversation_id.clone().unwrap_or_default();
    let kind: Option<(String,)> =
        match sqlx::query_as("select kind from conversations where id = $1::uuid")
            .bind(&conversation_id)
            .fetch_optional(&state.pg)
            .await
        {
            Ok(k) => k,
            Err(e) => return Ok(internal("[open] conversation read failed", e)),
        };
    let Some((kind,)) = kind else {
        return Ok(house_error(StatusCode::CONFLICT, NO_CONVERSATION));
    };
    let requester = match turn_requester(&state.pg, &landed.message_id).await {
        Ok(Some(who)) => who,
        Ok(None) => return Ok(house_error(StatusCode::CONFLICT, NO_CONVERSATION)),
        Err(e) => return Ok(internal("[open] requester read failed", e)),
    };
    let who = match who_of_user(&state.pg, &requester).await {
        Ok(w) => w,
        Err(e) => return Ok(internal("[open] requester read failed", e)),
    };

    match ask {
        Ask::List => list(&state, &caller, &conversation_id, &kind, &requester, who).await,
        Ask::Open(FileRef::Artifact(id), _) => {
            open_artifact(&state, &caller, &landed, &kind, &requester, who, &id).await
        }
        Ask::Open(FileRef::Google(file), link_named) => {
            // THE NAME THE SESSION ALREADY GIVES IT, else the link's text.
            // The session's is a Google answer or a pin; the link's is the
            // agent's own word for it — so the session's wins.
            let named = match google_title_in_session(&state.pg, &conversation_id, &file).await {
                Ok(t) => t,
                Err(e) => return Ok(internal("[open] trail read failed", e)),
            };
            let title = named.or(link_named);
            let chip = google_file_chip(&file, title.as_deref());
            let already = match already_on(&state.pg, &landed, &chip).await {
                Ok(a) => a,
                Err(e) => return Ok(internal("[open] row read failed", e)),
            };
            if let Err(e) = land_chips(&state.pg, state.redis().await.ok(), &landed, &[chip]).await
            {
                return Ok(internal("[open] chip landing failed", e));
            }
            let message = opened_message(&Opened {
                title: title.as_deref(),
                google: true,
                has_pane: PANE_KINDS.contains(&kind.as_str()),
                already,
                they_can_edit: None,
                you_can_read: true,
                you_can_edit: None,
            });
            let mut answer = json!({
                "key": file.key(),
                "kind": "google",
                "id": file.id,
                "type": format!("google {}", file.kind.as_str()),
                "url": file.url(),
            });
            if let (Some(t), Some(obj)) = (&title, answer.as_object_mut()) {
                obj.insert("title".into(), json!(t));
            }
            Ok(Json(json!({
                "ok": true,
                "conversationId": conversation_id,
                "file": answer,
                "alreadyOpen": already,
                "theyCanEdit": Value::Null,
                "youCanEdit": Value::Null,
                "message": message,
            }))
            .into_response())
        }
    }
}

/// The title this conversation already gives a Google file — the pane's own
/// reading of its chips and pins (`session_google_title`), cleaned to one
/// line: a Google file's title is anyone's to set, and it is quoted back to
/// the model in the answer.
async fn google_title_in_session(
    pg: &sqlx::PgPool,
    conversation_id: &str,
    file: &GoogleFile,
) -> Result<Option<String>, sqlx::Error> {
    let (rows, pinned) = conversation_file_trail(pg, conversation_id).await?;
    let trail: Vec<TrailMessage> = rows
        .into_iter()
        .map(|(created_at, chips, requested_by)| TrailMessage {
            created_at,
            chips,
            requested_by,
        })
        .collect();
    Ok(session_google_title(&trail, &pinned, file)
        .as_deref()
        .and_then(clean_focus_title))
}

/// The session's files for the agent: what the person's pane lists, newest
/// touch first, less the artifacts this agent cannot read.
async fn list(
    state: &AppState,
    caller: &AgentCaller,
    conversation_id: &str,
    kind: &str,
    requester: &str,
    who: Option<String>,
) -> Result<Response, Response> {
    let mut files = match session_files_for(
        &state.pg,
        requester,
        who.as_deref(),
        conversation_id,
        kind,
    )
    .await
    {
        Ok(f) => f,
        Err(FilesError(context, e)) => return Ok(internal(context, e)),
    };
    let ids: Vec<String> = files
        .iter()
        .filter(|f| f.kind == "artifact")
        .map(|f| f.id.clone())
        .collect();
    let heads = match artifact_heads(&state.pg, &ids).await {
        Ok(h) => h,
        Err(e) => return Ok(internal("[open] artifact read failed", e)),
    };
    let yours = match agent_readable(&state.pg, caller, &heads).await {
        Ok(y) => y,
        Err(e) => return Ok(internal("[open] agent read check failed", e)),
    };
    files.retain(|f| f.kind != "artifact" || yours.contains(&f.id));
    // Newest touch first; files nobody touched in a turn (pins) after them.
    files.sort_by(|a, b| b.last_touched_at.cmp(&a.last_touched_at));
    let message = if files.is_empty() {
        "No files in this session yet that you can read — pass documentId or url to open one."
    } else {
        "The files in this session, newest first. Pass one's documentId (a Talaria file) or url \
         (a Google file) to bring it back in front of them."
    };
    Ok(Json(json!({
        "ok": true,
        "conversationId": conversation_id,
        "files": files.iter().map(file_for_agent).collect::<Vec<_>>(),
        "message": message,
    }))
    .into_response())
}

async fn open_artifact(
    state: &AppState,
    caller: &AgentCaller,
    landed: &Landed,
    kind: &str,
    requester: &str,
    who: Option<String>,
    id: &str,
) -> Result<Response, Response> {
    let artifact = match get_artifact(&state.pg, id).await {
        Ok(Some(a)) => a,
        Ok(None) => return Ok(house_error(StatusCode::NOT_FOUND, "not found")),
        Err(e) => return Ok(internal("[open] artifact read failed", e)),
    };
    let g = guarded(&artifact);
    // THE PERSON'S READ, the files route's own — so a file opened here is a
    // file their pane lists. Before anything else: a refusal lands nothing,
    // and says nothing about a document they could not see.
    let head = ArtifactHead {
        id: artifact.id.clone(),
        kind: artifact.kind.clone(),
        title: artifact.title.clone(),
        guarded: g.clone(),
    };
    match readable_for(&state.pg, requester, who.as_deref(), vec![head]).await {
        Ok(r) if r.contains_key(&artifact.id) => {}
        Ok(_) => return Ok(house_error(StatusCode::FORBIDDEN, THEY_CANNOT_READ)),
        Err(e) => return Ok(internal("[open] read check failed", e)),
    }
    if !PANE_ARTIFACTS.contains(&artifact.kind.as_str()) {
        return Ok(house_error(
            StatusCode::BAD_REQUEST,
            &unrenderable(&artifact.kind),
        ));
    }
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => return Ok(internal("[open] grants read failed", e)),
    };
    let team_ids = match talaria_teams::team_ids_for_user(&state.pg, requester).await {
        Ok(t) => t,
        Err(e) => return Ok(internal("[open] team membership read failed", e)),
    };
    let they_can_edit = can_edit_human(&g, Some(requester), who.as_deref(), &editors, &team_ids);
    let (you_can_read, you_can_edit) = match (
        agent_reads_artifact(&state.pg, caller, &g, &editors).await,
        agent_edits_artifact(&state.pg, caller, &g, &editors).await,
    ) {
        (Ok(r), Ok(e)) => (r, e),
        (Err(e), _) | (_, Err(e)) => return Ok(internal("[open] agent access read failed", e)),
    };
    // Nothing of a file the agent cannot read reaches it — not on the chip
    // (a chip with no title reads as its kind, "Document"), not in the answer.
    let title = you_can_read.then_some(artifact.title.as_str());
    let chip = document_chip(&artifact.id, title, None);
    let already = match already_on(&state.pg, landed, &chip).await {
        Ok(a) => a,
        Err(e) => return Ok(internal("[open] row read failed", e)),
    };
    if let Err(e) = land_chips(&state.pg, state.redis().await.ok(), landed, &[chip]).await {
        return Ok(internal("[open] chip landing failed", e));
    }
    let message = opened_message(&Opened {
        title,
        google: false,
        has_pane: PANE_KINDS.contains(&kind),
        already,
        they_can_edit: Some(they_can_edit),
        you_can_read,
        you_can_edit: Some(you_can_edit),
    });
    let mut file = json!({
        "key": format!("artifact:{}", artifact.id),
        "kind": "artifact",
        "id": artifact.id,
    });
    if you_can_read && let Some(obj) = file.as_object_mut() {
        obj.insert("title".into(), json!(artifact.title));
        obj.insert("type".into(), json!(artifact_type(&artifact.kind)));
    }
    Ok(Json(json!({
        "ok": true,
        "conversationId": landed.conversation_id,
        "file": file,
        "alreadyOpen": already,
        "theyCanEdit": they_can_edit,
        "youCanEdit": you_can_read.then_some(you_can_edit),
        "message": message,
    }))
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2";

    fn body(v: Value) -> Result<Ask, String> {
        read_body(v.as_object().expect("an object"))
    }

    #[test]
    fn no_file_named_is_a_listing_and_both_named_is_refused() {
        assert_eq!(body(json!({})), Ok(Ask::List));
        assert_eq!(
            body(json!({ "documentId": "  ", "url": "" })),
            Ok(Ask::List)
        );
        let both = body(json!({ "documentId": DOC, "url": format!("/artifacts?a={DOC}") }));
        assert_eq!(
            both,
            Err("give documentId or url — one of them, not both".into())
        );
    }

    #[test]
    fn a_document_id_is_a_talaria_uuid_and_nothing_else() {
        assert_eq!(
            body(json!({ "documentId": DOC.to_uppercase() })),
            Ok(Ask::Open(FileRef::Artifact(DOC.to_string()), None))
        );
        let google_id = body(json!({ "documentId": "1AbCdEfGhIjKlMnOpQrStUvWxYz" }));
        assert!(
            google_id
                .as_ref()
                .is_err_and(|e| e.contains("pass a link to a Google file as url")),
            "{google_id:?}"
        );
        assert!(body(json!({ "documentId": 42 })).is_err());
    }

    #[test]
    fn a_link_opens_a_talaria_document_or_a_google_file_and_refuses_the_rest() {
        assert_eq!(
            body(json!({ "url": format!("https://talaria.example.com/artifacts?a={DOC}") })),
            Ok(Ask::Open(FileRef::Artifact(DOC.to_string()), None))
        );
        match body(json!({ "url": "https://docs.google.com/spreadsheets/d/1sheet/edit#gid=0" })) {
            Ok(Ask::Open(FileRef::Google(f), None)) => assert_eq!(f.key(), "google:1sheet"),
            other => panic!("{other:?}"),
        }
        let kb = body(json!({ "url": "/knowledge?doc=7e7e7e7e-aaaa-4bbb-8ccc-dddddddddddd" }));
        assert!(
            kb.as_ref()
                .is_err_and(|e| e.contains("knowledge docs open on the Knowledge page")),
            "{kb:?}"
        );
        assert_eq!(
            body(json!({ "url": "https://example.com/plan" })),
            Err(NOT_A_FILE.to_string())
        );
    }

    #[test]
    fn a_markdown_link_names_a_google_file_and_never_a_talaria_one() {
        // The text of a link the agent copied is the file's name for its tab…
        match body(
            json!({ "url": "[Q3 numbers](https://docs.google.com/spreadsheets/d/1sheet/edit#gid=0)" }),
        ) {
            Ok(Ask::Open(FileRef::Google(f), Some(title))) => {
                assert_eq!(f.key(), "google:1sheet");
                assert_eq!(title, "Q3 numbers");
            }
            other => panic!("{other:?}"),
        }
        // …in angle brackets, with a link title after it, or cleaned to one line.
        match body(
            json!({ "url": "[ \u{202E}Launch plan ](<https://docs.google.com/document/d/1doc/edit> \"Doc\")" }),
        ) {
            Ok(Ask::Open(FileRef::Google(f), Some(title))) => {
                assert_eq!(f.key(), "google:1doc");
                assert_eq!(title, "Launch plan");
            }
            other => panic!("{other:?}"),
        }
        // Text that is only the URL, its host or what the file is names nothing.
        for text in [
            "https://docs.google.com/document/d/1doc/edit",
            "docs.google.com",
            "Google Doc",
            "",
        ] {
            let url = format!("[{text}](https://docs.google.com/document/d/1doc/edit)");
            assert!(
                matches!(
                    body(json!({ "url": url })),
                    Ok(Ask::Open(FileRef::Google(_), None))
                ),
                "{text:?}"
            );
        }
        // A Talaria document's title is its row's, whatever the link says.
        assert_eq!(
            body(json!({ "url": format!("[Not its name](/artifacts?a={DOC})") })),
            Ok(Ask::Open(FileRef::Artifact(DOC.to_string()), None))
        );
        // A markdown link to something the pane cannot show is still refused.
        assert_eq!(
            body(json!({ "url": "[Plan](https://example.com/plan)" })),
            Err(NOT_A_FILE.to_string())
        );
        // A bare URL is read as itself.
        assert_eq!(
            markdown_link("https://docs.google.com/document/d/1doc/edit"),
            None
        );
        assert_eq!(markdown_link("[half](https://x"), None);
    }

    #[test]
    fn a_run_transcript_is_refused_with_where_it_opens() {
        assert!(unrenderable("run-transcript").contains("open from the run view"));
        assert!(!unrenderable("board").contains("run view"));
        for kind in PANE_ARTIFACTS {
            assert_ne!(artifact_type(kind), "", "{kind}");
        }
        assert_eq!(artifact_type("microsite"), "page");
    }

    fn opened() -> Opened<'static> {
        Opened {
            title: Some("Launch plan"),
            has_pane: true,
            they_can_edit: Some(true),
            you_can_read: true,
            you_can_edit: Some(true),
            ..Opened::default()
        }
    }

    #[test]
    fn the_relayed_line_says_what_happened_and_nothing_more() {
        assert_eq!(
            opened_message(&opened()),
            "Opened “Launch plan” in their pane."
        );
        let view_only = opened_message(&Opened {
            they_can_edit: Some(false),
            you_can_edit: Some(false),
            ..opened()
        });
        assert!(view_only.contains("They can read it but not edit it, so it opens view-only."));
        // Their missing edit right is the one to tell; the agent's is noise then.
        assert!(!view_only.contains("make you an editor"), "{view_only}");
        let agent_cannot_edit = opened_message(&Opened {
            you_can_edit: Some(false),
            ..opened()
        });
        assert!(agent_cannot_edit.contains("make you an editor under Share & permissions"));
    }

    #[test]
    fn an_agent_that_cannot_read_the_file_hears_nothing_of_it() {
        let line = opened_message(&Opened {
            title: None,
            you_can_read: false,
            you_can_edit: Some(false),
            ..opened()
        });
        assert!(line.starts_with("Opened the file in their pane."), "{line}");
        assert!(line.contains("You cannot read it yourself"), "{line}");
        assert!(!line.contains("Launch plan"));
    }

    #[test]
    fn an_already_linked_file_is_not_claimed_as_opened() {
        let line = opened_message(&Opened {
            already: true,
            ..opened()
        });
        assert!(
            line.starts_with("“Launch plan” is already linked on this turn"),
            "{line}"
        );
        assert!(!line.contains("Opened"), "{line}");
    }

    #[test]
    fn a_named_google_file_is_opened_by_its_name() {
        let line = opened_message(&Opened {
            title: Some("Q3 numbers"),
            google: true,
            they_can_edit: None,
            you_can_edit: None,
            ..opened()
        });
        assert!(
            line.starts_with("Opened “Q3 numbers” in their pane."),
            "{line}"
        );
    }

    #[test]
    fn a_google_file_and_a_chat_without_a_pane_say_so() {
        let google = opened_message(&Opened {
            title: None,
            google: true,
            they_can_edit: None,
            you_can_edit: None,
            ..opened()
        });
        assert!(google.contains("request-access page"), "{google}");
        assert!(!google.contains("view-only"), "{google}");
        let chat = opened_message(&Opened {
            has_pane: false,
            ..opened()
        });
        assert!(
            chat.starts_with("Linked “Launch plan” in this chat — only Work, Plan and Research"),
            "{chat}"
        );
    }

    #[test]
    fn a_listed_file_carries_what_opens_it_again() {
        let talaria = SessionFile {
            key: format!("artifact:{DOC}"),
            kind: "artifact",
            id: DOC.into(),
            title: Some("Launch plan".into()),
            artifact_kind: Some("doc".into()),
            google_type: None,
            url: None,
            source: "touched",
            last_touched_at: Some("2026-11-19T10:00:00.000Z".into()),
            touched_for: Some("11111111-1111-4111-8111-111111111111".into()),
        };
        let row = file_for_agent(&talaria);
        assert_eq!(row["documentId"], DOC);
        assert_eq!(row["type"], "document");
        // Who a turn was for is the pane's business, not the agent's.
        assert!(row.get("touchedFor").is_none(), "{row}");
        let google = SessionFile {
            key: "google:1sheet".into(),
            kind: "google",
            id: "1sheet".into(),
            title: None,
            artifact_kind: None,
            google_type: Some("spreadsheet"),
            url: Some("https://docs.google.com/spreadsheets/d/1sheet/edit".into()),
            source: "pinned",
            last_touched_at: None,
            touched_for: None,
        };
        let row = file_for_agent(&google);
        assert_eq!(
            row["url"],
            "https://docs.google.com/spreadsheets/d/1sheet/edit"
        );
        assert_eq!(row["type"], "google spreadsheet");
        assert!(row.get("documentId").is_none(), "{row}");
    }
}
