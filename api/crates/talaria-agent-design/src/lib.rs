// THE AGENT DESIGN — "describe it" in New agent, owned by the server.
//
// The owner's rule (2026-11-04): no run is controlled by browser state; its
// state lives in Redis and survives refreshes. Creating agents is one of the
// most important parts of the platform, so the design is durable:
//
//   WHERE IT LIVES. One record per person in Redis (store.rs), with a 72-hour
//   TTL: the instruction and chat, the validated draft, the status
//   (generating / ready / error), the error, and the start and finish times.
//   Any api process reads and writes it; nothing about a design is held in
//   process memory, and a write is a compare-and-set.
//
//   HOW IT RUNS. A describe or a refine writes the record, then enqueues an
//   `agent-design` run (run.rs) on the durable runs runtime. The run makes
//   the muse call and writes the answer into the record. It owes nothing to
//   the request that started it: the tab can close, the edge can cut the
//   connection, and a process that dies mid-design is reclaimed by the sweep.
//
//   HOW THE PERSON FINDS IT AGAIN. The modal reads the record on open (GET),
//   so a reload, a second tab or another device resumes exactly where it
//   stood. A design that finishes while no modal is watching it files a bell
//   notification that links back to the review step (`REVIEW_HREF`).
//
//   ONE AT A TIME. A describe while one generates returns the one generating;
//   a describe over an idle design replaces it. Every edit and refine names
//   the design it edits, so a stale tab is refused (409), never written over
//   a newer one. Discarding removes it and reads its notification.
//
//   THE HIRE IS A GATE. The hire route claims the design before it enqueues
//   (`claim_for_hire`): a second hire from it is refused. The design is
//   consumed when that hire finishes and handed back, with the hire's
//   sentence, when it fails — so a hire that needs an edit (a department
//   taken in a race) never loses it. Both are settled on read.
//
// The transitions are pure (record.rs); this file is the service the routes
// call and the real edges the run is armed with.

pub mod record;
pub mod run;
pub mod store;

#[cfg(test)]
mod design_live;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use redis::aio::ConnectionManager;
use talaria_harness::run::{HarnessError, RunContext, run_harness};
use talaria_harness_defs::defs::muse::{MuseDraftInput, muse_agent_harness};
use talaria_harness_model::{MUSE_CHAIN, ModelSpec, resolve_harness_model};
use talaria_notify::{
    NotificationInput, NotifyDeps, NotifyEntity, NotifyMeta, add_notification_with, clear_entity,
    entity, group_key,
};
use talaria_org::{org_line, org_profile};
use talaria_realtime::RealtimeDeps;
use talaria_runs_run::{EnqueueOptions, cancel_run, enqueue};
use talaria_state::AppState;
use talaria_tasks_types::BUILD_DISPATCH;
use uuid::Uuid;

pub use record::{AgentDesign, DesignStatus, DraftFields, Refusal, SkillDraft};
use record::{HireSeen, RunSeen, Settled, Start};
pub use run::{
    AGENT_DESIGN_KIND, AgentDesignDeps, AgentDesignInput, Landing, agent_design_run,
    arm_agent_design_step,
};
use store::Write;

/// Where the bell sends the person: the roster, with the New agent modal
/// open on the design (Agents.svelte reads `design`).
pub const REVIEW_HREF: &str = "/agents?design=open";

/// "Nothing routes": a configuration problem, for the admin. The muse route
/// answers the same sentence for every kind.
pub const NO_MODEL: &str = "no routable model found — add an endpoint with models on /models first";

/// The model answered, but not with an agent: a sentence pointed at the next
/// thing to try. The technical reason is on the harness_runs row.
pub const UNUSABLE_AGENT: &str =
    "Muse could not design an agent from that — try adding a sentence about what it should do.";

/// The design could not run at all (a database or harness fault, logged).
const DESIGN_FAULT: &str = "The design could not run. Try again in a moment.";

