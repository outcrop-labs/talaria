// THE AGENT-HIRE RUN — "create an agent" as durable work, on the same runtime
// research, plan-drafts, work sessions and retrieval repairs run on.
//
// WHY A RUN AT ALL, when the route used to do this synchronously in one POST:
// hiring is not a row write, it is a BOOT — render the fleet config, `docker
// compose up`, and wait out a healthcheck window that runs to two minutes on a
// cold pull. Inside one POST that is a promise to stay on the line: the modal
// could not close, a proxy could kill the request long before waitHealthy
// finished, and an agent whose request died was still created server-side,
// visible only after a refresh nobody knew they needed. A run is a row. The
// modal closes the moment the intent is recorded, the roster shows the hire
// working through its phases, and a server restart mid-boot re-enters from the
// last stage that finished.
//
// FOUR STAGES, ONE STEP FUNCTION (the engine's contract is a single `step`
// re-entered with a checkpoint; stages are the checkpoint's value):
//   create → key + def row + v1 config + starter skills + audit
//   render → write the fleet compose/config files
//   pull   → the agent image onto the host, only when `start` was asked for
//   boot   → up + waitHealthy, only when `start` was asked for
//
// THE PULL IS ITS OWN STAGE because it is the one part of a hire with no
// honest bound. The Hermes image is about 1.1 GB to download and 4.5 GB on
// disk, and a host that lacks it (a fresh install, or every host on the
// first hire after a pin bump) downloads it at whatever its link does. Inside
// `boot` it was capped by this run's `max_step_ms`, which a step that blows
// is filed as an error and never retried: on a link slower than about 2.5
// MB/s the hire failed at ten minutes with the download still going, and
// the roster said "starting the container" the whole time. So the pull runs
// on a task of its own (talaria-fleet-docker's single-flight pull, which
// every other start and roll joins), and each step of this stage waits a
// short slice on it and reports what it has downloaded so far as the phase.
// No step runs long, the roster shows the download moving, and a re-entered
// step joins the pull already in flight instead of starting it over.
//
// RESUME RULES. `create` is the one stage that is not naturally idempotent —
// createAgent refuses a taken slug — so a driver that dies between the def row
// and the checkpoint write would, on reclaim, error against its own work. The
// real deps therefore treat "already exists" as the resume signal and hand
// back the existing def: the work is done, not failed. `render`, `pull` and
// `boot` are idempotent by nature (they rewrite/re-up the same files, and a
// pull of an image the host has is a no-op).
//
// THE REAL DEPS ARE THE FLEET WRITE PLANE: createAgent
// (talaria_fleet_create::create_or_resume), writeSkill, renderFleet,
// fleetUp/waitHealthy. `real_agent_hire_deps` wires them below, and
// `jobs.rs`'s `try_arm` arms the step and touches the getter together — an
// armed-in-name-only def would let the boot census pass while a hire cannot
// run, which is the same hole the kind guard exists to plug.

use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use talaria_runs_define::{
    DEFAULT_MAX_ATTEMPTS, RunDefinition, RunStepContext, StepError, StepResult, audience,
    register_run,
};

pub const AGENT_HIRE_KIND: &str = "agent-hire";

/// Everything the modal sends, camelCase as the row's input column holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHireInput {
    pub slug: String,
    pub department: String,
    pub display_name: String,
    /// Free text under the display name; null when the hire is name-only.
    #[serde(default)]
    pub role: Option<String>,
    /// Clone this agent's config; null for the platform defaults.
    #[serde(default)]
    pub template_id: Option<String>,
    /// Override the starter-soul scaffold (e.g. an AI-designed soul).
    #[serde(default)]
    pub soul: Option<String>,
    /// Starter skills written after the def exists.
    #[serde(default)]
    pub skills: Vec<SkillSeed>,
    /// Render + up + wait for health, or just write the def.
    #[serde(default)]
    pub start: bool,
    /// Audit actor — the email/name of the admin who clicked Create.
    pub actor: String,
    /// Who the agent will BELONG to: the id of the person who hired it, its
    /// first manager (docs/PERMISSIONS.md, "Agent managers"). Defaulted so a
    /// hire row enqueued by the previous image still parses on resume — one
    /// without it lands admin-managed, which is where those agents already
    /// were.
    #[serde(default)]
    pub manager_user_id: Option<String>,
}

/// One starter skill: a name and the SKILL.md content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSeed {
    pub name: String,
    pub content: String,
}

/// A row checkpointed at `boot` by an image from before the pull stage still
/// parses and boots as it did: its `up` pulls the image itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HireStage {
    Create,
    Render,
    Pull,
    Boot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHireCheckpoint {
    /// The def row create wrote; null until the create stage finishes.
    #[serde(default)]
    pub def_id: Option<String>,
    pub stage: HireStage,
    /// Warnings the render stage saw, carried to the result — boot never
    /// re-renders to recover them.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// What a finished hire reports. `healthy` is absent when `start` was not
/// asked for; otherwise the healthcheck's answer — false means created but not
/// healthy, which is a warning the roster already knows how to show, not a
/// failed hire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHireResult {
    pub def_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    /// Warnings from the fleet render, surfaced verbatim.
    pub warnings: Vec<String>,
}

