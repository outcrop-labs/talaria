// Live-DB + live-Redis proof that every agent file call lands its chip in the
// conversation it was made in — the Work pane's tabs.
//
// THE WIRE IS REPLAYED, NOT IMAGINED. Every frame below is built the way the
// live wire builds it (FR1's Hermes v0.22.4 probe, the talaria-events
// plugin): the route's REAL answer, pretty-printed with a 2-space indent (the
// toolkit's `ok()`), wrapped as `{"result": "…"}` with Python's default
// separators and `ensure_ascii` (Hermes' MCP handler), cut to 4000 chars (the
// plugin's `_clamp`), and posted to /api/agents/tool-events under the name
// Hermes registered the tool by, with Hermes' own `api-<16 hex>` session — the
// one an unpinned agent's chat runs on — recorded in usage_attributions
// against the Work conversation, as the chat transport records it.
//
// WHY ONLY A REAL DATABASE PROVES IT. Which conversation a session names is a
// join; which row a chip lands on is an ORDER BY over live rows; the chip
// merge is one SQL statement; and the pane's list is read back through the
// files route, as the person's own session.
//
//   source ui/.env && cargo test -p talaria-api --test it pane_tabs_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::conversations::{
    create_conversation, insert_streaming_assistant, insert_user_message, session_conversation,
};
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "pane-tabs-live";

/// A throwaway person for ONE test, with a session cookie.
async fn member(state: &AppState, tag: &str) -> (String, String) {
    let email = format!("{}@{PREFIX}-{tag}.invalid", uuid::Uuid::new_v4());
    minted_member_with_email(state, &format!("{PREFIX}:{tag}"), &email).await
}

/// A fleet agent with its own credential — the key a container presents.
async fn fleet_agent(state: &AppState, tag: &str) -> (String, String) {
    let model = format!("{PREFIX}-{tag}-{}", uuid::Uuid::new_v4());
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'engineering', $2, 'Pane Tabs Live Agent') returning id::text",
    )
    .bind(&model)
    .bind(&model)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, &id)
        .await
        .expect("agent key mints");
    (model, key)
}

