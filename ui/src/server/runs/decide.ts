// PAUSE AS APPROVAL. The two sides of `awaiting`: `pause` puts a run down on a
// question and tells the people entitled to answer it, `decide` checks that the
// person answering is one of them and puts the run back to work.
//
// WHY THIS IS NOT A NEW CONCEPT
//   A run that needs a human IS an approval. The product already has exactly one
//   answer to "a person owes a decision, who may be told what it says, who is
//   nagged, and what happens when nobody looks" — server/approvals.ts, and the
//   forty lines of header on it are all scar tissue from getting that answer
//   wrong. A "needs input" queue for runs would be a second inbox with a second
//   disclosure rule, and the second one is always the one that leaks: that is
//   literally what happened to `request_repo`, whose census entry was carefully
//   bounded to a board while the verb that raised it mailed 120 characters of an
//   agent's free text to every admin in the workspace.
//
//   So a parked run is `ApprovalKind = 'run_decision'`, gathered by
//   `pendingApprovals`, announced by `announceApproval`, swept by
//   `sweepUnannounced`, aged by the SLA, and routed by the `approval_pending`
//   notify class each user has already configured. This file adds no queue, no
//   notification class and no audience of its own.
//
// THE AUTHORITY BOUNDARY, which is the one thing to get right here:
//
//   THE RUN DECLARES, THIS FILE ENFORCES, approvals.ts RESOLVES.
//   `RunDefinition.audience(run)` returns an approvals `Authority` — and that is
//   the whole extent of a run kind's involvement in access. It does not fetch
//   users, it does not know what a board editor is, and it never sees the list
//   of people its question reached. `runDecisionApproval` turns the row into the
//   same `PendingApproval` the census builds, `audienceFor` resolves it, and
//   `mayDecide` is the predicate — the same three functions the digest and the
//   SLA go through, with no fourth opinion introduced here.
//
//   A DECISION CANNOT ESCALATE WHAT A RUN MAY DO. The answer is DATA:
//   an `optionId` the step itself declared, plus an optional note. Three things
//   enforce that, and each of them is a thing somebody could otherwise smuggle
//   authority through:
//     · the option must be one the STEP offered — an id nobody declared is
//       refused, so a decider cannot hand the step an instruction it never wrote
//       a branch for;
//     · `answeredBy` is recorded for audit and is never consulted for
//       permission — the run goes on doing what its definition allows, under the
//       agent identity and guardrails it already had. A step that read
//       `decision.answeredBy` and did something on that person's behalf would be
//       a privilege escalation with a paper trail, and it is the step's job not
//       to;
//     · nothing here writes to the run's `input`, which is the only field that
//       could widen the work itself.
//
//   A PAUSED RUN CANNOT SELF-RESUME. `store.park` releases the lease and
//   `isDrivable` excludes `awaiting`, so no sweep, no reclaim and no detached
//   drive moves it. The single statement that takes a row out of `awaiting` is
//   `store.answer`, and this file is the only caller that gets to reach it with
//   a person's name attached.
//
// TESTABILITY IS A DESIGN CONSTRAINT, as everywhere else in server/runs: every
// edge — the store, the publisher, the audience resolver, the announcer, the
// registry, the clock — is a field on `DecideDeps` defaulted to the real thing,
// so decide.test.ts drives both halves with no database, no Redis and no clock.
import { announceApproval, audienceFor, mayDecide, runDecisionApproval, type Disclosure, type PendingApproval } from '../approvals'
import { markBriefStale } from '../daily-brief-stale'
import { runDefinition, type AnyRunDefinition, type DecisionAnswer, type DecisionRequest, type RunRow, type RunState } from './define'
import { drive, publishRunEvent, type RunDeps } from './run'
import { pgRunStore, type RunStore } from './store'

const LOG = '[runs]'

const errText = (e: unknown) => (e instanceof Error ? (e.stack ?? e.message) : String(e))