/// What the create dep hands back — the def row as the step uses it. The
/// step reads four fields and the checkpoint keeps one, and naming them here
/// keeps the seam honest about what the machine actually depends on.
#[derive(Debug, Clone, PartialEq)]
pub struct HiredDef {
    pub id: String,
    pub slug: String,
    pub department: String,
    pub display_name: String,
}

/// What the render dep answers. fleetUp answers with the compose service it
/// brought up; the run doesn't read it, so `up` just promises the effect.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderOutcome {
    pub warnings: Vec<String>,
}

// ── The seam ──────────────────────────────────────────────────────────────────
//
// Same shape as the other defs': every outward effect is a closure so the
// stage machine is testable with a recorder and no database, no docker, no
// clock. `create` is the one dep that can FAIL the run (a slug the pre-check
// missed, an install with no models); the real create dep swallows "already
// exists" itself — that is the resume signal, and the machine never sees it.

pub type CreateFn =
    Arc<dyn Fn(AgentHireInput) -> BoxFuture<'static, Result<HiredDef, StepError>> + Send + Sync>;
pub type WriteSkillsFn =
    Arc<dyn Fn(String, Vec<SkillSeed>, String) -> BoxFuture<'static, ()> + Send + Sync>;
/// The audit is fire-and-forget (log_audit itself swallows insert failures),
/// so the seam is synchronous and infallible.
pub type AuditFn = Arc<dyn Fn(&HiredDef, &str) + Send + Sync>;
pub type RenderFn =
    Arc<dyn Fn() -> BoxFuture<'static, Result<RenderOutcome, StepError>> + Send + Sync>;
/// Start (or join) the pull of the department's image and wait one slice on
/// it: done, failed, or still downloading with what it has so far. Never
/// waits long — the slice is the dep's, well inside `max_step_ms`.
pub type PullFn = Arc<dyn Fn(String) -> BoxFuture<'static, PullStatus> + Send + Sync>;
pub type UpFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), StepError>> + Send + Sync>;
pub type WaitHealthyFn = Arc<dyn Fn(String) -> BoxFuture<'static, bool> + Send + Sync>;

pub use talaria_fleet_docker::{PullProgress, PullStatus};

#[derive(Clone)]
pub struct AgentHireDeps {
    pub create: CreateFn,
    pub write_skills: WriteSkillsFn,
    pub audit: AuditFn,
    pub render: RenderFn,
    pub pull: PullFn,
    pub up: UpFn,
    pub wait_healthy: WaitHealthyFn,
}

/// How long one step of the pull stage waits on the pull before it reports
/// progress and comes back. Short enough that the roster's phase moves
/// every few polls; long enough that a fast or already-present image goes
/// straight through in one step.
pub const PULL_SLICE: std::time::Duration = std::time::Duration::from_secs(15);

/// The sentence a hire whose image could not be downloaded fails with — one
/// line (the run row keeps the first), saying what happened in words an
/// admin can act on, then docker's own reason. The agent exists by then:
/// Retry on the hire's row (`POST /api/fleet/hires/{id}/retry`) or Start on
/// its card runs the same pull again, which resumes what had arrived.
pub fn pull_failure_sentence(error: &str) -> String {
    let lower = error.to_lowercase();
    let what = if lower.contains("toomanyrequests") || lower.contains("rate limit") {
        "Docker Hub refused the agent image: this server has reached its pull limit. Sign the \
         server's docker in (docker login) or wait a few hours, then press Retry (or Start on the agent)."
    } else if lower.contains("no space left on device") {
        "The server ran out of disk space downloading the agent image (it needs about 6 GB \
         free). Free some space, then press Retry (or Start on the agent)."
    } else if lower.contains("never started the download") {
        "Docker on the server did not start the agent image download for 10 minutes: it is \
         overloaded or stuck (check its disk, or restart docker), then press Retry (or Start \
         on the agent)."
    } else if lower.contains("stopped making progress") {
        "The agent image download stopped making progress. Check the server's connection to \
         the registry, then press Retry (or Start on the agent)."
    } else {
        "The agent image could not be downloaded. Press Retry (or Start on the agent) to try again."
    };
    let why = error.lines().map(str::trim).find(|l| !l.is_empty());
    match why {
        Some(why) => format!("{what} Docker said: {why}"),
        None => what.to_string(),
    }
}

/// How long the hire's second `up` waits for a container the first one's
/// killed CLI asked docker to create to show up (see the up dep).
pub const CREATE_LANDS_WITHIN: std::time::Duration = std::time::Duration::from_secs(120);

