// The boot assembly — the one place the job bodies become a running schedule,
// and the one place the six armed run steps get their deps.
//
// Every job module owns its own `register_*_job(deps)`: the deps are runtime
// values (the pool, the realtime fan-out, the secretbox), so registration
// cannot live at module load — SOMETHING has to build the real edges at boot
// and hand them over. This module is that something, and its one rule is
// COMPLETENESS: every job is declared here in one visible list, because a
// register call that falls out of this function is work that silently never
// happens. `start_scheduler`'s boot check enforces the required nine against
// this list; the test below pins the whole table.
//
// THE OTHER HALF of the assembly is the run kinds. The defs self-register on
// first getter call, so `try_arm` touches each getter and then REFUSES to arm
// unless the census's six kinds are all present. The refusal is a boot list
// made executable: `run-reclaim` is one of the ten jobs, and a sweep that
// cannot define a kind leaves those rows to an instance that can — no such
// instance exists, and the rows would sit forever with only a warn line.
// A kind reappearing in it is a def module that fell out of the boot list
// below.
//
// update-check rides the same table now (api/src/update/job.rs): the
// restart-topology HOLD it sat under is retired by the update engine — a
// roll replaces the whole CONTAINER (slots behind an edge), so the binary
// that choreographs a roll is under no obligation to survive it. The job
// stays out of REQUIRED_JOBS: on the dormant installs (checkout, dev,
// off) it is an honest no-op, and a missing no-op is not work that
// silently never happened.

use std::sync::Arc;
use std::time::Duration;

use talaria_comms_decay::real_decay_deps;
use talaria_daily_brief::real_brief_deps;
use talaria_digest::real_digest_deps;
use talaria_model_info::BlurbDeps;
use talaria_notify::real_drain_deps;
use talaria_outreach::OutreachDeps;
use talaria_price_oracle::PriceRefreshDeps;
use talaria_realtime::RealtimeDeps;
use talaria_runs_decide::assembly::real_run_deps;
use talaria_runs_define::run_definition;
use talaria_runs_reclaim::{ReclaimDeps, drive_fn, due_fn};
use talaria_runs_run::RunDeps;
use talaria_scheduler as scheduler;
use talaria_scheduler::REQUIRED_JOBS;
use talaria_secretbox::SecretBox;
use talaria_state::AppState;

const LOG: &str = "[jobs]";

/// The run kinds the boot census carries, spelled as strings because that is
/// what the registry keys on. Arming with fewer means the sweep strands rows
/// of the missing kinds (see the header).
const BOOT_RUN_KINDS: &[&str] = &[
    "agent-hire",
    "plan-draft",
    "rag-backfill",
    "rag-reindex",
    "research",
    "work-session",
];

