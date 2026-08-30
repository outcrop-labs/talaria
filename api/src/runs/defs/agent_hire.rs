// THE AGENT-HIRE RUN — "create an agent" as durable work, on the same runtime
// research and plan-drafts already run on. Port of
// ui/src/server/runs/defs/agent-hire.ts.
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
// THREE STAGES, ONE STEP FUNCTION (the engine's contract is a single `step`
// re-entered with a checkpoint; stages are the checkpoint's value):
//   create → key + def row + v1 config + starter skills + audit
//   render → write the fleet compose/config files
//   boot   → up + waitHealthy, only when `start` was asked for
//
// RESUME RULES. `create` is the one stage that is not naturally idempotent —
// createAgent refuses a taken slug — so a driver that dies between the def row
// and the checkpoint write would, on reclaim, error against its own work. The
// real deps therefore treat "already exists" as the resume signal and hand
// back the existing def: the work is done, not failed. `render` and `boot` are
// idempotent by nature (they rewrite/re-up the same files).
//
// THE REAL DEPS ARE THE FLEET WRITE PLANE, and every edge of it has crossed:
// createAgent (fleet_create::create_or_resume), writeSkill, renderFleet,
// fleetUp/waitHealthy. `real_agent_hire_deps` wires them below, and
// `jobs.rs`'s `try_arm` arms the step and touches the getter in the same
// slice the flip's kind guard stops naming agent-hire as missing — an
// armed-in-name-only def would let the flip arm while a hire cannot run,
// which is the same hole the kind guard exists to plug.

use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::runs::define::{
    Authority, DEFAULT_MAX_ATTEMPTS, RunDefinition, RunRow, RunStepContext, StepError, StepResult,
    register_run,
};

pub const AGENT_HIRE_KIND: &str = "agent-hire";

/// Everything the modal sends, camelCase as the row's input column holds it
/// (agent-hire.ts AgentHireInput).
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
}

/// One starter skill: a name and the SKILL.md content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkillSeed {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HireStage {
    Create,
    Render,
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

/// What the create dep hands back — the def row as the step uses it. TS hands
/// the whole AgentDef back; the step reads four fields and the checkpoint
/// keeps one, and naming them here keeps the seam honest about what the
/// machine actually depends on.
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
/// TS's audit is fire-and-forget (`void logAudit(...)` — logAudit itself
/// swallows insert failures), so the seam is synchronous and infallible.
pub type AuditFn = Arc<dyn Fn(&HiredDef, &str) + Send + Sync>;
pub type RenderFn =
    Arc<dyn Fn() -> BoxFuture<'static, Result<RenderOutcome, StepError>> + Send + Sync>;
pub type UpFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), StepError>> + Send + Sync>;
pub type WaitHealthyFn = Arc<dyn Fn(String) -> BoxFuture<'static, bool> + Send + Sync>;

#[derive(Clone)]
pub struct AgentHireDeps {
    pub create: CreateFn,
    pub write_skills: WriteSkillsFn,
    pub audit: AuditFn,
    pub render: RenderFn,
    pub up: UpFn,
    pub wait_healthy: WaitHealthyFn,
}

/// Who may watch and be told: the admin who clicked Create (the surface that
/// lists hires is admin-gated anyway, same as the roster it lands in).
fn audience(run: &RunRow) -> Authority {
    match &run.owner_user_id {
        Some(owner) => Authority::User {
            user_ids: vec![owner.clone()],
        },
        None => Authority::Admin { on_board: None },
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
            stage: HireStage::Boot,
            warnings: render.warnings,
        })
        .map_err(|e| e.to_string())?;
        return Ok(StepResult::Next {
            checkpoint,
            phase: Some("starting the container".into()),
        });
    }

    // boot
    (deps.up)(input.department.clone()).await?;
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

/// The real step deps, armed with the fleet write plane: the Rust driver does
/// not exist until the flip arms it, so the deps sit empty until the boot
/// wiring (which owns the AppState) installs them. An unarmed step is the
/// loud refusal below — reached only by a driver armed before its deps.
static ARMED_DEPS: OnceLock<AgentHireDeps> = OnceLock::new();

pub fn arm_agent_hire_step(deps: AgentHireDeps) {
    let _ = ARMED_DEPS.set(deps);
}

