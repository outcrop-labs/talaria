// PAUSE AS APPROVAL. The two sides of `awaiting`: `pause` puts a run down on a
// question and tells the people entitled to answer it, `decide` checks that the
// person answering is one of them and puts the run back to work. Port of
// ui/src/server/runs/decide.ts.
//
// WHY THIS IS NOT A NEW CONCEPT
//   A run that needs a human IS an approval. The product already has exactly one
//   answer to "a person owes a decision, who may be told what it says, who is
//   nagged, and what happens when nobody looks" — server/approvals.ts, and the
//   forty lines of header on it are all scar tissue from getting that answer
//   wrong. A "needs input" queue for runs would be a second inbox with a second
//   disclosure rule, and the second one is always the one that leaks.
//
//   So a parked run is `ApprovalKind = 'run_decision'`, gathered by
//   `pendingApprovals`, announced by `announceApproval`, swept by
//   `sweepUnannounced`, aged by the SLA, and routed by the `approval_pending`
//   notify class each user has already configured. This file adds no queue, no
//   notification class and no audience of its own.
//
// THE AUTHORITY BOUNDARY, which is the one thing to get right here:
//
//   THE RUN DECLARES, THIS FILE ENFORCES, approvals RESOLVES.
//   `RunDefinition.audience(run)` returns an approvals `Authority` — and that is
//   the whole extent of a run kind's involvement in access. It does not fetch
//   users, it does not know what a board editor is, and it never sees the list
//   of people its question reached. `run_decision_approval` turns the row into
//   the same `PendingApproval` the census builds, `audience_for` resolves it,
//   and `may_decide` is the predicate — the same three functions the digest and
//   the SLA go through, with no fourth opinion introduced here.
//
//   A DECISION CANNOT ESCALATE WHAT A RUN MAY DO. The answer is DATA:
//   an `option_id` the step itself declared, plus an optional note. Three things
//   enforce that, and each of them is a thing somebody could otherwise smuggle
//   authority through:
//     · the option must be one the STEP offered — an id nobody declared is
//       refused, so a decider cannot hand the step an instruction it never wrote
//       a branch for;
//     · `answered_by` is recorded for audit and is never consulted for
//       permission — the run goes on doing what its definition allows, under the
//       agent identity and guardrails it already had. A step that read
//       `decision.answered_by` and did something on that person's behalf would
//       be a privilege escalation with a paper trail, and it is the step's job
//       not to;
//     · nothing here writes to the run's `input`, which is the only field that
//       could widen the work itself.
//
//   A PAUSED RUN CANNOT SELF-RESUME. `store.park` releases the lease and
//   `is_drivable` excludes `awaiting`, so no sweep, no reclaim and no detached
//   drive moves it. The single statement that takes a row out of `awaiting` is
//   `store.answer`, and this file is the only caller that gets to reach it with
//   a person's name attached.
//
// THE SEAMS. approvals.ts and daily-brief-stale.ts have not crossed yet — they
// land later in this batch — so the three approvals shapes this file touches
// (`Disclosure`, `PendingApproval`, `may_decide`) are declared here and MOVE to
// the approvals module when it crosses, and `audience_for` / `announce` /
// `mark_brief_stale` are fields on the deps rather than imports. Their CONTRACTS
// are the TS ones: `audience_for` resolves an `Authority` to the two halves of a
// `Disclosure` and resolves "could not say" to NOBODY, never to "everybody";
// `announce` files the approval with the notify machinery and returns how many
// people were reached (ZERO IS A REAL ANSWER); `mark_brief_stale` clears the
// deciders' brief lines.

use crate::agent_auth::epoch_ms_to_iso;
use futures_util::future::BoxFuture;
use serde::Serialize;
use std::sync::Arc;

