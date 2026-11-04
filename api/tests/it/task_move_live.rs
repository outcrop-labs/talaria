// task_move_live (see main.rs): POST /api/tasks/{id}/move against a real
// database, through the REAL router — a person with a minted session, an
// agent with its own `tak_` key. What a unit test cannot vouch for, because
// it IS the route riding real rows:
//
//   permissions  a person needs edit on BOTH boards; an agent needs board
//                policy on both, and the HITL stops its other writes obey
//   done         an agent naming a closed column is refused (403); a person
//                moving an open ticket into one is refused (400)
//   re-key       the ticket takes the destination's next number, and its OLD
//                ref still opens it (task_ref_aliases)
//   mapping      labels the destination lacks are dropped (not minted), a
//                person not on the destination is unassigned, the column maps
//                by name, and both answer in `moved` and the activity line
//   holds        a live work session refuses the move (409)
//   preview      answers `moved` and writes nothing
//
// House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… TALARIA_SECRET_KEY=… \
//     cargo test -p talaria-api --test it task_move_live:: -- --ignored

use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_api::routes;
use talaria_state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email};

const PREFIX: &str = "task-move-live";

/// Each test sweeps its OWN tag at the start: users cascade to their boards,
/// boards to their tickets (and the aliases with them); agents and runs are
/// swept by name.
async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from runs where input->>'liveTest' = $1")
        .bind(format!("{PREFIX}-{tag}"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}-{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from agent_defs where slug like $1")
        .bind(format!("tml{tag}%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn member(state: &AppState, tag: &str, who: &str) -> (String, String) {
    minted_member_with_email(
        state,
        &format!("{PREFIX}-{tag}"),
        &format!(
            "{PREFIX}-{tag}-{who}-{}@move-test.invalid",
            uuid::Uuid::new_v4().simple()
        ),
    )
    .await
}

/// A board with its prefix, owned (board_members row) by `owner`.
async fn board(pg: &PgPool, owner: &str, name: &str, prefix: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id, ticket_prefix) values ($1, $2::uuid, $3) returning id::text",
    )
    .bind(name)
    .bind(owner)
    .bind(prefix)
    .fetch_one(pg)
    .await
    .unwrap();
    share(pg, &id, owner, "owner").await;
    id
}

async fn share(pg: &PgPool, board: &str, user: &str, role: &str) {
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, $3) \
         on conflict (board_id, user_id) do update set role = excluded.role",
    )
    .bind(board)
    .bind(user)
    .bind(role)
    .execute(pg)
    .await
    .unwrap();
}

/// Custom columns, so the two boards really differ.
async fn columns(pg: &PgPool, board: &str, cols: &[(&str, &str, &str, bool)]) {
    for (i, (key, label, category, start)) in cols.iter().enumerate() {
        sqlx::query(
            "insert into board_statuses (board_id, key, label, color, category, agent_start, position) \
             values ($1::uuid, $2, $3, 'slate', $4, $5, $6)",
        )
        .bind(board)
        .bind(key)
        .bind(label)
        .bind(category)
        .bind(start)
        .bind(i as i32)
        .execute(pg)
        .await
        .unwrap();
    }
}

async fn label(pg: &PgPool, board: &str, name: &str) {
    sqlx::query("insert into board_labels (board_id, name) values ($1::uuid, $2)")
        .bind(board)
        .bind(name)
        .execute(pg)
        .await
        .unwrap();
}

/// A ticket numbered off the board's counter, as create does.
async fn ticket(pg: &PgPool, board: &str, status: &str, tags: Value, assignees: Value) -> String {
    let (no,): (i32,) = sqlx::query_as(
        "update boards set ticket_seq = ticket_seq + 1 where id = $1::uuid returning ticket_seq",
    )
    .bind(board)
    .fetch_one(pg)
    .await
    .unwrap();
    let (id,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, ticket_no, title, status, priority, tags, assignees, created_by) \
         values ($1::uuid, $2, 'Move me', $3, 'medium', $4, $5, 'live-test') returning id::text",
    )
    .bind(board)
    .bind(no)
    .bind(status)
    .bind(tags)
    .bind(assignees)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// An agent with its own key, on the boards named.
