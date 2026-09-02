// The approval census — every thing a human is currently expected to approve,
// in one shape, with the person who owes the answer attached.
//
// WHY THIS FILE EXISTS
//   "Approvals" is not one queue in Talaria. It is five, and they were written
//   at five different times by five different features:
//
//     google_action    google_pending_actions — an agent drafted something that
//                      LEAVES the building (an email, an invite).
//     workbench_plan   workbench_jobs.status = 'awaiting_approval' — an agent
//                      wrote a plan and is BLOCKED until someone says build it.
//     ticket_review    a ticket sitting in a review-category column — an agent
//                      handed work over and only a person can sign it off.
//     repo_request     workbench_repo_requests — an agent asked for a repo that
//                      does not exist. Admin-only.
//     run_decision     runs.state = 'awaiting' — a durable run reached a
//                      question its own step cannot answer and PARKED. The
//                      first kind with no table of its own: the run IS the
//                      record, and the question is a column on it.
//
//   Each of those has its own table, its own surface and its own idea of who
//   decides. None of them had an idea of how OLD it was, which is the whole
//   problem: an approval nobody looks at is indistinguishable from no approval
//   at all, and the agent on the other side of it waits for ever.
//
//   So this module answers one question — "what is waiting on a human, since
//   when, and on WHICH human" — for the digest (which counts them) and the
//   escalation job (which ages them). Neither should know four table shapes,
//   and neither should be the place a fifth approval kind gets forgotten: add
//   it HERE and both surfaces pick it up.
//
// THE DISCLOSURE RULE — read this before adding a kind
//   An approval's title and detail are CONTENT. A personal confirm-send is one
//   person's mail; a workbench plan is a board's engineering; a ticket sign-off
//   names a ticket on a board. "It has been waiting a long time" is never a
//   reason to show any of that to someone new, because the person it would
//   reach cannot decide it either — every decision route refuses them.
//
//   So each kind declares the AUTHORITY its decision route enforces, and that
//   authority is the only audience anything downstream may address. A run kind
//   that wanted to invent its own audience would have to go around
//   `audience_for`, which is the one thing the invariants check fails on.
//
// WHO MAY BE TOLD, AND HOW MUCH
//   The resolver takes an AUTHORITY and not an approval because the question
//   had been answered three times, by hand, in three files, and two of the
//   three leaked — the judge's copy sent an unassigned ticket's title to every
//   org admin and never told the board's own editors; the gaps copy fanned an
//   agent's free text to every admin in the workspace. Two audiences, not one:
//   THE FACT MAY TRAVEL FURTHER THAN THE CONTENT. Somebody who may not be told
//   WHAT is stuck can still be told THAT something is stuck, when they are the
//   person who can unblock it: add an editor to that board, grant the tool.
//
// ONE ANNOUNCER IS THE WHOLE POINT, and it is not a tidiness argument. A
//   subject with two announcement paths has two answers to "who may be told",
//   and only one of them asks the resolver. `request_repo` was exactly that:
//   the census resolved its row to admins-bounded-to-a-board while the verb
//   itself, in the same request, selected every admin in raw SQL and mailed
//   each of them the agent's free text. Bounding the census did not bound the
//   leak, because the leak was the copy that always fired first. Nothing that
//   raises an approval may write its own notification.
//
// EXACTLY ONCE IS A CLAIM ABOUT CONCURRENCY, so announce marks are MERGED in
//   the database (`jsonb || `), never read-modify-written over the whole blob:
//   the two announce paths overlap in time by design, and a whole-blob write
//   at the end of a seconds-long sweep is precisely how the request-path mark
//   gets erased and the same approval announced twice. See `mark_announced`.

use std::collections::HashMap;

use crate::agent_auth::{epoch_ms_to_iso, iso_to_epoch_ms};
use crate::notify::{NotificationInput, NotifyDeps, add_notification};
use crate::realtime::RealtimeDeps;
use crate::runs::define::{Authority, RunRow, RunState};
use crate::runs::run::{DefinitionForFn, NowFn, clamp_text};
use crate::runs::store;
use crate::statuses::status_category_sql;
use crate::tasks::human_assignee_ids;
use serde_json::Value;
use sqlx::PgPool;

const LOG: &str = "[approvals]";

/// The five queues. Serialized as its snake_case strings (`google_action`, …),
/// which is what the announce marks' failed-kind log lines and the digest's
/// counters are written in terms of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    GoogleAction,
    WorkbenchPlan,
    TicketReview,
    RepoRequest,
    RunDecision,
}

impl ApprovalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalKind::GoogleAction => "google_action",
            ApprovalKind::WorkbenchPlan => "workbench_plan",
            ApprovalKind::TicketReview => "ticket_review",
            ApprovalKind::RepoRequest => "repo_request",
            ApprovalKind::RunDecision => "run_decision",
        }
    }
}

/// Who may be told WHAT: the content half may be shown the thing itself, the
/// fact half may only be told that it is stuck. Both halves in one answer
/// because this module once returned the content half alone and every consumer
/// that needed the other went and fetched the admin list by hand — which is how
/// an approval bounded to a board was announced to NOBODY while the SLA
/// reported it to admins the census had never heard of.
#[derive(Debug, Clone, serde::Serialize, Default, PartialEq)]
pub struct Disclosure {
    pub content: Vec<String>,
    pub fact: Vec<String>,
}

/// The census row — the same shape for all five kinds, so the digest and the
/// SLA are looking at exactly what the decision route authorizes against.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    pub kind: ApprovalKind,
    /// Stable across runs — the dedupe key the escalation job records against,
    /// and what the announce marks are keyed on. For a run, the ROW's key,
    /// never a second derivation of it: two spellings would announce one pause
    /// twice.
    pub key: String,
    pub id: String,
    /// One line. Goes in a subject, a list item, or a notification title.
    /// CONTENT: only the authority below may be shown it.
    pub title: String,
    /// What the reader needs to decide it, in one sentence. Also CONTENT.
    pub detail: String,
    /// In-app path to the surface that can actually decide it.
    pub href: String,
    /// When a human became responsible. This is what ages.
    pub waiting_since: String,
    /// The named person(s) already on the hook — always a SUBSET of the
    /// authority, never a widening of it. Who gets nagged first; empty means
    /// nobody in particular owes the answer.
    pub owner_user_ids: Vec<String>,
    /// Who may decide it, and so who may be told what it is.
    pub authority: Authority,
}

/// Human owners of a ticket-anchored approval: the ticket's HUMAN assignees.
///
/// Not watchers, and not "everyone on the board". A watcher subscribed to
/// outcomes; nagging them for someone else's decision is the noise that gets a
/// sender filtered. An approval with no human assignee has no owner at all —
/// `owner_user_ids` is empty and the nag stage has nobody to write to. The
/// escalation stage then addresses the ticket's board editors, who are the
/// people the decision route was always going to let act.
fn ticket_owners(assignees: Option<&Value>) -> Vec<String> {
    // A non-array cell
    // (null, a hand-edited object) means no owners rather than a decode error.
    assignees
        .and_then(|v| v.as_array())
        .map(|rows| {
            human_assignee_ids(
                &rows
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect::<Vec<_>>(),
            )
        })
        .unwrap_or_default()
}

/// One `google_pending_actions` row as the select hands it back, in column
/// order — summary/agent_model/owner are nullable, the epoch-ms tail is
/// created_at.
type GoogleActionRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
    i64,
);

