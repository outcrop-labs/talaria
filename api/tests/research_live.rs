// Live-DB proofs for the research harness's conversation plane (cargo test
// -- --ignored). The bug this file exists to pin: research conversations are
// kind='research', and the chat door's access predicates only admitted
// 'chat' and 'plan' — so history loaded (the read side knew research) while
// every send 404'd. Each test drives the REAL predicates against the REAL
// schema, because that is the only layer the omission lived on. House rule:
// #[ignore]d, never CI.
//
//   DATABASE_URL=postgres://… cargo test --test research_live -- --ignored

use sqlx::postgres::PgPool;
use talaria_api::conversations::{accessible_conversation, conversation_accessible};
use talaria_api::research::{add_research_member, ensure_research_conversation};

async fn pool() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

async fn fabricate_user(pg: &PgPool, tag: &str) -> String {
    let sub = format!("research-live:{tag}:{}", uuid::Uuid::new_v4());
    sqlx::query("insert into users (sub) values ($1)")
        .bind(&sub)
        .execute(pg)
        .await
        .unwrap();
    let id: String = sqlx::query_scalar("select id::text from users where sub = $1")
        .bind(&sub)
        .fetch_one(pg)
        .await
        .unwrap();
    id
}

async fn sweep_user_rows(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub like $1")
        .bind(format!("research-live:{tag}:%"))
        .execute(pg)
        .await
        .unwrap();
}

/// A parked research run's bare research record, for tests that only need the
/// conversation plane (no runs row, no driver): the columns the inserts below
/// actually depend on, everything else at its default.
async fn fabricate_research_run(pg: &PgPool, owner: &str, question: &str) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "insert into research_runs \
         (id, owner_user_id, requested_by, agent_model, mode, question) \
         values ($1::uuid, $2::uuid, $3, 'live-test-model', 'brief', $4)",
    )
    .bind(&id)
    .bind(owner)
    .bind("research-live@test")
    .bind(question)
    .execute(pg)
    .await
    .unwrap();
    id
}

/// THE BUG THIS FILE IS ABOUT: a research conversation must pass the same
/// gates every other conversation kind passes — the owner, a run member, and
/// nobody else. Pre-fix, `accessible_conversation` (the chat door) answered
/// None even for the owner, which is the whole "can't chat back and forth".
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_research_conversation_passes_the_chat_gates_for_owner_and_member_only() {
    let pg = pool().await;
    let owner = fabricate_user(&pg, "gates-owner").await;
    let member = fabricate_user(&pg, "gates-member").await;
    let stranger = fabricate_user(&pg, "gates-stranger").await;
    let run_id = fabricate_research_run(&pg, &owner, "who audits the auditors?").await;

    let conv = ensure_research_conversation(&pg, &run_id)
        .await
        .unwrap()
        .expect("an owned run gets a conversation");
    add_research_member(&pg, &run_id, &member).await.unwrap();

    // The chat door — the exact predicate POST /api/chat gates with.
    for who in [&owner, &member] {
        let row = accessible_conversation(&pg, who, &conv).await.unwrap();
        let row = row.expect("the run's people pass the chat gate");
        assert_eq!(row.kind, "research");
    }
    assert!(
        accessible_conversation(&pg, &stranger, &conv)
            .await
            .unwrap()
            .is_none(),
        "a stranger with no standing on the run does not"
    );

    // The bool twin (watch gates, plan surfaces) agrees with the full one.
    assert!(conversation_accessible(&pg, &owner, &conv).await.unwrap());
    assert!(conversation_accessible(&pg, &member, &conv).await.unwrap());
    assert!(
        !conversation_accessible(&pg, &stranger, &conv)
            .await
            .unwrap()
    );

    // Losing the report loses the conversation: research access follows the
    // run, so a member removed from the run is back outside the gate.
    sqlx::query("delete from research_members where run_id = $1::uuid")
        .bind(&run_id)
        .execute(&pg)
        .await
        .unwrap();
    assert!(
        !conversation_accessible(&pg, &member, &conv).await.unwrap(),
        "membership is the run's, not the conversation's"
    );

    sweep_user_rows(&pg, "gates-owner").await;
    sweep_user_rows(&pg, "gates-member").await;
    sweep_user_rows(&pg, "gates-stranger").await;
}
