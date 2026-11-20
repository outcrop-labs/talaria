// Chat chips — the record a conversation or channel message carries when an
// agent shares a platform link, exposes a tool, or attempts something that
// leaves the chat. Recognition and extraction are pure; landing a chip is
// the one write every caller shares, so a tool result and a confirm-send
// cannot invent two different shapes.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use talaria_body::clip_chars;
use talaria_conversations::fan_conversation_event;
use talaria_notify::{NotifyDeps, fan_channel_event};
use talaria_realtime::{ChannelEvent, RealtimeDeps, publish_channel};

mod google_file;
pub mod session_files;

pub use google_file::{GoogleFile, GoogleType, google_id_ok, parse_google_file_url};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformLink {
    pub href: String,
    pub entity: &'static str,
    pub id: String,
    pub board_id: Option<String>,
}

/// Tools an approval unlocks. The UI copy of this map lives in
/// `ui/src/lib/chips.ts` (`toolsUnlockedBy`) — the two lists are the contract.
pub fn tools_unlocked(kind: &str) -> &'static [&'static str] {
    match kind {
        "gmail_send" => &["draft_email"],
        "calendar_create" => &["draft_calendar_event"],
        "calendar_update" => &["update_google_event"],
        "calendar_cancel" => &["cancel_google_event"],
        "meeting_create" => &["create_google_meeting"],
        "doc_update" => &["update_google_doc", "append_google_doc"],
        "sheet_update" => &["update_google_sheet"],
        "slides_update" => &["update_google_slides"],
        "drive_move" => &["move_google_file"],
        "drive_rename" => &["rename_google_file"],
        // A ticket move unlocks nothing: the approval IS the move, made by
        // the person who approved it. (It used to post a `triage_ticket`
        // grant that no door ever consumed.)
        "board_access" => &["join_board"],
        _ => &[],
    }
}

/// Approval kinds whose row lives in `google_pending_actions` and executes
/// through the confirm-send plane (`decide_action`). The chip approve route
/// asks this rather than re-listing the kinds: a kind that queues a Google
/// write but is missing here would be marked approved and never sent.
/// `mcp_call` is a held call to one of Google's Workspace MCP servers.
pub fn is_google_action(kind: &str) -> bool {
    matches!(
        kind,
        "mcp_call"
            | "gmail_send"
            | "calendar_create"
            | "calendar_update"
            | "calendar_cancel"
            | "meeting_create"
            | "doc_update"
            | "sheet_update"
            | "slides_update"
            | "drive_move"
            | "drive_rename"
            | "doc_create"
            | "drive_folder_create"
    )
}

/// The pending-action kind a queued Google tool produces, with the card's
/// fallback wording. EVERY tool whose route answers through `answer_queued`
/// belongs here: the queue is not the surface, the chip is, and a queued
/// write with no chip is one the person only finds by asking the agent to
/// list pending sends. `import_drive_file` is deliberately absent — it never
/// queues; `create_google_doc` / `create_google_folder` queue only when the
/// caller named a parent folder the agent did not create (a file placed in
/// someone's shared folder is a share), so they are here for that case.
fn google_approval_for(tool: &str) -> Option<(&'static str, &'static str)> {
    Some(match tool {
        "create_google_doc" => ("doc_create", "Create a Google Doc in a folder"),
        "create_google_folder" => ("drive_folder_create", "Create a Drive folder in a folder"),
        "draft_email" => ("gmail_send", "Send email"),
        "draft_calendar_event" => ("calendar_create", "Create event"),
        "update_google_event" => ("calendar_update", "Change an event"),
        "cancel_google_event" => ("calendar_cancel", "Cancel an event"),
        "create_google_meeting" => ("meeting_create", "Create a meeting"),
        "update_google_doc" => ("doc_update", "Edit a Google Doc"),
        "update_google_sheet" => ("sheet_update", "Write to a Google Sheet"),
        "update_google_slides" => ("slides_update", "Replace text in a Slides deck"),
        "append_google_doc" => ("doc_update", "Add to a Google Doc"),
        "move_google_file" => ("drive_move", "Move a Drive file"),
        "rename_google_file" => ("drive_rename", "Rename a Drive file"),
        _ => return None,
    })
}

/// A ticket tool's answer was a REFUSAL: a 403 as the toolkit words it
/// (`Talaria API 403: …`), or the api's own `{error}` body. Never a landed
/// write's answer — `{ task, … }`, the ticket itself, whose title or
/// description may say anything, "Talaria API 403" included.
fn is_refusal(result: &str, result_v: &Value) -> bool {
    if result_v.get("task").is_some() {
        return false;
    }
    result.contains("Talaria API 403") || result_v.get("error").is_some()
}

/// The approval kinds that move a ticket: a refused `triage_ticket`
/// (`ticket_move`) and a refused `move_ticket` (`ticket_board_move`). The
/// approve route gates both on the ticket's BOARD — an editor of it, or an
/// admin — and runs the move as the person who approved.
pub fn is_ticket_approval(kind: &str) -> bool {
    matches!(kind, "ticket_move" | "ticket_board_move")
}

fn is_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Path-shaped recognition. An absolute URL keeps only its path and query.
/// Unrecognized URLs return None — the caller leaves them as links.
pub fn classify_href(raw: &str) -> Option<PlatformLink> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("mention:")
        || trimmed.to_ascii_lowercase().starts_with("javascript:")
    {
        return None;
    }
    let path = if let Some(rest) = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
    {
        let after_host = rest.find('/')?;
        &rest[after_host..]
    } else if trimmed.starts_with('/') {
        trimmed
    } else {
        return None;
    };
    let (pathname, query) = path.split_once('?').unwrap_or((path, ""));
    let pathname = pathname.trim_end_matches('/');
    let seg: Vec<&str> = pathname.split('/').filter(|s| !s.is_empty()).collect();
    let param = |key: &str| {
        query.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key && is_id(v)).then(|| v.to_string())
        })
    };

    if seg.first() == Some(&"boards") && seg.len() == 3 && is_id(seg[1]) && is_id(seg[2]) {
        return Some(PlatformLink {
            href: format!("/boards/{}/{}", seg[1], seg[2]),
            entity: "ticket",
            id: seg[2].to_string(),
            board_id: Some(seg[1].to_string()),
        });
    }
    if seg.first() == Some(&"boards") && seg.len() == 2 && is_id(seg[1]) {
        return Some(PlatformLink {
            href: format!("/boards/{}", seg[1]),
            entity: "board",
            id: seg[1].to_string(),
            board_id: None,
        });
    }
    if seg.first() == Some(&"knowledge") {
        if let Some(doc) = param("doc").or_else(|| param("d")) {
            return Some(PlatformLink {
                href: format!("/knowledge?doc={doc}"),
                entity: "kb",
                id: doc,
                board_id: None,
            });
        }
        if seg.len() == 3 && is_id(seg[1]) && is_id(seg[2]) {
            return Some(PlatformLink {
                href: format!("/knowledge/{}/{}", seg[1], seg[2]),
                entity: "kb",
                id: seg[2].to_string(),
                board_id: None,
            });
        }
    }
    if seg.first() == Some(&"artifacts")
        && let Some(id) = param("a")
    {
        return Some(PlatformLink {
            href: format!("/artifacts?a={id}"),
            entity: "document",
            id,
            board_id: None,
        });
    }
    if seg.first() == Some(&"comms")
        && seg.get(1) == Some(&"channel")
        && seg.len() == 3
        && is_id(seg[2])
    {
        return Some(PlatformLink {
            href: format!("/comms/channel/{}", seg[2]),
            entity: "channel",
            id: seg[2].to_string(),
            board_id: None,
        });
    }
    if seg.first() == Some(&"plan") {
        let id = if seg.len() == 2 && is_id(seg[1]) {
            Some(seg[1].to_string())
        } else {
            param("p")
        };
        if let Some(id) = id {
            return Some(PlatformLink {
                href: format!("/plan/{id}"),
                entity: "plan",
                id,
                board_id: None,
            });
        }
    }
    if seg.first() == Some(&"research") {
        let id = if seg.len() == 2 && is_id(seg[1]) {
            Some(seg[1].to_string())
        } else {
            param("r")
        };
        if let Some(id) = id {
            return Some(PlatformLink {
                href: format!("/research/{id}"),
                entity: "research",
                id,
                board_id: None,
            });
        }
    }
    None
}

fn link_chip(link: &PlatformLink, title: Option<&str>) -> Value {
    json!({
        "id": format!("link:{}", link.href),
        "kind": "link",
        "href": link.href,
        "entity": link.entity,
        "title": title.filter(|t| !t.is_empty()),
    })
}

/// THE DOCUMENT CHIP, written once because it has TWO PRODUCERS.
///
/// The edit route lands it the moment an agent edits a document, and the
/// tool-events door lands it again from the tool result. That redundancy is
/// deliberate: the plugin clamps a tool result to 4000 chars, so the round
/// trip is best-effort by construction, while the route holds the change
/// list in full. Both go through here, so both land the SAME chip — the id
/// is the href, which is per DOCUMENT, and the second landing merges onto
/// the first instead of adding a row.
///
/// `changes` is whatever the producer has (the route's `changes` array, or
/// an `edit_document` result's) and is narrowed HERE, so neither producer
/// can put a row on a chip that the other would have dropped.
pub fn document_chip(id: &str, title: Option<&str>, changes: Option<&Value>) -> Value {
    let mut chip = link_chip(
        &PlatformLink {
            href: format!("/artifacts?a={id}"),
            entity: "document",
            id: id.to_string(),
            board_id: None,
        },
        title,
    );
    // WHAT THE EDIT DID, on the same chip rather than a second one. The way
    // into the document is still the point; the changes are how a reader
    // sees whether they need to go.
    if let Some(rows) = changes
        .and_then(Value::as_array)
        .and_then(|rows| doc_change_rows(rows))
        && let Some(obj) = chip.as_object_mut()
    {
        obj.insert("changes".into(), rows);
    }
    chip
}

/// The approval chip for a held Google Workspace MCP call (`mcp_call`): the
/// one generic "Google tool call" card, whose summary names the product, the
/// tool and the start of the arguments. The gateway lands it when it queues
/// the call — an MCP tool's name reaches the tool-events door prefixed by the
/// agent's MCP client, so `chips_from_tool` would never recognize it.
pub fn mcp_call_chip(action_id: &str, summary: &str) -> Value {
    approval_chip(action_id, "mcp_call", summary)
}

fn approval_chip(action_id: &str, kind: &str, summary: &str) -> Value {
    json!({
        "id": format!("approval:{action_id}"),
        "kind": "approval",
        "actionId": action_id,
        "actionKind": kind,
        "summary": summary,
        "status": "pending",
    })
}

pub fn unlock_chip(approval_id: &str, kind: &str) -> Value {
    unlock_chip_for(approval_id, kind, tools_unlocked(kind))
}

/// `unlock_chip` naming the tools the approval actually unlocked — for an
/// `mcp_call`, the `<server>/<tool>` key its grant was written under.
pub fn unlock_chip_for<S: AsRef<str>>(approval_id: &str, kind: &str, tools: &[S]) -> Value {
    json!({
        "id": format!("unlock:{approval_id}"),
        "kind": "unlock",
        "approvalId": approval_id,
        "actionKind": kind,
        "tools": tools.iter().map(AsRef::as_ref).collect::<Vec<&str>>(),
        "status": "approved",
    })
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// The platform links in one tool answer, as link chips: a string the href
/// classifier accepts, or an object naming a ticket by `boardId` + `id`.
///
/// A LISTING IS NOT A TRAIL, so the walk never enters an array. A link chip
/// says "this turn made or worked on this" — the ticket a call created, the
/// one it moved. An array in an answer is a listing: `list_tickets`' rows,
/// search hits, a ticket's comments or its similar tickets, each one only
/// looked at. A chip per entry buried the one thing the turn was about under
/// every row it read, on a transcript everyone in the thread sees. (Search
/// pages are read separately, by `sources_from`, and Google files by
/// `google_file_from`; neither goes through here.)
fn walk_links(v: &Value, title: Option<&str>, out: &mut Vec<Value>) {
    match v {
        Value::String(s) => {
            if let Some(link) = classify_href(s) {
                out.push(link_chip(&link, title));
            }
        }
        Value::Object(map) => {
            let sibling = str_field(v, "title").or(str_field(v, "name")).or(title);
            if let (Some(board), Some(id)) = (str_field(v, "boardId"), str_field(v, "id"))
                && is_id(board)
                && is_id(id)
            {
                let link = PlatformLink {
                    href: format!("/boards/{board}/{id}"),
                    entity: "ticket",
                    id: id.to_string(),
                    board_id: Some(board.to_string()),
                };
                out.push(link_chip(&link, sibling.or(str_field(v, "ticketRef"))));
            }
            for (k, child) in map {
                if k == "boardId" || k == "id" {
                    continue;
                }
                walk_links(child, sibling, out);
            }
        }
        _ => {}
    }
}

fn first_id(v: &Value) -> Option<&str> {
    str_field(v, "id")
        .or_else(|| v.get("artifact").and_then(|a| str_field(a, "id")))
        .or_else(|| v.get("doc").and_then(|a| str_field(a, "id")))
        .or_else(|| v.get("task").and_then(|a| str_field(a, "id")))
        .or_else(|| v.get("document").and_then(|a| str_field(a, "id")))
}

fn first_title(v: &Value) -> Option<&str> {
    str_field(v, "title")
        .or_else(|| v.get("artifact").and_then(|a| str_field(a, "title")))
        .or_else(|| v.get("doc").and_then(|a| str_field(a, "title")))
        .or_else(|| v.get("task").and_then(|a| str_field(a, "title")))
        .or_else(|| str_field(v, "name"))
}

/// How many change rows a document chip carries. A surgical edit is a handful
/// of anchors; a batch of fifty is a rewrite wearing an edit's clothes, and a
/// chip is not where a person reads all of it.
const MAX_DOC_CHANGES: usize = 20;

/// How many pages one search lands on the row. Six is what a reader scans
/// before the row stops being a trail and starts being the answer.
const MAX_SOURCES: usize = 6;

/// A source chip's snippet ceiling, with the clip mark inside it.
const SNIPPET: usize = 200;

/// A source chip's label ceiling. A page title is whatever the page says it
/// is, and some pages say it in a paragraph.
const LABEL: usize = 120;

/// Longest URL worth putting on a chip.
const MAX_URL: usize = 2048;

fn num_field<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    v.get(key).filter(|n| n.is_number())
}

/// An edit's change rows, narrowed to the shape the chip states:
/// `{label, added, removed}` (LINE deltas — `talaria_doc_edit::Change`),
/// plus `preview: {old, new}` for a text replacement (a section edit has
/// none).
///
/// A ROW THAT IS NOT THAT SHAPE IS DROPPED rather than passed through. The
/// chip renders what it is handed, so a row missing its counts would render
/// as a change of nothing — which reads as a fact about the document when it
/// is a fact about the payload.
fn doc_change_rows(rows: &[Value]) -> Option<Value> {
    let mut out = Vec::new();
    for row in rows {
        if out.len() >= MAX_DOC_CHANGES {
            break;
        }
        let Some(label) = str_field(row, "label") else {
            continue;
        };
        let (Some(added), Some(removed)) = (num_field(row, "added"), num_field(row, "removed"))
        else {
            continue;
        };
        let mut entry = json!({ "label": label, "added": added, "removed": removed });
        // `new` may be the empty string — that is a deletion — so the
        // preview's sides are checked for their TYPE, not their length.
        if let Some(p) = row.get("preview")
            && let Some(old) = p.get("old").and_then(Value::as_str)
            && let Some(fresh) = p.get("new").and_then(Value::as_str)
            && let Some(obj) = entry.as_object_mut()
        {
            obj.insert("preview".into(), json!({ "old": old, "new": fresh }));
        }
        out.push(entry);
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Array(out))
    }
}

/// The tools that bring pages back from the open web. `web_search` is
/// Talaria's own (dispatched in `talaria_capability_platform`); `search_web`
/// is the other spelling the capability catalogue recognizes
/// (`talaria_capability_reach`'s reach rules). Provider-prefixed names
/// (`brave_web_search`, `exa_search`) are deliberately NOT guessed at here —
/// a chip kind invented for a tool nobody calls is worse than none.
fn is_web_search(tool: &str) -> bool {
    matches!(tool, "web_search" | "search_web")
}

