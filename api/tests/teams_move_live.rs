mod support;
// Live-DB proof of the board team move (cargo test -- --ignored). The gates
// live on the ROUTE and in the store, and they are all Postgres questions:
// destination-team membership, the access-loss preview against
// board_members/team_members, the audit row's before/after teams, and the
// personal-move guard's direct-owner requirement. Each case drives the REAL
// router with a minted session, because the move gate reads `acting_user`,
// not a test-shaped caller.
//
// House rule: #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test --test teams_move_live -- --ignored --test-threads=1

use axum::body::Body;
use axum::http::Request;
use sqlx::PgPool;
use talaria_api::boards::board_visibility_sql;
use talaria_api::config::Config;
use talaria_api::routes;
use talaria_api::session::{SessionUser, create_session};
use talaria_api::state::AppState;
use tower::ServiceExt; // oneshot

const PREFIX: &str = "tala38";
const TAG: &str = "move";

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

/// A failed previous run's leftovers would shadow this one's assertions —
/// sweep first, exactly the way the house `sweep_user_rows` does for its
/// fabricated rows. Each test owns its TAG (they run concurrently); the
/// teams sweep is prefix-wide because team rows cascade from users too.
async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("{PREFIX}:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
    sqlx::query("delete from teams where name like $1")
        .bind(format!("{PREFIX} team-%"))
        .execute(pg)
        .await
        .unwrap();
}

async fn person(pg: &PgPool, tag: &str, who: &str) -> (String, SessionUser) {
    let sub = format!("{PREFIX}:{tag}:{who}:{}", uuid::Uuid::new_v4());
    let email = format!("{PREFIX}-{tag}-{who}@test.invalid");
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, $3, 'member') returning id::text",
    )
    .bind(&sub)
    .bind(&email)
    .bind(who)
    .fetch_one(pg)
    .await
    .unwrap();
    (
        id.clone(),
        SessionUser {
            id,
            sub,
            email: Some(email),
            name: Some(who.to_string()),
            picture: None,
            role: "member".into(),
            provider: "google".into(),
        },
    )
}

async fn team(pg: &PgPool, name: &str, owner: &str) -> String {
    sqlx::query_scalar(
        "insert into teams (name, created_by) values ($1, $2::uuid) returning id::text",
    )
    .bind(format!("{PREFIX} team-{name}"))
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn on_team(pg: &PgPool, team_id: &str, user_id: &str, role: &str) {
    sqlx::query(
        "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, $3)",
    )
    .bind(team_id)
    .bind(user_id)
    .bind(role)
    .execute(pg)
    .await
    .unwrap();
}

/// A board under `team_id`, with `owner` as boards.owner_id and an optional
/// direct board_members owner row (create_board's invariant: the creator has
/// one — omit it for the team-derived-only owner cases).
async fn team_board(pg: &PgPool, owner: &str, team_id: &str, direct_owner_row: bool) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id, team_id) \
         values ($1, $2::uuid, $3::uuid) returning id::text",
    )
    .bind(format!("{PREFIX} move target"))
    .bind(owner)
    .bind(team_id)
    .fetch_one(pg)
    .await
    .unwrap();
    if direct_owner_row {
        sqlx::query(
            "insert into board_members (board_id, user_id, role) \
             values ($1::uuid, $2::uuid, 'owner')",
        )
        .bind(&id)
        .bind(owner)
        .execute(pg)
        .await
        .unwrap();
    }
    id
}

async fn sid(state: &AppState, user: &SessionUser) -> String {
    create_session(state, user).await.unwrap()
}