/// One request through the REAL router with an agent's credential.
async fn agent_call(
    state: &AppState,
    key: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("x-api-key", key);
    let req = match body {
        Some(b) => req
            .header("content-type", "application/json")
            .body(axum::body::Body::from(b.to_string())),
        None => req.body(axum::body::Body::empty()),
    }
    .unwrap();
    let res = talaria_api::routes::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// A session of `kind` with the person's message at seq 0 and the agent's
/// reply streaming at seq 1. Returns (conversation, live row).
async fn live_turn(state: &AppState, owner: &str, model: &str, kind: &str) -> (String, String) {
    let conv = create_conversation(&state.pg, owner, model, "Session", kind, None)
        .await
        .unwrap();
    insert_user_message(
        &state.pg,
        &conv,
        0,
        "write the launch brief and keep the tracker current",
        &json!([]),
        Some(owner),
        &json!({}),
    )
    .await
    .unwrap();
    let row = insert_streaming_assistant(&state.pg, &conv, 1, &json!({}))
        .await
        .unwrap();
    (conv, row)
}

/// A channel the agent is replying in right now: its streaming row, a moment
/// old. Returns (channel, row).
async fn live_channel_reply(state: &AppState, creator: &str, model: &str) -> (String, String) {
    let (channel,): (String,) = sqlx::query_as(
        "insert into channels (name, created_by) values ($1, $2::uuid) returning id::text",
    )
    .bind(format!("{PREFIX}-{}", uuid::Uuid::new_v4()))
    .bind(creator)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    let (row,): (String,) = sqlx::query_as(
        "insert into channel_messages (channel_id, seq, author_type, author, status, created_at) \
         values ($1::uuid, 1, 'agent', $2, 'streaming', now() - interval '2 minutes') \
         returning id::text",
    )
    .bind(&channel)
    .bind(model)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    (channel, row)
}

/// Hermes' completions-arm session for one conversation, recorded the way
/// the chat transport records it when the stream opens
/// (talaria-gateway `record_attribution`).
async fn attribute(pg: &PgPool, model: &str, conversation: &str) -> String {
    let session = format!("api-{}", hex(16));
    attribute_key(pg, model, &session, conversation).await;
    session
}

async fn attribute_key(pg: &PgPool, model: &str, session: &str, conversation: &str) {
    sqlx::query(
        "insert into usage_attributions (agent_model, affinity_key, source, ref_id) \
         values ($1, $2, 'chat', $3::uuid)",
    )
    .bind(model)
    .bind(session)
    .bind(conversation)
    .execute(pg)
    .await
    .unwrap();
}

/// An artifact row, as the Files page would have made it.
async fn artifact(pg: &PgPool, owner: &str, title: &str, kind: &str, body: &str) -> String {
    sqlx::query_scalar(
        "insert into artifacts (kind, title, body, visibility, edit_policy, owner_user_id) \
         values ($1, $2, $3, 'org', 'owner', $4::uuid) returning id::text",
    )
    .bind(kind)
    .bind(title)
    .bind(body)
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn grant_editor(pg: &PgPool, artifact: &str, model: &str) {
    sqlx::query(
        "insert into kb_editors (item_type, item_id, principal_type, principal_id, role) \
         values ('artifact', $1::uuid, 'agent', $2, 'editor')",
    )
    .bind(artifact)
    .bind(model)
    .execute(pg)
    .await
    .unwrap();
}

async fn chips_of(pg: &PgPool, message: &str) -> Value {
    sqlx::query_scalar("select chips from messages where id = $1::uuid")
        .bind(message)
        .fetch_one(pg)
        .await
        .unwrap()
}

async fn channel_chips_of(pg: &PgPool, message: &str) -> Value {
    sqlx::query_scalar("select chips from channel_messages where id = $1::uuid")
        .bind(message)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// The row's created_at, as the files route reports a touch.
async fn created_iso(pg: &PgPool, message: &str) -> String {
    let ms: i64 = sqlx::query_scalar(
        "select (trunc(extract(epoch from created_at) * 1000))::bigint \
         from messages where id = $1::uuid",
    )
    .bind(message)
    .fetch_one(pg)
    .await
    .unwrap();
    talaria_api::agent_auth::epoch_ms_to_iso(ms)
}

/// How many chips on the row name this href.
fn naming(chips: &Value, href: &str) -> usize {
    chips.as_array().map_or(0, |all| {
        all.iter()
            .filter(|c| c["href"].as_str() == Some(href))
            .count()
    })
}

fn is_empty(chips: &Value) -> bool {
    chips.as_array().is_none_or(Vec::is_empty)
}

/// The session's files as one person's pane reads them.
async fn files(state: &AppState, cookie: &str, conv: &str) -> Vec<Value> {
    let (status, body) = call_json(
        state,
        "GET",
        &format!("/api/conversations/{conv}/files"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["files"].as_array().expect("files is an array").clone()
}

fn keys(files: &[Value]) -> Vec<String> {
    files
        .iter()
        .filter_map(|f| f["key"].as_str().map(str::to_string))
        .collect()
}

fn hex(n: usize) -> String {
    let mut out = String::new();
    while out.len() < n {
        out.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    out.truncate(n);
    out
}

// ── The frame builder ───────────────────────────────────────────────────────

/// A JSON string's body as Python's `json.dumps` writes it. Both forms escape
/// `"`, `\` and the control characters (the short escapes where they exist);
/// `ensure_ascii` also writes every character outside printable ASCII as
/// `\uXXXX` — UTF-16 units, lowercase hex.
fn py_string(s: &str, ensure_ascii: bool) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            ' '..='~' => out.push(c),
            c if !ensure_ascii => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out
}

/// `json.dumps(value, ensure_ascii=…)` with Python's default separators —
/// `", "` between items, `": "` after a key — keys in insertion order.
fn py_dumps(v: &Value, ensure_ascii: bool) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => format!("\"{}\"", py_string(s, ensure_ascii)),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|x| py_dumps(x, ensure_ascii))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, x)| format!(
                    "\"{}\": {}",
                    py_string(k, ensure_ascii),
                    py_dumps(x, ensure_ascii)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The plugin's `_clamp`: the first 4000 characters.
fn clamp(s: &str) -> String {
    s.chars().take(4_000).collect()
}

/// The `result` the plugin posts for an answer the toolkit gave.
fn wire_result(answer: &Value) -> String {
    let text = serde_json::to_string_pretty(answer).expect("plain data");
    clamp(&py_dumps(&json!({ "result": text }), true))
}

/// One tool event exactly as the talaria-events plugin posts it.
fn frame(tool: &str, args: &Value, answer: &Value, session: &str) -> Value {
    json!({
        "sessionId": session,
        "toolName": format!("mcp__talaria__{tool}"),
        "toolCallId": format!("call_{}", hex(24)),
        "status": "completed",
        "args": clamp(&py_dumps(args, false)),
        "turnId": "",
        "at": talaria_api::agent_auth::now_ms(),
        "result": wire_result(answer),
    })
}

async fn post_frame(state: &AppState, key: &str, frame: Value) {
    let (status, body) =
        agent_call(state, key, "POST", "/api/agents/tool-events", Some(frame)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], json!(true), "{body}");
}

/// Markdown of exactly `len` characters: headings, bullets and a table, the
/// shape an agent's brief has. Section headings are unique, so an edit can
/// anchor on one.
fn markdown(len: usize) -> String {
    let mut out = String::new();
    let mut n = 0;
    while out.chars().count() < len {
        n += 1;
        out.push_str(&format!(
            "## Section {n}\n\n- the first point of section {n}, with a [link](https://example.com/{n})\n\
             - a second point that says a little more about it\n\n| item | owner |\n|---|---|\n| row {n} | ops |\n\n"
        ));
    }
    out.chars().take(len).collect()
}

/// An agent write's answer: the artifact without its body, id first — and so
/// short on the wire that nothing was cut.
fn assert_slim(what: &str, answer: &Value) {
    let artifact = answer["artifact"].as_object().expect("an artifact");
    assert_eq!(
        artifact.keys().map(String::as_str).collect::<Vec<_>>(),
        vec!["id", "kind", "title", "updatedAt"],
        "{what}: {answer}"
    );
    let wire = wire_result(answer);
    assert!(
        wire.chars().count() < 1_500 && wire.ends_with("\"}"),
        "{what}: {} chars on the wire",
        wire.chars().count()
    );
}

// ── The owner's sequence ────────────────────────────────────────────────────

struct Turn<'a> {
    state: &'a AppState,
    key: &'a str,
    /// The session id every frame carries.
    session: &'a str,
}

/// What the sequence made or changed.
struct Touched {
    created: String,
    second: String,
    sheet: String,
}

impl Turn<'_> {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, answer) = agent_call(self.state, self.key, method, path, body).await;
        assert_eq!(status, StatusCode::OK, "{method} {path}: {answer}");
        answer
    }

    async fn report(&self, tool: &str, args: Value, answer: &Value) {
        post_frame(
            self.state,
            self.key,
            frame(tool, &args, answer, self.session),
        )
        .await;
    }

    async fn read(&self, doc: &str) {
        let answer = self
            .call("GET", &format!("/api/artifacts/{doc}"), None)
            .await;
        assert!(
            answer["artifact"]["body"].is_string(),
            "a read answers the body"
        );
        self.report("get_document", json!({ "documentId": doc }), &answer)
            .await;
    }

    async fn create(&self, title: &str, body: &str) -> String {
        let answer = self
            .call(
                "POST",
                "/api/artifacts",
                Some(json!({ "kind": "doc", "title": title, "body": body })),
            )
            .await;
        assert_slim("create", &answer);
        let id = answer["artifact"]["id"]
            .as_str()
            .expect("an id")
            .to_string();
        self.report(
            "create_document",
            json!({ "title": title, "markdown": body }),
            &answer,
        )
        .await;
        id
    }

    async fn edit(&self, doc: &str, old: &str, new: &str) {
        let edits = json!([{ "oldString": old, "newString": new }]);
        let answer = self
            .call(
                "POST",
                &format!("/api/artifacts/{doc}/edit"),
                Some(json!({ "edits": edits })),
            )
            .await;
        self.report(
            "edit_document",
            json!({ "documentId": doc, "edits": edits }),
            &answer,
        )
        .await;
    }

    async fn rewrite_sheet(&self, sheet: &str, rows: &[Vec<String>]) {
        let grid = serde_json::to_string(rows).expect("plain data");
        let answer = self
            .call(
                "PUT",
                &format!("/api/artifacts/{sheet}"),
                Some(json!({ "body": grid })),
            )
            .await;
        assert_slim("rewrite", &answer);
        self.report(
            "update_document",
            json!({ "documentId": sheet, "rows": rows }),
            &answer,
        )
        .await;
    }

    /// The owner's report, step by step: two reads, a long create and its
    /// edit, a second create edited 2 s later and again 4 s after that, and a
    /// long rewrite of an existing sheet. Each call is followed by the frame
    /// its plugin posts.
    async fn owner_sequence(&self, doc_a: &str, doc_b: &str, sheet: &str) -> Touched {
        self.read(doc_a).await;
        self.read(doc_b).await;
        let created = self.create("Launch brief", &markdown(4_739)).await;
        self.edit(&created, "## Section 1\n", "## Section 1 — revised\n")
            .await;
        let second = self
            .create("Launch notes", "# Notes\n\nfirst line\n\nsecond line\n")
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        self.edit(&second, "first line", "first line, edited").await;
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        self.edit(&second, "second line", "second line, edited again")
            .await;
        let rows: Vec<Vec<String>> = (0..500)
            .map(|r| {
                vec![
                    format!("Item {r}"),
                    format!("owner-{}", r % 7),
                    format!("2026-11-{:02}", 1 + r % 28),
                    format!("note on item {r}"),
                ]
            })
            .collect();
        self.rewrite_sheet(sheet, &rows).await;
        // The routes' own chips land detached, a beat after each answer.
        tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
        Touched {
            created,
            second,
            sheet: sheet.to_string(),
        }
    }
}

