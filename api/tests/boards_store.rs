// Live-DB proof of boards.rs (cargo test -- --ignored). The agent listing is
// one SELECT whose legality is a Postgres rule — DISTINCT demands every
// ORDER BY expression in the select list — and a query that violates it does
// not fail a unit test, it 500s on the first agent that ever calls it: the
// port rewrote the timestamps as epoch expressions and left the sort on the
// raw column, and the statement was refused outright until the fleet's first
// smoke test found it. These tests execute the listing against the real
// table so an illegal shape can never land again. House rule: #[ignore]d,
// never CI.
//
//   DATABASE_URL=postgres://… cargo test --test boards_store -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::boards::list_boards_for_agent;

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

/// Everything this suite fabricates hangs off one throwaway user, so the
/// cascade removes board, board_agents and grants along with it.
async fn cleanup(pg: &PgPool) {
    sqlx::query("delete from users where email = 'boards-test@link-test.invalid'")
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_agent_listing_is_a_legal_statement_on_an_empty_result() {
    let pg = pool().await;
    // A model with no per-agent boards still runs the FULL statement, and on
    // a shared database it still sees the allow_all_agents boards — the
    // point is that Postgres accepts the statement at all, which is what
    // .expect proves.
    let _boards = list_boards_for_agent(&pg, "no-such-agent-personal")
        .await
        .expect("the listing is a legal statement");
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn the_agent_listing_returns_the_board_the_policy_admits() {
    let pg = pool().await;
    cleanup(&pg).await;
    let (owner,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) \
         values ('boards-test-owner', 'boards-test@link-test.invalid', 'Boards Test', 'member') \
         returning id::text",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    let (board,): (String,) = sqlx::query_as(
        "insert into boards (name, owner_id) \
         values ('boards-store live test', $1::uuid) returning id::text",
    )
    .bind(&owner)
    .fetch_one(&pg)
    .await
    .unwrap();
    // The policy fragment's per-agent arm (not allow_all): exactly this
    // model may see exactly this board.
    sqlx::query(
        "insert into board_agents (board_id, agent_model) \
         values ($1::uuid, 'boards-test-personal')",
    )
    .bind(&board)
    .execute(&pg)
    .await
    .unwrap();

    let listed = list_boards_for_agent(&pg, "boards-test-personal")
        .await
        .expect("the listing is a legal statement");
    let hit = listed
        .iter()
        .find(|b| b.id == board)
        .expect("the admitted board is listed");
    assert_eq!(hit.name, "boards-store live test");
    assert_eq!(hit.owner_id, owner);
    assert!(
        hit.updated_at.starts_with("20"),
        "the epoch-ms column maps to an ISO date, got {}",
        hit.updated_at
    );

    // A different model sees nothing: the admission is per agent.
    let other = list_boards_for_agent(&pg, "boards-test-other")
        .await
        .expect("the listing is a legal statement");
    assert!(other.iter().all(|b| b.id != board));

    cleanup(&pg).await;
}
