// Live proof that a design is the server's: Postgres for the run row and the
// bell, Redis for the record, the real runs runtime driving the real step —
// only the model is a stand-in (an instruction containing "slow" answers in
// 1.5 s, "fail" answers the muse's refusal). `#[ignore]`d; CI's live-tests
// job runs them (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… REDIS_URL=redis://… \
//     cargo test -p talaria-agent-design --lib -- --ignored design_live
//
// "The client went away" is literal here: `start_design` returns before the
// model answers, nothing holds a request open, and every later read comes
// from a FRESH AppState (its own pool and Redis connection), the way a
// reloaded tab or another api process would ask.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use sqlx::PgPool;
use talaria_state::AppState;
use talaria_tasks_types::BUILD_DISPATCH;

use super::*;
use crate::record::{DraftFields, MISSING_RUN_GRACE_MS, STALLED, SkillDraft};

async fn fresh_state() -> AppState {
    let url =
        std::env::var("DATABASE_URL").expect("set DATABASE_URL to run the ignored live tests");
    let redis = std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6399".into());
    let cfg = talaria_config::Config::from_parts(
        url.clone(),
        redis,
        "agent-design-live-root".into(),
        String::new(),
        String::new(),
        String::new(),
    )
    .expect("test config assembles");
    AppState::new(PgPool::connect(&url).await.expect("connect"), Arc::new(cfg))
}

fn draft_for(instruction: &str) -> DraftFields {
    let name = if instruction.contains("terse") {
        "Terse"
    } else {
        "Scout"
    };
    DraftFields {
        name: name.into(),
        handle: name.to_lowercase(),
        department: "research".into(),
        role: "Research Analyst".into(),
        soul: format!("Designed from: {instruction}"),
        skills: vec![SkillDraft {
            name: "weekly-digest".into(),
            content: "# Weekly digest".into(),
        }],
    }
}

/// Arm the step once per test process: a stand-in muse, the REAL landing
/// (record write + bell), and the real run dispatch.
static ARMED: LazyLock<()> = LazyLock::new(|| {
    let _ = BUILD_DISPATCH.set(Arc::new(|pg, redis, rt| {
        talaria_runs_decide::assembly::real_run_deps(pg, redis, rt)
    }));
    arm_agent_design_step(AgentDesignDeps {
        draft: Arc::new(|input: AgentDesignInput| {
            Box::pin(async move {
                if input.instruction.contains("slow") {
                    tokio::time::sleep(Duration::from_millis(1_500)).await;
                }
                if input.instruction.contains("fail") {
                    return Err(UNUSABLE_AGENT.to_string());
                }
                Ok(draft_for(&input.instruction))
            })
        }),
        land: Arc::new(|landing| {
            Box::pin(async move {
                let st = fresh_state().await;
                land_answer(&st, landing).await
            })
        }),
    });
});

