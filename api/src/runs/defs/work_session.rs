// AGENT WORK SESSIONS, as a durable run — the port of
// ui/src/server/runs/defs/work-session.ts, grown slice by slice.
//
// WHAT HAS CROSSED is the row's IDENTITY: the `kind`, the DERIVED run id that
// turns "one live session per ticket+agent" from a process-local promise into
// a primary key, and the registration that makes a Rust-written row legible
// to the registry. The STEP MACHINE — the three-stage send/record/failed
// loop, the turn cap, the interrupted-turn retirement — runs a harness turn,
// so it crosses with the harness plane at the scheduler handover. Until then
// this process WRITES work-session rows and never drives one: `dispatch`
// enqueues with `start: false`, and the TS sweep — which still owns the
// scheduler and still has the real step — picks the row up and runs it.
//
// THE COEXISTENCE REGISTRATION, then, is not a stub by omission. The
// definition carries the real metadata (label, audience, eleven-minute step
// budget, five attempts) because those describe the ROW, and the registry,
// and the approvals census — all of which read them in this process today.
// The STEP is a refusal that names the posture, because a Rust driver that
// reached it would be a driver armed before its plane crossed: the loud
// failure is the correct one, and it says so in the sentence it fails with.
//
// THE SESSION ITSELF IS A DURABLE RUN. What used to be a `for` loop inside a
// fire-and-forget promise, guarded by a process-local `Set`, is a run: the
// loop index is a checkpoint, the guard is a Redis lease, and a driver that
// dies is reclaimed and re-enters at the turn it left. See work-dispatch.ts's
// header for the push side of that story.

use crate::runs::define::{Authority, RunDefinition, RunRow, StepResult, register_run};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};

/// The registry key, and the `kind` column on every row this definition has
/// ever produced. Stable forever — a rename orphans every session mid-flight.
pub const WORK_SESSION_KIND: &str = "work-session";

// ── The row's identity ───────────────────────────────────────────────────────

/// THE run id for one session, derived and not random.
///
/// `enqueue` deduplicates nothing above the row, so a caller that retries its
/// request — or a second instance handling the same ticket mutation — creates
/// a SECOND run doing the same work. That is precisely the bug `liveSessions`
/// was written to prevent, and the runtime's answer to it is a deterministic
/// id: the primary key refuses the duplicate, so a double dispatch produces
/// one run and one loser that reads its own rejection as "somebody already
/// has this".
///
/// `generation` is what keeps that claim from becoming a life sentence. The
/// invariant is ONE LIVE SESSION per ticket+agent, not one ever: a ticket
/// that legitimately comes back to the same agent next week needs a new row,
/// and a row whose id is a pure function of (task, agent) has nowhere to put
/// it. So `dispatch_ticket_work` walks generations, skipping the finished
/// ones and standing down on the first live one it finds.
///
/// A NAME-BASED UUID rather than a hash string, because `runs.id` is `uuid`.
/// Version 8 is RFC 9562's "custom" — an honest label for "the bytes are a
/// SHA-256 of a name", which is what they are; v5 would claim SHA-1.
///
/// PINNED to the TS implementation by vectors generated from
/// runs/defs/work-session.ts itself — both runtimes must derive the SAME id
/// from the same ticket or the coexistence claim (Rust enqueues, TS drives)
/// silently splits one session into two.
pub fn session_run_id(task_id: &str, agent_model: &str, generation: u32) -> String {
    let digest = Sha256::digest(format!(
        "{WORK_SESSION_KIND}\u{0}{task_id}\u{0}{agent_model}\u{0}{generation}"
    ));
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0f) | 0x80; // version 8
    b[8] = (b[8] & 0x3f) | 0x80; // variant 10x
    let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

// ── The definition ───────────────────────────────────────────────────────────

/// The board's editors, which is who the ticket routes every other decision
/// to. Never the run's owner (there isn't one) and never the agent: a session
/// that could name its own audience would be an agent deciding who supervises
/// it. `nobody` when the input cannot name a board, which is a real stall to
/// report rather than a reason to widen.
fn audience(run: &RunRow) -> Authority {
    match run
        .input
        .get("boardId")
        .and_then(|v| v.as_str())
        .filter(|b| !b.is_empty())
    {
        Some(board_id) => Authority::Board {
            board_id: board_id.to_string(),
        },
        None => Authority::Nobody,
    }
}

