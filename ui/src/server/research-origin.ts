// WHERE A RESEARCH RUN WAS ASKED FOR, so the answer can go back there.
//
// THE HOLE THIS FILLS. `research` starts a detached run and hands the agent a
// runId; the tool description then tells it to POLL `research_status`. Inside a
// chat turn there is nothing to poll with — no sleep, no resume — so an agent
// asked to research something in conversation does the only thing it can: it
// checks two or three times in a row, sees `running`, and ends its turn. A brief
// takes minutes. Nothing ever woke it, and the person who asked sat looking at a
// reply that said "giving it a minute" until they typed "?".
//
// The completion path had exactly one signal, `addNotification(ownerUserId, …)`,
// and `ownerUserId` is the owner of a PERSONAL ASSISTANT. Every departmental
// agent — which is most of them — resolves it to null, so for those runs the
// branch never ran and literally nothing was told: not the agent, not the person,
// not the UI.
//
// WHY REDIS AND NOT A COLUMN. The link has to outlive the HTTP request that
// created it and no longer than the run itself, and `sweepStale` already declares
// what a run's lifetime is worth: an app restart marks every queued/running row
// `error: run went stale`. A key that dies with the process is therefore exactly
// as durable as the thing it points at, and a column would promise more than the
// pipeline delivers.
//
// THE TWO HALVES, and they are different questions:
//
//   the TURN     which conversation is this agent answering in right now. Written
//                by the chat route before it hands the turn to the container,
//                read moments later when the agent's tool call comes back. Short
//                TTL — it is a fact about an in-flight turn.
//   the ORIGIN   which conversation is owed the answer to this run. Written once
//                when the run is created, read when it finishes, possibly an hour
//                later on an expedition.
//
// AN AGENT ANSWERS ONE TURN AT A TIME, which is what makes the first half sound.
// A second turn for the same agent overwrites the key, and the loser is a run
// whose answer lands in the agent's other conversation — a misdelivery, not a
// leak, and it is bounded by the same agent and the same workspace.
import { getRedis } from './db/redis'

/** Long enough for an expedition, short enough that a stale key cannot outlive
 *  the process that would have read it. */
const ORIGIN_TTL_SECONDS = 2 * 60 * 60

/** One turn. Generous against a slow first tool call, far short of a second
 *  conversation with the same agent being mistaken for this one. */
const TURN_TTL_SECONDS = 15 * 60

const turnKey = (agentModel: string) => `agent-turn:${agentModel}`
const originKey = (runId: string) => `research-origin:${runId}`

/** This agent is now answering in this conversation. Called on the way INTO a
 *  turn, never on the way out — a tool call arrives while the turn is still
 *  open, which is the whole point.
 *
 *  Never throws. A research run that cannot be traced back to its chat is the
 *  behaviour this file improves on, not a reason to fail the chat turn itself. */
export async function markAgentTurn(agentModel: string, conversationId: string): Promise<void> {
  try {
    await getRedis().set(turnKey(agentModel), conversationId, 'EX', TURN_TTL_SECONDS)
  } catch {
    /* best effort — see above */
  }
}

/** The conversation this agent is mid-turn in, or null. */
export async function currentAgentTurn(agentModel: string): Promise<string | null> {
  try {
    return await getRedis().get(turnKey(agentModel))
  } catch {
    return null
  }
}

/** Remember that this run owes its answer to this conversation. */
export async function rememberResearchOrigin(runId: string, conversationId: string): Promise<void> {
  try {
    await getRedis().set(originKey(runId), conversationId, 'EX', ORIGIN_TTL_SECONDS)
  } catch {
    /* best effort */
  }
}

/** The conversation owed this run's answer, or null when it was started from
 *  the Research page, by a cron, or long enough ago that the key has expired.
 *  Null is an ordinary answer and every caller treats it as one. */
export async function researchOrigin(runId: string): Promise<string | null> {
  try {
    return await getRedis().get(originKey(runId))
  } catch {
    return null
  }
}

/** Forget it — the answer has been delivered, or the run died. Not required for
 *  correctness (the TTL collects it either way), but a delivered run should not
 *  leave a key naming a conversation for two hours. */
export async function forgetResearchOrigin(runId: string): Promise<void> {
  try {
    await getRedis().del(originKey(runId))
  } catch {
    /* best effort */
  }
}