/// The authority of an absolute http(s) URL — the fallback label when a
/// result carries no title, because a host says where a reader would be
/// going and a bare URL makes them parse that themselves.
///
/// WHAT A READER RECOGNIZES, not what the URL spells: `www.` and a `:port`
/// are machinery, and a chip that says `www.nytimes.com:443` reads as a
/// different site from the `nytimes.com` the research report cites for the
/// same page.
fn host_of(url: &str) -> &str {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    // `user:pass@host` → `host`: a chip label is not a place for credentials.
    let host = authority.rsplit('@').next().unwrap_or(authority);
    // An IPv6 literal's colons ARE the address — only the `]` ends it.
    let host = if host.starts_with('[') {
        host.find(']').map_or(host, |i| &host[..=i])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    host.strip_prefix("www.").unwrap_or(host)
}

/// An absolute http(s) URL, or None. Everything else a result might carry —
/// a relative path, a `data:` blob, a mail link — is not a page a reader can
/// be sent to.
fn web_url(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.len() > MAX_URL || !(t.starts_with("http://") || t.starts_with("https://")) {
        return None;
    }
    (!host_of(t).is_empty()).then_some(t)
}

/// A page's chip id: `source:` plus the first 16 hex of the URL's sha256.
/// DERIVED FROM THE URL, never from its position in the list, so the same
/// page found by two engines is one chip and a retried search re-lands the
/// chips it already landed instead of stacking copies — the merge is
/// first-wins on the id.
fn source_id(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    h.finalize()
        .into_iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn source_chip(url: &str, title: Option<&str>, snippet: Option<&str>) -> Value {
    let label = title
        .map(|t| clip_chars(t.trim(), LABEL))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| host_of(url).to_string());
    let mut chip = json!({
        "id": format!("source:{}", source_id(url)),
        "kind": "source",
        "label": label,
        "href": url,
    });
    // ONE LINE. An engine's snippet arrives with the page's own line breaks
    // and runs of spaces in it, and a chip has one line to say what the page
    // is.
    let detail = snippet
        .map(|s| clip_chars(&s.split_whitespace().collect::<Vec<_>>().join(" "), SNIPPET))
        .filter(|s| !s.is_empty());
    if let Some(detail) = detail
        && let Some(obj) = chip.as_object_mut()
    {
        obj.insert("detail".into(), json!(detail));
    }
    chip
}

/// The Google tools that work ON ONE FILE, and the type of file their `id`
/// names. A search (`find_google_files`, `search_drive`) is absent on
/// purpose: a list of hits is not a file the agent opened, and landing ten of
/// them would fill the Work pane with files nobody looked at. So are the
/// queued Drive moves and renames — those manage a file, they do not work in
/// it.
fn google_file_tool(tool: &str) -> Option<GoogleType> {
    Some(match tool {
        "create_google_doc" | "read_google_doc" | "update_google_doc" | "append_google_doc" => {
            GoogleType::Document
        }
        "read_google_sheet" | "update_google_sheet" => GoogleType::Spreadsheet,
        "read_google_slides" | "update_google_slides" => GoogleType::Presentation,
        "read_drive_file" => GoogleType::Drive,
        _ => return None,
    })
}

/// The Google file one of those tools touched: the URL the result returned
/// when it parses (Google's own link knows the type exactly), else the id —
/// the result's, then the argument's — under the type the tool implies.
///
/// NOTHING FOR A FAILED CALL, and nothing for a create that QUEUED: a
/// `create_google_doc` into somebody's folder answers `{ pending }`, and
/// until a person approves it there is no file to point at. A queued EDIT
/// still names a file that exists, so it lands one.
fn google_file_from(tool: &str, args_v: &Value, result_v: &Value) -> Option<GoogleFile> {
    let kind = google_file_tool(tool)?;
    if result_v.get("error").is_some() {
        return None;
    }
    if tool == "create_google_doc" && result_v.get("pending").is_some() {
        return None;
    }
    if let Some(file) = str_field(result_v, "url").and_then(parse_google_file_url) {
        return Some(file);
    }
    let id = str_field(result_v, "id").or_else(|| str_field(args_v, "id"))?;
    google_id_ok(id).then(|| GoogleFile {
        id: id.to_string(),
        kind,
    })
}

/// A Google file as a chip: a `source` — an off-platform link with a label,
/// which is exactly how a reader should meet a Google file in the transcript
/// (a pill that opens it in a new tab) — at the file's CANONICAL URL, so every
/// tool that touches the same file lands the same chip id and the row keeps
/// one. `title` rides beside the label only when the tool knew it; the label
/// falls back to what the file is ("Google Doc"), never to a bare host.
pub fn google_file_chip(file: &GoogleFile, title: Option<&str>) -> Value {
    let url = file.url();
    let title = title
        .map(|t| clip_chars(t.trim(), LABEL))
        .filter(|t| !t.is_empty());
    let mut chip = json!({
        "id": format!("source:{}", source_id(&url)),
        "kind": "source",
        "label": title.clone().unwrap_or_else(|| file.kind.noun().to_string()),
        "href": url,
    });
    if let Some(t) = title
        && let Some(obj) = chip.as_object_mut()
    {
        obj.insert("title".into(), json!(t));
    }
    chip
}

/// Pages in a structured result, however the tool shaped them: any object
/// carrying a `url`/`link`/`href` string is a hit, with `title`/`name` and
/// `snippet`/`description`/`content` beside it when they are there.
///
/// KEYS RATHER THAN A SCHEMA, because the tool on the other end is usually an
/// MCP server the platform does not own — exa, brave and tavily each spell
/// this slightly differently, and a parser that only knew Talaria's own
/// `SearchResult` would land no sources at all for any of them.
fn walk_sources(v: &Value, seen: &mut Vec<String>, out: &mut Vec<Value>) {
    if out.len() >= MAX_SOURCES {
        return;
    }
    match v {
        Value::Array(items) => {
            for item in items {
                walk_sources(item, seen, out);
            }
        }
        Value::Object(map) => {
            let hit = str_field(v, "url")
                .or_else(|| str_field(v, "link"))
                .or_else(|| str_field(v, "href"))
                .and_then(web_url);
            if let Some(url) = hit
                && !seen.iter().any(|s| s == url)
            {
                seen.push(url.to_string());
                out.push(source_chip(
                    url,
                    str_field(v, "title").or_else(|| str_field(v, "name")),
                    str_field(v, "snippet")
                        .or_else(|| str_field(v, "description"))
                        .or_else(|| str_field(v, "content")),
                ));
            }
            for child in map.values() {
                walk_sources(child, seen, out);
            }
        }
        _ => {}
    }
}

/// The URLs in one line of prose, with the punctuation a sentence wraps them
/// in trimmed back off.
fn urls_in(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = line[from..].find("http") {
        let at = from + rel;
        let tail = &line[at..];
        if tail.starts_with("http://") || tail.starts_with("https://") {
            let end = tail
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
                .unwrap_or(tail.len());
            let url = tail[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
            if let Some(url) = web_url(url) {
                out.push(url.to_string());
            }
            // `end` is at least the length of the scheme, so this always
            // advances.
            from = at + end;
        } else {
            from = at + "http".len();
        }
        if out.len() >= MAX_SOURCES {
            break;
        }
    }
    out
}

/// A list item's number or bullet, removed so a title line reads as a title.
/// Digits are only an enumerator when a `.` or `)` follows them — "2026
/// outlook" is a title that starts with a number.
fn strip_enumerator(s: &str) -> &str {
    let t = s.trim();
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits)
        && let Some(rest) = t[digits..]
            .strip_prefix(". ")
            .or_else(|| t[digits..].strip_prefix(") "))
    {
        return rest.trim();
    }
    t.trim_start_matches(['-', '*', ' ']).trim()
}

/// Pages in a PROSE result. Talaria's own `web_search` answers the model in
/// numbered title/url/snippet triples, and a Hermes tool reports whatever
/// text it printed — so the common result never parses as JSON, and a parser
/// that gave up there would leave the turn with no citations at all.
///
/// A URL ON ITS OWN LINE takes the line above as its title and the line below
/// as its snippet, which is exactly that shape; a URL inside a sentence is
/// carried alone, because the sentence around it is not a title.
fn prose_sources(text: &str, seen: &mut Vec<String>, out: &mut Vec<Value>) {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if out.len() >= MAX_SOURCES {
            return;
        }
        let urls = urls_in(line);
        let whole_line = urls.len() == 1 && line.trim() == urls[0];
        for url in urls {
            if out.len() >= MAX_SOURCES {
                return;
            }
            if seen.iter().any(|s| s == &url) {
                continue;
            }
            seen.push(url.clone());
            let neighbour = |at: usize| -> Option<&str> {
                let l = lines.get(at)?.trim();
                (!l.is_empty() && urls_in(l).is_empty()).then_some(l)
            };
            let (title, snippet) = if whole_line {
                (
                    i.checked_sub(1).and_then(neighbour).map(strip_enumerator),
                    neighbour(i + 1),
                )
            } else {
                (None, None)
            };
            out.push(source_chip(&url, title, snippet));
        }
    }
}

/// The pages one web search found, structured form first and prose as the
/// fallback. Bounded and deduped by URL: the row is a trail, not the result
/// set.
///
/// STRUCTURED INCLUDES A TRUNCATED PAYLOAD. Hermes's own `web_search`
/// answers `{success, data: {web: [{title, url, description, position}]}}`
/// pretty-printed (indent 2), and a full page of rows runs past the plugin's
/// 4000-char clamp — so a common case is a payload that `serde_json`
/// refuses. `parse_loose` has already rewound it to its last whole result,
/// which is why prose is the LAST resort rather than the usual one: the prose
/// walk can only label a chip by its host, and the snippet the engine wrote is
/// the part a reader actually uses.
fn sources_from(result_v: &Value, raw: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    walk_sources(result_v, &mut seen, &mut out);
    if out.is_empty() {
        prose_sources(raw, &mut seen, &mut out);
    }
    out
}

/// A tool call's JSON, REPAIRED WHEN IT ARRIVES TRUNCATED.
///
/// Every tool result on this wire has already been cut to 4000 chars by the
/// Hermes plugin (`_MAX_CHARS` in `talaria-events`), so a result that was
/// JSON reaches this crate as a PREFIX of JSON — and `serde_json` refuses a
/// prefix. Without the repair every structured reader below silently falls
/// through to reading prose: a search's ten titled, snippeted results become
/// ten bare hosts, and an edit's change list becomes nothing at all.
///
/// THE REPAIR IS A CUT, NEVER A GUESS. The text is rewound to the end of the
/// last element that CLOSED while something was still open, and the open
/// containers are closed in order. A half-written object is dropped whole;
/// no field is invented, no string is completed.
fn parse_loose(raw: &str) -> Value {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        return v;
    }
    let bytes = raw.as_bytes();
    let mut open: Vec<u8> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    // The byte index just past the last element that closed, with the
    // delimiters that were still open at that moment.
    let mut cut: Option<(usize, Vec<u8>)> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' | b'[' => open.push(b),
            b'}' | b']' => {
                open.pop();
                // Only an element INSIDE something still open is a cut: when
                // the stack empties the value was whole, and a whole value
                // that still failed to parse is not a truncation.
                if !open.is_empty() {
                    cut = Some((i + 1, open.clone()));
                }
            }
            _ => {}
        }
    }
    let Some((at, still_open)) = cut else {
        return Value::Null;
    };
    let mut text = raw[..at].to_string();
    for b in still_open.iter().rev() {
        text.push(if *b == b'{' { '}' } else { ']' });
    }
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// THE TOOLKIT'S OWN NAME FOR A TOOL, however the agent's MCP client spelled
/// it. Hermes registers every MCP tool as `mcp__<server>__<tool>` and reports
/// that registry name to the talaria-events plugin, which posts it to the
/// tool-events door as it is — so a Talaria writer reaches this crate as
/// `mcp__talaria__create_document`, never `create_document`, and every arm of
/// `chips_from_tool` that keys on the toolkit's names missed it: a document
/// an agent created landed no chip, so it never joined the session's files
/// and never opened in the Work pane (seen on a live stack, Hermes v0.22.4).
/// Only the toolkit's own server is stripped — it is always `talaria`
/// (talaria-fleet-render renders it under that key and refuses an authored
/// server of that name) — plus the single-underscore display spelling some
/// Hermes surfaces use. Another server's tool keeps its prefix: a Google
/// Workspace MCP tool is not a toolkit tool, and must not be read as one.
pub fn bare_tool_name(tool: &str) -> &str {
    let tool = tool.trim();
    tool.strip_prefix("mcp__talaria__")
        .or_else(|| tool.strip_prefix("mcp_talaria_"))
        .filter(|rest| !rest.is_empty())
        .unwrap_or(tool)
}

/// THE ANSWER AN MCP TOOL GAVE, out of the envelope Hermes puts it in.
///
/// Hermes' MCP handler returns `json.dumps({"result": text})` for a tool's
/// text content, and that envelope is the `result` the talaria-events plugin
/// posts — so a toolkit writer's `{"artifact": {"id": …}}` reached
/// `chips_from_tool` as one JSON string under `result`, and no arm keyed on an
/// id, a title or a URL found anything (seen on a live stack, Hermes v0.22.4).
/// Unwrapped here: a whole envelope gives back its string; one the plugin's
/// 4000-char clamp cut short gives back as much of the string as arrived,
/// decoded, for `parse_loose` to repair like any other truncated payload.
/// Anything else — a native tool's answer, an object with other keys — is
/// returned as it came.
pub fn mcp_result_text(raw: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    let t = raw.trim_start();
    if let Ok(v) = serde_json::from_str::<Value>(t) {
        if let Some(obj) = v.as_object()
            && obj.len() == 1
            && let Some(Value::String(inner)) = obj.get("result")
        {
            return Cow::Owned(inner.clone());
        }
        return Cow::Borrowed(raw);
    }
    match t
        .strip_prefix("{\"result\": \"")
        .or_else(|| t.strip_prefix("{\"result\":\""))
    {
        Some(rest) => Cow::Owned(json_string_prefix(rest)),
        None => Cow::Borrowed(raw),
    }
}

/// The decoded text of a JSON string body that may have been cut off: up to
/// its closing quote, or to wherever the text ends. An escape cut in half is
/// dropped rather than guessed at.
fn json_string_prefix(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if hex.len() < 4 {
                        break;
                    }
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

/// The toolkit writers whose answer names the Talaria document they made or
/// changed — the only calls whose document chip means the session TOUCHED
/// that file (and so the Work pane lists it, `session_files`).
fn writes_document(tool: &str) -> bool {
    matches!(
        tool,
        "create_document"
            | "create_sheet"
            | "create_page"
            | "save_image_artifact"
            // The import answers `{ artifact, message }`: a NEW Talaria file,
            // and the session that asked for it should see it like any other.
            | "import_drive_file"
            | "update_document"
            // A surgical edit lands the same chip as a full rewrite: what the
            // reader wants is a way into the document that just changed, and
            // how much of it the agent resent is not their problem.
            | "edit_document"
    )
}

/// Chips implied by one completed tool call. Link-producing tools always
/// emit a link chip, even when the result text never pasted the URL.
/// Protected tools emit an approval chip instead of pretending the action
/// already happened.
pub fn chips_from_tool(tool: &str, args: &str, result: &str) -> Vec<Value> {
    let args_v = parse_loose(args);
    let result_v = parse_loose(result);
    let mut out = Vec::new();
    walk_links(&result_v, None, &mut out);
    if let Some(link) = classify_href(result) {
        out.push(link_chip(&link, None));
    }
    // A DOCUMENT A CALL ONLY MENTIONED IS NOT ONE IT OPENED. Every chip that
    // names a Talaria document is a file of the session: the Work pane lists
    // it and brings it forward for the person the turn is for. So a document
    // link in a reader's answer — a `get_document`, a knowledge search whose
    // hits carry `/artifacts?a=…` — must not land one, or grounding an answer
    // on five documents opened five tabs and yanked the pane five times.
    // Writers land theirs below; an existing file opens through
    // `open_in_pane`, whose door lands its own chip.
    if !writes_document(tool) {
        out.retain(|c| c.get("entity").and_then(Value::as_str) != Some("document"));
    }

    let title = first_title(&result_v).or_else(|| str_field(&args_v, "title"));
    let id = first_id(&result_v);
    match tool {
        t if writes_document(t) => {
            if let Some(id) = id.filter(|s| is_id(s)) {
                // Only `edit_document` reports changes — a full rewrite's
                // "change" is the whole body, which is not a diff anyone
                // wants on a chip. THE SAME CHIP THE EDIT ROUTE LANDS: this
                // arm is the fallback for every other producer of an edit
                // (a non-Talaria MCP client, a result replayed), and the
                // shared id makes the two idempotent.
                let changes = (tool == "edit_document")
                    .then(|| result_v.get("changes"))
                    .flatten();
                out.push(document_chip(id, title, changes));
            }
        }
        "create_kb_doc" | "edit_kb_doc" => {
            if let Some(id) = id.filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/knowledge?doc={id}"),
                        entity: "kb",
                        id: id.to_string(),
                        board_id: None,
                    },
                    title,
                ));
            }
        }
        "post_to_channel" => {
            if let Some(id) = str_field(&args_v, "channelId").filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/comms/channel/{id}"),
                        entity: "channel",
                        id: id.to_string(),
                        board_id: None,
                    },
                    str_field(&args_v, "name"),
                ));
            }
        }
        "research" => {
            if let Some(id) = id.filter(|s| is_id(s)) {
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/research/{id}"),
                        entity: "research",
                        id: id.to_string(),
                        board_id: None,
                    },
                    title.or(str_field(&args_v, "question")),
                ));
            }
        }
        // WHERE THE ANSWER CAME FROM. A search that only reaches the reader
        // as prose leaves them with an assertion and no way to check it —
        // the pages are the one part of a search a person can verify, so
        // they land as chips they can click.
        _ if is_web_search(tool) => {
            out.extend(sources_from(&result_v, result));
        }
        _ if google_approval_for(tool).is_some() => {
            // A queued Google write answers `{ pending: { id }, message }`
            // (integrations_google_agent_queue::answer_queued). An immediate
            // one — the agent editing a doc it created — answers without
            // `pending`, and correctly produces no card.
            if let Some(pending) = result_v.get("pending").and_then(|p| str_field(p, "id")) {
                let (kind, fallback) = google_approval_for(tool).expect("guarded by the arm above");
                let summary = str_field(&result_v, "message").unwrap_or(fallback);
                out.push(approval_chip(pending, kind, summary));
            }
        }
        // THE PERSON'S WAY THROUGH A REFUSED TICKET MOVE. An agent refused a
        // column change (out of review without a proven ask, say) leaves a
        // card in the thread; a person who can edit the board approves it and
        // the move runs as THAT person (chips_approvals_id). Matched on the
        // bare tool name, so the card does not depend on how the client
        // prefixed the toolkit's tools.
        //
        // Only a REFUSAL leaves a card — a 403 as the toolkit words it, or the
        // api's own `{error}` — never a move that landed: its answer is the
        // moved ticket, whose title or description may say "cannot", and a
        // stale card approved later would re-apply the move as that person.
        t if bare_tool_name(t) == "triage_ticket" => {
            if is_refusal(result, &result_v) {
                let task = str_field(&args_v, "taskId").unwrap_or("");
                let status = str_field(&args_v, "status").unwrap_or("move");
                if is_id(task) {
                    let external = format!("ticket:{task}:{status}");
                    // The ticket as the agent named it (its ref, usually), so
                    // two refused moves never read as one card.
                    let mut chip = approval_chip(
                        &external,
                        "ticket_move",
                        &format!("Move {task} to {status}"),
                    );
                    if let Some(obj) = chip.as_object_mut() {
                        obj.insert("payload".into(), args_v.clone());
                    }
                    out.push(chip);
                }
            }
        }
        // The board-move twin, refused the same way.
        t if bare_tool_name(t) == "move_ticket" => {
            let task = str_field(&args_v, "taskId").unwrap_or("");
            let board = str_field(&args_v, "boardId").unwrap_or("");
            let preview = args_v.get("preview").and_then(Value::as_bool) == Some(true);
            if is_refusal(result, &result_v) && !preview && is_id(task) && is_id(board) {
                let status = str_field(&args_v, "status");
                let external = format!("board-move:{task}:{board}:{}", status.unwrap_or(""));
                let summary = match status {
                    Some(s) => format!("Move {task} to another board ({s})"),
                    None => format!("Move {task} to another board"),
                };
                let mut chip = approval_chip(&external, "ticket_board_move", &summary);
                if let Some(obj) = chip.as_object_mut() {
                    obj.insert("payload".into(), args_v.clone());
                }
                out.push(chip);
                // WHERE IT WOULD GO, beside the card. A link chip's title is
                // resolved per viewer (`resolve_titles`), so an editor sees the
                // destination board's name before approving, and a reader who
                // cannot see that board gets the bare entity label — the name
                // is never baked into a summary everyone in the thread reads.
                out.push(link_chip(
                    &PlatformLink {
                        href: format!("/boards/{board}"),
                        entity: "board",
                        id: board.to_string(),
                        board_id: None,
                    },
                    None,
                ));
            }
        }
        "request_board_access" => {
            if let Some(board) = str_field(&args_v, "boardId").filter(|s| is_id(s)) {
                let mut chip = approval_chip(
                    &format!("board:{board}"),
                    "board_access",
                    "Request access to a board",
                );
                if let Some(obj) = chip.as_object_mut() {
                    obj.insert("payload".into(), args_v.clone());
                }
                out.push(chip);
            }
        }
        _ => {}
    }
    // THE FILE A GOOGLE TOOL WORKED ON, beside whatever card it produced. It
    // is how the Work pane learns the session touched a Google file (the
    // pane lists the files a conversation's chips name), and how a reader of
    // the transcript gets back to it.
    //
    // The title is the RESULT's only: a queued edit's `title` argument is a
    // rename nobody has approved yet, not what the file is called.
    if let Some(file) = google_file_from(tool, &args_v, &result_v) {
        out.push(google_file_chip(&file, first_title(&result_v)));
    }
    dedupe(out)
}

