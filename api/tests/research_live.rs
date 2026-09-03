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
use talaria_api::conversations::{
    accessible_conversation, conversation_accessible, post_agent_turn,
};
use talaria_api::realtime::RealtimeDeps;
use talaria_api::runs::decide::{DecideArgs, DecideResult, decide};
use talaria_api::runs::real_decide_deps;
use talaria_api::research::{
    add_research_member, awaiting_scope_answer, ensure_research_conversation, get_research_run,
};
use talaria_api::runs::define::{DecisionOption, DecisionRequest, RunDecision};
use talaria_api::runs::store::{NewRun, PgRunStore, RunStore};

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

/// post_agent_turn is the run's mouth, and a run step that crashes between
/// saying something and parking is RE-ENTERED by the reclaim sweep: one
/// marker must mean one message, however many times the step runs.
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn an_agent_turn_posts_once_per_marker() {
    let pg = pool().await;
    let owner = fabricate_user(&pg, "turn-owner").await;
    let run_id = fabricate_research_run(&pg, &owner, "who repeats themselves?").await;
    let conv = ensure_research_conversation(&pg, &run_id)
        .await
        .unwrap()
        .expect("an owned run gets a conversation");

    let first = post_agent_turn(&pg, &conv, "live:marker", "Report ready.")
        .await
        .unwrap();
    let second = post_agent_turn(&pg, &conv, "live:marker", "Report ready.")
        .await
        .unwrap();
    assert_eq!(first, second, "the re-entry returns the posted turn's id");

    let (count,): (i64,) = sqlx::query_as(
        "select count(*) from messages \
         where conversation_id = $1::uuid and metadata->>'marker' = 'live:marker'",
    )
    .bind(&conv)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(count, 1, "one marker, one message row");

    // A different marker is a different turn — the guard keys on the marker,
    // not on the conversation.
    let other = post_agent_turn(&pg, &conv, "live:other", "Second line.")
        .await
        .unwrap();
    assert_ne!(other, first);

    sweep_user_rows(&pg, "turn-owner").await;
}

/// A run parked exactly the way the scope step parks one: research record,
/// conversation, runs row, claim, park with a free-text scope decision. The
/// store's own park() writes the awaiting state and the decision — the same
/// writes the real driver makes — so what the projection reads below is what
/// production would read.
async fn fabricate_parked_scope_run(pg: &PgPool, owner: &str, question: &str) -> String {
    let id = fabricate_research_run(pg, owner, question).await;
    ensure_research_conversation(pg, &id)
        .await
        .unwrap()
        .expect("an owned run gets a conversation");

    let store = PgRunStore::new(pg.clone());
    store
        .insert(NewRun {
            id: id.clone(),
            kind: "research".into(),
            owner_user_id: Some(owner.into()),
            subject_type: Some("research".into()),
            subject_id: Some(id.clone()),
            input: serde_json::json!({"question": question, "mode": "brief"}),
            phase: "reading the ask".into(),
        })
        .await
        .unwrap();
    match store.claim(&id, "live-test-driver", 60_000).await.unwrap() {
        talaria_api::runs::store::ClaimOutcome::Claimed { .. } => {}
        other => panic!("the fresh row must claim, got {other:?}"),
    }
    store
        .park(
            &id,
            "live-test-driver",
            RunDecision {
                request: DecisionRequest {
                    key: "scope".into(),
                    question: "Before this run starts: which database?".into(),
                    detail: None,
                    options: vec![DecisionOption {
                        id: "answered".into(),
                        label: "Answered".into(),
                        detail: None,
                    }],
                    href: Some(format!("/research/{id}")),
                    free_text: true,
                },
                answer: None,
            },
            format!("scope:{id}"),
            "asking".into(),
        )
        .await
        .unwrap()
        .unwrap();
    id
}

/// THE TICKET-#5 LESSON, as a wire contract: a parked run is 'awaiting', its
/// own word — not a shade of 'running' — and it spells the question it waits
/// on, free-text flag and all, so the surface can say "reply in the
/// discussion" instead of drawing buttons. Read through the projection the
/// list and run routes share, because the five-value CASE is the only
/// sanctioned spelling of "is this run alive".
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_parked_scope_run_reads_as_awaiting_with_its_free_text_question() {
    let pg = pool().await;
    let owner = fabricate_user(&pg, "awaiting-owner").await;
    let id = fabricate_parked_scope_run(&pg, &owner, "which database should we move to?").await;

    let (run, _sources) = get_research_run(&pg, &id)
        .await
        .unwrap()
        .expect("the parked run is still a run");
    assert_eq!(run.status, "awaiting", "its own word, not 'running'");
    let waiting = run.awaiting.expect("the open question is spelled");
    assert_eq!(waiting["key"], "scope");
    assert_eq!(
        waiting["free_text"], true,
        "the surface answers this one in the discussion, not with buttons"
    );
    assert_eq!(waiting["options"].as_array().map(Vec::len), Some(1));
    assert!(
        run.conversation_id.is_some(),
        "the discussion exists to answer in"
    );

    // And the chat plane's lookup agrees: this conversation's run is the one
    // waiting on a scope answer.
    let conv = run.conversation_id.clone().unwrap();
    assert_eq!(
        talaria_api::research::awaiting_scope_answer(&pg, &conv)
            .await
            .unwrap()
            .as_deref(),
        Some(id.as_str())
    );

    sqlx::query("delete from runs where id = $1::uuid")
        .bind(&id)
        .execute(&pg)
        .await
        .unwrap();
    sweep_user_rows(&pg, "awaiting-owner").await;
}