async fn agent(state: &AppState, tag: &str, boards: &[&str]) -> (String, String) {
    let slug = format!(
        "tml{tag}{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    );
    let model = format!("{slug}-engineering");
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'engineering', $2, 'Move Live Agent') returning id::text",
    )
    .bind(&slug)
    .bind(&model)
    .fetch_one(&state.pg)
    .await
    .unwrap();
    for b in boards {
        sqlx::query("insert into board_agents (board_id, agent_model) values ($1::uuid, $2)")
            .bind(b)
            .bind(&model)
            .execute(&state.pg)
            .await
            .unwrap();
    }
    let sb = state
        .secretbox()
        .await
        .expect("secretbox loads — TALARIA_SECRET_KEY must match (source ui/.env)");
    let key = talaria_api::agent_auth::rotate_agent_api_key(&state.pg, &sb, &id)
        .await
        .expect("agent key mints");
    (model, key)
}

async fn as_agent(
    state: &AppState,
    key: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let mut req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("x-api-key", key);
    let req = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            req.body(axum::body::Body::from(b.to_string()))
        }
        None => req.body(axum::body::Body::empty()),
    }
    .unwrap();
    let res = routes::router(state.clone()).oneshot(req).await.unwrap();
    let status = res.status();
    let raw = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&raw).unwrap_or(Value::Null))
}

fn path(id: &str) -> String {
    format!("/api/tasks/{id}/move")
}

#[tokio::test]
#[ignore]
async fn a_person_moves_a_ticket_and_the_old_ref_still_opens_it() {
    crate::support::wire_boot_seams();
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "p").await;
    let (ada, ada_cookie) = member(&state, "p", "ada").await;
    let (bo, _) = member(&state, "p", "bo").await;
    let src = board(&pg, &ada, "Move Source", "TMLS").await;
    let dest = board(&pg, &ada, "Move Dest", "TMLD").await;
    columns(
        &pg,
        &dest,
        &[
            ("triage", "Triage", "open", false),
            ("doing", "In Progress", "active", true),
            ("qa", "QA", "review", false),
            ("shipped", "Shipped", "done", false),
        ],
    )
    .await;
    label(&pg, &src, "billing").await;
    label(&pg, &src, "urgent").await;
    label(&pg, &dest, "Billing").await;
    // Bo is on the source only: an assignee the destination cannot see.
    share(&pg, &src, &bo, "editor").await;
    let id = ticket(
        &pg,
        &src,
        "in_progress",
        json!(["billing", "urgent"]),
        json!([format!("user:{ada}"), format!("user:{bo}")]),
    )
    .await;
    let (old_ref,): (String,) = sqlx::query_as(
        "select b.ticket_prefix || '-' || t.ticket_no from tasks t join boards b on b.id = t.board_id where t.id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();

    // Preview: the answer, and nothing written.
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&ada_cookie),
        Some(json!({ "boardId": dest, "preview": true })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["preview"], true);
    assert_eq!(
        body["moved"]["status"], "doing",
        "In progress ↔ In Progress, by name: {body}"
    );
    assert_eq!(body["moved"]["labelsKept"], json!(["Billing"]));
    assert_eq!(body["moved"]["labelsDropped"], json!(["urgent"]));
    assert_eq!(body["moved"]["unassigned"], json!([format!("user:{bo}")]));
    let (still,): (String,) =
        sqlx::query_as("select board_id::text from tasks where id = $1::uuid")
            .bind(&id)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!(still, src, "a preview moved the ticket");

    // A person moving an open ticket into a done column: refused, it is not a sign-off.
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&ada_cookie),
        Some(json!({ "boardId": dest, "status": "shipped" })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("never closes"),
        "{body}"
    );

    // The move.
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&ada_cookie),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["task"]["boardId"], dest.as_str());
    assert_eq!(body["task"]["status"], "doing");
    assert_eq!(body["task"]["tags"], json!(["Billing"]));
    assert_eq!(body["task"]["assignees"], json!([format!("user:{ada}")]));
    assert_eq!(body["task"]["ticketRef"], "TMLD-1");
    assert_eq!(body["moved"]["fromRef"], old_ref.as_str());
    // Nothing minted on the destination.
    let (minted,): (i64,) = sqlx::query_as(
        "select count(*) from board_labels where board_id = $1::uuid and name = 'urgent'",
    )
    .bind(&dest)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(minted, 0, "a move created a label on the destination");

    // The old ref still opens it; so does the new one; both are the same row.
    for r in [old_ref.as_str(), "TMLD-1"] {
        let (s, body) = call_json(
            &state,
            "GET",
            &format!("/api/tasks/{r}"),
            Some(&ada_cookie),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}: {body}");
        assert_eq!(body["task"]["id"], id.as_str(), "{r}");
    }
    // The record: the activity line names both refs and what was dropped.
    let (line,): (String,) = sqlx::query_as(
        "select description from task_activity where task_id = $1::uuid and type = 'moved'",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        line.contains(&old_ref) && line.contains("TMLD-1") && line.contains("urgent"),
        "{line}"
    );
    let (audits,): (i64,) = sqlx::query_as(
        "select count(*) from audit_log where action = 'task.board_move' and target_id = $1",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(audits, 1);
    sweep(&pg, "p").await;
}

