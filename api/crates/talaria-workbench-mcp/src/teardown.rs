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
// Three moves, all platform-side, all by docker exec into the agent's
// managed container (the agent never has to remember):
//   · Stop:    kill every process whose cwd is inside the workdir. A PR
//                opening ends the job's builds; the checkout stays, because a
//                revise bounce pushes to the same branch from it.
//   · Reclaim: drop what the repo says it can rebuild, keeping the checkout,
//                the branch and every uncommitted edit. For a job that is
//                alive and legitimate but holding a build tree nobody is
//                using this minute. Refuses while anything is running there.
//   · Remove:  Stop, then delete the workdir. The job is over: abandoned,
//                or its ticket closed.
//
// The sweep is the backstop for everything that never calls a verb, and it
// runs in two passes. The STATUS pass asks "is this job over?": jobs whose
// ticket reached a terminal column or was archived while `started` (abandoned
// here), pr_open jobs whose ticket closed, and abandoned jobs a failed inline
// teardown left behind. `workspace_cleared_at` is the done-marker, so a job is
// removed once and never scanned again.
//
// The SIZE pass asks the question the status pass cannot: "how big is this?"
// A job can be entirely legitimate and still hold 20 GiB, and ten of those is
// a full volume made of work nobody should cancel. It measures each live job,
// asks its repo what is rebuildable, and reclaims oldest-first — either
// because one job is over its ceiling, or because the volume is tight.

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