async fn person(pg: &PgPool, tag: &str) -> String {
    cleanup(pg, tag).await;
    let (id,): (String,) = sqlx::query_as(
        "insert into users (sub, email, name, role) values ($1, $2, 'Design Live', 'admin') \
         returning id::text",
    )
    .bind(format!("agent-design-live-{tag}"))
    .bind(format!("agent-design-live-{tag}@example.com"))
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn cleanup(pg: &PgPool, tag: &str) {
    sqlx::query("delete from users where sub = $1")
        .bind(format!("agent-design-live-{tag}"))
        .execute(pg)
        .await
        .unwrap();
}

/// Read from a fresh session until the design leaves `generating`.
async fn until_settled(user: &str) -> AgentDesign {
    for _ in 0..200 {
        let st = fresh_state().await;
        let d = current_design(&st, user, false)
            .await
            .unwrap()
            .expect("a design");
        if d.status != DesignStatus::Generating {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("the design never settled");
}

async fn run_state(pg: &PgPool, run_id: &str) -> String {
    let (s,): (String,) = sqlx::query_as("select state from runs where id = $1::uuid")
        .bind(run_id)
        .fetch_one(pg)
        .await
        .unwrap();
    s
}

async fn notices(pg: &PgPool, user: &str) -> Vec<(String, String, bool)> {
    sqlx::query_as(
        "select title, href, read_at is not null from notifications \
         where user_id = $1::uuid and kind = 'agent-design' order by created_at",
    )
    .bind(user)
    .fetch_all(pg)
    .await
    .unwrap()
}

const CALLER: &str = "platform:muse:agent-design-live";

/// The tab that started it closed: its watch mark (set by the start, renewed
/// by an open modal's poll) is gone before the answer lands.
async fn leave(user: &str) {
    let mut conn = fresh_state().await.redis().await.unwrap();
    let _: () = redis::cmd("DEL")
        .arg(store::watch_key(user))
        .query_async(&mut conn)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_runs_to_ready_with_no_client_and_resumes_from_a_fresh_session() {
    LazyLock::force(&ARMED);
    let started_state = fresh_state().await;
    let pg = started_state.pg.clone();
    let user = person(&pg, "ready").await;
    let started = start_design(&started_state, &user, CALLER, "a slow release manager")
        .await
        .unwrap();
    assert_eq!(started.status, DesignStatus::Generating);
    // The browser leaves: nothing that started it is kept.
    drop(started_state);
    leave(&user).await;

    let d = until_settled(&user).await;
    assert_eq!(d.status, DesignStatus::Ready);
    assert_eq!(d.id, started.id);
    assert_eq!(d.draft.as_ref().unwrap().name, "Scout");
    assert_eq!(d.chat.len(), 2);
    assert!(d.finished_at.unwrap() >= d.started_at.unwrap() + 1_400);
    let run_id = d.run_id.clone().unwrap();
    // The run row reaches `done` just after the landing write.
    for _ in 0..50 {
        if run_state(&pg, &run_id).await == "done" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(run_state(&pg, &run_id).await, "done");
    // Nobody watched it land: one bell row, unread, back to the review step.
    assert_eq!(
        notices(&pg, &user).await,
        vec![(
            "Agent design ready: Scout".to_string(),
            REVIEW_HREF.to_string(),
            false
        )]
    );
    // It waits at least a day for a hire or a discard.
    let mut conn = fresh_state().await.redis().await.unwrap();
    let ttl: i64 = redis::cmd("TTL")
        .arg(store::key(&user))
        .query_async(&mut conn)
        .await
        .unwrap();
    assert!(
        ttl > 24 * 3600 && ttl <= store::DESIGN_TTL_SECS as i64,
        "ttl {ttl}"
    );
    discard_design(&fresh_state().await, &user, None)
        .await
        .unwrap();
    cleanup(&pg, "ready").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_second_start_while_generating_follows_the_first() {
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "second").await;
    let first = start_design(&st, &user, CALLER, "a slow scout")
        .await
        .unwrap();
    // A second tab, another process: the same run comes back.
    let again = start_design(&fresh_state().await, &user, CALLER, "something else")
        .await
        .unwrap();
    assert_eq!(again.id, first.id);
    assert_eq!(again.run_id, first.run_id);
    assert_eq!(again.purpose, "a slow scout");
    let d = until_settled(&user).await;
    assert_eq!(d.status, DesignStatus::Ready);
    // Idle now: a describe replaces it, and its bell row is read.
    let next = start_design(&st, &user, CALLER, "a new intent")
        .await
        .unwrap();
    assert_ne!(next.id, first.id);
    assert!(
        notices(&st.pg, &user)
            .await
            .iter()
            .all(|(_, _, read)| *read)
    );
    until_settled(&user).await;
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "second").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_refine_then_discard() {
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "refine").await;
    start_design(&st, &user, CALLER, "a slow scout")
        .await
        .unwrap();
    leave(&user).await;
    let ready = until_settled(&user).await;
    let mut edited = ready.draft.clone().unwrap();
    edited.role = "Lead Analyst".into();
    // Hand edits are the server's too.
    let saved = edit_design(&st, &user, edited.clone()).await.unwrap();
    assert_eq!(saved.draft.as_ref().unwrap().role, "Lead Analyst");
    let refining = refine_design(
        &st,
        &user,
        CALLER,
        "make it terse, slowly",
        Some(edited.clone()),
    )
    .await
    .unwrap();
    leave(&user).await;
    assert!(refining.refining);
    assert_eq!(refining.status, DesignStatus::Generating);
    // An edit while the refine runs is refused, not lost.
    assert!(matches!(
        edit_design(&st, &user, edited.clone()).await,
        Err(DesignError::Refused(Refusal::Generating))
    ));
    let d = until_settled(&user).await;
    assert_eq!(d.draft.as_ref().unwrap().name, "Terse");
    assert_eq!(d.previous, Some(edited));
    assert_eq!(d.chat.len(), 4);
    assert_eq!(d.chat[2].content, "make it terse, slowly");
    // One bell row for the design, re-alerted by the refine's landing.
    assert_eq!(notices(&st.pg, &user).await.len(), 1);

    assert!(discard_design(&st, &user, Some(&d.id)).await.unwrap());
    assert!(
        current_design(&fresh_state().await, &user, false)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        notices(&st.pg, &user)
            .await
            .iter()
            .all(|(_, _, read)| *read)
    );
    // Gone is gone: a second discard finds nothing.
    assert!(!discard_design(&st, &user, Some(&d.id)).await.unwrap());
    cleanup(&st.pg, "refine").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_discard_mid_generation_cancels_and_drops_the_answer() {
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "discard").await;
    let d = start_design(&st, &user, CALLER, "a slow auditor")
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(discard_design(&st, &user, None).await.unwrap());
    // The model answers after the discard; the answer finds no record.
    tokio::time::sleep(Duration::from_millis(2_000)).await;
    assert!(current_design(&st, &user, false).await.unwrap().is_none());
    assert_eq!(
        run_state(&st.pg, d.run_id.as_deref().unwrap()).await,
        "cancelled"
    );
    assert!(notices(&st.pg, &user).await.is_empty());
    cleanup(&st.pg, "discard").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_watched_landing_files_no_notice_and_a_failure_lands_its_sentence() {
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "watched").await;
    start_design(&st, &user, CALLER, "a slow watcher")
        .await
        .unwrap();
    // The open modal's poll renews the mark the start set.
    leave(&user).await;
    current_design(&st, &user, true).await.unwrap();
    assert_eq!(until_settled(&user).await.status, DesignStatus::Ready);
    assert!(notices(&st.pg, &user).await.is_empty());

    start_design(&st, &user, CALLER, "please fail")
        .await
        .unwrap();
    let d = until_settled(&user).await;
    assert_eq!(d.status, DesignStatus::Error);
    assert_eq!(d.error.as_deref(), Some(UNUSABLE_AGENT));
    for _ in 0..50 {
        if run_state(&st.pg, d.run_id.as_deref().unwrap()).await == "error" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        run_state(&st.pg, d.run_id.as_deref().unwrap()).await,
        "error"
    );
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "watched").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_design_whose_run_is_gone_reads_as_stalled() {
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "stalled").await;
    let mut conn = st.redis().await.unwrap();
    // A record a dead process left: generating, its run row never written.
    let old = now_ms() - MISSING_RUN_GRACE_MS - 5_000;
    let Start::Fresh(orphan) = record::start(
        None,
        "an orphan",
        old,
        Uuid::new_v4().to_string(),
        Uuid::new_v4().to_string(),
    ) else {
        unreachable!()
    };
    store::update(&mut conn, &user, |_| Write::put(orphan.clone(), ()))
        .await
        .unwrap();
    let d = current_design(&st, &user, false).await.unwrap().unwrap();
    assert_eq!(d.status, DesignStatus::Error);
    assert_eq!(d.error.as_deref(), Some(STALLED));
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "stalled").await;
}