#[tokio::test]
#[ignore]
async fn a_person_needs_edit_rights_on_both_boards() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "r").await;
    let (ada, _) = member(&state, "r", "ada").await;
    let (cy, cy_cookie) = member(&state, "r", "cy").await;
    let src = board(&pg, &ada, "Perm Source", "TMRS").await;
    let dest = board(&pg, &ada, "Perm Dest", "TMRD").await;
    let id = ticket(&pg, &src, "inbox", json!([]), json!([])).await;
    let mv = |b: &str| json!({ "boardId": b });

    // Not on the destination at all.
    share(&pg, &src, &cy, "editor").await;
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cy_cookie),
        Some(mv(&dest)),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    // A viewer of the destination.
    share(&pg, &dest, &cy, "viewer").await;
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cy_cookie),
        Some(mv(&dest)),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    // A viewer of the source, editor of the destination.
    share(&pg, &src, &cy, "viewer").await;
    share(&pg, &dest, &cy, "editor").await;
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cy_cookie),
        Some(mv(&dest)),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    // A board id that names nothing: the same 403 (forbidden, not hidden).
    let ghost = uuid::Uuid::new_v4().to_string();
    share(&pg, &src, &cy, "editor").await;
    let (s, _) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cy_cookie),
        Some(mv(&ghost)),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Already there.
    let (s, _) = call_json(&state, "POST", &path(&id), Some(&cy_cookie), Some(mv(&src))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // Editor on both: it moves.
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cy_cookie),
        Some(mv(&dest)),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    // No session: 401.
    let (s, _) = call_json(&state, "POST", &path(&id), None, Some(mv(&src))).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    sweep(&pg, "r").await;
}

#[tokio::test]
#[ignore]
async fn an_agent_moves_only_between_its_boards_and_never_into_done() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "a").await;
    let (ada, _) = member(&state, "a", "ada").await;
    let src = board(&pg, &ada, "Agent Source", "TMAS").await;
    let dest = board(&pg, &ada, "Agent Dest", "TMAD").await;
    let off = board(&pg, &ada, "Agent Off", "TMAO").await;
    let (model, key) = agent(&state, "a", &[&src, &dest]).await;
    let id = ticket(&pg, &src, "in_progress", json!([]), json!([model.clone()])).await;

    // A board the agent is not on.
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &path(&id),
        Some(json!({ "boardId": off })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("not allowed on that board"),
        "{body}"
    );
    // A closed column, named.
    for closed in ["done", "cancelled"] {
        let (s, body) = as_agent(
            &state,
            &key,
            "POST",
            &path(&id),
            Some(json!({ "boardId": dest, "status": closed })),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{closed}: {body}");
        assert!(
            body["error"].as_str().unwrap().contains("closed column"),
            "{body}"
        );
    }
    // In review: a person's to move.
    let review = ticket(&pg, &src, "quality_review", json!([]), json!([])).await;
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &path(&review),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    // The move: the default columns map by key, the agent stays assigned.
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &path(&id),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["task"]["boardId"], dest.as_str());
    assert_eq!(body["task"]["status"], "in_progress");
    assert_eq!(body["task"]["assignees"], json!([model]));
    assert_eq!(body["task"]["ticketRef"], "TMAD-1");
    sweep(&pg, "a").await;
}

