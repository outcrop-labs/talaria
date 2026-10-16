// Live-DB proof for the browser half of toolset governance: the PATCH that
// decides which toolsets an agent is denied, and the door a browsing agent
// posts its screenshot through.
//
// WHY A LIVE DATABASE: both halves are SQL guarantees wearing a route. The
// toolset list is a jsonb column read back by the roster — a column that
// saved under a different name still passes a route-only assertion. And an
// upload is reachable to a conversation's readers ONLY when its id sits in a
// message's `attachments` JSON: `can_access_upload` probes that column with
// `@>`, and `speaking_target` picks the row to append to by evaluating the
// streaming tier's interval arithmetic. Nothing but Postgres can answer
// either, which is why the chip and the attachment are asserted from the
// row rather than from the handler's return.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it browser_toolsets_live:: -- --ignored

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::conversations::{create_conversation, insert_user_message};
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use talaria_uploads::{UploadViewer, can_access_upload};
use tower::ServiceExt; // oneshot

use crate::support::{call_json, fabricate_user, live_app_state, sweep_user_rows};

const PREFIX: &str = "browser-toolsets-live";

/// A real 1×1 RGBA PNG. The plugin's job is to hand over bytes it read off
/// disk, so the door is proved with bytes that ARE a PNG rather than with a
/// blob that merely decodes — the store, the mime and the image chip all
/// claim this is an image, and one of them being wrong should show here.
const ONE_PIXEL_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

/// The decoded length the `uploads` row must record — derived, not a magic
/// 70, so the assertion stays true if the fixture PNG is ever swapped.
fn png_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(ONE_PIXEL_PNG)
        .expect("the fixture PNG is valid base64")
}

/// The def's display name, which is also the audit row's `target_label` —
/// the handle the reset deletes this test's audit trail by, since the def's
/// own id is fresh on every run and cannot be matched before it exists.
fn label(tag: &str) -> String {
    format!("Browser Toolsets Live {tag}")
}

/// The one filename this suite ever uploads, so the reset can find the
/// `uploads` rows: they are referenced only from an attachments JSON, so
/// nothing cascades them.
fn shot_name(tag: &str) -> String {
    format!("{PREFIX}-{tag}.png")
}

/// Every test hangs off its OWN tag, and the reset deletes by tag:
/// the tests in a file run concurrently, and one test's reset deleting a
/// sibling's def mid-flight is the failure this layout prevents. Order is
/// dependency order — conversations cascade their messages, the def cascades
/// its key — and the user rows go last.
async fn reset(pg: &PgPool, tag: &str) {
    for sql in [
        "delete from conversations where agent_model like $1",
        "delete from agent_defs where model like $1",
    ] {
        sqlx::query(sql)
            .bind(format!("{PREFIX}-{tag}-%"))
            .execute(pg)
            .await
            .unwrap();
    }
    sqlx::query("delete from uploads where filename = $1")
        .bind(shot_name(tag))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from audit_log where target_type = 'agent' and target_label = $1")
        .bind(label(tag))
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, PREFIX, tag).await;
}

/// An ADMIN with a session minted the way the auth routes mint one.
/// `require_agent_manager` answers yes for the admin role without an
/// agent_managers row, which keeps this fixture about toolsets rather than
/// about who manages whom. The sub carries the prefix+tag the sweep deletes
/// by.
async fn minted_admin(state: &AppState, tag: &str) -> String {
    let sub = format!("{PREFIX}:{tag}:{}", uuid::Uuid::new_v4());
    let (id,): (String,) =
        sqlx::query_as("insert into users (sub, role) values ($1, 'admin') returning id::text")
            .bind(&sub)
            .fetch_one(&state.pg)
            .await
            .unwrap();
    let user = SessionUser {
        id,
        sub,
        email: None,
        name: None,
        picture: None,
        role: "admin".into(),
        provider: "password".into(),
    };
    let sid = create_session(state, &user)
        .await
        .expect("session mints — needs REDIS_URL");
    format!("talaria_session={sid}")
}

