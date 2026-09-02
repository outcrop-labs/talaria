// Live-DB proof of the self-service grant path (cargo test -- --ignored).
// The one-step grant is two single-row writes against board_agents and one
// partial-unique dedup on board_agent_requests — none of which a unit test
// can vouch for, because their guarantees ARE Postgres rules (the conflict
// target, the partial index's where clause). House rule: #[ignore]d, never
// CI.
//
//   DATABASE_URL=postgres://… cargo test --test board_agent_access -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::boards::{
    add_board_agent_row, get_board_agent_config, remove_board_agent_row,
};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Each test hangs off its OWN throwaway user (tests run concurrently, and a
/// shared one would mean one test's cleanup deleting the board out from
/// under the other) — the cascade removes board, board_agents and requests
/// along with the user.
async fn cleanup(pg: &PgPool, email: &str) {
    sqlx::query("delete from users where email = $1")
        .bind(email)
        .execute(pg)
        .await
        .unwrap();
}

async fn throwaway_board(pg: &PgPool, email: &str) -> String {
    let (owner,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) \
         values ($1, $1, 'Board Access Test', 'member') \
         returning id::text",
    )
    .bind(email)
    .fetch_one(pg)
    .await
    .unwrap();
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) \
         values ('board access live test', $1::uuid) returning id::text",
    )
    .bind(&owner)
    .fetch_one(pg)
    .await
    .unwrap();
    board
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn self_service_rows_round_trip_without_touching_the_rest_of_the_policy() {
    let pg = pool().await;
    let email = "board-access-rows@link-test.invalid";
    cleanup(&pg, email).await;
    let board = throwaway_board(&pg, email).await;
    // An editor's policy row rides along: the self-service add must leave it
    // standing (the whole point of the single-row insert).
    add_board_agent_row(&pg, &board, "some-editor-added-agent")
        .await
        .unwrap();

    add_board_agent_row(&pg, &board, "personal-test-assistant")
        .await
        .unwrap();
    // A repeat add is a no-op, not a duplicate and not an error.
    add_board_agent_row(&pg, &board, "personal-test-assistant")
        .await
        .unwrap();
    let cfg = get_board_agent_config(&pg, &board).await.unwrap();
    assert_eq!(cfg.models.len(), 2, "both rows stand: {cfg:?}");

    remove_board_agent_row(&pg, &board, "personal-test-assistant")
        .await
        .unwrap();
    // Removing a row that is already gone is a no-op, not an error.
    remove_board_agent_row(&pg, &board, "personal-test-assistant")
        .await
        .unwrap();
    let cfg = get_board_agent_config(&pg, &board).await.unwrap();
    assert_eq!(cfg.models, vec!["some-editor-added-agent".to_string()]);

    cleanup(&pg, email).await;
}

/// File (or attempt to file) one open request; rows_affected is the dedup's
/// answer.
async fn file_open_request(pg: &PgPool, board: &str) -> u64 {
    sqlx::query(
        "insert into board_agent_requests (board_id, agent_model) \
         values ($1::uuid, 'personal-test-assistant') on conflict do nothing",
    )
    .bind(board)
    .execute(pg)
    .await
    .unwrap()
    .rows_affected()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn one_open_request_per_board_and_agent_and_refile_after_a_decision() {
    let pg = pool().await;
    let email = "board-access-request@link-test.invalid";
    cleanup(&pg, email).await;
    let board = throwaway_board(&pg, email).await;

    assert_eq!(
        file_open_request(&pg, &board).await,
        1,
        "the first open request lands"
    );
    assert_eq!(
        file_open_request(&pg, &board).await,
        0,
        "the partial unique index swallows the duplicate open request"
    );

    // A decision closes the open request; the index covers 'open' only, so
    // the same agent may file again afterwards.
    sqlx::query(
        "update board_agent_requests set status = 'declined', decided_at = now() \
         where board_id = $1::uuid and agent_model = 'personal-test-assistant' \
           and status = 'open'",
    )
    .bind(&board)
    .execute(&pg)
    .await
    .unwrap();
    assert_eq!(
        file_open_request(&pg, &board).await,
        1,
        "a declined request can be re-filed"
    );

    cleanup(&pg, email).await;
}
