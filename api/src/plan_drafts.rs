// PLAN DRAFTS — the domain half of the durable ticket-draft job. The run row
// (kind 'plan-draft', same uuid) owns the state machine; this module owns the
// row the review reads: starting one, finding the conversation's latest,
// saving the walk's edits back, dropping a consumed or discarded batch.
// Port of ui/src/server/plan-drafts.ts.
//
// ONE DRAFT PER CONVERSATION AT A TIME. The single-flight check is read-then-
// write, so two truly concurrent POSTs can both pass it and enqueue two runs —
// the modal prevents this client-side and the loser simply overwrites the
// conversation's "latest" slot; the row never duplicates a TICKET, because
// nothing is created until the human reviews.
//
// THE KEY IS A CONVERSATION OR A CHANNEL. `plan_drafts.conversation_id` holds
// a plan conversation id on the Plan surface and a CHANNEL id behind the
// channel Plan button — one domain module, two conversation kinds. It was born
// with a foreign key to conversations(id) that the channel half violated on
// every click; the migration dropping that constraint shipped with this port
// (see pg.ts's MIGRATIONS tail).

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::agent_auth::epoch_ms_to_iso;
use crate::realtime::RealtimeDeps;
use crate::runs::defs::plan_draft::{
    PLAN_DRAFT_KIND, PlanDraftInput, StoredProposal, plan_draft_run,
};
use crate::runs::run::{EnqueueOptions, cancel_run, enqueue};
use crate::state::AppState;
use crate::work_dispatch::coexistence_dispatch_deps;

/// The draft as the review reads it (plan-drafts.ts PlanDraft) — the fields
/// in toDraft's literal order, camelCase on the wire.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDraft {
    pub id: String,
    pub conversation_id: String,
    /// 'plan' | 'channel'
    pub source: String,
    pub agent_model: String,
    pub board_id: Option<String>,
    pub template_id: Option<String>,
    pub proposals: Vec<StoredProposal>,
    pub note: Option<String>,
    /// The joined run's state. 'awaiting' cannot happen to this definition (it
    /// never parks on a decision) but a missing run row can — that reads as
    /// 'done' so the proposals stay reviewable rather than vanishing.
    pub status: String, // 'queued' | 'running' | 'done' | 'error' | 'cancelled'
    pub phase: String,
    pub error: Option<String>,
    pub created_at: String,
}

/// The row both reads reduce to (plan-drafts.ts DraftRow + toDraft). `active`
/// picks the join: the latest draft ANY state (the review's way back), or the
/// latest whose run is still live (the single-flight check — inner join, so a
/// draft whose run row is gone does not block a new one).
async fn draft_for(
    pg: &PgPool,
    conversation_id: &str,
    active: bool,
) -> Result<Option<PlanDraft>, sqlx::Error> {
    let join = if active {
        "join runs r on r.id = d.id and r.kind = 'plan-draft' \
         where d.conversation_id = $1::uuid and r.state in ('queued', 'running', 'awaiting')"
    } else {
        "left join runs r on r.id = d.id and r.kind = 'plan-draft' \
         where d.conversation_id = $1::uuid"
    };
    // AssertSqlSafe: the interpolation picks the JOIN half, not user input.
    let sql = format!(
        "select d.id::text, d.conversation_id::text, d.source, d.agent_model, \
                d.board_id::text, d.template_id::text, d.proposals, d.note, \
                (trunc(extract(epoch from d.created_at) * 1000))::bigint as created_ms, \
                r.state::text, r.phase, r.error \
         from plan_drafts d {join} \
         order by d.created_at desc limit 1"
    );
    // The joined row in toDraft's field order: the draft's own columns, then
    // the run's three.
    type DraftRow = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        serde_json::Value,
        Option<String>,
        i64,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<DraftRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(conversation_id)
        .fetch_optional(pg)
        .await?;
    let Some((
        id,
        conversation_id,
        source,
        agent_model,
        board_id,
        template_id,
        proposals,
        note,
        created_ms,
        run_state,
        phase,
        error,
    )) = row
    else {
        return Ok(None);
    };
    Ok(Some(PlanDraft {
        id,
        conversation_id,
        source,
        agent_model,
        board_id,
        template_id,
        // `proposals ?? []` — a column that is not the reviewed shape reads
        // as no proposals rather than a 500 on the review's way back.
        proposals: serde_json::from_value(proposals).unwrap_or_default(),
        note,
        status: project_status(run_state.as_deref()),
        phase: phase.unwrap_or_default(),
        error,
        created_at: epoch_ms_to_iso(created_ms),
    }))
}