fn dedupe(chips: Vec<Value>) -> Vec<Value> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for chip in chips {
        let id = chip
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if id.is_empty() || seen.iter().any(|s| s == &id) {
            continue;
        }
        seen.push(id);
        out.push(chip);
    }
    out
}

/// Append chips that are not already on the row. Identity is the chip id, so
/// a retried tool call does not stack a second copy of the same link.
/// Chip kinds whose content is a SNAPSHOT of current state rather than a
/// record of something that happened.
///
/// FIRST-WINS IS THE RIGHT RULE FOR AN EVENT and the wrong one for a
/// snapshot. An approval chip must not duplicate when the same action lands
/// twice, so the merge below keeps the one already there. The agent's plan is
/// the other case: it is rewritten several times inside one turn — "here is
/// the plan", then "step one done, step two active" — and a reader wants the
/// latest, not the first. Keeping the first would pin the stream to a plan the
/// agent had already moved past, which is worse than showing no plan at all,
/// because it reads as current.
///
/// A KIND IS NOT THE WHOLE ANSWER: a document chip carrying `changes` is
/// neither — `merge_doc_changes` below adds both edits' rows rather than
/// choosing between them — so this predicate answers only "replace outright".
fn snapshot_kind(kind: &str) -> bool {
    // `todo`: the plan, rewritten several times inside one turn.
    // `question`: keyed on the question's own words, so a collision means the
    // SAME question asked again — and fresher options are more use to the
    // person than the ones that did not get answered.
    // `subagent`: one chip per delegated child, keyed on the child's session
    // id. The start can only say what the child was asked to do; the stop
    // carries what came back. Keeping the start would leave a finished child
    // reading as still running, which is the one thing the chip exists to say.
    matches!(kind, "todo" | "question" | "subagent")
}

/// The change rows on a chip, or nothing.
fn changes_on(chip: &Value) -> &[Value] {
    chip.get("changes")
        .and_then(Value::as_array)
        .map_or(&[][..], |rows| rows.as_slice())
}

/// A document chip that says what an edit did — the OTHER case first-wins
/// gets wrong, and it is not a snapshot either.
///
/// The chip's id is per DOCUMENT (`link:/artifacts?a={id}`), so two edits to
/// one document in one turn — or a create and then an edit — are one id
/// twice. First-wins drops the second edit's change list; last-wins hides
/// the first. Both edits happened, so both stay: the rows are
/// CONCATENATED, existing first, capped at the same ceiling one result's
/// rows are.
fn merge_doc_changes(existing: &Value, incoming: &Value) -> Value {
    let mut chip = incoming.clone();
    let mut rows: Vec<Value> = changes_on(existing).to_vec();
    // A UNION, not a concatenation: the edit route lands this chip itself
    // and the agent's tool-event round trip lands the SAME rows again a
    // moment later, so a row already on the chip is the same edit reported
    // twice, not a second edit.
    for row in changes_on(incoming) {
        if !rows.contains(row) {
            rows.push(row.clone());
        }
    }
    rows.truncate(MAX_DOC_CHANGES);
    if let Some(obj) = chip.as_object_mut() {
        obj.insert("changes".into(), Value::Array(rows));
    }
    chip
}

pub fn merge_chips(existing: &Value, incoming: &[Value]) -> Value {
    let mut out = match existing {
        Value::Array(items) => items.clone(),
        _ => Vec::new(),
    };
    for chip in incoming {
        let id = chip.get("id").and_then(Value::as_str).unwrap_or("");
        if id.is_empty() {
            continue;
        }
        let at = out
            .iter()
            .position(|c| c.get("id").and_then(Value::as_str) == Some(id));
        match at {
            // Already there: a snapshot is overwritten in PLACE (so it keeps
            // its position in the row rather than jumping to the end), a
            // document chip carrying changes takes the place over with BOTH
            // edits' rows, and any other event is left alone.
            Some(i) => {
                let kind = chip.get("kind").and_then(Value::as_str).unwrap_or("");
                if snapshot_kind(kind) {
                    out[i] = chip.clone();
                } else if !changes_on(chip).is_empty() {
                    out[i] = merge_doc_changes(&out[i], chip);
                }
            }
            None => out.push(chip.clone()),
        }
    }
    Value::Array(out)
}

#[derive(Debug, Clone)]
pub struct Landed {
    pub conversation_id: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: String,
}

#[derive(Debug, Clone)]
pub struct ApprovalRow {
    pub id: String,
    pub kind: String,
    pub summary: String,
    pub payload: Value,
    pub external_id: Option<String>,
    pub status: String,
    pub owner_user_id: Option<String>,
    pub conversation_id: Option<String>,
    pub channel_id: Option<String>,
    pub message_id: Option<String>,
    pub agent_model: Option<String>,
}

/// Where a door's chip lands — or why it cannot.
///
/// THE DOORS ARE CONVERSATION-ADDRESSED. One agent answers several people at
/// once (Hermes runs sessions concurrently, and nothing serialises turns per
/// agent), so "the agent's newest live turn" is the wrong row for every other
/// person's turn — a screenshot taken for Alice would be stamped on Bob's
/// reply. A door that names the conversation lands there and nowhere else;
/// a door that cannot name it is served only while the answer is
/// unambiguous.
#[derive(Debug, Clone)]
pub enum Target {
    Found(Landed),
    /// No live turn and nothing recent: the agent is not speaking anywhere.
    Nowhere,
    /// Several turns and no conversation named — nothing to choose by, and
    /// guessing is the cross-conversation leak this type exists to close.
    Ambiguous {
        turns: usize,
    },
}

impl Target {
    /// The landing row, when there is one.
    pub fn landed(self) -> Option<Landed> {
        match self {
            Target::Found(landed) => Some(landed),
            _ => None,
        }
    }

    /// The sentence a door answers 409 with when the target is an
    /// ambiguity — it names the count and the field that resolves it, because
    /// the reader is a model that can act on exactly that.
    pub fn ambiguity(&self) -> Option<String> {
        match self {
            Target::Ambiguous { turns } => Some(format!(
                "this agent is speaking in {turns} places at once and the request named none of \
                 them — send conversationId (your prompt's conversation line carries it)"
            )),
            _ => None,
        }
    }
}

/// The conversation a door's body addresses, if it addresses one: the
/// `conversationId` the MCP tools pass, else the Hermes `sessionId` the
/// plugin stamps on every event (for a pinned agent that session id IS the
/// conversation id), else a subagent hook's `parentSessionId`. Only a UUID
/// counts: a Hermes session on the completions arm carries the harness's own
/// id (`api-<16 hex>`), which names no conversation and must not be mistaken
/// for an address. The id comes back in its canonical text, the spelling
/// every conversation key and channel is written in.
pub fn conversation_hint(body: &Value) -> Option<String> {
    ["conversationId", "sessionId", "parentSessionId"]
        .iter()
        .filter_map(|key| body.get(*key).and_then(Value::as_str))
        .find_map(|s| talaria_params::uuid_canonical(s.trim()))
}

/// The rule, pure: one candidate is the target, several are an ambiguity,
/// none is "look further". Live rows are asked first and only when nothing
/// is live does the recent tier count — a turn that closed a moment before
/// its chip arrived is still the right row, but only when it is the only one.
fn unaddressed(live: Vec<Landed>, recent: Vec<Landed>) -> Target {
    fn only_one(mut candidates: Vec<Landed>) -> Option<Target> {
        match candidates.len() {
            0 => None,
            1 => Some(Target::Found(candidates.remove(0))),
            turns => Some(Target::Ambiguous { turns }),
        }
    }
    only_one(live)
        .or_else(|| only_one(recent))
        .unwrap_or(Target::Nowhere)
}

/// The row a door addressed by conversation lands on: the reply the agent is
/// streaming in THAT conversation, else its newest reply there within the
/// last 15 minutes (the chip that arrives a beat after the turn closed), else
/// nowhere. The conversation must be this agent's own — an id naming another
/// agent's thread, or no thread, lands nothing rather than falling back to a
/// guess.
///
/// A CHANNEL ID IS AN ADDRESS TOO. A channel turn's prompt names its channel
/// id as the thread to pass under `conversationId` (talaria-channel-replies),
/// so an id that is no conversation of this agent's lands on the agent's own
/// reply in that channel by the same two tiers — and still nowhere when the
/// agent has no such reply there.
async fn addressed_target(
    pg: &PgPool,
    agent: &str,
    conversation_id: &str,
) -> Result<Target, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select m.id::text \
         from messages m join conversations c on c.id = m.conversation_id \
         where c.id = $2::uuid and c.agent_model = $1 and m.role = 'assistant' \
           and ((m.status = 'streaming' and m.streamed_at > now() - interval '15 minutes') \
             or m.created_at > now() - interval '15 minutes') \
         order by (m.status = 'streaming') desc, m.seq desc limit 1",
    )
    .bind(agent)
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    if let Some((message_id,)) = row {
        return Ok(Target::Found(Landed {
            conversation_id: Some(conversation_id.to_string()),
            channel_id: None,
            message_id,
        }));
    }
    let row: Option<(String,)> = sqlx::query_as(
        "select id::text from channel_messages \
         where channel_id = $2::uuid and author_type = 'agent' and author = $1 \
           and created_at > now() - interval '15 minutes' \
         order by (status = 'streaming') desc, seq desc limit 1",
    )
    .bind(agent)
    .bind(conversation_id)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((message_id,)) => Target::Found(Landed {
            conversation_id: None,
            channel_id: Some(conversation_id.to_string()),
            message_id,
        }),
        None => Target::Nowhere,
    })
}

/// The agent's LIVE rows: its streaming conversation replies and streaming
/// channel replies of the last 15 minutes, up to two of each — two so that
/// "several" is observable without counting the table.
async fn live_rows(pg: &PgPool, agent: &str) -> Result<Vec<Landed>, sqlx::Error> {
    let mut live: Vec<Landed> = sqlx::query_as::<_, (String, String)>(
        "select m.id::text, m.conversation_id::text \
         from messages m join conversations c on c.id = m.conversation_id \
         where c.agent_model = $1 and m.role = 'assistant' and m.status = 'streaming' \
           and m.streamed_at > now() - interval '15 minutes' \
         order by m.streamed_at desc limit 2",
    )
    .bind(agent)
    .fetch_all(pg)
    .await?
    .into_iter()
    .map(|(message_id, conversation_id)| Landed {
        conversation_id: Some(conversation_id),
        channel_id: None,
        message_id,
    })
    .collect();
    live.extend(
        sqlx::query_as::<_, (String, String)>(
            "select id::text, channel_id::text from channel_messages \
             where author_type = 'agent' and author = $1 and status = 'streaming' \
               and created_at > now() - interval '15 minutes' \
             order by created_at desc limit 2",
        )
        .bind(agent)
        .fetch_all(pg)
        .await?
        .into_iter()
        .map(|(message_id, channel_id)| Landed {
            conversation_id: None,
            channel_id: Some(channel_id),
            message_id,
        }),
    );
    Ok(live)
}

/// Where an agent's chips land. With a conversation named, that
/// conversation's own turn (`addressed_target`). Without one: the agent's
/// live streaming rows — conversation turns and channel turns alike — when
/// there is exactly one; else its rows of the last 15 minutes regardless of
/// status, when there is exactly one; else `Ambiguous` when several could be
/// meant, and `Nowhere` when none could. Two rows are read per tier rather
/// than one so "several" is observable without counting the table.
pub async fn landing_target(
    pg: &PgPool,
    agent: &str,
    conversation_id: Option<&str>,
) -> Result<Target, sqlx::Error> {
    if let Some(id) = conversation_id.and_then(|s| talaria_params::uuid_canonical(s.trim())) {
        return addressed_target(pg, agent, &id).await;
    }
    let live = live_rows(pg, agent).await?;
    if !live.is_empty() {
        return Ok(unaddressed(live, Vec::new()));
    }
    // Recent: one candidate per THREAD, not per row — a 15-minute chat has
    // several replies in it, and they are one place, not several.
    let mut recent: Vec<Landed> = sqlx::query_as::<_, (String, String)>(
        "select (array_agg(id::text order by created_at desc))[1], channel_id::text \
         from channel_messages \
         where author_type = 'agent' and author = $1 \
           and created_at > now() - interval '15 minutes' \
         group by channel_id order by max(created_at) desc limit 2",
    )
    .bind(agent)
    .fetch_all(pg)
    .await?
    .into_iter()
    .map(|(message_id, channel_id)| Landed {
        conversation_id: None,
        channel_id: Some(channel_id),
        message_id,
    })
    .collect();
    recent.extend(
        sqlx::query_as::<_, (String, String)>(
            "select (array_agg(m.id::text order by m.created_at desc))[1], \
                    m.conversation_id::text \
             from messages m join conversations c on c.id = m.conversation_id \
             where c.agent_model = $1 and m.role = 'assistant' \
               and m.created_at > now() - interval '15 minutes' \
             group by m.conversation_id order by max(m.created_at) desc limit 2",
        )
        .bind(agent)
        .fetch_all(pg)
        .await?
        .into_iter()
        .map(|(message_id, conversation_id)| Landed {
            conversation_id: Some(conversation_id),
            channel_id: None,
            message_id,
        }),
    );
    Ok(unaddressed(Vec::new(), recent))
}

/// The unaddressed resolver as an Option, for the callers that have no
/// conversation to name: an ambiguity is a dropped chip, logged — never a
/// guess at somebody else's thread.
async fn speaking_target(pg: &PgPool, agent: &str) -> Result<Option<Landed>, sqlx::Error> {
    let target = landing_target(pg, agent, None).await?;
    if let Target::Ambiguous { turns } = &target {
        // The crate carries no logger; the fleet transport says its piece
        // the same way.
        eprintln!(
            "[chips] {agent} is speaking in {turns} places and the caller named none — dropped"
        );
    }
    Ok(target.landed())
}

/// The chips the write below is allowed to REPLACE rather than skip: the
/// snapshot kinds, plus a document chip carrying changes — the `merge_chips`
/// rule, stated for the SQL that has to apply it on the row.
fn snapshot_chips(chips: &[Value]) -> Vec<Value> {
    chips
        .iter()
        .filter(|c| {
            snapshot_kind(c.get("kind").and_then(Value::as_str).unwrap_or(""))
                || !changes_on(c).is_empty()
        })
        .cloned()
        .collect()
}

/// `merge_chips` stated as one statement, which is why it is SQL rather than
/// a read, a merge in Rust and a write back: two chip writes land on one row
/// concurrently (a tool result and the plan it just rewrote), and the loser
/// of that race would drop the winner's chip. `$2` is every incoming chip,
/// appended when its id is new; `$3` is the snapshot subset, which REPLACES
/// the chip already sitting at that id, in its place in the row; `$4` is the
/// change-row ceiling. The ordinality keeps that position: a plan that jumped
/// to the end of the row on every rewrite would drag the reader's eye with
/// it.
///
/// THE `changes` ARM IS `merge_doc_changes`, in SQL. When the chip already on
/// the row and the one replacing it both carry change rows — and the chip is
/// not a snapshot kind, which replaces outright, as the Rust does — the
/// replacement carries the UNION of both lists (`group by` the row's value,
/// keeping its first position): the row already there first, then the new
/// edit's rows that are not already on it, capped at `$4`. Replacing outright would hide the first of two edits to
/// one document inside one turn, which is the bug this arm exists for; the
/// two implementations are the contract, so a change to either is a change
/// to both.
///
/// A macro over the table name because the statement has to stay a LITERAL —
/// sqlx refuses a built SQL string by type, and the two tables carrying chips
/// otherwise mean two copies of this to keep in step.
macro_rules! merge_chips_sql {
    ($table:literal) => {
        concat!(
            "update ",
            $table,
            " set chips = ( \
               select coalesce(jsonb_agg(c order by ord), '[]'::jsonb) from ( \
                 select coalesce(( \
                          select case \
                                   when jsonb_typeof(n.c->'changes') = 'array' \
                                    and jsonb_typeof(e.c->'changes') = 'array' \
                                    and (n.c->>'kind') not in ('todo', 'question', 'subagent') \
                                   then jsonb_set(n.c, '{changes}', ( \
                                          select coalesce(jsonb_agg(k.v order by k.ord), '[]'::jsonb) from ( \
                                            select u.v, min(u.ord) as ord from ( \
                                              select p.v, p.ord \
                                                from jsonb_array_elements(e.c->'changes') with ordinality p(v, ord) \
                                              union all \
                                              select q.v, 1000000 + q.ord \
                                                from jsonb_array_elements(n.c->'changes') with ordinality q(v, ord) \
                                            ) u group by u.v order by ord limit $4::bigint \
                                          ) k \
                                        )) \
                                   else n.c \
                                 end \
                            from jsonb_array_elements($3::jsonb) n(c) \
                           where n.c->>'id' = e.c->>'id' limit 1 \
                        ), e.c) as c, e.ord \
                   from jsonb_array_elements(chips) with ordinality e(c, ord) \
                 union all \
                 select n.c, 1000000 + n.ord \
                   from jsonb_array_elements($2::jsonb) with ordinality n(c, ord) \
                  where not exists ( \
                    select 1 from jsonb_array_elements(chips) x(c) \
                     where x.c->>'id' = n.c->>'id' \
                  ) \
               ) s \
             ) where id = $1::uuid"
        )
    };
}

const MERGE_ONTO_CHANNEL: &str = merge_chips_sql!("channel_messages");
const MERGE_ONTO_CONVERSATION: &str = merge_chips_sql!("messages");

async fn write_chips(pg: &PgPool, landed: &Landed, chips: &[Value]) -> Result<(), sqlx::Error> {
    if chips.is_empty() {
        return Ok(());
    }
    let incoming = Value::Array(chips.to_vec());
    let snapshots = Value::Array(snapshot_chips(chips));
    let sql = if landed.channel_id.is_some() {
        MERGE_ONTO_CHANNEL
    } else {
        MERGE_ONTO_CONVERSATION
    };
    sqlx::query(sql)
        .bind(&landed.message_id)
        .bind(&incoming)
        .bind(&snapshots)
        // The one ceiling, bound rather than spelled into the statement: a
        // literal in the SQL is a second number to keep in step with
        // `MAX_DOC_CHANGES`.
        .bind(MAX_DOC_CHANGES as i64)
        .execute(pg)
        .await?;
    Ok(())
}