/// The sentence a hire whose container would not start fails with — one line,
/// with docker's own error rather than compose's first line of progress
/// ("Container … Creating", which is what the run row kept before).
pub fn boot_failure_sentence(error: &str) -> String {
    if error.starts_with("could not pull the image") {
        // A row checkpointed at `boot` before the pull stage pulls in its up.
        return pull_failure_sentence(error);
    }
    let lines: Vec<&str> = error
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let said = lines
        .iter()
        .rev()
        .find(|l| l.contains("Error") || l.contains("error"))
        .or(lines.first())
        .copied()
        .unwrap_or("");
    let what = if talaria_fleet_docker::is_timeout(error) {
        "Docker did not finish starting the agent's container in time (the server's disk may be \
         busy). Press Retry (or Start on the agent) to try again."
    } else {
        "The agent's container could not be started. Press Retry (or Start on the agent) to try \
         again."
    };
    if said.is_empty() {
        what.to_string()
    } else {
        format!("{what} Docker said: {said}")
    }
}

/// The stage machine. Pure with respect to the deps: every branch is entered
/// from the checkpoint the previous stage wrote, and a hire that did not ask
/// to start never touches the container.
pub async fn agent_hire_step(
    ctx: RunStepContext,
    deps: &AgentHireDeps,
) -> Result<StepResult, StepError> {
    let input: AgentHireInput = serde_json::from_value(ctx.input.clone())
        .map_err(|e| format!("agent-hire input does not parse: {e}"))?;
    let cp: AgentHireCheckpoint = if ctx.checkpoint.is_null() {
        AgentHireCheckpoint {
            def_id: None,
            stage: HireStage::Create,
            warnings: Vec::new(),
        }
    } else {
        serde_json::from_value(ctx.checkpoint.clone())
            .map_err(|e| format!("agent-hire checkpoint does not parse: {e}"))?
    };

    if cp.stage == HireStage::Create {
        let def = (deps.create)(input.clone()).await?;
        (deps.write_skills)(def.slug.clone(), input.skills.clone(), input.actor.clone()).await;
        (deps.audit)(&def, &input.actor);
        let skills_note = if input.skills.is_empty() {
            String::new()
        } else {
            format!(
                ", {} starter skill{}",
                input.skills.len(),
                if input.skills.len() == 1 { "" } else { "s" }
            )
        };
        (ctx.log)(format!(
            "hiring {}: gateway key, config v1{skills_note}",
            def.display_name
        ));
        let checkpoint = serde_json::to_value(AgentHireCheckpoint {
            def_id: Some(def.id),
            stage: HireStage::Render,
            warnings: Vec::new(),
        })
        .map_err(|e| e.to_string())?;
        return Ok(StepResult::Next {
            checkpoint,
            phase: Some("rendering the fleet config".into()),
        });
    }

    // render or boot — both need the def id create wrote. A checkpoint here
    // without one is a corrupt row; the honest recovery is to run create
    // again, whose already-exists path makes that a read, not a duplicate.
    let Some(def_id) = cp.def_id.clone() else {
        let checkpoint = serde_json::to_value(AgentHireCheckpoint {
            def_id: None,
            stage: HireStage::Create,
            warnings: cp.warnings,
        })
        .map_err(|e| e.to_string())?;
        return Ok(StepResult::Next {
            checkpoint,
            phase: None,
        });
    };

    if cp.stage == HireStage::Render {
        let render = (deps.render)().await?;
        if !input.start {
            for w in &render.warnings {
                (ctx.log)(w.clone());
            }
            let result = serde_json::to_value(AgentHireResult {
                def_id,
                healthy: None,
                warnings: render.warnings,
            })
            .map_err(|e| e.to_string())?;
            return Ok(StepResult::Done { result });
        }
        (ctx.log)(if render.warnings.is_empty() {
            "fleet config rendered".to_string()
        } else {
            format!(
                "rendered with {} warning{}",
                render.warnings.len(),
                if render.warnings.len() == 1 { "" } else { "s" }
            )
        });
        let checkpoint = serde_json::to_value(AgentHireCheckpoint {
            def_id: Some(def_id),
            stage: HireStage::Pull,
            warnings: render.warnings,
        })
        .map_err(|e| e.to_string())?;
        return Ok(StepResult::Next {
            checkpoint,
            phase: Some("checking the agent image".into()),
        });
    }

    if cp.stage == HireStage::Pull {
        return match (deps.pull)(input.department.clone()).await {
            PullStatus::Done => {
                let checkpoint = serde_json::to_value(AgentHireCheckpoint {
                    def_id: Some(def_id),
                    stage: HireStage::Boot,
                    warnings: cp.warnings,
                })
                .map_err(|e| e.to_string())?;
                Ok(StepResult::Next {
                    checkpoint,
                    phase: Some("starting the container".into()),
                })
            }
            // Still downloading: the same checkpoint again, with what has
            // arrived so far as the phase. The next step joins the same pull.
            PullStatus::Running(progress) => {
                let checkpoint = serde_json::to_value(&cp).map_err(|e| e.to_string())?;
                Ok(StepResult::Next {
                    checkpoint,
                    phase: Some(progress.phase()),
                })
            }
            PullStatus::Failed(error) => Err(pull_failure_sentence(&error)),
        };
    }

    // boot
    (deps.up)(input.department.clone())
        .await
        .map_err(|e| boot_failure_sentence(&e))?;
    let healthy = (deps.wait_healthy)(input.department.clone()).await;
    (ctx.log)(if healthy {
        format!("{} is up and healthy", input.display_name)
    } else {
        format!(
            "{} created, but the container is not healthy yet",
            input.display_name
        )
    });
    let result = serde_json::to_value(AgentHireResult {
        def_id,
        healthy: Some(healthy),
        warnings: cp.warnings,
    })
    .map_err(|e| e.to_string())?;
    Ok(StepResult::Done { result })
}