/// An agent def this suite owns. slug and model are each UNIQUE, so both
/// carry the tag AND a uuid — a rerun must not collide with a prior run's
/// leftovers. It stays `managed = false` (the column's default) on purpose:
/// the PATCH fires a detached roll, and an unmanaged def names no department
/// to enqueue, so the test can never reach out and roll a container on the
/// developer's own machine. Returns (def id, model).
async fn def(pg: &PgPool, tag: &str) -> (String, String) {
    let unique = uuid::Uuid::new_v4();
    let model = format!("{PREFIX}-{tag}-{unique}");
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'personal', $2, $3) returning id::text",
    )
    .bind(format!("{PREFIX}-{tag}-{unique}"))
    .bind(&model)
    .bind(label(tag))
    .fetch_one(pg)
    .await
    .unwrap();
    (id, model)
}

/// The agent's own `tak_` credential — the exact thing a container presents.
async fn agent_key(state: &AppState, def_id: &str) -> String {
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, def_id)
        .await
        .expect("agent key mints")
}

/// What `speaking_target` calls live: a conversation whose `agent_model` is
/// the caller's, carrying a STREAMING assistant row. `streamed_at` defaults
/// to now(), and that is the column the tier's 15-minute bound reads — the
/// row is live because it was just written, exactly as a real turn is.
/// Returns (the chatter's user id, conversation id, the streaming row's id).
async fn live_turn(pg: &PgPool, tag: &str, model: &str) -> (String, String, String) {
    let chatter = fabricate_user(pg, PREFIX, tag).await;
    let conv = create_conversation(pg, &chatter, model, "live dm", "chat", None)
        .await
        .unwrap();
    insert_user_message(
        pg,
        &conv,
        0,
        "take a look at the page",
        &json!([]),
        Some(&chatter),
        &json!({}),
    )
    .await
    .unwrap();
    let (message,): (String,) = sqlx::query_as(
        "insert into messages (conversation_id, seq, role, content, status) \
         values ($1::uuid, 1, 'assistant', '', 'streaming') returning id::text",
    )
    .bind(&conv)
    .fetch_one(pg)
    .await
    .unwrap();
    (chatter, conv, message)
}

