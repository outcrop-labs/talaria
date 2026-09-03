// Live-DB proof of the ticket thread (cargo test -- --ignored). A ticket's
// discussion is a kind='ticket' conversation bound to the task, and every
// guarantee this file pins is one a unit test cannot vouch for because it IS
// a Postgres rule or a router gate, not Rust: the thread passes the same
// access gates as its board (through the real router, with minted sessions),
// the ensure route is idempotent under a genuine race (the conditional link
// write plus the re-read winner), the relevance gate fails OPEN when its
// judge cannot even be looked up, and the legacy comment door posts real
// turns into the thread it finds.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test ticket_threads_live -- --ignored

use axum::body::Body;
use axum::http::Request;
use sqlx::postgres::PgPool;
use talaria_api::config::Config;
use talaria_api::conversations::conversation_accessible;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use talaria_api::tasks::{
    TaskDeps, add_comment, ensure_task_conversation, get_task, list_comments,
};
use talaria_api::ticket_chat::{TicketHead, ticket_message_relevant};
use talaria_api::uploads::resolve_attachments;
use tower::ServiceExt; // oneshot

/// Real services, the same ones the process boots with — this file is about
/// gates the running stack enforces, and Redis carries the minted sessions.
async fn app_state() -> AppState {
    let cfg = Config::from_parts(
        std::env::var("DATABASE_URL").expect("set DATABASE_URL (source ui/.env)"),
        std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env)"),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg))
}

async fn pg() -> PgPool {
    sqlx::PgPool::connect(&std::env::var("DATABASE_URL").unwrap())
        .await
        .expect("connect")
}

/// The whole fixture hangs off user rows and one team: deleting the users
/// cascades board → task → thread → messages and activity, exactly the
/// direction production deletes run. The team row itself survives its
/// creator (created_by is SET NULL), so it goes explicitly.
///
/// Every test hangs off its OWN tag — tests run concurrently, and one test's
/// reset deleting the board out from under another's is the failure this
/// layout exists to prevent.
async fn reset(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where email like $1")
        .bind(format!("ticket-threads-{tag}-%@link-test.invalid"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from teams where name = $1")
        .bind(format!("live team {tag}"))
        .execute(pg)
        .await
        .unwrap();
}

/// The cast: an owner (the thread's owner ladder resolves to them by email),
/// a direct board member, a teammate through the board's TEAM, and a
/// stranger with an account. Plus one task carrying an agent assignee, so
/// the binder has its first rung without leaning on the org default.
struct Fixture {
    task_id: String,
    thread_id: String,
    owner: SessionUser,
    member: SessionUser,
    teammate: SessionUser,
    stranger: SessionUser,
}

async fn user_row(pg: &PgPool, email: &str, name: &str) -> SessionUser {
    let sub = format!("ticket-live:{email}");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') \
         returning id::text",
    )
    .bind(&sub)
    .bind(email)
    .bind(name)
    .fetch_one(pg)
    .await
    .unwrap();
    SessionUser {
        id,
        sub,
        email: Some(email.to_string()),
        name: Some(name.to_string()),
        picture: None,
        role: "member".into(),
        provider: "google".into(),
    }
}

async fn fixture(state: &AppState, tag: &str) -> Fixture {
    let pg = &state.pg;
    reset(pg, tag).await;
    let owner = user_row(
        pg,
        &format!("ticket-threads-{tag}-owner@link-test.invalid"),
        "Threads Live Owner",
    )
    .await;
    let member = user_row(
        pg,
        &format!("ticket-threads-{tag}-member@link-test.invalid"),
        "Board Member",
    )
    .await;
    let teammate = user_row(
        pg,
        &format!("ticket-threads-{tag}-teammate@link-test.invalid"),
        "Team Member",
    )
    .await;
    let stranger = user_row(
        pg,
        &format!("ticket-threads-{tag}-stranger@link-test.invalid"),
        "Stranger",
    )
    .await;

    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) values ('ticket threads live', $1::uuid) \
         returning id::text",
    )
    .bind(&owner.id)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'editor')",
    )
    .bind(&board)
    .bind(&member.id)
    .execute(pg)
    .await
    .unwrap();
    // The board belongs to a team; the teammate is on the team and nowhere
    // near board_members — the access legs must carry them anyway.
    let (team,): (String,) = sqlx::query_as(
        "insert into teams (name, created_by) values ($2, $1::uuid) returning id::text",
    )
    .bind(&owner.id)
    .bind(format!("live team {tag}"))
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query("update boards set team_id = $1::uuid where id = $2::uuid")
        .bind(&team)
        .bind(&board)
        .execute(pg)
        .await
        .unwrap();
    sqlx::query(
        "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, 'member')",
    )
    .bind(&team)
    .bind(&teammate.id)
    .execute(pg)
    .await
    .unwrap();

    let (task_id,): (String,) = sqlx::query_as(
        "insert into tasks (board_id, title, status, priority, created_by, assignees) \
         values ($1::uuid, 'Prove the thread gates', 'todo', 'medium', $2, $3::jsonb) \
         returning id::text",
    )
    .bind(&board)
    .bind(owner.email.as_deref().expect("owner carries an email"))
    .bind(serde_json::json!([&member.id, "live-test-agent"]))
    .fetch_one(pg)
    .await
    .unwrap();

    let thread_id = ensure_task_conversation(pg, &task_id)
        .await
        .unwrap()
        .expect("owner by email + agent assignee: both ladders resolve");
    Fixture {
        task_id,
        thread_id,
        owner,
        member,
        teammate,
        stranger,
    }
}

