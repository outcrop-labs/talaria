// Job teardown: the half of the lifecycle start_job never had.
//
// start_job hands the agent a workdir (/opt/data/workbench/jobs/<id>) inside
// its long-lived container, and the agent clones, builds, and tests there.
// Nothing took it back: finish_job opened the PR, abandon flipped a status,
// and a ticket closing under a live job did neither. Each job left a full
// checkout plus a cold cargo `target/` (4–20 GiB), and whatever the agent
// was still running in it. A `cargo test --ignored` nobody would read kept
// compiling for hours. On the 2026-09-24 dogfood box one agent held 30
// workdirs (78 GiB) and seven concurrent cargo trees, which is what pinned
// the VM's disk and the hypervisor behind it.
//
// Two moves, both platform-side, both by docker exec into the agent's
// managed container (the agent never has to remember):
//   · Stop:   kill every process whose cwd is inside the workdir. A PR
//                opening ends the job's builds; the checkout stays, because a
//                revise bounce pushes to the same branch from it.
//   · Remove: Stop, then delete the workdir. The job is over: abandoned,
//                or its ticket closed.
//
// The sweep is the backstop for everything that never calls a verb: jobs
// whose ticket reached a terminal column or was archived while `started`
// (abandoned here), pr_open jobs whose ticket closed, and abandoned jobs a
// failed inline teardown left behind. `workspace_cleared_at` is the
// done-marker, so a job is removed once and never scanned again.

use sqlx::PgPool;
use std::sync::Arc;

use talaria_fleet_docker::{docker_exec, managed_container};
use talaria_statuses::status_meta;

pub const JOBS_ROOT: &str = "/opt/data/workbench/jobs";

