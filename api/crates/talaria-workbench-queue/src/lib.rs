// Workbench job queue — the durable FIFO behind the admission gate.
//
// The workbench MCP used to answer a full host with a failure: the agent
// retried, or gave up, and the ticket sat. A refused start is not a failed
// start — the work is wanted, the box is just full — so the row now lands
// with status='queued' (start_job writes it in place), and the sweep below
// flips it to 'started' the moment the admission door
// (`fleet::budget::admit_work`) says the host has room again.
//
// ONE line, box-wide: `workbench_jobs` carries the queue in place (status
// 'queued' + queued_at/queued_reason columns), ordered by queued_at with the
// job id as tiebreak — the same position semantics `work_wait` spells for
// the board's ticket-level view. No per-agent lanes: a job the host can
// admit is a job that runs, whichever desk filed it.
//
// Head-of-line, no skip-ahead: a full box stays full, and the sweep stops
// at the first refusal rather than letting a light job leapfrog a blocked
// heavy one. Simpler, and fairer — the queue order is the start order.

use std::sync::Arc;

use sqlx::PgPool;
use talaria_scheduler::{JobName, JobSpec};

const LOG: &str = "[wbqueue]";

/// One queued job as the sweep walks it: what to flip, the effort whose
/// reserve the admission door checks, and the ticket (if any) whose
/// board-level wait clears when this job actually starts.
struct QueuedJob {
    id: String,
    agent_id: String,
    effort: String,
    task_id: Option<String>,
}

/// The two rules `start_job` checks before it writes a row, asked AGAIN at
/// promotion: the per-agent live cap and one-live-job-per-ticket. A job
/// queues after passing both, but the queue is a wait of minutes to hours,
/// and in that time the agent's other jobs started and a human approved a
/// heavy plan on the same ticket. Promotion is the moment the job becomes a
/// clone and a harness in the container, so it is the moment the rules
/// have to be true — not the moment it was filed.
///
/// `Hold` is not `break`: the host has room (the admission door said so),
/// this one job is what cannot start, and the next in line may well be
/// another agent's. It stays queued, keeps its place, and is asked again
/// next pass.
#[derive(Debug, PartialEq, Eq)]
enum Promotion {
    Go,
    Hold(String),
}

/// Pure over the two counts, so the rule is pinned without a database.
fn promotion_of(live_for_agent: usize, live_for_ticket: usize) -> Promotion {
    if live_for_agent >= talaria_fleet_budget::MAX_LIVE_JOBS_PER_AGENT {
        return Promotion::Hold(format!(
            "the agent already has {live_for_agent} live job(s), the cap is {}",
            talaria_fleet_budget::MAX_LIVE_JOBS_PER_AGENT
        ));
    }
    if live_for_ticket > 0 {
        return Promotion::Hold("another job for the same ticket is already started".into());
    }
    Promotion::Go
}

async fn promotion_for(pg: &PgPool, job: &QueuedJob) -> Result<Promotion, sqlx::Error> {
    let live_for_agent: i64 = sqlx::query_scalar(
        "select count(*) from workbench_jobs \
         where agent_id = $1::uuid and status = 'started'",
    )
    .bind(&job.agent_id)
    .fetch_one(pg)
    .await?;
    let live_for_ticket: i64 = match &job.task_id {
        Some(task_id) => {
            sqlx::query_scalar(
                "select count(*) from workbench_jobs \
                 where task_id = $1::uuid and status = 'started' and id <> $2::uuid",
            )
            .bind(task_id)
            .bind(&job.id)
            .fetch_one(pg)
            .await?
        }
        None => 0,
    };
    Ok(promotion_of(
        live_for_agent.max(0) as usize,
        live_for_ticket.max(0) as usize,
    ))
}

