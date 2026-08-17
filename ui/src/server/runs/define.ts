// THE run contract. A long action DECLARES how it makes one unit of progress;
// it never owns its own durability, its own retry policy, its own "is this
// still mine" check, or its own answer to "what happens when a person has to
// decide something".
//
// WHY THIS FILE EXISTS
//   Long-running work in this tree is durable by accident. Two places got it
//   right and neither generalizes: routes/api/chat.ts tees the upstream into a
//   DETACHED persist so the write completes after the client disconnects, and
//   server/scheduler.ts holds a Redis lease per job so two instances never
//   double-run shared-state work. Everything else re-invented a piece and got a
//   different piece wrong:
//
//     server/research.ts:352      a restart mid-run MARKS THE RUN FAILED —
//                                 'run went stale (app restarted mid-research?)'
//                                 is not a diagnosis, it is a shrug written into
//                                 a user's report
//     server/fitness/surface.ts   a status blob in app_settings and a runner
//     server/retrieval/migrate.ts that is a bare `void fn()`; a deploy leaves
//                                 state:'running' forever with nothing driving
//                                 it and nothing that will ever notice
//     server/work-dispatch.ts:51  `liveSessions`, a process-local Set carrying
//                                 an explicit TODO(multi-instance) — a restart
//                                 drops a live work session silently
//     chat-persist 'continuing', inbox-focus-conversation 'locks',
//     price-oracle 'inFlight'     three more process-local guards, each wrong
//                                 the moment there are two instances
//
//   Every one of those is the same missing noun. This is the noun: a run is a
//   SERVER-OWNED RECORD that survives a tab close, a view change, a restart and
//   a deploy; that PAUSES AND ASKS when it needs a human decision rather than
//   guessing or grinding on; and that any device can attach to and see.
//
// A RUN IS A SEQUENCE OF STEPS OVER A CHECKPOINT, and that is the whole reason
// resume means anything. Resuming is re-entering `step()` with the last
// PERSISTED checkpoint — not replaying the run from zero, which is what every
// "just start it again" retry in this codebase actually does today.
//
// AT-LEAST-ONCE. Say it out loud, because every author of a `step` has to hold
// it: a reclaimed run RE-ENTERS `step()`. If a step archived a chat, sent a DM,
// opened a PR or billed a model call and the process died before its checkpoint
// was persisted, the retry DOES IT AGAIN. Persist before the side effect
// wherever the ordering allows; where it does not, say so in a comment at the
// step, because the guard is then owed by the step and not by the driver.
//
// PURE BY CONSTRUCTION, like harness/define.ts: this module is types, one
// identity function and a registry Map. It imports no database, no Redis and no
// clock, so a definition can be written — and the whole registry enumerated —
// without booting Talaria.
import type { Authority } from '../approvals'

/** The lifecycle. Six states, and the new one is the point.
 *
 *  `awaiting` is a run PARKED on a human decision: not failed (nothing is
 *  wrong), not running (nothing is burning), not queued (no amount of driving
 *  advances it). Today that state does not exist anywhere in the product, so
 *  every long action facing a decision it cannot make either guesses, grinds,
 *  or dies — and the fitness runner and the retrieval migration both express
 *  "somebody has to look at this" as `running`, forever. */
export type RunState = 'queued' | 'running' | 'awaiting' | 'done' | 'error' | 'cancelled'

/** One row of the `runs` table, as every consumer of it reads it.
 *
 *  Timestamps are ISO strings rather than Dates: this row is published over SSE
 *  and rendered on a device that never talked to Postgres, and a type that only
 *  survives a JSON round trip in one direction is a type two surfaces will
 *  disagree about. */
export interface RunRow {
  id: string
  /** The definition that drives it — the key into the registry below. */
  kind: string
  /** Whose run it is, for "my active runs". Null for org-wide work with no one
   *  person behind it (a fitness sweep, a retrieval migration). */
  ownerUserId: string | null
  /** What the run is ABOUT, so a surface can attach a run to the thing it acts
   *  on without every kind inventing its own foreign key: 'task', 'channel',
   *  'conversation', 'research', 'board'. Free text on purpose — a check
   *  constraint here would make every port a migration. */
  subjectType: string | null
  subjectId: string | null
  state: RunState
  /** Human-readable progress, written by `ctx.log(phase)`. This is the sentence
   *  a person reads while they wait; it is not a state machine and nothing may
   *  branch on it. */
  phase: string
  /** The last PERSISTED checkpoint. `null` before the first `next`. */
  checkpoint: unknown
  input: unknown
  result: unknown
  error: string | null
  /** How many times a driver has ENTERED this run. Incremented on RECLAIM only
   *  — see the note on `maxAttempts`. */
  attempt: number
  /** The driver token that currently owns it, and until when. Mirrored from the
   *  Redis lease so the reclaim sweep is a plain indexed SQL scan: Redis has no
   *  index over "every run whose lease expired", and asking it for one would
   *  mean keeping a second set of every in-flight run id. */
  leaseOwner: string | null
  leaseExpiresAt: string | null
  /** The approvals key while this run is `awaiting`, so the existing announce
   *  and nag machinery in server/approvals.ts can dedupe on it exactly the way
   *  it dedupes every other pending decision. Null in every other state. */
  approvalKey: string | null
  /** The question this run is parked on, and the answer once somebody gives it.
   *
   *  IT IS A COLUMN AND NOT A CLOSURE. A decision that lives only in the
   *  process that asked it is the whole disease: park a run on one instance,
   *  open the approval on your phone, and the question is gone. `awaiting` is
   *  only worth having if the question survives the process that raised it. */
  decision: RunDecision | null
  createdAt: string
  updatedAt: string
  startedAt: string | null
  finishedAt: string | null
}