/** The approvals key for a parked run, derived from the run and the question
 *  and nothing else.
 *
 *  STABLE ACROSS RE-ASKS BY CONSTRUCTION, which is what makes the pause
 *  idempotent under at-least-once delivery: a reclaimed run re-enters `step()`,
 *  the step asks the same question again, and the same key comes out — so the
 *  announce marks in server/approvals.ts dedupe it instead of paging somebody
 *  twice about one decision.
 *
 *  THE FLIP SIDE, and the porting phase owes it a thought: a step that answers a
 *  question, carries on, and later asks a GENUINELY NEW question under the SAME
 *  `question.key` produces the same approval key, and may inherit the earlier
 *  announcement mark — nobody is told the second time. Vary the key when the
 *  question is new (`"assignee:round-2"`, `"conflict:<file>"`); reuse it only
 *  when re-asking the same thing. */
export function runApprovalKey(run: Pick<RunRow, 'id' | 'kind'>, questionKey: string): string {
  return `run:${run.kind}:${run.id}:${questionKey}`
}

// ── Deps ─────────────────────────────────────────────────────────────────────

/** Everything either half of this file touches outside itself. `RunDeps` is
 *  extended rather than re-declared because `decide` hands the whole bag to
 *  `answerRun`, and two dep shapes describing one world is how a test ends up
 *  faking the store for one call and hitting Postgres on the next. */
export interface DecideDeps extends Partial<RunDeps> {
  /** The row → approval translation. Defaulted to the census's own builder, so
   *  the thing this file authorizes against is byte-for-byte the thing the
   *  digest, the announcement and the SLA are looking at. */
  approvalFor?: (run: RunRow) => PendingApproval | null
  /** File the approval with the announce machinery. Returns how many people were
   *  reached; 0 is meaningful and is reported, never swallowed. */
  announce?: (approvalKey: string) => Promise<number>
}

// The fan-out is `publishRunEvent` in run.ts — ONE publisher for the whole runs
// system, going through server/realtime.ts's `run:<id>` and `user:<id>` topics
// and dropping the question field by field on the way out. This file had its
// own copy of that function for a round; the driver had a third, publishing the
// per-user event on a topic (`runs:user:<id>`) that no SSE route subscribes to.
// A publisher is not a detail worth two spellings.

// ── pause ────────────────────────────────────────────────────────────────────

export type PauseResult =
  | {
      ok: true
      approvalKey: string
      /** How many people were actually told. ZERO IS A REAL ANSWER and it is not
       *  a failure of the pause: the row is `awaiting` and durable either way.
       *  It means the key was left UNMARKED, so `sweepUnannounced` picks it up
       *  on the next tick — which is the safety net an approval nobody was told
       *  about exists to fall into. */
      announced: number
      /** Who could have been told, from the run's own declared authority. */
      audience: Disclosure
    }
  /** The park did not land, and every reason is a normal one: another instance
   *  owns the run now, somebody cancelled it, the row is gone. The question is
   *  simply not asked; nothing is half-parked. */
  | { ok: false; reason: 'missing' | 'lease-lost' | 'cancelled' | 'state'; state?: RunState }

/** Park a run on a question and file it as an approval.
 *
 *  Callable only by whoever HOLDS THE RUN'S LEASE, and the token is a required
 *  argument for that reason rather than as a formality: `store.park` is a
 *  compare-and-set on `(id, lease_owner, state = 'running')`, so a park without
 *  the lease is a park that races the driver still executing the step. The
 *  driver's own `decide` branch is the ordinary caller; this is the entry point
 *  for work that pauses outside the step loop.
 *
 *  ORDER: persist, then publish, then tell. The row is `awaiting` before anybody
 *  hears it is, and the telling cannot fail the pause — a notification is a
 *  DELIVERY of the record, and a delivery that did not happen must not destroy
 *  the thing it was about.
 *
 *  AT-LEAST-ONCE, FROM THE OTHER SIDE. A process that dies between the park and
 *  the announcement leaves a parked run with an UNMARKED key, and the approvals
 *  sweep announces it on its next tick — a late notification rather than a lost
 *  one. That works only because the mark is written by `announceApproval` after
 *  a delivery lands, never here: a pause that marked a key it had not managed to
 *  send would produce the one failure this system exists to end, a run parked
 *  for ever that nobody was told about and no sweep will look at again. */