/// Park a job in the queue. `queued_at` keeps the FIRST time this job
/// queued (re-queueing after a rollback does not buy a new place in line),
/// mirroring `work_wait`'s upsert law.
pub async fn enqueue(pg: &PgPool, job_id: &str, reason: &str) {
    let _ = sqlx::query(
        "update workbench_jobs \
         set status = 'queued', \
             queued_at = coalesce(queued_at, now()), \
             queued_reason = $2, \
             updated_at = now() \
         where id = $1::uuid",
    )
    .bind(job_id)
    .bind(reason)
    .execute(pg)
    .await;
}

/// This job's place in the queue — 1 is the head. Mirrors `work_wait`'s
/// position semantics (rank by queued_at, id tiebreak), but across ALL
/// queued jobs, not per agent: one FIFO for the box.
pub async fn queue_position(pg: &PgPool, job_id: &str) -> Option<i64> {
    sqlx::query_scalar(
        "select rank from ( \
             select id, rank() over (order by queued_at, id) as rank \
             from workbench_jobs where status = 'queued' \
         ) q where q.id = $1::uuid",
    )
    .bind(job_id)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
}

/// Walk the queue oldest-first and promote while the host has room.
///
/// Head-of-line only: the first job the admission door refuses stops the
/// pass (a full box stays full; a light job does not skip a blocked heavy
/// one). `guard` caps the flips per pass — the sweep promotes at most what
/// it found when it started, so a queue growing mid-pass cannot loop it
/// past its `max_run_ms`.
///
/// Returns the promoted job ids, oldest first. Best-effort by nature (a
/// scheduler job calls it): every failure is logged, never thrown.
///
/// A promoted job is a started job: any ticket-level `work_wait` row the
/// refusal left behind is cleared here, the same clearing `work-dispatch`
/// does when a session finally starts — the board must not show a wait
/// for a job that is now running.
pub async fn promote_head_of_line(pg: &PgPool, guard: usize) -> Vec<String> {
    let queued = match read_queue(pg).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!("{LOG} queue read failed, not promoting: {e}");
            return Vec::new();
        }
    };
    let mut promoted = Vec::new();
    for job in queued.into_iter().take(guard) {
        // The admission door, and ONLY the door: `effort_reserve` is the
        // RAM the job's harness will need — the same figure start_job
        // packed before queueing. CPU/disk ride the same door.
        if let Err(reason) =
            talaria_fleet_budget::admit_work(talaria_fleet_budget::effort_reserve(&job.effort))
                .await
        {
            tracing::debug!("{LOG} head of line {} still blocked: {reason}", job.id);
            break;
        }
        // The host has room; does THIS job still pass the rules it was
        // filed under? A hold skips it and keeps walking — the next job is
        // likely another agent's, and the box is not full.
        match promotion_for(pg, &job).await {
            Ok(Promotion::Go) => {}
            Ok(Promotion::Hold(why)) => {
                tracing::debug!("{LOG} {} held in the queue: {why}", job.id);
                continue;
            }
            Err(e) => {
                tracing::error!("{LOG} rule check for {} failed, not promoting: {e}", job.id);
                break;
            }
        }
        let flipped = sqlx::query(
            "update workbench_jobs \
             set status = 'started', queued_reason = null, updated_at = now() \
             where id = $1::uuid and status = 'queued'",
        )
        .bind(&job.id)
        .execute(pg)
        .await;
        if let Err(e) = flipped {
            // A failed flip is not a promotion: say so and stop, so the
            // next pass (and the agent's job_status) sees the truth.
            tracing::error!("{LOG} promote {} failed: {e}", job.id);
            break;
        }
        if let Some(task_id) = &job.task_id {
            talaria_work_wait::clear_waiting(pg, task_id).await;
        }
        promoted.push(job.id);
    }
    if !promoted.is_empty() {
        tracing::info!("{LOG} promoted {} queued job(s)", promoted.len());
    }
    promoted
}