/// One request through the REAL router with the agent's own credential.
/// `call_json` speaks cookies, and the whole subject here is the `tak_` door
/// — so this helper exists to send (or withhold) `x-api-key`.
async fn agent_post(
    state: &AppState,
    path: &str,
    key: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(k) = key {
        req = req.header("x-api-key", k);
    }
    let res = routes::router(state.clone())
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

/// The def as the roster hands it to the manage modal.
async fn wire_def(state: &AppState, cookie: &str, id: &str) -> Value {
    let (status, body) = call_json(state, "GET", "/api/fleet/defs", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["defs"]
        .as_array()
        .expect("the roster carries defs")
        .iter()
        .find(|d| d["id"] == id)
        .cloned()
        .expect("an admin reads the whole fleet, so the def just patched is on the roster")
}

/// The toolset list as stored — the column the render reads, named.
async fn stored_toolsets(pg: &PgPool, id: &str) -> Value {
    let (stored,): (Value,) =
        sqlx::query_as("select disabled_toolsets from agent_defs where id = $1::uuid")
            .bind(id)
            .fetch_one(pg)
            .await
            .unwrap();
    stored
}

/// The attachments and chips a message row carries — the landed truth, read
/// back from Postgres rather than from the handler's answer.
async fn landed_on(pg: &PgPool, message: &str) -> (Value, Value) {
    sqlx::query_as("select attachments, chips from messages where id = $1::uuid")
        .bind(message)
        .fetch_one(pg)
        .await
        .unwrap()
}

/// Which toolsets an agent is denied is a manager's decision, taken on the
/// def and read back by the roster. Both ends matter: the column is what the
/// render merges into `agent.disabled_toolsets`, and the wire field is what
/// the manage modal shows the manager who just set it.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn disabled_toolsets_are_governed_on_the_way_in_and_read_back_on_the_roster() {
    let state = live_app_state().await;
    let tag = "patch";
    reset(&state.pg, tag).await;
    let cookie = minted_admin(&state, tag).await;
    let (id, _model) = def(&state.pg, tag).await;
    let path = format!("/api/fleet/defs/{id}");

    let (status, body) = call_json(
        &state,
        "PATCH",
        &path,
        Some(&cookie),
        Some(json!({ "disabledToolsets": ["browser", "web"] })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        stored_toolsets(&state.pg, &id).await,
        json!(["browser", "web"]),
        "the column the render reads, under the name the render reads it by"
    );
    assert_eq!(
        wire_def(&state, &cookie, &id).await["disabledToolsets"],
        json!(["browser", "web"])
    );

    // Taking a toolset away from an agent is a governance act, so it leaves
    // a trail with both sides of the change on it.
    let (action, before, after): (String, Option<Value>, Option<Value>) = sqlx::query_as(
        "select action, before, after from audit_log \
         where target_type = 'agent' and target_id = $1 and action = 'agent.toolsets'",
    )
    .bind(&id)
    .fetch_one(&state.pg)
    .await
    .expect("the toolset change is audited");
    assert_eq!(action, "agent.toolsets");
    assert_eq!(before, Some(json!({ "disabledToolsets": [] })));
    assert_eq!(
        after,
        Some(json!({ "disabledToolsets": ["browser", "web"] }))
    );

    // A name nothing governs is refused BY NAME. A config full of toolsets
    // Hermes has never heard of is exactly what this validation is for, and
    // a bare "invalid" would leave the manager guessing which of their names
    // was the typo.
    let (status, body) = call_json(
        &state,
        "PATCH",
        &path,
        Some(&cookie),
        Some(json!({ "disabledToolsets": ["nope"] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains("nope"),
        "the refusal must name the stranger: {body}"
    );

    // Forty-one names, every one of them real: the only thing wrong here is
    // the count, which pins the array cap rather than the name check.
    let (status, body) = call_json(
        &state,
        "PATCH",
        &path,
        Some(&cookie),
        Some(json!({ "disabledToolsets": vec!["browser"; 41] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // Neither refusal wrote anything. A 400 that half-saved is worse than a
    // 500: the manager is told their change was rejected while the agent
    // quietly loses its tools.
    assert_eq!(
        stored_toolsets(&state.pg, &id).await,
        json!(["browser", "web"])
    );

    reset(&state.pg, tag).await;
}

/// A browsing agent's screenshot reaches the person it is talking to. The
/// upload alone is not enough — an id with no `attachments` row behind it is
/// a link every reader 404s on, because `can_access_upload` probes that
/// column and nothing else. So the door does three things at once, and all
/// three are asserted from the row: the bytes, the attachment, the chip.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_screenshot_lands_on_the_live_turn_as_an_attachment_and_an_image_chip() {
    let state = live_app_state().await;
    let tag = "door";
    reset(&state.pg, tag).await;
    let (def_id, model) = def(&state.pg, tag).await;
    let key = agent_key(&state, &def_id).await;
    let (chatter, _conv, message) = live_turn(&state.pg, tag, &model).await;
    let filename = shot_name(tag);

    let (status, body) = agent_post(
        &state,
        "/api/agents/uploads",
        Some(&key),
        json!({
            "filename": filename,
            "mime": "image/png",
            "dataBase64": ONE_PIXEL_PNG,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(
        body["landed"], true,
        "the agent is told whether the surface actually got it: {body}"
    );
    let upload = body["uploadId"]
        .as_str()
        .expect("the door answers with the upload's id")
        .to_string();

    let (attachments, chips) = landed_on(&state.pg, &message).await;
    let attached = attachments
        .as_array()
        .expect("attachments is an array")
        .clone();
    assert_eq!(
        attached.len(),
        1,
        "exactly the one screenshot: {attachments}"
    );
    assert_eq!(attached[0]["id"], upload);
    assert_eq!(attached[0]["mime"], "image/png");
    assert_eq!(attached[0]["filename"], filename.as_str());
    assert_eq!(
        attached[0]["size"],
        png_bytes().len() as i64,
        "the row records the DECODED length, not the base64's"
    );

    let chips = chips.as_array().expect("chips is an array").clone();
    assert_eq!(chips.len(), 1, "exactly the one image chip: {chips:?}");
    assert_eq!(chips[0]["kind"], "image");
    assert_eq!(chips[0]["id"], format!("image:{upload}"));
    assert_eq!(chips[0]["uploadId"], upload);
    assert_eq!(chips[0]["label"], filename.as_str());
    assert_eq!(chips[0]["href"], format!("/api/uploads/{upload}"));

    // The person on the other side of the turn can fetch it. This def owns
    // nobody, so `uploaded_by` is null and the owner shortcut cannot be what
    // answers — the only door left open is the attachments probe, which is
    // the same door the chip's href goes through.
    assert!(
        can_access_upload(
            &state.pg,
            &upload,
            UploadViewer::Human {
                user_id: &chatter,
                who: None,
                is_admin: false,
            },
        )
        .await,
        "a reader of the conversation reaches the screenshot through the attachment"
    );

    reset(&state.pg, tag).await;
}

/// The refusals. Each one of these is a way the door could be left ajar: an
/// unsigned caller, a screenshot with no turn to land on, a mime the surface
/// cannot render, and a body big enough to be an attack. Every refusal must
/// also leave the live turn untouched.
#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_uploads_door_refuses_an_unsigned_caller_a_dead_turn_a_bad_mime_and_an_oversized_body()
{
    let state = live_app_state().await;
    let tag = "refuse";
    reset(&state.pg, tag).await;
    let (def_id, model) = def(&state.pg, tag).await;
    let key = agent_key(&state, &def_id).await;
    let filename = shot_name(tag);
    let good = json!({
        "filename": filename,
        "mime": "image/png",
        "dataBase64": ONE_PIXEL_PNG,
    });

    // No credential at all. Asserted first and with a body that would
    // otherwise succeed: the door is agent-only, and a 401 that arrives
    // only after the body parsed would hand anyone who can reach the port a
    // way to probe the caps.
    let (status, body) = agent_post(&state, "/api/agents/uploads", None, good.clone()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");

    // A real agent with nothing in flight. The attachment and the chip both
    // need a row to land on, and inventing one would put a screenshot in a
    // conversation nobody was having.
    let (status, body) = agent_post(&state, "/api/agents/uploads", Some(&key), good.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no live turn"),
        "the refusal says what is missing: {body}"
    );

    // Now give it a turn, so every refusal below is about the body.
    let (_chatter, _conv, message) = live_turn(&state.pg, tag, &model).await;

    let (status, body) = agent_post(
        &state,
        "/api/agents/uploads",
        Some(&key),
        json!({
            "filename": filename,
            "mime": "text/plain",
            "dataBase64": ONE_PIXEL_PNG,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap_or_default().contains("mime"),
        "the refusal names the field: {body}"
    );

    // One base64 character past the cap. The body is sent for real because
    // the cap is only provable at size: a 413 here means axum's hidden 2 MB
    // DefaultBodyLimit owns the stream before the handler's own ceiling can
    // answer, which is the bug /api/uploads carries an explicit limit for.
    let (status, body) = agent_post(
        &state,
        "/api/agents/uploads",
        Some(&key),
        json!({
            "filename": filename,
            "mime": "image/png",
            "dataBase64": "A".repeat(7 * 1024 * 1024 + 1),
        }),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the handler's own cap must be what refuses this, not the extractor: {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("dataBase64"),
        "the refusal names the field: {body}"
    );

    // Nothing landed. A refused screenshot that still left a chip would
    // render a link to an upload that was never stored.
    let (attachments, chips) = landed_on(&state.pg, &message).await;
    assert!(
        attachments.as_array().is_none_or(|a| a.is_empty()),
        "{attachments}"
    );
    assert!(chips.as_array().is_none_or(|a| a.is_empty()), "{chips}");
    let (n,): (i64,) = sqlx::query_as("select count(*) from uploads where filename = $1")
        .bind(&filename)
        .fetch_one(&state.pg)
        .await
        .unwrap();
    assert_eq!(n, 0, "a refused upload stores no bytes and no row");

    reset(&state.pg, tag).await;
}
