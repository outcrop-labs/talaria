// WHERE A RESEARCH RUN WAS ASKED FOR, so the answer can go back there.
// A Redis-only leaf with no database and no harness.
//
// THE HOLE THIS FILLS. `research` starts a detached run and hands the agent a
// runId; the tool description then tells it to POLL `research_status`. Inside
// a chat turn there is nothing to poll with — no sleep, no resume — so an
// agent asked to research something in conversation does the only thing it
// can: it checks two or three times in a row, sees `running`, and ends its
// turn. A brief takes minutes. Nothing ever woke it, and the person who asked
// sat looking at a reply that said "giving it a minute" until they typed "?".
//
// The completion path had exactly one signal, `addNotification(ownerUserId)`,
// and `ownerUserId` is the owner of a PERSONAL ASSISTANT. Every departmental
// agent — which is most of them — resolves it to null, so for those runs the
// branch never ran and literally nothing was told: not the agent, not the
// person, not the UI.
//
// WHY REDIS AND NOT A COLUMN. The link has to outlive the HTTP request that
// created it and no longer than the run itself. A key that dies with the
// process is exactly as durable as the thing it points at, and a column would
// promise more than the pipeline delivers.
//
// THE TWO HALVES, and they are different questions:
//
//   the TURN     which conversation this agent is answering in right now.
//                Written by the chat route on the way INTO a turn, read
//                moments later when the
//                agent's tool call comes back through here. Short TTL — it is
//                a fact about an in-flight turn. Ownership attribution reads
//                the same half (attribution.rs): a mid-turn creation belongs
//                to the human on the other end of that conversation.
//   the ORIGIN   which conversation is owed the answer to this run. Written
//                once when the run is created, read when it finishes,
//                possibly an hour later on an expedition.
//
// AN AGENT ANSWERS ONE TURN AT A TIME, which is what makes the first half
// sound. A second turn for the same agent overwrites the key, and the loser
// is a run whose answer lands in the agent's other conversation — a
// misdelivery, not a leak, and it is bounded by the same agent and the same
// workspace.
//
// NEVER THROWS. A research run that cannot be traced back to its chat is the
// behaviour this module improves on, not a reason to fail anything else:
// every fallible edge here answers its None.

use redis::AsyncCommands;

use crate::state::AppState;

/// Long enough for an expedition, short enough that a stale key cannot
/// outlive the process that would have read it.
const ORIGIN_TTL_SECONDS: u64 = 2 * 60 * 60;

/// One turn. Generous against a slow first tool call, far short of a second
/// conversation with the same agent being mistaken for this one.
const TURN_TTL_SECONDS: u64 = 15 * 60;

fn turn_key(agent_model: &str) -> String {
    format!("agent-turn:{agent_model}")
}

fn origin_key(run_id: &str) -> String {
    format!("research-origin:{run_id}")
}

/// This agent is now answering in this conversation. Called on the way INTO a
/// turn, never on the way out.
pub async fn mark_agent_turn(state: &AppState, agent_model: &str, conversation_id: &str) {
    if let Ok(mut conn) = state.redis().await
        && let Ok(()) = conn
            .set_ex(turn_key(agent_model), conversation_id, TURN_TTL_SECONDS)
            .await
    {}
}

/// The conversation this agent is mid-turn in, or None.
pub async fn current_agent_turn(state: &AppState, agent_model: &str) -> Option<String> {
    current_agent_turn_on(state.redis().await.ok(), agent_model).await
}

/// The turn half with the connection handed in — ownership attribution
/// (attribution::responsible_user_for) reads it from contexts that hold a
/// ConnectionManager rather than an AppState, the workbench spawn among them.
/// Same key, same never-throw answer: None is an ordinary reply.
pub async fn current_agent_turn_on(
    redis: Option<redis::aio::ConnectionManager>,
    agent_model: &str,
) -> Option<String> {
    let mut conn = redis?;
    let v: Option<String> = conn.get(turn_key(agent_model)).await.ok()?;
    v
}

/// Remember that this run owes its answer to this conversation.
pub async fn remember_research_origin(state: &AppState, run_id: &str, conversation_id: &str) {
    if let Ok(mut conn) = state.redis().await
        && let Ok(()) = conn
            .set_ex(origin_key(run_id), conversation_id, ORIGIN_TTL_SECONDS)
            .await
    {}
}

/// The conversation owed this run's answer, or None when it was started from
/// the Research page, by a cron, or long enough ago that the key has expired.
/// None is an ordinary answer and every caller treats it as one.
pub async fn research_origin(state: &AppState, run_id: &str) -> Option<String> {
    let mut conn = state.redis().await.ok()?;
    let v: Option<String> = conn.get(origin_key(run_id)).await.ok()?;
    v
}

/// Forget it — the answer has been delivered, or the run died. Not required
/// for correctness (the TTL collects it either way), but a delivered run
/// should not leave a key naming a conversation for two hours.
pub async fn forget_research_origin(state: &AppState, run_id: &str) {
    if let Ok(mut conn) = state.redis().await
        && let Ok(()) = conn.del(origin_key(run_id)).await
    {}
}
