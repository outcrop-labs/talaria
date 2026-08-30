// THE run contract. A long action DECLARES how it makes one unit of progress;
// it never owns its own durability, its own retry policy, its own "is this
// still mine" check, or its own answer to "what happens when a person has to
// decide something". Port of ui/src/server/runs/define.ts — pure by
// construction: types, one registry, and two predicates. No database, no
// Redis, no clock, so a definition can be written and the registry enumerated
// without booting Talaria.
//
// A RUN IS A SEQUENCE OF STEPS OVER A CHECKPOINT, and that is the whole
// reason resume means anything. Resuming is re-entering `step` with the last
// PERSISTED checkpoint — not replaying the run from zero.
//
// AT-LEAST-ONCE: say it out loud, because every author of a step has to hold
// it. A reclaimed run RE-ENTERS `step`. The checklist for anyone porting real
// work onto the runtime (from define.ts, where the person who needs it is the
// person writing the step):
//
//   1. THE STEP THAT RAN AND DID NOT CHECKPOINT. The core case: everything
//      the step did happens again. Split work so one step does ONE outward
//      effect and checkpoints immediately; where the effect cannot be undone,
//      get an idempotency handle from the far side INTO the checkpoint before
//      you use it.
//   2. THE STEP ABANDONED WHILE STILL RUNNING. max_step_ms and a lost lease
//      both abort by REJECTING the race, not by stopping the step. Honor
//      ctx.signal before every outward call; treat attempt > 0 as "somebody
//      may have got further than the checkpoint says".
//   3. THE PARK. decide writes the row, publishes, then announces. A crash in
//      between leaves an UNMARKED approval key the approvals sweep announces
//      later — the correct failure. Do not notify from a step; the question
//      IS the notification.
//   4. THE RE-ASK. The approval key is derived from the run and the request
//      key, so the SAME key dedupes. Hazard: a genuinely NEW question reusing
//      an old key inherits the old announcement mark. Vary the key.
//   5. THE ANSWER. ctx.decision clears in the same write as the consuming
//      step's checkpoint — only if that step checkpoints. A step that acts on
//      a decision and returns retry (or throws) is handed the same answer
//      again. Consume and checkpoint in the same step.
//   6. THE ENQUEUE. enqueue writes the row and starts a detached drive; the
//      runtime deduplicates nothing above the row. Pass a deterministic
//      opts.id when "one run per thing" is the rule.
//   7. THE TERMINAL WRITE. complete/fail are CAS on the lease and cannot
//      double-write — but anything a step does after its last outward effect
//      and before returning done can repeat. The last step should be small.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The approvals `Authority` — the same discriminator every "who may be told
/// about this thing" question in the product answers with. A run whose
/// audience was hand-rolled would be the fourth place that decides who may
/// read a ticket's contents. (Ported from approvals.ts; the census and the
/// announce machinery come with the rest of that module later in the batch.)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "by", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum Authority {
    /// These users and nobody else — the one person whose mailbox it is.
    User { user_ids: Vec<String> },
    /// Every admin. `on_board` narrows the CONTENT to the admins who can also
    /// see that board; omitted or null = org-wide, every admin.
    Admin { on_board: Option<String> },
    /// The board's EDITORS (`canEdit`). Admin is not a bypass: `boardRole` is
    /// membership only.
    Board { board_id: String },
    /// No route in the product can decide this. Resolves to nobody; the stall
    /// is reported to the admins without the content.
    Nobody,
}

/// The lifecycle. Six states, and `awaiting` is the point: a run PARKED on a
/// human decision — not failed (nothing is wrong), not running (nothing is
/// burning), not queued (no amount of driving advances it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Queued,
    Running,
    Awaiting,
    Done,
    Error,
    Cancelled,
}

