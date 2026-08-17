// THE `runs` table, and nothing else.
//
// Separated from the driver for one reason that is not tidiness: every write in
// here is a COMPARE-AND-SET on `(id, lease_owner, state)`, and those predicates
// are the entire correctness argument of the runs system. A driver that could
// write `set state = 'done'` without the `where lease_owner = $token and state
// = 'running'` clause would be a driver that finishes a run another instance is
// already advancing, and a cancellation that only the running instance honors
// (which is exactly what server/fitness/surface.ts's module-level stop flag is
// today). Holding them in one file, in one shape, is how they stay reviewable.
//
// THE PREDICATE IS ALSO THE CANCELLATION CHECK. Because every write requires
// `state = 'running'`, a run that any instance set to `cancelled` rejects the
// next write from its driver automatically — no polling, no flag, no
// "whichever process happens to be running it". The driver still re-reads the
// row at each step boundary so it can stop BEFORE burning another step, but
// even a driver that did not could not finish a cancelled run.
//
// Every method is on the `RunStore` interface, defaulted to the Postgres
// implementation and overridable per call, so run.test.ts drives the whole
// runner with no database. See harness/run.ts for the house pattern.
import { db } from '../db/pg'
import { isTerminal } from './define'
import type { DecisionAnswer, RunDecision, RunRow, RunState } from './define'

/** Why a compare-and-set write did not land. Never "it failed" — every one of
 *  these means something different to the driver, and collapsing them is how a
 *  clean handover gets logged as an error. */
export type WriteFailure =
  /** Another instance owns this run now. A CLEAN STOP, not a fault. */
  | { ok: false; reason: 'lease-lost'; state: RunState }
  /** Somebody cancelled it while the step was running. Honor it. */
  | { ok: false; reason: 'cancelled' }
  /** The row is gone (the owner's account was deleted mid-run, say). */
  | { ok: false; reason: 'missing' }
  /** The row is no longer `running` and not cancelled — another driver parked
   *  or finished it. Also a clean stop. */
  | { ok: false; reason: 'state'; state: RunState }

export type WriteResult = { ok: true } | WriteFailure

export type ClaimResult =
  | {
      ok: true
      run: RunRow
      /** This run was RECLAIMED from a driver that stopped renewing — i.e. a
       *  crash, a deploy, or a container paused past its lease. `attempt` has
       *  already been incremented on the row. */
      reclaimed: boolean
    }
  | { ok: false; reason: 'missing' }
  /** Held by a live lease, or deferred by a `retry` whose wait has not elapsed.
   *  Not an error at either call site: the sweep will find it again. */
  | { ok: false; reason: 'taken'; state: RunState; until: string | null }
  | { ok: false; reason: 'not-runnable'; state: RunState }

export interface NewRun {
  id: string
  kind: string
  ownerUserId: string | null
  subjectType: string | null
  subjectId: string | null
  input: unknown
  phase: string
}