/** One thing a person may pick. `id` is what comes back in the answer and what
 *  the step branches on, so it is stable and machine-readable; `label` is the
 *  button. */
export interface DecisionOption {
  id: string
  label: string
  /** The consequence, in one line. Shown under the button. */
  detail?: string
}

/** What a run needs a human to decide.
 *
 *  Deliberately shaped like `PendingApproval` (title / detail / href) rather
 *  than like a form: the whole product already has one place where a person is
 *  asked something and one set of rules about who may be told what it says.
 *  A run that invented a second one would be a second inbox. */
export interface DecisionRequest {
  /** Stable WITHIN the run, and re-askable: a step that returns `decide` again
   *  with the same key after a reclaim must be understood as the same question,
   *  not a new one. This is what makes the pause idempotent under at-least-once
   *  delivery, which nothing else in the pause path can be. */
  key: string
  /** One line. Goes in a notification title and a list item. */
  question: string
  /** What the reader needs in order to decide, in one sentence. */
  detail?: string
  options: DecisionOption[]
  /** In-app path to the surface that can actually decide it. */
  href?: string
}

export interface DecisionAnswer {
  /** The `DecisionRequest.key` this answers. Checked on the way in: an answer
   *  to last week's question must not resume a run parked on this week's. */
  key: string
  optionId: string
  note?: string
  /** Null when the answer came from something other than a person (a policy, a
   *  timeout rule) — which is allowed, and is exactly the thing an audit needs
   *  to be able to tell apart from a human having looked. */
  answeredBy: string | null
  answeredAt: string
}

/** The `decision` column: the question, and the answer when there is one. */
export interface RunDecision {
  request: DecisionRequest
  answer: DecisionAnswer | null
}

/** What one call to `step` can say. Four outcomes, and the difference between
 *  the last two is the difference between bothering a person and not. */
export type StepResult<C> =
  /** Progress. The driver PERSISTS this checkpoint and then continues; a step
   *  that returns `next` with the checkpoint it was given is a step that will
   *  be re-entered with the same state, i.e. a loop, and the driver cannot tell
   *  that apart from real progress. */
  | { kind: 'next'; checkpoint: C; phase?: string }
  | { kind: 'done'; result?: unknown }
  /** PAUSE. The run parks in `awaiting`, an approval is filed under
   *  `approval_key`, and whoever `audience` names is told. Nothing burns and
   *  nothing is lost while it waits. */
  | { kind: 'decide'; question: DecisionRequest }
  /** A SOFT pause: come back in `after` ms. Nobody is notified, no attempt is
   *  consumed, and the run stays `queued` — this is "the rate limit says wait",
   *  not "something went wrong". Keeping it distinct from `error` is what stops
   *  a throttled run from burning its attempts on the throttle. */
  | { kind: 'retry'; after: number; reason: string }

/** Everything a step is given. Note what is NOT here: no database handle, no
 *  Redis, no way to write its own row. A step advances a checkpoint and does
 *  its own domain work; the driver owns persistence, and a step that reached
 *  around it would be re-inventing the file it is running inside. */