/// Docs A and B (org documents the agent may read) and an existing sheet the
/// agent may edit.
async fn existing_files(pg: &PgPool, owner: &str, model: &str) -> (String, String, String) {
    let a = artifact(pg, owner, "Pricing notes", "doc", &markdown(3_000)).await;
    let b = artifact(pg, owner, "Retry design", "doc", "a short design note").await;
    let sheet = artifact(
        pg,
        owner,
        "Launch tracker",
        "sheet",
        r#"[["item","owner"]]"#,
    )
    .await;
    grant_editor(pg, &sheet, model).await;
    (a, b, sheet)
}

fn href(id: &str) -> String {
    format!("/artifacts?a={id}")
}

// ── Run 1: one live turn ────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn one_live_turn_lists_every_file_the_agent_wrote_once() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "one").await;
    let (model, key) = fleet_agent(&state, "one").await;
    let (conv, row) = live_turn(&state, &owner, &model, "work").await;
    let session = attribute(&state.pg, &model, &conv).await;
    let (doc_a, doc_b, sheet) = existing_files(&state.pg, &owner, &model).await;

    let turn = Turn {
        state: &state,
        key: &key,
        session: &session,
    };
    let t = turn.owner_sequence(&doc_a, &doc_b, &sheet).await;

    let listed = files(&state, &cookie, &conv).await;
    assert_eq!(
        keys(&listed),
        vec![
            format!("artifact:{}", t.created),
            format!("artifact:{}", t.second),
            format!("artifact:{}", t.sheet),
        ],
        "every written file once, in the order it joined; the reads never: {listed:?}"
    );
    // The route's chip and the door's are one chip per file.
    let chips = chips_of(&state.pg, &row).await;
    for id in [&t.created, &t.second, &t.sheet] {
        assert_eq!(naming(&chips, &href(id)), 1, "{id}: {chips}");
    }
    for id in [&doc_a, &doc_b] {
        assert_eq!(
            naming(&chips, &href(id)),
            0,
            "a read lands nothing: {chips}"
        );
    }
    // The edits said what they did, on the same chip.
    let changes_on = |id: &str| {
        chips
            .as_array()
            .and_then(|all| all.iter().find(|c| c["href"] == json!(href(id))))
            .and_then(|c| c["changes"].as_array().map(Vec::len))
            .unwrap_or(0)
    };
    assert!(changes_on(&t.created) >= 1, "{chips}");
    assert!(changes_on(&t.second) >= 2, "both edits: {chips}");
}

