// THE AGENT-DESIGN RUN — one muse `agent` draft as durable work, on the
// runtime research, plan drafts and hires run on.
//
// WHY A RUN. The modal used to await `POST /api/muse` itself, so the design
// was as durable as the tab: a reload, a navigation or a closed laptop threw
// it away, and nothing on the server knew it had existed. The run row
// survives all of that, any api process can drive it (the lease is Redis'),
// and a process that dies mid-draft is reclaimed by the sweep, which re-enters
// this step. Nothing here waits on an HTTP request, so no edge's idle timer
// (Cloudflare's 100 s first) can cut it either.
//
// ONE STEP, NO CHECKPOINTS, like plan-draft: one model call, then one write
// to the design record. AT-LEAST-ONCE: a driver that dies between the call
// and the write makes the call again on reclaim. Accepted for the same reason
// plan-draft accepts it — the output is a draft the person regenerates at
// will, and the window is one Redis write. A second answer for a turn that
// already landed is dropped by `record::land`.

use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use talaria_harness_defs::defs::muse::MuseTurn;
use talaria_runs_define::{RunDefinition, RunStepContext, StepResult, audience, register_run};

use crate::record::DraftFields;

pub const AGENT_DESIGN_KIND: &str = "agent-design";

/// What the step reads, camelCase as the row's input column holds it: the
/// muse request exactly as the turn was asked, so a reclaimed step asks the
/// same question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDesignInput {
    pub user_id: String,
    pub design_id: String,
    /// The ledger and findings attribution, `platform:muse:<who>` — the same
    /// caller the muse route stamps, so a design costs what it always did.
    pub caller: String,
    pub instruction: String,
    /// A refine's base: the form as the person saw it, as JSON.
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub chat: Vec<MuseTurn>,
}

/// The answer, for the record that asked.
#[derive(Debug, Clone)]
pub struct Landing {
    pub user_id: String,
    pub run_id: String,
    pub outcome: Result<DraftFields, String>,
}

pub type DraftFn =
    Arc<dyn Fn(AgentDesignInput) -> BoxFuture<'static, Result<DraftFields, String>> + Send + Sync>;
pub type LandFn = Arc<dyn Fn(Landing) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

/// The step's edges: ask the muse, write the answer (and tell the person).
#[derive(Clone)]
pub struct AgentDesignDeps {
    pub draft: DraftFn,
    pub land: LandFn,
}

async fn agent_design_step(
    ctx: RunStepContext,
    deps: &AgentDesignDeps,
) -> Result<StepResult, String> {
    let input: AgentDesignInput = serde_json::from_value(ctx.input.clone())
        .map_err(|e| format!("agent-design input: {e}"))?;
    let outcome = (deps.draft)(input.clone()).await;
    let failed = outcome.as_ref().err().cloned();
    (deps.land)(Landing {
        user_id: input.user_id.clone(),
        run_id: ctx.run.id.clone(),
        outcome,
    })
    .await?;
    // A design the muse could not make is a failed run as well as an error
    // on the record: the run's own row tells the same story the review does.
    match failed {
        Some(e) => Err(e),
        None => Ok(StepResult::Done {
            result: serde_json::json!({ "designId": input.design_id }),
        }),
    }
}

/// Installed by the boot wiring (talaria-jobs), which owns the AppState the
/// real edges need; an unarmed step is the loud refusal below.
static ARMED_DEPS: OnceLock<AgentDesignDeps> = OnceLock::new();

pub fn arm_agent_design_step(deps: AgentDesignDeps) {
    let _ = ARMED_DEPS.set(deps);
}

/// The registered definition, exactly once per process.
pub fn agent_design_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: AGENT_DESIGN_KIND.into(),
            label: "Design an agent".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_DEPS.get().cloned() else {
                        return Err(
                            "agent-design steps are armed by the scheduler's boot wiring; this \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    agent_design_step(ctx, &deps).await
                })
            }),
            // The owner, and only the owner.
            audience: Arc::new(audience),
            // One muse call. The ceiling clears the gateway's worst case, not
            // the typical design (20–30 s; the slowest on record ran 2m20s),
            // and it is also the lease TTL: a driver killed mid-design is
            // reclaimable about five minutes later.
            max_step_ms: 300_000,
            idle_step_ms: None,
            max_attempts: talaria_runs_define::DEFAULT_MAX_ATTEMPTS,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use talaria_runs_define::{RunRow, RunState, StepActivity, StepSignal};

    fn ctx(input: &AgentDesignInput) -> RunStepContext {
        let (tx, signal) = StepSignal::channel();
        drop(tx);
        RunStepContext {
            run: RunRow {
                id: "r-1".into(),
                kind: AGENT_DESIGN_KIND.into(),
                owner_user_id: Some(input.user_id.clone()),
                subject_type: Some(AGENT_DESIGN_KIND.into()),
                subject_id: Some(input.design_id.clone()),
                state: RunState::Running,
                phase: "queued".into(),
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
            },
            input: serde_json::to_value(input).unwrap(),
            checkpoint: serde_json::Value::Null,
            decision: None,
            signal,
            log: Arc::new(|_| {}),
            attempt: 0,
            activity: StepActivity::new(),
        }
    }

    fn input() -> AgentDesignInput {
        AgentDesignInput {
            user_id: "u-1".into(),
            design_id: "d-1".into(),
            caller: "platform:muse:ada@example.com".into(),
            instruction: "a release manager".into(),
            current: None,
            chat: Vec::new(),
        }
    }

    fn deps(
        answer: Result<DraftFields, String>,
        landed: Arc<Mutex<Vec<Landing>>>,
    ) -> AgentDesignDeps {
        AgentDesignDeps {
            draft: Arc::new(move |_| {
                let a = answer.clone();
                Box::pin(async move { a })
            }),
            land: Arc::new(move |l| {
                landed.lock().unwrap().push(l);
                Box::pin(async { Ok(()) })
            }),
        }
    }

    #[tokio::test]
    async fn the_step_lands_the_answer_for_its_own_run() {
        let landed = Arc::new(Mutex::new(Vec::new()));
        let d = deps(Ok(DraftFields::default()), landed.clone());
        let out = agent_design_step(ctx(&input()), &d).await.unwrap();
        assert!(matches!(out, StepResult::Done { .. }));
        let l = landed.lock().unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].run_id, "r-1");
        assert_eq!(l[0].user_id, "u-1");
        assert!(l[0].outcome.is_ok());
    }

    #[tokio::test]
    async fn a_failed_draft_lands_its_sentence_and_fails_the_run() {
        let landed = Arc::new(Mutex::new(Vec::new()));
        let d = deps(Err("Muse could not design an agent".into()), landed.clone());
        let err = agent_design_step(ctx(&input()), &d).await.unwrap_err();
        assert_eq!(err, "Muse could not design an agent");
        assert_eq!(
            landed.lock().unwrap()[0].outcome.as_ref().unwrap_err(),
            "Muse could not design an agent"
        );
    }

    #[test]
    fn the_kind_registers_once() {
        assert!(Arc::ptr_eq(agent_design_run(), agent_design_run()));
        assert_eq!(agent_design_run().kind, AGENT_DESIGN_KIND);
    }
}