/// The resume hook's whole contract: a conversation's parked scope run is
/// findable BEFORE the answer and gone AFTER — a reply is spent once, not
/// re-spent on every message that follows. The answer is spent through
/// decide() exactly the way the chat hook spends it (minus the detached
/// resume, which a test does not want).
#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL, REDIS_URL)"]
async fn answering_the_scope_park_clears_the_resume_hook() {
    let pg = pool().await;
    let owner = fabricate_user(&pg, "resume-owner").await;
    let id = fabricate_parked_scope_run(&pg, &owner, "which cache, and how warm?").await;
    let conv = get_research_run(&pg, &id)
        .await
        .unwrap()
        .expect("the run reads back")
        .0
        .conversation_id
        .expect("the discussion exists");

    // BEFORE: the hook finds exactly this run.
    assert_eq!(
        awaiting_scope_answer(&pg, &conv).await.unwrap().as_deref(),
        Some(id.as_str())
    );

    // THE ANSWER, the chat hook's call verbatim. decide() resolves who may
    // answer through the kind's registered definition, which registers on
    // first use — a test binary that never started a run must touch it.
    let _ = talaria_api::runs::defs::research::research_run();
    let redis_url =
        std::env::var("REDIS_URL").expect("set REDIS_URL (source ui/.env) for the decide deps");
    let redis = redis::aio::ConnectionManager::new(
        redis::Client::open(redis_url).expect("parse redis url"),
    )
    .await
    .expect("connect redis");
    let deps = real_decide_deps(
        pg.clone(),
        redis.clone(),
        RealtimeDeps::publish_only(Some(redis)),
    );
    let res = decide(
        DecideArgs {
            run_id: id.clone(),
            option_id: "answered".into(),
            note: Some("redis, and it must survive a restart".into()),
            by: owner.clone(),
            start: Some(false), // queued for the sweep, not driven here
        },
        &deps,
    )
    .await
    .unwrap();
    assert!(
        matches!(res, DecideResult::Decided { .. }),
        "the owner's reply answers its own run, got {res:?}"
    );

    // AFTER: the hook finds nothing — the next message in this conversation
    // is a persona turn again, not a second answer to a spent question.
    assert_eq!(awaiting_scope_answer(&pg, &conv).await.unwrap(), None);

    // And the authority is decide()'s, not the hook's: a run MEMBER can chat
    // in this conversation but cannot answer the owner's park — their
    // message spends nothing, and the run stays parked for the owner.
    let member = fabricate_user(&pg, "resume-member").await;
    let id2 = fabricate_parked_scope_run(&pg, &owner, "a second, member-watched ask").await;
    add_research_member(&pg, &id2, &member).await.unwrap();
    let conv2 = get_research_run(&pg, &id2)
        .await
        .unwrap()
        .expect("the second run reads back")
        .0
        .conversation_id
        .expect("its discussion exists");
    let res2 = decide(
        DecideArgs {
            run_id: id2.clone(),
            option_id: "answered".into(),
            note: Some("speaking as a member, not the owner".into()),
            by: member.clone(),
            start: Some(false),
        },
        &deps,
    )
    .await
    .unwrap();
    assert!(
        matches!(
            res2,
            DecideResult::Refused {
                reason: talaria_api::runs::decide::DecideRefusal::Forbidden,
                ..
            }
        ),
        "a member does not answer the owner's park, got {res2:?}"
    );
    assert_eq!(
        awaiting_scope_answer(&pg, &conv2).await.unwrap().as_deref(),
        Some(id2.as_str()),
        "refused spends nothing — still parked"
    );

    for row_id in [&id, &id2] {
        sqlx::query("delete from runs where id = $1::uuid")
            .bind(row_id)
            .execute(&pg)
            .await
            .unwrap();
    }
    sweep_user_rows(&pg, "resume-owner").await;
    sweep_user_rows(&pg, "resume-member").await;
}