/// One row of the `runs` table, as every consumer reads it. Timestamps are
/// ISO strings rather than DB types: this row is published over SSE and
/// rendered on a device that never talked to Postgres.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRow {
    pub id: String,
    /// The definition that drives it — the key into the registry.
    pub kind: String,
    /// Whose run it is. Null for org-wide work with no one person behind it.
    pub owner_user_id: Option<String>,
    /// What the run is ABOUT ('task', 'channel', 'conversation', 'research',
    /// 'board'). Free text on purpose — a check constraint here would make
    /// every port a migration.
    pub subject_type: Option<String>,
    pub subject_id: Option<String>,
    pub state: RunState,
    /// Human-readable progress, written by `ctx.log(phase)`. The sentence a
    /// person reads while they wait; not a state machine and nothing branches
    /// on it.
    pub phase: String,
    /// The last PERSISTED checkpoint. Null before the first `next`.
    pub checkpoint: Value,
    pub input: Value,
    pub result: Value,
    pub error: Option<String>,
    /// How many times a driver has ENTERED this run. Incremented on RECLAIM
    /// only — see `max_attempts`.
    pub attempt: i32,
    /// The driver token that currently owns it, and until when. Mirrored
    /// from the Redis lease so the reclaim sweep is a plain indexed SQL scan:
    /// Redis has no index over "every run whose lease expired".
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<String>,
    /// The approvals key while this run is `awaiting`, so the existing
    /// announce and nag machinery dedupes on it exactly like every other
    /// pending decision. Null in every other state.
    pub approval_key: Option<String>,
    /// The question this run is parked on, and the answer once somebody gives
    /// it. IT IS A COLUMN AND NOT A CLOSURE: a decision that lives only in
    /// the process that asked it is the whole disease — park a run on one
    /// instance, open the approval on your phone, and the question is gone.
    pub decision: Option<RunDecision>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// One thing a person may pick. `id` is what the answer carries back and the
/// step branches on; `label` is the button.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionOption {
    pub id: String,
    pub label: String,
    /// The consequence, in one line. Shown under the button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// What a run needs a human to decide — shaped like `PendingApproval`, not
/// like a form: the product already has one place where a person is asked
/// something. A run that invented a second one would be a second inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRequest {
    /// Stable WITHIN the run, and re-askable: a step that returns `decide`
    /// again with the same key after a reclaim is the same question. This is
    /// what makes the pause idempotent under at-least-once delivery.
    pub key: String,
    /// One line. Goes in a notification title and a list item.
    pub question: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub options: Vec<DecisionOption>,
    /// In-app path to the surface that can actually decide it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionAnswer {
    /// The `DecisionRequest.key` this answers. Checked on the way in: an
    /// answer to last week's question must not resume a run parked on this
    /// week's.
    pub key: String,
    pub option_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Null when the answer came from something other than a person (a
    /// policy, a timeout rule) — allowed, and exactly what an audit needs to
    /// tell apart from a human having looked.
    pub answered_by: Option<String>,
    pub answered_at: String,
}

/// The `decision` column: the question, and the answer when there is one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunDecision {
    pub request: DecisionRequest,
    pub answer: Option<DecisionAnswer>,
}

/// What one call to `step` can say. Four outcomes, and the difference between
/// the last two is the difference between bothering a person and not.
#[derive(Debug, Clone)]
pub enum StepResult {
    /// Progress. The driver PERSISTS this checkpoint and then continues; a
    /// step that returns `next` with the checkpoint it was given is a loop.
    Next {
        checkpoint: Value,
        phase: Option<String>,
    },
    Done {
        /// Value::Null for "no result" (TS `result?: unknown`).
        result: Value,
    },
    /// PAUSE. The run parks in `awaiting`, an approval is filed under
    /// `approval_key`, and whoever `audience` names is told. Nothing burns
    /// and nothing is lost while it waits.
    Decide { question: DecisionRequest },
    /// A SOFT pause: come back in `after`. Nobody is notified, no attempt is
    /// consumed, the run stays `queued` — "the rate limit says wait", not
    /// "something went wrong". Distinct from error so a throttled run never
    /// burns its attempts on the throttle.
    Retry { after: Duration, reason: String },
}