/// The run could not be enqueued (logged).
const START_FAILED: &str = "The design could not start. Try again in a moment.";

/// Why a design operation did not happen.
#[derive(Debug)]
pub enum DesignError {
    /// The record refuses the transition — a 409 with its sentence.
    Refused(Refusal),
    /// Redis, Postgres or the run machinery failed — a logged 500.
    Fault(String),
}

impl From<redis::RedisError> for DesignError {
    fn from(e: redis::RedisError) -> Self {
        DesignError::Fault(format!("redis: {e}"))
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn redis(state: &AppState) -> Result<ConnectionManager, DesignError> {
    state
        .redis()
        .await
        .map_err(|e| DesignError::Fault(format!("redis is unavailable: {e}")))
}

/// The person's design, if any. `watching` is the open modal saying it is
/// looking, so a landing in the next few seconds skips the bell. A record
/// still generating whose run can no longer answer is turned into its error
/// here, so nobody watches a spinner forever.
pub async fn current_design(
    state: &AppState,
    user_id: &str,
    watching: bool,
) -> Result<Option<AgentDesign>, DesignError> {
    let mut conn = redis(state).await?;
    if watching && let Err(e) = store::mark_watching(&mut conn, user_id).await {
        tracing::warn!("[agent-design] watch mark failed: {e}");
    }
    let Some(stored) = store::read(&mut conn, user_id).await? else {
        return Ok(None);
    };
    if let Some(hire) = stored.design.hire_run_id.clone() {
        return settle_hire(state, &mut conn, user_id, &hire).await;
    }
    if stored.design.status != DesignStatus::Generating {
        return Ok(Some(stored.design));
    }
    let seen = run_seen(state, stored.design.run_id.as_deref()).await;
    let now = now_ms();
    if record::stalled(&stored.design, seen, now).is_none() {
        return Ok(Some(stored.design));
    }
    let out = store::update(&mut conn, user_id, |d| {
        match d.and_then(|d| record::stalled(d, seen, now)) {
            Some(next) => Write::put(next.clone(), Some(next)),
            None => Write::Keep(d.cloned()),
        }
    })
    .await?;
    Ok(out)
}

/// A run row's state and error. Err is a failed read.
async fn run_row(
    state: &AppState,
    run_id: &str,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    sqlx::query_as("select state, error from runs where id = $1::uuid")
        .bind(run_id)
        .fetch_optional(&state.pg)
        .await
}

fn live(state: &str) -> bool {
    matches!(state, "queued" | "running" | "awaiting")
}

/// Where the run behind a generating record stands. A read that fails says
/// Live: a database blip must not be the thing that declares a design dead.
async fn run_seen(state: &AppState, run_id: Option<&str>) -> RunSeen {
    let Some(run_id) = run_id else {
        return RunSeen::Missing;
    };
    match run_row(state, run_id).await {
        Ok(None) => RunSeen::Missing,
        Ok(Some((s, _))) if live(&s) => RunSeen::Live,
        Ok(Some(_)) => RunSeen::Ended,
        Err(e) => {
            tracing::warn!("[agent-design] run state read failed: {e}");
            RunSeen::Live
        }
    }
}

/// Where the hire a design is spent on stands. A failed read says Live, for
/// the same reason.
async fn hire_seen(state: &AppState, hire_run_id: &str) -> HireSeen {
    match run_row(state, hire_run_id).await {
        Ok(None) => HireSeen::Missing,
        Ok(Some((s, _))) if live(&s) => HireSeen::Live,
        Ok(Some((s, _))) if s == "done" => HireSeen::Done,
        Ok(Some((_, e))) => HireSeen::Failed(e),
        Err(e) => {
            tracing::warn!("[agent-design] hire state read failed: {e}");
            HireSeen::Live
        }
    }
}

/// A design spent on a hire, as a read finds it: still spent (the hire runs;
/// the modal treats it as gone), consumed (the hire finished), or handed
/// back with the hire's sentence (it failed, and needs an edit or a retry).
async fn settle_hire(
    state: &AppState,
    conn: &mut ConnectionManager,
    user_id: &str,
    hire_run_id: &str,
) -> Result<Option<AgentDesign>, DesignError> {
    let seen = hire_seen(state, hire_run_id).await;
    let now = now_ms();
    let (out, consumed) = store::update(conn, user_id, |d| match d {
        Some(d) if d.hire_run_id.as_deref() == Some(hire_run_id) => {
            match record::settle_hire(d, &seen, now) {
                Settled::Unchanged => Write::Keep((Some(d.clone()), None)),
                Settled::Consumed => Write::Delete((None, Some(d.id.clone()))),
                Settled::Released(back) => Write::put((*back).clone(), (Some(*back), None)),
            }
        }
        other => Write::Keep((other.cloned(), None)),
    })
    .await?;
    if let Some(id) = consumed {
        clear_notice(state, conn, user_id, &id).await;
    }
    Ok(out)
}

/// The hire route's gate: spend the design `design_id` on the hire
/// `hire_run_id` before that hire is enqueued. Refused (409) when the design
/// is not the person's current one, still generating, or already spent on a
/// hire that may still finish — so two tabs cannot make two agents from one
/// design. Its bell row is read: the person acted on it.
pub async fn claim_for_hire(
    state: &AppState,
    user_id: &str,
    design_id: &str,
    hire_run_id: &str,
) -> Result<(), DesignError> {
    let mut conn = redis(state).await?;
    // An earlier claim may be taken over only when its hire is seen ended.
    let held = store::read(&mut conn, user_id)
        .await?
        .and_then(|s| s.design.hire_run_id);
    let other = match held {
        Some(h) => {
            let alive = matches!(
                hire_seen(state, &h).await,
                HireSeen::Live | HireSeen::Missing
            );
            Some((h, alive))
        }
        None => None,
    };
    let now = now_ms();
    store::update(&mut conn, user_id, |d| {
        let seen = other.as_ref().map(|(h, alive)| (h.as_str(), *alive));
        match record::claim(d, design_id, hire_run_id, seen, now) {
            Ok(next) => Write::put(next, Ok(())),
            Err(r) => Write::Keep(Err(r)),
        }
    })
    .await?
    .map_err(DesignError::Refused)?;
    clear_notice(state, &conn, user_id, design_id).await;
    Ok(())
}

/// The hire could not be enqueued: hand the design back at once.
pub async fn release_hire(state: &AppState, user_id: &str, hire_run_id: &str) {
    let now = now_ms();
    let released = async {
        let mut conn = redis(state).await?;
        store::update(&mut conn, user_id, |d| match d {
            Some(d) if d.hire_run_id.as_deref() == Some(hire_run_id) => {
                Write::put(record::release(d, None, now), ())
            }
            _ => Write::Keep(()),
        })
        .await
        .map_err(DesignError::from)
    };
    if let Err(e) = released.await {
        tracing::warn!("[agent-design] releasing the design from {hire_run_id} failed: {e:?}");
    }
}

/// A describe: start a design, or follow the one already generating.
pub async fn start_design(
    state: &AppState,
    user_id: &str,
    caller: &str,
    purpose: &str,
) -> Result<AgentDesign, DesignError> {
    let mut conn = redis(state).await?;
    // A record still generating is followed only while its run can answer;
    // one whose run died is replaced at once rather than on the next read.
    let seen = match store::read(&mut conn, user_id).await? {
        Some(s) if s.design.status == DesignStatus::Generating => {
            run_seen(state, s.design.run_id.as_deref()).await
        }
        _ => RunSeen::Live,
    };
    let now = now_ms();
    let (design_id, run_id) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    let (design, fresh, replaced) = store::update(&mut conn, user_id, |d| {
        match record::start(d, purpose, now, (design_id.clone(), run_id.clone()), seen) {
            Start::Existing(e) => Write::Keep((e, false, None)),
            Start::Fresh(f) => Write::put(f.clone(), (f, true, d.map(|d| d.id.clone()))),
        }
    })
    .await?;
    if !fresh {
        return Ok(design);
    }
    // The describe came from an open modal: it is looking until its own
    // poll takes over, so a fast landing does not ring the bell.
    if let Err(e) = store::mark_watching(&mut conn, user_id).await {
        tracing::warn!("[agent-design] watch mark failed: {e}");
    }
    if let Some(old) = replaced {
        clear_notice(state, &conn, user_id, &old).await;
    }
    let input = AgentDesignInput {
        user_id: user_id.to_string(),
        design_id: design.id.clone(),
        caller: caller.to_string(),
        instruction: design.instruction.clone(),
        current: None,
        chat: Vec::new(),
    };
    launch(state, &mut conn, &design, input).await?;
    Ok(design)
}

/// A refine of the current draft. `current` is the form as the person sees
/// it, hand edits included.
pub async fn refine_design(
    state: &AppState,
    user_id: &str,
    caller: &str,
    id: Option<&str>,
    instruction: &str,
    current: Option<DraftFields>,
) -> Result<AgentDesign, DesignError> {
    let mut conn = redis(state).await?;
    let now = now_ms();
    let run_id = Uuid::new_v4().to_string();
    let refined = store::update(&mut conn, user_id, |d| {
        match record::refine(d, id, instruction, current.clone(), now, run_id.clone()) {
            Ok(next) => Write::put(next.clone(), Ok(next)),
            Err(r) => Write::Keep(Err(r)),
        }
    })
    .await?
    .map_err(DesignError::Refused)?;
    if let Err(e) = store::mark_watching(&mut conn, user_id).await {
        tracing::warn!("[agent-design] watch mark failed: {e}");
    }
    let input = AgentDesignInput {
        user_id: user_id.to_string(),
        design_id: refined.id.clone(),
        caller: caller.to_string(),
        instruction: refined.instruction.clone(),
        // The form the model revises, as the modal always sent it.
        current: refined
            .draft
            .as_ref()
            .map(|d| serde_json::to_string(d).expect("a draft serializes")),
        chat: refined.chat.clone(),
    };
    launch(state, &mut conn, &refined, input).await?;
    Ok(refined)
}

/// Enqueue the run the record now names. The record goes in first because
/// the run's only write is to it; a run that cannot be enqueued turns the
/// record into its error rather than leaving it generating.
async fn launch(
    state: &AppState,
    conn: &mut ConnectionManager,
    design: &AgentDesign,
    input: AgentDesignInput,
) -> Result<(), DesignError> {
    let user_id = input.user_id.clone();
    let run_id = design.run_id.clone().unwrap_or_default();
    let started = match BUILD_DISPATCH.get() {
        None => Err("the run dispatch is not wired".to_string()),
        Some(build) => {
            let deps = build(
                state.pg.clone(),
                conn.clone(),
                RealtimeDeps::publish_only(Some(conn.clone())),
            );
            enqueue(
                agent_design_run(),
                serde_json::to_value(&input).expect("the design input serializes"),
                EnqueueOptions {
                    id: Some(run_id.clone()),
                    owner_user_id: Some(user_id.clone()),
                    subject_type: Some(AGENT_DESIGN_KIND.into()),
                    subject_id: Some(design.id.clone()),
                    phase: Some("queued".into()),
                    // Row + publish + drive, detached from this request; the
                    // reclaim sweep is the guarantee either way.
                    start: Some(true),
                },
                &deps,
            )
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
        }
    };
    let Err(e) = started else {
        return Ok(());
    };
    tracing::error!("[agent-design] enqueue failed: {e}");
    let now = now_ms();
    store::update(conn, &user_id, |d| {
        match d.and_then(|d| record::fail(d, &run_id, START_FAILED, now)) {
            Some(next) => Write::put(next, ()),
            None => Write::Keep(()),
        }
    })
    .await?;
    Err(DesignError::Fault(format!("enqueue: {e}")))
}

/// The person's edits on the review step, a role template, or a manual
/// start: the draft is theirs. Refused while a run is writing it.
pub async fn edit_design(
    state: &AppState,
    user_id: &str,
    want: Option<&str>,
    fields: DraftFields,
) -> Result<AgentDesign, DesignError> {
    let mut conn = redis(state).await?;
    let now = now_ms();
    let id = Uuid::new_v4().to_string();
    store::update(&mut conn, user_id, |d| {
        match record::edit(d, want, fields.clone(), now, id.clone()) {
            Ok(next) => Write::put(next.clone(), Ok(next)),
            Err(r) => Write::Keep(Err(r)),
        }
    })
    .await?
    .map_err(DesignError::Refused)
}

/// Remove the person's design — `id`, when given, must be the one they hold.
/// A design still generating has its run cancelled; its answer, if it comes,
/// finds no record and is dropped. Answers whether anything was removed.
pub async fn discard_design(
    state: &AppState,
    user_id: &str,
    id: Option<&str>,
) -> Result<bool, DesignError> {
    let mut conn = redis(state).await?;
    let removed = store::update(&mut conn, user_id, |d| match d {
        Some(d) if id.is_none_or(|want| want == d.id) => Write::Delete(Some(d.clone())),
        _ => Write::Keep(None),
    })
    .await?;
    let Some(gone) = removed else {
        return Ok(false);
    };
    if gone.status == DesignStatus::Generating
        && let (Some(run_id), Some(build)) = (gone.run_id.as_deref(), BUILD_DISPATCH.get())
    {
        let deps = build(
            state.pg.clone(),
            conn.clone(),
            RealtimeDeps::publish_only(Some(conn.clone())),
        );
        if let Err(e) = cancel_run(run_id, Some("design discarded".into()), &deps).await {
            tracing::warn!("[agent-design] cancel of {run_id} failed: {e}");
        }
    }
    clear_notice(state, &conn, user_id, &gone.id).await;
    Ok(true)
}

/// Read the person's bell row about a design that is spent or replaced.
async fn clear_notice(state: &AppState, conn: &ConnectionManager, user_id: &str, design_id: &str) {
    let deps = NotifyDeps::publishing(state.pg.clone(), Some(conn.clone()));
    let about = NotifyEntity::new(entity::AGENT_DESIGN, design_id);
    if let Err(e) = clear_entity(&deps, user_id, &about).await {
        tracing::warn!("[agent-design] clearing the notice for {design_id} failed: {e}");
    }
}

// ── The real edges the run is armed with ─────────────────────────────────────

/// The step's real deps over the AppState: the muse `agent` harness, and the
/// record write plus the bell.
pub fn real_agent_design_deps(state: AppState) -> AgentDesignDeps {
    let for_draft = state.clone();
    AgentDesignDeps {
        draft: Arc::new(move |input| {
            let st = for_draft.clone();
            Box::pin(async move { muse_agent_draft(&st, input).await })
        }),
        land: Arc::new(move |landing| {
            let st = state.clone();
            Box::pin(async move { land_answer(&st, landing).await })
        }),
    }
}

/// One muse `agent` draft — the structured half of the muse route, for one
/// kind, answered in the sentences that route uses.
async fn muse_agent_draft(
    state: &AppState,
    input: AgentDesignInput,
) -> Result<DraftFields, String> {
    // `user_id` arms the member model allowlist inside the chain: a design
    // without it would hand a member the expensive model an admin gated.
    let spec = ModelSpec {
        pin: Some("muse"),
        slot: None,
        role: None,
        chain: Some(&MUSE_CHAIN),
        user_id: Some(&input.user_id),
    };
    let resolved = resolve_harness_model(&state.pg, &spec).await.map_err(|e| {
        tracing::error!("[agent-design] model resolution failed: {e}");
        DESIGN_FAULT.to_string()
    })?;
    let Some(resolved) = resolved else {
        return Err(NO_MODEL.to_string());
    };
    let value = serde_json::to_value(MuseDraftInput {
        instruction: input.instruction,
        current: input.current,
        context: None,
        chat: Some(input.chat),
        org: org_line(&org_profile(&state.pg).await),
    })
    .expect("the draft input serializes");
    let ctx = RunContext {
        caller: input.caller,
        user_id: Some(input.user_id),
        model: Some(resolved.model),
        step: Some(resolved.step),
        ..Default::default()
    };
    match run_harness(state, &muse_agent_harness(), &value, ctx).await {
        Ok(r) => match r.value {
            Some(v) => serde_json::from_value::<DraftFields>(v).map_err(|e| {
                tracing::warn!("[agent-design] the validated draft did not read: {e}");
                UNUSABLE_AGENT.to_string()
            }),
            None if r.model.is_none() => Err(r.error.unwrap_or_else(|| NO_MODEL.to_string())),
            None => {
                tracing::warn!("[agent-design] unusable draft: {:?}", r.error);
                Err(UNUSABLE_AGENT.to_string())
            }
        },
        Err(HarnessError(e)) => {
            tracing::error!("[agent-design] muse run failed: {e}");
            Err(DESIGN_FAULT.to_string())
        }
    }
}

/// Write the answer into the record that asked for it, then tell the person
/// unless they are looking at it.
async fn land_answer(state: &AppState, landing: Landing) -> Result<(), String> {
    let mut conn = state.redis().await.map_err(|e| format!("redis: {e}"))?;
    let now = now_ms();
    let landed = store::update(&mut conn, &landing.user_id, |d| {
        let next = d.and_then(|d| match &landing.outcome {
            Ok(draft) => record::land(d, &landing.run_id, draft.clone(), now),
            Err(e) => record::fail(d, &landing.run_id, e, now),
        });
        match next {
            Some(n) => Write::put(n.clone(), Some(n)),
            None => Write::Keep(None),
        }
    })
    .await
    .map_err(|e| format!("redis: {e}"))?;
    // Replaced or discarded meanwhile: nobody is waiting for this answer.
    let Some(design) = landed else {
        return Ok(());
    };
    let watching = store::watched(&mut conn, &landing.user_id)
        .await
        .unwrap_or(false);
    if !watching {
        file_notice(state, conn, &landing.user_id, &design).await;
    }
    Ok(())
}

/// The durable "it finished": a bell row about the design, one per design
/// (a refine that lands re-alerts it), linking back to the review step.
async fn file_notice(state: &AppState, conn: ConnectionManager, user_id: &str, d: &AgentDesign) {
    let (title, body) = match d.status {
        DesignStatus::Ready => {
            let name = d.draft.as_ref().map(|f| f.name.trim()).unwrap_or("");
            let name = if name.is_empty() {
                "the new agent"
            } else {
                name
            };
            (
                format!("Agent design ready: {}", clip(name, 120)),
                clip(&d.purpose, 240),
            )
        }
        DesignStatus::Error => (
            "The agent design failed".to_string(),
            d.error.clone().unwrap_or_default(),
        ),
        DesignStatus::Generating => return,
    };
    let deps = NotifyDeps::publishing(state.pg.clone(), Some(conn));
    let filed = add_notification_with(
        &deps,
        user_id,
        &NotificationInput {
            kind: "agent-design",
            title: &title,
            body: (!body.is_empty()).then_some(body.as_str()),
            href: Some(REVIEW_HREF),
        },
        &NotifyMeta {
            group_key: Some(group_key::agent_design(&d.id)),
            entity: Some(NotifyEntity::new(entity::AGENT_DESIGN, d.id.clone())),
            ..Default::default()
        },
    )
    .await;
    if let Err(e) = filed {
        tracing::error!("[agent-design] the notice for {} was not filed: {e}", d.id);
    }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}
