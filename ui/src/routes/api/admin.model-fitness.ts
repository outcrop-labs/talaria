// Admin → Models → Fitness, over HTTP. THIS FILE IS PLUMBING: the admin gate,
// the query string, the zod body, the audit line, the status code. Every
// decision — what a capability tag says across a pooled endpoint set, what a
// run will cost, what the archive keeps and evicts, what the drill-down shows —
// lives in `server/fitness/surface.ts`.
//
// WHY THE SPLIT. `vitest.config.ts` excludes `src/routes/**`, because in a
// file-based router a dot is a path separator and `routes/api/foo.test.ts` is
// the handler for POST /api/foo/test rather than a suite. Nothing under routes/
// can be unit tested, so the house rule written at the top of that config is:
// parse the request, call ONE function in `src/server/*`, serialize the result.
// This route had ~920 lines of untestable decisions in it — `mergeFact` alone
// decides whether an admin sees a capability tag at all — and now has none.
//
// The types the client imports are re-exported here so `components/models/
// fitness.ts` keeps one import site for the payload contract.
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { actorOf, parseBody, requireAdmin } from '@/server/api-guard'
import { logAudit } from '@/server/audit'
import { clearFitnessResults, forgetModel, readFitness, startFitnessRun, stopFitnessRun } from '@/server/fitness/surface'

export type {
  CapabilityState,
  CapabilityView,
  FitnessIndexEntry,
  FitnessRunStatus,
  ModelRow,
  RunEstimate,
  TierEstimate,
  TierId,
} from '@/server/fitness/surface'

const Post = z.union([
  z.object({
    action: z.literal('start'),
    model: z.string().min(1).max(200),
    tiers: z.array(z.enum(['probes', 'evals', 'adversarial'])).min(1),
    adversaryModel: z.string().max(200).nullish(),
    only: z.array(z.string().max(120)).max(64).optional(),
    restart: z.boolean().optional(),
    /** Re-measure capabilities an earlier run already probed. Off by default —
     *  a probe fact is a property of an endpoint:model and does not go stale. */
    reprobe: z.boolean().optional(),
    /** Cases in flight at once. Clamped server-side; 1 is the old strictly
     *  sequential sweep. */
    concurrency: z.number().int().min(1).max(8).optional(),
    /** Keep the passing cases and re-run only what failed, timed out or could
     *  not be measured. Cheaper than `restart` by two orders of magnitude on a
     *  run that mostly worked. */
    retryFailed: z.boolean().optional(),
    /** Run only fixtures no archived run has answered — the mode for a registry
     *  that has gained fixtures since the model was last tested. Also prunes
     *  verdicts for fixtures that no longer exist. */
    supplement: z.boolean().optional(),
  }),
  // A model stops ONE run; omitted stops every run in flight.
  z.object({ action: z.literal('stop'), model: z.string().max(200).nullish() }),
  z.object({ action: z.literal('forget'), model: z.string().min(1).max(200) }),
  // CLEAR is not FORGET. Forget drops what a model CAN DO (probe facts, paid
  // for once and true until the id is re-pointed); this drops what a RUN FOUND,
  // so a candidate can be swept again from nothing. `model: null` clears every
  // tested candidate.
  z.object({ action: z.literal('clear'), model: z.string().max(200).nullish() }),
])

// GET  ?view=matrix (default) → slots + models + capability facts + cells + runs
//      ?view=capabilities     → models + facts only (the model pickers)
//      ?view=detail&model=    → one archived report + production telemetry
//      ?view=value            → price against performance, over the measured workload
//      ?view=estimate&model=&tiers=&adversary= → what a run will cost
//      ?view=transcripts&model=&run=    → every case of one run, in full (audit)
// POST { action: 'start' | 'stop' | 'forget' }
export const Route = defineApi('/api/admin/model-fitness', {
  GET: async ({ request }) => {
    const gate = await requireAdmin(request)
    if (gate instanceof Response) return gate
    const q = new URL(request.url).searchParams
    const result = await readFitness({
      view: q.get('view') ?? 'matrix',
      model: q.get('model'),
      tiers: q.get('tiers'),
      adversary: q.get('adversary'),
      only: q.get('only'),
      reprobe: q.get('reprobe') === '1',
      run: q.get('run'),
    })
    return result.ok ? json(result.body) : json({ error: result.error }, { status: 400 })
  },

  POST: async ({ request }) => {
    const user = await requireAdmin(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Post)
    if (body instanceof Response) return body
    const actor = actorOf(user)

    if (body.action === 'stop') {
      // `{ stopped, status, runs }` — two things can be running per candidate
      // (the tier-2 sweep and the tier loop) and `stopFitnessRun` signals both.
      const result = await stopFitnessRun(body.model ?? null)
      void logAudit({ actor, action: 'model_fitness.stop', targetType: 'model-fitness', targetId: body.model ?? 'all' })
      return json(result)
    }

    if (body.action === 'clear') {
      const cleared = await clearFitnessResults(body.model ?? null)
      void logAudit({
        actor,
        action: 'model_fitness.clear',
        targetType: 'model-fitness',
        targetId: body.model ?? 'all',
        after: cleared,
      })
      return json(cleared)
    }

    if (body.action === 'forget') {
      const result = await forgetModel(body.model)
      if (!result.ok) return json({ error: result.error }, { status: 400 })
      void logAudit({
        actor,
        action: 'model_fitness.forget',
        targetType: 'model',
        targetId: body.model,
        after: { keys: result.keys, report: result.report },
      })
      return json({ models: result.models, report: result.report })
    }

    const started = await startFitnessRun({
      model: body.model,
      tiers: body.tiers,
      adversaryModel: body.adversaryModel ?? null,
      restart: body.restart ?? false,
      ...(body.reprobe ? { reprobe: true } : {}),
      ...(body.concurrency !== undefined ? { concurrency: body.concurrency } : {}),
      ...(body.retryFailed ? { retryFailed: true } : {}),
      ...(body.supplement ? { supplement: true } : {}),
      ...(body.only?.length ? { only: body.only } : {}),
    })
    if (!started.ok) {
      // 409 means "a run is already in flight, here it is" — the second press of
      // Start shows the run rather than buying a second one. 400 is a request
      // that could never have run.
      return started.reason === 'busy'
        ? json(
            {
              started: false,
              refusal: started.refusal,
              error:
                started.refusal === 'at-capacity'
                  ? `already testing ${started.runs.max} models — stop one, or wait for it to finish`
                  : 'that model is already being tested',
              status: started.status,
              runs: started.runs,
            },
            { status: 409 },
          )
        : json({ error: started.error }, { status: 400 })
    }
    void logAudit({
      actor,
      action: 'model_fitness.start',
      targetType: 'model',
      targetId: body.model,
      after: { tiers: body.tiers, adversary: body.adversaryModel ?? null, reprobe: body.reprobe ?? false },
    })
    return json({ started: true, status: started.status, runs: started.runs })
  },
})
