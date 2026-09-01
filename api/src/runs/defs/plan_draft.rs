// THE PLAN-DRAFT RUN — "draft tickets from this conversation" as durable
// work, on the same runtime research and the workbench already run on.
// Port of ui/src/server/runs/defs/plan-draft.ts.
//
// WHY A RUN AT ALL, when the old route did this synchronously in one POST: a
// draft is an agent reading a conversation for tens of seconds, and the human
// asked to be able to close the tab. A synchronous POST is a promise to stay
// on the line; a run is a row. The row survives the closed browser, the
// reloaded tab, and a server restart mid-draft (the reclaim sweep re-enters
// this step from the last checkpoint — there is none, so it simply runs
// again), and the review finds its way back by asking for the conversation's
// latest draft.
//
// ONE STEP, NO CHECKPOINTS. There is nothing to resume INTO: the step is one
// model call followed by one row write, and the checkpoint a caller would
// persist between them would be the model's reply itself — see the comment at
// the call.

use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::channel_plan::{
    DraftTemplateCtx, PlanOutcome, plan_from_channel, plan_from_conversation,
};
use crate::harness::defs::channel_plan::{Effort, Priority, TicketProposal};
use crate::runs::define::{
    Authority, RunDefinition, RunRow, RunStepContext, StepResult, register_run,
};
use crate::state::AppState;

pub const PLAN_DRAFT_KIND: &str = "plan-draft";

/// Everything the driver hands the step, camelCase as the row's input column
/// holds it (plan-draft.ts PlanDraftInput).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDraftInput {
    pub conversation_id: String,
    /// Which transcript the agent reads — the two planners gather differently
    /// (channel messages vs a conversation with its living document).
    pub source: String, // 'plan' | 'channel'
    pub agent_model: String,
    pub routed_model: String,
    #[serde(default)]
    pub board_id: Option<String>,
    #[serde(default)]
    pub template_id: Option<String>,
}

/// What lands in `plan_drafts.proposals` — the reviewed shape, normalized
/// once here so the row never holds a half-shaped batch. The review walk
/// PATCHes this same shape back, which is why `include` starts life explicit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProposal {
    pub title: String,
    pub description: String,
    pub priority: Priority,
    /// `xs`–`xl`, or null when the model did not hazard one.
    pub effort: Option<Effort>,
    #[serde(default)]
    pub depends_on: Vec<usize>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub include: bool,
}

/// TicketProposal's arrays are required by type, but this is model output
/// arriving as JSON — `to_proposals` already answers a missing array with the
/// empty one (the same `??` the synchronous route defended with), so the
/// normalize left to do is the one field the reviewed shape adds.
fn normalize(p: TicketProposal) -> StoredProposal {
    StoredProposal {
        title: p.title,
        description: p.description,
        priority: p.priority,
        effort: p.effort,
        depends_on: p.depends_on,
        tags: p.tags,
        include: true,
    }
}

/// The step's two edges, each overridable, so tests drive a draft with no
/// database and no model.
pub type DraftTicketsFn =
    Arc<dyn Fn(&PlanDraftInput) -> BoxFuture<'static, Result<PlanOutcome, String>> + Send + Sync>;
pub type SaveResultFn = Arc<
    dyn Fn(&str, Vec<StoredProposal>, Option<String>) -> BoxFuture<'static, Result<(), String>>
        + Send
        + Sync,
>;

#[derive(Clone)]
pub struct PlanDraftDeps {
    pub draft_tickets: DraftTicketsFn,
    pub save_result: SaveResultFn,
}