use super::define::{Authority, DecisionAnswer, DecisionRequest, RunDecision, RunRow, RunState};
use super::run::{
    DefinitionForFn, PauseArgs, PauseFn, PauseOutcome, PublishFn, RunDeps, RunEvent, clamp_text,
    drive,
};
use super::store::{AnswerOutcome, RunStore, WriteFailure};

const LOG: &str = "[runs]";

// ── The approvals shapes this file touches (they move when approvals crosses) ─

/// Who may be told WHAT: the content half may be shown the thing itself, the
/// fact half may only be told that it is stuck. Both halves in one answer
/// because approvals.ts once returned the content half alone and every consumer
/// that needed the other went and fetched the admin list by hand — which is how
/// an approval bounded to a board was announced to NOBODY while the SLA
/// reported it to admins the census had never heard of.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Disclosure {
    pub content: Vec<String>,
    pub fact: Vec<String>,
}

/// The census row for a parked run — the same shape `pendingApprovals` builds,
/// so the thing this file authorizes against is byte-for-byte the thing the
/// digest, the announcement and the SLA are looking at.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingApproval {
    /// Always "run_decision" here; the census's other kinds arrive with the
    /// approvals port.
    pub kind: &'static str,
    /// Stable across runs — the dedupe key the escalation job records against.
    /// The ROW's key, never a second derivation of it: the announce marks are
    /// keyed on this string, and two spellings would announce one pause twice.
    pub key: String,
    pub id: String,
    /// One line. Goes in a subject, a list item, or a notification title.
    /// CONTENT: only the authority below may be shown it.
    pub title: String,
    /// What the reader needs to decide it. Also CONTENT.
    pub detail: String,
    /// In-app path to the surface that can actually decide it.
    pub href: String,
    /// When a human became responsible. This is what ages.
    pub waiting_since: String,
    /// The named person(s) already on the hook — always a SUBSET of the
    /// authority, never a widening of it.
    pub owner_user_ids: Vec<String>,
    /// Who may decide it, and so who may be told what it is.
    pub authority: Authority,
}

/// The approvals key for a parked run, derived from the run and the question
/// and nothing else.
///
/// STABLE ACROSS RE-ASKS BY CONSTRUCTION, which is what makes the pause
/// idempotent under at-least-once delivery: a reclaimed run re-enters `step()`,
/// the step asks the same question again, and the same key comes out — so the
/// announce marks dedupe it instead of paging somebody twice about one decision.
///
/// THE FLIP SIDE: a step that answers a question, carries on, and later asks a
/// GENUINELY NEW question under the SAME `question.key` produces the same
/// approval key, and may inherit the earlier announcement mark — nobody is told
/// the second time. Vary the key when the question is new; reuse it only when
/// re-asking the same thing.
pub fn run_approval_key(kind: &str, run_id: &str, question_key: &str) -> String {
    format!("run:{kind}:{run_id}:{question_key}")
}

/// The row → approval translation. The DEFAULT for `DecideDeps.approval_for`,
/// and pure: it reads the row, asks the definition for its authority, and
/// builds the census entry. FAILS CLOSED — `None` means "this instance cannot
/// say who may decide this run", never "anybody".
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
                "[approvals] run {} ({}) is awaiting with no {} on the row — nobody can be told \
                 about it",
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
            "[approvals] run {} is awaiting a decision but kind \"{}\" is not registered on this \
             instance — leaving it for one that has it. It stays unannounced and UNMARKED, so the \
             next sweep on an instance that imports the definition announces it normally.",
            run.id,
            run.kind
        );
        return None;
    };
    // The definition's own code, running inside the census. In TS a throw here
    // resolved to nobody hearing anything; the Rust `AudienceFn` type cannot
    // throw, so "cannot say" is structural rather than caught.
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
        kind: "run_decision",
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
        // WHEN IT PARKED, and unusually for an approvals row that is exactly
        // what `updated_at` means here: an `awaiting` run's row is touched by
        // nothing (every write in the store requires `state = 'running'`, and
        // the only statement that moves a row out of `awaiting` is the answer
        // itself), so the timestamp cannot drift the way a ticket's does.
        waiting_since: run.updated_at.clone(),
        // The owner is who the run belongs to, NOT proof they may decide it: a
        // run owned by one person can pause to a board they are not an editor
        // of. Every stage that uses `owner_user_ids` intersects it with the
        // resolved audience first.
        owner_user_ids: run.owner_user_id.clone().into_iter().collect(),
        authority,
    })
}