// ── Run 2: the agent answering in three places ──────────────────────────────

#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn with_three_live_turns_every_file_lands_in_the_conversation_it_was_made_in() {
    let state = live_app_state().await;
    let (owner, cookie) = member(&state, "three").await;
    let (other, other_cookie) = member(&state, "three-other").await;
    let (model, key) = fleet_agent(&state, "three").await;
    let (work, work_row) = live_turn(&state, &owner, &model, "work").await;
    // The same agent, streaming a reply to somebody else, and in a channel.
    let (elsewhere, elsewhere_row) = live_turn(&state, &other, &model, "work").await;
    let (_channel, channel_row) = live_channel_reply(&state, &other, &model).await;
    let target = talaria_chips::live_landing_target(&state.pg, &model, None)
        .await
        .unwrap();
    assert!(
        matches!(target, talaria_chips::Target::Ambiguous { turns } if turns >= 2),
        "the setup: no unaddressed landing can choose — {target:?}"
    );
    let session = attribute(&state.pg, &model, &work).await;
    let (doc_a, doc_b, sheet) = existing_files(&state.pg, &owner, &model).await;

    // WHAT THE ROUTE ALONE DOES HERE: its own chip names no conversation, so
    // with three live rows it is dropped — the file lands nowhere until its
    // frame arrives. This is all today's door could do too, since it read no
    // conversation out of an `api-…` session.
    let probe = agent_call(
        &state,
        &key,
        "POST",
        "/api/artifacts",
        Some(json!({ "kind": "doc", "title": "Probe", "body": markdown(4_739) })),
    )
    .await;
    assert_eq!(probe.0, StatusCode::OK, "{}", probe.1);
    let probe_id = probe.1["artifact"]["id"]
        .as_str()
        .expect("an id")
        .to_string();
    tokio::time::sleep(std::time::Duration::from_millis(1_500)).await;
    assert_eq!(
        naming(&chips_of(&state.pg, &work_row).await, &href(&probe_id)),
        0
    );
    assert!(
        keys(&files(&state, &cookie, &work).await).is_empty(),
        "nothing lands without the frame"
    );

    let turn = Turn {
        state: &state,
        key: &key,
        session: &session,
    };
    let t = turn.owner_sequence(&doc_a, &doc_b, &sheet).await;

    // The Work session's pane: every file written there, touched at the
    // Work turn, for the person who asked.
    let listed = files(&state, &cookie, &work).await;
    assert_eq!(
        keys(&listed),
        vec![
            format!("artifact:{}", t.created),
            format!("artifact:{}", t.second),
            format!("artifact:{}", t.sheet),
        ],
        "{listed:?}"
    );
    let at = created_iso(&state.pg, &work_row).await;
    for f in &listed {
        assert_eq!(f["source"], json!("touched"), "{f}");
        assert_eq!(f["lastTouchedAt"], json!(at), "{f}");
        assert_eq!(f["touchedFor"], json!(owner), "{f}");
    }
    let chips = chips_of(&state.pg, &work_row).await;
    for id in [&t.created, &t.second, &t.sheet] {
        assert_eq!(naming(&chips, &href(id)), 1, "{id}: {chips}");
    }
    for id in [&doc_a, &doc_b] {
        assert_eq!(naming(&chips, &href(id)), 0, "{chips}");
    }
    // The edits' change lists came through the door, the only lander here.
    assert!(
        chips
            .as_array()
            .is_some_and(|all| all.iter().any(|c| c["href"] == json!(href(&t.second))
                && c["changes"].as_array().is_some_and(|r| r.len() >= 2))),
        "{chips}"
    );

    // And nowhere else.
    assert!(files(&state, &other_cookie, &elsewhere).await.is_empty());
    assert!(is_empty(&chips_of(&state.pg, &elsewhere_row).await));
    assert!(is_empty(&channel_chips_of(&state.pg, &channel_row).await));
}

