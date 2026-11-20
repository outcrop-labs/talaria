// Live-DB proof of the plan gate's write in PUT /api/workbench/jobs. The route
// reads a job as awaiting approval, then writes its verdict. A rewind that lands
// between the two must win. A transaction held open on the row puts the rewind
// exactly there: the route reads past it and blocks on its write.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test -p talaria-api --test it workbench_approval_race_live:: -- --ignored

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::json;
use sqlx::PgPool;
use talaria_api::state::AppState;

use crate::support::{call_json, live_app_state, minted_member_with_email, sweep_user_rows};

const PREFIX: &str = "workbench-approval-race-live";

async fn sweep(pg: &PgPool, tag: &str) {
    sqlx::query("delete from agent_defs where department = $1")
        .bind(format!("{PREFIX}-{tag}"))
        .execute(pg)
        .await
        .unwrap();
    sweep_user_rows(pg, PREFIX, tag).await;
}

/// A heavy job waiting on its plan, on a ticket `owner` may decide.
async fn gated_job(state: &AppState, tag: &str, owner: &str) -> String {
    let pg = &state.pg;
    let board: String = sqlx::query_scalar(
        "insert into boards (name, owner_id) values ('race live', $1::uuid) returning id::text",
    )
    .bind(owner)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&board)
    .bind(owner)
    .execute(pg)
    .await
    .unwrap();
    let task: String = sqlx::query_scalar(
        "insert into tasks (board_id, title) values ($1::uuid, 'race') returning id::text",
    )
    .bind(&board)
    .fetch_one(pg)
    .await
    .unwrap();
    let model = format!("{PREFIX}-{tag}");
    let agent: String = sqlx::query_scalar(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, $1, $1, 'Race') returning id::text",
    )
    .bind(&model)
    .fetch_one(pg)
    .await
    .unwrap();
    sqlx::query_scalar(
        "insert into workbench_jobs (agent_id, agent_model, task_id, repo, branch, effort, status) \
         values ($1::uuid, $2, $3::uuid, 'acme/widgets', 'talaria/race', 'heavy', 'awaiting_approval') \
         returning id::text",
    )
    .bind(&agent)
    .bind(&model)
    .bind(&task)
    .fetch_one(pg)
    .await
    .unwrap()
}

async fn a_rewind_lands_while_the_verdict_is_in_flight(tag: &str, action: &str) {
    let state = live_app_state().await;
    let pg = state.pg.clone();
    sweep(&pg, tag).await;
    let email = format!("owner-{tag}@{PREFIX}.invalid");
    let (owner, cookie) =
        minted_member_with_email(&state, &format!("{PREFIX}:{tag}"), &email).await;
    let job = gated_job(&state, tag, &owner).await;

    let mut rewind = pg.begin().await.unwrap();
    sqlx::query("update workbench_jobs set status = 'superseded' where id = $1::uuid")
        .bind(&job)
        .execute(&mut *rewind)
        .await
        .unwrap();

    let verdict = tokio::spawn({
        let (state, job, action) = (state.clone(), job.clone(), action.to_string());
        async move {
            let body = json!({ "jobId": job, "action": action });
            call_json(
                &state,
                "PUT",
                "/api/workbench/jobs",
                Some(&cookie),
                Some(body),
            )
            .await
        }
    });
    let mut blocked = false;
    for _ in 0..200 {
        blocked = sqlx::query_scalar(
            "select exists (select 1 from pg_stat_activity \
             where wait_event_type = 'Lock' and query like '%update workbench_jobs set status%')",
        )
        .fetch_one(&pg)
        .await
        .unwrap();
        if blocked || verdict.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(blocked, "the route reached its write behind the rewind");
    rewind.commit().await.unwrap();

    let (status, body) = verdict.await.unwrap();
    let after: String = sqlx::query_scalar("select status from workbench_jobs where id = $1::uuid")
        .bind(&job)
        .fetch_one(&pg)
        .await
        .unwrap();
    assert_eq!(after, "superseded", "the verdict overwrote the rewind");
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    sweep(&pg, tag).await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn a_rejection_does_not_overwrite_a_rewind() {
    a_rewind_lands_while_the_verdict_is_in_flight("reject", "reject").await;
}

#[tokio::test]
#[ignore = "needs a live dev database and redis (source ui/.env)"]
async fn an_approval_does_not_overwrite_a_rewind() {
    a_rewind_lands_while_the_verdict_is_in_flight("approve", "approve").await;
}