/// Declare the whole job table, one register call per job. The `run` and
/// `rt` edges are parameters (not built here) only so
/// the completeness test can inject fakes: registration only CAPTURES deps,
/// never invokes them, and `try_arm` builds the real ones. Every other deps
/// constructor in this list is pure closure-building over the lazy pool.
pub async fn register_all(state: &AppState, run: Arc<RunDeps>, rt: RealtimeDeps, sb: &SecretBox) {
    talaria_comms_decay::register_comms_decay_job(Arc::new(real_decay_deps(state)));
    talaria_outreach::register_outreach_job(Arc::new(OutreachDeps {
        state: state.clone(),
    }));
    let _ =
        talaria_web_search::CALL_MCP_TOOL.set(std::sync::Arc::new(|pg, sb, server, tool, args| {
            Box::pin(async move {
                talaria_mcp::registry::call_mcp_tool(&pg, &sb, &server, &tool, &args)
                    .await
                    .map(|out| (out.structured, out.text))
            })
        }));
    let _ = talaria_fleet_create::RENDER_FLEET.set(std::sync::Arc::new(|pg, sb| {
        Box::pin(async move {
            talaria_fleet_render::render_fleet(&pg, &sb, None)
                .await
                .map(|_| ())
        })
    }));
    let _ = talaria_mcp_apply::ROLL_AGENT.set(std::sync::Arc::new(|pg, sb, dept| {
        Box::pin(async move { talaria_fleet_reconcile::roll_agent(&pg, &sb, &dept).await })
    }));
    // THE ROLL'S OWN TWO EDGES. `roll_agent` reaches the renderer through
    // them — a roll renders the incoming slot, brings it up, flips the
    // manifest, then re-renders — and an edge nothing sets is a roll that
    // returns "… not wired" into callers that discard it: the roster never
    // changes, the logs stay silent, and the agent keeps running the config
    // the operator just replaced. Same completeness rule as the job table
    // above, one layer down.
    let _ = talaria_fleet_reconcile::RENDER_FLEET.set(std::sync::Arc::new(|pg, sb, roll| {
        Box::pin(async move {
            let overlay =
                roll.as_ref()
                    .map(|(slug, slot, port)| talaria_fleet_render::RollOverlay {
                        slug,
                        slot: *slot,
                        port: *port,
                    });
            talaria_fleet_render::render_fleet(&pg, &sb, overlay)
                .await
                .map(|render| (render.agents.len(), render.warnings))
        })
    }));
    let _ = talaria_fleet_reconcile::NEXT_FREE_PORT.set(std::sync::Arc::new(|pg| {
        Box::pin(async move {
            talaria_fleet_render::next_free_port(&pg)
                .await
                .map_err(|e| e.to_string())
        })
    }));
    let _ = talaria_fleet_docker::PREFLIGHT.set(|pool| {
        tokio::spawn(async move {
            let _ = talaria_fleet_preflight::run_fleet_preflight(&pool).await;
        });
    });
    let _ = talaria_gateway::usage::NUDGE_AUTO_PRICES.set(talaria_price_oracle::nudge_auto_prices);
    // THE ATTRIBUTION LADDER'S CHATTER RUNG. The seam (CONVERSATION_OWNER) and
    // the resolver it wants (conversations::conversation_owner — whose doc
    // comment names this ladder) both existed; this wiring did not, so the
    // live-turn rung read an unset OnceLock and silently fell to the hirer on
    // every install. Nothing noticed for the same reason nothing ever does: an
    // unset optional rung answers SOMEONE, just the wrong one. The live suite's
    // first CI run (api-integration.yml, attribution's
    // a_live_turn_outranks_the_hirer) is what caught it.
    let _ = talaria_attribution::CONVERSATION_OWNER.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_conversations::conversation_owner(&pg, &id).await })
    }));
    // THE TWO GET_TASK EDGES — same disease as CONVERSATION_OWNER, found the
    // same week by the live suite's first runs: workchains' turn/pause paths
    // and inbox-focus's focus scoring both `.expect("GET_TASK")` on a seam
    // nothing ever set, so those paths PANICKED in production (caught by
    // catch-panic as opaque 500s) instead of doing their work.
    let _ = talaria_workchains::GET_TASK.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_tasks::get_task(&pg, &id).await })
    }));
    let _ = talaria_inbox_focus::GET_TASK.set(std::sync::Arc::new(|pg, id| {
        Box::pin(async move { talaria_tasks::get_task(&pg, &id).await })
    }));
    talaria_price_oracle::register_price_refresh_job(Arc::new(PriceRefreshDeps {
        pg: state.pg.clone(),
    }));
    talaria_digest::register_digest_job(real_digest_deps(state, rt.clone()));
    talaria_digest::register_approval_escalation_job(real_digest_deps(state, rt.clone()));
    talaria_notify::register_notification_mail_job(real_drain_deps(state.pg.clone(), sb.clone()));
    talaria_work_dispatch::register_redispatch_job(state.pg.clone(), run.clone());
    talaria_runs_reclaim::register_reclaim_job(Arc::new(ReclaimDeps {
        due: due_fn(run.store.clone()),
        definition_for: run.definition_for.clone(),
        drive: drive_fn(run.clone()),
        now: run.now.clone(),
    }));
    talaria_daily_brief::register_daily_brief_job(real_brief_deps(state).await);
    talaria_model_info::register_blurb_rewrite_job(Arc::new(BlurbDeps {
        state: state.clone(),
    }));
    talaria_scheduler::register_job(talaria_fleet_resources::resource_job_spec(state.pg.clone()));
    // The optional trio. mcp-library-refresh is per-instance cache warming
    // and arms on every Rust instance; update-check and update-reconcile
    // self-gate by install mode (a quiet no-op on checkout/dev/off) and by
    // adoption — the engine acts only on instances that handed over the
    // keys (the minute hand's reconcile is one settings-row read when idle).
    talaria_mcp_library::register_mcp_library_refresh_job(talaria_mcp_library::library());
    talaria_mcp::pkg::register_pkg_reconcile_job(state.pg.clone());
    talaria_update_job::register_update_check_job(Arc::new(talaria_update_job::UpdateDeps {
        state: state.clone(),
    }));
    talaria_update_job::register_update_reconcile_job(Arc::new(talaria_update_job::UpdateDeps {
        state: state.clone(),
    }));
}

/// The census kinds this build cannot define. Empty is arming's
/// precondition; non-empty is the checklist, spelled out for the operator.
fn missing_run_kinds() -> Vec<&'static str> {
    BOOT_RUN_KINDS
        .iter()
        .filter(|k| run_definition(k).is_none())
        .copied()
        .collect()
}