/// The Rust spelling of a THROWN STEP: TS's `step` signals failure by throwing
/// and the driver files the message on an error row; here it returns `Err` and
/// the driver does the same with the text. A step that merely wants to come
/// back later returns `Retry`; `Err` is always terminal for this entry.
pub type StepError = String;

/// The abort signal a step is given. Fired when the run exceeds
/// `max_step_ms`, when the driver loses its lease, and when the process is
/// shutting down — a step that ignores it is a step the driver has to abandon
/// while it is still running.
#[derive(Debug, Clone)]
pub struct StepSignal {
    rx: tokio::sync::watch::Receiver<bool>,
}

impl StepSignal {
    pub fn channel() -> (tokio::sync::watch::Sender<bool>, Self) {
        let (tx, rx) = tokio::sync::watch::channel(false);
        (tx, Self { rx })
    }

    /// A fresh receiver of an EXISTING abort sender — the driver owns one
    /// sender for the whole drive and subscribes each step to it, so a lease
    /// lost mid-drive reaches whichever step is in flight.
    pub(crate) fn from_sender(tx: &tokio::sync::watch::Sender<bool>) -> Self {
        Self { rx: tx.subscribe() }
    }

    pub fn is_aborted(&self) -> bool {
        *self.rx.borrow()
    }

    /// A second handle to the same abort channel — for a step that hands the
    /// signal to a helper (the backfill page checks it before every outward
    /// call, the way TS threads `signal` into `indexPage`). Cloning the
    /// receiver, not the shared one: each handle eats its own `changed()` mark.
    pub fn share(&self) -> StepSignal {
        StepSignal {
            rx: self.rx.clone(),
        }
    }

    /// Resolves when aborted (or immediately if it already was). Takes `&self`
    /// — a step holds `ctx.signal` by reference — so the cloned receiver eats
    /// the `changed()` mark instead of the shared one.
    pub async fn aborted(&self) {
        if self.is_aborted() {
            return;
        }
        let mut rx = self.rx.clone();
        let _ = rx.changed().await;
    }
}

/// Everything a step is given. Note what is NOT here: no database handle, no
/// Redis, no way to write its own row. A step advances a checkpoint and does
/// its own domain work; the driver owns persistence, and a step that reached
/// around it would be re-inventing the file it is running inside.
pub struct RunStepContext {
    /// The row as it was read at THIS step boundary — already re-read, so
    /// `run.state` is fresh and a cancellation is visible here.
    pub run: RunRow,
    pub input: Value,
    /// The last PERSISTED checkpoint; null on the first step. Not "the
    /// checkpoint I returned last time" — if the last step returned next and
    /// the write did not land, this is the one before it. The at-least-once
    /// contract, stated in a type.
    pub checkpoint: Value,
    /// The answer to the question this run was parked on, if it was parked
    /// and answered. Cleared once the consuming step checkpoints.
    pub decision: Option<DecisionAnswer>,
    pub signal: StepSignal,
    /// Say what is happening, in words a waiting human would accept.
    /// Persisted and published (in that order); safe to call as often as
    /// makes sense, and the driver awaits the tail at the next boundary so a
    /// log can never outrun the checkpoint it describes.
    pub log: Arc<dyn Fn(String) + Send + Sync>,
    /// Which entry into this run this is. 0 on the first; incremented on each
    /// RECLAIM. A step that must not repeat a side effect can look at this
    /// and refuse — not a substitute for a real guard, but enough to say "I
    /// have been here before" out loud.
    pub attempt: i32,
}

pub type StepFn = Arc<
    dyn Fn(
            RunStepContext,
        ) -> futures_util::future::BoxFuture<'static, Result<StepResult, StepError>>
        + Send
        + Sync,
>;
pub type AudienceFn = Arc<dyn Fn(&RunRow) -> Authority + Send + Sync>;

