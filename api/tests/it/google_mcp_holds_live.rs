// Live proof of the MCP gateway's HOLD for Google's Workspace MCP servers
// (the pack's rows), driven through the real router as an agent would drive
// it — `POST /api/mcp/gw/google-<product>` with the agent's own key — and
// through the chat chip a person approves (cargo test -- --ignored; ci.yml's
// api job runs it against the migrated scratch database and redis):
//
//   · a held call makes ONE `mcp_call` row naming its id, lands its approval
//     chip, and nothing reaches Google (a loopback stand-in here);
//   · the owner's chip approval runs it once and leaves a grant keyed
//     `<server>/<tool>`, bound to the people the call addressed (D7);
//   · that grant runs exactly one later call to those people, at once, and
//     not one to anyone else; the call after it is held again;
//   · an agent identity (admin-approved) is held even with a live grant on
//     its conversation;
//   · TRASH or SPAM in a label argument is refused at the gate, whatever the
//     tool's policy, with nothing queued or sent;
//   · an admin's ALLOW relays for an agent identity and never reaches a
//     personal assistant, whose write stays held for its owner;
//   · a personal hold asked for in a conversation with someone other than
//     the owner is refused with no row.
//
// The stand-in answers with the MCP specification's tool-result shape,
// labelled STUB — no authenticated call to Google's servers has ever been
// made here (nobody is enrolled in the Developer Preview).
//
//   source ui/.env && cargo test -p talaria-api --test it google_mcp_holds_live:: -- --ignored
//
// THE PACK'S ROWS ARE NAMED BY THEIR PRODUCT (`google-calendar`,
// `google-gmail`), so this test owns those two names on the database it runs
// against: it refuses to start when either exists and was not made by it.

use crate::support::{live_app_state, minted_member_with_email};
use axum::body::Body;
use axum::http::Request;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::sync::{Arc, Mutex};
use talaria_api::google::mcp::catalog::read_tools_list;
use talaria_state::AppState;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

/// Marks every row this test makes that has no user to cascade from.
const TAG: &str = "gmcphold";
const MADE_BY: &str = "test:gmcphold";

struct Agent {
    model: String,
    key: String,
}

/// A loopback stand-in for Google's MCP servers: every request to its MCP
/// endpoint kept as (path, authorization, JSON body), each `tools/call`
/// answered with the MCP specification's result shape, labelled STUB.
struct Stub {
    origin: String,
    seen: Arc<Mutex<Vec<(String, String, Value)>>>,
}

impl Stub {
    fn take(&self) -> Vec<(String, String, Value)> {
        self.seen.lock().unwrap().drain(..).collect()
    }

    /// Nothing reached the stand-in since the last look.
    #[track_caller]
    fn nothing_sent(&self, why: &str) {
        let seen = self.take();
        assert!(seen.is_empty(), "{why}: {seen:?}");
    }
}

async fn stub() -> Stub {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let seen: Arc<Mutex<Vec<(String, String, Value)>>> = Arc::default();
    let kept = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let kept = kept.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let (head, body) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len || n == 0 {
                            break (text[..end].to_string(), text[end + 4..].to_string());
                        }
                    } else if n == 0 {
                        break (text, String::new());
                    }
                };
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                let auth = head
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("authorization")
                            .then(|| v.trim().to_string())
                    })
                    .unwrap_or_default();
                let rpc: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                let tool = rpc["params"]["name"].as_str().unwrap_or("").to_string();
                let answer = json!({
                    "jsonrpc": "2.0",
                    "id": rpc.get("id").cloned().unwrap_or(Value::Null),
                    "result": {
                        "content": [{ "type": "text", "text": format!("STUB (the MCP specification's shape, not a Google capture): {tool} ran") }],
                        "isError": false,
                    },
                })
                .to_string();
                // Only the MCP endpoint is a call to Google's server. On a
                // shared host another process's health poll can land on this
                // ephemeral port (`GET /`, seen here); it is logged, not kept.
                if path == "/mcp/v1" {
                    kept.lock().unwrap().push((path, auth, rpc));
                } else {
                    eprintln!("[stub] ignored a request that is not to the MCP endpoint: {head:?}");
                }
                let out = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                    answer.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
            });
        }
    });
    Stub { origin, seen }
}