/// The conversation's latest draft — the review's way back after a close, a
/// reload, or a server restart. None means "nothing paired to this plan".
pub async fn latest_draft_for(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<PlanDraft>, sqlx::Error> {
    draft_for(pg, conversation_id, false).await
}

async fn active_draft_for(
    pg: &PgPool,
    conversation_id: &str,
) -> Result<Option<PlanDraft>, sqlx::Error> {
    draft_for(pg, conversation_id, true).await
}

/// What a start needs (plan-drafts.ts startPlanDraft's args).
pub struct StartPlanDraft<'a> {
    pub conversation_id: &'a str,
    /// 'plan' | 'channel'
    pub source: &'a str,
    pub user_id: &'a str,
    pub agent_model: &'a str,
    pub routed_model: &'a str,
    pub tier: Option<&'a str>,
    pub board_id: Option<&'a str>,
    pub template_id: Option<&'a str>,
}

/// The two ways a start fails, both of which the POST answers with the same
/// fixed 500 sentence: the row machinery is down, or the enqueue could not be
/// assembled. The run's OWN failures reach the client through the draft row's
/// `error` field, never through this.
#[derive(Debug)]
pub enum PlanDraftError {
    Db(sqlx::Error),
    Start(String),
}

impl From<sqlx::Error> for PlanDraftError {
    fn from(e: sqlx::Error) -> Self {
        PlanDraftError::Db(e)
    }
}

impl std::fmt::Display for PlanDraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanDraftError::Db(e) => write!(f, "{e}"),
            PlanDraftError::Start(s) => write!(f, "{s}"),
        }
    }
}

/// Start (or ride) the conversation's draft. Returns the projected draft —
/// queued or running; the POST answers with it immediately.
///
/// COEXISTENCE ORDER, and it is load-bearing: enqueue with `start: false`
/// (the run row + its publish, no drive), then the domain row, then — in TS —
/// a detached `drive`. The Rust side OMITS the drive: until the flip arms the
/// Rust driver, the TS scheduler's sweep is what advances the queued run, and
/// the reclaim sweep is the guarantee either way. The run definition must
/// exist and be byte-correct before that handover, which is why this plane
/// crosses in the runs batch and not with the chat family.
pub async fn start_plan_draft(
    state: &AppState,
    args: StartPlanDraft<'_>,
) -> Result<PlanDraft, PlanDraftError> {
    if let Some(existing) = active_draft_for(&state.pg, args.conversation_id).await? {
        return Ok(existing);
    }
    let id = Uuid::new_v4().to_string();
    let input = serde_json::to_value(PlanDraftInput {
        conversation_id: args.conversation_id.to_string(),
        source: args.source.to_string(),
        agent_model: args.agent_model.to_string(),
        routed_model: args.routed_model.to_string(),
        board_id: args.board_id.map(String::from),
        template_id: args.template_id.map(String::from),
    })
    .map_err(|e| PlanDraftError::Start(e.to_string()))?;

    // The enqueue needs a live Redis for the lease and the publish — unlike
    // the fire-and-forget dispatch legs (which stand down and let the write
    // win), the run row IS the feature here, so a start without it fails the
    // start rather than half-happening.
    let Some(redis) = state.redis().await.ok() else {
        return Err(PlanDraftError::Start(
            "the run could not be enqueued: redis is unavailable".into(),
        ));
    };
    let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
    let deps = coexistence_dispatch_deps(state.pg.clone(), redis, realtime);
    enqueue(
        plan_draft_run(),
        input,
        EnqueueOptions {
            id: Some(id.clone()),
            owner_user_id: Some(args.user_id.to_string()),
            subject_type: Some(PLAN_DRAFT_KIND.into()),
            subject_id: Some(id.clone()),
            phase: Some("queued".into()),
            start: Some(false),
        },
        &deps,
    )
    .await?;
    sqlx::query(
        "insert into plan_drafts \
         (id, conversation_id, created_by, source, agent_model, routed_model, tier, board_id, template_id) \
         values ($1::uuid, $2::uuid, $3::uuid, $4, $5, $6, $7, $8::uuid, $9::uuid)",
    )
    .bind(&id)
    .bind(args.conversation_id)
    .bind(args.user_id)
    .bind(args.source)
    .bind(args.agent_model)
    .bind(args.routed_model)
    .bind(args.tier)
    .bind(args.board_id)
    .bind(args.template_id)
    .execute(&state.pg)
    .await?;
    let created = latest_draft_for(&state.pg, args.conversation_id).await?;
    created.ok_or_else(|| PlanDraftError::Start("could not create the plan draft".into()))
}