/// GET through the REAL router with a minted session cookie — the stack a
/// browser rides, not the predicate it is built from.
async fn get_conversation(state: &AppState, sid: &str, thread_id: &str) -> u16 {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/conversations/{thread_id}"))
                .header("cookie", format!("talaria_session={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    res.status().as_u16()
}

/// POST the ensure route the Discussion tab opens, same stack.
async fn post_ensure(state: &AppState, sid: &str, task_id: &str) -> u16 {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/tasks/{task_id}/conversation"))
                .header("cookie", format!("talaria_session={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    res.status().as_u16()
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_thread_passes_the_same_gates_as_its_board() {
    let state = app_state().await;
    let f = fixture(&state, "gates").await;

    // The predicates: owner yes, member yes, teammate through the team yes,
    // stranger no. The same four answers the router is about to give.
    for (who, id, expected) in [
        ("owner", &f.owner.id, true),
        ("member", &f.member.id, true),
        ("teammate", &f.teammate.id, true),
        ("stranger", &f.stranger.id, false),
    ] {
        let ok = conversation_accessible(&state.pg, id, &f.thread_id)
            .await
            .unwrap();
        assert_eq!(ok, expected, "predicate said {ok} for {who}");
    }

    // The router: 200 for the room, 404 for the stranger — a refusal that
    // does not confirm the thread exists.
    for (who, user) in [
        ("owner", &f.owner),
        ("member", &f.member),
        ("teammate", &f.teammate),
    ] {
        let sid = create_session(&state, user).await.unwrap();
        assert_eq!(
            get_conversation(&state, &sid, &f.thread_id).await,
            200,
            "{who} could not read the thread"
        );
    }
    let sid = create_session(&state, &f.stranger).await.unwrap();
    assert_eq!(get_conversation(&state, &sid, &f.thread_id).await, 404);

    // The ensure route is the board's door too — the stranger cannot open
    // (or create) what they cannot see.
    let sid = create_session(&state, &f.member).await.unwrap();
    assert_eq!(post_ensure(&state, &sid, &f.task_id).await, 200);
    let sid = create_session(&state, &f.stranger).await.unwrap();
    assert_eq!(post_ensure(&state, &sid, &f.task_id).await, 404);

    reset(&state.pg, "gates").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_ensure_is_idempotent_and_race_safe() {
    let state = app_state().await;
    let f = fixture(&state, "race").await;
    let pg = &state.pg;

    // Twice in a row: the same room, not a second one.
    let again = ensure_task_conversation(pg, &f.task_id).await.unwrap();
    assert_eq!(again.as_deref(), Some(f.thread_id.as_str()));

    // A genuine race: the link nulled, two ensures in flight at once. The
    // conditional link write means exactly one conversation is created, and
    // the re-read winner means BOTH handles agree on which.
    sqlx::query("update tasks set conversation_id = null where id = $1::uuid")
        .bind(&f.task_id)
        .execute(pg)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        ensure_task_conversation(pg, &f.task_id),
        ensure_task_conversation(pg, &f.task_id),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b, "the race produced two answers: {a:?} vs {b:?}");
    let (count, linked): (i64, Option<String>) = sqlx::query_as(
        "select (select count(*) from conversations c \
                  where c.id = $2::uuid and c.kind = 'ticket'), \
               t.conversation_id::text \
         from tasks t where t.id = $1::uuid",
    )
    .bind(&f.task_id)
    .bind(&a)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(count, 1, "the race created more than one thread");
    assert_eq!(
        linked, a,
        "the task's link is not the winner the handles agreed on"
    );

    reset(pg, "race").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_gate_folds_every_judge_failure_open() {
    // A state whose database is an EMPTY SCHEMA: the seat lookup itself
    // fails, which is the loudest possible "the judge cannot run" — and the
    // one worth pinning, because it is also what a real outage looks like.
    let base = std::env::var("DATABASE_URL").unwrap();
    let scratch_url = base
        .trim_end_matches('/')
        .rsplit_once('/')
        .map(|(head, _)| format!("{head}/ticket_gate_scratch"))
        .unwrap_or_else(|| format!("{base}ticket_gate_scratch"));
    let admin = PgPool::connect(
        &base
            .trim_end_matches('/')
            .rsplit_once('/')
            .map(|(head, _)| format!("{head}/postgres"))
            .unwrap_or_default(),
    )
    .await
    .unwrap();
    sqlx::raw_sql("drop database if exists ticket_gate_scratch with (force)")
        .execute(&admin)
        .await
        .unwrap();
    sqlx::raw_sql("create database ticket_gate_scratch")
        .execute(&admin)
        .await
        .unwrap();
    let cfg = Config::from_parts(
        scratch_url,
        "redis://127.0.0.1:1".into(),
        std::env::var("TALARIA_SECRET_KEY").unwrap_or_default(),
        std::env::var("TALARIA_SECRET_KEY_FILE").unwrap_or_default(),
        String::new(),
        String::new(),
    )
    .expect("scratch config assembles");
    let state = AppState::new(talaria_api::db::pool(&cfg), std::sync::Arc::new(cfg));

    let head = TicketHead {
        ticket_ref: Some("LIVE-1".into()),
        title: "Prove the gate folds open".into(),
        status: "todo".into(),
        description: Some("Anything the judge cannot decide must reach the agent.".into()),
    };
    // The structural cases never reach the judge at all — they are the same
    // answers with a dead database as with a live one.
    assert!(
        ticket_message_relevant(&state, &head, "   ", 1, &[]).await,
        "an attachments-only turn is a handoff — relevant by construction"
    );
    assert!(
        !ticket_message_relevant(&state, &head, "", 0, &[]).await,
        "the one message the agent must never answer"
    );
    assert!(
        ticket_message_relevant(&state, &head, "LIVE-1 is still failing", 0, &[]).await,
        "naming the ref is about this ticket by construction"
    );
    // The judged case with a judge that cannot even be looked up: open.
    assert!(
        ticket_message_relevant(&state, &head, "is the café on the corner open?", 0, &[]).await,
        "a dead judge may never silence a thread"
    );

    sqlx::raw_sql("drop database ticket_gate_scratch with (force)")
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn comments_post_as_turns_and_count_like_them() {
    let state = app_state().await;
    let f = fixture(&state, "comments").await;
    let pg = &state.pg;
    // Dead redis: the comment's fans degrade, the rows are what is on trial.
    let deps = TaskDeps::from_route(pg.clone(), None);

    // An AGENT author (a model string, the MCP door's shape) posts an
    // assistant turn that carries its own name.
    add_comment(
        &deps,
        &f.task_id,
        "live-test-agent",
        "Interim note: reproduced on staging.",
        None,
    )
    .await
    .unwrap();
    let (role, agent): (String, Option<String>) = sqlx::query_as(
        "select m.role, m.metadata->>'agent' from messages m \
         join tasks t on t.conversation_id = m.conversation_id \
         where t.id = $1::uuid order by m.seq desc limit 1",
    )
    .bind(&f.task_id)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(role, "assistant");
    assert_eq!(agent.as_deref(), Some("live-test-agent"));

    // A HUMAN author (an email the owner ladder resolves) posts a user turn
    // with the author's row attached.
    add_comment(
        &deps,
        &f.task_id,
        f.owner.email.as_deref().unwrap(),
        "Bumping: any movement?",
        None,
    )
    .await
    .unwrap();
    let (role, author): (String, Option<String>) = sqlx::query_as(
        "select m.role, m.author_user_id::text from messages m \
         join tasks t on t.conversation_id = m.conversation_id \
         where t.id = $1::uuid order by m.seq desc limit 1",
    )
    .bind(&f.task_id)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(role, "user");
    assert_eq!(author.as_deref(), Some(f.owner.id.as_str()));

    // The activity log knows both comments happened, the wire count agrees
    // with the message rows, and the synthesized read names both authors.
    let (activity,): (i64,) = sqlx::query_as(
        "select count(*) from task_activity where task_id = $1::uuid and type = 'comment'",
    )
    .bind(&f.task_id)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(activity, 2);
    let task = get_task(pg, &f.task_id).await.unwrap().unwrap();
    assert_eq!(task.comment_count, 2);
    let comments = list_comments(pg, &f.task_id).await.unwrap();
    assert_eq!(comments.len(), 2);
    // Arrival order: the agent's interim note first, the human's bump after.
    assert_eq!(comments[0].author, "live-test-agent");
    assert_eq!(comments[1].author, "Threads Live Owner");

    reset(pg, "comments").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn every_ticket_thread_is_one_tasks_room() {
    // Post-boot invariants over whatever the database actually holds — the
    // backfill ran at migration time, and these are the shapes it promised.
    let pg = pg().await;
    let (rooms_two_tasks_point_at, bad_links): (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from (select conversation_id from tasks \
              where conversation_id is not null \
              group by conversation_id having count(*) > 1) s), \
           (select count(*) from tasks t where t.conversation_id is not null \
              and not exists (select 1 from conversations c \
                 where c.id = t.conversation_id and c.kind = 'ticket'))",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(rooms_two_tasks_point_at, 0, "two tasks share one thread");
    assert_eq!(
        bad_links, 0,
        "a task points at something that is not its thread"
    );

    // Per-thread order: seq is contiguous from zero and every row is
    // complete — the shape the chat door's chaining and the count's read
    // both assume.
    let (gaps, incomplete): (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from ( \
              select m.conversation_id from messages m \
                join conversations c on c.id = m.conversation_id \
               where c.kind = 'ticket' \
               group by m.conversation_id \
              having min(m.seq) <> 0 \
                  or max(m.seq) <> count(*) - 1) s), \
           (select count(*) from messages m \
              join conversations c on c.id = m.conversation_id \
             where c.kind = 'ticket' and m.status <> 'complete')",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(gaps, 0, "a ticket thread's seq has a hole");
    assert_eq!(incomplete, 0, "a ticket thread carries an incomplete row");
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn attachments_resolve_with_their_metadata() {
    // The stamp leg: a thread message carrying files is the point of the
    // room, and the resolver is what turns upload ids into the metadata the
    // message row keeps. Pinned live because the failure that hid here for
    // the whole port era was a sqlx DECODE mismatch — int4 column into an
    // i64 slot — which only a real row with a real column type can catch.
    let pg = pg().await;
    let (id,): (String,) = sqlx::query_as(
        "insert into uploads (filename, mime, size, path) \
         values ('live-proof.bin', 'application/octet-stream', 1234, 'live-proof/nowhere') \
         returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();

    let resolved = resolve_attachments(
        &pg,
        &[
            id.clone(),
            "00000000-0000-0000-0000-000000000000".to_string(),
        ],
    )
    .await
    .unwrap();
    // Caller order preserved, unknown ids dropped: one attachment, the right
    // one, with every field the message row stamps.
    assert_eq!(resolved.len(), 1, "the real upload did not resolve");
    assert_eq!(resolved[0].id, id);
    assert_eq!(resolved[0].filename, "live-proof.bin");
    assert_eq!(resolved[0].mime, "application/octet-stream");
    assert_eq!(resolved[0].size, 1234, "size did not decode");

    sqlx::query("delete from uploads where id = $1::uuid")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
}