/// THE predicate, not a re-derivation of it: this person is in the CONTENT half
/// — never the `fact` half, which is people who may be told something is stuck
/// and cannot be told what. TS's `mayDecide` takes the whole census and the
/// approval and looks the audience up by key; at the single-approval call site
/// here that indirection drops out, and the census-shaped wrapper arrives with
/// the approvals port.
pub fn may_decide(who: &Disclosure, user_id: &str) -> bool {
    who.content.iter().any(|u| u == user_id)
}

// ── Deps ─────────────────────────────────────────────────────────────────────

pub type AudienceForFn = Arc<dyn Fn(&Authority) -> BoxFuture<'static, Disclosure> + Send + Sync>;
pub type AnnounceFn = Arc<dyn Fn(&str) -> BoxFuture<'static, usize> + Send + Sync>;
pub type MarkBriefStaleFn = Arc<dyn Fn(Vec<String>) -> BoxFuture<'static, ()> + Send + Sync>;
pub type ApprovalForFn = Arc<dyn Fn(&RunRow) -> Option<PendingApproval> + Send + Sync>;

/// The edges `pause` touches. TS gave `pause` the whole `DecideDeps` and let the
/// DRIVER's `deps.pause` default to it lazily; a Rust struct literal resolves
/// eagerly, and `DecideDeps` wraps `RunDeps` which contains the pause — so the
/// pause is built from the edges instead. Same world, one construction order:
/// edges → `pause_fn` → `RunDeps` → `DecideDeps`.
#[derive(Clone)]
pub struct PauseDeps {
    pub store: Arc<dyn RunStore>,
    pub publish: PublishFn,
    pub definition_for: DefinitionForFn,
    pub audience_for: AudienceForFn,
    pub announce: AnnounceFn,
}

/// Everything either half of this file touches outside itself. Wraps the whole
/// of `RunDeps` rather than re-declaring its fields because `decide` hands the
/// bag to the resume drive — two dep shapes describing one world is how a test
/// ends up faking the store for one call and hitting Postgres on the next.
#[derive(Clone)]
pub struct DecideDeps {
    pub run: RunDeps,
    /// The row → approval translation. Defaulted to the census's own builder.
    pub approval_for: ApprovalForFn,
    pub audience_for: AudienceForFn,
    pub announce: AnnounceFn,
    pub mark_brief_stale: MarkBriefStaleFn,
}

impl DecideDeps {
    /// The edges `pause` uses, out of the same bag the answer half uses.
    pub fn pause_deps(&self) -> PauseDeps {
        PauseDeps {
            store: self.run.store.clone(),
            publish: self.run.publish.clone(),
            definition_for: self.run.definition_for.clone(),
            audience_for: self.audience_for.clone(),
            announce: self.announce.clone(),
        }
    }
}

// ── pause ────────────────────────────────────────────────────────────────────

/// What `pause` answers. The ok half carries how many people were actually
/// told — ZERO IS A REAL ANSWER and it is not a failure of the pause: the row
/// is `awaiting` and durable either way, with an UNMARKED key the approvals
/// sweep announces on its next tick.
#[derive(Debug, Clone)]
pub enum PauseResult {
    Parked {
        approval_key: String,
        announced: usize,
        /// Who could have been told, from the run's own declared authority.
        audience: Disclosure,
    },
    /// The park did not land, and every reason is a normal one: another
    /// instance owns the run now, somebody cancelled it, the row is gone. The
    /// question is simply not asked; nothing is half-parked. The state rides
    /// inside the reason, where the TS result carried it beside.
    Refused { reason: WriteFailure },
}