/// The real edges. `state` carries the pool the save writes through and the
/// harness assembly the draft runs over.
pub fn real_plan_draft_deps(state: AppState) -> PlanDraftDeps {
    let state_for_draft = state.clone();
    let state_for_save = state;
    PlanDraftDeps {
        draft_tickets: Arc::new(move |input: &PlanDraftInput| {
            let state = state_for_draft.clone();
            let input = input.clone();
            Box::pin(async move {
                let tpl = DraftTemplateCtx {
                    board_id: input.board_id.as_deref(),
                    template_id: input.template_id.as_deref(),
                };
                if input.source == "channel" {
                    plan_from_channel(
                        &state,
                        &input.conversation_id,
                        &input.agent_model,
                        &input.routed_model,
                        &tpl,
                    )
                    .await
                } else {
                    plan_from_conversation(
                        &state,
                        &input.conversation_id,
                        &input.agent_model,
                        &input.routed_model,
                        &tpl,
                    )
                    .await
                }
            })
        }),
        save_result: Arc::new(
            move |id: &str, proposals: Vec<StoredProposal>, note: Option<String>| {
                let pg = state_for_save.pg.clone();
                let id = id.to_string();
                Box::pin(async move {
                    let json = serde_json::to_value(&proposals).map_err(|e| e.to_string())?;
                    sqlx::query(
                    "update plan_drafts set proposals = $1::jsonb, note = $2, updated_at = now() \
                     where id = $3::uuid",
                )
                .bind(json)
                .bind(note)
                .bind(id)
                .execute(&pg)
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
                })
            },
        ),
    }
}

/// The drafter's own run: who may watch it and be told about it is the person
/// who clicked Draft tickets. (Same authority shape as research.)
fn audience(run: &RunRow) -> Authority {
    match &run.owner_user_id {
        Some(owner) => Authority::User {
            user_ids: vec![owner.clone()],
        },
        None => Authority::Admin { on_board: None },
    }
}

async fn plan_draft_step(ctx: RunStepContext, deps: &PlanDraftDeps) -> Result<StepResult, String> {
    let input: PlanDraftInput =
        serde_json::from_value(ctx.input.clone()).map_err(|e| format!("plan-draft input: {e}"))?;
    // AT-LEAST-ONCE (the checklist in define.rs): the model call bills on
    // re-entry. A driver that dies between the call and the row write below
    // will make the call once more on reclaim. Accepted deliberately: the
    // output is a draft the human regenerates at will, the repeat window is
    // one database write, and the alternative — checkpointing the model's
    // reply before persisting it — would still bill twice on every real
    // failure while saving only the rare reclaim.
    let out = (deps.draft_tickets)(&input).await?;
    // `raw` is for exactly one distinction, the same one the synchronous
    // route made: the agent ANSWERED but not in tickets, vs nothing to plan.
    let note = if out.proposals.is_empty() {
        Some(if out.raw.is_empty() {
            "nothing to plan yet"
        } else {
            "the agent did not return parseable tickets"
        })
    } else {
        None
    };
    let stored: Vec<StoredProposal> = out.proposals.into_iter().map(normalize).collect();
    let count = stored.len();
    (deps.save_result)(&ctx.run.id, stored, note.map(str::to_string)).await?;
    Ok(StepResult::Done {
        result: serde_json::json!({ "count": count }),
    })
}

/// The real step deps, armed with the scheduler handover: the Rust driver
/// does not exist until the flip arms it, so the deps sit empty until the
/// boot wiring (which owns the AppState) installs them. An unarmed step is
/// the loud refusal below — reached only by a driver armed before its deps,
/// which is exactly what the sentence says.
static ARMED_DEPS: OnceLock<PlanDraftDeps> = OnceLock::new();

pub fn arm_plan_draft_step(deps: PlanDraftDeps) {
    let _ = ARMED_DEPS.set(deps);
}