/// The review walk's edits, persisted: proposals written back to the latest
/// draft so ticks, drops and retitles survive a reload of the review.
pub async fn save_draft_proposals(
    pg: &PgPool,
    conversation_id: &str,
    proposals: &[StoredProposal],
) -> Result<(), sqlx::Error> {
    let json = serde_json::to_value(proposals).expect("StoredProposal is plain data");
    sqlx::query(
        "update plan_drafts set proposals = $1::jsonb, updated_at = now() \
         where id = (select id from plan_drafts where conversation_id = $2::uuid \
                     order by created_at desc limit 1)",
    )
    .bind(json)
    .bind(conversation_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Drop the conversation's draft — Back to config, or the batch was created
/// and consumed. A still-live run is cancelled first so it stops at its next
/// boundary instead of landing proposals into a deleted row; a cancel that
/// cannot happen (Redis down, run already finished) is swallowed and the
/// delete proceeds, exactly the TS `.catch(() => {})` posture.
pub async fn drop_draft(state: &AppState, conversation_id: &str) -> Result<(), sqlx::Error> {
    let latest: Option<(String,)> = sqlx::query_as(
        "select id::text from plan_drafts where conversation_id = $1::uuid \
         order by created_at desc limit 1",
    )
    .bind(conversation_id)
    .fetch_optional(&state.pg)
    .await?;
    let Some((id,)) = latest else {
        return Ok(());
    };
    if let Ok(redis) = state.redis().await {
        let realtime = RealtimeDeps::publish_only(Some(redis.clone()));
        let deps = coexistence_dispatch_deps(state.pg.clone(), redis, realtime);
        let _ = cancel_run(&id, Some("draft discarded".into()), &deps).await;
    }
    sqlx::query("delete from plan_drafts where id = $1::uuid")
        .bind(&id)
        .execute(&state.pg)
        .await?;
    Ok(())
}

/// toDraft's status projection: the joined run's state as the review reads
/// it. 'awaiting' cannot happen to this definition (it never parks on a
/// decision) but a MISSING run row can — that reads as 'done' so the
/// proposals stay reviewable rather than vanishing with the run row.
fn project_status(run_state: Option<&str>) -> String {
    match run_state {
        Some("awaiting") => "running".into(),
        None => "done".into(),
        Some(s) => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_run_row_reads_as_done_and_awaiting_as_running() {
        assert_eq!(project_status(None), "done");
        assert_eq!(project_status(Some("awaiting")), "running");
        assert_eq!(project_status(Some("queued")), "queued");
        assert_eq!(project_status(Some("running")), "running");
        assert_eq!(project_status(Some("error")), "error");
        assert_eq!(project_status(Some("cancelled")), "cancelled");
    }

    #[test]
    fn the_kind_is_the_registry_key() {
        assert_eq!(PLAN_DRAFT_KIND, "plan-draft");
    }
}