/// Everything this test made, gone — at the start (a failed run's leftovers)
/// and at the end. Users cascade their conversations, grants and rows; the
/// agents cascade their keys, principals and identities; the pack rows
/// cascade their assignments and overrides.
async fn sweep(pg: &PgPool) {
    for sql in [
        "delete from google_pending_actions where agent_model like 'gmcphold-%'",
        "delete from chat_approvals where agent_model like 'gmcphold-%'",
        "delete from mcp_tool_calls where agent like 'gmcphold-%'",
        "delete from conversations where agent_model like 'gmcphold-%'",
        "delete from agent_defs where slug like 'gmcphold-%'",
        "delete from mcp_servers where created_by = 'test:gmcphold'",
        "delete from users where sub like 'gmcphold:%'",
    ] {
        sqlx::query(sql).execute(pg).await.expect(sql);
    }
}

/// A pack row as the admin route makes it, with Google's catalog for the
/// product as captured with no credential on 2026-11-17.
async fn pack_row(pg: &PgPool, product: &str, tools_list: &str) -> String {
    let catalog = Value::Array(read_tools_list(200, tools_list).expect("a captured catalog"));
    sqlx::query_scalar(
        "insert into mcp_servers (name, label, url, auth_mode, all_agents, enabled, tools, created_by) \
         values ($1, $2, $3, 'google', false, true, $4, $5) returning id::text",
    )
    .bind(format!("google-{product}"))
    .bind(format!("Google {product} (Google MCP, Developer Preview)"))
    .bind(format!("https://{product}mcp.googleapis.com/mcp/v1"))
    .bind(catalog)
    .bind(MADE_BY)
    .fetch_one(pg)
    .await
    .expect("a pack row")
}

/// An agent with its own key, its principal, and both pack rows assigned.
async fn agent(
    state: &AppState,
    slug: &str,
    kind: &str,
    owner: Option<&str>,
    servers: &[&str],
) -> Agent {
    let pg = &state.pg;
    let sb = state.secretbox().await.expect("the secretbox loads");
    let model = format!("{slug}-ops");
    let id: String = sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name, owner_user_id) \
         values ($1, 'ops', $2, $1, $3::uuid) returning id::text",
    )
    .bind(slug)
    .bind(&model)
    .bind(owner)
    .fetch_one(pg)
    .await
    .expect("an agent");
    let key = format!(
        "tak_{}",
        &talaria_api::auth::sha256_hex(&format!("{TAG}-key-{slug}"))[..48]
    );
    sqlx::query("insert into agent_keys (agent_id, key_hash, key_enc) values ($1::uuid, $2, $3)")
        .bind(&id)
        .bind(talaria_api::auth::sha256_hex(&key))
        .bind(sb.seal(&key).unwrap())
        .execute(pg)
        .await
        .expect("its key");
    sqlx::query(
        "insert into agent_principals (agent_model, principal_kind, principal_user_id) \
         values ($1, $2, $3::uuid)",
    )
    .bind(&model)
    .bind(kind)
    .bind(owner)
    .execute(pg)
    .await
    .expect("its principal");
    for server in servers {
        sqlx::query("insert into mcp_server_agents (server_id, agent_model) values ($1::uuid, $2)")
            .bind(server)
            .bind(&model)
            .execute(pg)
            .await
            .expect("its assignment");
    }
    Agent { model, key }
}