export async function pause(
  args: { runId: string; token: string; question: DecisionRequest; phase?: string },
  deps: DecideDeps = {},
): Promise<PauseResult> {
  const store: RunStore = deps.store ?? pgRunStore
  const publish = deps.publish ?? publishRunEvent
  const definitionFor: (kind: string) => AnyRunDefinition | null = deps.definitionFor ?? runDefinition
  const resolve = deps.audienceFor ?? audienceFor
  const announce = deps.announce ?? announceApproval

  const run = await store.get(args.runId)
  if (!run) return { ok: false, reason: 'missing' }

  const approvalKey = runApprovalKey(run, args.question.key)
  const phase = (args.phase ?? run.phase).slice(0, 300)
  const decision = { request: args.question, answer: null }
  const write = await store.park({ id: args.runId, token: args.token, decision, approvalKey, phase })
  if (!write.ok) return { ok: false, reason: write.reason, state: 'state' in write ? write.state : undefined }

  publish({ type: 'run', runId: run.id, kind: run.kind, state: 'awaiting', phase, question: args.question }, run.ownerUserId)

  /** The row as it now IS, rather than as it was read a moment ago: the park
   *  wrote exactly these fields, and handing a definition a row that still says
   *  `running` would be asking it who may decide a run that is not parked. */
  const parked: RunRow = { ...run, state: 'awaiting', decision, approvalKey, phase, leaseOwner: null, leaseExpiresAt: null }

  // The audience is resolved from the DEFINITION's declared authority, on the
  // row as it now stands. Resolved here only so the caller and the log can say
  // how many people could have been told; the announcement itself goes through
  // `announceApproval`, which resolves it again from the census. That is not a
  // wasted round trip, it is the guarantee: nothing in this file may hand
  // `announce` an audience of its own.
  const def = definitionFor(run.kind)
  let audience: Disclosure = { content: [], fact: [] }
  if (def) {
    try {
      audience = await resolve(def.audience(parked))
    } catch (e) {
      console.error(`${LOG} ${run.id} (${run.kind}): could not resolve who may decide "${args.question.question}":`, errText(e))
    }
  }

  let announced = 0
  try {
    announced = await announce(approvalKey)
  } catch (e) {
    console.error(`${LOG} ${run.id} (${run.kind}) parked on "${args.question.question}" but the announcement threw:`, errText(e))
  }
  if (announced === 0)
    // LOUD, because a parked run nobody was told about is the exact silence this
    // whole system exists to end — and it is survivable rather than fatal only
    // because the key is left unmarked for `sweepUnannounced` to find. Say which
    // key, so the gap is a log line somebody can grep and not a mystery.
    console.warn(
      `${LOG} ${run.id} (${run.kind}) is awaiting a decision and nobody was announced to (${audience.content.length} could decide it) — ` +
        `${approvalKey} stays unannounced and UNMARKED, so the approvals sweep will pick it up.`,
    )
  return { ok: true, approvalKey, announced, audience }
}

// ── decide ───────────────────────────────────────────────────────────────────

/** THE `awaiting → queued` write, and the reason it is not exported.
 *
 *  This is the only statement in the system that takes a row out of `awaiting`,
 *  and for one round of this project it lived in run.ts as a public `answerRun`
 *  with no authority check in it — beside `decide()`, which has one. Two doors
 *  into one write, one of them open, makes the gate a convention: a route that
 *  imported the more obvious name would resume somebody else's run on the
 *  strength of a request body. The house rule is the block above `logTicket` in
 *  server/workbench-mcp.ts — an audit line is a write to the ticket like any
 *  other, so it goes THROUGH the one gate rather than around it, and the ungated
 *  form is not something a caller can express.
 *
 *  So: module-private, one caller, and `decide()` above it does the resolving
 *  and the asking. A future non-human answer path (a policy, a timeout rule —
 *  `DecisionAnswer.answeredBy` is nullable for exactly those) does not get to
 *  reuse this door either; it needs its own entry point with its own explicit
 *  statement of what authorized it, because the alternative is that the one
 *  function enforcing "a person may decide this" acquires a way to be called
 *  with no person. */