export interface RunStore {
  insert(row: NewRun): Promise<RunRow>
  get(id: string): Promise<RunRow | null>
  /** Take the run, if it is takeable. Atomic: the state flip, the lease stamp
   *  and the attempt increment are one statement, because a claim split into a
   *  read and a write is a claim two instances can both win. */
  claim(args: { id: string; token: string; leaseMs: number }): Promise<ClaimResult>
  /** Push the lease out while a step is still going. */
  heartbeat(args: { id: string; token: string; leaseMs: number }): Promise<WriteResult>
  /** Progress. The ONE write that must land before its side effect is visible
   *  anywhere — see the ordering rule in run.ts. */
  checkpoint(args: { id: string; token: string; checkpoint: unknown; phase: string; clearDecision: boolean }): Promise<WriteResult>
  /** Words only. Split from `checkpoint` so `ctx.log` cannot accidentally
   *  persist a checkpoint the step has not returned yet. */
  phase(args: { id: string; token: string; phase: string }): Promise<WriteResult>
  complete(args: { id: string; token: string; result: unknown }): Promise<WriteResult>
  fail(args: { id: string; token: string; error: string }): Promise<WriteResult>
  /** Park on a human decision. */
  park(args: { id: string; token: string; decision: RunDecision; approvalKey: string; phase: string }): Promise<WriteResult>
  /** Soft pause: back to `queued`, but not takeable until `until`. The lease
   *  stamp IS the wait — there is no `next_attempt_at` column because that is
   *  precisely what a lease expiry already means, and two columns describing
   *  one instant is two columns that will disagree. */
  defer(args: { id: string; token: string; until: number; reason: string }): Promise<WriteResult>
  /** Give the lease back without changing state, so the next driver does not
   *  have to wait out the TTL. */
  release(args: { id: string; token: string }): Promise<void>
  /** Answer the question a run is parked on and put it back in the queue.
   *  Callable by any instance — the person answering is on whichever one their
   *  request landed on, which is never reliably the one that asked. */
  answer(args: { id: string; answer: DecisionAnswer }): Promise<{ ok: true; run: RunRow } | { ok: false; reason: 'missing' | 'not-awaiting' | 'stale-key'; state?: RunState }>
  /** Cancel from ANYWHERE. The honorable stop the fitness runner does not have:
   *  its stop is a module-level boolean that only works on the instance that
   *  happens to be running the sweep. */
  cancel(args: { id: string; reason?: string }): Promise<{ ok: true; state: RunState } | { ok: false; reason: 'missing' | 'terminal'; state?: RunState }>
  /** Runs whose driver stopped renewing, oldest first. THE reclaim query. */
  due(args: { limit: number }): Promise<RunRow[]>
  /** "This user's active runs" — the other real query. */
  activeFor(args: { userId: string; limit?: number }): Promise<RunRow[]>
}

// ── The Postgres implementation ──────────────────────────────────────────────

/** Selected explicitly and aliased, rather than `select *`, so adding a column
 *  to the table cannot silently change the shape every consumer reads. */
const COLS = `
  id,
  kind,
  owner_user_id as "ownerUserId",
  subject_type as "subjectType",
  subject_id as "subjectId",
  state,
  phase,
  checkpoint,
  input,
  result,
  error,
  attempt,
  lease_owner as "leaseOwner",
  lease_expires_at as "leaseExpiresAt",
  approval_key as "approvalKey",
  decision,
  created_at as "createdAt",
  updated_at as "updatedAt",
  started_at as "startedAt",
  finished_at as "finishedAt"
`

/** postgres.js hands timestamps back as Dates. The row is published over SSE
 *  and rendered on a device that never talked to Postgres, so it leaves this
 *  file as ISO strings — see the note on `RunRow`. */
type RawRow = Omit<RunRow, 'createdAt' | 'updatedAt' | 'startedAt' | 'finishedAt' | 'leaseExpiresAt'> & {
  createdAt: Date | string
  updatedAt: Date | string
  startedAt: Date | string | null
  finishedAt: Date | string | null
  leaseExpiresAt: Date | string | null
}

const iso = (v: Date | string | null): string | null => (v === null ? null : v instanceof Date ? v.toISOString() : new Date(v).toISOString())

function hydrate(raw: RawRow): RunRow {
  return {
    ...raw,
    createdAt: iso(raw.createdAt) ?? '',
    updatedAt: iso(raw.updatedAt) ?? '',
    startedAt: iso(raw.startedAt),
    finishedAt: iso(raw.finishedAt),
    leaseExpiresAt: iso(raw.leaseExpiresAt),
  }
}

/** Explain a compare-and-set that matched nothing. Costs one extra read, and
 *  only on the path where something already went sideways — the alternative is
 *  a driver that logs 'write failed' for a clean handover, a cancellation and a
 *  deleted row alike, which is the exact silence this project is eliminating. */
async function why(id: string, token: string): Promise<WriteFailure> {
  const run = await pgRunStore.get(id)
  if (!run) return { ok: false, reason: 'missing' }
  if (run.state === 'cancelled') return { ok: false, reason: 'cancelled' }
  if (run.leaseOwner !== token) return { ok: false, reason: 'lease-lost', state: run.state }
  return { ok: false, reason: 'state', state: run.state }
}

