// Real-time fan-out over Redis pub/sub → SSE. Mutations publish a small event
// to a topic (`board:<id>`, `channel:<id>`, `run:<id>`, `user:<id>`); each
// connected client holds an SSE stream fed by a dedicated Redis subscriber.
// Multiplayer without websockets.
//
// THE RULE THAT MAKES THIS SAFE, and it is worth stating once at the top
// because two new topics below are the first ones carrying a thing a person
// might not be allowed to read: AN EVENT SAYS WHAT CHANGED, NEVER WHAT IT SAYS.
// The client re-fetches through the ordinary route, which applies the ordinary
// read ACL. A payload that carried the content would be a second read path with
// no gate on it — the fan-out would become the disclosure.
import Redis from 'ioredis'
import { getRedis } from './db/redis'
import type { RunRow, RunState } from './runs/define'

export interface BoardEvent {
  type: 'task' | 'comment' | 'board'
  taskId?: string
  deleted?: boolean
}

export function publishBoard(boardId: string, event: BoardEvent): void {
  void getRedis().publish(`board:${boardId}`, JSON.stringify(event))
}

export interface ChannelEvent {
  type: 'message' | 'channel'
  messageId?: string
  seq?: number
  deleted?: boolean
}

export function publishChannel(channelId: string, event: ChannelEvent): void {
  void getRedis().publish(`channel:${channelId}`, JSON.stringify(event))
}

/** An SSE ReadableStream of a board's events (own Redis subscriber per client). */
export function boardEventStream(boardId: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  return topicEventStream(`board:${boardId}`, signal)
}

/** An SSE ReadableStream of a chat channel's events. */
export function channelEventStream(channelId: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  return topicEventStream(`channel:${channelId}`, signal)
}

// ── Runs: `run:<id>` and `user:<id>` ─────────────────────────────────────────
//
// TWO TOPICS BECAUSE THERE ARE TWO QUESTIONS, and they have different answers
// about who may listen:
//
//   run:<id>    "how is THIS run doing" — a detail view, a progress strip. Its
//               gate is the run's READ ACL (see `mayWatchRun`).
//   user:<id>   "what do I have in flight" — the per-user firehose a SECOND
//               DEVICE attaches to. Its gate is identity: it is your own id and
//               nobody else's, which is the only reason it can be one topic per
//               person rather than one per thing.
//
// The second one is the point of the whole file for runs. A run is durable
// server state, so "I started it on my laptop and I am now on my phone" has to
// be a live view and not a poll — and it must update for a run the person is
// NOT looking at, because the transition that matters most (`awaiting`: this
// run has stopped and is asking you something) is exactly the one that happens
// while their attention is elsewhere.
//
// NOTHING PUBLISHES TO `user:<id>` YET EXCEPT RUNS. It is deliberately named
// for the PERSON and not for runs, because notifications belong here too:
// today `addNotification` publishes to nothing at all and the bell polls every
// 30 seconds for a row that has been sitting in Postgres since. That is the
// next thing to land on this topic, and `UserEvent` already carries the
// discriminant for it.

/** What goes over `run:<id>`.
 *
 *  Deliberately the run's OWN fields and nothing else. Compare `RunEvent` in
 *  server/runs/run.ts, which additionally carries the `DecisionRequest` a run
 *  parked on — that one field is NOT published here and must not be:
 *
 *  WHO MAY READ THE QUESTION IS A DIFFERENT QUESTION FROM WHO MAY WATCH THE
 *  RUN. A decision's text is governed by the definition's `audience`, an
 *  approvals `Authority`; this stream is gated by the run's read ACL, which is
 *  its owner plus whoever may read its SUBJECT. Those overlap in the common
 *  case and they are not the same set — a run about a board whose audience is
 *  `{ by: 'user' }` would hand every board member the question's `detail` if
 *  this payload carried it. A device that sees `state: 'awaiting'` re-fetches
 *  the run through the route that resolves the audience properly, and pays one
 *  round trip on the rare transition rather than routing content around the
 *  resolver on every transition. */
export interface RunEvent {
  type: 'run'
  runId: string
  kind: string
  state: RunState
  /** The human sentence a waiting person reads. Governed by the same read ACL
   *  as the run itself, which is what this topic is gated on. */
  phase: string
  error?: string
}

