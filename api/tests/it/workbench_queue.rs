use crate::support::pg;
// Live-DB proof of the workbench queue's FIFO (cargo test -- --ignored).
// promote_head_of_line walks queued jobs oldest-first and flips at most
// `guard` per pass, stopping at the first admission refusal — the
// head-of-line law: a full box stays full, nothing skips ahead. These
// tests execute that law against the real table (the rank window, the
// status flip, the reason clear) so the SQL shapes can never drift from
// the crate's promises.
//
// House rule: #[ignore]d, never CI.
//
//   source ui/.env && cargo test --test workbench_queue -- --ignored

use sqlx::PgPool;
use talaria_workbench_queue::{promote_head_of_line, queue_position};
use uuid::Uuid;

/// The whole suite hangs off one throwaway agent row; the FK cascade
/// removes its jobs along with it. Run at the START so a failed previous
/// run's leftovers cannot shadow the next one's assertions.
async fn reset(pg: &PgPool, slug: &str) {
    sqlx::query("delete from agent_defs where slug = $1")
        .bind(slug)
        .execute(pg)
        .await
        .unwrap();
}

async fn agent_row(pg: &PgPool, slug: &str) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into agent_defs (slug, department, model, display_name) \
         values ($1, 'wb-queue-test', $1, 'WB Queue Test') returning id::text",
    )
    .bind(slug)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

/// A queued job row, `minutes_ago` back in the queue so FIFO order is
/// explicit rather than a clock race between two inserts.
async fn queued_job(pg: &PgPool, agent_id: &str, repo: &str, minutes_ago: i64) -> String {
    let (id,): (String,) = sqlx::query_as(
        "insert into workbench_jobs \
           (agent_id, agent_model, task_id, repo, branch, effort, plan, status, \
            queued_at, queued_reason) \
         values ($1::uuid, 'wb-queue-test', null, $2, 'test/branch', 'standard', '', \
                 'queued', now() - make_interval(mins => $3::int), 'host has 1.0 GiB RAM free') \
         returning id::text",
    )
    .bind(agent_id)
    .bind(repo)
    .bind(minutes_ago)
    .fetch_one(pg)
    .await
    .unwrap();
    id
}

async fn status_of(pg: &PgPool, job_id: &str) -> (String, Option<String>) {
    sqlx::query_as("select status, queued_reason from workbench_jobs where id = $1::uuid")
        .bind(job_id)
        .fetch_one(pg)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn one_pass_promotes_only_the_head_and_clears_its_reason() {
    let pg = pg().await;
    let slug = format!("wb-queue-{}", Uuid::new_v4());
    reset(&pg, &slug).await;
    let agent = agent_row(&pg, &slug).await;
    let older = queued_job(&pg, &agent, "o/older", 5).await;
    let newer = queued_job(&pg, &agent, "o/newer", 0).await;

    // The rank window is the position contract: oldest first, id tiebreak.
    assert_eq!(queue_position(&pg, &older).await, Some(1));
    assert_eq!(queue_position(&pg, &newer).await, Some(2));

    // guard 1: at most ONE flip per pass, and only the head.
    let promoted = promote_head_of_line(&pg, 1).await;
    let (older_status, older_reason) = status_of(&pg, &older).await;
    let (newer_status, _) = status_of(&pg, &newer).await;
    match promoted.first() {
        // The door passed: exactly the older job flipped, its reason
        // cleared, and the newer one is now the head of the line.
        Some(id) if id == &older => {
            assert_eq!(
                promoted.len(),
                1,
                "guard 1 promotes at most one: {promoted:?}"
            );
            assert_eq!(older_status, "started");
            assert_eq!(older_reason, None, "promotion clears the wait reason");
            assert_eq!(newer_status, "queued");
            assert_eq!(queue_position(&pg, &newer).await, Some(1));
            assert_eq!(
                queue_position(&pg, &older).await,
                None,
                "a started job is not in the queue"
            );
        }
        // The door refused: head-of-line means NOTHING flips — the newer
        // job must not skip ahead past a blocked head.
        None => {
            assert_eq!(older_status, "queued");
            assert_eq!(newer_status, "queued");
        }
        Some(other) => panic!("the head of line is the oldest job, promoted {other:?}"),
    }

    reset(&pg, &slug).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_refused_head_blocks_the_line_not_just_itself() {
    let pg = pg().await;
    let slug = format!("wb-queue-{}", Uuid::new_v4());
    reset(&pg, &slug).await;
    let agent = agent_row(&pg, &slug).await;
    // Heavy head, light second: even if the host could take the light
    // job, the sweep must not skip the blocked heavy head.
    let older = queued_job(&pg, &agent, "o/heavy-head", 5).await;
    sqlx::query("update workbench_jobs set effort = 'heavy' where id = $1::uuid")
        .bind(&older)
        .execute(&pg)
        .await
        .unwrap();
    let newer = queued_job(&pg, &agent, "o/light-second", 0).await;

    let promoted = promote_head_of_line(&pg, 10).await;
    match promoted.first() {
        // Door open for both: FIFO order — the older job is promoted
        // FIRST even under a generous guard.
        Some(id) if id == &older => {
            assert!(
                promoted.contains(&newer),
                "with room for both, both promote: {promoted:?}"
            );
        }
        // Door shut on the heavy head: nothing flips, the light job
        // waits behind it. This is the no-skip-ahead law itself.
        None => {
            let (older_status, _) = status_of(&pg, &older).await;
            let (newer_status, _) = status_of(&pg, &newer).await;
            assert_eq!(older_status, "queued");
            assert_eq!(newer_status, "queued");
        }
        Some(other) => panic!("FIFO says the oldest promotes first, promoted {other:?}"),
    }

    reset(&pg, &slug).await;
}