/// Park a run on a question and file it as an approval.
///
/// Callable only by whoever HOLDS THE RUN'S LEASE, and the token is a required
/// argument for that reason rather than as a formality: `store.park` is a
/// compare-and-set on `(id, lease_owner, state = 'running')`, so a park without
/// the lease is a park that races the driver still executing the step.
///
/// ORDER: persist, then publish, then tell. The row is `awaiting` before anybody
/// hears it is, and the telling cannot fail the pause — a notification is a
/// DELIVERY of the record, and a delivery that did not happen must not destroy
/// the thing it was about.
///
/// AT-LEAST-ONCE, FROM THE OTHER SIDE. A process that dies between the park and
/// the announcement leaves a parked run with an UNMARKED key, and the approvals
/// sweep announces it on its next tick — a late notification rather than a lost
/// one. That works only because the mark is written by the announcer after a
/// delivery lands, never here: a pause that marked a key it had not managed to
/// send would produce the one failure this system exists to end, a run parked
/// for ever that nobody was told about and no sweep will look at again.
pub async fn pause(args: PauseArgs, deps: &PauseDeps) -> Result<PauseResult, sqlx::Error> {
    let Some(run) = deps.store.get(&args.run_id).await? else {
        return Ok(PauseResult::Refused {
            reason: WriteFailure::Missing,
        });
    };

    let approval_key = run_approval_key(&run.kind, &run.id, &args.question.key);
    // TS slices the phase at 300 UTF-16 units; the clamp here is 300 bytes on a
    // char boundary — the standing surrogate divergence, as on the note below.
    let phase = clamp_text(args.phase.as_deref().unwrap_or(&run.phase), 300);
    let decision = RunDecision {
        request: args.question.clone(),
        answer: None,
    };
    if let Err(reason) = deps
        .store
        .park(
            &args.run_id,
            &args.token,
            decision.clone(),
            approval_key.clone(),
            phase.clone(),
        )
        .await?
    {
        return Ok(PauseResult::Refused { reason });
    }

    // THE ONE EVENT THAT CARRIES THE QUESTION — and only on the way to the
    // people the definition's audience will resolve to, which is the publisher
    // at the other end of this seam's job to guarantee.
    (deps.publish)(
        RunEvent {
            kind_tag: "run",
            run_id: run.id.clone(),
            kind: run.kind.clone(),
            state: RunState::Awaiting,
            phase: phase.clone(),
            question: Some(args.question.clone()),
            error: None,
        },
        run.owner_user_id.as_deref(),
    );

    // The row as it now IS, rather than as it was read a moment ago: the park
    // wrote exactly these fields, and handing a definition a row that still
    // says `running` would be asking it who may decide a run that is not
    // parked.
    let mut parked = run.clone();
    parked.state = RunState::Awaiting;
    parked.decision = Some(decision);
    parked.approval_key = Some(approval_key.clone());
    parked.phase = phase;
    parked.lease_owner = None;
    parked.lease_expires_at = None;

    // The audience is resolved from the DEFINITION's declared authority, on the
    // row as it now stands. Resolved here only so the caller and the log can say
    // how many people could have been told; the announcement itself goes through
    // the announcer, which resolves it again from the census. That is not a
    // wasted round trip, it is the guarantee: nothing in this file may hand the
    // announcer an audience of its own. The seam's contract carries the TS
    // resolver's error posture — "could not say" resolves to NOBODY, never to
    // everybody.
    let audience = match (deps.definition_for)(&run.kind) {
        Some(def) => (deps.audience_for)(&(def.audience)(&parked)).await,
        None => Disclosure::default(),
    };

    let announced = (deps.announce)(&approval_key).await;
    if announced == 0 {
        // LOUD, because a parked run nobody was told about is the exact silence
        // this whole system exists to end — and it is survivable rather than
        // fatal only because the key is left unmarked for the sweep to find.
        // Say which key, so the gap is a log line somebody can grep.
        tracing::warn!(
            "{LOG} {} ({}) is awaiting a decision and nobody was announced to ({} could decide \
             it) — {approval_key} stays unannounced and UNMARKED, so the approvals sweep will \
             pick it up.",
            run.id,
            run.kind,
            audience.content.len()
        );
    }
    Ok(PauseResult::Parked {
        approval_key,
        announced,
        audience,
    })
}