/// The real deps over the fleet write plane — TS's REAL_AGENT_HIRE_DEPS. Each
/// closure owns its own clone of the state: registration captures, never
/// invokes, and a run may drive minutes after the arm.
pub fn real_agent_hire_deps(state: crate::state::AppState) -> AgentHireDeps {
    AgentHireDeps {
        create: {
            let pg = state.pg.clone();
            Arc::new(move |input: AgentHireInput| {
                let pg = pg.clone();
                Box::pin(async move {
                    let created = crate::fleet_create::create_or_resume(
                        &pg,
                        &crate::fleet_create::CreateAgentInput {
                            slug: input.slug,
                            department: input.department,
                            display_name: input.display_name,
                            role: input.role,
                            template_id: input.template_id,
                            created_by: input.actor,
                            soul: input.soul,
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
                    // TS writes each skill `.catch(() => {})` — a starter
                    // skill that fails to land is lost quietly, never a
                    // failed hire.
                    for s in skills {
                        let _ = crate::agent_skills::write_skill(
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
                // TS fires and forgets (`void logAudit(...)`; logAudit itself
                // swallows insert failures). The spawn is that void; the
                // swallow is log_audit's own.
                let pg = pg.clone();
                let (id, label, slug, department, actor) = (
                    def.id.clone(),
                    def.display_name.clone(),
                    def.slug.clone(),
                    def.department.clone(),
                    actor.to_string(),
                );
                tokio::spawn(async move {
                    crate::audit::log_audit(
                        &pg,
                        crate::audit::AuditEntry {
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
                    let out = crate::fleet_render::render_fleet(&st.pg, &sb, None).await?;
                    Ok(RenderOutcome {
                        warnings: out.warnings,
                    })
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
                    crate::fleet_docker::fleet_up(&pg, &department)
                        .await
                        .map(|_| ())
                })
            })
        },
        wait_healthy: {
            let pg = state.pg.clone();
            Arc::new(move |department: String| {
                let pg = pg.clone();
                Box::pin(async move {
                    // TS's default window: two minutes, enough for a cold
                    // pull's healthcheck to settle.
                    crate::fleet_docker::wait_healthy(&pg, &department, 120_000).await
                })
            })
        },
    }
}

/// The registered definition, exactly once per process — TS registers at
/// module load; the Rust equivalent is the first call. The callers are
/// `jobs.rs`'s `try_arm` (the boot list) and the fleet routes (a process that
/// lists or enqueues hires can also be the process a reclaim sweep asks to
/// resume one, and a kind that route never registered would be a run nothing
/// can drive).
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
                            "agent-hire steps are armed with the fleet write plane; this Rust \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    agent_hire_step(ctx, &deps).await
                })
            }),
            audience: Arc::new(audience),
            // Boot is the long stage: a cold pull plus the healthcheck window
            // is waitHealthy's own two-minute ceiling, and the up before it
            // can take its time building. The step ceiling clears the worst
            // case rather than the typical one; a step that blows THIS is
            // filed as an error, not retried, because it is probably still
            // running.
            max_step_ms: 10 * 60_000,
            // TS sets no override — the default three.
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    })
}

#[cfg(test)]
mod tests {
    // The definition's contract, driven with fake deps the way agent-hire's
    // own TS suite drives them — no database, no docker, no clock. What is
    // under test is the stage machine: the checkpoint is the only state, each
    // stage is entered from the checkpoint the previous one wrote, and a hire
    // that did not ask to start never touches the container.
    //
    // NOTE: these tests drive `agent_hire_step` directly and never call
    // `agent_hire_run()` — the getter REGISTERS the kind in the process-wide
    // registry, and jobs.rs's `the_flip_refuses_to_arm_without_the_whole_kind_
    // table` touches the getters itself to mirror a real boot. A test here
    // registering the kind would not break that assertion (the census test is
    // self-contained), but it would put the registry's state at the mercy of
    // test scheduling for nothing: the machine under test is right here.
    use super::*;
    use crate::runs::define::{RunState, StepSignal};
    use std::sync::Mutex;

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
    async fn walks_create_render_boot_each_stage_entered_from_the_last_checkpoint() {
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
            serde_json::json!({"defId": "def-1", "stage": "boot", "warnings": ["one warning"]})
        );
        assert_eq!(phase, Some("starting the container".to_string()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["create", "skills", "audit", "render"]
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
            vec!["create", "skills", "audit", "render", "up", "healthy"]
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
        // `healthy` is absent, not false — TS's `undefined` key, dropped by
        // JSON.stringify, is the shape the roster's result reader expects.
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
        let StepResult::Done { result } = c else {
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
        // at create, whose already-exists path makes that a read. The TS
        // machine has the branch and no test for it — this pins the recovery.
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
}