#[tokio::test]
#[ignore]
async fn a_live_work_session_holds_the_ticket_where_it_is() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "w").await;
    let (ada, cookie) = member(&state, "w", "ada").await;
    let src = board(&pg, &ada, "Work Source", "TMWS").await;
    let dest = board(&pg, &ada, "Work Dest", "TMWD").await;
    let id = ticket(&pg, &src, "in_progress", json!([]), json!([])).await;
    sqlx::query("insert into runs (kind, state, subject_id, input) values ('work-session', 'running', $1, $2)")
        .bind(&id)
        .bind(json!({ "liveTest": format!("{PREFIX}-w") }))
        .execute(&pg)
        .await
        .unwrap();
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cookie),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("work session"),
        "{body}"
    );
    // The session ends: the move goes through.
    sqlx::query("update runs set state = 'done' where subject_id = $1")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cookie),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    sweep(&pg, "w").await;
}

// ── The review's follow-ups (S1, S2, S4, S5, N1) ─────────────────────────────

async fn watch(pg: &PgPool, task: &str, email: &str) {
    sqlx::query("insert into task_watchers (task_id, watcher) values ($1::uuid, $2)")
        .bind(task)
        .bind(email)
        .execute(pg)
        .await
        .unwrap();
}

async fn email_of(pg: &PgPool, user: &str) -> String {
    sqlx::query_scalar("select email from users where id = $1::uuid")
        .bind(user)
        .fetch_one(pg)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore]