// ── Live controls ───────────────────────────────────────────────────────────

/// AN `api-…` SESSION WITH NO ATTRIBUTION ROW BEHAVES AS TODAY: unaddressed —
/// on the agent's one live turn, and nowhere while it has several.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_session_with_no_attribution_keeps_the_unaddressed_rule() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "unknown").await;
    let (other, _) = member(&state, "unknown-other").await;
    let (model, key) = fleet_agent(&state, "unknown").await;
    let (_, row) = live_turn(&state, &owner, &model, "work").await;
    let unknown = format!("api-{}", hex(16));
    let doc = |id: &str| json!({ "artifact": { "id": id, "kind": "doc", "title": "Brief", "updatedAt": "2026-11-20T22:30:00.000Z" } });

    // One live turn: it lands there, as it always did.
    let one = uuid::Uuid::new_v4().to_string();
    post_frame(
        &state,
        &key,
        frame(
            "create_document",
            &json!({ "title": "Brief" }),
            &doc(&one),
            &unknown,
        ),
    )
    .await;
    assert_eq!(naming(&chips_of(&state.pg, &row).await, &href(&one)), 1);

    // Two: dropped, never guessed.
    let (_, other_row) = live_turn(&state, &other, &model, "work").await;
    let two = uuid::Uuid::new_v4().to_string();
    post_frame(
        &state,
        &key,
        frame(
            "create_document",
            &json!({ "title": "Brief" }),
            &doc(&two),
            &unknown,
        ),
    )
    .await;
    assert_eq!(naming(&chips_of(&state.pg, &row).await, &href(&two)), 0);
    assert_eq!(
        naming(&chips_of(&state.pg, &other_row).await, &href(&two)),
        0
    );
}