/// `google_pending_actions` held for a person: an agent drafted something that
/// leaves the building. Org actions are an admin's; a personal one is the
/// owner's ALONE — the route refuses an admin outright, and an admin is not
/// entitled to the subject line of somebody's mailbox by virtue of being an
/// admin, because they cannot action it either.
async fn google_action_approvals(pg: &PgPool) -> Result<Vec<PendingApproval>, sqlx::Error> {
    let rows: Vec<GoogleActionRow> = sqlx::query_as(
        "select id::text, kind, summary, agent_model, owner_user_id::text, is_org, \
                    (trunc(extract(epoch from created_at) * 1000))::bigint \
             from google_pending_actions where status = 'pending'",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, kind, summary, agent_model, owner_user_id, is_org, created_ms)| {
                let what = if kind == "gmail_send" {
                    "send an email"
                } else {
                    "create a calendar event"
                };
                // The summary belongs in the TITLE, not only the body: this string
                // is a notification headline and an email subject fragment, and "an
                // agent wants to send an email" is not a thing anyone can decide
                // from.
                let summary = summary.as_deref().map(str::trim).filter(|s| !s.is_empty());
                PendingApproval {
                    kind: ApprovalKind::GoogleAction,
                    key: format!("google_action:{id}"),
                    id: id.clone(),
                    title: format!(
                        "{} wants to {what}{}",
                        agent_model.as_deref().unwrap_or("An agent"),
                        summary.map(|s| format!(": {s}")).unwrap_or_default()
                    ),
                    detail: summary.map(str::to_string).unwrap_or_else(|| {
                        format!(
                            "A drafted {} is held until someone approves it.",
                            kind.replace('_', " ")
                        )
                    }),
                    // The focus queue is the surface that owns this decision.
                    href: "/".into(),
                    waiting_since: epoch_ms_to_iso(created_ms),
                    // Org rows name no owner; over a uuid
                    // column the only null-ish value is null.
                    owner_user_ids: match (&owner_user_id, is_org) {
                        (Some(owner), false) => vec![owner.clone()],
                        _ => Vec::new(),
                    },
                    // The route: org → any admin; personal → the owner ALONE, which
                    // refuses an admin outright. A personal confirm-send is one
                    // person's mail or calendar and an admin is not entitled to its
                    // subject line by virtue of being an admin — they cannot action
                    // it either.
                    authority: if is_org {
                        Authority::Admin { on_board: None }
                    } else if let Some(owner) = owner_user_id.as_deref() {
                        Authority::User {
                            user_ids: vec![owner.to_string()],
                        }
                    } else {
                        Authority::Nobody
                    },
                }
            },
        )
        .collect())
}

/// One `workbench_jobs`-anchored row, in column order — task_id/ticket_ref and
/// the task's title/board are nullable on the left join, assignees is the
/// ticket's jsonb.
type WorkbenchPlanRow = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    Option<String>,
    Option<Value>,
    Option<String>,
);

/// `workbench_jobs` parked on a plan: an agent is stopped dead behind this one
/// and heavy plans are rare, so loud is correct. The route requires board
/// editors on the job's ticket; a job with NO ticket is therefore decidable by
/// NOBODY — which is a real stall to report, not a reason to hand an agent's
/// plan to every admin in the workspace.
async fn workbench_plan_approvals(pg: &PgPool) -> Result<Vec<PendingApproval>, sqlx::Error> {
    let rows: Vec<WorkbenchPlanRow> = sqlx::query_as(
        "select j.id::text, j.agent_model, j.repo, j.effort, j.plan, j.task_id::text, \
                (trunc(extract(epoch from j.created_at) * 1000))::bigint, \
                t.title, t.board_id::text, t.assignees, \
                case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end \
         from workbench_jobs j \
         left join tasks t on t.id = j.task_id \
         left join boards b on b.id = t.board_id \
         where j.status = 'awaiting_approval' \
           and (t.id is null or (t.archived_at is null and b.archived_at is null))",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                agent_model,
                repo,
                effort,
                plan,
                task_id,
                created_ms,
                task_title,
                board_id,
                assignees,
                ticket_ref,
            )| {
                let for_line = match (&ticket_ref, &task_title) {
                    (Some(r), Some(t)) => format!(" for {r} · {t}"),
                    (Some(r), None) => format!(" for {r}"),
                    (None, Some(t)) => format!(" for {t}"),
                    (None, None) => String::new(),
                };
                // `clamp_text` caps the plan at 240
                // bytes on a char boundary — a lone surrogate never splits.
                let plan_line = {
                    let trimmed = plan.trim();
                    if trimmed.is_empty() {
                        "The agent posted a plan and stopped.".to_string()
                    } else {
                        clamp_text(trimmed, 240)
                    }
                };
                PendingApproval {
                    kind: ApprovalKind::WorkbenchPlan,
                    key: format!("workbench_plan:{id}"),
                    id: id.clone(),
                    title: format!("{agent_model} is waiting to build on {repo}"),
                    detail: format!("{effort}-effort work on {repo}{for_line}. {plan_line}"),
                    href: match (&task_id, &board_id) {
                        (Some(t), Some(b)) => format!("/boards/{b}/{t}"),
                        _ => "/".into(),
                    },
                    waiting_since: epoch_ms_to_iso(created_ms),
                    owner_user_ids: ticket_owners(assignees.as_ref()),
                    authority: match board_id {
                        Some(b) => Authority::Board { board_id: b },
                        None => Authority::Nobody,
                    },
                }
            },
        )
        .collect())
}

/// A ticket sitting in a review-category column: finished work handed over,
/// sign-off pending.
///
/// THE CLOCK. `tasks.updated_at` is NOT when this started waiting — it is when
/// the row was last written, and every later touch (a comment's side effects, a
/// label, an agent patching its own outcome) resets it. Combined with
/// "escalate once, ever", an actively-edited ticket escalates arbitrarily late
/// or never: the busiest ticket in review is the one the SLA forgets.
///
/// The ticket's own history knows the answer: a `task_activity` row of type
/// 'status' — `moved to <key>` (plus an optional "(why)") — is written every
/// time the column changes, however it changed. The LATEST such row for the
/// status the ticket is in NOW is the moment it entered review, and nothing but
/// another column move can move it. `starts_with` rather than LIKE because a
/// status key is user-defined and `_` is a LIKE wildcard. Fallback stays
/// `updated_at` for tickets that entered review before their history was kept:
/// younger than the truth, so a late nag rather than a false one.
/// One review-column ticket as the select hands it back — assignees is the
/// jsonb, the epoch-ms pair is updated_at?/enteredAt (the history subquery),
/// ticket_ref is nullable on ticket_no.
type TicketReviewRow = (
    String,
    String,
    String,
    Option<Value>,
    Option<i64>,
    Option<i64>,
    String,
    Option<String>,
);

async fn ticket_review_approvals(pg: &PgPool) -> Result<Vec<PendingApproval>, sqlx::Error> {
    let rows: Vec<TicketReviewRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select t.id::text, t.title, t.board_id::text, t.assignees, \
                (trunc(extract(epoch from t.updated_at) * 1000))::bigint, \
                (select max((trunc(extract(epoch from a.created_at) * 1000))::bigint) from task_activity a \
                  where a.task_id = t.id and a.type = 'status' \
                    and (a.description = 'moved to ' || t.status \
                         or starts_with(a.description, 'moved to ' || t.status || ' ('))), \
                b.name, \
                case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end \
         from tasks t join boards b on b.id = t.board_id \
         where t.archived_at is null and b.archived_at is null and {}",
        status_category_sql("review", &["quality_review"])
    )))
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, title, board_id, assignees, updated_ms, entered_ms, board, ticket_ref)| {
                PendingApproval {
                    kind: ApprovalKind::TicketReview,
                    key: format!("ticket_review:{id}"),
                    id: id.clone(),
                    title: format!(
                        "{}{title} needs sign-off",
                        ticket_ref
                            .as_deref()
                            .map(|r| format!("{r} · "))
                            .unwrap_or_default()
                    ),
                    detail: format!(
                        "Finished work is parked in {board}'s review column until a person \
                         approves it or asks for changes."
                    ),
                    href: format!("/boards/{board_id}/{id}"),
                    waiting_since: epoch_ms_to_iso(entered_ms.or(updated_ms).unwrap_or(0)),
                    owner_user_ids: ticket_owners(assignees.as_ref()),
                    // Sign-off is a column move, and both routes that perform
                    // one require board editors. An admin who is not on the
                    // board is not one of them.
                    authority: Authority::Board { board_id },
                }
            },
        )
        .collect())
}

/// `workbench_repo_requests`: an agent asked for a repo that does not exist.
///
/// `why` is the AGENT'S free text, written while working a ticket, so it
/// routinely quotes that board's work — which is why the authority carries
/// `on_board`: the request is still an ADMIN's to grant (the route is
/// admin-gated), and `on_board` only narrows who may read the words.
/// One `workbench_repo_requests` row, in column order — only the anchoring
/// board is nullable (a request without a task is genuinely org-wide).
type RepoRequestRow = (String, String, String, String, String, i64, Option<String>);