async fn an_agent_cannot_unassign_a_person_and_whoever_loses_sight_is_told() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "u").await;
    let (ada, ada_cookie) = member(&state, "u", "ada").await;
    let (bo, _) = member(&state, "u", "bo").await;
    let (cy, _) = member(&state, "u", "cy").await;
    let src = board(&pg, &ada, "Unassign Source", "TMUS").await;
    let dest = board(&pg, &ada, "Unassign Dest", "TMUD").await;
    share(&pg, &src, &bo, "editor").await;
    share(&pg, &src, &cy, "viewer").await;
    let (model, key) = agent(&state, "u", &[&src, &dest]).await;
    let id = ticket(
        &pg,
        &src,
        "in_progress",
        json!([]),
        json!([format!("user:{bo}"), model]),
    )
    .await;
    let bo_email = email_of(&pg, &bo).await;
    let cy_email = email_of(&pg, &cy).await;
    watch(&pg, &id, &cy_email).await;

    // The agent: refused, naming the person, and nothing moved.
    let (s, body) = as_agent(
        &state,
        &key,
        "POST",
        &path(&id),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{body}");
    let msg = body["error"].as_str().unwrap();
    assert!(
        msg.contains("cannot unassign people") && msg.contains(&bo_email),
        "{msg}"
    );
    // A preview says the same, before anything is decided.
    let (s, _) = as_agent(
        &state,
        &key,
        "POST",
        &path(&id),
        Some(json!({ "boardId": dest, "preview": true })),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // A person may: Bo is unassigned, named in the activity by email (not a
    // uuid), and Bo and the watcher Cy — who lose sight — are told it left,
    // without being told where. The mover is not told.
    let (s, body) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&ada_cookie),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (line,): (String,) = sqlx::query_as(
        "select description from task_activity where task_id = $1::uuid and type = 'moved'",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        line.contains(&bo_email) && !line.contains("user:"),
        "{line}"
    );
    let mut told: Vec<(String, String, Option<String>)> = Vec::new();
    for _ in 0..50 {
        told = sqlx::query_as(
            "select user_id::text, title, href from notifications \
             where entity_id = $1 and title like '%moved to a board you can''t see%' order by user_id",
        )
        .bind(&id)
        .fetch_all(&pg)
        .await
        .unwrap();
        if told.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let who: Vec<&str> = told.iter().map(|(u, _, _)| u.as_str()).collect();
    assert!(
        who.contains(&bo.as_str()) && who.contains(&cy.as_str()),
        "{told:?}"
    );
    assert!(!who.contains(&ada.as_str()), "the mover was told: {told:?}");
    assert!(
        told.iter()
            .all(|(_, title, href)| !title.contains("Unassign Dest")
                && href.as_deref().unwrap_or("").is_empty()),
        "{told:?}"
    );
    sweep(&pg, "u").await;
}

#[tokio::test]
#[ignore]
async fn a_ticket_that_changes_mid_decision_refuses_the_move() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "c").await;
    let (ada, cookie) = member(&state, "c", "ada").await;
    let src = board(&pg, &ada, "Race Source", "TMCS").await;
    let dest = board(&pg, &ada, "Race Dest", "TMCD").await;
    let closed = ticket(&pg, &src, "in_progress", json!([]), json!([])).await;
    let parent = ticket(&pg, &src, "inbox", json!([]), json!([])).await;
    // A move that pauses between its decision and its write (debug builds;
    // keyed to the one ticket), and a write that lands in that window.
    async fn racing(
        state: &AppState,
        cookie: &str,
        id: &str,
        dest: &str,
        meanwhile: &'static str,
        pg: &PgPool,
    ) -> (StatusCode, Value) {
        unsafe { std::env::set_var("TALARIA_TEST_MOVE_PAUSE", format!("{id}:1500")) };
        let (st, c, p, d) = (
            state.clone(),
            cookie.to_string(),
            path(id),
            dest.to_string(),
        );
        let mv = tokio::spawn(async move {
            call_json(&st, "POST", &p, Some(&c), Some(json!({ "boardId": d }))).await
        });
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        sqlx::query(meanwhile).bind(id).execute(pg).await.unwrap();
        let out = mv.await.unwrap();
        unsafe { std::env::remove_var("TALARIA_TEST_MOVE_PAUSE") };
        out
    }
    // A person closes it while the move is deciding: the move must not
    // reopen it into an open column.
    let (s, body) = racing(
        &state,
        &cookie,
        &closed,
        &dest,
        "update tasks set status = 'done' where id = $1::uuid",
        &pg,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("changed while"),
        "{body}"
    );
    let (st, board_now): (String, String) =
        sqlx::query_as("select status, board_id::text from tasks where id = $1::uuid")
            .bind(&closed)
            .fetch_one(&pg)
            .await
            .unwrap();
    assert_eq!((st.as_str(), board_now.as_str()), ("done", src.as_str()));
    // A sub-task is created under it while the move is deciding: the parent
    // must not move without it.
    let (s, body) = racing(
        &state,
        &cookie,
        &parent,
        &dest,
        "insert into tasks (board_id, title, status, priority, parent_id, created_by) \
         select board_id, 'late child', 'inbox', 'medium', id, 'live-test' from tasks where id = $1::uuid",
        &pg,
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("sub-task"),
        "{body}"
    );
    sweep(&pg, "c").await;
}