/// The real step deps, armed with the fleet write plane. They need the
/// AppState, which the boot wiring owns, so they are installed separately
/// from registration; an unarmed step is the loud refusal below — reached
/// only by a driver armed before its deps, which is a wiring bug.
static ARMED_DEPS: OnceLock<AgentHireDeps> = OnceLock::new();

pub fn arm_agent_hire_step(deps: AgentHireDeps) {
    let _ = ARMED_DEPS.set(deps);
}

/// The real deps over the fleet write plane. Each closure owns its own clone
/// of the state: arming captures, never invokes, and a run may drive minutes
/// after the arm.
pub fn real_agent_hire_deps(state: talaria_state::AppState) -> AgentHireDeps {
    AgentHireDeps {
        create: {
            let pg = state.pg.clone();
            Arc::new(move |input: AgentHireInput| {
                let pg = pg.clone();
                Box::pin(async move {
                    let created = talaria_fleet_create::create_or_resume(
                        &pg,
                        &talaria_fleet_create::CreateAgentInput {
                            slug: input.slug,
                            department: input.department,
                            display_name: input.display_name,
                            role: input.role,
                            template_id: input.template_id,
                            created_by: input.actor,
                            soul: input.soul,
                            manager_user_id: input.manager_user_id,
                        },
                    )
                    .await?;
                    Ok(HiredDef {
                        id: created.def.id,
                        slug: created.def.slug,
                        department: created.def.department,
                        display_name: created.def.display_name,
                    })
                })
            })
        },
        write_skills: {
            let pg = state.pg.clone();
            Arc::new(move |slug: String, skills: Vec<SkillSeed>, actor: String| {
                let pg = pg.clone();
                Box::pin(async move {
                    // A starter skill that fails to land is lost quietly,
                    // never a failed hire.
                    for s in skills {
                        let _ = talaria_agent_skills::write_skill(
                            &pg,
                            &slug,
                            &s.name,
                            &s.content,
                            Some(&actor),
                        )
                        .await;
                    }
                })
            })
        },
        audit: {
            let pg = state.pg.clone();
            Arc::new(move |def: &HiredDef, actor: &str| {
                // Fire-and-forget: log_audit itself swallows insert failures
                // — the spawn only detaches the write from the step.
                let pg = pg.clone();
                let (id, label, slug, department, actor) = (
                    def.id.clone(),
                    def.display_name.clone(),
                    def.slug.clone(),
                    def.department.clone(),
                    actor.to_string(),
                );
                tokio::spawn(async move {
                    talaria_audit::log_audit(
                        &pg,
                        talaria_audit::AuditEntry {
                            actor: &actor,
                            action: "agent.create",
                            target_type: "agent",
                            target_id: Some(&id),
                            target_label: Some(&label),
                            before: None,
                            after: Some(serde_json::json!({
                                "slug": slug,
                                "department": department,
                            })),
                        },
                    )
                    .await;
                });
            })
        },
        render: {
            let st = state.clone();
            Arc::new(move || {
                let st = st.clone();
                Box::pin(async move {
                    // The box is read lazily here, not captured at arm time:
                    // arm already refuses to fire until it loads, and a
                    // post-arm failure surfaces through the render's own error
                    // path rather than a poisoned closure.
                    let sb = st.secretbox().await.unwrap_or_default();
                    let out = talaria_fleet_render::render_fleet(&st.pg, &sb).await?;
                    Ok(RenderOutcome {
                        warnings: out.warnings,
                    })
                })
            })
        },
        pull: {
            let pg = state.pg.clone();
            Arc::new(move |department: String| {
                let pg = pg.clone();
                Box::pin(async move {
                    // The pull runs on a task of its own inside fleet-docker
                    // and outlives this wait: a slice that ends, a step the
                    // engine drops, or a run cancelled mid-download leaves
                    // the download going for whoever asks next.
                    talaria_fleet_docker::pull_agent_image_within(&pg, &department, PULL_SLICE)
                        .await
                })
            })
        },
        up: {
            let pg = state.pg.clone();
            Arc::new(move |department: String| {
                let pg = pg.clone();
                Box::pin(async move {
                    // fleetUp answers with the compose service it brought up;
                    // the run doesn't read it — the promise IS the effect.
                    // A failure is answered by the container, not by compose:
                    // an `up` that ran out its deadline under heavy IO can
                    // leave the daemon finishing the create and the start, and
                    // a step that errors here is retried from the top — a
                    // second hire of an agent that is already up.
                    //
                    // On a task of its own: a step future the engine drops
                    // (a lost lease, a shutdown) must not take the compose
                    // verb with it, halfway — `docker()` kills its CLI with
                    // the future that waits on it. The step still waits for
                    // the verdict; a re-entered step finds the container.
                    //
                    // An `up` killed at its deadline that did not land gets
                    // ONE more: measured under heavy IO, compose had created
                    // the container and was killed before the start, leaving
                    // it `Created` with the hire failed — and a second `up`
                    // starts exactly that container in seconds. Any other
                    // failure is compose's answer, and repeating it would
                    // only repeat it.
                    let up = tokio::spawn(async move {
                        let mut tries = 0;
                        loop {
                            tries += 1;
                            match talaria_fleet_docker::fleet_up(&pg, &department).await {
                                Ok(_) => return Ok(()),
                                Err(e)
                                    if talaria_fleet_docker::up_landed(&pg, &department, &e)
                                        .await =>
                                {
                                    tracing::warn!(
                                        "[fleet] hire of {department}: compose reported a failure ({e}), but the container runs the current render — going on"
                                    );
                                    return Ok(());
                                }
                                Err(e) if tries == 1 && talaria_fleet_docker::is_timeout(&e) => {
                                    tracing::warn!(
                                        "[fleet] hire of {department}: up ran out its time and did not land ({e}) — once more"
                                    );
                                    // The killed CLI's create may still be
                                    // landing in the daemon. Measured: a
                                    // second `up` sent before docker showed
                                    // the container failed on the name
                                    // ("Conflict. The container name … is
                                    // already in use") and the container
                                    // turned up `Created` a minute later. So
                                    // wait for docker to show it first.
                                    let until = tokio::time::Instant::now() + CREATE_LANDS_WITHIN;
                                    while talaria_fleet_docker::observe(&pg, &department).await
                                        == talaria_fleet_docker::Observed::Absent
                                        && tokio::time::Instant::now() < until
                                    {
                                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("[fleet] hire of {department}: up failed: {e}");
                                    return Err(e);
                                }
                            }
                        }
                    });
                    up.await
                        .unwrap_or_else(|e| Err(format!("the hire's up task failed: {e}")))
                })
            })
        },
        wait_healthy: {
            let pg = state.pg.clone();
            Arc::new(move |department: String| {
                let pg = pg.clone();
                Box::pin(async move {
                    // Two minutes — enough for a cold pull's healthcheck to
                    // settle.
                    talaria_fleet_docker::wait_healthy(&pg, &department, 120_000).await
                })
            })
        },
    }
}

/// The registered definition, exactly once per process — registered on the
/// first call. The callers are `jobs.rs`'s `try_arm` (the boot list) and the
/// fleet routes (a process that lists or enqueues hires can also be the
/// process a reclaim sweep asks to resume one, and a kind that route never
/// registered would be a run nothing can drive).
pub fn agent_hire_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: AGENT_HIRE_KIND.into(),
            label: "Hire agent".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_DEPS.get().cloned() else {
                        return Err(
                            "agent-hire steps are armed by the scheduler's boot wiring; this \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    agent_hire_step(ctx, &deps).await
                })
            }),
            audience: Arc::new(audience),
            // Boot is the long stage: an up (two minutes at most, its
            // settle window, and — when it ran out its time without landing
            // — up to `CREATE_LANDS_WITHIN` for the create to show and a
            // second up) and waitHealthy's own two-minute window: about ten
            // minutes in the worst case measured on a saturated disk. The image is already
            // on the host by then — the pull stage's steps each wait one
            // `PULL_SLICE` and come back, however long the download takes.
            // The step ceiling clears the worst case rather than the typical
            // one; a step that blows THIS is filed as an error, not retried,
            // because it is probably still running.
            max_step_ms: 15 * 60_000,
            idle_step_ms: None,
            // No override — the default three.
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    })
}

