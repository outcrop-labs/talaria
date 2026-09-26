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
//   source ui/.env && cargo test --test it -- --ignored workbench_queue::

use sqlx::PgPool;
use talaria_workbench_queue::{promote_head_of_line, queue_position};
use uuid::Uuid;

/// THE QUEUE IS BOX-WIDE, SO THIS SUITE IS SINGLE-FILE.
///
/// `queue_position` ranks over EVERY queued job and `promote_head_of_line`
/// walks the same global line — that is the crate's law ("one FIFO for the
/// box"), not an accident of the SQL. Two tests seeding their own jobs and
/// then asking the box-wide questions therefore answer each other's rows:
/// CI runs a module's tests in one process (`cargo test --test it --
/// --ignored "workbench_queue::"`), and both of these failed there the first
/// time — one read position 3 where it seeded 2, the other watched the sweep
/// promote the sibling test's job. Isolating by agent would not help; scoping
/// the queries per agent would test a queue the product does not have.
///
/// So the suite owns the line one test at a time. Held across the whole body,
/// released on panic (a tokio mutex does not poison), and every test resets
/// the WHOLE suite's rows on both sides of it.
static BOX_QUEUE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Every test hangs off its own throwaway agent row; the FK cascade removes
/// its jobs along with it. Deletes the whole SUITE (the department marker),
/// not just this test's slug: a run that panicked mid-body left queued jobs
/// behind, and those rows are in the box-wide line the next run measures.
/// Run on both sides of every test.
async fn reset(pg: &PgPool) {
    sqlx::query("delete from agent_defs where department = 'wb-queue-test'")
        .execute(pg)
        .await
        .unwrap();
}

/// The box-wide line holds THIS test's jobs and nothing else. Without it a
/// foreign queued row turns every position assertion below into
/// `left: Some(3), right: Some(2)` — true, and no help at all.
async fn assert_queue_is_ours(pg: &PgPool, expected: i64) {
    let (n,): (i64,) = sqlx::query_as(
        "select count(*)::bigint from workbench_jobs w \
         join agent_defs a on a.id = w.agent_id \
         where w.status = 'queued' and a.department <> 'wb-queue-test'",
    )
    .fetch_one(pg)
    .await
    .unwrap();
    assert_eq!(
        n, 0,
        "this database holds {n} queued workbench job(s) outside the suite — \
         the queue is box-wide, so they would shadow every position below"
    );
    let (mine,): (i64,) =
        sqlx::query_as("select count(*)::bigint from workbench_jobs where status = 'queued'")
            .fetch_one(pg)
            .await
            .unwrap();
    assert_eq!(mine, expected, "the suite seeded {expected} queued job(s)");
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
    let _line = BOX_QUEUE.lock().await;
    let pg = pg().await;
    let slug = format!("wb-queue-{}", Uuid::new_v4());
    reset(&pg).await;
    let agent = agent_row(&pg, &slug).await;
    let older = queued_job(&pg, &agent, "o/older", 5).await;
    let newer = queued_job(&pg, &agent, "o/newer", 0).await;
    assert_queue_is_ours(&pg, 2).await;

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

    reset(&pg).await;
}

#[tokio::test]
#[ignore = "needs a live dev database (DATABASE_URL)"]
async fn a_refused_head_blocks_the_line_not_just_itself() {
    let _line = BOX_QUEUE.lock().await;
    let pg = pg().await;
    let slug = format!("wb-queue-{}", Uuid::new_v4());
    reset(&pg).await;
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
    assert_queue_is_ours(&pg, 2).await;

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

    reset(&pg).await;
}