/// The registry entry — TS's `RunDefinition<I, C>` with the type parameters
/// erased. The driver learns the kind from a row and cannot know the input or
/// checkpoint types at that point: the types are the STEP AUTHOR's guarantee
/// about their own column, and the driver moves those columns without ever
/// looking inside them.
pub struct RunDefinition {
    /// The registry key, and the `kind` column. Stable forever: it is written
    /// into every row this definition has ever produced, and a rename
    /// orphans every one of them mid-flight.
    pub kind: String,
    /// What a person sees this called.
    pub label: String,
    /// ONE UNIT OF PROGRESS, called with the LAST PERSISTED CHECKPOINT.
    /// IT MUST BE RE-ENTERABLE — see the checklist in the header.
    pub step: StepFn,
    /// Who may SEE and DECIDE this run when it pauses.
    pub audience: AudienceFn,
    /// How long ONE step may take before the driver abandons it. A budget,
    /// not a timeout knob: it is also the lease TTL, so a crashed driver's
    /// run is reclaimable roughly this long after it stops renewing.
    pub max_step_ms: u64,
    /// How many times a driver may ENTER this run before it is given up on.
    /// IT COUNTS ENTRIES, NOT STEPS: a healthy run that takes four hundred
    /// steps must not exhaust its attempts by succeeding. The counter moves
    /// only on RECLAIM — only when a previous driver died holding this run —
    /// so it measures exactly "this run kills the process that touches it".
    pub max_attempts: u32,
}

/// Three attempts: the crash, the retry, and one more for the deploy that
/// happened to land in the middle of the retry. A fourth entry into a run
/// that has killed three drivers is a bug report, not a retry.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

// ── The registry ──────────────────────────────────────────────────────────────
//
// `drive(run_id)` reads a row, gets a `kind`, and needs the code that advances
// it. The alternative — passing the definition in at every drive site — puts
// the reclaim sweep in the impossible position of importing every definition
// in the product to recover a row it found by scanning.

fn registry() -> &'static Mutex<HashMap<String, Arc<RunDefinition>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<RunDefinition>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Declare a run kind. Called at module load by the module that owns the
/// work, so the definition lives next to the thing it drives.
pub fn register_run(def: RunDefinition) -> Arc<RunDefinition> {
    let arc = Arc::new(def);
    let mut reg = registry().lock().unwrap();
    if let Some(existing) = reg.get(&arc.kind) {
        // Two modules claiming one kind is a real bug and a quietly replaced
        // definition is the worst version of it: rows written by the first
        // get driven by the second's step, with a checkpoint shaped for
        // neither. Keep the first, say so loudly.
        tracing::error!(
            "[runs] kind \"{}\" registered twice — keeping the first registration. This is a bug.",
            arc.kind
        );
        return existing.clone();
    }
    reg.insert(arc.kind.clone(), arc.clone());
    arc
}

/// The definition for a row's `kind`, or None when nothing in this process
/// knows how to advance it — a REAL state, not an impossible one: a row
/// enqueued by a newer deploy, or a kind whose module is not loaded here. The
/// driver reports it rather than failing the run, because an instance that
/// cannot drive a run is not the same as a run that cannot be driven.
pub fn run_definition(kind: &str) -> Option<Arc<RunDefinition>> {
    registry().lock().unwrap().get(kind).cloned()
}

/// Every registered kind, for the admin surface and the cross-check that
/// every persisted `kind` still has code behind it.
pub fn run_definitions() -> Vec<Arc<RunDefinition>> {
    registry().lock().unwrap().values().cloned().collect()
}

// ── Small shared helpers ──────────────────────────────────────────────────────

/// A run is FINISHED when nothing will move it again without a person.
pub fn is_terminal(state: RunState) -> bool {
    matches!(
        state,
        RunState::Done | RunState::Error | RunState::Cancelled
    )
}