async fn read_queue(pg: &PgPool) -> Result<Vec<QueuedJob>, sqlx::Error> {
    Ok(
        sqlx::query_as::<_, (String, String, String, Option<String>)>(
            "select id::text, agent_id::text, effort, task_id::text from workbench_jobs \
         where status = 'queued' \
         order by queued_at, id",
        )
        .fetch_all(pg)
        .await?
        .into_iter()
        .map(|(id, agent_id, effort, task_id)| QueuedJob {
            id,
            agent_id,
            effort,
            task_id,
        })
        .collect(),
    )
}

/// The pure position math, split for tests: given the queue's (queued_at,
/// rank) pairs as the window query orders them, this job's queued_at picks
/// its rank. `None` = not queued. A rank of 0 from the SQL is clamped to
/// the head — position is 1-based, the same floor `work_wait`'s
/// `position_of` gives.
pub fn position_of(rank_rows: &[(i64, i64)], queued_at: i64) -> Option<i64> {
    rank_rows
        .iter()
        .find(|(at, _)| *at == queued_at)
        .map(|(_, rank)| (*rank).max(1))
}

/// The phase line an agent reads in job_status: the same sentence
/// `work_wait::phase_of` spells for the board, so ticket-level and
/// job-level surfaces name the wait identically.
pub fn phase_of(position: i64) -> String {
    talaria_work_wait::phase_of(position)
}

pub fn promotion_job_spec(pg: PgPool) -> JobSpec {
    JobSpec {
        name: JobName::WorkbenchQueueSweep,
        every_ms: 60_000,
        first_run_delay_ms: Some(90_000),
        // NOT `per_instance`: the queue is a table every instance shares —
        // the lease picks one runner per minute, exactly like
        // work-redispatch. Two instances promoting the same head would
        // double-flip rows the lease exists to prevent.
        per_instance: false,
        max_run_ms: Some(30_000),
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move {
                // The guard is the queue's own depth read up front: one
                // pass promotes at most what it found, so a queue growing
                // mid-pass cannot loop the sweep past max_run_ms.
                let depth: i64 = sqlx::query_scalar(
                    "select count(*) from workbench_jobs where status = 'queued'",
                )
                .fetch_one(&pg)
                .await
                .unwrap_or(0);
                let promoted = promote_head_of_line(&pg, depth.max(0) as usize).await;
                Ok(if promoted.is_empty() {
                    None
                } else {
                    Some(format!("promoted {} queued job(s)", promoted.len()))
                })
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{Promotion, phase_of, position_of, promotion_of};

    #[test]
    fn promotion_re_asks_the_cap_and_the_one_job_per_ticket_rule() {
        let cap = talaria_fleet_budget::MAX_LIVE_JOBS_PER_AGENT;
        assert_eq!(promotion_of(0, 0), Promotion::Go);
        assert_eq!(promotion_of(cap - 1, 0), Promotion::Go);
        // At the cap: held, and the hold names the cap.
        match promotion_of(cap, 0) {
            Promotion::Hold(why) => assert!(why.contains(&cap.to_string()), "{why}"),
            Promotion::Go => panic!("at the cap must hold"),
        }
        // Another started job on the same ticket: held, whatever the cap.
        assert!(matches!(promotion_of(0, 1), Promotion::Hold(_)));
    }

    #[test]
    fn position_is_the_rank_at_the_queue_time() {
        // (queued_at ms, rank) pairs as the window query orders them.
        let rows: &[(i64, i64)] = &[(100, 1), (200, 2), (200, 3), (300, 4)];
        assert_eq!(position_of(rows, 100), Some(1));
        assert_eq!(position_of(rows, 200), Some(2));
        assert_eq!(position_of(rows, 300), Some(4));
        // A rank of 0 from the SQL is clamped to the head — 1-based, the
        // same floor work_wait's position_of gives.
        assert_eq!(position_of(&[(50, 0)], 50), Some(1));
        // Not queued: no position.
        assert_eq!(position_of(rows, 999), None);
    }

    #[test]
    fn the_phase_names_the_wait_the_way_the_board_does() {
        assert_eq!(phase_of(1), "next up — waiting for RAM");
        assert_eq!(phase_of(4), "queued · 3 ahead");
    }
}