/// The registered definition, exactly once per process — TS registers at
/// module load; the Rust equivalent is the first call, which `dispatch` makes
/// before any enqueue, so the row's kind is always registered before it is
/// written. The returned `&'static Arc` is the same one the registry holds.
pub fn plan_draft_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: PLAN_DRAFT_KIND.into(),
            label: "Draft tickets".into(),
            step: Arc::new(|ctx| {
                Box::pin(async move {
                    let Some(deps) = ARMED_DEPS.get().cloned() else {
                        return Err(
                            "plan-draft steps are armed with the scheduler handover; this Rust \
                             step was reached by a driver armed before its deps were"
                                .into(),
                        );
                    };
                    plan_draft_step(ctx, &deps).await
                })
            }),
            audience: Arc::new(audience),
            // One agent call over a conversation. The ceiling has to clear
            // the transport's worst case (the gateway's own timeout is ten
            // minutes), not the typical draft — a step that blows this is
            // FILED AS AN ERROR, not retried, because it is probably still
            // running. The price is the lease TTL: a driver killed mid-draft
            // is reclaimable about five minutes later.
            max_step_ms: 300_000,
            // TS sets no override — the default three. A draft that killed
            // three drivers is a bug report, not a fourth try.
            max_attempts: crate::runs::define::DEFAULT_MAX_ATTEMPTS,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn minimal_row() -> RunRow {
        use crate::runs::define::RunState;
        RunRow {
            id: "r-1".into(),
            kind: PLAN_DRAFT_KIND.into(),
            owner_user_id: None,
            subject_type: Some("plan-draft".into()),
            subject_id: Some("d-1".into()),
            state: RunState::Queued,
            phase: "queued".into(),
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

    fn ctx(input: serde_json::Value) -> RunStepContext {
        let (tx, signal) = crate::runs::define::StepSignal::channel();
        // A dropped watch sender leaves the channel at its last value —
        // `false`, never aborted — which is the shape an uncontended run has.
        drop(tx);
        RunStepContext {
            run: minimal_row(),
            input,
            checkpoint: serde_json::Value::Null,
            decision: None,
            signal,
            log: Arc::new(|_| {}),
            attempt: 0,
        }
    }

    fn ticket(title: &str) -> TicketProposal {
        TicketProposal {
            title: title.into(),
            description: "the work".into(),
            priority: Priority::Medium,
            effort: None,
            depends_on: Vec::new(),
            tags: Vec::new(),
        }
    }

    /// Where recording deps land their save: the batch and the note, once.
    type Saved = Arc<Mutex<Option<(Vec<StoredProposal>, Option<String>)>>>;

    /// Recording deps: the draft is a fixed reply, the save lands in a mutex.
    /// Nothing here touches a database — the SQL edge is one bound query whose
    /// shape the real deps state once.
    fn recording_deps(proposals: Vec<TicketProposal>, raw: &str) -> (PlanDraftDeps, Saved) {
        let saved: Saved = Arc::new(Mutex::new(None));
        let saved_for_write = saved.clone();
        let raw = raw.to_string();
        let deps = PlanDraftDeps {
            draft_tickets: Arc::new(move |_input| {
                let proposals = proposals.clone();
                let raw = raw.clone();
                Box::pin(async move { Ok(PlanOutcome { proposals, raw }) })
            }),
            save_result: Arc::new(move |id, proposals, note| {
                let saved_for_write = saved_for_write.clone();
                let id = id.to_string();
                Box::pin(async move {
                    assert_eq!(id, "r-1");
                    *saved_for_write.lock().unwrap() = Some((proposals, note));
                    Ok(())
                })
            }),
        };
        (deps, saved)
    }

    fn input_json() -> serde_json::Value {
        serde_json::json!({
            "conversationId": "c-1",
            "source": "plan",
            "agentModel": "engineer",
            "routedModel": "engineer-opus",
            "boardId": null,
            "templateId": null,
        })
    }

    #[test]
    fn registration_carries_the_real_metadata_once() {
        let def = plan_draft_run();
        assert_eq!(def.kind, PLAN_DRAFT_KIND);
        assert_eq!(def.label, "Draft tickets");
        assert_eq!(def.max_step_ms, 300_000);
        assert_eq!(def.max_attempts, 3);
        // The same Arc every time — register_run is once per process, and a
        // second registration would be the bug define.rs refuses.
        assert!(Arc::ptr_eq(def, plan_draft_run()));
    }

    #[test]
    fn the_audience_is_the_clicker_else_every_admin() {
        let mut row = minimal_row();
        assert_eq!(audience(&row), Authority::Admin { on_board: None });
        row.owner_user_id = Some("u-1".into());
        assert_eq!(
            audience(&row),
            Authority::User {
                user_ids: vec!["u-1".into()]
            }
        );
    }

    #[tokio::test]
    async fn an_unarmed_step_refuses_loudly() {
        // No test arms ARMED_DEPS, so the registered step is unarmed for the
        // whole binary — the refusal below is the only thing it can say.
        let res = (plan_draft_run().step.clone())(ctx(input_json())).await;
        let err = res.unwrap_err();
        assert!(err.contains("armed"), "{err}");
        assert!(err.contains(PLAN_DRAFT_KIND), "{err}");
    }

    #[tokio::test]
    async fn a_draft_normalizes_saves_and_counts() {
        let (deps, saved) = recording_deps(vec![ticket("First"), ticket("Second")], "…");
        let res = plan_draft_step(ctx(input_json()), &deps).await.unwrap();
        match res {
            StepResult::Done { result } => assert_eq!(result, serde_json::json!({ "count": 2 })),
            other => panic!("expected done, got {other:?}"),
        }
        let (proposals, note) = saved.lock().unwrap().clone().unwrap();
        assert_eq!(proposals.len(), 2);
        // Normalized once: include starts explicit, arrays are arrays.
        assert!(proposals.iter().all(|p| p.include && p.tags.is_empty()));
        assert_eq!(note, None);
    }

    #[tokio::test]
    async fn the_note_distinguishes_unparseable_from_empty() {
        // The agent ANSWERED (raw) but no tickets came out.
        let (deps, saved) = recording_deps(vec![], "here is why not, at length");
        plan_draft_step(ctx(input_json()), &deps).await.unwrap();
        assert_eq!(
            saved.lock().unwrap().clone().unwrap().1.as_deref(),
            Some("the agent did not return parseable tickets")
        );
        // Nothing arrived at all: a quiet conversation, not a bad one.
        let (deps, saved) = recording_deps(vec![], "");
        plan_draft_step(ctx(input_json()), &deps).await.unwrap();
        assert_eq!(
            saved.lock().unwrap().clone().unwrap().1.as_deref(),
            Some("nothing to plan yet")
        );
    }

    #[test]
    fn normalize_adds_the_reviewed_fields_and_keeps_the_rest() {
        let mut p = ticket("T");
        p.tags = vec!["billing".into()];
        p.depends_on = vec![0];
        let s = normalize(p);
        assert_eq!(s.title, "T");
        assert_eq!(s.tags, vec!["billing"]);
        assert_eq!(s.depends_on, vec![0]);
        assert!(s.include);
        // And round-trips: the row column is this exact shape, camelCase.
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["dependsOn"], serde_json::json!([0]));
        assert_eq!(json["include"], true);
    }

    #[tokio::test]
    async fn a_failed_draft_fails_the_step_without_saving() {
        let saved: Saved = Arc::new(Mutex::new(None));
        let saved_for_write = saved.clone();
        let deps = PlanDraftDeps {
            draft_tickets: Arc::new(|_input| Box::pin(async { Err("gateway error 502".into()) })),
            save_result: Arc::new(move |_id, _p, _n| {
                let saved_for_write = saved_for_write.clone();
                Box::pin(async move {
                    *saved_for_write.lock().unwrap() = None;
                    Ok(())
                })
            }),
        };
        let err = plan_draft_step(ctx(input_json()), &deps).await.unwrap_err();
        assert_eq!(err, "gateway error 502");
        // The step died before the save: the draft row keeps whatever it had
        // (in the real deps, proposals from a previous successful draft), and
        // the run's error row carries the sentence.
        assert!(saved.lock().unwrap().is_none());
    }
}