async function resumeAnswered(
  args: { runId: string; answer: DecisionAnswer; start?: boolean },
  deps: DecideDeps,
): Promise<{ ok: true; run: RunRow } | { ok: false; reason: 'missing' | 'not-awaiting' | 'stale-key'; state?: RunState }> {
  const store: RunStore = deps.store ?? pgRunStore
  const publish = deps.publish ?? publishRunEvent
  // `store.answer` re-checks the question key against the row it writes, which
  // is what actually closes the race with a second decider — the check in
  // `decide` above is against a row read a moment earlier.
  const res = await store.answer({ id: args.runId, answer: args.answer })
  if (!res.ok) return res
  // PERSIST, THEN PUBLISH, as everywhere else: a device told the run is queued
  // before the row says so would refetch and see it still parked.
  publish({ type: 'run', runId: res.run.id, kind: res.run.kind, state: res.run.state, phase: res.run.phase }, res.run.ownerUserId)
  if (args.start !== false)
    void drive(res.run.id, deps).catch((e) => console.error(`${LOG} drive after answering ${res.run.id} threw:`, errText(e)))
  return res
}

export type DecideRefusal =
  /** No such run. */
  | 'missing'
  /** Not parked on anything — already answered, cancelled, or still running.
   *  Two people racing the same question is the common cause and it is not an
   *  error worth showing: somebody answered it, which is what they wanted. */
  | 'not-awaiting'
  /** Answering a question the run is no longer parked on — a stale tab. */
  | 'stale-key'
  /** This person may not decide this run. Deliberately ONE reason for both "you
   *  are not in the audience" and "nobody is": the caller returns 403 either
   *  way, and a route that distinguished them would tell a stranger which runs
   *  exist and who can see them. */
  | 'forbidden'
  /** An option the step never offered. */
  | 'unknown-option'

export type DecideResult =
  | { ok: true; run: RunRow; answer: DecisionAnswer }
  | { ok: false; reason: DecideRefusal; state?: RunState }

/** Answer the question a run is parked on, as a named person, and put it back in
 *  the queue.
 *
 *  Callable from ANY instance — that is the whole reason the question is a
 *  column and not a closure. The person answering is on whichever instance their
 *  request landed on, which is never reliably the one that asked; park a run on
 *  one instance, open the approval on your phone, and this still works.
 *
 *  WHERE THE ANSWER GOES. Into the run's `decision` column, which the driver
 *  hands to the next step as `ctx.decision` and CLEARS in the same write as the
 *  checkpoint that step produces. That is the design's "the answer goes into the
 *  checkpoint", implemented where it is atomic: two writes would leave a window
 *  in which a reclaim hands the next step an answer that has already been acted
 *  on, and it would act on it again. Writing into the checkpoint BLOB instead
 *  was the alternative and it is worse — the checkpoint's shape is the step
 *  author's type parameter `C`, and a decision route reaching into it would be
 *  this file corrupting a value only the step knows how to read. */