/** What goes over `user:<id>`. STRICTLY ID-SHAPED — ids, and closed enums that
 *  are ids of a sort. No phase, no error text, no question.
 *
 *  Not because this topic is less trusted (it is the person's own firehose)
 *  but because it is the one place a fan-out could quietly acquire a second
 *  disclosure rule: a `run` event here is published for the run's OWNER, while
 *  a run's content is governed by its audience, and the day something publishes
 *  a shared run to a participant's user topic the payload must already be
 *  incapable of leaking. Keeping it id-shaped from the start means that day is
 *  a routing change and not a disclosure review. */
export type UserEvent =
  | { type: 'run'; runId: string; state: RunState }
  /** Reserved, and the reason this topic is named for the person: the bell
   *  currently polls because nothing publishes when a notification is written. */
  | { type: 'notification'; notificationId: string }
  /** Something was appended to the person's daily brief.
   *
   *  ID-SHAPED LIKE EVERY OTHER EVENT HERE, and on this topic that is not a
   *  formality: the brief is the densest private thing in the product — one
   *  person's approvals, blocked work and unread DMs on a single page — so an
   *  event carrying so much as a title would make the fan-out a second read
   *  path with no ACL on it. `seq` is the append cursor, which is all a client
   *  needs to decide whether the page it is holding is behind. */
  | { type: 'brief'; briefId: string; seq: number }

/** Publish a run transition to `run:<id>`.
 *
 *  THE PAYLOAD IS BUILT FIELD BY FIELD, not `JSON.stringify(event)`. TypeScript
 *  does not stop a caller passing a WIDER object than the parameter type — only
 *  object literals get excess-property checking, and every real call site here
 *  will be handing over a variable it got from somewhere else. Serializing a
 *  fixed field list is the only version of "this event carries no content" that
 *  survives a caller who is holding the whole row. */
export function publishRun(runId: string, event: RunEvent): void {
  const wire: RunEvent = {
    type: 'run',
    runId,
    kind: event.kind,
    state: event.state,
    phase: event.phase,
    ...(event.error === undefined ? {} : { error: event.error }),
  }
  void getRedis()
    .publish(`run:${runId}`, JSON.stringify(wire))
    // Not swallowed. A fan-out that fails silently is how a device sits on a
    // stale run forever while the row underneath it finished ten minutes ago.
    .catch((e: unknown) => console.error(`[realtime] publish run:${runId} failed:`, e))
}

/** Publish to a person's own firehose. Same explicit serialization, same
 *  reason — and here it is load-bearing rather than defensive, because the
 *  natural call site is "I already have a RunEvent, forward it". */
export function publishUser(userId: string, event: UserEvent): void {
  const wire: UserEvent =
    event.type === 'run'
      ? { type: 'run', runId: event.runId, state: event.state }
      : event.type === 'brief'
        ? { type: 'brief', briefId: event.briefId, seq: event.seq }
        : { type: 'notification', notificationId: event.notificationId }
  void getRedis()
    .publish(`user:${userId}`, JSON.stringify(wire))
    .catch((e: unknown) => console.error(`[realtime] publish user:${userId} failed:`, e))
}

/** An SSE ReadableStream of one run's events. Gate the caller with
 *  `mayWatchRun` BEFORE calling this — see the note there. */
export function runEventStream(runId: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  return topicEventStream(`run:${runId}`, signal)
}

/** An SSE ReadableStream of one person's own firehose. The only legitimate
 *  argument is the authenticated caller's own id. */
export function userEventStream(userId: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  return topicEventStream(`user:${userId}`, signal)
}