#[cfg(test)]
mod tests {
    use talaria_runs_define::RunRow;
    // The definition's contract, driven with fake deps — no database, no
    // docker, no clock. What is under test is the stage machine: the
    // checkpoint is the only state, each stage is entered from the checkpoint
    // the previous one wrote, and a hire that did not ask to start never
    // touches the container.
    //
    // NOTE: these tests drive `agent_hire_step` directly and never call
    // `agent_hire_run()` — the getter REGISTERS the kind in the process-wide
    // registry, and jobs.rs's kind-census test touches the getters itself to
    // mirror a real boot. A test here registering the kind would not break
    // that assertion (the census test is self-contained), but it would put
    // the registry's state at the mercy of test scheduling for nothing: the
    // machine under test is right here.
    use super::*;
    use std::sync::Mutex;
    use talaria_runs_define::{RunState, StepSignal};

    fn minimal_row() -> RunRow {
        RunRow {
            id: "hire-1".into(),
            kind: AGENT_HIRE_KIND.into(),
            owner_user_id: None,
            subject_type: None,
            subject_id: None,
            state: RunState::Running,
            phase: String::new(),
            checkpoint: serde_json::Value::Null,
            input: serde_json::Value::Null,
            result: serde_json::Value::Null,
            error: None,
            attempt: 0,
            crashes: 0,
            lease_owner: None,
            lease_expires_at: None,
            approval_key: None,
            decision: None,
            created_at: String::new(),
            updated_at: String::new(),
            started_at: None,
            finished_at: None,
        }
    }