export const pgRunStore: RunStore = {
  async insert(row) {
    const sql = await db()
    const rows = (await sql`
      insert into runs (id, kind, owner_user_id, subject_type, subject_id, state, phase, input)
      values (${row.id}, ${row.kind}, ${row.ownerUserId}, ${row.subjectType}, ${row.subjectId}, 'queued', ${row.phase}, ${sql.json(row.input as never)})
      returning ${sql.unsafe(COLS)}
    `) as unknown as RawRow[]
    const first = rows[0]
    if (!first) throw new Error(`[runs] insert of ${row.kind} returned no row`)
    return hydrate(first)
  },

  async get(id) {
    const sql = await db()
    const rows = (await sql`select ${sql.unsafe(COLS)} from runs where id = ${id}`) as unknown as RawRow[]
    const first = rows[0]
    return first ? hydrate(first) : null
  },

  async claim({ id, token, leaseMs }) {
    const sql = await db()
    // ONE statement. The predicate says exactly what "takeable" means:
    //   · queued, and no live lease on it (a fresh run, or one deferred by a
    //     `retry` whose wait has elapsed)
    //   · running, and the lease has EXPIRED — the driver that had it is gone.
    //     That, and only that, is a reclaim, and it is the only branch that
    //     touches `attempt`. A healthy run taking four hundred steps never
    //     passes through here again, so it cannot exhaust `maxAttempts` by
    //     succeeding.
    // `started_at` is stamped once and never moved: it is when the WORK began,
    // not when this driver picked it up, and a resumed run whose start time
    // jumps forward is a run whose age no queue-depth graph can read.
    //
    // The CTE is not decoration. `returning` reads the NEW row, so the state
    // this run was in BEFORE the flip — the one thing that distinguishes a
    // reclaim from a first pickup — is unreadable from a plain update. The
    // `for update` row lock also serializes two instances claiming the same run
    // in the same millisecond, so exactly one of them sees `queued` and the
    // other sees a live lease.
    const rows = (await sql`
      with prev as (
        select id as pid, state as prev_state from runs where id = ${id} for update
      )
      update runs set
        state = 'running',
        attempt = runs.attempt + case when prev.prev_state = 'running' then 1 else 0 end,
        lease_owner = ${token},
        lease_expires_at = now() + make_interval(secs => ${Math.max(1, leaseMs) / 1000}),
        started_at = coalesce(runs.started_at, now()),
        updated_at = now()
      from prev
      where runs.id = prev.pid
        and prev.prev_state in ('queued', 'running')
        and (runs.lease_expires_at is null or runs.lease_expires_at <= now())
      returning ${sql.unsafe(COLS)}, (prev.prev_state = 'running') as "wasRunning"
    `) as unknown as Array<RawRow & { wasRunning: boolean }>
    const first = rows[0]
    if (first) return { ok: true, run: hydrate(first), reclaimed: first.wasRunning }
    const current = await pgRunStore.get(id)
    if (!current) return { ok: false, reason: 'missing' }
    if (current.state === 'queued' || current.state === 'running')
      return { ok: false, reason: 'taken', state: current.state, until: current.leaseExpiresAt }
    return { ok: false, reason: 'not-runnable', state: current.state }
  },

  async heartbeat({ id, token, leaseMs }) {
    const sql = await db()
    const rows = (await sql`
      update runs set lease_expires_at = now() + make_interval(secs => ${Math.max(1, leaseMs) / 1000})
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    // Deliberately does NOT touch `updated_at`: a heartbeat is not progress,
    // and "my active runs, most recently updated first" must not be reordered
    // by a run that has done nothing for an hour but is still breathing.
    return rows[0] ? { ok: true } : why(id, token)
  },

  async checkpoint({ id, token, checkpoint, phase, clearDecision }) {
    const sql = await db()
    const rows = (await sql`
      update runs set
        checkpoint = ${sql.json(checkpoint as never)},
        phase = ${phase},
        decision = case when ${clearDecision} then null else decision end,
        approval_key = case when ${clearDecision} then null else approval_key end,
        updated_at = now()
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    return rows[0] ? { ok: true } : why(id, token)
  },

  async phase({ id, token, phase }) {
    const sql = await db()
    const rows = (await sql`
      update runs set phase = ${phase}, updated_at = now()
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    return rows[0] ? { ok: true } : why(id, token)
  },

  async complete({ id, token, result }) {
    const sql = await db()
    // The lease is dropped in the same statement that finishes the run. Two
    // statements would leave a window where a done run still looks leased, and
    // the reclaim query would have to special-case terminal states forever.
    const rows = (await sql`
      update runs set
        state = 'done', result = ${sql.json(result as never)}, error = null,
        decision = null, approval_key = null,
        lease_owner = null, lease_expires_at = null,
        finished_at = now(), updated_at = now()
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    return rows[0] ? { ok: true } : why(id, token)
  },

  async fail({ id, token, error }) {
    const sql = await db()
    const rows = (await sql`
      update runs set
        state = 'error', error = ${error.slice(0, 4000)},
        lease_owner = null, lease_expires_at = null,
        finished_at = now(), updated_at = now()
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    return rows[0] ? { ok: true } : why(id, token)
  },

  async park({ id, token, decision, approvalKey, phase }) {
    const sql = await db()
    // The lease is released here too. An `awaiting` run is not being driven by
    // anybody, and a lease held across a wait for a human would either expire
    // (making the row look reclaimable when it is not) or have to be renewed by
    // a process with nothing to do.
    const rows = (await sql`
      update runs set
        state = 'awaiting', decision = ${sql.json(decision as never)}, approval_key = ${approvalKey},
        phase = ${phase}, lease_owner = null, lease_expires_at = null, updated_at = now()
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    return rows[0] ? { ok: true } : why(id, token)
  },

  async defer({ id, token, until, reason }) {
    const sql = await db()
    // Back to `queued`, but the lease stamp stays in the FUTURE, which is what
    // makes the wait real: `claim` refuses anything with a live lease, so no
    // instance takes this run before `until` — including this one. The lease
    // OWNER stays set as well, so the row says who deferred it.
    const rows = (await sql`
      update runs set
        state = 'queued', phase = ${reason},
        lease_expires_at = to_timestamp(${Math.floor(until) / 1000}),
        updated_at = now()
      where id = ${id} and lease_owner = ${token} and state = 'running'
      returning id
    `) as unknown as Array<{ id: string }>
    return rows[0] ? { ok: true } : why(id, token)
  },

  async release({ id, token }) {
    const sql = await db()
    // Only from `running`: a run this driver parked, deferred or finished has
    // already had its lease dealt with, and clearing it again would strip the
    // deferral wait off a `retry`.
    await sql`
      update runs set lease_owner = null, lease_expires_at = null
      where id = ${id} and lease_owner = ${token} and state = 'running'
    `
  },

  async answer({ id, answer }) {
    const sql = await db()
    const current = await pgRunStore.get(id)
    if (!current) return { ok: false, reason: 'missing' }
    if (current.state !== 'awaiting') return { ok: false, reason: 'not-awaiting', state: current.state }
    // An answer names the question it answers. Without this check, an answer
    // submitted from a stale tab — the run having since been re-parked on a
    // DIFFERENT question — would resume the run with the wrong decision, and
    // the step would read it as an answer to the question it is actually
    // waiting on. Two people, two devices, one run: this is not hypothetical.
    if (current.decision?.request.key !== answer.key) return { ok: false, reason: 'stale-key', state: current.state }
    const decision: RunDecision = { request: current.decision.request, answer }
    const rows = (await sql`
      update runs set
        state = 'queued', decision = ${sql.json(decision as never)},
        lease_owner = null, lease_expires_at = null, updated_at = now()
      where id = ${id} and state = 'awaiting'
      returning ${sql.unsafe(COLS)}
    `) as unknown as RawRow[]
    const first = rows[0]
    // Lost a race with another answer of the same question. Not an error worth
    // raising to the person: somebody answered it, which is what they wanted.
    if (!first) return { ok: false, reason: 'not-awaiting' }
    return { ok: true, run: hydrate(first) }
  },

  async cancel({ id, reason }) {
    const sql = await db()
    // NO LEASE PREDICATE, and that is the entire point. Cancellation must be
    // honorable by any instance, including one that has never touched this run
    // — the current fitness stop is a module-level flag that only works on the
    // process that happens to be running the sweep, so a cancel from the other
    // instance does nothing and the button lies. The driver that owns the run
    // finds out at its next step boundary, or when its next write is refused.
    const rows = (await sql`
      update runs set
        state = 'cancelled', error = ${reason ?? null},
        lease_owner = null, lease_expires_at = null,
        finished_at = now(), updated_at = now()
      where id = ${id} and state in ('queued', 'running', 'awaiting')
      returning state
    `) as unknown as Array<{ state: RunState }>
    const first = rows[0]
    if (first) return { ok: true, state: first.state }
    const current = await pgRunStore.get(id)
    if (!current) return { ok: false, reason: 'missing' }
    return { ok: false, reason: 'terminal', state: current.state }
  },

  async due({ limit }) {
    const sql = await db()
    // THE reclaim query, and one of the two the indexes exist for. Oldest
    // expiry first so a queue that has fallen behind drains in the order it
    // fell behind, rather than starving whatever has been waiting longest.
    const rows = (await sql`
      select ${sql.unsafe(COLS)} from runs
      where state in ('queued', 'running')
        and (lease_expires_at is null or lease_expires_at <= now())
      order by lease_expires_at asc nulls first, created_at asc
      limit ${Math.max(1, limit)}
    `) as unknown as RawRow[]
    return rows.map(hydrate)
  },

  async activeFor({ userId, limit }) {
    const sql = await db()
    // The other real query: what this person has in flight, newest activity
    // first. `awaiting` is in the list because a run parked on a question the
    // user has to answer is the MOST active thing they have.
    const rows = (await sql`
      select ${sql.unsafe(COLS)} from runs
      where owner_user_id = ${userId} and state in ('queued', 'running', 'awaiting')
      order by updated_at desc
      limit ${Math.max(1, limit ?? 50)}
    `) as unknown as RawRow[]
    return rows.map(hydrate)
  },
}

// ── The latest run of a KIND ─────────────────────────────────────────────────
//
// The third real query, and it lives here rather than beside any one definition
// because it is what EVERY ownerless run needs: `due` answers "what is nobody
// driving", `activeFor` answers "what has this person got in flight", and this
// answers "what is the admin panel showing". A fitness sweep, a retrieval
// backfill and a reindex are all nobody's run about nothing, so the kind is the
// only handle they have.
//
// IT WAS EXPORTED FROM `runs/defs/reindex.ts` and imported from there by
// `fitness/surface.ts` — one spelling in the wrong home, and an expensive one:
// it made the model-fitness surface import the retrieval definitions module,
// which drags Qdrant, the embedder and every indexer into a graph that has no
// business with them. Same function, same behavior, correct file.
//
// PLAIN FUNCTIONS RATHER THAN `RunStore` METHODS, deliberately. `RunStore` is
// the compare-and-set surface the DRIVER writes through, and every method on it
// is faked in run.test.ts; a read that no driver performs does not belong in
// that contract, and adding it would make every fake store owe an
// implementation of something it never calls.
//
// NOT INDEXED YET: `runs` has indexes for the reclaim scan and for a user's
// active runs, and none on `kind`. Fine at today's volumes (a handful of rows
// per kind) and the first thing to add — `runs (kind, created_at desc)` — when
// the history grows.

/** The projection an org-wide status panel reads. Narrow on purpose: the
 *  panels need these nine columns, and handing back the row shape the driver
 *  uses would invite a caller to write through it. */
export interface KindRunView {
  id: string
  state: RunState
  phase: string
  input: unknown
  checkpoint: unknown
  result: unknown
  error: string | null
  startedAt: string | null
  finishedAt: string | null
}

export async function latestRunOfKind(kind: string): Promise<KindRunView | null> {
  const sql = await db()
  const rows = (await sql`
    select id, state, phase, input, checkpoint, result, error,
           started_at as "startedAt", finished_at as "finishedAt"
    from runs where kind = ${kind}
    order by created_at desc limit 1
  `) as unknown as Array<Omit<KindRunView, 'startedAt' | 'finishedAt'> & { startedAt: Date | string | null; finishedAt: Date | string | null }>
  const first = rows[0]
  return first ? { ...first, startedAt: iso(first.startedAt), finishedAt: iso(first.finishedAt) } : null
}

/** The run this kind is currently doing, or null. `awaiting` counts as active —
 *  a run parked on a question is not finished, however long it sits. */
export async function activeRunOfKind(kind: string): Promise<KindRunView | null> {
  const latest = await latestRunOfKind(kind)
  return latest && !isTerminal(latest.state) ? latest : null
}