/// The driver's `PauseFn` over `pause` — the adapter the handoff slice puts in
/// `RunDeps.pause`. The driver's coarser `PauseOutcome` is the contract it
/// needs; the `Disclosure` stays on this side.
pub fn pause_fn(deps: PauseDeps) -> PauseFn {
    Arc::new(move |args: PauseArgs| {
        let deps = deps.clone();
        let run_id = args.run_id.clone();
        Box::pin(async move {
            match pause(args, &deps).await {
                Ok(PauseResult::Parked {
                    approval_key,
                    announced,
                    ..
                }) => PauseOutcome::Parked {
                    approval_key,
                    announced,
                },
                Ok(PauseResult::Refused { reason }) => PauseOutcome::Refused {
                    state: match &reason {
                        WriteFailure::LeaseLost { state } | WriteFailure::State { state } => {
                            Some(*state)
                        }
                        _ => None,
                    },
                    reason,
                },
                // A database error under the park: the row is still `running`
                // under our lease, and the at-least-once answer is a clean stop
                // that releases it for the reclaim sweep to re-ask the
                // question. `PauseFn` cannot carry the error, so it is said
                // here, loudly, before the stop it becomes.
                Err(e) => {
                    tracing::error!("{LOG} pause of {run_id} hit the database: {e}");
                    PauseOutcome::Refused {
                        reason: WriteFailure::State {
                            state: RunState::Running,
                        },
                        state: Some(RunState::Running),
                    }
                }
            }
        })
    })
}

// ── decide ───────────────────────────────────────────────────────────────────