    fn ctx(input: &AgentHireInput, checkpoint: Option<AgentHireCheckpoint>) -> RunStepContext {
        let (tx, signal) = StepSignal::channel();
        // A dropped watch sender leaves the channel at its last value —
        // `false`, never aborted — which is the shape an uncontended run has.
        drop(tx);
        RunStepContext {
            activity: talaria_runs_define::StepActivity::new(),
            run: minimal_row(),
            input: serde_json::to_value(input).expect("the test input serializes"),
            checkpoint: checkpoint
                .map(|c| serde_json::to_value(c).expect("the checkpoint serializes"))
                .unwrap_or(serde_json::Value::Null),
            decision: None,
            signal,
            log: Arc::new(|_| {}),
            attempt: 0,
        }
    }

    fn input(start: bool) -> AgentHireInput {
        AgentHireInput {
            slug: "sloane".into(),
            department: "research".into(),
            display_name: "Sloane".into(),
            role: Some("Research Analyst".into()),
            template_id: None,
            soul: Some("# Sloane".into()),
            skills: vec![SkillSeed {
                name: "weekly-sweep".into(),
                content: "the playbook".into(),
            }],
            start,
            actor: "jon@example.com".into(),
            manager_user_id: Some("00000000-0000-0000-0000-00000000beef".into()),
        }
    }

    fn def() -> HiredDef {
        HiredDef {
            id: "def-1".into(),
            slug: "sloane".into(),
            department: "research".into(),
            display_name: "Sloane".into(),
        }
    }