/// The boot step: build every real edge, declare the table, arm.
///
/// RETRIED, not fatal. Boot is not allowed to depend on Postgres or Redis
/// being up (the pools are lazy for exactly that reason), but arming needs a
/// live ConnectionManager for the leases and a loaded secretbox for the mail
/// drain. Rather than exit — which would take the API down with the schedule,
/// a crash loop that serves nobody — this waits and tries again every 5s
/// until the dependencies answer. A schedule that never arms is the "work
/// that silently never happens" failure this whole plane exists to end, so
/// the first failure and then every ~30s worth is said out loud, and the
/// message names what is missing (a dead dependency, or run kinds this
/// build cannot define — the one condition that will not fix itself).
pub async fn arm(state: AppState) {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match try_arm(&state).await {
            Ok(()) => return,
            Err(e) => {
                if attempt == 1 || attempt.is_multiple_of(6) {
                    tracing::warn!(
                        "{LOG} cannot arm the schedule yet (attempt {attempt}): {e} — retrying every 5s"
                    );
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn try_arm(state: &AppState) -> Result<(), String> {
    let conn = state
        .redis()
        .await
        .map_err(|e| format!("redis is unreachable: {e}"))?;
    let sb = state
        .secretbox()
        .await
        .map_err(|e| format!("the secretbox did not load: {e}"))?;

    // The six armed run steps: their deps are the AppState's edges, and an
    // unarmed step is the loud refusal in the def — reached only by a driver
    // armed before its deps, which this order makes impossible.
    talaria_research_def::arm_research_step(talaria_research_def::real_research_deps(
        state.clone(),
    ));
    talaria_runs_plan_draft::arm_plan_draft_step(talaria_runs_plan_draft::real_plan_draft_deps(
        state.clone(),
    ));
    talaria_runs_work_session::arm_work_session_step(
        talaria_runs_work_session::real_work_session_deps(state.clone()),
    );
    talaria_runs_agent_hire::arm_agent_hire_step(talaria_runs_agent_hire::real_agent_hire_deps(
        state.clone(),
    ));
    talaria_runs_reindex::arm_backfill_step(talaria_runs_reindex::real_backfill_deps(
        state.clone(),
    ));
    talaria_runs_reindex::arm_reindex_step(talaria_runs_reindex::real_reindex_deps(state.clone()));
    // The boot kind list: touch each getter so its kind registers NOW, then
    // hold arming to the census's table.
    let _ = talaria_research_def::research_run();
    let _ = talaria_runs_plan_draft::plan_draft_run();
    let _ = talaria_runs_work_session::work_session_run();
    let _ = talaria_runs_agent_hire::agent_hire_run();
    let _ = talaria_runs_reindex::backfill_run();
    let _ = talaria_runs_reindex::reindex_run();
    let missing = missing_run_kinds();
    if !missing.is_empty() {
        return Err(format!(
            "run kinds with no definition in this build: {} — the sweep would strand their rows. \
             The schedule arms when the census's kind table is whole.",
            missing.join(", ")
        ));
    }

    let rt = RealtimeDeps::publish_only(Some(conn.clone()));
    let run = Arc::new(real_run_deps(state.pg.clone(), conn.clone(), rt.clone()));
    register_all(state, run, rt, &sb).await;
    // The push plane rides the same boot, one line below registration: one
    // install and every row the single notification writer files can also
    // reach closed tabs. Here in try_arm rather than register_all on
    // purpose — the completeness test drives register_all with a dead lazy
    // pool, and the plane's OnceLock must hold the real one or nothing.
    talaria_push::install_push_plane(state.pg.clone(), sb.clone());
    // start_scheduler runs the missing-jobs boot check and logs the armed
    // summary itself; its return is that list again.
    let armed = scheduler::start_scheduler(conn);
    tracing::info!(
        "{LOG} the schedule is this process's — {} job(s) declared, {} armed",
        REQUIRED_JOBS.len() + 1,
        armed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_scheduler::JobName;

    // The completeness test injects fake edges because registration only
    // captures them. The real constructors are each tested in their own
    // module; what this pins is the LIST — the one thing no other test can,
    // because a register call that falls out of `register_all` is silent
    // everywhere except production.
    #[tokio::test]
    async fn register_all_declares_the_whole_ported_table() {
        let url = "postgres://jobs-flip-test@localhost:5432/jobs-flip-test";
        let pg = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy(url)
            .expect("a lazy pool connects to nothing");
        let cfg = talaria_config::Config::from_parts(
            url.into(),
            "redis://jobs-flip-test@localhost:6379".into(),
            "test-root".into(),
            String::new(),
            String::new(),
            "0".into(),
        )
        .expect("the test config is valid on its face");
        let state = AppState::new(pg, Arc::new(cfg));
        let sb = SecretBox::from_parts([0u8; 32], std::collections::HashMap::new(), None);
        register_all(
            &state,
            Arc::new(test_run_deps()),
            RealtimeDeps::publish_only(None),
            &sb,
        )
        .await;

        let names: Vec<JobName> = scheduler::scheduler_status(0)
            .iter()
            .map(|s| s.name)
            .collect();
        for required in REQUIRED_JOBS {
            assert!(
                names.contains(required),
                "{} fell out of the job table",
                required.as_str()
            );
        }
        assert!(
            names.contains(&JobName::McpLibraryRefresh),
            "mcp-library-refresh fell out of the job table"
        );
        // The retired hold, pinned from the other side: update-check IS in
        // the table now (the update engine's restart topology landed with
        // it), and it stays out of REQUIRED_JOBS — a no-op job on dormant
        // installs must not fail anyone's boot.
        assert!(
            names.contains(&JobName::UpdateCheck),
            "update-check fell out of the job table — the scheduled check silently stopped"
        );
        assert!(
            !REQUIRED_JOBS.contains(&JobName::UpdateCheck),
            "update-check must stay optional: dormant installs legitimately run it as a no-op"
        );
        // The minute hand, same contract: a stranded cutover must not wait
        // hours for a reader, so its finisher must never silently stop.
        assert!(
            names.contains(&JobName::UpdateReconcile),
            "update-reconcile fell out of the job table — a stranded cutover would wait hours for a reader"
        );
        assert!(
            !REQUIRED_JOBS.contains(&JobName::UpdateReconcile),
            "update-reconcile must stay optional: dormant installs legitimately run it as a no-op"
        );

        // THE ROLL'S EDGES, the same completeness rule one layer down. These
        // OnceLocks are how the reconcile crate reaches the renderer, and an
        // unwired one makes `roll_agent` return "… not wired" — a sentence
        // the control route discards while answering `{"rolling": true}`.
        // Nothing else in the crate graph runs boot, so this is the only
        // place the absence can be caught before an operator notices that
        // their agents never changed.
        assert!(
            talaria_fleet_reconcile::RENDER_FLEET.get().is_some(),
            "the roll's overlay renderer fell out of the boot wiring — every roll would silently do nothing"
        );
        assert!(
            talaria_fleet_reconcile::NEXT_FREE_PORT.get().is_some(),
            "the roll's port allocator fell out of the boot wiring — every roll would silently do nothing"
        );
    }

    #[test]
    fn arming_refuses_without_the_whole_kind_table() {
        // Touch the getters exactly as try_arm does, so the registry reflects
        // a real boot and not whatever earlier tests loaded.
        let _ = talaria_research_def::research_run();
        let _ = talaria_runs_plan_draft::plan_draft_run();
        let _ = talaria_runs_work_session::work_session_run();
        let _ = talaria_runs_agent_hire::agent_hire_run();
        let _ = talaria_runs_reindex::backfill_run();
        let _ = talaria_runs_reindex::reindex_run();
        // The census's kind table is WHOLE. An empty list is the assertion
        // now, not the goal: a kind showing up here means a def module fell
        // out of try_arm's boot list, and the sweep would strand that kind's
        // rows from the moment arming succeeds.
        let missing = missing_run_kinds();
        assert!(
            missing.is_empty(),
            "the census's kind table has holes: {missing:?} — every BOOT_RUN_KINDS entry \
             needs its getter touched in try_arm"
        );
    }

    /// The lease edge no test can build (ConnectionManager dials on
    /// construction) and no registration ever invokes. Same shape as
    /// work_dispatch's test deps.
    fn test_run_deps() -> RunDeps {
        use futures_util::future::BoxFuture;
        use talaria_runs_run::{LeaseClaim, LeaseRenewal, PauseArgs, PauseOutcome, RunLease};
        use talaria_runs_store::WriteFailure;

        struct NeverLease;
        impl RunLease for NeverLease {
            fn acquire<'a>(&'a self, _: &'a str, _: i64) -> BoxFuture<'a, LeaseClaim> {
                Box::pin(std::future::pending())
            }
            fn renew<'a>(&'a self, _: &'a str, _: &'a str, _: i64) -> BoxFuture<'a, LeaseRenewal> {
                Box::pin(std::future::pending())
            }
            fn release<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, ()> {
                Box::pin(std::future::pending())
            }
        }
        RunDeps {
            store: Arc::new(talaria_runs_store::PgRunStore::new(
                sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy("postgres://jobs-flip-test@localhost:5432/jobs-flip-test")
                    .expect("a lazy pool connects to nothing"),
            )),
            lease: Arc::new(NeverLease),
            publish: Arc::new(|_, _| ()),
            pause: Arc::new(|_: PauseArgs| {
                Box::pin(async {
                    PauseOutcome::Refused {
                        reason: WriteFailure::Missing,
                        state: None,
                    }
                })
            }),
            definition_for: Arc::new(|_| None),
            now: Arc::new(|| 0),
            new_id: Arc::new(|| "new".into()),
        }
    }
}