/// THE `awaiting → queued` write, and the reason it is not exported.
///
/// This is the only statement in the system that takes a row out of `awaiting`,
/// and for one round of the TS project it lived in run.ts as a public
/// `answerRun` with no authority check in it — beside `decide()`, which has one.
/// Two doors into one write, one of them open, makes the gate a convention: a
/// route that imported the more obvious name would resume somebody else's run on
/// the strength of a request body.
///
/// So: module-private, one caller, and `decide()` above it does the resolving
/// and the asking. A future non-human answer path (a policy, a timeout rule —
/// `DecisionAnswer.answered_by` is nullable for exactly those) does not get to
/// reuse this door either; it needs its own entry point with its own explicit
/// statement of what authorized it, because the alternative is that the one
/// function enforcing "a person may decide this" acquires a way to be called
/// with no person.
async fn resume_answered(
    run_id: &str,
    answer: DecisionAnswer,
    start: bool,
    deps: &DecideDeps,
) -> Result<DecideResult, sqlx::Error> {
    // `store.answer` re-checks the question key against the row it writes,
    // which is what actually closes the race with a second decider — the check
    // in `decide` is against a row read a moment earlier.
    let run = match deps.run.store.answer(run_id, answer.clone()).await? {
        AnswerOutcome::Answered(run) => run,
        AnswerOutcome::Missing => {
            return Ok(DecideResult::Refused {
                reason: DecideRefusal::Missing,
                state: None,
            });
        }
        AnswerOutcome::NotAwaiting { state } => {
            return Ok(DecideResult::Refused {
                reason: DecideRefusal::NotAwaiting,
                state,
            });
        }
        AnswerOutcome::StaleKey { state } => {
            return Ok(DecideResult::Refused {
                reason: DecideRefusal::StaleKey,
                state: Some(state),
            });
        }
    };
    // PERSIST, THEN PUBLISH, as everywhere else: a device told the run is queued
    // before the row says so would refetch and see it still parked.
    (deps.run.publish)(
        RunEvent {
            kind_tag: "run",
            run_id: run.id.clone(),
            kind: run.kind.clone(),
            state: run.state,
            phase: run.phase.clone(),
            question: None,
            error: None,
        },
        run.owner_user_id.as_deref(),
    );
    if start {
        let deps = deps.run.clone();
        let run_id = run.id.clone();
        tokio::spawn(async move {
            if let Err(e) = drive(&run_id, &deps).await {
                tracing::error!("{LOG} drive after answering {run_id} threw: {e}");
            }
        });
    }
    Ok(DecideResult::Decided { run, answer })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecideRefusal {
    /// No such run.
    Missing,
    /// Not parked on anything — already answered, cancelled, or still running.
    /// Two people racing the same question is the common cause and it is not an
    /// error worth showing: somebody answered it, which is what they wanted.
    NotAwaiting,
    /// Answering a question the run is no longer parked on — a stale tab.
    StaleKey,
    /// This person may not decide this run. Deliberately ONE reason for both
    /// "you are not in the audience" and "nobody is": the caller returns 403
    /// either way, and a route that distinguished them would tell a stranger
    /// which runs exist and who can see them.
    Forbidden,
    /// An option the step never offered.
    UnknownOption,
}

#[derive(Debug, Clone)]
pub enum DecideResult {
    Decided {
        run: Box<RunRow>,
        answer: DecisionAnswer,
    },
    Refused {
        reason: DecideRefusal,
        state: Option<RunState>,
    },
}

/// Answer the question a run is parked on, as a named person, and put it back in
/// the queue.
///
/// Callable from ANY instance — that is the whole reason the question is a
/// column and not a closure. The person answering is on whichever instance their
/// request landed on, which is never reliably the one that asked; park a run on
/// one instance, open the approval on your phone, and this still works.
///
/// WHERE THE ANSWER GOES. Into the run's `decision` column, which the driver
/// hands to the next step as `ctx.decision` and CLEARS in the same write as the
/// checkpoint that step produces. That is the design's "the answer goes into the
/// checkpoint", implemented where it is atomic: two writes would leave a window
/// in which a reclaim hands the next step an answer that has already been acted
/// on, and it would act on it again. Writing into the checkpoint BLOB instead
/// was the alternative and it is worse — the checkpoint's shape is the step
/// author's, and a decision route reaching into it would be this file corrupting
/// a value only the step knows how to read.
pub async fn decide(args: DecideArgs, deps: &DecideDeps) -> Result<DecideResult, sqlx::Error> {
    let Some(run) = deps.run.store.get(&args.run_id).await? else {
        return Ok(DecideResult::Refused {
            reason: DecideRefusal::Missing,
            state: None,
        });
    };
    // TS checks state and question together: either way there is nothing parked
    // here to answer.
    let request = match run
        .decision
        .as_ref()
        .filter(|_| run.state == RunState::Awaiting)
    {
        Some(d) => d.request.clone(),
        None => {
            return Ok(DecideResult::Refused {
                reason: DecideRefusal::NotAwaiting,
                state: Some(run.state),
            });
        }
    };

    // ── The authority check, before anything else is said about the run ──────
    //
    // Ordered deliberately: nothing below this point tells the caller anything
    // about the question — not its options, not whether their answer was
    // well-formed — until they have been established as somebody who may see it.
    let Some(approval) = (deps.approval_for)(&run) else {
        // We could not build the approval, which means we could not establish
        // who may decide this run: an unregistered kind, a missing approval
        // key. FAIL CLOSED — the builder has already said which of those it
        // was, loudly.
        tracing::warn!(
            "{LOG} {} ({}): refused a decision from {} because this instance cannot say who may \
             decide it",
            run.id,
            run.kind,
            args.by
        );
        return Ok(DecideResult::Refused {
            reason: DecideRefusal::Forbidden,
            state: Some(run.state),
        });
    };
    let who = (deps.audience_for)(&approval.authority).await;
    if !may_decide(&who, &args.by) {
        return Ok(DecideResult::Refused {
            reason: DecideRefusal::Forbidden,
            state: Some(run.state),
        });
    }

    // ── The answer is DATA, and only the data the step offered ──────────────
    let Some(option) = request.options.iter().find(|o| o.id == args.option_id) else {
        let offered = request
            .options
            .iter()
            .map(|o| o.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            "{LOG} {} ({}): \"{}\" is not one of the options the step offered ({offered})",
            run.id,
            run.kind,
            args.option_id
        );
        return Ok(DecideResult::Refused {
            reason: DecideRefusal::UnknownOption,
            state: Some(run.state),
        });
    };

    let answer = DecisionAnswer {
        // Named from the ROW rather than from the request, so an answer can
        // only ever be about the question the run is parked on right now.
        // `store.answer` checks it again against the row it writes, which is
        // what actually closes the race with a second decider.
        key: request.key.clone(),
        option_id: option.id.clone(),
        // Free text from a person, clamped — TS at 2000 UTF-16 units, here at
        // 2000 bytes on a char boundary (the standing surrogate divergence). It
        // is a note for the step and for the audit trail — never an instruction
        // that widens what the run may do.
        note: args
            .note
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| clamp_text(n, 2000)),
        answered_by: Some(args.by.clone()),
        answered_at: epoch_ms_to_iso((deps.run.now)()),
    };

    // One write path back into the queue, publish and resume included — and it
    // is reachable from nowhere but this line, which is what makes the check
    // above an enforcement rather than a habit.
    let res = resume_answered(
        &args.run_id,
        answer.clone(),
        args.start != Some(false),
        deps,
    )
    .await?;
    if matches!(res, DecideResult::Refused { .. }) {
        return Ok(res);
    }

    // THE DECISION IS ALSO A BRIEF EVENT. Everyone in the content audience had
    // this approval on their brief (that is what the announcement put there);
    // the answer resolves those lines. Detached, because the decider is waiting
    // on this response and bookkeeping for other people's pages must not cost
    // them a millisecond — the nudge clears a throttle and the next read does
    // the work.
    let mut stale = who.content.clone();
    if !stale.contains(&args.by) {
        stale.push(args.by.clone());
    }
    let nudge = deps.mark_brief_stale.clone();
    tokio::spawn(async move {
        nudge(stale).await;
    });
    Ok(res)
}