async fn repo_request_approvals(pg: &PgPool) -> Result<Vec<PendingApproval>, sqlx::Error> {
    let rows: Vec<RepoRequestRow> = sqlx::query_as(
        "select r.id::text, r.agent_model, r.org, r.name, r.why, \
                    (trunc(extract(epoch from r.created_at) * 1000))::bigint, t.board_id::text \
             from workbench_repo_requests r \
             left join tasks t on t.id = r.task_id \
             where r.status = 'pending'",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, agent_model, org, name, why, created_ms, board_id)| {
            // The clamp is 240 bytes on a char
            // boundary — a lone surrogate never splits.
            let why_line = {
                let trimmed = why.trim();
                if trimmed.is_empty() {
                    "The agent needs a repository that does not exist yet.".to_string()
                } else {
                    clamp_text(trimmed, 240)
                }
            };
            PendingApproval {
                kind: ApprovalKind::RepoRequest,
                key: format!("repo_request:{id}"),
                id: id.clone(),
                title: format!("{agent_model} asked for a new repo: {org}/{name}"),
                detail: why_line,
                href: "/admin/agents".into(),
                waiting_since: epoch_ms_to_iso(created_ms),
                owner_user_ids: Vec::new(),
                authority: Authority::Admin { on_board: board_id },
            }
        })
        .collect())
}

/// ONE description of what a parked run is as an approval — the census reader
/// below and the decide route's authorization check both go through this
/// function, because the only correct way to answer "may this person decide
/// this run" is to build the same approval the census builds. A second,
/// hand-written answer at the decision route is the shape this file's header
/// spends forty lines on: it is how `request_repo` ended up mailing every admin
/// the free text the census had carefully bounded to a board.
///
/// The approval key is the one the DRIVER already wrote on the row (derived
/// once, in runs/decide.rs `run_approval_key`); deriving it a second time here
/// would be a second opinion about the identity of one approval, and the
/// announce marks are keyed on it: the two spellings would announce twice.
///
/// Returns None when the run is not parked, has no question, or when THIS
/// process has no definition for its kind — LOUDLY and unmarked rather than
/// announced as widely as possible, because an approval announced to the wrong
/// people cannot be un-announced, while an approval that goes round again on
/// the next tick costs five minutes. That last one is a real state and not an
/// impossible one: a row from a newer deploy, or a kind whose module is not in
/// this instance's import graph, and it is the same call the runner makes when
/// it declines to drive a kind it does not know — leave it for an instance that
/// has it.
pub fn run_decision_approval(
    run: &RunRow,
    definition_for: &DefinitionForFn,
) -> Option<PendingApproval> {
    if run.state != RunState::Awaiting {
        return None;
    }
    let decision = run.decision.as_ref();
    let key = run.approval_key.as_deref();
    let (request, key) = match (decision, key) {
        (Some(d), Some(k)) => (&d.request, k),
        _ => {
            tracing::error!(
                "{LOG} run {} ({}) is awaiting with no {} on the row — nobody can be told about it",
                run.id,
                run.kind,
                if decision.is_some() {
                    "approval key"
                } else {
                    "question"
                }
            );
            return None;
        }
    };
    let Some(def) = definition_for(&run.kind) else {
        tracing::warn!(
            "{LOG} run {} is awaiting a decision but kind \"{}\" is not registered on this \
             instance — leaving it for one that has it. It stays unannounced and UNMARKED, so the \
             next sweep on an instance that imports the definition announces it normally.",
            run.id,
            run.kind
        );
        return None;
    };
    // The definition's own code, running inside the census. The `AudienceFn`
    // type cannot throw, so "cannot say" is structural rather than caught.
    let authority = (def.audience)(run);

    let options: Vec<&str> = request
        .options
        .iter()
        .map(|o| o.label.trim())
        .filter(|l| !l.is_empty())
        .collect();
    let detail = match request
        .detail
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        Some(d) => d.to_string(),
        // The fallback names where the run stopped, because the question alone
        // does not always say.
        None => format!(
            "{} stopped at \"{}\" and cannot go on until somebody chooses.",
            def.label, run.phase
        ),
    };
    Some(PendingApproval {
        kind: ApprovalKind::RunDecision,
        key: key.to_string(),
        id: run.id.clone(),
        title: format!("{} needs a decision: {}", def.label, request.question),
        detail: if options.is_empty() {
            detail
        } else {
            format!("{detail} Options: {}.", options.join(" · "))
        },
        // A run that has nowhere to send a reader is not a bug worth
        // suppressing the approval over — the notification still says what the
        // question is.
        href: request.href.clone().unwrap_or_else(|| "/".into()),
        // WHEN IT PARKED, and unusually for this file that is exactly what
        // `updated_at` means here. Compare the care `ticket_review` needs: a
        // ticket's row is touched by every later edit, so its timestamp drifts.
        // An `awaiting` run's row is touched by nothing — every write in the
        // store requires `state = 'running'`, and the only statement that moves
        // a row out of `awaiting` is the answer itself.
        waiting_since: run.updated_at.clone(),
        // The owner is who the run belongs to, NOT proof they may decide it: a
        // run owned by one person can pause to a board they are not an editor
        // of. Every stage that uses `owner_user_ids` intersects it with the
        // resolved audience first.
        owner_user_ids: run.owner_user_id.clone().into_iter().collect(),
        authority,
    })
}

/// The fifth reader: every `awaiting` run with a question on it. The set is
/// bounded by the number of decisions humans currently owe, so this is a small
/// read however large the run history gets — and it selects the SAME whole-row
/// shape the store does, because `audience(run)` is the run definition's own
/// code and a subset would be a `RunRow` with holes in it.
async fn run_decision_approvals(
    pg: &PgPool,
    definition_for: &DefinitionForFn,
) -> Result<Vec<PendingApproval>, sqlx::Error> {
    // AssertSqlSafe: the only interpolated fragment is the store's own
    // pub(crate) column list — same contract the store's queries carry.
    let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
        "select {} from runs where state = 'awaiting' and approval_key is not null",
        store::COLS
    )))
    .fetch_all(pg)
    .await?;
    let mut out = Vec::new();
    for row in &rows {
        let run = store::hydrate(row)?;
        if let Some(approval) = run_decision_approval(&run, definition_for) {
            out.push(approval);
        }
    }
    Ok(out)
}

/// Every pending approval in the workspace, oldest first.
///
/// One kind failing must not hide the other four — an escalation sweep that
/// reports nothing because one table was locked is the silence this whole
/// module is about. Each kind is settled independently — the contract is
/// the independence; a rejection is logged and recorded in `failed_kinds` so the
/// caller can say the census was incomplete rather than say it was empty.
pub struct PendingApprovals {
    pub approvals: Vec<PendingApproval>,
    pub failed_kinds: Vec<ApprovalKind>,
}

pub async fn pending_approvals(pg: &PgPool, definition_for: &DefinitionForFn) -> PendingApprovals {
    let mut approvals = Vec::new();
    let mut failed_kinds = Vec::new();
    // The newest source is also the one most likely to fail on a tree that has
    // not migrated yet (no `runs` table at all), which is precisely what
    // `failed_kinds` is for: a workspace mid-deploy still gets its other four
    // queues, and the sweep knows not to prune marks it could not see.
    let reads: [(ApprovalKind, Result<Vec<PendingApproval>, sqlx::Error>); 5] = [
        (
            ApprovalKind::GoogleAction,
            google_action_approvals(pg).await,
        ),
        (
            ApprovalKind::WorkbenchPlan,
            workbench_plan_approvals(pg).await,
        ),
        (
            ApprovalKind::TicketReview,
            ticket_review_approvals(pg).await,
        ),
        (ApprovalKind::RepoRequest, repo_request_approvals(pg).await),
        (
            ApprovalKind::RunDecision,
            run_decision_approvals(pg, definition_for).await,
        ),
    ];
    for (kind, result) in reads {
        match result {
            Ok(mut rows) => approvals.append(&mut rows),
            Err(e) => {
                tracing::error!(
                    "{LOG} could not read pending {} approvals: {e}",
                    kind.as_str()
                );
                failed_kinds.push(kind);
            }
        }
    }
    // ISO-8601 UTC strings compare identically in
    // byte order and in collation — oldest first either way.
    approvals.sort_by(|a, b| a.waiting_since.cmp(&b.waiting_since));
    PendingApprovals {
        approvals,
        failed_kinds,
    }
}

// ── The resolver ──────────────────────────────────────────────────────────────