/// A workdir is only ever the jobs root plus a job uuid. Anything else is
/// refused before a shell sees it, because this path goes to `rm -rf`.
pub fn job_workdir(job_id: &str) -> Option<String> {
    uuid::Uuid::parse_str(job_id)
        .ok()
        .map(|id| format!("{JOBS_ROOT}/{id}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Teardown {
    Stop,
    Remove,
}

impl Teardown {
    fn as_str(self) -> &'static str {
        match self {
            Teardown::Stop => "stop",
            Teardown::Remove => "remove",
        }
    }
}

/// Positional args only ($1 = workdir, $2 = mode): nothing is interpolated
/// into the script. TERM first so a harness can flush its session, KILL for
/// what ignored it. Matching on cwd catches the whole tree the agent started
/// there (the shell, cargo, every rustc and test binary) and nothing the
/// container runs elsewhere (the Hermes gateway lives outside the jobs root).
const TEARDOWN_SH: &str = r#"
dir="$1"; mode="$2"
in_dir() {
  cwd=$(readlink "$1/cwd" 2>/dev/null) || return 1
  case "$cwd/" in "$dir"/*) return 0 ;; esac
  return 1
}
signal_all() {
  for p in /proc/[0-9]*; do
    [ "${p#/proc/}" = "$$" ] && continue
    in_dir "$p" && kill "-$1" "${p#/proc/}" 2>/dev/null
  done
}
signal_all TERM
sleep 3
signal_all KILL
if [ "$mode" = remove ]; then rm -rf -- "$dir"; fi
"#;

/// Deleting a cold `target/` takes minutes on a busy disk; the stop half is
/// seconds. Generous on purpose: a timed-out removal just retries next sweep.
const TEARDOWN_TIMEOUT_MS: u64 = 15 * 60_000;

pub async fn teardown_job(
    pg: &PgPool,
    department: &str,
    job_id: &str,
    mode: Teardown,
) -> Result<(), String> {
    let Some(dir) = job_workdir(job_id) else {
        return Err(format!("not a job id: {job_id:?}"));
    };
    let container = managed_container(pg, department).await;
    docker_exec(
        &container,
        &["sh", "-c", TEARDOWN_SH, "sh", &dir, mode.as_str()],
        TEARDOWN_TIMEOUT_MS,
    )
    .await?;
    if mode == Teardown::Remove {
        sqlx::query("update workbench_jobs set workspace_cleared_at = now() where id = $1::uuid")
            .bind(job_id)
            .execute(pg)
            .await
            .map_err(|e| format!("job update: {e}"))?;
    }
    Ok(())
}

/// Fire-and-forget from a verb: the agent's tool call returns now, the
/// teardown finishes behind it. A failure is logged and left for the sweep.
pub fn spawn_teardown(pg: PgPool, department: String, job_id: String, mode: Teardown) {
    tokio::spawn(async move {
        if let Err(e) = teardown_job(&pg, &department, &job_id, mode).await {
            tracing::warn!(
                "[workbench] teardown {} of job {job_id} failed: {e}",
                mode.as_str()
            );
        }
    });
}

/// What the sweep does with one job. Pure, so the rules are pinned without
/// a database or a container.
#[derive(Debug, PartialEq, Eq)]
pub enum SweepAction {
    Leave,
    /// Close the job out (status → abandoned), then remove its workdir.
    AbandonAndRemove,
    Remove,
}

pub fn sweep_action(job_status: &str, ticket_closed: Option<bool>) -> SweepAction {
    match (job_status, ticket_closed) {
        ("abandoned", _) => SweepAction::Remove,
        ("started" | "awaiting_approval", Some(true)) => SweepAction::AbandonAndRemove,
        ("pr_open", Some(true)) => SweepAction::Remove,
        // No ticket (ad-hoc job, or the ticket was deleted and the FK nulled
        // it) says nothing about whether the work is over, so leave it.
        _ => SweepAction::Leave,
    }
}

type SweepRow = (
    String,         // job id
    String,         // job status
    String,         // agent id
    String,         // department
    Option<String>, // board id (null = no ticket)
    Option<String>, // ticket status
    bool,           // ticket archived
);

pub async fn sweep_job_workspaces(pg: &PgPool) -> Result<Option<String>, String> {
    let rows: Vec<SweepRow> = sqlx::query_as(
        "select j.id::text, j.status, j.agent_id::text, d.department, \
                t.board_id::text, t.status, t.archived_at is not null \
         from workbench_jobs j \
         join agent_defs d on d.id = j.agent_id \
         left join tasks t on t.id = j.task_id \
         where j.workspace_cleared_at is null \
           and j.status in ('started', 'awaiting_approval', 'pr_open', 'abandoned') \
         order by j.updated_at",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("job scan: {e}"))?;

    let (mut abandoned, mut removed, mut failed) = (0, 0, 0);
    let mut budgets: Vec<(String, String)> = Vec::new();
    for (job_id, status, agent_id, department, board_id, ticket_status, archived) in rows {
        let closed = match (&board_id, &ticket_status) {
            (Some(board), Some(ticket_status)) => Some(
                archived
                    || status_meta(pg, board)
                        .await
                        .map(|meta| meta.terminal(ticket_status))
                        .unwrap_or(false),
            ),
            _ => None,
        };
        let action = sweep_action(&status, closed);
        if action == SweepAction::Leave {
            continue;
        }
        if action == SweepAction::AbandonAndRemove {
            // Guarded on the status read above so a finish_job racing the
            // sweep wins: its pr_open is not overwritten.
            let hit = sqlx::query(
                "update workbench_jobs set status = 'abandoned', updated_at = now() \
                 where id = $1::uuid and status = $2",
            )
            .bind(&job_id)
            .bind(&status)
            .execute(pg)
            .await
            .map_err(|e| format!("job update: {e}"))?;
            if hit.rows_affected() == 0 {
                continue;
            }
            abandoned += 1;
            if !budgets.contains(&(department.clone(), agent_id.clone())) {
                budgets.push((department.clone(), agent_id.clone()));
            }
        }
        match teardown_job(pg, &department, &job_id, Teardown::Remove).await {
            Ok(()) => removed += 1,
            Err(e) => {
                failed += 1;
                tracing::warn!("[workbench] sweep could not clear job {job_id}: {e}");
            }
        }
    }
    // Abandoned jobs stop counting against the container's reservation.
    for (department, agent_id) in &budgets {
        crate::sync_agent_budget(pg, department, agent_id).await;
    }
    if abandoned + removed + failed == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "closed {abandoned} job(s) whose ticket closed, cleared {removed} workdir(s){}",
        if failed > 0 {
            format!(", {failed} left for the next sweep")
        } else {
            String::new()
        }
    )))
}

/// Ten minutes: a closed ticket's builds stop within one interval, and a
/// removal that outlives it is skipped (not doubled) by the overlap guard.
const SWEEP_EVERY_MS: u64 = 10 * 60_000;

pub fn job_sweep_spec(pg: PgPool) -> talaria_scheduler::JobSpec {
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::WorkbenchJobSweep,
        every_ms: SWEEP_EVERY_MS,
        first_run_delay_ms: Some(5 * 60_000),
        // Leased, not per-instance: the input is workbench_jobs, which every
        // instance reaches. The docker exec needs this host's docker, which
        // the fleet already assumes of the api (fleet-docker is how every
        // agent verb reaches its container).
        per_instance: false,
        max_run_ms: Some(30 * 60_000),
        run: Arc::new(move || {
            let pg = pg.clone();
            Box::pin(async move { sweep_job_workspaces(&pg).await })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workdir_is_only_ever_a_uuid_under_the_jobs_root() {
        assert_eq!(
            job_workdir("11111111-1111-1111-1111-111111111111").as_deref(),
            Some("/opt/data/workbench/jobs/11111111-1111-1111-1111-111111111111")
        );
        assert_eq!(job_workdir(""), None);
        assert_eq!(job_workdir("../../etc"), None);
        assert_eq!(job_workdir("x; rm -rf /"), None);
    }

    #[test]
    fn abandoned_jobs_are_always_cleared() {
        assert_eq!(sweep_action("abandoned", None), SweepAction::Remove);
        assert_eq!(sweep_action("abandoned", Some(false)), SweepAction::Remove);
    }

    #[test]
    fn a_live_job_on_a_closed_ticket_is_closed_out() {
        assert_eq!(
            sweep_action("started", Some(true)),
            SweepAction::AbandonAndRemove
        );
        assert_eq!(
            sweep_action("awaiting_approval", Some(true)),
            SweepAction::AbandonAndRemove
        );
    }

    #[test]
    fn a_pr_keeps_its_checkout_until_the_ticket_closes() {
        assert_eq!(sweep_action("pr_open", Some(false)), SweepAction::Leave);
        assert_eq!(sweep_action("pr_open", Some(true)), SweepAction::Remove);
    }

    #[test]
    fn open_tickets_and_ticketless_jobs_are_left_alone() {
        assert_eq!(sweep_action("started", Some(false)), SweepAction::Leave);
        assert_eq!(sweep_action("started", None), SweepAction::Leave);
        assert_eq!(sweep_action("pr_open", None), SweepAction::Leave);
    }
}