/// A SESSION ANOTHER AGENT RECORDED, OR A ROW THAT POINTS AT ANOTHER AGENT'S
/// CONVERSATION, NAMES NOTHING: the frame lands in neither agent's threads.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn another_agents_session_lands_nothing() {
    let state = live_app_state().await;
    let (owner, _) = member(&state, "foreign").await;
    let (other, _) = member(&state, "foreign-other").await;
    let (theirs, _) = fleet_agent(&state, "foreign-theirs").await;
    let (model, key) = fleet_agent(&state, "foreign-mine").await;
    let (their_conv, their_row) = live_turn(&state, &owner, &theirs, "work").await;
    let their_session = attribute(&state.pg, &theirs, &their_conv).await;
    // A row of this agent's own that names the other agent's conversation —
    // the join refuses it.
    let crossed = format!("api-{}", hex(16));
    attribute_key(&state.pg, &model, &crossed, &their_conv).await;
    let (_, mine) = live_turn(&state, &owner, &model, "work").await;
    let (_, mine_too) = live_turn(&state, &other, &model, "work").await;

    for session in [&their_session, &crossed] {
        let id = uuid::Uuid::new_v4().to_string();
        let answer = json!({ "artifact": { "id": id, "kind": "doc", "title": "Brief", "updatedAt": "2026-11-20T22:30:00.000Z" } });
        post_frame(
            &state,
            &key,
            frame(
                "create_document",
                &json!({ "title": "Brief" }),
                &answer,
                session,
            ),
        )
        .await;
        for row in [&their_row, &mine, &mine_too] {
            assert_eq!(
                naming(&chips_of(&state.pg, row).await, &href(&id)),
                0,
                "{session}: nothing on {row}"
            );
        }
    }
}

// ── The resolver ────────────────────────────────────────────────────────────

/// ONE RULE FROM A SESSION TO A CONVERSATION: a UUID that is this agent's own
/// conversation, or a key this agent's chat recorded against a conversation
/// of its own — nothing else.
#[tokio::test]
#[ignore = "needs a live dev database and redis (ui/.env)"]
async fn a_session_names_only_the_agents_own_conversation() {
    let state = live_app_state().await;
    let pg = &state.pg;
    let (owner, _) = member(&state, "resolver").await;
    let (model, _) = fleet_agent(&state, "resolver").await;
    let (theirs, _) = fleet_agent(&state, "resolver-theirs").await;
    let (conv, _) = live_turn(&state, &owner, &model, "work").await;
    let (their_conv, _) = live_turn(&state, &owner, &theirs, "work").await;
    let mine = attribute(pg, &model, &conv).await;
    let their_key = attribute(pg, &theirs, &their_conv).await;
    let crossed = format!("api-{}", hex(16));
    attribute_key(pg, &model, &crossed, &their_conv).await;

    let m: &str = &model;
    let named = |s: String| async move { session_conversation(pg, m, &s).await.unwrap() };
    assert_eq!(
        named(conv.clone()).await,
        Some(conv.clone()),
        "its own uuid"
    );
    assert_eq!(
        named(mine.clone()).await,
        Some(conv.clone()),
        "its own api- key"
    );
    assert_eq!(
        named(format!("api-{}", hex(16))).await,
        None,
        "an unknown key"
    );
    assert_eq!(named(their_key.clone()).await, None, "another agent's key");
    assert_eq!(
        named(crossed.clone()).await,
        None,
        "a row naming another agent's thread"
    );
    assert_eq!(
        named(their_conv.clone()).await,
        None,
        "another agent's uuid"
    );
    assert_eq!(named(String::new()).await, None, "nothing");

    // The chips' reading of a frame: a uuid first, else the session, else
    // the parent's session.
    let hint =
        |body: Value| async move { talaria_chips::session_hint(pg, m, &body).await.unwrap() };
    assert_eq!(hint(json!({ "sessionId": mine })).await, Some(conv.clone()));
    assert_eq!(
        hint(json!({ "parentSessionId": mine })).await,
        Some(conv.clone())
    );
    assert_eq!(
        hint(json!({ "conversationId": their_conv, "sessionId": mine })).await,
        Some(their_conv.clone()),
        "a uuid the frame names is the address (the landing then checks it is this agent's)"
    );
    assert_eq!(hint(json!({ "sessionId": their_key })).await, None);
    assert_eq!(
        hint(json!({ "sessionId": "api-0000000000000000" })).await,
        None
    );
    assert_eq!(hint(json!({})).await, None);
}