/// A conversation of `agent` with `user`, mid-turn (an assistant reply
/// streaming now), with `unlocked` as its grants. Returns its id.
async fn speaking(pg: &PgPool, agent: &Agent, user: &str, unlocked: Value) -> String {
    let conv: String = sqlx::query_scalar(
        "insert into conversations (user_id, agent_model, title, unlocked_tools) \
         values ($1::uuid, $2, 'google mcp holds', $3) returning id::text",
    )
    .bind(user)
    .bind(&agent.model)
    .bind(unlocked)
    .fetch_one(pg)
    .await
    .expect("a conversation");
    sqlx::query(
        "insert into messages (conversation_id, seq, role, content) values ($1::uuid, 1, 'user', 'please')",
    )
    .bind(&conv)
    .execute(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into messages (conversation_id, seq, role, content, status, streamed_at) \
         values ($1::uuid, 2, 'assistant', '', 'streaming', now())",
    )
    .bind(&conv)
    .execute(pg)
    .await
    .unwrap();
    conv
}

/// One JSON-RPC message through the gateway as `agent`: (HTTP status, body).
async fn gw(state: &AppState, server: &str, agent: &Agent, rpc: Value) -> (u16, Value) {
    let req = Request::builder()
        .method("POST")
        .uri(format!("/api/mcp/gw/{server}"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("x-api-key", &agent.key)
        .header("x-agent-name", &agent.model)
        .body(Body::from(rpc.to_string()))
        .unwrap();
    let res = talaria_api::routes::router(state.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = res.status().as_u16();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    let body = serde_json::from_str(&text).unwrap_or_else(|_| {
        // An SSE-framed relay: the last `data:` line.
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter_map(|d| serde_json::from_str(d.trim()).ok())
            .next_back()
            .unwrap_or(Value::Null)
    });
    (status, body)
}

fn call(id: i64, tool: &str, args: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": tool, "arguments": args } })
}

/// The text of a tool result.
fn text_of(body: &Value) -> String {
    body["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The approval id a held answer names.
fn held_id(body: &Value) -> String {
    let text = text_of(body);
    let at = text
        .find("Queued for approval as ")
        .unwrap_or_else(|| panic!("not a held answer: {body}"));
    text[at + "Queued for approval as ".len()..]
        .split(|c: char| c != '-' && !c.is_ascii_hexdigit())
        .next()
        .unwrap()
        .to_string()
}

async fn rows_of(pg: &PgPool, agent: &Agent) -> Vec<(String, String, String, bool, Value)> {
    sqlx::query_as(
        "select id::text, status, principal_kind, is_org, payload from google_pending_actions \
         where agent_model = $1 and kind = 'mcp_call' order by created_at",
    )
    .bind(&agent.model)
    .fetch_all(pg)
    .await
    .unwrap()
}

async fn grants(pg: &PgPool, conv: &str) -> Value {
    sqlx::query_scalar("select unlocked_tools from conversations where id = $1::uuid")
        .bind(conv)
        .fetch_one(pg)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (DATABASE_URL, REDIS_URL)"]
async fn google_mcp_holds_queue_for_a_person_and_never_reach_google_before() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg).await;
    let foreign: Vec<String> = sqlx::query_scalar(
        "select name from mcp_servers where name in ('google-calendar', 'google-gmail') \
         and created_by is distinct from 'test:gmcphold'",
    )
    .fetch_all(&pg)
    .await
    .unwrap();
    assert!(
        foreign.is_empty(),
        "this test owns the pack rows {foreign:?} by name; run it on a database without them (CI's scratch database, or remove them)"
    );

    // Google's servers are a loopback stand-in for this process. The
    // override is DEBUG-only and loopback-only (talaria-google-mcp's
    // `pack::origin`); one test in this module, so the process environment
    // is this test's alone.
    let google = stub().await;
    // SAFETY: the only test in this module; CI runs one module at a time.
    unsafe {
        std::env::set_var(
            talaria_api::google::mcp::pack::TEST_ORIGINS_ENV,
            format!("calendar={0},gmail={0}", google.origin),
        );
        std::env::set_var("TALARIA_FETCH_ALLOW_HOSTS", "127.0.0.1");
    }

    // People: the assistant's owner (with a session, for the chip), and
    // someone else.
    let (owner, owner_cookie) =
        minted_member_with_email(&state, TAG, "gmcphold-owner@example.test").await;
    let (other, _) = minted_member_with_email(&state, TAG, "gmcphold-other@example.test").await;
    let sb = state.secretbox().await.expect("the secretbox loads");
    let later =
        talaria_api::agent_auth::epoch_ms_to_iso(talaria_api::agent_auth::now_ms() + 50 * 60_000);
    let scope = "openid email https://www.googleapis.com/auth/calendar.events https://www.googleapis.com/auth/gmail.modify";
    // The owner's own Google grant, its access token cached and valid, so
    // no refresh reaches Google.
    sqlx::query(
        "insert into google_connections (user_id, google_sub, email, scope, refresh_token_enc, \
         access_token_enc, access_expires_at) values ($1::uuid, 'g-gmcphold-owner', \
         'gmcphold-owner@example.test', $2, $3, $4, $5::timestamptz)",
    )
    .bind(&owner)
    .bind(scope)
    .bind(sb.seal("1//gmcphold-refresh-owner").unwrap())
    .bind(sb.seal("ya29.gmcphold-owner").unwrap())
    .bind(&later)
    .execute(&pg)
    .await
    .expect("the owner's grant");

    let calendar = pack_row(
        &pg,
        "calendar",
        include_str!("../../crates/talaria-google-mcp/fixtures/tools-list/calendarmcp.json"),
    )
    .await;
    let gmail = pack_row(
        &pg,
        "gmail",
        include_str!("../../crates/talaria-google-mcp/fixtures/tools-list/gmailmcp.json"),
    )
    .await;
    // A personal assistant of the owner, and an agent identity with its
    // own Google grant (admin-approved, like the org account).
    let pa = agent(
        &state,
        "gmcphold-pa",
        "owner",
        Some(&owner),
        &[&calendar, &gmail],
    )
    .await;
    let ident = agent(&state, "gmcphold-id", "agent", None, &[&calendar, &gmail]).await;
    sqlx::query(
        "insert into agent_google_connections (agent_model, google_sub, email, scope, \
         refresh_token_enc, access_token_enc, access_expires_at) \
         values ($1, 'g-gmcphold-id', 'gmcphold-id@example.test', $2, $3, $4, $5::timestamptz)",
    )
    .bind(&ident.model)
    .bind(scope)
    .bind(sb.seal("1//gmcphold-refresh-id").unwrap())
    .bind(sb.seal("ya29.gmcphold-id").unwrap())
    .bind(&later)
    .execute(&pg)
    .await
    .expect("the identity's grant");

    // ── A personal assistant's write is held for its owner ──────────────
    let conv = speaking(&pg, &pa, &owner, json!([])).await;
    let standup = json!({
        "summary": "Standup",
        "startTime": "2026-11-18T09:00:00Z",
        "endTime": "2026-11-18T09:15:00Z",
        "attendees": [{ "email": "a@example.test" }],
    });
    let (status, body) = gw(
        &state,
        "google-calendar",
        &pa,
        call(1, "create_event", standup.clone()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["result"]["isError"], false, "{body}");
    let first = held_id(&body);
    assert!(text_of(&body).contains("Not done yet"), "{body}");
    google.nothing_sent("nothing reaches Google for a held call");
    let rows = rows_of(&pg, &pa).await;
    assert_eq!(rows.len(), 1);
    let (id, status, principal, is_org, payload) = &rows[0];
    assert_eq!(id, &first);
    assert_eq!(
        (status.as_str(), principal.as_str(), *is_org),
        ("pending", "owner", false)
    );
    assert_eq!(payload["tool"], "create_event");
    assert_eq!(payload["server"], "google-calendar");
    assert_eq!(payload["arguments"], standup);
    // Its approval chip, in the owner's conversation.
    let (chip, chip_owner): (String, Option<String>) = sqlx::query_as(
        "select id::text, owner_user_id::text from chat_approvals where external_id = $1",
    )
    .bind(&first)
    .fetch_one(&pg)
    .await
    .expect("the approval chip landed");
    assert_eq!(chip_owner.as_deref(), Some(owner.as_str()));

    // ── The owner approves the chip: it runs once, and leaves a grant bound
    //    to the people it addressed ─────────────────────────────────────
    let (status, out) = crate::support::call_json(
        &state,
        "POST",
        &format!("/api/chat/chips/approvals/{chip}"),
        Some(&owner_cookie),
        Some(json!({ "decision": "approve" })),
    )
    .await;
    assert_eq!(status.as_u16(), 200, "{out}");
    assert_eq!(
        out["tools"],
        json!(["google-calendar/create_event"]),
        "{out}"
    );
    let sent = google.take();
    assert_eq!(sent.len(), 1, "exactly one call reached Google");
    let (path, auth, rpc) = &sent[0];
    assert_eq!(path, "/mcp/v1");
    assert_eq!(auth, "Bearer ya29.gmcphold-owner");
    assert_eq!(rpc["method"], "tools/call");
    assert_eq!(rpc["params"]["name"], "create_event");
    assert_eq!(rpc["params"]["arguments"], standup);
    let grant = grants(&pg, &conv).await;
    assert_eq!(grant[0]["tool"], "google-calendar/create_event", "{grant}");
    assert_eq!(grant[0]["usesLeft"], 1, "{grant}");
    assert_eq!(grant[0]["recipients"], json!(["a@example.test"]), "{grant}");

    // ── The grant covers no one else: an outside attendee is held, and the
    //    grant is not spent ───────────────────────────────────────────────
    let outsider = json!({ "summary": "Standup + guest", "attendees": [{ "email": "a@example.test" }, { "email": "outsider@elsewhere.test" }] });
    let (_, body) = gw(
        &state,
        "google-calendar",
        &pa,
        call(2, "create_event", outsider),
    )
    .await;
    let held_outsider = held_id(&body);
    google.nothing_sent("an addressee the owner did not approve is held");
    assert_eq!(grants(&pg, &conv).await[0]["usesLeft"], 1);

    // ── One later call to the same people runs at once, in Google's words;
    //    the grant is spent ───────────────────────────────────────────────
    let again =
        json!({ "summary": "Standup, moved", "attendees": [{ "email": "A@example.test" }] });
    let (status, body) = gw(
        &state,
        "google-calendar",
        &pa,
        call(3, "create_event", again.clone()),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(
        text_of(&body).starts_with("STUB"),
        "the unlocked call answers Google's result: {body}"
    );
    let sent = google.take();
    assert_eq!(sent.len(), 1, "the unlocked call ran once");
    assert_eq!(sent[0].2["params"]["arguments"], again);
    assert_eq!(grants(&pg, &conv).await, json!([]), "one use");
    let unlocked_row = rows_of(&pg, &pa)
        .await
        .into_iter()
        .find(|r| r.4["arguments"] == again)
        .expect("the unlocked call has its row");
    assert_eq!(unlocked_row.1, "executed");
    // And the call after that is held again.
    let (_, body) = gw(
        &state,
        "google-calendar",
        &pa,
        call(
            4,
            "create_event",
            json!({ "summary": "Third", "attendees": [{ "email": "a@example.test" }] }),
        ),
    )
    .await;
    held_id(&body);
    google.nothing_sent("nothing sent");

    // ── TRASH or SPAM in a label argument: refused at the gate, held tool or
    //    not, with nothing queued or sent ───────────────────────────────
    let before = rows_of(&pg, &pa).await.len();
    for (n, (tool, args)) in [
        (
            "unlabel_thread",
            json!({ "threadId": "t1", "labelIds": ["TRASH"] }),
        ),
        (
            "label_message",
            json!({ "messageId": "m1", "labelIds": ["SPAM"] }),
        ),
        (
            "update_message_labels",
            json!({ "messageId": "m1", "removeLabelIds": ["TRASH"] }),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = gw(&state, "google-gmail", &pa, call(10 + n as i64, tool, args)).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["error"]["code"], -32602, "{tool}: {body}");
        assert!(
            body["error"]["message"]
                .as_str()
                .unwrap_or("")
                .contains("never lets an agent move mail to or out of the trash or spam"),
            "{body}"
        );
    }
    assert_eq!(rows_of(&pg, &pa).await.len(), before, "nothing queued");
    google.nothing_sent("nothing sent");

    // ── An admin's ALLOW on a label tool: the agent identity's call is
    //    relayed; the personal assistant's stays held for its owner; and
    //    TRASH stays refused under the allow ─────────────────────────────
    sqlx::query("insert into mcp_tool_policies (server_id, tool, policy) values ($1::uuid, 'label_thread', 'allow')")
        .bind(&gmail)
        .execute(&pg)
        .await
        .unwrap();
    let starred = json!({ "threadId": "t1", "labelIds": ["STARRED"] });
    let (_, body) = gw(
        &state,
        "google-gmail",
        &ident,
        call(20, "label_thread", starred.clone()),
    )
    .await;
    assert!(
        text_of(&body).starts_with("STUB"),
        "an allowed tool relays: {body}"
    );
    let sent = google.take();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].1, "Bearer ya29.gmcphold-id");
    assert!(
        rows_of(&pg, &ident).await.is_empty(),
        "an allowed call queues nothing"
    );
    let (_, body) = gw(
        &state,
        "google-gmail",
        &pa,
        call(21, "label_thread", starred),
    )
    .await;
    held_id(&body);
    google.nothing_sent("an admin's allow never reaches a personal assistant");
    let (_, body) = gw(
        &state,
        "google-gmail",
        &ident,
        call(
            22,
            "label_thread",
            json!({ "threadId": "t1", "labelIds": ["TRASH"] }),
        ),
    )
    .await;
    assert_eq!(body["error"]["code"], -32602, "{body}");
    google.nothing_sent("nothing sent");

    // ── An agent identity never rides an unlock: held for an admin even with
    //    a live grant for the tool on its conversation, which stays unspent ─
    let live_grant = json!([talaria_chips::unlock_grant(
        "google-calendar/create_event",
        Some(&[]),
        talaria_api::agent_auth::now_ms()
    )]);
    let ident_conv = speaking(&pg, &ident, &owner, live_grant).await;
    let (_, body) = gw(
        &state,
        "google-calendar",
        &ident,
        call(30, "create_event", json!({ "summary": "Org thing" })),
    )
    .await;
    let ident_held = held_id(&body);
    assert!(text_of(&body).contains("an admin approves it"), "{body}");
    google.nothing_sent("nothing sent");
    let ident_rows = rows_of(&pg, &ident).await;
    assert_eq!(ident_rows.len(), 1);
    assert_eq!(ident_rows[0].0, ident_held);
    assert_eq!((ident_rows[0].2.as_str(), ident_rows[0].3), ("agent", true));
    assert_eq!(
        grants(&pg, &ident_conv).await[0]["usesLeft"],
        1,
        "not spent"
    );

    // ── A personal hold asked for in a conversation with someone else is
    //    refused, with no row ───────────────────────────────────────────
    sqlx::query("update messages set status = 'done' where conversation_id = $1::uuid")
        .bind(&conv)
        .execute(&pg)
        .await
        .unwrap();
    speaking(&pg, &pa, &other, json!([])).await;
    let before = rows_of(&pg, &pa).await.len();
    let (_, body) = gw(
        &state,
        "google-calendar",
        &pa,
        call(40, "create_event", json!({ "summary": "Not yours" })),
    )
    .await;
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("not with the assistant's owner"),
        "{body}"
    );
    assert_eq!(rows_of(&pg, &pa).await.len(), before, "nothing queued");
    google.nothing_sent("nothing sent");

    // The outsider's call still waits for its owner, untouched.
    let waiting: String =
        sqlx::query_scalar("select status from google_pending_actions where id = $1::uuid")
            .bind(&held_outsider)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(waiting, "pending");

    unsafe {
        std::env::remove_var(talaria_api::google::mcp::pack::TEST_ORIGINS_ENV);
        std::env::remove_var("TALARIA_FETCH_ALLOW_HOSTS");
    }
    sweep(&pg).await;
}
