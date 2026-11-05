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
use crate::record::{DraftFields, Held, MISSING_RUN_GRACE_MS, STALLED, SkillDraft};

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
            .design
            .expect("a design");
        if d.status != DesignStatus::Generating {
            // The landing files (or re-alerts) its bell row just after the
            // record lands and before its run is done: settled means both,
            // or a test reading the bell races the landing.
            if let Some(run) = d.run_id.as_deref() {
                for _ in 0..300 {
                    let state: Option<(String,)> =
                        sqlx::query_as("select state from runs where id = $1::uuid")
                            .bind(run)
                            .fetch_optional(&st.pg)
                            .await
                            .unwrap();
                    if state.is_none_or(|(s,)| !matches!(s.as_str(), "queued" | "running")) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
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
    for _ in 0..300 {
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
    // The landing files its bell row (if any) before its run is done.
    for _ in 0..300 {
        if run_state(&st.pg, d.run_id.as_deref().unwrap()).await == "done" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
    let saved = edit_design(&st, &user, Some(&ready.id), edited.clone())
        .await
        .unwrap();
    assert_eq!(saved.draft.as_ref().unwrap().role, "Lead Analyst");
    let refining = refine_design(
        &st,
        &user,
        CALLER,
        Some(&ready.id),
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
        edit_design(&st, &user, Some(&ready.id), edited.clone()).await,
        Err(DesignError::Refused(Refusal::Generating))
    ));
    let d = until_settled(&user).await;
    // The landing re-alerts the bell row just after the record lands; wait
    // for its run to finish so the discard below reads the final row.
    for _ in 0..300 {
        if run_state(&st.pg, d.run_id.as_deref().unwrap()).await == "done" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
            .design
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
    assert!(
        current_design(&st, &user, false)
            .await
            .unwrap()
            .design
            .is_none()
    );
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
    for _ in 0..300 {
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
        (Uuid::new_v4().to_string(), Uuid::new_v4().to_string()),
        record::RunSeen::Live,
    ) else {
        unreachable!()
    };
    store::update(&mut conn, &user, |_| Write::put(orphan.clone(), ()))
        .await
        .unwrap();
    let d = current_design(&st, &user, false)
        .await
        .unwrap()
        .design
        .unwrap();
    assert_eq!(d.status, DesignStatus::Error);
    assert_eq!(d.error.as_deref(), Some(STALLED));
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "stalled").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_discarded_design_cannot_be_written_back() {
    // S1: the landing that read the old record and swapped after a discard
    // and a fresh describe. The revision is never reused, so its swap fails.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "aba").await;
    let mut conn = st.redis().await.unwrap();
    start_design(&st, &user, CALLER, "a slow first design")
        .await
        .unwrap();
    let stale = store::read(&mut conn, &user).await.unwrap().unwrap();
    discard_design(&st, &user, None).await.unwrap();
    let second = start_design(&st, &user, CALLER, "a slow second design")
        .await
        .unwrap();
    let mut resurrected = stale.design.clone();
    resurrected.status = DesignStatus::Ready;
    assert!(
        !store::swap(&mut conn, &user, &stale.rev, Some(&resurrected))
            .await
            .unwrap(),
        "a swap naming a revision from before the discard must not land"
    );
    let now = store::read(&mut conn, &user).await.unwrap().unwrap();
    assert_eq!(now.design.id, second.id);
    assert_ne!(now.rev, stale.rev);
    until_settled(&user).await;
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "aba").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_stale_tab_cannot_edit_or_refine_the_newer_design() {
    // S5: tab A holds design X; elsewhere X is discarded and a manual design
    // Z started. A's save and refine name X and are refused; Z is untouched.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "stale").await;
    start_design(&st, &user, CALLER, "design x").await.unwrap();
    let x = until_settled(&user).await;
    discard_design(&st, &user, Some(&x.id)).await.unwrap();
    let z = edit_design(&st, &user, None, draft_for("manual z"))
        .await
        .unwrap();
    let mut from_a = x.draft.clone().unwrap();
    from_a.role = "written by the stale tab".into();
    assert!(matches!(
        edit_design(&st, &user, Some(&x.id), from_a.clone()).await,
        Err(DesignError::Refused(Refusal::Stale))
    ));
    assert!(matches!(
        edit_design(&st, &user, None, from_a.clone()).await,
        Err(DesignError::Refused(Refusal::Stale))
    ));
    assert!(matches!(
        refine_design(
            &st,
            &user,
            CALLER,
            Some(&x.id),
            "make it terse",
            Some(from_a)
        )
        .await,
        Err(DesignError::Refused(Refusal::Stale))
    ));
    let held = current_design(&st, &user, false)
        .await
        .unwrap()
        .design
        .unwrap();
    assert_eq!(held.id, z.id);
    assert_eq!(held.draft, z.draft);
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "stale").await;
}

async fn hire_row(pg: &PgPool, id: &str, state: &str, error: Option<&str>, created: bool) {
    let checkpoint = if created {
        serde_json::json!({ "defId": "d-made", "stage": "boot" })
    } else {
        serde_json::json!(null)
    };
    sqlx::query(
        "insert into runs (id, kind, state, error, checkpoint) values ($1::uuid, 'agent-hire', $2, $3, $4) \
         on conflict (id) do update set state = excluded.state, error = excluded.error, checkpoint = excluded.checkpoint",
    )
    .bind(id)
    .bind(state)
    .bind(error)
    .bind(checkpoint)
    .execute(pg)
    .await
    .unwrap();
}

async fn held_of(st: &AppState, user: &str) -> Vec<Held> {
    store::read_held(&mut st.redis().await.unwrap(), user)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.held)
        .collect()
}

/// A design on a stand-in hire that has just failed before create.
async fn failed_hold(st: &AppState, user: &str, purpose: &str) -> (AgentDesign, String) {
    start_design(st, user, CALLER, purpose).await.unwrap();
    leave(user).await;
    let d = until_settled(user).await;
    let h = Uuid::new_v4().to_string();
    claim_for_hire(st, user, &d.id, &h).await.unwrap();
    hire_row(&st.pg, &h, "error", Some("the department is taken"), false).await;
    (d, h)
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_the_design_gates_the_hire_and_new_agent_stays_open() {
    // S7 + NS5: one design, one agent; the slot is free for the next design
    // while the hire runs; a pre-create failure hands it back only into an
    // empty slot and never over the design started meanwhile.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "gate").await;
    start_design(&st, &user, CALLER, "agent a").await.unwrap();
    leave(&user).await;
    let a = until_settled(&user).await;
    for _ in 0..300 {
        if !notices(&st.pg, &user).await.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (h1, h2) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    claim_for_hire(&st, &user, &a.id, &h1).await.unwrap();
    hire_row(&st.pg, &h1, "awaiting", None, false).await;
    // A second tab hiring the same design: refused.
    assert!(matches!(
        claim_for_hire(&st, &user, &a.id, &h2).await,
        Err(DesignError::Refused(Refusal::Hiring))
    ));
    assert!(
        notices(&st.pg, &user)
            .await
            .iter()
            .all(|(_, _, read)| *read)
    );
    // New agent is open: the slot is empty and a describe of B starts.
    let view = current_design(&st, &user, false).await.unwrap();
    assert!(view.design.is_none());
    assert_eq!(view.hiring.len(), 1);
    start_design(&st, &user, CALLER, "agent b").await.unwrap();
    let b = until_settled(&user).await;

    // A's hire fails before the agent exists: B stays, A waits (bell).
    hire_row(
        &st.pg,
        &h1,
        "error",
        Some("the department research is taken"),
        false,
    )
    .await;
    let view = current_design(&st, &user, false).await.unwrap();
    assert_eq!(
        view.design.as_ref().unwrap().id,
        b.id,
        "the design started meanwhile is never overwritten"
    );
    assert_eq!(view.returned.len(), 1);
    assert!(
        view.returned[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("the department research is taken")
    );
    let rows = notices(&st.pg, &user).await;
    assert!(
        rows.iter()
            .any(|(t, _, read)| t.starts_with("The hire of") && !read)
    );
    // B is discarded: A comes back into the slot, saying why.
    discard_design(&st, &user, Some(&b.id)).await.unwrap();
    let back = current_design(&st, &user, false)
        .await
        .unwrap()
        .design
        .unwrap();
    assert_eq!(back.id, a.id);
    assert_eq!(back.draft, a.draft);
    assert!(
        back.error
            .as_deref()
            .unwrap()
            .contains("the department research is taken")
    );
    assert!(held_of(&st, &user).await.is_empty());

    // Fixed and hired again; this hire makes the agent and spends the design
    // even though its boot failed afterwards (NS8).
    let mut fixed = a.draft.clone().unwrap();
    fixed.department = "research-2".into();
    edit_design(&st, &user, Some(&a.id), fixed).await.unwrap();
    let h3 = Uuid::new_v4().to_string();
    claim_for_hire(&st, &user, &a.id, &h3).await.unwrap();
    hire_row(
        &st.pg,
        &h3,
        "error",
        Some("the container did not become healthy"),
        true,
    )
    .await;
    let view = current_design(&st, &user, false).await.unwrap();
    assert!(view.design.is_none() && view.returned.is_empty() && view.hiring.is_empty());

    // An enqueue that failed hands it straight back into the empty slot.
    start_design(&st, &user, CALLER, "agent c").await.unwrap();
    let c = until_settled(&user).await;
    let h4 = Uuid::new_v4().to_string();
    claim_for_hire(&st, &user, &c.id, &h4).await.unwrap();
    release_hire(st.clone(), user.clone(), h4.clone()).await;
    assert_eq!(
        current_design(&st, &user, false)
            .await
            .unwrap()
            .design
            .unwrap()
            .id,
        c.id
    );

    for h in [&h1, &h3] {
        sqlx::query("delete from runs where id = $1::uuid")
            .bind(h)
            .execute(&st.pg)
            .await
            .unwrap();
    }
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "gate").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_waiting_returned_design_can_be_dismissed() {
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "dismiss").await;
    start_design(&st, &user, CALLER, "agent a").await.unwrap();
    let a = until_settled(&user).await;
    let h1 = Uuid::new_v4().to_string();
    claim_for_hire(&st, &user, &a.id, &h1).await.unwrap();
    edit_design(&st, &user, None, draft_for("manual b"))
        .await
        .unwrap();
    hire_row(&st.pg, &h1, "error", None, false).await;
    assert_eq!(
        current_design(&st, &user, false)
            .await
            .unwrap()
            .returned
            .len(),
        1
    );
    assert!(discard_design(&st, &user, Some(&a.id)).await.unwrap());
    let view = current_design(&st, &user, false).await.unwrap();
    assert!(view.returned.is_empty());
    assert_eq!(view.design.unwrap().draft.unwrap().name, "Scout");
    sqlx::query("delete from runs where id = $1::uuid")
        .bind(&h1)
        .execute(&st.pg)
        .await
        .unwrap();
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "dismiss").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_retried_hire_takes_the_design_with_it() {
    // NS3: the hiring strip's Retry runs a new hire; the design follows it
    // (held, or already handed back) and is spent when the retry finishes.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "retry").await;
    start_design(&st, &user, CALLER, "a scout to retry")
        .await
        .unwrap();
    let d = until_settled(&user).await;
    let (h1, r1) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    claim_for_hire(&st, &user, &d.id, &h1).await.unwrap();
    hire_row(&st.pg, &h1, "error", Some("a transient failure"), false).await;
    let back = current_design(&st, &user, false)
        .await
        .unwrap()
        .design
        .unwrap();
    assert_eq!(back.id, d.id, "handed back into the empty slot");
    follow_retry(&st, &user, &h1, &r1).await;
    hire_row(&st.pg, &r1, "awaiting", None, false).await;
    let view = current_design(&st, &user, false).await.unwrap();
    assert!(view.design.is_none());
    assert_eq!(view.hiring[0].hire_run_id, r1);
    hire_row(&st.pg, &r1, "done", None, true).await;
    let view = current_design(&st, &user, false).await.unwrap();
    assert!(view.design.is_none() && view.hiring.is_empty() && view.returned.is_empty());
    for h in [&h1, &r1] {
        sqlx::query("delete from runs where id = $1::uuid")
            .bind(h)
            .execute(&st.pg)
            .await
            .unwrap();
    }
    cleanup(&st.pg, "retry").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_two_reads_settle_one_failure_once() {
    // NS9: two tabs (or a poll and an event) read at once as the hire fails.
    // The design ends in ONE place — the slot — never also in the hires.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "twin").await;
    for round in 0..5 {
        let (d, h) = failed_hold(&st, &user, &format!("twin read {round}")).await;
        let (a, b) = (fresh_state().await, fresh_state().await);
        let (va, vb) = tokio::join!(
            current_design(&a, &user, false),
            current_design(&b, &user, false)
        );
        va.unwrap();
        vb.unwrap();
        let view = current_design(&st, &user, false).await.unwrap();
        assert_eq!(
            view.design.as_ref().map(|x| x.id.as_str()),
            Some(d.id.as_str()),
            "round {round}"
        );
        assert!(
            view.returned.is_empty() && view.hiring.is_empty(),
            "round {round}: also left in the hires"
        );
        assert!(held_of(&st, &user).await.is_empty(), "round {round}");
        let waits: Vec<(String, String, bool)> = notices(&st.pg, &user)
            .await
            .into_iter()
            .filter(|(t, _, _)| t.starts_with("The hire of"))
            .collect();
        assert!(
            waits.is_empty(),
            "round {round}: a waiting row about the open design: {waits:?}"
        );
        sqlx::query("delete from runs where id = $1::uuid")
            .bind(&h)
            .execute(&st.pg)
            .await
            .unwrap();
        discard_design(&st, &user, None).await.unwrap();
    }
    cleanup(&st.pg, "twin").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_read_racing_the_retry_follow_cannot_undo_it() {
    // NS9's second case: a read settling the failure and the strip's Retry
    // following it, at once, in either order: the design ends on the retry.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "race").await;
    for round in 0..5 {
        let (d, h) = failed_hold(&st, &user, &format!("race {round}")).await;
        let r = Uuid::new_v4().to_string();
        let (a, b) = (fresh_state().await, fresh_state().await);
        let (_, _) = tokio::join!(
            current_design(&a, &user, false),
            follow_retry(&b, &user, &h, &r)
        );
        hire_row(&st.pg, &r, "awaiting", None, false).await;
        let view = current_design(&st, &user, false).await.unwrap();
        assert!(
            view.design.as_ref().is_none_or(|x| x.id != d.id),
            "round {round}: back in the slot for an agent the retry is making"
        );
        assert_eq!(view.hiring.len(), 1, "round {round}");
        assert_eq!(view.hiring[0].hire_run_id, r, "round {round}");
        hire_row(&st.pg, &r, "done", None, true).await;
        let view = current_design(&st, &user, false).await.unwrap();
        assert!(
            view.hiring.is_empty() && view.returned.is_empty(),
            "round {round}"
        );
        for x in [&h, &r] {
            sqlx::query("delete from runs where id = $1::uuid")
                .bind(x)
                .execute(&st.pg)
                .await
                .unwrap();
        }
    }
    cleanup(&st.pg, "race").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_the_waiting_bell_row_is_filed_once() {
    // NS10: reading "The hire of A did not finish" must not file it again.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "once").await;
    start_design(&st, &user, CALLER, "agent a").await.unwrap();
    leave(&user).await;
    let a = until_settled(&user).await;
    let h = Uuid::new_v4().to_string();
    claim_for_hire(&st, &user, &a.id, &h).await.unwrap();
    edit_design(&st, &user, None, draft_for("manual b"))
        .await
        .unwrap();
    hire_row(&st.pg, &h, "error", None, false).await;
    let waits = |pg: PgPool, user: String| async move {
        let rows: Vec<(bool,)> = sqlx::query_as(
            "select read_at is not null from notifications where user_id = $1::uuid and title like 'The hire of %'",
        )
        .bind(user)
        .fetch_all(&pg)
        .await
        .unwrap();
        rows
    };
    current_design(&st, &user, false).await.unwrap();
    current_design(&fresh_state().await, &user, false)
        .await
        .unwrap();
    assert_eq!(
        waits(st.pg.clone(), user.clone()).await,
        vec![(false,)],
        "one row, unread"
    );
    // The person reads it; the next reads (the link it opens, a poll) file nothing.
    sqlx::query("update notifications set read_at = now() where user_id = $1::uuid and title like 'The hire of %'")
        .bind(&user)
        .execute(&st.pg)
        .await
        .unwrap();
    for _ in 0..3 {
        current_design(&st, &user, false).await.unwrap();
    }
    assert_eq!(
        waits(st.pg.clone(), user.clone()).await,
        vec![(true,)],
        "still the one row, read"
    );
    assert_eq!(
        current_design(&st, &user, false)
            .await
            .unwrap()
            .returned
            .len(),
        1
    );
    sqlx::query("delete from runs where id = $1::uuid")
        .bind(&h)
        .execute(&st.pg)
        .await
        .unwrap();
    discard_design(&st, &user, Some(&a.id)).await.unwrap();
    discard_design(&st, &user, None).await.unwrap();
    cleanup(&st.pg, "once").await;
}

#[tokio::test]
#[ignore = "live: set DATABASE_URL and REDIS_URL"]
async fn design_live_a_bell_row_about_a_design_gone_is_read() {
    // NN10: a design that expired (or ended elsewhere) leaves no row pointing at it.
    LazyLock::force(&ARMED);
    let st = fresh_state().await;
    let user = person(&st.pg, "orphan").await;
    start_design(&st, &user, CALLER, "an orphan").await.unwrap();
    leave(&user).await;
    until_settled(&user).await;
    assert_eq!(notices(&st.pg, &user).await.len(), 1);
    // Gone without a discard (as a TTL expiry leaves it).
    let mut conn = st.redis().await.unwrap();
    let _: () = redis::cmd("DEL")
        .arg(store::key(&user))
        .query_async(&mut conn)
        .await
        .unwrap();
    current_design(&st, &user, false).await.unwrap();
    assert!(
        notices(&st.pg, &user)
            .await
            .iter()
            .all(|(_, _, read)| *read)
    );
    cleanup(&st.pg, "orphan").await;
}