/// The registered definition, exactly once per process — TS registers at
/// module load; the Rust equivalent is the first call, which `dispatch` makes
/// before any enqueue, so the row's kind is always registered before it is
/// written. The returned `&'static Arc` is the same one the registry holds.
pub fn work_session_run() -> &'static Arc<RunDefinition> {
    static DEF: OnceLock<Arc<RunDefinition>> = OnceLock::new();
    DEF.get_or_init(|| {
        register_run(RunDefinition {
            kind: WORK_SESSION_KIND.into(),
            label: "Agent work session".into(),
            // THE STEP HAS NOT CROSSED. One step is one harness TURN, and the
            // harness is still TS; a Rust driver that reached this step would
            // be the scheduler armed early. Fail the step with the posture
            // rather than pretending to advance — the run's error column is
            // read by a person, and the sentence says what actually happened.
            step: Arc::new(|_ctx| {
                Box::pin(async {
                    Err::<StepResult, String>(
                        "work-session turns run in the TypeScript harness during coexistence; \
                         this Rust step was reached by a driver armed before the harness plane \
                         crossed"
                            .into(),
                    )
                })
            }),
            audience: Arc::new(audience),
            // ELEVEN MINUTES, because one step is one turn and one turn is
            // one call to the work-session harness, whose hold is ten: an
            // agent restarting under a config propagation refuses
            // connections for tens of seconds and a fleet re-render
            // mid-session must not kill the session. This is also the lease
            // TTL, so a session whose instance died is reclaimable about
            // eleven minutes later — the right answer, not an unfortunate
            // one: coming back sooner would reclaim turns that are
            // genuinely still running.
            max_step_ms: 660_000,
            // FIVE, against the default three, and the reason is duration.
            // `attempt` counts drivers that DIED holding this run, and a
            // work session is the longest-lived run in the product — up to
            // twelve model turns of up to ten minutes each — so it is the
            // one kind that routinely spans more than one deploy. Three
            // would file a healthy session as an error for the crime of a
            // busy release day. The count is still bounded, and it is
            // self-limiting in a way no other kind's is: every reclaim
            // retires the turn it interrupted, so a session that keeps
            // killing drivers spends its turn budget doing it.
            max_attempts: 5,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Generated by importing the TS module itself (bun -e over
    // runs/defs/work-session.ts) — not by re-deriving them here, which would
    // test the port against the port.
    #[test]
    fn session_run_id_matches_the_ts_derivation() {
        assert_eq!(
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 0),
            "b1715287-c692-8f1e-9255-ea7ba786880b"
        );
        assert_eq!(
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 1),
            "a3cd749a-6cbe-87dd-a6f7-1495296efdce"
        );
        // The generation is in the name: same pair, different session.
        assert_ne!(
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 0),
            session_run_id("11111111-1111-1111-1111-111111111111", "claude-opus-4-5", 1)
        );
        assert_eq!(
            session_run_id("22222222-2222-2222-2222-222222222222", "gpt-5.3", 24),
            "3c589dd9-691b-8cc5-9e8b-03a4d26b9675"
        );
        assert_eq!(
            session_run_id("11111111-1111-1111-1111-111111111111", "gpt-5.3", 0),
            "1a25157f-29df-8201-a2ef-c381add3169e"
        );
    }

    #[test]
    fn the_id_is_a_version_8_uuid_shape() {
        let id = session_run_id("t", "a", 0);
        // 8-4-4-4-12 lowercase hex, version nibble 8, variant 10x.
        assert_eq!(id.len(), 36);
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert!(id.chars().all(|c| !c.is_ascii_uppercase()));
        assert!(parts[2].starts_with('8'));
        assert!(
            parts[3].starts_with('8')
                || parts[3].starts_with('9')
                || parts[3].starts_with('a')
                || parts[3].starts_with('b')
        );
    }

    #[test]
    fn registration_carries_the_real_metadata_once() {
        let def = work_session_run();
        assert_eq!(def.kind, WORK_SESSION_KIND);
        assert_eq!(def.label, "Agent work session");
        assert_eq!(def.max_step_ms, 660_000);
        assert_eq!(def.max_attempts, 5);
        // The same Arc every time — register_run is once per process, and a
        // second registration would be the bug define.rs refuses.
        assert!(Arc::ptr_eq(def, work_session_run()));
        // The audience reads the input's boardId, never the owner.
        let mut row = minimal_row();
        assert!(matches!(audience(&row), Authority::Nobody));
        row.input = serde_json::json!({"boardId": "b-1"});
        assert_eq!(
            audience(&row),
            Authority::Board {
                board_id: "b-1".into()
            }
        );
        // TS's truthiness: an empty boardId string is nobody's, not a board
        // named "".
        row.input = serde_json::json!({"boardId": ""});
        assert!(matches!(audience(&row), Authority::Nobody));
    }

    #[tokio::test]
    async fn the_step_refuses_with_the_coexistence_posture() {
        let def = work_session_run();
        let err = (def.step)(minimal_row_ctx()).await.unwrap_err();
        assert!(err.contains("coexistence"), "{err}");
        assert!(err.contains("harness"), "{err}");
    }

    // ── Row/ctx helpers ──────────────────────────────────────────────────────

    fn minimal_row() -> crate::runs::define::RunRow {
        use crate::runs::define::{RunRow, RunState};
        RunRow {
            id: "r-1".into(),
            kind: WORK_SESSION_KIND.into(),
            owner_user_id: None,
            subject_type: Some("task".into()),
            subject_id: Some("t-1".into()),
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

    fn minimal_row_ctx() -> crate::runs::define::RunStepContext {
        use crate::runs::define::RunStepContext;
        let (tx, signal) = crate::runs::define::StepSignal::channel();
        drop(tx);
        RunStepContext {
            run: minimal_row(),
            input: serde_json::Value::Null,
            checkpoint: serde_json::Value::Null,
            decision: None,
            signal,
            log: Arc::new(|_| {}),
            attempt: 0,
        }
    }
}