// ── Who may watch a run ──────────────────────────────────────────────────────
//
// THIS IS THE WHOLE RISK OF THE TWO NEW TOPICS. `user:<id>` gates itself — it
// is the caller's own id. `run:<id>` does not: a run id is an opaque string a
// caller can guess at or hold onto after losing access, and the topic name
// carries no authority of its own.
//
// IT DERIVES FROM WHAT ALREADY DECIDES, and invents nothing:
//
//   the run's OWNER            your own run is yours to watch
//   the run's SUBJECT         `boardRole` for a board or a task, `channelRole`
//                              for a channel, `accessibleConversation` for a
//                              conversation — the same predicates the routes
//                              that SERVE those subjects already call
//
//   ORG-WIDE WORK          no owner AND no subject — an ADMIN. See below.
//
// and where it cannot answer — a subject type nobody taught it — IT REFUSES.
// That direction is not a coin flip: a refusal on a run somebody should have
// seen is a missing live update they can still get by reloading, while an allow
// on a run they should not have seen is a disclosure with no floor under it,
// because a `subject_type` is free text by design and a future port can
// introduce one at any time without touching this file. Defaulting the unknown
// case to "allow" would make every new subject type a silent widening.
//
// ── THE ORG-WIDE CASE, argued ────────────────────────────────────────────────
//
// A run with NO OWNER AND NO SUBJECT is not an edge case, it is a whole class of
// this system's work: `fitness-sweep`, `rag-reindex` and `rag-backfill` are all
// exactly that shape. Nobody's run, about no ticket, started from Admin. Under
// the original rule the refusal above caught every one of them, so NOBODY —
// not even the admin who pressed the button — could attach to
// /api/runs/:id/events for a fitness sweep or a retrieval migration. The
// durable, attachable, survives-a-deploy progress view existed and had no
// audience. That is a bug, and the widening is deliberate rather than
// convenient.
//
// IT IS NOT "ADMINS MAY READ ANYTHING", which is the mistake the paragraph below
// warns about and which check-invariants watches for elsewhere. Three things
// keep it narrow, and all three are load-bearing:
//
//   · BOTH halves must be absent. A run with an owner stays that person's
//     (an admin watching somebody's research run is refused here, exactly as it
//     was), and a run with a subject is still governed by that subject's read
//     ACL (an admin with no membership of a private board still cannot watch
//     its ticket's work session).
//   · Ownerless-and-subjectless is not a gap in the ACL, it is a COMPLETE
//     description: there is no owner to ask and no subject to resolve, and the
//     three definitions of that shape all declare `audience: () => ({ by:
//     'admin' })` independently. This makes the watch gate agree with what the
//     definitions already say instead of contradicting them.
//   · What is disclosed is the run's `phase` — "rebuilding rag_docs (3 of 7)",
//     "tier 2: 41 of 70 cases" — which is operational text about the
//     deployment's own infrastructure, and which the same admin already reads
//     synchronously from /api/admin/rag and /api/admin/model-fitness. This is
//     the live version of a page they can already open, not a new one.
//
// The gate still is NOT the definition's `audience`; see the next paragraph.
// This is a rule about a SHAPE OF ROW, decided from the row alone, so it keeps
// working for a `kind` this instance has never registered — which is the whole
// reason `audience` cannot be used here.
//
// IT IS NOT THE DEFINITION'S `audience`. That answers "who may DECIDE when
// this pauses" and it is resolved, correctly and in one place, by
// `audienceFor`. This answers "who may WATCH", it runs on an unauthenticated-
// until-proven request, and it must be answerable for a run whose `kind` is not
// registered on this instance at all (a row enqueued by a newer deploy). Those
// are different questions; conflating them would give a run with a
// `{ by: 'admin' }` audience a stream every admin could attach to, which is
// precisely the "being an admin is not a read grant" mistake check-invariants
// already watches for elsewhere.

/** Why a watch was refused. Reported rather than collapsed to a boolean so the
 *  unknown-subject case is distinguishable in a log — it is the one that means
 *  THIS FILE IS OUT OF DATE, not that the caller did anything wrong. */
export type RunWatchVerdict =
  | { ok: true }
  | { ok: false; reason: 'missing' | 'not-audience' | 'unknown-subject' }

/** Every edge to the outside world, so the predicate is testable with no
 *  database — the same rule server/harness/run.ts follows. */
export interface RunWatchDeps {
  getRun: (id: string) => Promise<Pick<RunRow, 'ownerUserId' | 'subjectType' | 'subjectId'> | null>
  boardRole: (userId: string, boardId: string) => Promise<unknown>
  channelRole: (userId: string, channelId: string) => Promise<unknown>
  taskBoardId: (taskId: string) => Promise<string | null>
  conversationAccess: (userId: string, conversationId: string) => Promise<unknown>
  /** ONLY consulted for the org-wide shape (no owner, no subject). Kept as a
   *  dep rather than inlined so the widening is visible in every test's fake and
   *  so a future caller cannot reach admin-ness through this file by accident. */
  isAdmin: (userId: string) => Promise<boolean>
}

/** The real ones, every import DEFERRED to the call.
 *
 *  Not a style choice: `tasks.ts` and `channels.ts` both import THIS module to
 *  publish, so a static import back into them is a load-time cycle, and the
 *  half-initialized-module bug that comes out of one shows up as an undefined
 *  function in an unrelated request. Deferring also keeps the many modules that
 *  import this file only to call `publishBoard` from dragging the task, board,
 *  channel, conversation and runs modules into their graph. */