fn fan(pg: &PgPool, redis: Option<redis::aio::ConnectionManager>, landed: &Landed) {
    let deps = NotifyDeps::publishing(pg.clone(), redis.clone());
    if let Some(id) = &landed.conversation_id {
        fan_conversation_event(deps, id.clone());
    }
    if let Some(id) = &landed.channel_id {
        let realtime = RealtimeDeps::publish_only(redis);
        publish_channel(
            &realtime,
            id,
            &ChannelEvent {
                kind_tag: "message",
                message_id: Some(landed.message_id.clone()),
                seq: None,
                deleted: None,
            },
        );
        fan_channel_event(NotifyDeps::publishing(pg.clone(), None), id.clone());
    }
}

/// Attach chips to the agent's live turn and tell the open thread to refetch.
pub async fn surface_for_agent(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    agent: &str,
    chips: Vec<Value>,
) -> Result<Option<Landed>, sqlx::Error> {
    if chips.is_empty() || agent.is_empty() {
        return Ok(None);
    }
    let Some(landed) = speaking_target(pg, agent).await? else {
        return Ok(None);
    };
    write_chips(pg, &landed, &chips).await?;
    fan(pg, redis, &landed);
    Ok(Some(landed))
}

/// Attach chips to the agent's LIVE turn only — exactly one streaming row,
/// else nowhere — and tell the open thread to refetch.
///
/// FOR A ROUTE THAT LANDS ITS OWN CHIP (a create, an edit), detached from any
/// tool answer and naming no conversation. `surface_for_agent`'s recent tier
/// exists for the tool-events door, whose frame can arrive a beat after the
/// turn closed; a route runs INSIDE the call, so a turn that is not streaming
/// is not the one the call belongs to. With no live row — a cron run, whose
/// reply row is written at delivery; a background job — the recent tier would
/// land the file on whichever single thread the agent spoke in during the
/// last 15 minutes, and since a document chip is a file of the session, it
/// would open and take focus in an unrelated person's Work pane.
pub async fn surface_on_live_turn(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    agent: &str,
    chips: Vec<Value>,
) -> Result<Option<Landed>, sqlx::Error> {
    if chips.is_empty() || agent.is_empty() {
        return Ok(None);
    }
    let target = unaddressed(live_rows(pg, agent).await?, Vec::new());
    if let Target::Ambiguous { turns } = &target {
        eprintln!(
            "[chips] {agent} is streaming in {turns} places and the caller named none — dropped"
        );
    }
    let Some(landed) = target.landed() else {
        return Ok(None);
    };
    write_chips(pg, &landed, &chips).await?;
    fan(pg, redis, &landed);
    Ok(Some(landed))
}

/// Write chips onto a row the caller already resolved (`landing_target`),
/// and tell the open thread to refetch. The door that raises an agent's
/// approval card names its conversation, so it lands there or nowhere.
pub async fn land_chips(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    landed: &Landed,
    chips: &[Value],
) -> Result<(), sqlx::Error> {
    write_chips(pg, landed, chips).await?;
    fan(pg, redis, landed);
    Ok(())
}

/// Set one chip's `status` where it sits on a conversation's message, in its
/// place in the row, and tell the open thread to refetch. An approval chip
/// that a person or a deadline settled reads as what became of it, not as
/// still waiting. The rest of the chip, and every other chip on the row, is
/// left as it was; a row without that chip is left alone.
pub async fn set_chip_status(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    conversation_id: &str,
    message_id: &str,
    chip_id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update messages set chips = ( \
           select coalesce(jsonb_agg( \
                    case when e.c->>'id' = $2 \
                         then e.c || jsonb_build_object('status', $3::text) \
                         else e.c end \
                    order by e.ord), '[]'::jsonb) \
             from jsonb_array_elements(chips) with ordinality e(c, ord) \
         ) \
         where id = $1::uuid and conversation_id = $4::uuid \
           and exists (select 1 from jsonb_array_elements(chips) x(c) where x.c->>'id' = $2)",
    )
    .bind(message_id)
    .bind(chip_id)
    .bind(status)
    .bind(conversation_id)
    .execute(pg)
    .await?;
    fan(
        pg,
        redis,
        &Landed {
            conversation_id: Some(conversation_id.to_string()),
            channel_id: None,
            message_id: message_id.to_string(),
        },
    );
    Ok(())
}

/// Idempotent pending row. `external_id` is the google pending id, or a
/// stable key for a ticket move (`ticket:{id}:{status}`). A retry returns
/// the row that is already waiting. The arguments are the row's columns.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_approval(
    pg: &PgPool,
    kind: &str,
    summary: &str,
    payload: &Value,
    external_id: &str,
    agent: &str,
    owner_user_id: Option<&str>,
    landed: Option<&Landed>,
) -> Result<String, sqlx::Error> {
    let existing: Option<String> = sqlx::query_scalar(
        "select id::text from chat_approvals where external_id = $1 and status = 'pending' limit 1",
    )
    .bind(external_id)
    .fetch_optional(pg)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let (conversation_id, channel_id, message_id) = match landed {
        Some(l) => (
            l.conversation_id.as_deref(),
            l.channel_id.as_deref(),
            Some(l.message_id.as_str()),
        ),
        None => (None, None, None),
    };
    let (id,): (String,) = sqlx::query_as(
        "insert into chat_approvals \
           (kind, summary, payload, external_id, agent_model, owner_user_id, conversation_id, channel_id, message_id) \
         values ($1, $2, $3, $4, $5, $6::uuid, $7::uuid, $8::uuid, $9::uuid) \
         returning id::text",
    )
    .bind(kind)
    .bind(summary)
    .bind(payload)
    .bind(external_id)
    .bind(agent)
    .bind(owner_user_id)
    .bind(conversation_id)
    .bind(channel_id)
    .bind(message_id)
    .fetch_one(pg)
    .await?;
    Ok(id)
}

pub async fn load_approval(pg: &PgPool, id: &str) -> Result<Option<ApprovalRow>, sqlx::Error> {
    let row: Option<(
        String,
        String,
        String,
        Value,
        Option<String>,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "select id::text, kind, summary, payload, external_id, status, owner_user_id::text, \
                conversation_id::text, channel_id::text, message_id::text, agent_model \
         from chat_approvals where id = $1::uuid or external_id = $1 limit 1",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| ApprovalRow {
        id: r.0,
        kind: r.1,
        summary: r.2,
        payload: r.3,
        external_id: r.4,
        status: r.5,
        owner_user_id: r.6,
        conversation_id: r.7,
        channel_id: r.8,
        message_id: r.9,
        agent_model: r.10,
    }))
}

pub async fn mark_approval(
    pg: &PgPool,
    id: &str,
    status: &str,
    decided_by: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update chat_approvals set status = $2, decided_by = $3::uuid, decided_at = now() where id = $1::uuid",
    )
    .bind(id)
    .bind(status)
    .bind(decided_by)
    .execute(pg)
    .await?;
    Ok(())
}

// ── Bounded unlocks ──────────────────────────────────────────────────────────
//
// An approval leaves a GRANT on the conversation, not a standing permission.
// `conversations.unlocked_tools` is a jsonb array of
//   { "tool": "draft_email", "expiresAt": <epoch ms>, "usesLeft": 1,
//     "recipients": ["bob@example.com"] | null }
// — one per tool, written by `merge_unlocked` when the person approves, and
// CONSUMED (one use, under a row lock) by `tool_unlocked` when the agent's
// next call of that tool arrives. A grant lasts `UNLOCK_TTL_MS`, covers
// `UNLOCK_USES` later calls, and — where the approved kind names people — only
// a call whose recipients are a subset of the ones the person approved. A
// bare string in the array (the pre-bounded shape) grants nothing and is
// dropped on the next merge.

/// How long one approval stays usable.
pub const UNLOCK_TTL_MS: i64 = 30 * 60 * 1000;
/// How many later calls one approval covers.
pub const UNLOCK_USES: i64 = 1;