/// Reclaim — the rung between Stop and Remove.
///
/// Stop keeps the artifacts and kills the processes; Remove takes the whole
/// workdir. Neither helps the case that actually fills a disk: a job that is
/// alive and legitimate, holding a build tree nobody is using this minute.
/// Reclaim drops only what the project said it can rebuild and leaves the
/// checkout, the branch, `.git` and every uncommitted edit untouched, so a
/// reclaimed job that resumes pays a rebuild and nothing else.
///
/// It REFUSES while anything is running in the workdir — Stop's own
/// predicate, inverted — because a build whose `target/` disappears mid-link
/// fails in a way nobody can read. Positional args only ($1 = workdir,
/// $2.. = repo-relative paths), and each one is re-checked here even though
/// `valid_rel_path` already passed it: this is the last thing before
/// `rm -rf`, and it is running inside somebody else's container.
const RECLAIM_SH: &str = r#"
dir="$1"; shift
[ -d "$dir" ] || exit 0
for p in /proc/[0-9]*; do
  [ "${p#/proc/}" = "$$" ] && continue
  cwd=$(readlink "$p/cwd" 2>/dev/null) || continue
  case "$cwd/" in "$dir"/*) echo BUSY; exit 0 ;; esac
done
before=$(du -sb -- "$dir" 2>/dev/null | cut -f1)
for rel in "$@"; do
  case "$rel" in "") continue ;; /*) continue ;; *..*) continue ;; .git|.git/*|*/.git|*/.git/*) continue ;; esac
  [ -e "$dir/$rel" ] || continue
  rm -rf -- "$dir/$rel"
done
after=$(du -sb -- "$dir" 2>/dev/null | cut -f1)
echo "FREED $((before - after))"
"#;

/// A workdir bigger than this is worth reclaiming even when the volume has
/// room, because one job is not entitled to a disk. A repo's own
/// `maxWorkdirGib` overrides it either way.
pub const DEFAULT_MAX_WORKDIR_GIB: u64 = 25;

/// Reclaim oldest-idle-first once the workbench volume is this full. The
/// per-job ceiling is the routine pass; this is the backstop for the case it
/// cannot help with — several genuinely active large jobs at once.
pub const VOLUME_BUDGET_PCT: u64 = 80;

/// $1 = path. Bytes used, and the percentage full of the filesystem holding
/// it, as `BYTES <n>` and `PCT <n>`.
const MEASURE_SH: &str = r#"
p="$1"
[ -e "$p" ] || { echo "BYTES 0"; echo "PCT 0"; exit 0; }
echo "BYTES $(du -sb -- "$p" 2>/dev/null | cut -f1)"
echo "PCT $(df -P -- "$p" 2>/dev/null | awk 'NR==2 {gsub(/%/,"",$5); print $5}')"
"#;

fn field(out: &str, key: &str) -> Option<u64> {
    out.lines()
        .find_map(|l| l.strip_prefix(key)?.trim().parse::<u64>().ok())
}

/// Bytes in a path inside the agent's container, and how full its filesystem
/// is. `du` on a cold target is slow, so this is only asked of jobs the
/// sweep is already considering.
async fn measure(pg: &PgPool, department: &str, path: &str) -> Option<(u64, u64)> {
    let container = managed_container(pg, department).await;
    let (out, _) = docker_exec(
        &container,
        &["sh", "-c", MEASURE_SH, "sh", path],
        5 * 60_000,
    )
    .await
    .ok()?;
    Some((field(&out, "BYTES ")?, field(&out, "PCT ").unwrap_or(0)))
}

/// Is this job worth reclaiming? Pure, so the rule is pinned by tests.
///
/// `over_budget` is the volume backstop: once the disk is tight, any idle job
/// with something to give back is fair game, oldest first. Otherwise a job
/// has to be over its own ceiling — its repo's `maxWorkdirGib`, or the
/// platform default.
pub fn should_reclaim(
    bytes: u64,
    max_workdir_gib: Option<u64>,
    over_budget: bool,
    has_artifacts: bool,
) -> bool {
    if !has_artifacts {
        return false;
    }
    if over_budget {
        return true;
    }
    let ceiling = max_workdir_gib.unwrap_or(DEFAULT_MAX_WORKDIR_GIB);
    bytes >= ceiling.saturating_mul(1024 * 1024 * 1024)
}

/// A job status that could still be building. `pr_open` and
/// `awaiting_approval` cannot be, by definition — the agent is waiting on a
/// person. `started` might be, so the script checks for live processes and
/// refuses; this only decides who is worth asking.
fn reclaim_candidate(status: &str) -> bool {
    matches!(status, "started" | "awaiting_approval" | "pr_open")
}

/// Drop a job's rebuildable output. `Ok(None)` means the job was busy and
/// nothing was touched; `Ok(Some(bytes))` is what came back.
pub async fn reclaim_job(
    pg: &PgPool,
    department: &str,
    job_id: &str,
    paths: &[String],
) -> Result<Option<u64>, String> {
    let Some(dir) = job_workdir(job_id) else {
        return Err(format!("not a job id: {job_id:?}"));
    };
    // Validated once more on the way out. A path that fails here is a bug
    // upstream, not a thing to pass to a shell and hope.
    let safe: Vec<&str> = paths
        .iter()
        .map(String::as_str)
        .filter(|p| crate::devenv::valid_rel_path(p))
        .collect();
    if safe.is_empty() {
        return Ok(Some(0));
    }
    let container = managed_container(pg, department).await;
    let mut cmd: Vec<&str> = vec!["sh", "-c", RECLAIM_SH, "sh", &dir];
    cmd.extend(safe);
    let (stdout, _) = docker_exec(&container, &cmd, TEARDOWN_TIMEOUT_MS).await?;
    if stdout.contains("BUSY") {
        return Ok(None);
    }
    Ok(Some(
        stdout
            .split_whitespace()
            .skip_while(|t| *t != "FREED")
            .nth(1)
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0),
    ))
}

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
    let reclaimed = match reclaim_pass(pg).await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!("[workbench] reclaim pass failed: {e}");
            0
        }
    };
    if abandoned + removed + failed == 0 && reclaimed == 0 {
        return Ok(None);
    }
    Ok(Some(format!(
        "closed {abandoned} job(s) whose ticket closed, cleared {removed} workdir(s){}{}",
        if reclaimed > 0 {
            format!(", reclaimed {} from live jobs", human_bytes(reclaimed))
        } else {
            String::new()
        },
        if failed > 0 {
            format!(", {failed} left for the next sweep")
        } else {
            String::new()
        }
    )))
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "MiB", "GiB", "TiB"];
    if n < 1024 * 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64 / (1024.0 * 1024.0);
    let mut i = 1;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

/// The size half of the sweep: what the status pass cannot see.
///
/// A job that is alive and legitimate still holds whatever its toolchain
/// wrote, and nothing was measuring it. This asks each live job how big it
/// is, and drops what its repo says is rebuildable — oldest first, so the
/// job most likely to be resumed keeps its cache longest.
///
/// Everything here is best-effort and per job: a container that cannot be
/// reached, a repo with no policy, or a busy workdir is skipped, never
/// retried in a tight loop, and never allowed to fail the sweep.
async fn reclaim_pass(pg: &PgPool) -> Result<u64, String> {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "select j.id::text, j.status, d.department \
         from workbench_jobs j \
         join agent_defs d on d.id = j.agent_id \
         where j.workspace_cleared_at is null \
           and j.status in ('started', 'awaiting_approval', 'pr_open') \
         order by j.updated_at",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("reclaim scan: {e}"))?;

    let mut freed = 0u64;
    for (job_id, status, department) in rows {
        if !reclaim_candidate(&status) {
            continue;
        }
        let Some(dir) = job_workdir(&job_id) else {
            continue;
        };
        let Some((bytes, pct)) = measure(pg, &department, &dir).await else {
            continue;
        };
        if bytes == 0 {
            continue;
        }
        let over_budget = pct >= VOLUME_BUDGET_PCT;
        // Ask the repo what it is willing to lose. No checkout, no policy,
        // nothing to do — the platform does not guess on a repo it cannot read.
        let Some(files) = crate::devenv::scan_repo_files(pg, &department, &dir).await else {
            continue;
        };
        let plan = crate::devenv::plan_cleanup(&files);
        if !should_reclaim(
            bytes,
            plan.max_workdir_gib,
            over_budget,
            !plan.artifacts.is_empty(),
        ) {
            continue;
        }
        match reclaim_job(pg, &department, &job_id, &plan.artifacts).await {
            // None = a live build. Left alone; the next sweep asks again.
            Ok(None) => {}
            Ok(Some(n)) if n > 0 => {
                freed += n;
                tracing::info!(
                    "[workbench] reclaimed {} from job {job_id} ({})",
                    human_bytes(n),
                    plan.artifacts.join(", ")
                );
            }
            Ok(Some(_)) => {}
            Err(e) => tracing::warn!("[workbench] reclaim of job {job_id} failed: {e}"),
        }
    }
    Ok(freed)
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

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn a_job_over_its_ceiling_is_reclaimed_and_a_small_one_is_left() {
        // The platform default applies when the repo names no ceiling.
        assert!(should_reclaim(30 * GIB, None, false, true));
        assert!(!should_reclaim(2 * GIB, None, false, true));
        // The repo's own ceiling wins in both directions.
        assert!(should_reclaim(5 * GIB, Some(4), false, true));
        assert!(!should_reclaim(30 * GIB, Some(100), false, true));
    }

    #[test]
    fn a_repo_with_nothing_to_give_back_is_never_touched() {
        // Not even when the volume is tight: an opted-out repo has no
        // rebuildable paths, so there is nothing to take.
        assert!(!should_reclaim(500 * GIB, None, true, false));
        assert!(!should_reclaim(500 * GIB, Some(1), true, false));
    }

    #[test]
    fn a_tight_volume_reclaims_a_job_under_its_ceiling() {
        // The backstop: several legitimately active jobs, none individually
        // over its ceiling, together filling the disk.
        assert!(!should_reclaim(GIB, None, false, true));
        assert!(should_reclaim(GIB, None, true, true));
    }

    #[test]
    fn only_a_job_that_could_still_be_holding_a_workdir_is_asked() {
        assert!(reclaim_candidate("started"));
        assert!(reclaim_candidate("awaiting_approval"));
        assert!(reclaim_candidate("pr_open"));
        // Abandoned is the status pass's business — it removes the whole
        // workdir, so reclaiming part of it first would be wasted work.
        assert!(!reclaim_candidate("abandoned"));
        assert!(!reclaim_candidate("merged"));
    }

    #[test]
    fn measured_fields_are_read_off_the_script_output() {
        let out = "BYTES 1234\nPCT 91\n";
        assert_eq!(field(out, "BYTES "), Some(1234));
        assert_eq!(field(out, "PCT "), Some(91));
        assert_eq!(field("", "BYTES "), None);
        // A df that printed nothing must not read as 0% free-and-clear.
        assert_eq!(field("BYTES 5\nPCT \n", "PCT "), None);
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