#[tokio::test]
#[ignore]
async fn search_drops_a_moved_tickets_hits_for_the_board_it_left() {
    use talaria_retrieval::index::{RetrievalHit, drop_moved_ticket_hits};
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "s").await;
    let (ada, cookie) = member(&state, "s", "ada").await;
    let src = board(&pg, &ada, "Search Source", "TMSS").await;
    let dest = board(&pg, &ada, "Search Dest", "TMSD").await;
    let id = ticket(&pg, &src, "inbox", json!([]), json!([])).await;
    let (s, _) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&cookie),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // The points as a failed re-index would leave them: indexed under the
    // board the ticket LEFT.
    let hit = |source_type: &str, source_id: &str| RetrievalHit {
        collection: "activity".into(),
        score: 0.9,
        source_type: source_type.into(),
        source_id: source_id.into(),
        title: None,
        snippet: "stale".into(),
        href: Some(format!("/boards/{src}/{id}")),
        task_id: Some(id.clone()),
    };
    let doc = RetrievalHit {
        task_id: None,
        href: None,
        ..hit("kb-doc", "doc-1")
    };
    let hits = vec![hit("ticket", &id), hit("comment", "c-1"), doc.clone()];
    // A reader of the OLD board only: both stale hits are gone, the doc stays.
    let only_src = std::iter::once(src.clone()).collect();
    let kept = drop_moved_ticket_hits(&pg, &only_src, hits.clone())
        .await
        .unwrap();
    assert_eq!(kept, vec![doc.clone()]);
    // A reader of the NEW board: kept, and pointed at where the ticket is.
    let only_dest = std::iter::once(dest.clone()).collect();
    let kept = drop_moved_ticket_hits(&pg, &only_dest, hits).await.unwrap();
    assert_eq!(kept.len(), 3);
    assert!(
        kept[..2]
            .iter()
            .all(|h| h.href.as_deref() == Some(format!("/boards/{dest}/{id}").as_str()))
    );
    sweep(&pg, "s").await;
}

#[tokio::test]
#[ignore]
async fn someone_only_on_the_new_board_opens_the_old_ref() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "n").await;
    let (ada, ada_cookie) = member(&state, "n", "ada").await;
    let (cy, cy_cookie) = member(&state, "n", "cy").await;
    let src = board(&pg, &ada, "Link Source", "TMNS").await;
    let dest = board(&pg, &ada, "Link Dest", "TMND").await;
    share(&pg, &dest, &cy, "viewer").await;
    let id = ticket(&pg, &src, "inbox", json!([]), json!([])).await;
    // Before the move Cy cannot see it; after it, the old ref and the id
    // both open it from the board it is on now — what the old link follows.
    let (s, _) = call_json(&state, "GET", "/api/tasks/TMNS-1", Some(&cy_cookie), None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call_json(
        &state,
        "POST",
        &path(&id),
        Some(&ada_cookie),
        Some(json!({ "boardId": dest })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    for r in ["TMNS-1", id.as_str()] {
        let (s, body) = call_json(
            &state,
            "GET",
            &format!("/api/tasks/{r}"),
            Some(&cy_cookie),
            None,
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{r}: {body}");
        assert_eq!(body["task"]["boardId"], dest.as_str(), "{r}");
    }
    sweep(&pg, "n").await;
}

#[tokio::test]
#[ignore]
async fn an_old_ref_that_collides_with_a_live_one_stays_ambiguous() {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, "x").await;
    let (ada, cookie) = member(&state, "x", "ada").await;
    // Two boards, one prefix (initials collide in real life: "Sales Board",
    // "Support Board"), each with a #1.
    let sales = board(&pg, &ada, "Collide Sales", "TMXX").await;
    let support = board(&pg, &ada, "Collide Support", "TMXX").await;
    let elsewhere = board(&pg, &ada, "Collide Elsewhere", "TMXE").await;
    let _sales_1 = ticket(&pg, &sales, "inbox", json!([]), json!([])).await;
    let support_1 = ticket(&pg, &support, "inbox", json!([]), json!([])).await;
    let (s, _) = call_json(&state, "GET", "/api/tasks/TMXX-1", Some(&cookie), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "ambiguous before the move");
    let (s, _) = call_json(
        &state,
        "POST",
        &path(&support_1),
        Some(&cookie),
        Some(json!({ "boardId": elsewhere })),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Still ambiguous: the old ref must not silently open Sales' ticket.
    let (s, body) = call_json(&state, "GET", "/api/tasks/TMXX-1", Some(&cookie), None).await;
    assert_eq!(s, StatusCode::CONFLICT, "{body}");
    sweep(&pg, "x").await;
}
