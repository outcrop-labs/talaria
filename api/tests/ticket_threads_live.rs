// Live-DB proof of the ticket ROOM (cargo test -- --ignored). A ticket's
// discussion is a channel linked to its task (channels.task_id), and every
// guarantee this file pins is one a unit test cannot vouch for because it IS
// a Postgres rule or a router gate, not Rust: the room passes the same access
// gates as its board (through the real router, with minted sessions) and
// never shows in the comms rail, the open route is idempotent under a genuine
// race (the unique task_id index plus the re-read winner), the relevance gate
// fails OPEN when its judge cannot even be looked up, and the comment door
// posts real rows into the room it finds.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test ticket_threads_live -- --ignored

use axum::body::Body;
use axum::http::Request;
use sqlx::postgres::PgPool;
use talaria_api::channels::channel_role;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use talaria_api::tasks::{TaskDeps, add_comment, ensure_task_channel, get_task, list_comments};
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
/// cascades board → task → room → messages and activity, exactly the
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

/// The cast: an owner (the room's owner ladder resolves them by email),
/// a direct board member, a teammate through the board's TEAM, and a
/// stranger with an account. Plus one task carrying a human and an agent
/// assignee, so the ladder and the relevance path both have their rungs
/// without leaning on the org default.
struct Fixture {
    task_id: String,
    room_id: String,
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
         values ($1::uuid, 'Prove the room gates', 'todo', 'medium', $2, $3::jsonb) \
         returning id::text",
    )
    .bind(&board)
    .bind(owner.email.as_deref().expect("owner carries an email"))
    .bind(serde_json::json!([
        format!("user:{}", member.id),
        "live-test-agent"
    ]))
    .fetch_one(pg)
    .await
    .unwrap();

    let room_id = ensure_task_channel(pg, &task_id)
        .await
        .unwrap()
        .expect("owner by email: the ladder's first rung resolves");
    Fixture {
        task_id,
        room_id,
        owner,
        member,
        teammate,
        stranger,
    }
}