export interface RunStepContext<I, C> {
  /** The row as it was read at THIS step boundary — already re-read, so
   *  `run.state` is fresh and a cancellation is visible here. */
  run: RunRow
  input: I
  /** The last PERSISTED checkpoint; null on the first step of a run. Not "the
   *  checkpoint I returned last time" — if the last step returned `next` and
   *  the write did not land, this is the one before it, and the step runs
   *  again. That is the at-least-once contract, stated in a type. */
  checkpoint: C | null
  /** The answer to the question this run was parked on, if it was parked and
   *  has been answered. Cleared once the step that consumed it checkpoints, so
   *  a later step cannot mistake a stale answer for a fresh one. */
  decision: DecisionAnswer | null
  /** Aborted when the run exceeds `maxStepMs`, when the driver loses its lease,
   *  and when the process is shutting down. A step that ignores it is a step
   *  the driver has to abandon while it is still running — see the timeout
   *  branch in run.ts, which says what that costs. */
  signal: AbortSignal
  /** Say what is happening, in words a waiting human would accept. Persisted
   *  and published (in that order — see run.ts); safe to call as often as makes
   *  sense, and the driver awaits the tail of it at the next step boundary so a
   *  log can never outrun the checkpoint it describes. */
  log: (phase: string) => void
  /** Which entry into this run this is. 0 on the first; incremented on each
   *  RECLAIM. A step that must not repeat a side effect can look at this and
   *  refuse — that is not a substitute for a real guard, but it is enough to
   *  say "I have been here before" out loud. */
  attempt: number
}

export interface RunDefinition<I, C> {
  /** The registry key, and the `kind` column. Stable forever: it is written
   *  into every row this definition has ever produced, and a rename orphans
   *  every one of them mid-flight. */
  kind: string
  /** What a person sees this called. */
  label: string
  /** ONE UNIT OF PROGRESS, called with the LAST PERSISTED CHECKPOINT.
   *
   *  IT MUST BE RE-ENTERABLE. A step that ran and did not persist WILL run
   *  again — after a crash, after a deploy, after a lease expiry on a paused
   *  container. If it archived a chat, sent a DM, opened a PR or billed a model
   *  call, the retry does that a second time. Persist the checkpoint before the
   *  side effect wherever the ordering allows; where it does not, put the
   *  reason in a comment at the step so the guard is owed somewhere visible. */
  step: (ctx: RunStepContext<I, C>) => Promise<StepResult<C>>
  /** Who may SEE and DECIDE this run when it pauses — an approvals `Authority`,
   *  the same discriminator every "who may be told about this thing" question
   *  in the product already answers with. Not a new access model: a run whose
   *  audience was hand-rolled would be the fourth place that decides who may
   *  read a ticket's contents. */
  audience: (run: RunRow) => Authority
  /** How long ONE step may take before the driver abandons it. A budget, not a
   *  timeout knob: it is also the lease TTL, so a crashed driver's run is
   *  reclaimable roughly this long after it stops renewing. */
  maxStepMs: number
  /** How many times a driver may ENTER this run before it is given up on and
   *  filed as an error.
   *
   *  IT COUNTS ENTRIES, NOT STEPS, and that is deliberate: a healthy run that
   *  takes four hundred steps must not exhaust its attempts by succeeding. The
   *  counter moves only on RECLAIM — i.e. only when a previous driver died
   *  holding this run — so it measures exactly the thing worth bounding, which
   *  is "this run kills the process that touches it". Defaults to
   *  DEFAULT_MAX_ATTEMPTS. */
  maxAttempts?: number
}

// ── AT-LEAST-ONCE: WHAT A STEP AUTHOR OWES ───────────────────────────────────
//
// This is the checklist for anyone porting real work onto the runtime, and it
// is here rather than in a document because the person who needs it is the
// person writing `step` above. The driver's ordering rule (persist, then the
// next step; persist, then publish) makes the repeat window as small as one
// database write and CANNOT make it zero. Every entry below is a place where a
// step could be entered twice with the same checkpoint.
//
//   1. THE STEP THAT RAN AND DID NOT CHECKPOINT. The core case. A step returns
//      `next`, the process dies before `store.checkpoint` lands, the reclaim
//      sweep re-enters `step()` with the PREVIOUS checkpoint. Everything the
//      step did happens again. Split work so that one step does ONE outward
//      effect and checkpoints immediately, and where the effect cannot be
//      undone, get an idempotency handle from the far side (a provider request
//      id, a git ref, a message id) INTO the checkpoint before you use it.
//
//   2. THE STEP THAT WAS ABANDONED WHILE STILL RUNNING. `maxStepMs` and a lost
//      lease both abort by REJECTING the race, not by stopping the step —
//      nothing can stop a promise that ignores `ctx.signal`. So a step can be
//      re-entered on another instance while the original is still in flight.
//      Honor `ctx.signal` before every outward call, and treat `ctx.attempt > 0`
//      as "somebody may have got further than the checkpoint says".
//
//   3. THE PARK. `decide` writes the row, publishes, and then announces. A
//      crash between the park and the announcement leaves an UNMARKED approval
//      key that the ordinary approvals sweep announces later, which is the
//      correct failure — but a step that also sent its own message alongside
//      the question would send that message twice. Do not notify from a step;
//      the question IS the notification.
//
//   4. THE RE-ASK. A reclaimed run re-enters the step, which asks again. The
//      approval key is derived from the run and `DecisionRequest.key`, so the
//      SAME key dedupes and nobody is paged twice. The corollary is a real
//      hazard: a genuinely NEW question reusing an old `key` inherits the old
//      announcement mark and nobody is told. Vary the key per question.
//
//   5. THE ANSWER. `ctx.decision` is cleared in the same write as the
//      checkpoint the consuming step produced — but only if that step
//      checkpoints. A step that acts on a decision and returns `retry`, or
//      throws, will be handed the same answer again. Consume a decision and
//      checkpoint in the same step.
//
//   6. THE ENQUEUE. `enqueue` writes the row and starts a detached drive. A
//      caller that retries its own request enqueues a SECOND run — the runtime
//      deduplicates nothing above the row. Pass a deterministic `opts.id`
//      derived from the subject when "one run per thing" is the rule.
//
//   7. THE TERMINAL WRITE. `complete` and `fail` are compare-and-set on the
//      lease, so they cannot double-write the row — but anything a step does
//      AFTER its last outward effect and BEFORE returning `done` can repeat if
//      the process dies in between. The last step should be small.
//
// The three kinds queued for the porting phase, and the effect each owes a
// guard: research.ts (a billed model call per section — carry the provider
// request id in the checkpoint), fitness/surface.ts (a billed probe per model,
// and an app_settings blob that is not the run's own row), work-dispatch.ts (a
// pushed branch and an opened PR — check for the existing ref before creating).