    /// Deps that record every call, so the stage boundaries are observable.
    fn recording() -> (Arc<Mutex<Vec<String>>>, AgentHireDeps) {
        let calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let deps = AgentHireDeps {
            create: {
                let calls = calls.clone();
                Arc::new(move |_| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push("create".into());
                        Ok(def())
                    })
                })
            },
            write_skills: {
                let calls = calls.clone();
                Arc::new(move |_, _, _| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push("skills".into());
                    })
                })
            },
            audit: {
                let calls = calls.clone();
                Arc::new(move |_, _| {
                    calls.lock().unwrap().push("audit".into());
                })
            },
            render: {
                let calls = calls.clone();
                Arc::new(move || {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push("render".into());
                        Ok(RenderOutcome {
                            warnings: vec!["one warning".into()],
                        })
                    })
                })
            },
            pull: {
                let calls = calls.clone();
                Arc::new(move |_| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push("pull".into());
                        PullStatus::Done
                    })
                })
            },
            up: {
                let calls = calls.clone();
                Arc::new(move |_| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push("up".into());
                        Ok(())
                    })
                })
            },
            wait_healthy: {
                let calls = calls.clone();
                Arc::new(move |_| {
                    let calls = calls.clone();
                    Box::pin(async move {
                        calls.lock().unwrap().push("healthy".into());
                        true
                    })
                })
            },
        };
        (calls, deps)
    }

    fn cp_of_from(checkpoint: serde_json::Value) -> AgentHireCheckpoint {
        serde_json::from_value(checkpoint).expect("the checkpoint parses back")
    }

    #[tokio::test]
    async fn walks_create_render_pull_boot_each_stage_entered_from_the_last_checkpoint() {
        let (calls, deps) = recording();

        let a = agent_hire_step(ctx(&input(true), None), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = a else {
            panic!("expected next after create")
        };
        assert_eq!(
            checkpoint,
            serde_json::json!({"defId": "def-1", "stage": "render", "warnings": []})
        );
        assert_eq!(phase, Some("rendering the fleet config".to_string()));
        assert_eq!(*calls.lock().unwrap(), vec!["create", "skills", "audit"]);

        let b = agent_hire_step(ctx(&input(true), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = b else {
            panic!("expected next after render")
        };
        assert_eq!(
            checkpoint,
            serde_json::json!({"defId": "def-1", "stage": "pull", "warnings": ["one warning"]})
        );
        assert_eq!(phase, Some("checking the agent image".to_string()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["create", "skills", "audit", "render"]
        );

        let p = agent_hire_step(ctx(&input(true), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, phase } = p else {
            panic!("expected next after pull")
        };
        assert_eq!(
            checkpoint,
            serde_json::json!({"defId": "def-1", "stage": "boot", "warnings": ["one warning"]})
        );
        assert_eq!(phase, Some("starting the container".to_string()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["create", "skills", "audit", "render", "pull"]
        );

        let c = agent_hire_step(ctx(&input(true), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        let StepResult::Done { result } = c else {
            panic!("expected done after boot")
        };
        assert_eq!(
            result,
            serde_json::json!({"defId": "def-1", "healthy": true, "warnings": ["one warning"]})
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                "create", "skills", "audit", "render", "pull", "up", "healthy"
            ]
        );
    }

    #[tokio::test]
    async fn never_touches_the_container_when_start_was_not_asked_for() {
        let (calls, deps) = recording();

        let a = agent_hire_step(ctx(&input(false), None), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, .. } = a else {
            panic!("expected next after create")
        };
        let b = agent_hire_step(ctx(&input(false), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        // `healthy` is absent, not false — the absent key is the shape the
        // roster's result reader expects.
        let StepResult::Done { result } = b else {
            panic!("expected done without booting")
        };
        assert_eq!(
            result,
            serde_json::json!({"defId": "def-1", "warnings": ["one warning"]})
        );
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["create", "skills", "audit", "render"]
        );
    }

    #[tokio::test]
    async fn files_not_healthy_as_a_warning_in_the_result_not_a_failed_run() {
        let (calls, deps) = recording();
        let deps = AgentHireDeps {
            wait_healthy: Arc::new(|_| Box::pin(async move { false })),
            ..deps
        };

        let a = agent_hire_step(ctx(&input(true), None), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, .. } = a else {
            panic!("expected next after create")
        };
        let b = agent_hire_step(ctx(&input(true), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, .. } = b else {
            panic!("expected next after render")
        };
        let c = agent_hire_step(ctx(&input(true), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        let StepResult::Next { checkpoint, .. } = c else {
            panic!("expected next after pull")
        };
        let d = agent_hire_step(ctx(&input(true), Some(cp_of_from(checkpoint))), &deps)
            .await
            .unwrap();
        let StepResult::Done { result } = d else {
            panic!("expected done")
        };
        assert_eq!(result["healthy"], serde_json::json!(false));
        let _ = calls; // the recorder's order is the first test's business
    }

    #[tokio::test]
    async fn resumes_from_a_mid_boot_crash_without_re_running_create() {
        // A driver that died after render re-enters at boot: the def, the
        // skills and the audit are the previous driver's completed work, not
        // a rerun.
        let (calls, deps) = recording();

        let c = agent_hire_step(
            ctx(
                &input(true),
                Some(AgentHireCheckpoint {
                    def_id: Some("def-1".into()),
                    stage: HireStage::Boot,
                    warnings: vec![],
                }),
            ),
            &deps,
        )
        .await
        .unwrap();
        assert!(matches!(c, StepResult::Done { .. }));
        assert_eq!(*calls.lock().unwrap(), vec!["up", "healthy"]);
    }

    #[tokio::test]
    async fn a_corrupt_mid_stage_checkpoint_re_runs_create_as_a_read() {
        // render/boot without a defId is a corrupt row; the machine restarts
        // at create, whose already-exists path makes that a read. This pins
        // the recovery.
        let (calls, deps) = recording();

        let res = agent_hire_step(
            ctx(
                &input(true),
                Some(AgentHireCheckpoint {
                    def_id: None,
                    stage: HireStage::Boot,
                    warnings: vec!["kept".into()],
                }),
            ),
            &deps,
        )
        .await
        .unwrap();
        let StepResult::Next { checkpoint, phase } = res else {
            panic!("expected next back to create")
        };
        assert_eq!(
            checkpoint,
            serde_json::json!({"defId": null, "stage": "create", "warnings": ["kept"]})
        );
        assert_eq!(phase, None);
        assert_eq!(*calls.lock().unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_failed_create_fails_the_step_and_writes_no_checkpoint() {
        let (calls, deps) = recording();
        let deps = AgentHireDeps {
            create: Arc::new(|_| {
                Box::pin(async move { Err("agent \"sloane\" already exists".into()) })
            }),
            ..deps
        };

        let err = agent_hire_step(ctx(&input(true), None), &deps)
            .await
            .unwrap_err();
        assert!(
            err.contains("already exists"),
            "the create error carries through: {err}"
        );
        assert_eq!(*calls.lock().unwrap(), Vec::<String>::new());
    }
    fn at(stage: HireStage) -> AgentHireCheckpoint {
        AgentHireCheckpoint {
            def_id: Some("def-1".into()),
            stage,
            warnings: vec!["kept".into()],
        }
    }

    #[tokio::test]
    async fn a_slow_pull_stays_its_own_stage_and_says_what_has_arrived_until_it_lands() {
        // The pull answers a slice at a time: twice still downloading, then
        // done. Each step comes back at once with the same checkpoint and
        // the download so far as its phase, and nothing touches the
        // container until the image is on the host. The old machine pulled
        // inside boot's one step, under max_step_ms.
        let (calls, deps) = recording();
        let answers = Arc::new(Mutex::new(vec![
            PullStatus::Done,
            PullStatus::Running(PullProgress {
                downloaded: 1_020_000_000,
                layers_ready: 36,
                layers: 38,
                unpacking: true,
                started: tokio::time::Instant::now(),
            }),
            PullStatus::Running(PullProgress {
                downloaded: 412_000_000,
                layers_ready: 36,
                layers: 38,
                unpacking: false,
                started: tokio::time::Instant::now(),
            }),
        ]));
        let deps = AgentHireDeps {
            pull: {
                let calls = calls.clone();
                Arc::new(move |department| {
                    assert_eq!(department, "research");
                    let next = answers.lock().unwrap().pop().expect("asked once too often");
                    calls.lock().unwrap().push("pull".into());
                    Box::pin(async move { next })
                })
            },
            ..deps
        };

        let mut cp = at(HireStage::Pull);
        let mut phases = Vec::new();
        loop {
            let step = agent_hire_step(ctx(&input(true), Some(cp.clone())), &deps)
                .await
                .unwrap();
            let StepResult::Next { checkpoint, phase } = step else {
                panic!("the pull stage only ever moves on")
            };
            phases.push(phase.unwrap_or_default());
            let next = cp_of_from(checkpoint);
            if next.stage == HireStage::Boot {
                assert_eq!(next.warnings, vec!["kept".to_string()]);
                break;
            }
            assert_eq!(next, cp, "a pull still running keeps its checkpoint");
            cp = next;
        }
        assert_eq!(
            phases,
            vec![
                "downloading the agent image — 412 MB so far, 36 of 38 layers ready",
                "unpacking the agent image — 1.0 GB so far, 36 of 38 layers ready",
                "starting the container",
            ]
        );
        assert_eq!(*calls.lock().unwrap(), vec!["pull", "pull", "pull"]);
    }

    #[tokio::test]
    async fn a_failed_pull_fails_the_hire_with_what_to_do_and_never_boots() {
        let (calls, deps) = recording();
        let deps = AgentHireDeps {
            pull: Arc::new(|_| {
                Box::pin(async move {
                    PullStatus::Failed(
                        "could not pull the image for agent-research: docker compose failed: \
                         toomanyrequests: You have reached your unauthenticated pull rate limit."
                            .into(),
                    )
                })
            }),
            ..deps
        };
        let err = agent_hire_step(ctx(&input(true), Some(at(HireStage::Pull))), &deps)
            .await
            .unwrap_err();
        assert!(
            err.starts_with("Docker Hub refused the agent image"),
            "{err}"
        );
        assert!(err.contains("press Retry (or Start on the agent)"), "{err}");
        assert!(
            err.contains("toomanyrequests"),
            "docker's own reason is kept: {err}"
        );
        assert!(!err.contains('\n'), "the run row keeps one line: {err}");
        assert_eq!(*calls.lock().unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_boot_failure_keeps_docker_s_error_not_compose_s_progress() {
        // Verbatim shape of a failed second up, measured on this branch.
        let e = "docker compose failed: Container talaria-wt-rel-hire-fleet-agent-fxone-1 Creating \n \
                 service:agent-fxone:1 Error response from daemon: Conflict. The container name \
                 \"/talaria-wt-rel-hire-fleet-agent-fxone-1\" is already in use by container \"9be6\". \n\
                 Error response from daemon: Conflict. The container name \"/x\" is already in use.";
        let s = boot_failure_sentence(e);
        assert!(
            s.starts_with("The agent's container could not be started."),
            "{s}"
        );
        assert!(s.ends_with("Docker said: Error response from daemon: Conflict. The container name \"/x\" is already in use."), "{s}");
        assert!(!s.contains('\n'), "{s}");
        let t = boot_failure_sentence("docker compose …: timed out after 120s");
        assert!(t.starts_with("Docker did not finish starting"), "{t}");
        let p = boot_failure_sentence(
            "could not pull the image for agent-ops: docker compose failed: manifest unknown",
        );
        assert!(
            p.starts_with("The agent image could not be downloaded."),
            "{p}"
        );
    }

    #[tokio::test]
    async fn a_failed_up_fails_the_hire_with_docker_s_reason() {
        let (_, deps) = recording();
        let deps = AgentHireDeps {
            up: Arc::new(|_| {
                Box::pin(async move {
                    Err("docker compose failed: Container x Creating \nError response from daemon: port is already allocated".into())
                })
            }),
            ..deps
        };
        let err = agent_hire_step(ctx(&input(true), Some(at(HireStage::Boot))), &deps)
            .await
            .unwrap_err();
        assert!(
            err.ends_with("Docker said: Error response from daemon: port is already allocated"),
            "{err}"
        );
    }

    #[test]
    fn a_pull_failure_reads_as_what_an_admin_can_do_about_it() {
        let full = pull_failure_sentence(
            "could not pull the image for agent-ops: docker compose failed: write /var/lib/docker/x: no space left on device",
        );
        assert!(
            full.starts_with("The server ran out of disk space"),
            "{full}"
        );
        let stalled = pull_failure_sentence(
            "could not pull the image for agent-ops: the download stopped making progress (nothing from docker for 10 min) — downloading the agent image — 412 MB so far",
        );
        assert!(
            stalled.starts_with("The agent image download stopped making progress"),
            "{stalled}"
        );
        let never = pull_failure_sentence(
            "could not pull the image for agent-ops: docker never started the download (nothing from docker for 10 min)",
        );
        assert!(
            never.starts_with("Docker on the server did not start"),
            "{never}"
        );
        let other = pull_failure_sentence(
            "could not pull the image for agent-ops: docker compose failed: manifest unknown\nmore",
        );
        assert_eq!(
            other,
            "The agent image could not be downloaded. Press Retry (or Start on the agent) to try \
             again. Docker said: could not pull the image for agent-ops: docker compose failed: manifest unknown"
        );
    }
}