const REAL_WATCH_DEPS: RunWatchDeps = {
  getRun: async (id) => (await import('./runs/store')).pgRunStore.get(id),
  boardRole: async (userId, boardId) => (await import('./boards')).boardRole(userId, boardId),
  channelRole: async (userId, channelId) => (await import('./channels')).channelRole(userId, channelId),
  taskBoardId: async (taskId) => (await import('./tasks')).getTask(taskId).then((t) => t?.boardId ?? null),
  conversationAccess: async (userId, conversationId) =>
    (await import('./conversations')).accessibleConversation(userId, conversationId),
  isAdmin: async (userId) => (await (await import('./users')).getUserRole(userId)) === 'admin',
}

/** May this person attach to `run:<id>`? */
export async function mayWatchRun(
  userId: string,
  runId: string,
  deps: RunWatchDeps = REAL_WATCH_DEPS,
): Promise<RunWatchVerdict> {
  const run = await deps.getRun(runId)
  // A run that is not there and a run you may not see are the same answer to a
  // caller, on purpose: 404-vs-403 on a guessable id is an existence oracle,
  // and the board and channel event routes already collapse the two.
  if (!run) return { ok: false, reason: 'missing' }
  if (run.ownerUserId && run.ownerUserId === userId) return { ok: true }

  const { subjectType, subjectId } = run

  // ORG-WIDE WORK: nobody's run, about nothing in particular — a fitness sweep,
  // a retrieval migration. See the argument above the deps: both halves must be
  // absent, and the disclosure is a phase line about the deployment's own
  // infrastructure. A run with an owner, or with a subject, never reaches here.
  if (!run.ownerUserId && !subjectType) return (await deps.isAdmin(userId)) ? { ok: true } : { ok: false, reason: 'not-audience' }

  // Everything else needs a subject to resolve. A run with an OWNER who is not
  // this caller falls through to here and is refused unless its subject admits
  // them — sharing a run is done by sharing the thing it is about.
  if (!subjectType || !subjectId) return { ok: false, reason: 'not-audience' }

  switch (subjectType) {
    case 'board':
      return (await deps.boardRole(userId, subjectId)) ? { ok: true } : { ok: false, reason: 'not-audience' }
    case 'task': {
      // A ticket's read ACL is its BOARD's, which is why this hop exists rather
      // than a `task_members` check that does not exist.
      const boardId = await deps.taskBoardId(subjectId)
      if (!boardId) return { ok: false, reason: 'not-audience' }
      return (await deps.boardRole(userId, boardId)) ? { ok: true } : { ok: false, reason: 'not-audience' }
    }
    case 'channel':
      return (await deps.channelRole(userId, subjectId)) ? { ok: true } : { ok: false, reason: 'not-audience' }
    case 'conversation':
      // `accessibleConversation`, not `ownedConversation`: a PLAN admits its
      // collaborators and a chat does not, and that distinction is already made
      // in one place. Re-deciding it here would be a second answer.
      return (await deps.conversationAccess(userId, subjectId)) ? { ok: true } : { ok: false, reason: 'not-audience' }
    default:
      // LOUD, because this is a gap in this file and not a misbehaving caller.
      // `subject_type` is free text so a port can add one without a migration;
      // the cost of that freedom is that a new subject type must be taught here
      // or its runs have no live view. Refusing keeps the failure visible and
      // recoverable (reload works); allowing would make it invisible and not.
      console.warn(
        `[realtime] refusing run watch: no read predicate for subject_type "${subjectType}" (run ${runId}). Teach mayWatchRun about it.`,
      )
      return { ok: false, reason: 'unknown-subject' }
  }
}

function topicEventStream(channel: string, signal: AbortSignal): ReadableStream<Uint8Array> {
  const enc = new TextEncoder()
  const sub = new Redis(process.env.REDIS_URL!, { maxRetriesPerRequest: 3 })

  return new ReadableStream<Uint8Array>({
    start(controller) {
      const send = (s: string) => {
        try {
          controller.enqueue(enc.encode(s))
        } catch {
          /* stream closed */
        }
      }
      send(': connected\n\n')
      // Swallow connect/subscribe failures (e.g. the client aborts before the
      // subscriber finishes connecting) — an unhandled rejection or 'error'
      // event here would take down the whole server process.
      sub.on('error', () => {})
      sub.subscribe(channel).catch(() => {})
      sub.on('message', (_ch, msg) => send(`data: ${msg}\n\n`))
      const ping = setInterval(() => send(': ping\n\n'), 25_000)

      const cleanup = () => {
        clearInterval(ping)
        sub.disconnect()
        try {
          controller.close()
        } catch {
          /* already closed */
        }
      }
      if (signal.aborted) cleanup()
      else signal.addEventListener('abort', cleanup, { once: true })
    },
    cancel() {
      sub.disconnect()
    },
  })
}