/** Three attempts: the crash, the retry, and one more for the deploy that
 *  happened to land in the middle of the retry. A fourth entry into a run that
 *  has killed three drivers is a bug report, not a retry. */
export const DEFAULT_MAX_ATTEMPTS = 3

/** A definition with its type parameters erased — what the registry and the
 *  driver hold, since `drive(runId)` learns the kind from a row and cannot know
 *  the input or checkpoint types at that point. Every read of `input` and
 *  `checkpoint` inside the driver is `unknown` for exactly that reason: the
 *  types are the STEP AUTHOR's guarantee about their own column, and the driver
 *  moves those columns without ever looking inside them. */
export type AnyRunDefinition = RunDefinition<unknown, unknown>

/** Identity, for inference and for one obvious place to put the shape.
 *  `defineHarness` is the same function for the same reason. */
export function defineRun<I, C>(def: RunDefinition<I, C>): RunDefinition<I, C> {
  return def
}

// ── The registry ─────────────────────────────────────────────────────────────
//
// `drive(runId)` reads a row, gets a `kind`, and needs the code that advances
// it. The alternative — passing the definition in at every drive site — puts
// the reclaim sweep in the impossible position of having to import every
// definition in the product in order to recover a row it found by scanning.

const g = globalThis as unknown as { __talariaRunDefs?: Map<string, AnyRunDefinition> }
const registry: Map<string, AnyRunDefinition> = (g.__talariaRunDefs ??= new Map())

/** Declare a run kind. Called at module load by the module that owns the work,
 *  so the definition lives next to the thing it drives and the import is what
 *  puts it in the runtime graph — same rule as `registerJob`. */
export function registerRun<I, C>(def: RunDefinition<I, C>): RunDefinition<I, C> {
  const existing = registry.get(def.kind)
  if (existing && existing !== (def as unknown as AnyRunDefinition)) {
    // Two modules claiming one kind is a real bug and a quietly replaced
    // definition is the worst version of it: rows written by the first one get
    // driven by the second one's `step`, with a checkpoint shaped for neither.
    // Keep the first, say so loudly.
    console.error(`[runs] kind "${def.kind}" registered twice — keeping the first registration. This is a bug.`)
    return existing as unknown as RunDefinition<I, C>
  }
  registry.set(def.kind, def as unknown as AnyRunDefinition)
  return def
}

/** The definition for a row's `kind`, or null when nothing in this process
 *  knows how to advance it — which is a REAL state, not an impossible one: a
 *  row enqueued by a newer deploy, or a kind whose module has not been imported
 *  on this instance. The driver reports it rather than failing the run, because
 *  an instance that cannot drive a run is not the same as a run that cannot be
 *  driven. */
export function runDefinition(kind: string): AnyRunDefinition | null {
  return registry.get(kind) ?? null
}

/** Every registered kind. For an admin surface and for the cross-check that
 *  every persisted `kind` still has code behind it. */
export function runDefinitions(): AnyRunDefinition[] {
  return [...registry.values()]
}

// ── Small shared helpers ─────────────────────────────────────────────────────

/** A run is FINISHED when nothing will move it again without a person. */
export function isTerminal(state: RunState): boolean {
  return state === 'done' || state === 'error' || state === 'cancelled'
}

/** A run a driver may pick up. `awaiting` is deliberately NOT drivable: the
 *  thing it is waiting for is an answer, and driving it would re-ask the
 *  question it is already parked on. */
export function isDrivable(state: RunState): boolean {
  return state === 'queued' || state === 'running'
}