export async function decide(
  args: {
    runId: string
    optionId: string
    note?: string
    /** The user id of the person deciding. Required: this function's entire job
     *  is to check that somebody specific is allowed to answer.
     *
     *  `DecisionAnswer.answeredBy` is nullable because an answer can legitimately
     *  come from something other than a person — a policy, a timeout rule — and
     *  an audit has to be able to tell those apart from a human having looked.
     *  Such a path does NOT come through here: it would need its own explicit
     *  authorization argument, and letting it share this door would mean the one
     *  function that enforces "a person may decide this" had a way to be called
     *  with no person. */
    by: string
    /** Resume immediately, detached. Default true; false leaves the run
     *  `queued` for the sweep, which is what a test wants. */
    start?: boolean
  },
  deps: DecideDeps = {},
): Promise<DecideResult> {
  const store: RunStore = deps.store ?? pgRunStore
  const approvalFor = deps.approvalFor ?? runDecisionApproval
  const resolve = deps.audienceFor ?? audienceFor
  const now = deps.now ?? Date.now

  const run = await store.get(args.runId)
  if (!run) return { ok: false, reason: 'missing' }
  const request = run.decision?.request
  if (run.state !== 'awaiting' || !request) return { ok: false, reason: 'not-awaiting', state: run.state }

  // ── The authority check, before anything else is said about the run ────────
  //
  // Ordered deliberately: nothing below this point tells the caller anything
  // about the question — not its options, not whether their answer was
  // well-formed — until they have been established as somebody who may see it.
  const approval = approvalFor(run)
  if (!approval) {
    // We could not build the approval, which means we could not establish who
    // may decide this run: an unregistered kind, a missing approval key, an
    // `audience` that threw. FAIL CLOSED. `runDecisionApproval` has already said
    // which of those it was, loudly.
    console.warn(`${LOG} ${run.id} (${run.kind}): refused a decision from ${args.by} because this instance cannot say who may decide it`)
    return { ok: false, reason: 'forbidden', state: run.state }
  }
  const who: Disclosure = await resolve(approval.authority)
  // THE predicate, not a re-derivation of it. `mayDecide` takes the census shape
  // and asks whether this person is in the CONTENT half — never the `fact` half,
  // which is people who may be told something is stuck and cannot be told what.
  if (!mayDecide({ audience: new Map([[approval.key, who]]) }, approval, args.by))
    return { ok: false, reason: 'forbidden', state: run.state }

  // ── The answer is DATA, and only the data the step offered ─────────────────
  const option = request.options.find((o) => o.id === args.optionId)
  if (!option) {
    console.warn(`${LOG} ${run.id} (${run.kind}): "${args.optionId}" is not one of the options the step offered (${request.options.map((o) => o.id).join(', ')})`)
    return { ok: false, reason: 'unknown-option', state: run.state }
  }

  const answer: DecisionAnswer = {
    // Named from the ROW rather than from the request, so an answer can only
    // ever be about the question the run is parked on right now. `store.answer`
    // checks it again against the row it writes, which is what actually closes
    // the race with a second decider.
    key: request.key,
    optionId: option.id,
    // Free text from a person, clamped. It is a note for the step and for the
    // audit trail — never an instruction that widens what the run may do.
    ...(args.note?.trim() ? { note: args.note.trim().slice(0, 2000) } : {}),
    answeredBy: args.by,
    answeredAt: new Date(now()).toISOString(),
  }

  // One write path back into the queue, publish and resume included — and it is
  // reachable from nowhere but this line, which is what makes the check above
  // an enforcement rather than a habit.
  const res = await resumeAnswered({ runId: args.runId, answer, start: args.start }, deps)
  if (!res.ok) return res

  // THE DECISION IS ALSO A BRIEF EVENT. Everyone in the content audience had
  // this approval on their brief (that is what `announce` put there); the
  // answer resolves those lines. Detached, because the decider is waiting on
  // this response and bookkeeping for other people's pages must not cost them
  // a millisecond — the nudge clears a throttle and the next read does the work.
  void markBriefStale([...new Set([...who.content, args.by])]).catch(() => {})
  return { ok: true, run: res.run, answer }
}

/** What a person is being asked, for a surface that has the run row already.
 *  Null unless the run is parked — a decided or finished run has no question,
 *  and a surface that rendered the last one would be showing a decision that has
 *  already been made as if it were still open. */
export function pendingQuestion(run: RunRow): DecisionRequest | null {
  return run.state === 'awaiting' && run.decision && !run.decision.answer ? run.decision.request : null
}