async fn patch(state: &AppState, session: &str, board_id: &str, body: &str) -> (u16, String) {
    let res = routes::router(state.clone())
        .oneshot(
            Request::patch(format!("/api/boards/{board_id}"))
                .header("content-type", "application/json")
                .header("cookie", format!("talaria_session={session}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn board_team_id(pg: &PgPool, board_id: &str) -> Option<String> {
    sqlx::query_scalar("select team_id::text from boards where id = $1::uuid")
        .bind(board_id)
        .fetch_optional(pg)
        .await
        .unwrap()
        .flatten()
}

/// Can this user see the board under the ROUTE's own predicate — the exact
/// `board_visibility_sql` fragment the listing ships, with the same user id
/// bound at both of its sites.
async fn sees_board(pg: &PgPool, board_id: &str, user_id: &str) -> bool {
    let frag = board_visibility_sql("$1::uuid", "$2::uuid", true);
    let sql = format!("select exists (select 1 from boards b where b.id = $3::uuid and {frag})");
    let (seen,): (bool,) = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(user_id)
        .bind(user_id)
        .bind(board_id)
        .fetch_one(pg)
        .await
        .unwrap();
    seen
}

/// The whole TALA-38 contract in one pass, against one fixture set: the
/// move itself, both visibility flips, the audit row, the preview, and both
/// refusals. `--test-threads=1` keeps the live suites from sharing rows.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn a_team_move_regrants_access_previews_loss_and_refuses_bad_movers() {
    let state = app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, TAG).await;

    // The cast: mover/owner (on A and B, with a direct owner row), an A-only
    // teammate, a B-only teammate, and an A-only owner with no direct row.
    let (mover, mover_user) = person(&pg, TAG, "mover").await;
    let (a_member, _) = person(&pg, TAG, "old-mate").await;
    let (b_member, _) = person(&pg, TAG, "new-mate").await;
    let (a_owner, a_owner_user) = person(&pg, TAG, "old-owner").await;
    let team_a = team(&pg, "a", &mover).await;
    let team_b = team(&pg, "b", &mover).await;
    on_team(&pg, &team_a, &mover, "owner").await;
    on_team(&pg, &team_b, &mover, "owner").await;
    on_team(&pg, &team_a, &a_member, "member").await;
    on_team(&pg, &team_b, &b_member, "member").await;
    on_team(&pg, &team_a, &a_owner, "owner").await;
    let board = team_board(&pg, &mover, &team_a, true).await;
    let mover_sid = sid(&state, &mover_user).await;
    let a_owner_sid = sid(&state, &a_owner_user).await;

    // (d) the preview answers BEFORE anything moves, and the A-only
    // teammate is exactly who it names.
    let (status, body) = patch(
        &state,
        &mover_sid,
        &board,
        &format!("{{\"teamId\": \"{team_b}\", \"previewAccessLoss\": true}}"),
    )
    .await;
    assert_eq!(status, 200, "the preview answers 200: {body}");
    let preview: serde_json::Value = serde_json::from_str(&body).unwrap();
    let losers: Vec<String> = preview["loseAccess"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v["userId"].as_str().map(String::from))
        .collect();
    // BOTH team-derived-only members lose: the A-only teammate and the
    // A-only owner (no direct row on the board either). Sorted for a stable
    // comparison — the SQL orders by name, which is who, not insertion.
    let mut expected = vec![a_member.clone(), a_owner.clone()];
    expected.sort();
    let mut losers_sorted = losers;
    losers_sorted.sort();
    assert_eq!(
        losers_sorted, expected,
        "loseAccess is exactly the team-derived-only members: {body}"
    );

    // (a) the move: A → B, the mover sits on B.
    let (status, body) = patch(
        &state,
        &mover_sid,
        &board,
        &format!("{{\"teamId\": \"{team_b}\"}}"),
    )
    .await;
    assert_eq!(status, 200, "the move lands: {body}");
    assert_eq!(
        board_team_id(&pg, &board).await.as_deref(),
        Some(team_b.as_str())
    );

    // (b) the visibility flip, under the route's own predicate.
    assert!(
        !sees_board(&pg, &board, &a_member).await,
        "the A-only teammate lost sight of the moved board"
    );
    assert!(
        sees_board(&pg, &board, &b_member).await,
        "the B teammate gained it"
    );
    assert!(
        sees_board(&pg, &board, &mover).await,
        "the direct owner keeps it"
    );

    // (c) the audit row: from team A to team B, actor = the mover's
    // acting_user label (their email).
    let (actor, action, before, after): (String, String, serde_json::Value, serde_json::Value) =
        sqlx::query_as(
            "select actor, action, before, after from audit_log \
             where action = 'board.team_move' and target_id = $1::uuid \
             order by created_at desc limit 1",
        )
        .bind(&board)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(action, "board.team_move");
    assert_eq!(actor, format!("{PREFIX}-{TAG}-mover@test.invalid"));
    assert_eq!(before["teamName"], format!("{PREFIX} team-a"));
    assert_eq!(after["teamName"], format!("{PREFIX} team-b"));

    // (e) a mover NOT on the destination is refused, and the board stays
    // put. The mover is on both teams here — refuse via a third team they
    // are not on, which is the same destination-membership arm.
    let orphan =
        sqlx::query_scalar::<_, String>("insert into teams (name) values ($1) returning id::text")
            .bind(format!("{PREFIX} team-orphan"))
            .fetch_one(&pg)
            .await
            .unwrap();
    let (status, body) = patch(
        &state,
        &mover_sid,
        &board,
        &format!("{{\"teamId\": \"{orphan}\"}}"),
    )
    .await;
    assert_eq!(status, 400, "a move into an unjoined team is a 400: {body}");
    assert!(
        body.contains("moving a board into a team requires membership"),
        "the refusal is the destination-membership one: {body}"
    );
    assert_eq!(
        board_team_id(&pg, &board).await.as_deref(),
        Some(team_b.as_str()),
        "the refused move left the board on B"
    );

    // (e, the A-not-B shape) an owner on A but not B cannot march a board
    // into B: the same membership arm refuses, the board stays on A.
    let held = team_board(&pg, &a_owner, &team_a, false).await;
    let (status, body) = patch(
        &state,
        &a_owner_sid,
        &held,
        &format!("{{\"teamId\": \"{team_b}\"}}"),
    )
    .await;
    assert_eq!(status, 400, "an A-only owner cannot move into B: {body}");
    assert_eq!(
        board_team_id(&pg, &held).await.as_deref(),
        Some(team_a.as_str())
    );

    // (f) a team owner WITHOUT a direct owner row cannot go personal: the
    // move would leave them outside their own board.
    let (status, body) = patch(&state, &a_owner_sid, &held, "{\"teamId\": null}").await;
    assert_eq!(
        status, 400,
        "a team-derived owner cannot move to personal: {body}"
    );
    assert!(
        body.contains("you would lose access"),
        "the refusal is the personal-move guard's: {body}"
    );
    assert_eq!(
        board_team_id(&pg, &held).await.as_deref(),
        Some(team_a.as_str()),
        "the refused personal move left the board on A"
    );

    sweep(&pg, TAG).await;
}

/// The second pass: an owner who IS a direct board member moves a PERSONAL
/// board into their team (a direct share follows the board), and a
/// team-derived owner with a direct row detaches to personal cleanly — the
/// guard above only fires when there is no direct row to fall back on.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn personal_board_moves_in_and_a_direct_row_detaches_out() {
    let state = app_state().await;
    let pg = state.pg.clone();
    const TAG2: &str = "move2";
    sweep(&pg, TAG2).await;

    let (owner, owner_user) = person(&pg, TAG2, "owner").await;
    let (direct, _) = person(&pg, TAG2, "direct").await;
    let t = team(&pg, "personal", &owner).await;
    on_team(&pg, &t, &owner, "owner").await;
    // A personal board (no team) with a direct editor share.
    let b = team_board(&pg, &owner, &t, true).await;
    sqlx::query("update boards set team_id = null where id = $1::uuid")
        .bind(&b)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'editor')",
    )
    .bind(&b)
    .bind(&direct)
    .execute(&pg)
    .await
    .unwrap();
    let owner_sid = sid(&state, &owner_user).await;

    // Personal → team (the owner is on the destination).
    let (status, body) = patch(&state, &owner_sid, &b, &format!("{{\"teamId\": \"{t}\"}}")).await;
    assert_eq!(
        status, 200,
        "the owner moves their personal board in: {body}"
    );
    assert_eq!(board_team_id(&pg, &b).await.as_deref(), Some(t.as_str()));
    // The direct share survives the move.
    assert!(
        sees_board(&pg, &b, &direct).await,
        "the direct editor keeps access through the move"
    );

    // Back to personal: the owner has a direct row, so the guard passes.
    let (status, body) = patch(&state, &owner_sid, &b, "{\"teamId\": null}").await;
    assert_eq!(status, 200, "a direct-row owner detaches cleanly: {body}");
    assert_eq!(board_team_id(&pg, &b).await, None);

    sweep(&pg, TAG2).await;
}