/// Order-preserving dedupe — first occurrence wins, and that order is what
/// the disclosure halves are written in terms of.
fn dedup(ids: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter()
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// Every admin in the workspace. NOT exported: an admin list is an INGREDIENT
/// of an audience, never an audience — the disclosure resolver is the only
/// thing that may ask the question, which is the invariant the three
/// hand-rolled copies leaked through.
async fn admin_user_ids(pg: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as("select id::text from users where role = 'admin'")
        .fetch_all(pg)
        .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The EDITORS of each of these boards — `canEdit(boardRole(user, board))`
/// expressed as a set instead of a predicate, because the resolver needs the
/// whole membership rather than one yes/no.
///
/// Same two sources `board_role` unions: an explicit share of owner/editor
/// rank, and membership of the board's team (which `board_role` maps to owner
/// for a team owner and editor for everyone else — so every team member is at
/// least an editor). Viewers are excluded because the routes exclude them.
async fn board_editors(
    pg: &PgPool,
    board_ids: &[String],
) -> Result<HashMap<String, Vec<String>>, sqlx::Error> {
    let mut out: HashMap<String, Vec<String>> = board_ids
        .iter()
        .cloned()
        .map(|id| (id, Vec::new()))
        .collect();
    if board_ids.is_empty() {
        return Ok(out);
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select board_id::text, user_id::text from board_members \
         where board_id = any($1::uuid[]) and role in ('owner', 'editor') \
         union \
         select b.id::text, tm.user_id::text \
         from boards b join team_members tm on tm.team_id = b.team_id \
         where b.id = any($1::uuid[])",
    )
    .bind(board_ids)
    .fetch_all(pg)
    .await?;
    for (board_id, user_id) in rows {
        if let Some(list) = out.get_mut(&board_id) {
            list.push(user_id);
        }
    }
    Ok(out)
}

/// The resolver's composition — everything after the two reads, pure, so the
/// one part that decides who hears WHAT is testable with no database.
///
/// `admins` is "the admin list, if anything needed it" — the caller fetches it
/// only when some authority could not be answered directly (empty `direct`),
/// which is the only reason a `board` or `user` subject ever costs a second
/// query.
fn compose_disclosures(
    authorities: &[Authority],
    editors: &HashMap<String, Vec<String>>,
    admins: &[String],
) -> Vec<Disclosure> {
    // Everything answerable without the admin list, answered first.
    let direct: Vec<Option<Vec<String>>> = authorities
        .iter()
        .map(|a| match a {
            Authority::User { user_ids } => Some(dedup(user_ids.clone())),
            Authority::Board { board_id } => {
                Some(dedup(editors.get(board_id).cloned().unwrap_or_default()))
            }
            _ => None,
        })
        .collect();
    authorities
        .iter()
        .zip(direct)
        .map(|(a, direct)| {
            let bounded_to_board = matches!(a, Authority::Admin { on_board: Some(_) });
            let content = direct.unwrap_or_else(|| match a {
                Authority::Admin {
                    on_board: Some(board),
                } => {
                    // CONTENT narrowed to the admins who can also see that
                    // board: the thing is an admin's to action but quotes one
                    // board's work.
                    let editors = editors.get(board).cloned().unwrap_or_default();
                    admins
                        .iter()
                        .filter(|id| editors.contains(id))
                        .cloned()
                        .collect()
                }
                Authority::Admin { on_board: None } => dedup(admins.to_vec()),
                _ => Vec::new(),
            });
            // The fact goes to the admins the content could not reach: all of
            // them when it reached nobody at all, and the off-board ones when
            // it was bounded to a board. Never to somebody already getting the
            // content.
            let fact = if !content.is_empty() && !bounded_to_board {
                Vec::new()
            } else {
                admins
                    .iter()
                    .filter(|id| !content.contains(id))
                    .cloned()
                    .collect()
            };
            Disclosure { content, fact }
        })
        .collect()
}

/// THE resolver: who may be told about this thing, and how much of it. Every
/// caller in the product goes through this function or the batch form below —
/// there is exactly one of it, and a second copy is how the judge and gaps
/// leaks happened.
///
/// "Could not say" (a failed read) resolves to NOBODY, never to "everybody":
/// the resolver's errors are logged and answered empty, because the wider
/// audience is the one mistake this file exists to prevent.
pub async fn audience_for(pg: &PgPool, authority: &Authority) -> Disclosure {
    resolve_disclosures(pg, std::slice::from_ref(authority))
        .await
        .unwrap_or_default()
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// Resolve a batch of authorities in one pass. Two round trips at most — the
/// board memberships, then the admin list — shared across every subject and
/// every recipient, so two people can never be told different things about the
/// same subject and nobody can be told about one they cannot act on.
pub async fn resolve_disclosures(
    pg: &PgPool,
    authorities: &[Authority],
) -> Result<Vec<Disclosure>, sqlx::Error> {
    if authorities.is_empty() {
        return Ok(Vec::new());
    }
    let boards: Vec<String> = {
        let mut seen = std::collections::HashSet::new();
        authorities
            .iter()
            .filter_map(|a| match a {
                Authority::Board { board_id } => Some(board_id.clone()),
                Authority::Admin {
                    on_board: Some(board),
                } => Some(board.clone()),
                _ => None,
            })
            .filter(|id| seen.insert(id.clone()))
            .collect()
    };
    let editors = board_editors(pg, &boards).await?;
    // The admins are needed by every authority that IS the admins — and by the
    // FACT report of every authority that reached nobody, which is the only
    // reason a `board` or `user` subject ever costs a second query.
    let needs_admins = authorities.iter().any(|a| match a {
        Authority::User { user_ids } => dedup(user_ids.clone()).is_empty(),
        Authority::Board { board_id } => editors.get(board_id).is_none_or(|l| l.is_empty()),
        _ => true,
    });
    let admins = if needs_admins {
        admin_user_ids(pg).await?
    } else {
        Vec::new()
    };
    Ok(compose_disclosures(authorities, &editors, &admins))
}

/// The approvals-shaped call: each approval's declared authority, resolved to
/// the people its decision route would let act — AND to the people who may be
/// told only that it is stuck. Both halves, in one map, for the reason the
/// whole file exists.
pub async fn approval_audience(
    pg: &PgPool,
    approvals: &[PendingApproval],
) -> Result<HashMap<String, Disclosure>, sqlx::Error> {
    let resolved = resolve_disclosures(
        pg,
        &approvals
            .iter()
            .map(|a| a.authority.clone())
            .collect::<Vec<_>>(),
    )
    .await?;
    Ok(approvals
        .iter()
        .zip(resolved)
        .map(|(a, d)| (a.key.clone(), d))
        .collect())
}

/// A census plus the one thing every consumer of it needs: WHO each approval
/// may be shown to. Built together, in one pass, so no caller can hold a list
/// of approvals without also holding the answer to "who is allowed to read
/// this".
#[derive(Default)]
pub struct ApprovalCensus {
    pub approvals: Vec<PendingApproval>,
    pub failed_kinds: Vec<ApprovalKind>,
    /// approval key → who may be told about it, and HOW MUCH.
    pub audience: HashMap<String, Disclosure>,
}

pub async fn approval_census(pg: &PgPool, definition_for: &DefinitionForFn) -> ApprovalCensus {
    let pending = pending_approvals(pg, definition_for).await;
    // An audience read that fails resolves to nobody for everything — the
    // resolver's error posture, applied to the batch. The census is still
    // returned: the counts are real even when the disclosure is not.
    let audience = match approval_audience(pg, &pending.approvals).await {
        Ok(map) => map,
        Err(e) => {
            tracing::error!("{LOG} could not resolve any audience: {e}");
            HashMap::new()
        }
    };
    ApprovalCensus {
        approvals: pending.approvals,
        failed_kinds: pending.failed_kinds,
        audience,
    }
}

/// May this person be told what this approval is? THE predicate the digest and
/// the SLA both go through — the CONTENT half only, never the `fact` half,
/// which is people who may be told something is stuck and cannot be told what.
pub fn may_decide(census: &ApprovalCensus, approval: &PendingApproval, user_id: &str) -> bool {
    census
        .audience
        .get(&approval.key)
        .is_some_and(|who| may_decide_content(who, user_id))
}

/// The same predicate at a call site that already holds the resolved
/// disclosure — the decide route's shape, where the indirection through the
/// census map drops out.
pub fn may_decide_content(who: &Disclosure, user_id: &str) -> bool {
    who.content.iter().any(|u| u == user_id)
}

// ── Announcing an approval when it is RAISED ─────────────────────────────────
//
// The digest is a floor and the SLA is a ceiling; neither is an announcement.
// A confirm-send drafted at 09:05 was, until this existed, first heard about in
// the next morning's digest — the agent stopped for a day over a decision that
// takes four seconds. A raised approval is announced to the people who can
// decide it, immediately.
//
// TWO WAYS IN, ONE ANNOUNCER. `announce_approval` is what the code that RAISES
// an approval calls; `sweep_unannounced` is the safety net that catches
// everything nobody called it for, on the SLA job's tick. Both mark the same
// state, so a raised approval is announced exactly once.

const ANNOUNCE_STATE_KEY: &str = "approval_announce_state";

/// The stored marks: approval key → when it was announced. Read through the
/// forgiving settings read (a missing or malformed row is "nothing announced"),
/// which is the only read here — the WRITES merge, see `mark_announced`.
async fn announce_state(pg: &PgPool) -> HashMap<String, String> {
    let stored = crate::gateway::settings::get_setting(pg, ANNOUNCE_STATE_KEY, Value::Null).await;
    stored
        .get("announced")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// Record marks WITHOUT reading the blob first, because the read-then-write
/// the rest of the settings plane uses is exactly what would break the
/// exactly-once promise.
///
/// The two announce paths OVERLAP IN TIME by design: `announce_approval` fires
/// from the request that raised the approval, and the sweep's pass is seconds
/// long — a whole-blob write at the end of that pass would write a map read
/// BEFORE the request marked its key, erasing the fresh mark, and the next tick
/// would announce the same approval twice. So marks are merged in the database
/// instead: `||` on jsonb is a union and the statement is one row update, so
/// both writers survive whatever the order. Nothing is read first, so there is
/// no window to lose.
async fn mark_announced(pg: &PgPool, marks: &HashMap<String, String>) -> Result<(), sqlx::Error> {
    if marks.is_empty() {
        return Ok(());
    }
    let mut announced = serde_json::Map::new();
    for (k, v) in marks {
        announced.insert(k.clone(), Value::String(v.clone()));
    }
    sqlx::query(
        "insert into app_settings (key, value) \
         values ($1, jsonb_build_object('announced', $2::jsonb)) \
         on conflict (key) do update set \
           value = jsonb_build_object(\
             'announced', coalesce(app_settings.value -> 'announced', '{}'::jsonb) || $2::jsonb\
           ), \
           updated_at = now()",
    )
    .bind(ANNOUNCE_STATE_KEY)
    .bind(Value::Object(announced))
    .execute(pg)
    .await?;
    Ok(())
}

/// Forget marks for approvals that are no longer pending, so a
/// decided-and-recreated id cannot inherit a stale "already announced".
///
/// Same merge-safe statement shape, same reason — but `live_keys` comes from a
/// census taken at a moment in the past, and an approval raised SINCE that
/// moment is not in it. Deleting its mark because it "isn't live" is the
/// double-notify by another route, so a mark written at or after
/// `census_taken_at` is kept regardless: the census could not have seen what it
/// refers to. ISO-8601 UTC strings compare lexicographically in the order they
/// compare chronologically, and comparing as text rather than casting means a
/// hand-edited value cannot make this statement throw.
async fn prune_announced(
    pg: &PgPool,
    live_keys: &[String],
    census_taken_at: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update app_settings set value = jsonb_build_object('announced', (\
           select coalesce(jsonb_object_agg(e.key, e.value), '{}'::jsonb) \
           from jsonb_each(coalesce(value -> 'announced', '{}'::jsonb)) as e \
           where e.key = any($1::text[]) or (e.value #>> '{}') >= $2\
         )), updated_at = now() \
         where key = $3",
    )
    .bind(live_keys)
    .bind(census_taken_at)
    .bind(ANNOUNCE_STATE_KEY)
    .execute(pg)
    .await?;
    Ok(())
}

/// What a content-free message may say an approval IS. Fixed per kind and
/// never `approval.title` — the recipient is being told the workspace has
/// something stuck in it, not what is in it. A fifth kind gets its
/// content-free name in the same file it gets its authority.
pub fn kind_label(kind: ApprovalKind) -> &'static str {
    match kind {
        ApprovalKind::GoogleAction => "an outbound email or calendar invite an agent drafted",
        ApprovalKind::WorkbenchPlan => "a build plan an agent is blocked on",
        ApprovalKind::TicketReview => "a ticket waiting for sign-off",
        ApprovalKind::RepoRequest => "a repository an agent asked for",
        ApprovalKind::RunDecision => {
            "a long-running job parked on a question only a person can answer"
        }
    }
}

/// The fact, in words that are true in every case it is sent. `reached` is how
/// many people were told what it says, which is the difference between "this
/// is in hand and you are simply not on that board" and "nobody has been told
/// at all" — and an admin cannot tell those apart from the outside.
fn fact_body(approval: &PendingApproval, reached: usize) -> String {
    let bounded = matches!(approval.authority, Authority::Admin { on_board: Some(_) });
    let who = match (bounded, reached > 0) {
        (true, true) => {
            "It was raised while working a board you are not a member of, so what the \
                        agent wrote went to the admins who are."
        }
        (true, false) => {
            "It was raised while working a board no admin of this workspace is a \
                          member of, so nobody has been told what it says."
        }
        (false, true) => "The people whose decision it is have been told what it says.",
        (false, false) => {
            "Nobody in this workspace can decide it as it stands, so nobody has \
                           been told what it says."
        }
    };
    format!(
        "{} is waiting for a decision. {}\n\nNot a word of it is repeated here. Open Talaria to \
         act on it, or to give somebody who can the access they need.",
        kind_label(approval.kind),
        who
    )
}

/// Does first contact go to the whole audience, or only to the named? Declared
/// per kind so a fifth kind cannot be added without answering it.
///
/// `ticket_review` is the one that narrows. Its audience is every EDITOR of
/// the board, and a ticket entering review is not a rare event — it is the
/// ordinary end of every piece of agent work on that board. Six editors and
/// three tickets is eighteen notifications, and `approval_pending` routes to
/// `both` by default, so it is also eighteen emails, for three decisions at
/// most two people were ever going to make. That is the shape that teaches a
/// workspace to filter Talaria into a folder — and the classes that genuinely
/// had to reach somebody go into the folder with it. So a review's
/// FIRST-CONTACT announcement is addressed to the people already NAMED on the
/// ticket, and the rest of the board still hears it three ways this does not
/// remove: the digest (built from the same queues Home draws, every editor's,
/// every review ticket), Home itself, and the SLA, which escalates to EVERY
/// decider once the review has genuinely aged.
fn announce_to_named_only(kind: ApprovalKind) -> bool {
    match kind {
        // For a personal action the audience IS the owner, so narrowing is a
        // no-op; for an ORG action there is no owner at all and narrowing
        // would silence it.
        ApprovalKind::GoogleAction => false,
        // An agent is stopped dead behind this one and heavy plans are rare.
        // Loud is correct, and an unassigned ticket must not make it quiet.
        ApprovalKind::WorkbenchPlan => false,
        // The noisy one. See above.
        ApprovalKind::TicketReview => true,
        // Admin-only, never has an owner. Narrowing would silence it.
        ApprovalKind::RepoRequest => false,
        // Same argument as workbench_plan, and stronger: a parked run is a
        // piece of work stopped dead, holding a checkpoint, going nowhere
        // until somebody answers. Narrowing would also silence every ORG-WIDE
        // run — the owner is empty for a fitness sweep or a retrieval
        // migration, which are exactly the runs whose questions nobody
        // currently hears at all.
        ApprovalKind::RunDecision => false,
    }
}

/// Narrow an approval's disclosure to the people it is announced to on sight.
pub struct AnnounceTarget {
    /// Who to tell right now, in full. Always a SUBSET of the disclosure's
    /// `content` — this narrows, and nothing in this file may widen.
    pub to: Vec<String>,
    /// Who to tell that it EXISTS. Passed straight through from the disclosure:
    /// the fact audience is the people who can unblock it, and no per-kind
    /// first-contact policy narrows that. Narrowing `to` for a noisy kind must
    /// not quietly narrow this, or the bound that made a kind quiet would be
    /// the thing that also stopped anybody hearing it was stuck.
    pub fact: Vec<String>,
    /// Nobody is being told YET, and that is the policy working rather than
    /// failing: somebody can decide it, nobody is NAMED on it, so there is no
    /// one it is waiting on and nothing urgent to say. Deliberately distinct
    /// from an empty `to` because NOBODY can decide it, which is a stall the
    /// SLA reports to the admins.
    pub awaiting_owner: bool,
}

pub fn announce_target(approval: &PendingApproval, who: &Disclosure) -> AnnounceTarget {
    let fact = who.fact.clone();
    if !announce_to_named_only(approval.kind) {
        return AnnounceTarget {
            to: who.content.clone(),
            fact,
            awaiting_owner: false,
        };
    }
    // Intersected for the same reason the nag stage intersects: an assignee
    // who has since lost edit access on the board cannot open what we would
    // send.
    let named: Vec<String> = approval
        .owner_user_ids
        .iter()
        .filter(|id| who.content.contains(id))
        .cloned()
        .collect();
    if !named.is_empty() {
        return AnnounceTarget {
            to: named,
            fact,
            awaiting_owner: false,
        };
    }
    AnnounceTarget {
        to: Vec::new(),
        fact,
        awaiting_owner: !who.content.is_empty(),
    }
}

/// Everything the census and the announcer touch outside this module. The pool
/// and the realtime fan-out ride `notify` because the notification write is the
/// one thing the announce performs; `definition_for` is the run registry — the
/// same edge the driver holds, so a run kind this instance cannot name is
/// invisible to both for the same reason; `now` is the engine's clock, so the
/// announce marks carry the same timestamp discipline the store's do.
#[derive(Clone)]
pub struct ApprovalDeps {
    pub notify: NotifyDeps,
    pub definition_for: DefinitionForFn,
    pub now: NowFn,
}

impl ApprovalDeps {
    fn pg(&self) -> &PgPool {
        &self.notify.pg
    }

    /// Both halves over the pieces every caller already holds.
    pub fn new(
        pg: sqlx::PgPool,
        realtime: RealtimeDeps,
        definition_for: DefinitionForFn,
        now: NowFn,
    ) -> Self {
        Self {
            notify: NotifyDeps { pg, realtime },
            definition_for,
            now,
        }
    }
}

/// THE announcement. Both halves of the disclosure, in one call, because they
/// are one event: the people who may read the words get the words, and the
/// people who can act on it but may not read them get the FACT.
///
/// The fact half is not a nicety. Before it existed this function addressed
/// `content` and nothing else, so bounding an audience could silence the whole
/// announcement: admins-bounded-to-a-board resolves content to the admins who
/// are ALSO members of that board, which is EMPTY in a workspace where no
/// admin is — and every admin in that workspace can still GRANT the thing.
/// Nobody was told, and thirty days later the SLA reported that the workspace
/// had no admin at all.
///
/// Returns how many were reached on each channel — the caller marks the
/// approval announced when EITHER landed, and says which, because "announced
/// to the admins as a fact" and "announced to the people who can read it" are
/// different states of the workspace and only one of them is fine.
#[derive(Debug, Default, PartialEq)]
struct Told {
    content: usize,
    fact: usize,
}

async fn announce(
    deps: &ApprovalDeps,
    approval: &PendingApproval,
    target: &AnnounceTarget,
) -> Told {
    let mut told = Told::default();
    for user_id in dedup(target.to.clone()) {
        // A per-person failure is logged and skipped, never the whole
        // announcement's: one bad row must not silence the other recipients.
        let written = add_notification(
            &deps.notify,
            &user_id,
            &NotificationInput {
                kind: "approval_pending",
                title: &approval.title,
                body: Some(&format!(
                    "{}\n\nNothing happens until someone approves or rejects it.",
                    approval.detail
                )),
                href: Some(&approval.href),
            },
        )
        .await;
        match written {
            Ok(()) => told.content += 1,
            Err(e) => {
                tracing::error!(
                    "{LOG} could not announce {} to {user_id}: {e}",
                    approval.key
                )
            }
        }
    }
    for user_id in dedup(target.fact.clone()) {
        // Never to somebody already getting the content — the resolver
        // guarantees the two lists are disjoint, and this is the belt on it,
        // because the one thing worse than a fact nobody gets is a person told
        // the same approval twice in two different amounts of detail.
        if target.to.contains(&user_id) {
            continue;
        }
        let written = add_notification(
            &deps.notify,
            &user_id,
            &NotificationInput {
                kind: "approval_pending",
                title: &format!("Waiting for a decision: {}", kind_label(approval.kind)),
                body: Some(&fact_body(approval, told.content)),
                // NO href, on purpose. A fact report is not a link to the
                // thing — the recipient may not read the thing. What they can
                // act on is on a screen they can already find.
                href: None,
            },
        )
        .await;
        match written {
            Ok(()) => told.fact += 1,
            Err(e) => {
                tracing::error!(
                    "{LOG} could not tell {user_id} that {} exists: {e}",
                    approval.key
                )
            }
        }
    }
    told
}

/// Announce ONE just-raised approval, now. Safe to call from the request that
/// created it — it is idempotent (a second call is a no-op) — and unknown keys
/// are ignored: an approval decided between the insert and this call has
/// nothing to announce.
///
/// An approval still AWAITING AN OWNER is left unmarked, on purpose: a review
/// raised with no human on it that gets one two minutes later is still
/// announced to that person by the next sweep.
///
/// Returns how many were reached (either channel); every failure inside is
/// logged and answered 0 — the key stays unmarked for the sweep.
pub async fn announce_approval(deps: &ApprovalDeps, key: &str) -> usize {
    let pending = pending_approvals(deps.pg(), &deps.definition_for).await;
    let Some(approval) = pending.approvals.iter().find(|a| a.key == key) else {
        return 0;
    };
    if announce_state(deps.pg()).await.contains_key(key) {
        return 0;
    }
    // The resolver failing is "this process cannot say who may be told", and
    // "cannot say" resolves to nobody hearing anything rather than to the
    // widest audience available.
    let audience = match approval_audience(deps.pg(), std::slice::from_ref(approval)).await {
        Ok(map) => map,
        Err(e) => {
            tracing::error!("{LOG} could not resolve an audience for {key}: {e}");
            return 0;
        }
    };
    let who = audience.get(key).cloned().unwrap_or_default();
    let target = announce_target(approval, &who);
    if target.awaiting_owner {
        return 0;
    }
    let told = announce(deps, approval, &target).await;
    // Marked when EITHER channel landed. A fact that reached the admins is an
    // announcement — leaving it unmarked would repeat it every five minutes
    // for the life of the approval. A mark that failed to WRITE logs and
    // answers 0, leaving the key unmarked: the sweep will announce again —
    // a double-send, the lesser failure.
    if told.content > 0 || told.fact > 0 {
        let mut marks = HashMap::new();
        marks.insert(key.to_string(), epoch_ms_to_iso((deps.now)()));
        if let Err(e) = mark_announced(deps.pg(), &marks).await {
            tracing::error!("{LOG} could not record the announce mark for {key}: {e}");
            return 0;
        }
    }
    told.content + told.fact
}

/// A sweep's answer, in the units its caller reports.
pub struct AnnounceSweepResult {
    pub announced: usize,
    /// Announced as a FACT only: nobody may be told what it says, and the
    /// people who can unblock that were told it exists. Marked (it WAS
    /// announced), and counted separately because it is a workspace with a
    /// hole in it — an admin missing from a board — and a run that reports it
    /// as an ordinary announcement is the silence this module keeps
    /// rediscovering.
    pub fact_only: usize,
    /// Raised, still unannounced, and NOBODY can decide it. Left unmarked; the
    /// SLA's stall report is what raises those.
    pub unreachable: usize,
    /// Raised, decidable, but nobody is named on it yet and its kind announces
    /// to the named only. Not a failure and not a silence.
    pub awaiting_owner: usize,
}

/// Announce everything raised since the last pass.
///
/// `max_age_minutes` is the nag threshold: past it, the nag stage is already
/// going to speak and a first-contact announcement would be a second message
/// about the same thing. It is also what stops the first run after a deploy
/// from announcing a backlog that has been sitting there for a week.
pub async fn sweep_unannounced(
    deps: &ApprovalDeps,
    census: &ApprovalCensus,
    now_ms: i64,
    max_age_minutes: f64,
) -> AnnounceSweepResult {
    let pg = deps.pg();
    let already = announce_state(pg).await;
    // Only what THIS pass decided. Merged, never written over the top of the
    // map it was read from — see `mark_announced` for the mark a whole-blob
    // write eats.
    let mut marks: HashMap<String, String> = HashMap::new();
    let mut result = AnnounceSweepResult {
        announced: 0,
        fact_only: 0,
        unreachable: 0,
        awaiting_owner: 0,
    };
    // One instant for the whole pass — a sweep that runs long must not stamp
    // its last mark minutes after its first, or the prune guard below would
    // keep marks the census DID see.
    let now = epoch_ms_to_iso(now_ms);
    for approval in &census.approvals {
        if already.contains_key(&approval.key) {
            continue;
        }
        // Age is (now - waiting_since) in minutes, compared with >=; an
        // unparseable timestamp behaves as age 0 — it falls through to the
        // announce path, never to retirement.
        let age_minutes =
            iso_to_epoch_ms(&approval.waiting_since).map(|ms| (now_ms - ms) as f64 / 60_000.0);
        if age_minutes.is_some_and(|age| age >= max_age_minutes) {
            // Older than the nag threshold: not new, and the nag stage owns
            // it. Marked so it is not reconsidered every tick for the rest of
            // its life.
            marks.insert(approval.key.clone(), now.clone());
            continue;
        }
        let who = census
            .audience
            .get(&approval.key)
            .cloned()
            .unwrap_or_default();
        let target = announce_target(approval, &who);
        if target.awaiting_owner {
            // Nobody named yet. NOT marked — whoever gets assigned before the
            // nag threshold is still announced to, and if nobody is, the
            // retirement branch above owns it. Not counted as unreachable:
            // somebody can decide this.
            result.awaiting_owner += 1;
            continue;
        }
        let told = announce(deps, approval, &target).await;
        if told.content > 0 {
            marks.insert(approval.key.clone(), now.clone());
            result.announced += 1;
        } else if told.fact > 0 {
            // Nobody may be told what it says, and the people who can fix that
            // have been told it exists. That IS the announcement for this
            // approval — mark it, or it repeats every tick — but it is not the
            // same as somebody who can decide it having heard.
            marks.insert(approval.key.clone(), now.clone());
            result.fact_only += 1;
        } else {
            // Deliberately NOT marked: nobody heard, so nothing was announced.
            result.unreachable += 1;
        }
    }
    if let Err(e) = mark_announced(pg, &marks).await {
        tracing::error!("{LOG} could not record this sweep's announce marks: {e}");
    }
    // Only prune against a COMPLETE census — a kind that failed to read this
    // pass looks like a kind with nothing pending, and dropping its marks
    // would re-announce every one of its approvals the moment the table came
    // back.
    if census.failed_kinds.is_empty() {
        let live: Vec<String> = census.approvals.iter().map(|a| a.key.clone()).collect();
        if let Err(e) = prune_announced(pg, &live, &now).await {
            tracing::error!("{LOG} could not prune the announce marks: {e}");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::define::RunDecision;
    use std::sync::Arc;

    fn approval(kind: ApprovalKind, authority: Authority) -> PendingApproval {
        PendingApproval {
            kind,
            key: "k".into(),
            id: "id".into(),
            title: "t".into(),
            detail: "d".into(),
            href: "/".into(),
            waiting_since: "2026-08-29T00:00:00.000Z".into(),
            owner_user_ids: Vec::new(),
            authority,
        }
    }

    fn editors(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(board, users)| {
                (
                    board.to_string(),
                    users.iter().map(|u| u.to_string()).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn a_user_authority_is_itself_and_nobody_else() {
        let out = compose_disclosures(
            &[Authority::User {
                user_ids: vec!["u1".into(), "u1".into()],
            }],
            &HashMap::new(),
            &[],
        );
        // Deduped in first-occurrence order; no admins were needed, so no fact.
        assert_eq!(
            out[0],
            Disclosure {
                content: vec!["u1".into()],
                fact: vec![],
            }
        );
    }

    #[test]
    fn a_board_authority_is_its_editors() {
        let out = compose_disclosures(
            &[Authority::Board {
                board_id: "b1".into(),
            }],
            &editors(&[("b1", &["e1", "e2"])]),
            &[],
        );
        assert_eq!(out[0].content, ["e1", "e2"]);
        assert!(out[0].fact.is_empty());
    }

    #[test]
    fn an_admin_authority_is_every_admin_and_facts_nobody() {
        let admins = vec!["a1".into(), "a2".into()];
        let out = compose_disclosures(
            &[Authority::Admin { on_board: None }],
            &HashMap::new(),
            &admins,
        );
        assert_eq!(out[0].content, ["a1", "a2"]);
        // Content reached people and was not bounded: no fact, or an admin
        // would hear their own approval twice.
        assert!(out[0].fact.is_empty());
    }

    #[test]
    fn an_admin_authority_bounded_to_a_board_narrows_content_and_facts_the_rest() {
        let admins = vec!["a1".into(), "a2".into()];
        let out = compose_disclosures(
            &[Authority::Admin {
                on_board: Some("b1".into()),
            }],
            &editors(&[("b1", &["a2", "e1"])]),
            &admins,
        );
        // Only the admin who can also see the board gets the words…
        assert_eq!(out[0].content, ["a2"]);
        // …and the off-board admin gets the fact, not the content.
        assert_eq!(out[0].fact, ["a1"]);
    }

    #[test]
    fn a_bounded_authority_no_admin_can_see_announces_a_fact_to_all_of_them() {
        // The hole-in-the-workspace case: content is EMPTY, so the fact goes to
        // every admin — the people who can unblock it by joining the board.
        let admins = vec!["a1".into(), "a2".into()];
        let out = compose_disclosures(
            &[Authority::Admin {
                on_board: Some("b1".into()),
            }],
            &editors(&[("b1", &["e1"])]),
            &admins,
        );
        assert!(out[0].content.is_empty());
        assert_eq!(out[0].fact, ["a1", "a2"]);
    }

    #[test]
    fn an_authority_that_reached_nobody_facts_the_admins() {
        // A board with no editors at all: content empty, fact = every admin,
        // unbounded shape included.
        let admins = vec!["a1".into()];
        let out = compose_disclosures(
            &[Authority::Board {
                board_id: "b-empty".into(),
            }],
            &HashMap::new(),
            &admins,
        );
        assert!(out[0].content.is_empty());
        assert_eq!(out[0].fact, ["a1"]);
    }

    #[test]
    fn nobody_resolves_to_nobody_on_both_halves() {
        let out = compose_disclosures(&[Authority::Nobody], &HashMap::new(), &["a1".into()]);
        // Content empty by definition; the fact half still names the admins —
        // they are the ones the SLA's stall report reaches.
        assert!(out[0].content.is_empty());
        assert_eq!(out[0].fact, ["a1"]);
    }

    #[test]
    fn the_batch_shares_one_admin_read_across_every_subject() {
        // Two board authorities (both answerable directly) and one admin
        // authority: the composition sees the same admin list for all three,
        // so two people can never be told different things about the same
        // subject.
        let admins = vec!["a1".into()];
        let out = compose_disclosures(
            &[
                Authority::Board {
                    board_id: "b1".into(),
                },
                Authority::Admin { on_board: None },
                Authority::User {
                    user_ids: vec!["u1".into()],
                },
            ],
            &editors(&[("b1", &["e1"])]),
            &admins,
        );
        assert_eq!(out[0].content, ["e1"]);
        assert_eq!(out[1].content, ["a1"]);
        assert_eq!(out[2].content, ["u1"]);
    }

    #[test]
    fn kind_strings_are_the_wire_ones() {
        assert_eq!(ApprovalKind::GoogleAction.as_str(), "google_action");
        assert_eq!(ApprovalKind::RunDecision.as_str(), "run_decision");
        assert_eq!(
            serde_json::to_value(ApprovalKind::TicketReview).unwrap(),
            serde_json::json!("ticket_review")
        );
    }

    #[test]
    fn only_ticket_review_announces_to_the_named() {
        assert!(!announce_to_named_only(ApprovalKind::GoogleAction));
        assert!(!announce_to_named_only(ApprovalKind::WorkbenchPlan));
        assert!(announce_to_named_only(ApprovalKind::TicketReview));
        assert!(!announce_to_named_only(ApprovalKind::RepoRequest));
        assert!(!announce_to_named_only(ApprovalKind::RunDecision));
    }

    #[test]
    fn named_only_narrows_to_the_named_among_the_audience() {
        let mut review = approval(
            ApprovalKind::TicketReview,
            Authority::Board {
                board_id: "b1".into(),
            },
        );
        review.owner_user_ids = vec!["e1".into(), "e-viewer".into()];
        let who = Disclosure {
            content: vec!["e1".into(), "e2".into()],
            fact: vec!["a1".into()],
        };
        let target = announce_target(&review, &who);
        // Only the named ∩ audience: e-viewer is named but cannot decide it.
        assert_eq!(target.to, ["e1"]);
        assert_eq!(target.fact, ["a1"]);
        assert!(!target.awaiting_owner);
    }

    #[test]
    fn named_only_with_nobody_named_awaits_an_owner_rather_than_silencing() {
        let review = approval(
            ApprovalKind::TicketReview,
            Authority::Board {
                board_id: "b1".into(),
            },
        );
        let who = Disclosure {
            content: vec!["e1".into()],
            fact: vec![],
        };
        let target = announce_target(&review, &who);
        assert!(target.to.is_empty());
        // Somebody CAN decide it (content non-empty) — so this is the policy
        // working, not a stall.
        assert!(target.awaiting_owner);
    }

    #[test]
    fn a_loud_kind_announces_to_the_whole_content_audience() {
        let plan = approval(
            ApprovalKind::WorkbenchPlan,
            Authority::Board {
                board_id: "b1".into(),
            },
        );
        let who = Disclosure {
            content: vec!["e1".into(), "e2".into()],
            fact: vec![],
        };
        let target = announce_target(&plan, &who);
        assert_eq!(target.to, ["e1", "e2"]);
        assert!(!target.awaiting_owner);
    }

    #[test]
    fn the_fact_names_which_of_four_workspaces_it_is() {
        // Distinctive content, so the last assertion can prove none of it
        // leaks into the fact.
        let subject = |on_board: Option<String>| PendingApproval {
            title: "SECRET-TITLE".into(),
            detail: "SECRET-DETAIL".into(),
            authority: Authority::Admin { on_board },
            ..approval(
                ApprovalKind::RepoRequest,
                Authority::Admin { on_board: None },
            )
        };
        // Bounded, somebody told: "went to the admins who are".
        assert!(fact_body(&subject(Some("b1".into())), 1).contains("went to the admins who are"));
        // Bounded, nobody told: the hole in the workspace.
        assert!(
            fact_body(&subject(Some("b1".into())), 0).contains("no admin of this workspace is a")
        );
        // Unbounded, somebody told.
        assert!(fact_body(&subject(None), 2).contains("have been told what it says"));
        // Unbounded, nobody told: the stall.
        assert!(fact_body(&subject(None), 0).contains("Nobody in this workspace can decide it"));
        // And never the content itself.
        let body = fact_body(&subject(Some("b1".into())), 0);
        assert!(!body.contains("SECRET-TITLE"));
        assert!(!body.contains("SECRET-DETAIL"));
    }

    #[test]
    fn may_decide_reads_the_content_half_by_key() {
        let mut census = ApprovalCensus::default();
        census.audience.insert(
            "k".into(),
            Disclosure {
                content: vec!["u1".into()],
                fact: vec!["a1".into()],
            },
        );
        let run_decision = approval(ApprovalKind::RunDecision, Authority::Nobody);
        assert!(may_decide(&census, &run_decision, "u1"));
        // The fact half may NOT decide — and neither may an unknown key.
        assert!(!may_decide(&census, &run_decision, "a1"));
        let missing = PendingApproval {
            key: "missing".into(),
            ..run_decision.clone()
        };
        assert!(!may_decide(&census, &missing, "u1"));
    }

    #[test]
    fn may_decide_content_is_the_content_half_only() {
        let d = Disclosure {
            content: vec!["u-editor".into()],
            fact: vec!["u-admin".into()],
        };
        assert!(may_decide_content(&d, "u-editor"));
        assert!(
            !may_decide_content(&d, "u-admin"),
            "the fact half may be told it is stuck, not decide it"
        );
        assert!(!may_decide_content(&d, "u-stranger"));
    }

    // ── run_decision_approval ────────────────────────────────────────────────

    fn run_row(
        state: RunState,
        decision: Option<RunDecision>,
        approval_key: Option<String>,
    ) -> RunRow {
        RunRow {
            id: "run-1".into(),
            kind: "unit-kind".into(),
            owner_user_id: Some("u-owner".into()),
            subject_type: Some("task".into()),
            subject_id: Some("board-1".into()),
            state,
            phase: "picking an assignee".into(),
            checkpoint: serde_json::Value::Null,
            input: serde_json::Value::Null,
            result: serde_json::Value::Null,
            error: None,
            attempt: 0,
            lease_owner: None,
            lease_expires_at: None,
            approval_key,
            decision,
            created_at: "t0".into(),
            updated_at: "t0".into(),
            started_at: None,
            finished_at: None,
        }
    }

    /// A lookup that knows exactly one kind, so the fail-closed paths have an
    /// "unregistered kind" to be.
    fn lookup(kind: &str) -> DefinitionForFn {
        let kind = kind.to_string();
        Arc::new(move |k| {
            if *k != kind {
                return None;
            }
            Some(Arc::new(crate::runs::define::RunDefinition {
                kind: kind.clone(),
                label: "Ticket handover".into(),
                step: Arc::new(|_| {
                    Box::pin(async {
                        Ok(crate::runs::define::StepResult::Done {
                            result: serde_json::Value::Null,
                        })
                    })
                }),
                audience: Arc::new(|run| Authority::Board {
                    board_id: run.subject_id.clone().unwrap_or_else(|| "unknown".into()),
                }),
                max_step_ms: 30_000,
                max_attempts: 3,
            }))
        })
    }

    fn ask() -> crate::runs::define::DecisionRequest {
        crate::runs::define::DecisionRequest {
            key: "assignee".into(),
            question: "Who should take this ticket?".into(),
            detail: Some("Both are editors.".into()),
            options: vec![
                crate::runs::define::DecisionOption {
                    id: "ana".into(),
                    label: "Ana".into(),
                    detail: None,
                },
                crate::runs::define::DecisionOption {
                    id: "ben".into(),
                    label: "Ben".into(),
                    detail: None,
                },
            ],
            href: Some("/boards/board-1/task-1".into()),
        }
    }

    #[test]
    fn run_decision_approval_fails_closed() {
        let ask = ask();
        let parked = || {
            Some(RunDecision {
                request: ask.clone(),
                answer: None,
            })
        };
        let def = lookup("unit-kind");
        let nobody = lookup("another-kind");

        // Not awaiting, no question, no key, unknown kind: all None, never
        // "anybody".
        assert!(run_decision_approval(&run_row(RunState::Running, parked(), None), &def).is_none());
        assert!(
            run_decision_approval(&run_row(RunState::Awaiting, None, Some("k".into())), &def)
                .is_none()
        );
        assert!(
            run_decision_approval(&run_row(RunState::Awaiting, parked(), None), &def).is_none()
        );
        assert!(
            run_decision_approval(
                &run_row(RunState::Awaiting, parked(), Some("k".into())),
                &nobody
            )
            .is_none()
        );
    }

    #[test]
    fn run_decision_approval_translates_the_row() {
        let row = run_row(
            RunState::Awaiting,
            Some(RunDecision {
                request: ask(),
                answer: None,
            }),
            Some("run:unit-kind:run-1:assignee".into()),
        );
        let approval = run_decision_approval(&row, &lookup("unit-kind")).expect("translates");

        assert_eq!(approval.kind, ApprovalKind::RunDecision);
        // The key on the ROW, not a second derivation of it.
        assert_eq!(approval.key, "run:unit-kind:run-1:assignee");
        assert_eq!(
            approval.title,
            "Ticket handover needs a decision: Who should take this ticket?"
        );
        assert!(approval.detail.contains("Options: Ana · Ben."));
        assert_eq!(approval.href, "/boards/board-1/task-1");
        assert_eq!(
            approval.authority,
            Authority::Board {
                board_id: "board-1".into()
            }
        );
        assert_eq!(approval.owner_user_ids, vec!["u-owner".to_string()]);
        assert_eq!(approval.waiting_since, "t0");
    }

    #[test]
    fn ticket_owners_read_only_human_array_cells() {
        assert_eq!(
            ticket_owners(Some(&serde_json::json!(["user:u1", "agent-x", "user:u2"]))),
            ["u1", "u2"]
        );
        // A non-array cell (null, a hand-edited object) is no owners.
        assert!(ticket_owners(Some(&serde_json::json!(null))).is_empty());
        assert!(ticket_owners(Some(&serde_json::json!({"a": 1}))).is_empty());
        assert!(ticket_owners(None).is_empty());
        // Non-string cells are skipped.
        assert_eq!(
            ticket_owners(Some(&serde_json::json!(["user:u1", 7]))),
            ["u1"]
        );
    }
}