/// GET the room's messages through the REAL router with a minted session
/// cookie — the stack a browser rides, not the predicate it is built from.
async fn get_room_messages(state: &AppState, sid: &str, room_id: &str) -> u16 {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .uri(format!("/api/channels/{room_id}/messages"))
                .header("cookie", format!("talaria_session={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    res.status().as_u16()
}

/// The comms rail's list, through the same router — body included, because
/// the assertion is about the wire the rail renders, not the query under it.
async fn rail_body(state: &AppState, sid: &str) -> String {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .uri("/api/channels")
                .header("cookie", format!("talaria_session={sid}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// POST the open route the Discussion tab hits, same stack.
async fn post_open(state: &AppState, sid: &str, task_id: &str) -> u16 {
    let res = routes::router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/tasks/{task_id}/channel"))
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
async fn the_room_passes_the_same_gates_as_its_board() {
    let state = app_state().await;
    let f = fixture(&state, "gates").await;

    // The predicate: owner yes, member yes, teammate through the team yes,
    // stranger no — and nobody is the room's OWNER, because a task room
    // derives no owner at all (the owner-gated channel mutations refuse for
    // it by construction).
    for (who, id, expected) in [
        ("owner", &f.owner.id, true),
        ("member", &f.member.id, true),
        ("teammate", &f.teammate.id, true),
        ("stranger", &f.stranger.id, false),
    ] {
        let role = channel_role(&state.pg, id, &f.room_id).await.unwrap();
        assert_eq!(
            role.as_deref(),
            expected.then_some("member"),
            "predicate said {role:?} for {who}"
        );
    }

    // The router: 200 for the room, 403 for the stranger — the channel
    // door's grammar is forbidden, not hidden; the rail below is what keeps
    // a room invisible.
    for (who, user) in [
        ("owner", &f.owner),
        ("member", &f.member),
        ("teammate", &f.teammate),
    ] {
        let sid = create_session(&state, user).await.unwrap();
        assert_eq!(
            get_room_messages(&state, &sid, &f.room_id).await,
            200,
            "{who} could not read the room"
        );
    }
    let sid = create_session(&state, &f.stranger).await.unwrap();
    assert_eq!(get_room_messages(&state, &sid, &f.room_id).await, 403);

    // The open route is the board's door too — the stranger cannot open
    // (or create) what they cannot see.
    let sid = create_session(&state, &f.member).await.unwrap();
    assert_eq!(post_open(&state, &sid, &f.task_id).await, 200);
    let sid = create_session(&state, &f.stranger).await.unwrap();
    assert_eq!(post_open(&state, &sid, &f.task_id).await, 404);

    // And the room never shows in the comms rail — a room lives on its
    // ticket, not in the channel list, even for the person who owns both.
    let sid = create_session(&state, &f.owner).await.unwrap();
    let rail = rail_body(&state, &sid).await;
    assert!(
        !rail.contains(&f.room_id),
        "the task's room leaked into the comms rail"
    );

    reset(&state.pg, "gates").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn the_open_is_idempotent_and_race_safe() {
    let state = app_state().await;
    let f = fixture(&state, "race").await;
    let pg = &state.pg;

    // Twice in a row: the same room, not a second one.
    let again = ensure_task_channel(pg, &f.task_id).await.unwrap();
    assert_eq!(again.as_deref(), Some(f.room_id.as_str()));

    // A genuine race: the room deleted, two opens in flight at once. The
    // unique index on task_id means exactly one room is created, and the
    // re-read winner means BOTH handles agree on which.
    sqlx::query("delete from channels where task_id = $1::uuid")
        .bind(&f.task_id)
        .execute(pg)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        ensure_task_channel(pg, &f.task_id),
        ensure_task_channel(pg, &f.task_id),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b, "the race produced two answers: {a:?} vs {b:?}");
    let rooms: Vec<(String,)> =
        sqlx::query_as("select id::text from channels where task_id = $1::uuid")
            .bind(&f.task_id)
            .fetch_all(pg)
            .await
            .unwrap();
    assert_eq!(rooms.len(), 1, "the race created more than one room");
    assert_eq!(
        a.as_deref(),
        Some(rooms[0].0.as_str()),
        "the task's room is not the winner the handles agreed on"
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
async fn comments_post_as_room_rows_and_count_like_them() {
    let state = app_state().await;
    let f = fixture(&state, "comments").await;
    let pg = &state.pg;
    // Dead redis: the comment's fans degrade, the rows are what is on trial.
    let deps = TaskDeps::from_route(pg.clone(), None);

    // An AGENT author (a model string, the MCP door's shape) posts a room
    // row that carries its own name.
    add_comment(
        &deps,
        &f.task_id,
        "live-test-agent",
        "Interim note: reproduced on staging.",
        None,
    )
    .await
    .unwrap();
    let (author_type, author): (String, String) = sqlx::query_as(
        "select cm.author_type, cm.author from channel_messages cm \
         join channels c on c.id = cm.channel_id \
         where c.task_id = $1::uuid order by cm.seq desc limit 1",
    )
    .bind(&f.task_id)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(author_type, "agent");
    assert_eq!(author, "live-test-agent");

    // A HUMAN author (an email the owner ladder resolves) posts a user row —
    // the channel engine's stable identity for humans is the email itself.
    add_comment(
        &deps,
        &f.task_id,
        f.owner.email.as_deref().unwrap(),
        "Bumping: any movement?",
        None,
    )
    .await
    .unwrap();
    let (author_type, author): (String, String) = sqlx::query_as(
        "select cm.author_type, cm.author from channel_messages cm \
         join channels c on c.id = cm.channel_id \
         where c.task_id = $1::uuid order by cm.seq desc limit 1",
    )
    .bind(&f.task_id)
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(author_type, "user");
    assert_eq!(author, f.owner.email.as_deref().unwrap());

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
async fn every_ticket_room_keeps_its_shape() {
    // Post-boot invariants over whatever the database actually holds — the
    // copy migration ran at migration time, and these are the shapes it
    // promised.
    let pg = pg().await;
    // The conversation era is over — no kind='ticket' row survived the copy
    // — and no task room was ever provisioned a member row: access reads
    // the board, and a provisioned row would be a second truth to drift.
    let (stray_threads, provisioned_members): (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from conversations where kind = 'ticket'), \
           (select count(*) from channel_members m \
              join channels c on c.id = m.channel_id \
             where c.task_id is not null)",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(
        stray_threads, 0,
        "a kind='ticket' conversation survived the copy"
    );
    assert_eq!(
        provisioned_members, 0,
        "a task room carries a provisioned member row — access must stay derived"
    );

    // One room per task, and the seq the next post takes never collides
    // with the rows already there — the copy carried ids and seqs verbatim,
    // and a room whose msg_seq fell behind its rows would 500 every next
    // post on the unique(channel_id, seq) index. (msg_seq AHEAD is legal:
    // a deleted tail leaves the counter where it was.)
    let (two_rooms, colliding_seq): (i64, i64) = sqlx::query_as(
        "select \
           (select count(*) from (select task_id from channels \
              where task_id is not null \
              group by task_id having count(*) > 1) s), \
           (select count(*) from channels c \
             where c.task_id is not null \
               and c.msg_seq < coalesce( \
                    (select max(cm.seq) from channel_messages cm \
                      where cm.channel_id = c.id), 0))",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(two_rooms, 0, "two rooms claim one task");
    assert_eq!(
        colliding_seq, 0,
        "a room's msg_seq trails its rows — the next post collides"
    );
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn attachments_resolve_with_their_metadata() {
    // The stamp leg: a room message carrying files is the point of the
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