/// Lower-cased, trimmed, deduped addresses out of free-form recipient text:
/// `Bob <bob@x.com>, carol@x.com; dan@x.com` → the three bare addresses.
pub fn normalize_recipients<'a>(raw: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = raw
        .into_iter()
        .flat_map(|s| s.split([',', ';']))
        .filter_map(|piece| {
            let piece = piece.trim();
            if piece.is_empty() {
                return None;
            }
            let addr = match (piece.rfind('<'), piece.ends_with('>')) {
                (Some(open), true) => &piece[open + 1..piece.len() - 1],
                _ => piece,
            };
            let addr = addr.trim().to_ascii_lowercase();
            (!addr.is_empty()).then_some(addr)
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The people an approval kind binds its unlock to, read from the approved
/// action's payload. `None` for a kind that names nobody — its grant is not
/// recipient-bound.
pub fn approval_recipients(kind: &str, payload: &Value) -> Option<Vec<String>> {
    let s = |k: &str| payload.get(k).and_then(Value::as_str).unwrap_or("");
    match kind {
        "gmail_send" => Some(normalize_recipients([s("to"), s("cc"), s("bcc")])),
        "calendar_create" | "meeting_create" => Some(normalize_recipients(
            payload
                .get("attendees")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default(),
        )),
        // A held Google Workspace MCP call: the people and the Chat space its
        // arguments address, so its `<server>/<tool>` grant (D7) covers a
        // later call to no one else, as `gmail_send`'s does.
        "mcp_call" => Some(mcp_call_addressees(
            payload.get("arguments").unwrap_or(&Value::Null),
        )),
        _ => None,
    }
}

/// The prefix a Chat space carries among an `mcp_call` grant's addressees.
/// Compared exactly, never ignoring case: Chat's ids are case-sensitive.
const CHAT_ADDRESSEE: &str = "chat:";

/// Everyone a Google Workspace MCP call's arguments write to or notify, as
/// Google's catalogs (fixtures/tools-list, captured 2026-11-17) spell them,
/// at the top level of the arguments:
///   · Gmail `create_draft`: `to`, `cc`, `bcc`;
///   · Calendar `create_event` / `update_event`: `attendees` and
///     `addedAttendees` (each an Attendee's `email`, or a bare address),
///     `attendeeEmails`, `addedAttendeeEmails`, `removedAttendeeEmails`;
///   · Chat `send_message`: the space — `conversationId`, and the one a
///     `threadId` names — as `chat:spaces/<id>`.
///
/// Addresses come back lower-cased and deduped (`normalize_recipients`);
/// a call that names nobody yields an empty list, so its grant covers a
/// later call that names nobody either. Pure.
pub fn mcp_call_addressees(arguments: &Value) -> Vec<String> {
    let mut emails: Vec<&str> = Vec::new();
    for key in [
        "to",
        "cc",
        "bcc",
        "attendees",
        "addedAttendees",
        "attendeeEmails",
        "addedAttendeeEmails",
        "removedAttendeeEmails",
    ] {
        match arguments.get(key) {
            Some(Value::String(s)) => emails.push(s),
            // A bare address, or an Attendee object's `email`.
            Some(Value::Array(items)) => {
                emails.extend(items.iter().filter_map(|item| match item {
                    Value::String(s) => Some(s.as_str()),
                    Value::Object(o) => o.get("email").and_then(Value::as_str),
                    _ => None,
                }))
            }
            _ => {}
        }
    }
    let mut out = normalize_recipients(emails);
    for key in ["conversationId", "threadId"] {
        if let Some(space) = arguments
            .get(key)
            .and_then(Value::as_str)
            .and_then(chat_space)
        {
            out.push(format!("{CHAT_ADDRESSEE}{space}"));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The space a Chat conversation or thread name sits in: `spaces/<id>` out
/// of `spaces/<id>` or `spaces/<id>/threads/<id>`; anything else as written,
/// trimmed. None when empty.
fn chat_space(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let mut parts = name.split('/');
    Some(match (parts.next(), parts.next()) {
        (Some("spaces"), Some(id)) if !id.is_empty() => format!("spaces/{id}"),
        _ => name.to_string(),
    })
}

/// Is a call's addressee one the person approved? Addresses ignore case; a
/// Chat space (`chat:…`) matches exactly.
fn same_addressee(approved: &str, called: &str) -> bool {
    if approved.starts_with(CHAT_ADDRESSEE) || called.starts_with(CHAT_ADDRESSEE) {
        approved == called
    } else {
        approved.eq_ignore_ascii_case(called)
    }
}

/// The key an `mcp_call` approval's unlock is written and spent under: the
/// server's registry name and the tool, `google-calendar/create_event`. A
/// grant for one tool never covers another, on that server or any other.
pub fn mcp_unlock_key(server: &str, tool: &str) -> String {
    format!("{server}/{tool}")
}

/// Leave the bounded grant an approved `mcp_call` earns on the conversation:
/// one more call of the same tool on the same server, within
/// `UNLOCK_TTL_MS`, to no one the approved call did not address
/// (`mcp_call_addressees`), runs without a new card (owner decision D7, as
/// `draft_email`'s grant is bound to its recipients). ONLY for a personal
/// principal's row — an org-account or agent-identity call needs an admin
/// every time, and never rides a conversation unlock — and only once the row
/// really executed. Returns the key granted, or None when nothing was.
pub async fn merge_mcp_unlock(
    pg: &PgPool,
    conversation_id: &str,
    google_action_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String, Value, String, String)> = sqlx::query_as(
        "select kind, payload, principal_kind, status from google_pending_actions \
         where id::text = $1",
    )
    .bind(google_action_id)
    .fetch_optional(pg)
    .await?;
    let Some((key, addressees)) = row.and_then(|(kind, payload, principal, status)| {
        let key = mcp_unlock_for(&kind, &payload, &principal, &status)?;
        Some((
            key,
            approval_recipients(&kind, &payload).unwrap_or_default(),
        ))
    }) else {
        return Ok(None);
    };
    let now_ms = talaria_now_ms();
    let mut tx = pg.begin().await?;
    let stored: Option<Value> = sqlx::query_scalar(
        "select unlocked_tools from conversations where id = $1::uuid for update",
    )
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    sqlx::query("update conversations set unlocked_tools = $2 where id = $1::uuid")
        .bind(conversation_id)
        .bind(merge_grants(
            &stored,
            &[unlock_grant(&key, Some(&addressees), now_ms)],
            now_ms,
        ))
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Some(key))
}

/// The unlock key an approved `mcp_call` row earns, or None: another kind, a
/// principal that is not the owner's own, a row that did not execute, or a
/// payload naming no server or tool. Pure.
pub fn mcp_unlock_for(
    kind: &str,
    payload: &Value,
    principal: &str,
    status: &str,
) -> Option<String> {
    if kind != "mcp_call" || principal != "owner" || status != "executed" {
        return None;
    }
    let s = |k: &str| {
        payload
            .get(k)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
    };
    Some(mcp_unlock_key(s("server")?, s("tool")?))
}

/// One grant, as stored.
pub fn unlock_grant(tool: &str, recipients: Option<&[String]>, now_ms: i64) -> Value {
    json!({
        "tool": tool,
        "expiresAt": now_ms + UNLOCK_TTL_MS,
        "usesLeft": UNLOCK_USES,
        "recipients": recipients,
    })
}

/// Whether a stored grant covers a call of `tool` to `recipients` at
/// `now_ms`: unexpired, uses left, and — when bound — every recipient of
/// the call among the approved ones. A legacy string entry never matches.
pub fn grant_allows(grant: &Value, tool: &str, recipients: &[String], now_ms: i64) -> bool {
    let Some(obj) = grant.as_object() else {
        return false;
    };
    if obj.get("tool").and_then(Value::as_str) != Some(tool) {
        return false;
    }
    let expires = obj.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
    let uses = obj.get("usesLeft").and_then(Value::as_i64).unwrap_or(0);
    if expires <= now_ms || uses <= 0 {
        return false;
    }
    match obj.get("recipients") {
        None | Some(Value::Null) => true,
        Some(bound) => {
            let approved: Vec<&str> = bound
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            recipients
                .iter()
                .all(|r| approved.iter().any(|a| same_addressee(a, r)))
        }
    }
}

/// The array after one use of the first matching grant — `None` when no
/// grant covers the call, so nothing is written. Expired and legacy entries
/// fall out on the way through; a grant spent to zero is dropped too.
pub fn consume_grant(
    stored: &Value,
    tool: &str,
    recipients: &[String],
    now_ms: i64,
) -> Option<Value> {
    let entries = stored.as_array()?;
    let hit = entries
        .iter()
        .position(|g| grant_allows(g, tool, recipients, now_ms))?;
    let next: Vec<Value> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, g)| {
            let obj = g.as_object()?;
            let expires = obj.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
            let mut uses = obj.get("usesLeft").and_then(Value::as_i64).unwrap_or(0);
            if i == hit {
                uses -= 1;
            }
            if expires <= now_ms || uses <= 0 {
                return None;
            }
            let mut kept = obj.clone();
            kept.insert("usesLeft".into(), json!(uses));
            Some(Value::Object(kept))
        })
        .collect();
    Some(Value::Array(next))
}

/// The merged array: every live grant for another tool kept, the incoming
/// grants replacing any for the same tool, expired and legacy entries gone.
pub fn merge_grants(stored: &Value, incoming: &[Value], now_ms: i64) -> Value {
    let incoming_tools: Vec<&str> = incoming
        .iter()
        .filter_map(|g| g.get("tool").and_then(Value::as_str))
        .collect();
    let mut next: Vec<Value> = stored
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|g| {
                    let Some(obj) = g.as_object() else {
                        return false;
                    };
                    let tool = obj.get("tool").and_then(Value::as_str).unwrap_or("");
                    let expires = obj.get("expiresAt").and_then(Value::as_i64).unwrap_or(0);
                    let uses = obj.get("usesLeft").and_then(Value::as_i64).unwrap_or(0);
                    expires > now_ms && uses > 0 && !incoming_tools.contains(&tool)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    next.extend(incoming.iter().cloned());
    Value::Array(next)
}

/// Leave a bounded grant for `tools` on the conversation, bound to the
/// people the approval just decided named. The approval row is the newest
/// APPROVED `chat_approvals` row in this conversation whose kind unlocks
/// these tools (the chip route marks it approved before calling this); the
/// recipients come from the Google pending action it executed. A tool whose
/// approval cannot be found gets no grant — an unlock nobody approved is
/// the thing this shape exists to prevent.
pub async fn merge_unlocked(
    pg: &PgPool,
    conversation_id: &str,
    tools: &[&str],
) -> Result<(), sqlx::Error> {
    if tools.is_empty() {
        return Ok(());
    }
    let now_ms = talaria_now_ms();
    let mut incoming: Vec<Value> = Vec::with_capacity(tools.len());
    for tool in tools {
        let kinds: Vec<&str> = KINDS_WITH_UNLOCKS
            .iter()
            .copied()
            .filter(|k| tools_unlocked(k).contains(tool))
            .collect();
        let row: Option<(String, Option<Value>)> = sqlx::query_as(
            "select a.kind, g.payload \
             from chat_approvals a \
             left join google_pending_actions g on g.id::text = a.external_id \
             where a.conversation_id = $1::uuid and a.status = 'approved' \
               and a.kind = any($2) \
             order by a.decided_at desc nulls last limit 1",
        )
        .bind(conversation_id)
        .bind(&kinds)
        .fetch_optional(pg)
        .await?;
        let Some((kind, payload)) = row else {
            continue;
        };
        let recipients = approval_recipients(&kind, &payload.unwrap_or(Value::Null));
        incoming.push(unlock_grant(tool, recipients.as_deref(), now_ms));
    }
    if incoming.is_empty() {
        return Ok(());
    }
    let mut tx = pg.begin().await?;
    let stored: Option<Value> = sqlx::query_scalar(
        "select unlocked_tools from conversations where id = $1::uuid for update",
    )
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(());
    };
    sqlx::query("update conversations set unlocked_tools = $2 where id = $1::uuid")
        .bind(conversation_id)
        .bind(merge_grants(&stored, &incoming, now_ms))
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// Spend one use of the conversation's grant for `tool`, for a call to
/// `recipients`. True exactly once per use: the row is locked for the
/// read-decide-write, so two concurrent calls cannot both ride one approval.
pub async fn tool_unlocked(
    pg: &PgPool,
    conversation_id: &str,
    tool: &str,
    recipients: &[String],
) -> Result<bool, sqlx::Error> {
    let now_ms = talaria_now_ms();
    let mut tx = pg.begin().await?;
    let stored: Option<Value> = sqlx::query_scalar(
        "select unlocked_tools from conversations where id = $1::uuid for update",
    )
    .bind(conversation_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(stored) = stored else {
        return Ok(false);
    };
    let Some(next) = consume_grant(&stored, tool, recipients, now_ms) else {
        return Ok(false);
    };
    sqlx::query("update conversations set unlocked_tools = $2 where id = $1::uuid")
        .bind(conversation_id)
        .bind(next)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Every approval kind that unlocks a tool — the reverse index of
/// `tools_unlocked`, so a grant can find the approval that earned it.
const KINDS_WITH_UNLOCKS: &[&str] = &[
    "gmail_send",
    "calendar_create",
    "calendar_update",
    "calendar_cancel",
    "meeting_create",
    "doc_update",
    "sheet_update",
    "slides_update",
    "drive_move",
    "drive_rename",
    "board_access",
];

fn talaria_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The person in the agent's live conversation, when one is streaming (an
/// assistant reply mid-turn in the last fifteen minutes). None when the agent
/// is not mid-turn — a cron, a finished turn — which is not a mismatch. The
/// confirm-send doors (the REST drafts, the MCP gateway's hold) refuse a
/// personal action queued while someone other than its owner is talking.
pub async fn streaming_conversation_user(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select c.user_id::text \
         from messages m join conversations c on c.id = m.conversation_id \
         where c.agent_model = $1 and m.role = 'assistant' and m.status = 'streaming' \
           and m.streamed_at > now() - interval '15 minutes' \
         order by m.streamed_at desc limit 1",
    )
    .bind(agent_model)
    .fetch_optional(pg)
    .await
}

// ── The live turn: whom an agent is answering right now ─────────────────────
//
// `landing_target` answers WHERE an agent's turn is; this answers WHO it is
// answering, strictly: only a reply streaming now counts (no recent tier — a
// finished turn answers nobody), and only the human messages that turn
// answers come back. A door that lets an agent act on a person's word
// (talaria-tasks `asked`) asks this and nothing looser, so a cron, a work
// session or yesterday's chat cannot be cited as somebody's request.
//
// A channel reply answers exactly the message it was started for: the
// trigger stamps that message's id on the streaming row
// (`answers_message_id`, talaria-channels `insert_streaming_reply`). Nothing
// is inferred from what sits before the row — a person who posts while the
// reply is still being decided (a ticket room's relevance judgment) is not
// the one it answers. And a tool call names only the channel, so two replies
// this agent is streaming in one channel cannot be told apart: that is
// `Several`, never the newer one's guess.
//
// NOT THE SAME QUESTION AS `talaria_conversations::turn_requester`, which
// answers whose Work pane a file landed on a row belongs in — for any row,
// finished ones too: the newest person's message strictly below that row, a
// queued one never, a chip click counting as the person who clicked. Both
// read `messages`; the policies differ on purpose, and each names the other.

// The live window is the fifteen minutes every resolver in this file uses.
// Conversation replies refresh `streamed_at` every ten seconds while they
// write, so the window only bounds a dead row. Channel rows carry no beat:
// their window runs from `created_at`, which caps a channel turn at fifteen
// minutes (documented in docs/user/boards.md).

/// One human message a live turn answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnMessage {
    pub id: String,
    /// The person the row can be held to: a conversation row's
    /// `author_user_id`; a channel row's author when it is an email that
    /// names exactly one user. None is "nobody this row proves".
    pub user_id: Option<String>,
    /// How a record names them: the email, else the name.
    pub label: String,
    pub content: String,
    /// The words were the agent's — an ask option or a tool chip the person
    /// clicked, or a reply the person sent from a draft. Chosen, not written.
    pub drafted: bool,
}

/// Where the live turn is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnPlace {
    Conversation,
    /// A channel, with the task it is the discussion room of, if any.
    Channel {
        name: String,
        task_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveTurn {
    /// The thread's id in canonical text.
    pub thread_id: String,
    pub place: TurnPlace,
    /// The agent's streaming reply.
    pub reply_id: String,
    /// The human messages this turn answers, oldest first. In a conversation:
    /// the user rows after the last finished reply, including any queued
    /// behind (or steered into) the streaming one. In a channel: the one
    /// person's message the reply's row says it answers — none when the row
    /// carries no stamp, or the stamped row is no person's.
    pub messages: Vec<TurnMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnLookup {
    /// The id names no thread of this agent's: not a UUID, another agent's
    /// conversation, or a channel this agent has never spoken in.
    NotYours,
    /// This agent's thread, but no reply of its own is streaming there now.
    NotLive,
    /// More than one reply of this agent's is streaming in that channel, and
    /// the id alone cannot say which turn is asking.
    Several,
    Live(LiveTurn),
}

/// The live turn `thread_id` names for `agent`: a conversation of its own, or
/// a channel it is replying in. See the section comment above.
pub async fn live_turn(
    pg: &PgPool,
    agent: &str,
    thread_id: &str,
) -> Result<TurnLookup, sqlx::Error> {
    let Some(id) = talaria_params::uuid_canonical(thread_id.trim()) else {
        return Ok(TurnLookup::NotYours);
    };
    let owned: Option<(String,)> = sqlx::query_as(
        "select id::text from conversations where id = $1::uuid and agent_model = $2",
    )
    .bind(&id)
    .bind(agent)
    .fetch_optional(pg)
    .await?;
    if owned.is_some() {
        return conversation_turn(pg, &id).await;
    }
    channel_turn(pg, agent, &id).await
}

async fn conversation_turn(pg: &PgPool, id: &str) -> Result<TurnLookup, sqlx::Error> {
    let reply: Option<(String, i32)> = sqlx::query_as(
        "select id::text, seq from messages \
         where conversation_id = $1::uuid and role = 'assistant' and status = 'streaming' \
           and streamed_at > now() - interval '15 minutes' \
         order by seq desc limit 1",
    )
    .bind(id)
    .fetch_optional(pg)
    .await?;
    let Some((reply_id, reply_seq)) = reply else {
        return Ok(TurnLookup::NotLive);
    };
    // `author_user_id` is the session that sent the row; a cron's
    // responsible-user stamp sits on ASSISTANT rows, so it is never here.
    let rows: Vec<(String, Option<String>, Option<String>, String, bool)> = sqlx::query_as(
        "select m.id::text, m.author_user_id::text, coalesce(u.email, u.name), m.content, \
                coalesce(m.metadata->>'drafted', '') = 'true' \
         from messages m left join users u on u.id = m.author_user_id \
         where m.conversation_id = $1::uuid and m.role = 'user' \
           and m.seq > coalesce((select max(p.seq) from messages p \
                                 where p.conversation_id = $1::uuid and p.role = 'assistant' \
                                   and p.status <> 'streaming' and p.seq < $2), 0) \
         order by m.seq",
    )
    .bind(id)
    .bind(reply_seq)
    .fetch_all(pg)
    .await?;
    Ok(TurnLookup::Live(LiveTurn {
        thread_id: id.to_string(),
        place: TurnPlace::Conversation,
        reply_id,
        messages: rows
            .into_iter()
            .map(|(id, user_id, label, content, drafted)| TurnMessage {
                id,
                label: label.unwrap_or_else(|| "a person".into()),
                user_id,
                content,
                drafted,
            })
            .collect(),
    }))
}

async fn channel_turn(pg: &PgPool, agent: &str, id: &str) -> Result<TurnLookup, sqlx::Error> {
    type ReplyRow = (String, String, Option<String>, Option<String>);
    // Two rows are enough to know the call is ambiguous.
    let replies: Vec<ReplyRow> = sqlx::query_as(
        "select m.id::text, c.name, c.task_id::text, m.answers_message_id::text \
         from channel_messages m join channels c on c.id = m.channel_id \
         where m.channel_id = $1::uuid and m.author_type = 'agent' and m.author = $2 \
           and m.status = 'streaming' and m.created_at > now() - interval '15 minutes' \
         order by m.seq desc limit 2",
    )
    .bind(id)
    .bind(agent)
    .fetch_all(pg)
    .await?;
    let (reply_id, name, task_id, answers) = match replies.as_slice() {
        [] => {
            let spoke: Option<(i32,)> = sqlx::query_as(
                "select 1 from channel_messages \
                 where channel_id = $1::uuid and author_type = 'agent' and author = $2 limit 1",
            )
            .bind(id)
            .bind(agent)
            .fetch_optional(pg)
            .await?;
            return Ok(if spoke.is_some() {
                TurnLookup::NotLive
            } else {
                TurnLookup::NotYours
            });
        }
        [one] => one.clone(),
        _ => return Ok(TurnLookup::Several),
    };
    // The message the reply answers: the one its row was stamped with, if it
    // is a person's. The calling agent is never "a person", whatever its row
    // was typed as.
    let asked: Option<(String, String, String, bool)> = match answers.as_deref() {
        Some(message) => {
            sqlx::query_as(
                "select id::text, author, content, drafted from channel_messages \
                 where id = $1::uuid and channel_id = $2::uuid \
                   and author_type = 'user' and author <> $3",
            )
            .bind(message)
            .bind(id)
            .bind(agent)
            .fetch_optional(pg)
            .await?
        }
        None => None,
    };
    let mut messages = Vec::new();
    if let Some((message_id, author, content, drafted)) = asked {
        // A channel row stores the sender's email, else their name. Only an
        // email naming exactly one user is a person this row proves — a
        // shared display name resolves to nobody, which refuses safely.
        let users: Vec<(String,)> =
            sqlx::query_as("select id::text from users where lower(email) = lower($1) limit 2")
                .bind(&author)
                .fetch_all(pg)
                .await?;
        let user_id = match users.as_slice() {
            [(one,)] => Some(one.clone()),
            _ => None,
        };
        messages.push(TurnMessage {
            id: message_id,
            user_id,
            label: author,
            content,
            drafted,
        });
    }
    Ok(TurnLookup::Live(LiveTurn {
        thread_id: id.to_string(),
        place: TurnPlace::Channel { name, task_id },
        reply_id,
        messages,
    }))
}

/// The conversation an agent is speaking in, for callers with no id of
/// their own: served only while exactly one turn could be meant (the
/// `landing_target` rule); an ambiguity is None, never a guess.
pub async fn live_conversation_id(pg: &PgPool, agent: &str) -> Result<Option<String>, sqlx::Error> {
    Ok(speaking_target(pg, agent)
        .await?
        .and_then(|l| l.conversation_id))
}

/// Rewrite approval chips so their actionId is the chat_approvals row, then
/// land them. Callers that already queued a google action pass that id as
/// the chip's actionId; this function makes the decide endpoint one door.
/// The unaddressed form: an ambiguity is a dropped landing (see
/// `speaking_target`); doors that can name their conversation use
/// `surface_tool_chips_in`.
pub async fn surface_tool_chips(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    agent: &str,
    owner_user_id: Option<&str>,
    chips: Vec<Value>,
) -> Result<Option<Landed>, sqlx::Error> {
    let target = surface_tool_chips_in(pg, redis, agent, owner_user_id, None, chips).await?;
    if let Target::Ambiguous { turns } = &target {
        eprintln!(
            "[chips] {agent} is speaking in {turns} places and the caller named none — dropped"
        );
    }
    Ok(target.landed())
}

/// `surface_tool_chips`, addressed: `conversation_id` is where the chips
/// land when the caller knows it, and the answer says `Ambiguous` instead of
/// guessing when it does not and several turns could be meant. The chips
/// are written only on `Found`; the approval rows are ensured either way,
/// because an approval that was queued exists whether or not its chip has a
/// row to sit on.
pub async fn surface_tool_chips_in(
    pg: &PgPool,
    redis: Option<redis::aio::ConnectionManager>,
    agent: &str,
    owner_user_id: Option<&str>,
    conversation_id: Option<&str>,
    mut chips: Vec<Value>,
) -> Result<Target, sqlx::Error> {
    if chips.is_empty() {
        return Ok(Target::Nowhere);
    }
    let target = landing_target(pg, agent, conversation_id).await?;
    let landed = match &target {
        Target::Found(landed) => Some(landed.clone()),
        _ => None,
    };
    for chip in &mut chips {
        if chip.get("kind").and_then(Value::as_str) != Some("approval") {
            continue;
        }
        let kind = chip
            .get("actionKind")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let summary = chip
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let external = chip
            .get("actionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if external.is_empty() {
            continue;
        }
        let payload = chip
            .get("payload")
            .cloned()
            .unwrap_or_else(|| json!({ "externalId": external }));
        let id = ensure_approval(
            pg,
            &kind,
            &summary,
            &payload,
            &external,
            agent,
            owner_user_id,
            landed.as_ref(),
        )
        .await?;
        if let Some(obj) = chip.as_object_mut() {
            obj.insert("actionId".into(), json!(id));
            obj.insert("id".into(), json!(format!("approval:{id}")));
        }
    }
    let Some(landed) = landed else {
        return Ok(target);
    };
    write_chips(pg, &landed, &chips).await?;
    fan(pg, redis, &landed);
    Ok(Target::Found(landed))
}

#[derive(Debug, Clone)]
pub struct ResolvedTitle {
    pub href: String,
    pub entity: &'static str,
    pub title: Option<String>,
}

/// Titles the caller may see. A miss is an absent title, never another
/// person's private name — the chip falls back to the entity label.
pub async fn resolve_titles(
    pg: &PgPool,
    user_id: &str,
    hrefs: &[String],
) -> Result<Vec<ResolvedTitle>, sqlx::Error> {
    let mut out = Vec::new();
    for href in hrefs {
        let Some(link) = classify_href(href) else {
            continue;
        };
        let title = lookup_title(pg, user_id, &link).await?;
        out.push(ResolvedTitle {
            href: link.href,
            entity: link.entity,
            title,
        });
    }
    Ok(out)
}

async fn lookup_title(
    pg: &PgPool,
    user_id: &str,
    link: &PlatformLink,
) -> Result<Option<String>, sqlx::Error> {
    // Fixture-shaped ids (b-1) are not rows. Only a uuid is worth a lookup.
    if !talaria_params::uuid_shaped(&link.id) {
        return Ok(None);
    }
    let title: Option<String> = match link.entity {
        "board" => {
            sqlx::query_scalar(
                "select b.name from boards b \
                 where b.id = $1::uuid and (b.owner_id = $2::uuid or exists( \
                   select 1 from board_members m where m.board_id = b.id and m.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "ticket" => {
            sqlx::query_scalar(
                "select t.title from tasks t join boards b on b.id = t.board_id \
                 where t.id = $1::uuid and (b.owner_id = $2::uuid or exists( \
                   select 1 from board_members m where m.board_id = b.id and m.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "kb" => {
            sqlx::query_scalar(
                "select title from kb_docs where id = $1::uuid and visibility in ('org', 'public')",
            )
            .bind(&link.id)
            .fetch_optional(pg)
            .await?
        }
        "document" => {
            sqlx::query_scalar(
                "select title from artifacts \
                 where id = $1::uuid and (owner_user_id = $2::uuid or visibility in ('org', 'public', 'workspace'))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "channel" => {
            sqlx::query_scalar(
                "select c.name from channels c \
                 where c.id = $1::uuid and exists( \
                   select 1 from channel_members m where m.channel_id = c.id and m.user_id = $2::uuid)",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "plan" => {
            sqlx::query_scalar(
                "select coalesce(title, 'Plan') from conversations \
                 where id = $1::uuid and kind = 'plan' and (user_id = $2::uuid or exists( \
                   select 1 from conversation_members cm where cm.conversation_id = conversations.id and cm.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        "research" => {
            sqlx::query_scalar(
                "select question from research_runs \
                 where id = $1::uuid and (owner_user_id = $2::uuid or exists( \
                   select 1 from research_members rm where rm.run_id = research_runs.id and rm.user_id = $2::uuid))",
            )
            .bind(&link.id)
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        _ => None,
    };
    Ok(title.filter(|t| !t.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where an unaddressed door lands — the rule that keeps one agent's
    /// several conversations apart.
    mod landing {
        use super::*;

        fn row(id: &str) -> Landed {
            Landed {
                conversation_id: Some(format!("conv-{id}")),
                channel_id: None,
                message_id: id.to_string(),
            }
        }

        fn found(target: Target) -> String {
            match target {
                Target::Found(l) => l.message_id,
                other => panic!("expected a landing, got {other:?}"),
            }
        }

        /// Exactly one live turn is the turn; the recent tier is not even
        /// looked at while something is live.
        #[test]
        fn one_live_row_is_the_target() {
            assert_eq!(found(unaddressed(vec![row("a")], vec![row("old")])), "a");
        }

        /// Two live turns and no id: an ambiguity, not the newest one —
        /// that guess is the cross-conversation leak.
        #[test]
        fn several_live_rows_are_an_ambiguity_whatever_the_recent_tier_says() {
            match unaddressed(vec![row("a"), row("b")], vec![row("old")]) {
                Target::Ambiguous { turns } => assert_eq!(turns, 2),
                other => panic!("{other:?}"),
            }
        }

        /// Nothing live: the one recent thread catches the chip that arrived
        /// a beat after its turn closed; several recent threads do not.
        #[test]
        fn the_recent_tier_counts_only_when_nothing_is_live_and_by_the_same_rule() {
            assert_eq!(found(unaddressed(Vec::new(), vec![row("old")])), "old");
            assert!(matches!(
                unaddressed(Vec::new(), vec![row("x"), row("y")]),
                Target::Ambiguous { turns: 2 }
            ));
            assert!(matches!(
                unaddressed(Vec::new(), Vec::new()),
                Target::Nowhere
            ));
        }

        /// The ambiguity names the count and the field that resolves it; a
        /// landing and a nowhere have no sentence.
        #[test]
        fn only_an_ambiguity_has_a_refusal_sentence() {
            let said = Target::Ambiguous { turns: 3 }
                .ambiguity()
                .expect("a sentence");
            assert!(said.contains('3'), "{said}");
            assert!(said.contains("conversationId"), "{said}");
            assert!(Target::Nowhere.ambiguity().is_none());
            assert!(Target::Found(row("a")).ambiguity().is_none());
        }

        /// A body is addressed by the first UUID among the three keys the
        /// doors know; a Hermes session id that is not a conversation (the
        /// completions arm's own naming) is no address at all.
        #[test]
        fn the_hint_is_a_uuid_from_any_of_the_three_keys_or_nothing() {
            let id = "22f1e3a0-0000-4000-8000-000000000001";
            assert_eq!(
                conversation_hint(&json!({ "conversationId": format!("  {id} ") })).as_deref(),
                Some(id)
            );
            assert_eq!(
                conversation_hint(&json!({ "sessionId": id })).as_deref(),
                Some(id)
            );
            assert_eq!(
                conversation_hint(&json!({ "parentSessionId": id })).as_deref(),
                Some(id)
            );
            // conversationId wins over a session id when both are sent.
            let other = "22f1e3a0-0000-4000-8000-000000000002";
            assert_eq!(
                conversation_hint(&json!({ "sessionId": other, "conversationId": id })).as_deref(),
                Some(id)
            );
            assert_eq!(
                conversation_hint(&json!({ "sessionId": "20261020_143500_ab12" })),
                None
            );
            // The completions arm's derived session (Hermes v0.22.4's
            // `_derive_chat_session_id`): 16 hex digits, never an address.
            assert_eq!(
                conversation_hint(&json!({ "sessionId": "api-3f2504e04f8911d3" })),
                None
            );
            // Every spelling the cast takes is the same conversation, and it
            // comes back as the one text its rows and channels are keyed by.
            for spelling in [
                "22F1E3A0-0000-4000-8000-000000000001",
                "{22f1e3a0-0000-4000-8000-000000000001}",
                "22f1e3a0000040008000000000000001",
            ] {
                assert_eq!(
                    conversation_hint(&json!({ "conversationId": spelling })).as_deref(),
                    Some(id),
                    "{spelling}"
                );
            }
            assert_eq!(
                conversation_hint(&json!({ "conversationId": format!("urn:uuid:{id}") })),
                None
            );
            assert_eq!(conversation_hint(&json!({ "sessionId": "" })), None);
            assert_eq!(conversation_hint(&json!({ "sessionId": 42 })), None);
            assert_eq!(conversation_hint(&json!({})), None);
        }
    }

    #[test]
    fn a_plan_chip_is_replaced_in_place_and_an_approval_is_not() {
        // The agent rewrites its plan several times inside one turn, so the
        // chip has to carry the latest. An approval must NOT be overwritten
        // by a re-landing of the same action, which is why the rule is per
        // kind rather than global.
        let existing = json!([
            { "id": "todo", "kind": "todo", "label": "0/2 done", "items": [] },
            { "id": "a1", "kind": "approval", "summary": "first", "status": "pending" },
        ]);
        let merged = merge_chips(
            &existing,
            &[
                json!({ "id": "todo", "kind": "todo", "label": "1/2 done", "items": [] }),
                json!({ "id": "a1", "kind": "approval", "summary": "second", "status": "pending" }),
            ],
        );
        let arr = merged.as_array().expect("array");
        assert_eq!(arr.len(), 2, "neither chip duplicated");
        // In place: the plan keeps its position rather than jumping to the end.
        assert_eq!(arr[0]["id"], "todo");
        assert_eq!(arr[0]["label"], "1/2 done");
        assert_eq!(arr[1]["summary"], "first", "an approval is not overwritten");
    }

    #[test]
    fn a_repeated_question_refreshes_and_a_different_one_joins_it() {
        // The chip id is keyed on the question's words: the same question gets
        // the newer options, a different question is its own chip because both
        // are still open.
        let existing = json!([{ "id": "ask:a1", "kind": "question", "label": "Which board?",
                                "options": [{ "label": "old" }] }]);
        let merged = merge_chips(
            &existing,
            &[
                json!({ "id": "ask:a1", "kind": "question", "label": "Which board?",
                        "options": [{ "label": "new" }] }),
                json!({ "id": "ask:b2", "kind": "question", "label": "Which agent?",
                        "options": [{ "label": "x" }] }),
            ],
        );
        let arr = merged.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["options"][0]["label"], "new");
        assert_eq!(arr[1]["label"], "Which agent?");
    }

    #[test]
    fn a_subagent_stop_replaces_its_own_start_in_place() {
        // The chip is keyed on the child's session id because that is the only
        // handle both hooks carry. Stacking a second chip would read as two
        // children; keeping the start would leave a finished child reading as
        // still running.
        let existing = json!([
            { "id": "subagent:c1", "kind": "subagent", "label": "researcher · find the owner",
              "status": "running", "summary": "" },
            { "id": "link:/boards/b-1", "kind": "link" },
        ]);
        let merged = merge_chips(
            &existing,
            &[json!({ "id": "subagent:c1", "kind": "subagent",
                      "label": "researcher · find the owner", "status": "done",
                      "summary": "the owner is Dana" })],
        );
        let arr = merged.as_array().expect("array");
        assert_eq!(arr.len(), 2, "one chip per child: {merged}");
        assert_eq!(arr[0]["status"], "done");
        assert_eq!(arr[0]["summary"], "the owner is Dana");
        assert_eq!(
            arr[1]["id"], "link:/boards/b-1",
            "and the stop stayed in place"
        );
    }

    #[test]
    fn only_replaceable_chips_reach_the_replacing_leg_of_the_write() {
        // The subset bound as `$3`. Too wide and a re-landed approval gets
        // overwritten; too narrow and a plan, a finished child, or the
        // second of two edits to one document freezes at the version that
        // landed first.
        let chips = vec![
            json!({ "id": "subagent:c1", "kind": "subagent" }),
            json!({ "id": "approval:p1", "kind": "approval" }),
            json!({ "id": "todo", "kind": "todo" }),
            json!({ "id": "link:/boards/b-1", "kind": "link" }),
            json!({ "id": "link:/artifacts?a=d1", "kind": "link",
                    "changes": [{ "label": "x", "added": 1, "removed": 0 }] }),
        ];
        let subset = snapshot_chips(&chips);
        assert_eq!(subset.len(), 3, "{subset:?}");
        assert_eq!(subset[0]["id"], "subagent:c1");
        assert_eq!(subset[1]["id"], "todo");
        assert_eq!(
            subset[2]["id"], "link:/artifacts?a=d1",
            "the document chip replaces — and the SQL concatenates its rows"
        );
    }

    #[test]
    fn a_chip_with_no_id_is_dropped_rather_than_appended_blind() {
        let merged = merge_chips(&json!([]), &[json!({ "kind": "todo" })]);
        assert_eq!(merged.as_array().expect("array").len(), 0);
    }

    #[test]
    fn classifies_the_entities_agents_share_and_rejects_the_rest() {
        assert_eq!(classify_href("/boards/b-1").unwrap().entity, "board");
        let ticket = classify_href("https://talaria.example/boards/b-1/t-2").unwrap();
        assert_eq!(ticket.entity, "ticket");
        assert_eq!(ticket.href, "/boards/b-1/t-2");
        assert_eq!(classify_href("/knowledge?doc=doc-9").unwrap().entity, "kb");
        assert_eq!(
            classify_href("/artifacts?a=file-3").unwrap().entity,
            "document"
        );
        assert_eq!(
            classify_href("/comms/channel/chan-1").unwrap().entity,
            "channel"
        );
        assert!(classify_href("https://example.com/pricing").is_none());
        assert!(classify_href("/admin").is_none());
        assert!(classify_href("/boards").is_none());
        assert!(classify_href("javascript:alert(1)").is_none());
    }

    #[test]
    fn a_created_ticket_becomes_a_link_chip_without_a_pasted_url() {
        let chips = chips_from_tool(
            "create_ticket",
            r#"{"boardId":"b-1","title":"Fix the gate"}"#,
            r#"{"task":{"id":"t-2","boardId":"b-1","title":"Fix the gate"}}"#,
        );
        assert!(
            chips
                .iter()
                .any(|c| c["kind"] == "link" && c["href"] == "/boards/b-1/t-2")
        );
        assert_eq!(
            chips
                .iter()
                .filter(|c| c["href"] == "/boards/b-1/t-2")
                .count(),
            1
        );
    }

    #[test]
    fn a_moved_ticket_links_to_its_new_board_and_nothing_else() {
        // move_ticket answers { task, moved }: the task carries its NEW
        // board, and the summary names its boards fromBoardId / toBoardId so
        // it never reads as a second ticket (talaria-tasks MoveSummary).
        let chips = chips_from_tool(
            "move_ticket",
            r#"{"taskId":"t-2","boardId":"b-2"}"#,
            r#"{"task":{"id":"t-2","boardId":"b-2","title":"Gate"},"moved":{"fromBoardId":"b-1","toBoardId":"b-2","fromRef":"A-1","toRef":"B-1"}}"#,
        );
        let links: Vec<&Value> = chips.iter().filter(|c| c["kind"] == "link").collect();
        assert_eq!(links.len(), 1, "{chips:?}");
        assert_eq!(links[0]["href"], "/boards/b-2/t-2");
        assert!(chips.iter().all(|c| c["kind"] != "approval"));
    }

    #[test]
    fn a_drafted_email_becomes_an_approval_chip() {
        let chips = chips_from_tool(
            "draft_email",
            r#"{"to":"a@b.c"}"#,
            r#"{"pending":{"id":"p-1","status":"pending"},"message":"Drafted — waiting"}"#,
        );
        assert_eq!(chips[0]["kind"], "approval");
        assert_eq!(chips[0]["actionKind"], "gmail_send");
        assert_eq!(chips[0]["actionId"], "p-1");
    }

    #[test]
    fn a_refused_move_becomes_an_approval_and_a_success_does_not() {
        let refused = chips_from_tool(
            "triage_ticket",
            r#"{"taskId":"t-2","status":"done"}"#,
            r#"{"error":"agents cannot take a ticket out of review"}"#,
        );
        assert_eq!(refused[0]["actionKind"], "ticket_move");
        assert_eq!(refused[0]["summary"], "Move t-2 to done");
        let ok = chips_from_tool(
            "triage_ticket",
            r#"{"taskId":"t-2","status":"in_progress"}"#,
            r#"{"task":{"id":"t-2","boardId":"b-1","title":"Gate"}}"#,
        );
        assert!(ok.iter().all(|c| c["kind"] != "approval"));
    }

    /// A triage that LANDED never leaves a card, whatever its ticket says —
    /// a stale card a board editor approved later would re-apply the move as
    /// them. Hermes reports the prefixed name, so that spelling is the one
    /// that matters; an asked exit answers with the moved ticket too.
    #[test]
    fn a_landed_triage_whose_ticket_says_cannot_leaves_no_card() {
        for name in [
            "triage_ticket",
            "mcp_talaria_triage_ticket",
            "mcp__talaria__triage_ticket",
        ] {
            let ok = chips_from_tool(
                name,
                r#"{"taskId":"ABC-12","status":"in_progress","conversationId":"c-1"}"#,
                r#"{"task":{"id":"t-2","boardId":"b-1","title":"We cannot ship this","description":"agents cannot fix it alone"}}"#,
            );
            assert!(ok.iter().all(|c| c["kind"] != "approval"), "{name}: {ok:?}");
        }
        // Not even when the ticket's own text quotes a refusal: a landed
        // answer is the ticket, whatever it says.
        for name in ["mcp_talaria_triage_ticket", "mcp_talaria_move_ticket"] {
            let ok = chips_from_tool(
                name,
                r#"{"taskId":"ABC-12","boardId":"b-9","status":"triage"}"#,
                r#"{"task":{"id":"t-2","boardId":"b-9","title":"Talaria API 403: agents cannot take a ticket out of review"},"moved":{}}"#,
            );
            assert!(ok.iter().all(|c| c["kind"] != "approval"), "{name}: {ok:?}");
        }
        // A 400 is a bad call, not a refusal a person could approve past.
        let bad = chips_from_tool(
            "mcp_talaria_triage_ticket",
            r#"{"taskId":"ABC-12","status":"nowhere"}"#,
            "Talaria API 400: status cannot be nowhere",
        );
        assert!(bad.iter().all(|c| c["kind"] != "approval"), "{bad:?}");
    }

    /// The card does not depend on how the client spelled the tool: Hermes
    /// reports `mcp_talaria_*`, a Claude-style client `mcp__talaria__*`.
    #[test]
    fn a_prefixed_tool_name_still_leaves_the_card() {
        for name in [
            "triage_ticket",
            "mcp_talaria_triage_ticket",
            "mcp__talaria__triage_ticket",
        ] {
            let chips = chips_from_tool(
                name,
                r#"{"taskId":"t-2","status":"in_progress"}"#,
                "Talaria API 403: agents cannot take a ticket out of review on their own",
            );
            assert_eq!(chips[0]["actionKind"], "ticket_move", "{name}");
        }
        assert_eq!(bare_tool_name("mcp_talaria_move_ticket"), "move_ticket");
        assert_eq!(bare_tool_name("mcp__talaria__get_ticket"), "get_ticket");
        assert_eq!(bare_tool_name("mcp_other_thing"), "mcp_other_thing");
    }

    /// A refused board move leaves its own card, carrying the call so the
    /// approve route can run it; a move that landed, a preview, and a call
    /// with no board leave none.
    #[test]
    fn a_refused_board_move_becomes_an_approval_and_a_landed_one_does_not() {
        let refused = chips_from_tool(
            "mcp_talaria_move_ticket",
            r#"{"taskId":"t-2","boardId":"b-9","status":"triage"}"#,
            "Talaria API 403: agents cannot take a ticket out of review on their own",
        );
        let card = refused
            .iter()
            .find(|c| c["kind"] == "approval")
            .expect("a card");
        assert_eq!(card["actionKind"], "ticket_board_move");
        assert_eq!(card["actionId"], "board-move:t-2:b-9:triage");
        assert_eq!(card["payload"]["boardId"], "b-9");
        // The card names the ticket; the destination rides beside it as a
        // link whose title each viewer's own access resolves.
        assert_eq!(card["summary"], "Move t-2 to another board (triage)");
        let dest = refused
            .iter()
            .find(|c| c["kind"] == "link")
            .expect("the destination board");
        assert_eq!(dest["href"], "/boards/b-9");
        assert_eq!(dest["entity"], "board");
        assert!(dest["title"].is_null());
        assert!(is_ticket_approval("ticket_board_move"));
        assert!(is_ticket_approval("ticket_move"));
        assert!(!is_ticket_approval("gmail_send"));
        // The api's own `{error}` counts as a refusal too.
        let raw = chips_from_tool(
            "move_ticket",
            r#"{"taskId":"t-2","boardId":"b-9"}"#,
            r#"{"error":"agents cannot take a ticket out of review"}"#,
        );
        assert!(raw.iter().any(|c| c["actionKind"] == "ticket_board_move"));
        // A landed move whose ticket text happens to say "cannot".
        let landed = chips_from_tool(
            "move_ticket",
            r#"{"taskId":"t-2","boardId":"b-9"}"#,
            r#"{"task":{"id":"t-2","boardId":"b-9","title":"We cannot ship"},"moved":{}}"#,
        );
        assert!(landed.iter().all(|c| c["kind"] != "approval"));
        let preview = chips_from_tool(
            "move_ticket",
            r#"{"taskId":"t-2","boardId":"b-9","preview":true}"#,
            "Talaria API 403: agents cannot take a ticket out of review",
        );
        assert!(preview.iter().all(|c| c["kind"] != "approval"));
        let no_board = chips_from_tool(
            "move_ticket",
            r#"{"taskId":"t-2"}"#,
            "Talaria API 403: agents cannot take a ticket out of review",
        );
        assert!(no_board.iter().all(|c| c["kind"] != "approval"));
    }

    #[test]
    fn merge_does_not_stack_the_same_chip() {
        let existing = json!([{ "id": "link:/boards/b-1", "kind": "link" }]);
        let incoming = vec![
            json!({ "id": "link:/boards/b-1", "kind": "link" }),
            json!({ "id": "approval:p-1", "kind": "approval" }),
        ];
        let merged = merge_chips(&existing, &incoming);
        assert_eq!(merged.as_array().unwrap().len(), 2);
    }

    #[test]
    fn an_mcp_call_is_a_google_action_and_its_unlock_is_keyed_by_server_and_tool() {
        assert!(is_google_action("mcp_call"));
        assert_eq!(
            tools_unlocked("mcp_call"),
            [] as [&str; 0],
            "no static tool list"
        );
        let payload = json!({ "server": "google-calendar", "tool": "create_event" });
        assert_eq!(
            mcp_unlock_for("mcp_call", &payload, "owner", "executed").as_deref(),
            Some("google-calendar/create_event")
        );
        // D7: personal principals only, and only an executed row.
        assert_eq!(
            mcp_unlock_for("mcp_call", &payload, "org", "executed"),
            None
        );
        assert_eq!(
            mcp_unlock_for("mcp_call", &payload, "agent", "executed"),
            None
        );
        assert_eq!(
            mcp_unlock_for("mcp_call", &payload, "owner", "failed"),
            None
        );
        assert_eq!(
            mcp_unlock_for("mcp_call", &payload, "owner", "pending"),
            None
        );
        assert_eq!(
            mcp_unlock_for("gmail_send", &payload, "owner", "executed"),
            None
        );
        assert_eq!(
            mcp_unlock_for(
                "mcp_call",
                &json!({ "server": "google-calendar" }),
                "owner",
                "executed"
            ),
            None
        );
        // The grant covers ONE later call of THAT tool on THAT server, for
        // half an hour.
        let now = 1_000;
        let stored = json!([unlock_grant("google-calendar/create_event", None, now)]);
        assert!(consume_grant(&stored, "google-calendar/delete_event", &[], now + 1).is_none());
        assert!(consume_grant(&stored, "google-gmail/create_event", &[], now + 1).is_none());
        assert!(consume_grant(&stored, "create_event", &[], now + 1).is_none());
        let spent = consume_grant(&stored, "google-calendar/create_event", &[], now + 1)
            .expect("the grant covers the same tool");
        assert!(consume_grant(&spent, "google-calendar/create_event", &[], now + 2).is_none());
        assert!(
            consume_grant(
                &stored,
                "google-calendar/create_event",
                &[],
                now + UNLOCK_TTL_MS
            )
            .is_none(),
            "expired after thirty minutes"
        );
        let chip = unlock_chip_for(
            "a-1",
            "mcp_call",
            &["google-calendar/create_event".to_string()],
        );
        assert_eq!(chip["tools"][0], "google-calendar/create_event");
        assert_eq!(
            mcp_call_chip("p-1", "Google Calendar · create_event {}")["actionKind"],
            "mcp_call"
        );
    }

    /// D7 "as draft_email works today": the grant an approved `mcp_call`
    /// leaves is bound to the people and the Chat space the approved call
    /// addressed, read in the argument names Google's catalogs use.
    #[test]
    fn an_mcp_call_grant_covers_no_one_the_approved_call_did_not_address() {
        // Every addressee shape on Google's captured catalogs.
        assert_eq!(
            mcp_call_addressees(&json!({
                "to": ["Bob@X.test"], "cc": ["carol@x.test"], "bcc": ["dan@x.test"],
                "subject": "eve@x.test is not an addressee",
            })),
            ["bob@x.test", "carol@x.test", "dan@x.test"]
        );
        assert_eq!(
            mcp_call_addressees(&json!({
                "attendees": [{ "email": "a@x.test", "displayName": "A" }, "b@x.test"],
                "attendeeEmails": ["c@x.test"],
                "addedAttendees": [{ "email": "D@x.test" }],
                "addedAttendeeEmails": ["e@x.test"],
                "removedAttendeeEmails": ["f@x.test"],
            })),
            [
                "a@x.test", "b@x.test", "c@x.test", "d@x.test", "e@x.test", "f@x.test"
            ]
        );
        assert_eq!(
            mcp_call_addressees(&json!({
                "conversationId": "spaces/AAAAbc",
                "threadId": "spaces/AAAAbc/threads/T1",
                "messageText": "hi",
            })),
            ["chat:spaces/AAAAbc"]
        );
        assert!(mcp_call_addressees(&json!({ "summary": "Standup" })).is_empty());
        assert!(mcp_call_addressees(&Value::Null).is_empty());
        let payload = |args: Value| json!({ "server": "google-calendar", "tool": "create_event", "arguments": args });
        assert_eq!(
            approval_recipients(
                "mcp_call",
                &payload(json!({ "attendees": [{ "email": "a@x.test" }] }))
            ),
            Some(vec!["a@x.test".to_string()])
        );
        assert_eq!(
            approval_recipients("mcp_call", &payload(json!({ "summary": "Solo" }))),
            Some(vec![])
        );

        let now = 1_000;
        let key = "google-calendar/create_event";
        // Approved: an internal standup with one attendee.
        let approved = approval_recipients(
            "mcp_call",
            &payload(json!({ "attendees": [{ "email": "a@x.test" }] })),
        )
        .unwrap();
        let stored = json!([unlock_grant(key, Some(&approved), now)]);
        let next = |args: Value| mcp_call_addressees(&args);
        // The same people, or fewer: covered.
        assert!(
            consume_grant(
                &stored,
                key,
                &next(json!({ "attendees": [{ "email": "A@x.test" }] })),
                now + 1
            )
            .is_some()
        );
        assert!(consume_grant(&stored, key, &next(json!({ "summary": "x" })), now + 1).is_some());
        // Anyone else — an outside attendee, a bare address, an added one: held.
        for args in [
            json!({ "attendees": [{ "email": "a@x.test" }, { "email": "outsider@y.test" }] }),
            json!({ "attendeeEmails": ["outsider@y.test"] }),
            json!({ "addedAttendees": [{ "email": "outsider@y.test" }] }),
        ] {
            assert!(
                consume_grant(&stored, key, &next(args.clone()), now + 1).is_none(),
                "{args}"
            );
        }
        // A call that named nobody earns a grant for calls that name nobody.
        let solo = json!([unlock_grant(key, Some(&[]), now)]);
        assert!(consume_grant(&solo, key, &next(json!({ "summary": "x" })), now + 1).is_some());
        assert!(
            consume_grant(
                &solo,
                key,
                &next(json!({ "attendees": ["b@x.test"] })),
                now + 1
            )
            .is_none()
        );
        // Chat: one space, exactly — another space, or the same id in
        // another case, is held.
        let chat_key = "google-chat/send_message";
        let space = mcp_call_addressees(&json!({ "conversationId": "spaces/AAAAbc" }));
        let chat = json!([unlock_grant(chat_key, Some(&space), now)]);
        assert!(consume_grant(&chat, chat_key, &next(json!({ "conversationId": "spaces/AAAAbc", "threadId": "spaces/AAAAbc/threads/T9" })), now + 1).is_some());
        for other in ["spaces/BBBB", "spaces/aaaabc"] {
            assert!(
                consume_grant(
                    &chat,
                    chat_key,
                    &next(json!({ "conversationId": other })),
                    now + 1
                )
                .is_none(),
                "{other}"
            );
        }
        assert!(
            consume_grant(
                &chat,
                chat_key,
                &next(json!({ "conversationId": "spaces/AAAAbc", "threadId": "spaces/BBBB/threads/T1" })),
                now + 1
            )
            .is_none(),
            "a thread in another space"
        );
    }

    #[test]
    fn unlock_names_the_tools_the_approval_granted() {
        assert_eq!(tools_unlocked("gmail_send"), ["draft_email"]);
        assert!(tools_unlocked("ticket_move").is_empty());
        assert!(tools_unlocked("nope").is_empty());
        assert_eq!(unlock_chip("a-1", "gmail_send")["tools"][0], "draft_email");
    }

    #[test]
    fn every_unlocking_kind_is_in_the_reverse_index() {
        for kind in KINDS_WITH_UNLOCKS {
            assert!(!tools_unlocked(kind).is_empty(), "{kind} unlocks nothing");
        }
        for kind in [
            "gmail_send",
            "calendar_create",
            "doc_update",
            "board_access",
        ] {
            assert!(KINDS_WITH_UNLOCKS.contains(&kind), "{kind} missing");
        }
        // The ticket approvals ARE the move; they grant no later call.
        for kind in ["ticket_move", "ticket_board_move"] {
            assert!(tools_unlocked(kind).is_empty(), "{kind} unlocks a tool");
            assert!(!KINDS_WITH_UNLOCKS.contains(&kind), "{kind} listed");
        }
    }

    #[test]
    fn a_grant_is_the_bounded_shape() {
        let g = unlock_grant("draft_email", Some(&["bob@x.com".to_string()]), 1_000);
        assert_eq!(g["tool"], "draft_email");
        assert_eq!(g["expiresAt"], 1_000 + UNLOCK_TTL_MS);
        assert_eq!(g["usesLeft"], UNLOCK_USES);
        assert_eq!(g["recipients"], json!(["bob@x.com"]));
        let unbound = unlock_grant("triage_ticket", None, 1_000);
        assert!(unbound["recipients"].is_null());
    }

    #[test]
    fn recipients_normalize_to_bare_lowercase_addresses() {
        let got =
            normalize_recipients(["Bob <Bob@X.com>, carol@x.com; dan@x.com", " ", "bob@x.com"]);
        assert_eq!(got, ["bob@x.com", "carol@x.com", "dan@x.com"]);
        let mail = approval_recipients(
            "gmail_send",
            &json!({ "to": "a@x.com", "cc": "B@x.com", "bcc": "" }),
        )
        .expect("bound");
        assert_eq!(mail, ["a@x.com", "b@x.com"]);
        let event = approval_recipients(
            "calendar_create",
            &json!({ "attendees": ["C@x.com", "d@x.com"] }),
        )
        .expect("bound");
        assert_eq!(event, ["c@x.com", "d@x.com"]);
        assert!(approval_recipients("ticket_move", &json!({})).is_none());
    }

    #[test]
    fn a_grant_covers_a_subset_of_the_approved_recipients_and_nothing_more() {
        let g = unlock_grant(
            "draft_email",
            Some(&["bob@x.com".to_string(), "carol@x.com".to_string()]),
            1_000,
        );
        let now = 2_000;
        assert!(grant_allows(&g, "draft_email", &["bob@x.com".into()], now));
        assert!(grant_allows(&g, "draft_email", &["BOB@x.com".into()], now));
        assert!(grant_allows(&g, "draft_email", &[], now));
        assert!(!grant_allows(
            &g,
            "draft_email",
            &["bob@x.com".into(), "attacker@evil.com".into()],
            now
        ));
        assert!(!grant_allows(&g, "draft_calendar_event", &[], now));
        // Expired, spent, or the pre-bounded bare string: no grant.
        assert!(!grant_allows(&g, "draft_email", &[], 1_000 + UNLOCK_TTL_MS));
        let mut spent = g.clone();
        spent["usesLeft"] = json!(0);
        assert!(!grant_allows(&spent, "draft_email", &[], now));
        assert!(!grant_allows(
            &json!("draft_email"),
            "draft_email",
            &[],
            now
        ));
        // An unbound grant covers any recipients.
        let unbound = unlock_grant("triage_ticket", None, 1_000);
        assert!(grant_allows(
            &unbound,
            "triage_ticket",
            &["x@y.z".into()],
            now
        ));
    }

    #[test]
    fn consuming_spends_one_use_and_drops_what_is_spent_or_stale() {
        let bob = vec!["bob@x.com".to_string()];
        let stored = json!([
            "draft_email",
            unlock_grant("draft_email", Some(&bob), 1_000),
            unlock_grant("draft_calendar_event", Some(&bob), 1_000),
            { "tool": "update_google_doc", "expiresAt": 1, "usesLeft": 5, "recipients": null },
        ]);
        let next = consume_grant(&stored, "draft_email", &bob, 2_000).expect("covered");
        let arr = next.as_array().unwrap();
        // One use was all it had, so the spent grant is gone; the legacy
        // string and the expired doc grant fell out; the calendar one stays.
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["tool"], "draft_calendar_event");
        assert!(consume_grant(&next, "draft_email", &bob, 2_000).is_none());
        assert!(consume_grant(&stored, "draft_email", &["eve@x.com".into()], 2_000).is_none());
        assert!(consume_grant(&json!([]), "draft_email", &[], 2_000).is_none());
    }

    #[test]
    fn merging_replaces_the_same_tool_and_keeps_the_rest() {
        let stored = json!([
            "draft_email",
            unlock_grant("draft_email", None, 1_000),
            unlock_grant("draft_calendar_event", None, 1_000),
            { "tool": "update_google_doc", "expiresAt": 1, "usesLeft": 5 },
        ]);
        let fresh = unlock_grant("draft_email", Some(&["bob@x.com".to_string()]), 5_000);
        let merged = merge_grants(&stored, std::slice::from_ref(&fresh), 5_000);
        let arr = merged.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["tool"], "draft_calendar_event");
        assert_eq!(arr[1], fresh);
    }

    #[test]
    fn a_created_sheet_becomes_one_document_link_chip() {
        let chips = chips_from_tool(
            "create_sheet",
            r#"{"title":"Q3 numbers"}"#,
            r#"{"id":"9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2","title":"Q3 numbers"}"#,
        );
        assert_eq!(chips.len(), 1, "no stray chips: {chips:?}");
        assert_eq!(chips[0]["kind"], "link");
        assert_eq!(chips[0]["entity"], "document");
        assert_eq!(
            chips[0]["href"],
            "/artifacts?a=9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2"
        );
    }

    #[test]
    fn an_edited_document_carries_what_the_edit_did() {
        let chips = chips_from_tool(
            "edit_document",
            r#"{"documentId":"9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2"}"#,
            r###"{"artifact":{"id":"9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2","title":"Runbook"},
                "changes":[
                  {"label":"the old line","added":12,"removed":9,
                   "preview":{"old":"the old line","new":"the new line"}},
                  {"label":"## Rollback","added":40,"removed":31}
                ]}"###,
        );
        let doc = chips
            .iter()
            .find(|c| c["entity"] == "document")
            .expect("the way into the document is still the chip");
        assert_eq!(
            doc["href"], "/artifacts?a=9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2",
            "id, kind and href unchanged: {doc}"
        );
        assert_eq!(doc["title"], "Runbook");
        let changes = doc["changes"].as_array().expect("changes carried");
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0]["preview"]["old"], "the old line");
        assert_eq!(changes[0]["preview"]["new"], "the new line");
        assert_eq!(changes[0]["added"], 12);
        // A section edit has no honest before/after, and the key is absent
        // rather than null — one absence to test for, not two.
        assert_eq!(changes[1]["label"], "## Rollback");
        assert!(changes[1].get("preview").is_none(), "{}", changes[1]);
    }

    #[test]
    fn a_malformed_change_row_is_dropped_and_a_full_rewrite_carries_none() {
        let chips = chips_from_tool(
            "edit_document",
            "{}",
            r#"{"artifact":{"id":"doc-1"},"changes":[
                 {"label":"kept","added":1,"removed":0},
                 {"added":1,"removed":0},
                 {"label":"no counts"},
                 {"label":"string counts","added":"1","removed":"0"},
                 {"label":"bad preview","added":1,"removed":1,"preview":{"old":"x"}}
               ]}"#,
        );
        let doc = chips
            .iter()
            .find(|c| c["entity"] == "document")
            .expect("chip");
        let changes = doc["changes"].as_array().expect("changes");
        assert_eq!(changes.len(), 2, "only the well-formed rows: {doc}");
        assert_eq!(changes[0]["label"], "kept");
        assert_eq!(changes[1]["label"], "bad preview");
        assert!(
            changes[1].get("preview").is_none(),
            "half a preview is none"
        );

        // update_document rewrites the body; there is no diff to show.
        let rewrite = chips_from_tool(
            "update_document",
            "{}",
            r#"{"artifact":{"id":"doc-1"},"changes":[{"label":"x","added":1,"removed":1}]}"#,
        );
        let plain = rewrite
            .iter()
            .find(|c| c["entity"] == "document")
            .expect("chip");
        assert!(plain.get("changes").is_none(), "{plain}");
    }

    /// THE ROUND TRIP IS LOSSY BY CONSTRUCTION — the plugin clamps a tool
    /// result to 4000 chars — so an `edit_document` result that still
    /// carried the document body arrived as a prefix of JSON and landed NO
    /// chip at all: no id, no changes, nothing. Rewound to its last whole
    /// element, the id is there and so are the change rows that survived.
    ///
    /// The edit route lands this chip itself now (`document_chip`), which is
    /// what makes the full list reliable; this is the fallback still being
    /// worth something.
    #[test]
    fn a_truncated_edit_result_still_lands_the_document_chip() {
        let id = "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2";
        let rows: Vec<String> = (0..10)
            .map(|i| {
                format!(
                    r#"{{"label":"anchor {i}","added":2,"removed":1,"preview":{{"old":"was {i}","new":"now {i}"}}}}"#
                )
            })
            .collect();
        // The shape before the route went slim — and the shape any other
        // producer of this result may still send.
        let full = format!(
            r#"{{"artifact":{{"id":"{id}","title":"Runbook","body":"{}"}},"changes":[{}]}}"#,
            "x".repeat(3_500),
            rows.join(",")
        );
        let clamped: String = full.chars().take(4000).collect();
        assert!(
            serde_json::from_str::<Value>(&clamped).is_err(),
            "truncated"
        );

        let chips = chips_from_tool(
            "edit_document",
            &format!(r#"{{"documentId":"{id}"}}"#),
            &clamped,
        );
        let doc = chips
            .iter()
            .find(|c| c["entity"] == "document")
            .expect("the chip the reader clicks, from a result cut mid-flight");
        assert_eq!(doc["href"], format!("/artifacts?a={id}"));
        assert_eq!(doc["title"], "Runbook");
        let changes = doc["changes"].as_array().expect("the rows that survived");
        assert!(
            !changes.is_empty() && changes.len() < rows.len(),
            "some of the list, not all of it and not none: {}",
            changes.len()
        );
        assert_eq!(changes[0]["label"], "anchor 0");
        assert!(
            changes
                .iter()
                .all(|c| c["added"].is_number() && c["removed"].is_number()),
            "and no half-written row rode along: {doc}"
        );
    }

    /// TWO EDITS IN ONE TURN. The chip's id is per document, so the second
    /// edit's chip lands on the first one's id — and first-wins would drop
    /// what the second edit did while last-wins would hide the first.
    #[test]
    fn two_edits_to_one_document_keep_both_change_sets() {
        let chip = |label: &str| {
            json!({
                "id": "link:/artifacts?a=doc-1",
                "kind": "link",
                "href": "/artifacts?a=doc-1",
                "entity": "document",
                "title": "Runbook",
                "changes": [{ "label": label, "added": 1, "removed": 1 }],
            })
        };
        let merged = merge_chips(
            &Value::Array(vec![chip("first edit")]),
            &[chip("second edit")],
        );
        let arr = merged.as_array().expect("array");
        assert_eq!(arr.len(), 1, "one document, one chip: {merged}");
        let changes = arr[0]["changes"].as_array().expect("changes");
        assert_eq!(changes.len(), 2, "{merged}");
        assert_eq!(
            changes[0]["label"], "first edit",
            "in the order they happened"
        );
        assert_eq!(changes[1]["label"], "second edit");

        // And the ceiling holds: a turn of edits cannot grow the row without
        // bound.
        let many: Vec<Value> = (0..MAX_DOC_CHANGES + 5)
            .map(|i| json!({ "label": format!("row {i}"), "added": 1, "removed": 0 }))
            .collect();
        let mut big = chip("later");
        big["changes"] = Value::Array(many);
        let capped = merge_chips(&merged, &[big]);
        let rows = capped[0]["changes"].as_array().expect("changes");
        assert_eq!(rows.len(), MAX_DOC_CHANGES, "{capped}");
        assert_eq!(rows[0]["label"], "first edit", "the oldest rows are kept");

        // A link chip with NO changes is still an event: first-wins, so a
        // re-landed link cannot blank the list.
        let plain = json!({ "id": "link:/artifacts?a=doc-1", "kind": "link",
                            "href": "/artifacts?a=doc-1", "entity": "document" });
        let after = merge_chips(&merged, &[plain]);
        assert_eq!(
            after[0]["changes"].as_array().expect("still there").len(),
            2,
            "{after}"
        );
    }

    #[test]
    fn the_same_edit_reported_twice_is_one_set_of_rows() {
        // The edit route lands the chip, then the agent's tool-event round
        // trip lands the SAME chip: a union, so the card shows each change
        // once; a genuinely new row still appends.
        let first = document_chip(
            "d1",
            Some("Doc"),
            Some(&json!([{"label":"a","added":1,"removed":1}])),
        );
        let merged = merge_chips(
            &Value::Array(vec![first.clone()]),
            std::slice::from_ref(&first),
        );
        assert_eq!(changes_on(&merged[0]).len(), 1, "{merged}");
        let second = document_chip(
            "d1",
            Some("Doc"),
            Some(&json!([
                {"label":"a","added":1,"removed":1},
                {"label":"b","added":2,"removed":0},
            ])),
        );
        let merged = merge_chips(&merged, &[second]);
        assert_eq!(changes_on(&merged[0]).len(), 2, "{merged}");
        assert_eq!(changes_on(&merged[0])[1]["label"], "b");
    }

    #[test]
    fn a_web_search_lands_one_source_chip_per_page() {
        let chips = chips_from_tool(
            "web_search",
            r#"{"query":"talaria"}"#,
            r#"[{"title":"A page","url":"https://example.com/a","snippet":"what it says"},
                {"title":"B page","url":"https://example.com/b"},
                {"title":"A again","url":"https://example.com/a","snippet":"a second engine"}]"#,
        );
        assert_eq!(chips.len(), 2, "deduped by url: {chips:?}");
        assert_eq!(chips[0]["kind"], "source");
        // The id is the first 16 hex of the url's sha256 — the same handle
        // every time the same page comes back.
        assert_eq!(chips[0]["id"], "source:2dce0a4c50441bfc");
        assert_eq!(chips[0]["label"], "A page");
        assert_eq!(chips[0]["href"], "https://example.com/a");
        assert_eq!(chips[0]["detail"], "what it says");
        assert_eq!(chips[1]["id"], "source:d7fe568b31ae0fb9");
        assert!(chips[1].get("detail").is_none(), "no snippet, no detail");
    }

    #[test]
    fn a_search_lands_at_most_six_sources() {
        let rows: Vec<String> = (0..20)
            .map(|i| format!(r#"{{"title":"p{i}","url":"https://example.com/{i}"}}"#))
            .collect();
        let result = format!(r#"{{"results":[{}]}}"#, rows.join(","));
        let chips = chips_from_tool("web_search", "{}", &result);
        assert_eq!(chips.len(), 6, "{chips:?}");
        assert!(chips.iter().all(|c| c["kind"] == "source"));
    }

    #[test]
    fn a_prose_search_result_still_lands_its_pages() {
        // Talaria's own web_search answers the model in numbered triples,
        // which never parse as JSON.
        let text = "1. The gate\n   https://ex.org/page?q=1\n   How the gate works\n\n\
                    2. Elsewhere\n   https://example.com/b\n   Another one";
        let chips = chips_from_tool("search_web", "{}", text);
        assert_eq!(chips.len(), 2, "{chips:?}");
        assert_eq!(chips[0]["id"], "source:85c990dc49e499db");
        assert_eq!(chips[0]["label"], "The gate", "the line above is the title");
        assert_eq!(chips[0]["detail"], "How the gate works");
        assert_eq!(chips[1]["label"], "Elsewhere");
    }

    #[test]
    fn a_url_in_a_sentence_falls_back_to_its_host() {
        let chips = chips_from_tool("web_search", "{}", "I found it at https://ex.org/page?q=1.");
        assert_eq!(chips.len(), 1, "{chips:?}");
        assert_eq!(chips[0]["label"], "ex.org");
        assert_eq!(
            chips[0]["href"], "https://ex.org/page?q=1",
            "the sentence's full stop is not part of the url"
        );
    }

    /// A titleless source reads as the site a person recognizes — the
    /// spelling the research report's citations already use. `www.` and a
    /// port are machinery, and a label that carries them reads as a
    /// different site.
    #[test]
    fn a_titleless_source_is_labelled_by_the_site_not_the_authority() {
        let labelled = |url: &str| {
            let chips = chips_from_tool("web_search", "{}", &format!("see {url} for it"));
            assert_eq!(chips.len(), 1, "{url}: {chips:?}");
            chips[0]["label"].as_str().expect("a label").to_string()
        };
        assert_eq!(
            labelled("https://www.nytimes.com/2026/01/01/a.html"),
            "nytimes.com"
        );
        assert_eq!(labelled("http://localhost:3000/doc"), "localhost");
        assert_eq!(labelled("https://www.example.com:8443/x"), "example.com");
        // Not a prefix match: `wwwx.` is somebody's hostname.
        assert_eq!(labelled("https://wwwx.example.org/x"), "wwwx.example.org");
        // The href is untouched — the label is for reading, the link is for
        // going.
        let chips = chips_from_tool("web_search", "{}", "see https://www.nytimes.com:443/a now");
        assert_eq!(chips[0]["href"], "https://www.nytimes.com:443/a");
    }

    /// THE CLAMP IS A NORMAL CASE for a search. A search tool that answers
    /// ten pretty-printed rows runs past the plugin's clamp, so the payload
    /// that reaches the tool-events door is a prefix of JSON. Read as prose it
    /// yields six chips labelled by host with no snippet; repaired, it
    /// yields the engine's own titles and snippets.
    #[test]
    fn a_truncated_search_payload_keeps_its_titles_and_snippets() {
        let results: Vec<Value> = (0..10)
            .map(|i| {
                json!({
                    "title": format!("Result {i} — what the page calls itself"),
                    "url": format!("https://example.com/page/{i}"),
                    "snippet": format!("{} (result {i})", "a sentence of the page's own words, as the engine returned them, ".repeat(4)),
                    "engine": "searxng",
                })
            })
            .collect();
        let body = serde_json::to_string_pretty(&json!({
            "query": "talaria",
            "results": results,
            "via": Value::Null,
        }))
        .expect("plain data");
        assert!(
            body.chars().count() > 4000,
            "the real payload is past the plugin's clamp: {} chars",
            body.chars().count()
        );
        let clamped: String = body.chars().take(4000).collect();
        assert!(
            serde_json::from_str::<Value>(&clamped).is_err(),
            "and the clamp leaves it unparseable"
        );

        let chips = chips_from_tool("web_search", r#"{"query":"talaria"}"#, &clamped);
        assert_eq!(chips.len(), MAX_SOURCES, "{chips:?}");
        assert!(chips.iter().all(|c| c["kind"] == "source"));
        assert_eq!(chips[0]["label"], "Result 0 — what the page calls itself");
        assert_eq!(chips[0]["href"], "https://example.com/page/0");
        assert!(
            chips[0]["detail"]
                .as_str()
                .expect("the engine's snippet")
                .starts_with("a sentence of the page's own words"),
            "{}",
            chips[0]
        );
        assert!(
            chips.iter().all(|c| c["label"] != "example.com"),
            "no chip fell through to the prose walk: {chips:?}"
        );
    }

    #[test]
    fn a_search_that_found_nothing_lands_nothing() {
        assert!(chips_from_tool("web_search", "{}", "No results for \"talaria\".").is_empty());
        assert!(chips_from_tool("web_search", "{}", "[]").is_empty());
        assert!(
            chips_from_tool("web_search", "{}", r#"[{"title":"x","url":"/local/path"}]"#)
                .is_empty(),
            "a relative path is not a page to send a reader to"
        );
    }

    #[test]
    fn a_board_access_request_carries_its_payload() {
        let chips = chips_from_tool("request_board_access", r#"{"boardId":"b-9"}"#, "{}");
        assert_eq!(chips.len(), 1, "no stray chips: {chips:?}");
        assert_eq!(chips[0]["kind"], "approval");
        assert_eq!(chips[0]["actionKind"], "board_access");
        assert_eq!(chips[0]["actionId"], "board:b-9");
        assert_eq!(chips[0]["payload"], json!({ "boardId": "b-9" }));
    }

    // ── Google files the agent worked on ────────────────────────────────────

    fn google_chips(chips: &[Value]) -> Vec<&Value> {
        chips
            .iter()
            .filter(|c| {
                c["kind"] == "source"
                    && c["href"].as_str().and_then(parse_google_file_url).is_some()
            })
            .collect()
    }

    #[test]
    fn a_created_google_doc_lands_one_chip_at_its_canonical_url() {
        let chips = chips_from_tool(
            "create_google_doc",
            r#"{"title":"Cutover runbook"}"#,
            r#"{"id":"1AbCdoc","url":"https://docs.google.com/document/d/1AbCdoc/edit?usp=drivesdk","name":"Cutover runbook","mimeType":"application/vnd.google-apps.document","createdByAgent":true}"#,
        );
        let g = google_chips(&chips);
        assert_eq!(g.len(), 1, "{chips:?}");
        assert_eq!(
            g[0]["href"],
            "https://docs.google.com/document/d/1AbCdoc/edit"
        );
        assert_eq!(g[0]["label"], "Cutover runbook");
        assert_eq!(g[0]["title"], "Cutover runbook");
        assert!(
            g[0]["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("source:")),
            "{}",
            g[0]
        );
        assert!(chips.iter().all(|c| c["kind"] != "approval"));
    }

    #[test]
    fn a_create_that_queued_names_no_file_yet() {
        // Into somebody's folder: a card, and no file until it is approved.
        let chips = chips_from_tool(
            "create_google_doc",
            r#"{"title":"Plan","folderId":"f1"}"#,
            r#"{"pending":{"id":"p-7","status":"pending","kind":"doc_create"},"message":"Queued"}"#,
        );
        assert_eq!(chips.len(), 1, "{chips:?}");
        assert_eq!(chips[0]["kind"], "approval");
        assert!(google_chips(&chips).is_empty());
    }

    #[test]
    fn a_queued_edit_keeps_its_card_and_names_the_file_from_the_argument() {
        let chips = chips_from_tool(
            "update_google_doc",
            r#"{"id":"1theirs","body":"x","title":"An unapproved rename"}"#,
            r#"{"pending":{"id":"p-8","status":"pending","kind":"doc_update"},"message":"Waiting"}"#,
        );
        assert!(chips.iter().any(|c| c["kind"] == "approval"));
        let g = google_chips(&chips);
        assert_eq!(g.len(), 1, "{chips:?}");
        assert_eq!(
            g[0]["href"],
            "https://docs.google.com/document/d/1theirs/edit"
        );
        // A rename nobody approved is not what the file is called.
        assert_eq!(g[0]["label"], "Google Doc");
        assert!(g[0].get("title").is_none(), "{}", g[0]);
    }

    #[test]
    fn a_read_names_the_file_by_the_tools_type_when_the_url_was_clamped_away() {
        // The plugin cuts a result at 4000 chars, and a long markdown body
        // can take the url with it — the argument id still names the file.
        let clamped = format!(r#"{{"id":"1sheet","markdown":"{}"#, "x".repeat(50));
        let chips = chips_from_tool("read_google_sheet", r#"{"id":"1sheet"}"#, &clamped);
        let g = google_chips(&chips);
        assert_eq!(g.len(), 1, "{chips:?}");
        assert_eq!(
            g[0]["href"],
            "https://docs.google.com/spreadsheets/d/1sheet/edit"
        );
        assert_eq!(g[0]["label"], "Google Sheet");

        let deck = chips_from_tool(
            "read_google_slides",
            r#"{"id":"1deck"}"#,
            r#"{"id":"1deck","title":"Board deck","slides":[]}"#,
        );
        let g = google_chips(&deck);
        assert_eq!(
            g[0]["href"],
            "https://docs.google.com/presentation/d/1deck/edit"
        );
        assert_eq!(g[0]["title"], "Board deck");

        let file = chips_from_tool(
            "read_drive_file",
            r#"{"id":"1pdf"}"#,
            r#"{"id":"1pdf","encoding":"base64","text":"JVBERi0"}"#,
        );
        assert_eq!(
            google_chips(&file)[0]["href"],
            "https://drive.google.com/file/d/1pdf/view"
        );
    }

    #[test]
    fn a_failed_google_call_and_a_search_land_no_file() {
        assert!(
            google_chips(&chips_from_tool(
                "read_google_doc",
                r#"{"id":"1x"}"#,
                r#"{"error":"Google is not connected"}"#,
            ))
            .is_empty()
        );
        // A list of hits is not a file the agent opened.
        assert!(
            google_chips(&chips_from_tool(
                "find_google_files",
                r#"{"q":"plan"}"#,
                r#"{"files":[{"id":"1a","name":"Plan","url":"https://docs.google.com/document/d/1a/edit"}]}"#,
            ))
            .is_empty()
        );
        // An id outside the Drive alphabet is not trusted into a URL.
        assert!(
            google_chips(&chips_from_tool(
                "read_google_doc",
                r#"{"id":"../../etc"}"#,
                "{}",
            ))
            .is_empty()
        );
    }

    #[test]
    fn two_tools_on_one_file_land_one_chip_id() {
        let read = chips_from_tool(
            "read_google_doc",
            r#"{"id":"1same"}"#,
            r#"{"id":"1same","title":"Spec","url":"https://docs.google.com/document/d/1same/edit?usp=drivesdk"}"#,
        );
        let edit = chips_from_tool(
            "append_google_doc",
            r#"{"id":"1same","body":"more"}"#,
            r#"{"id":"1same","url":"https://docs.google.com/document/d/1same/edit","name":"Spec","createdByAgent":true}"#,
        );
        assert_eq!(google_chips(&read)[0]["id"], google_chips(&edit)[0]["id"]);
    }

    #[test]
    fn an_imported_drive_file_becomes_a_document_chip() {
        let chips = chips_from_tool(
            "import_drive_file",
            r#"{"fileId":"1src"}"#,
            r#"{"artifact":{"id":"9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2","kind":"doc","title":"Imported plan","url":"https://docs.google.com/document/d/1src/edit"},"message":"Imported."}"#,
        );
        let doc = chips
            .iter()
            .find(|c| c["entity"] == "document")
            .expect("the imported artifact is a document chip");
        assert_eq!(
            doc["href"],
            "/artifacts?a=9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2"
        );
        assert_eq!(doc["title"], "Imported plan");
    }

    #[test]
    fn a_toolkit_tool_reads_by_its_own_name_whatever_the_mcp_client_called_it() {
        for spelling in [
            "create_document",
            "mcp__talaria__create_document",
            "mcp_talaria_create_document",
            "  mcp__talaria__create_document ",
        ] {
            assert_eq!(bare_tool_name(spelling), "create_document", "{spelling:?}");
        }
        // Another server's tool is not the toolkit's, whatever it is called.
        assert_eq!(
            bare_tool_name("mcp__workspace__create_document"),
            "mcp__workspace__create_document"
        );
        assert_eq!(bare_tool_name("mcp__talaria__"), "mcp__talaria__");
        assert_eq!(bare_tool_name("web_search"), "web_search");
    }

    #[test]
    fn an_mcp_answer_is_read_out_of_its_envelope_whole_or_cut_short() {
        let inner = "{\n  \"artifact\": {\n    \"id\": \"9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2\",\n    \"title\": \"Launch plan\"\n  }\n}";
        let whole = serde_json::to_string(&json!({ "result": inner })).unwrap();
        assert_eq!(mcp_result_text(&whole), inner);
        // Hermes' own spelling (json.dumps puts a space after the colon), cut
        // by the plugin's clamp in the middle of the string.
        let spaced = whole.replacen("\"result\":", "\"result\": ", 1);
        let cut: String = spaced.chars().take(40).collect();
        let prefix = mcp_result_text(&cut);
        assert!(inner.starts_with(prefix.as_ref()), "{prefix:?}");
        assert!(prefix.starts_with("{\n  \"artifact\""), "{prefix:?}");
        // An escape cut in half is dropped, not guessed.
        assert_eq!(json_string_prefix("ab\\"), "ab");
        assert_eq!(json_string_prefix("a\\u00e9b\\u00"), "aéb");
        // Not an envelope: as it came.
        for raw in [
            r#"{"artifact":{"id":"x"}}"#,
            r#"{"result":5}"#,
            r#"{"result":"x","more":1}"#,
            "plain words",
        ] {
            assert_eq!(mcp_result_text(raw), raw);
        }
    }

    /// A DOCUMENT A READER ONLY MENTIONED OPENS NOTHING. Every document chip
    /// is a file of the session, which the Work pane lists and brings
    /// forward, so only a writer's answer may land one — not a search whose
    /// hits point at documents, and not a single read that carries a link.
    #[test]
    fn a_document_link_in_a_readers_answer_lands_no_chip() {
        let (a, b) = (
            "9f1d0c44-3ab2-4f65-9d1a-2c88f7b0e1f2",
            "1c2d3e4f-5a6b-4c7d-8e9f-0a1b2c3d4e5f",
        );
        let hits = format!(
            r#"{{"hits":[{{"title":"Launch plan","snippet":"…","href":"/artifacts?a={a}"}},{{"title":"Pricing","href":"/artifacts?a={b}"}}]}}"#
        );
        let single = format!(
            r#"{{"artifact":{{"id":"{a}","title":"Launch plan"}},"href":"/artifacts?a={a}"}}"#
        );
        let bare = format!("/artifacts?a={a}");
        for (tool, result) in [
            ("search_knowledge", hits.as_str()),
            ("get_document", single.as_str()),
            ("open_in_pane", single.as_str()),
            ("get_document", bare.as_str()),
        ] {
            let chips = chips_from_tool(tool, "{}", result);
            assert!(
                chips.iter().all(|c| c["entity"] != "document"),
                "{tool}: {chips:?}"
            );
        }
        // A writer still lands its own — and only its own.
        let made = chips_from_tool("create_document", "{}", &single);
        let docs: Vec<&Value> = made.iter().filter(|c| c["entity"] == "document").collect();
        assert_eq!(docs.len(), 1, "{made:?}");
        assert_eq!(docs[0]["href"], format!("/artifacts?a={a}"));
    }

    /// A LISTING IS NOT A TRAIL. A ticket a call created or moved lands its
    /// link; the rows a listing or a search returned land none — one chip
    /// per row buried what the turn was about.
    #[test]
    fn a_listing_lands_no_link_per_row_and_a_single_answer_still_does() {
        let rows = (0..5)
            .map(|i| {
                format!(r#"{{"id":"t-{i}","boardId":"b-1","ticketRef":"A-{i}","title":"Row {i}"}}"#)
            })
            .collect::<Vec<_>>()
            .join(",");
        let listing = format!(r#"{{"tasks":[{rows}]}}"#);
        assert!(
            chips_from_tool("list_tickets", r#"{"boardId":"b-1"}"#, &listing).is_empty(),
            "a listing"
        );
        let hits = r#"{"hits":[{"title":"A board","href":"/boards/b-1"},{"title":"A doc","href":"/knowledge?doc=k-1"}]}"#;
        assert!(
            chips_from_tool("search_knowledge", "{}", hits).is_empty(),
            "search hits"
        );
        // A ticket's own answer still links it, whatever lists ride beside it.
        let one = r#"{"task":{"id":"t-2","boardId":"b-1","title":"Fix the gate"},"subTasks":[{"id":"t-3","boardId":"b-1"}]}"#;
        let chips = chips_from_tool("get_ticket", r#"{"taskId":"t-2"}"#, one);
        let links: Vec<&Value> = chips.iter().filter(|c| c["kind"] == "link").collect();
        assert_eq!(links.len(), 1, "{chips:?}");
        assert_eq!(links[0]["href"], "/boards/b-1/t-2");
    }
}