/// What a person is being asked, for a surface that has the run row already.
/// None unless the run is parked — a decided or finished run has no question,
/// and a surface that rendered the last one would be showing a decision that
/// has already been made as if it were still open.
pub fn pending_question(run: &RunRow) -> Option<DecisionRequest> {
    let decision = run.decision.as_ref()?;
    if run.state == RunState::Awaiting && decision.answer.is_none() {
        Some(decision.request.clone())
    } else {
        None
    }
}

// ── Args ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct DecideArgs {
    pub run_id: String,
    pub option_id: String,
    pub note: Option<String>,
    /// The user id of the person deciding. Required: this function's entire job
    /// is to check that somebody specific is allowed to answer.
    ///
    /// `DecisionAnswer.answered_by` is nullable because an answer can
    /// legitimately come from something other than a person — a policy, a
    /// timeout rule — and an audit has to be able to tell those apart from a
    /// human having looked. Such a path does NOT come through here: it would
    /// need its own explicit authorization argument, and letting it share this
    /// door would mean the one function that enforces "a person may decide
    /// this" had a way to be called with no person.
    pub by: String,
    /// Resume immediately, detached. Default true; false leaves the run
    /// `queued` for the sweep, which is what a test wants.
    pub start: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::define::{DecisionOption, RunDefinition, StepResult};
    use serde_json::Value;

    fn row(state: RunState, decision: Option<RunDecision>, approval_key: Option<String>) -> RunRow {
        RunRow {
            id: "run-1".into(),
            kind: "unit-kind".into(),
            owner_user_id: Some("u-owner".into()),
            subject_type: Some("task".into()),
            subject_id: Some("board-1".into()),
            state,
            phase: "picking an assignee".into(),
            checkpoint: Value::Null,
            input: Value::Null,
            result: Value::Null,
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
            Some(Arc::new(RunDefinition {
                kind: kind.clone(),
                label: "Ticket handover".into(),
                step: Arc::new(|_| {
                    Box::pin(async {
                        Ok(StepResult::Done {
                            result: Value::Null,
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

    fn ask() -> DecisionRequest {
        DecisionRequest {
            key: "assignee".into(),
            question: "Who should take this ticket?".into(),
            detail: Some("Both are editors.".into()),
            options: vec![
                DecisionOption {
                    id: "ana".into(),
                    label: "Ana".into(),
                    detail: None,
                },
                DecisionOption {
                    id: "ben".into(),
                    label: "Ben".into(),
                    detail: None,
                },
            ],
            href: Some("/boards/board-1/task-1".into()),
        }
    }

    #[test]
    fn the_approval_key_is_the_run_and_the_question_and_nothing_else() {
        assert_eq!(
            run_approval_key("test-decision-run", "run-1", "assignee"),
            "run:test-decision-run:run-1:assignee"
        );
    }

    #[test]
    fn may_decide_is_the_content_half_only() {
        let d = Disclosure {
            content: vec!["u-editor".into()],
            fact: vec!["u-admin".into()],
        };
        assert!(may_decide(&d, "u-editor"));
        assert!(
            !may_decide(&d, "u-admin"),
            "the fact half may be told it is stuck, not decide it"
        );
        assert!(!may_decide(&d, "u-stranger"));
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
        assert!(run_decision_approval(&row(RunState::Running, parked(), None), &def).is_none());
        assert!(
            run_decision_approval(&row(RunState::Awaiting, None, Some("k".into())), &def).is_none()
        );
        assert!(run_decision_approval(&row(RunState::Awaiting, parked(), None), &def).is_none());
        assert!(
            run_decision_approval(
                &row(RunState::Awaiting, parked(), Some("k".into())),
                &nobody
            )
            .is_none()
        );
    }

    #[test]
    fn run_decision_approval_translates_the_row() {
        let ask = ask();
        let row = row(
            RunState::Awaiting,
            Some(RunDecision {
                request: ask,
                answer: None,
            }),
            Some("run:unit-kind:run-1:assignee".into()),
        );
        let approval = run_decision_approval(&row, &lookup("unit-kind")).expect("translates");

        assert_eq!(approval.kind, "run_decision");
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
    fn pending_question_is_none_unless_parked_unanswered() {
        let parked = row(
            RunState::Awaiting,
            Some(RunDecision {
                request: ask(),
                answer: None,
            }),
            Some("k".into()),
        );
        assert_eq!(
            pending_question(&parked).map(|q| q.key),
            Some("assignee".into())
        );

        let answered = row(
            RunState::Queued,
            Some(RunDecision {
                request: ask(),
                answer: Some(DecisionAnswer {
                    key: "assignee".into(),
                    option_id: "ana".into(),
                    note: None,
                    answered_by: Some("u-editor".into()),
                    answered_at: "t1".into(),
                }),
            }),
            None,
        );
        assert!(
            pending_question(&answered).is_none(),
            "a decided run has no open question"
        );
    }
}