/// A run a driver may pick up. `awaiting` is deliberately NOT drivable: the
/// thing it is waiting for is an answer, and driving it would re-ask the
/// question it is already parked on.
pub fn is_drivable(state: RunState) -> bool {
    matches!(state, RunState::Queued | RunState::Running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_serializes_as_the_ts_wire_strings() {
        assert_eq!(
            serde_json::to_string(&RunState::Queued).unwrap(),
            "\"queued\""
        );
        assert_eq!(
            serde_json::to_string(&RunState::Awaiting).unwrap(),
            "\"awaiting\""
        );
        assert_eq!(
            serde_json::to_string(&RunState::Cancelled).unwrap(),
            "\"cancelled\""
        );
        assert_eq!(
            serde_json::from_str::<RunState>("\"error\"").unwrap(),
            RunState::Error
        );
        assert!(serde_json::from_str::<RunState>("\"paused\"").is_err());
    }

    #[test]
    fn the_decision_column_round_trips_the_ts_shape() {
        // The exact JSON a TS-parked run stores in `runs.decision` — camelCase
        // keys, optional fields omitted — must read and write back identically.
        let ts = r#"{"request":{"key":"cite","question":"Which source?","options":[{"id":"a","label":"A"}],"href":"/r/1"},"answer":{"key":"cite","optionId":"a","answeredBy":null,"answeredAt":"2026-08-29T00:00:00.000Z"}}"#;
        let d: RunDecision = serde_json::from_str(ts).unwrap();
        assert_eq!(d.request.key, "cite");
        assert_eq!(d.request.detail, None);
        assert_eq!(d.answer.as_ref().unwrap().option_id, "a");
        assert_eq!(serde_json::to_string(&d).unwrap(), ts);
    }

    #[test]
    fn authority_is_the_approvals_discriminator() {
        let a = serde_json::json!({"by": "admin", "onBoard": "b1"});
        let auth: Authority = serde_json::from_value(a).unwrap();
        assert_eq!(
            auth,
            Authority::Admin {
                on_board: Some("b1".into())
            }
        );
        assert_eq!(
            serde_json::to_value(Authority::Nobody).unwrap(),
            serde_json::json!({"by": "nobody"})
        );
        assert_eq!(
            serde_json::to_value(Authority::User {
                user_ids: vec!["u1".into()]
            })
            .unwrap(),
            serde_json::json!({"by": "user", "userIds": ["u1"]})
        );
    }

    #[test]
    fn double_registration_keeps_the_first() {
        // The registry is global; use a kind no other test registers. The
        // Arc::ptr_eq check is the point: the second registration is REFUSED,
        // not merged.
        let first = register_run(RunDefinition {
            kind: "test-double".into(),
            label: "first".into(),
            step: Arc::new(|_| {
                Box::pin(async {
                    Ok(StepResult::Done {
                        result: Value::Null,
                    })
                })
            }),
            audience: Arc::new(|_| Authority::Nobody),
            max_step_ms: 1_000,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        });
        let second = register_run(RunDefinition {
            kind: "test-double".into(),
            label: "second".into(),
            step: Arc::new(|_| {
                Box::pin(async {
                    Ok(StepResult::Done {
                        result: Value::Null,
                    })
                })
            }),
            audience: Arc::new(|_| Authority::Nobody),
            max_step_ms: 9_999,
            max_attempts: 9,
        });
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(run_definition("test-double").unwrap().label, "first");
        assert!(run_definitions().iter().any(|d| d.kind == "test-double"));
    }

    #[test]
    fn awaiting_is_parked_not_dead_not_drivable() {
        assert!(!is_terminal(RunState::Awaiting));
        assert!(!is_drivable(RunState::Awaiting));
        assert!(is_drivable(RunState::Queued));
        assert!(is_drivable(RunState::Running));
        for s in [RunState::Done, RunState::Error, RunState::Cancelled] {
            assert!(is_terminal(s));
            assert!(!is_drivable(s));
        }
    }

    #[tokio::test]
    async fn the_signal_fires_and_stays_fired() {
        let (tx, signal) = StepSignal::channel();
        assert!(!signal.is_aborted());
        tx.send(true).unwrap();
        assert!(signal.is_aborted());
        signal.aborted().await; // resolves immediately once fired
    }
}
