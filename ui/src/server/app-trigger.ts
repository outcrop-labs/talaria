// Run one of an app's triggers — the host's half of talaria-app-triggers.
//
// The api decides WHEN (a schedule's minute, a platform event) and keeps the
// record; it calls `POST /api/app-host/<slug>/trigger/<id>` with the
// root-derived credential, and this runs the handler the app's server names
// for that id (`defineAppServer({ triggers: { <id>: (ctx) => … } })`) and
// answers how it went. The answer is the run's outcome, not an HTTP status:
// a handler that throws is `{ ok: false, error }`, which the api records as a
// failed run and shows on the app's row in Manage → Apps. A failing trigger
// never turns the app off.
//
// THE CONTEXT IS THE APP'S, with no user: there is no session to call the
// platform API with (no `ctx.api`), no person's grants (no `ctx.access`), and
// `ctx.agents` refuses — a triggered run drives no agent; `ctx.harness` runs
// as the app and may pin no agent. It is built through
// `appServices`, the factory the request and tool contexts use, so whatever
// every context carries reaches this one too. The payload of an event is
// ids, never content (talaria-platform-events).
import type { AppServer, AppTriggerCause, AppTriggerContext, AppTriggerEvent } from '@/sdk/server'
import { appServerModule, enabledAppSlugs } from './apps'
import { appSecretReader, appServices } from './app-context'
import { errorMessageOf } from './app-isolate'
import { withApp } from './app-scope'
import { getAuthConfig } from './auth/config'
import { PLATFORM_EVENTS } from './app-build/manifest'

/** What the api sends (talaria-app-triggers `call_host`). */
export interface TriggerRunBody {
  runId: string
  kind: 'cron' | 'event'
  dueAt: string | null
  event: string | null
  payload: Record<string, unknown>
  deadlineMs: number
}

/** The longest a run may take when the api names no deadline — the api's
 *  own `RUN_DEADLINE`. */
export const DEFAULT_DEADLINE_MS = 10 * 60_000
const MAX_DEADLINE_MS = 60 * 60_000

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)
const str = (v: unknown): string | null => (typeof v === 'string' && v ? v : null)

export function parseTriggerBody(raw: unknown): { ok: true; body: TriggerRunBody } | { ok: false; error: string } {
  if (!isObject(raw)) return { ok: false, error: 'the body must be a JSON object' }
  const runId = str(raw.runId)
  if (!runId) return { ok: false, error: 'runId is required' }
  if (raw.kind !== 'cron' && raw.kind !== 'event') return { ok: false, error: 'kind must be cron or event' }
  const deadline = typeof raw.deadlineMs === 'number' && Number.isFinite(raw.deadlineMs) && raw.deadlineMs > 0 ? raw.deadlineMs : DEFAULT_DEADLINE_MS
  return {
    ok: true,
    body: {
      runId,
      kind: raw.kind,
      dueAt: str(raw.dueAt),
      event: str(raw.event),
      payload: isObject(raw.payload) ? raw.payload : {},
      deadlineMs: Math.min(deadline, MAX_DEADLINE_MS),
    },
  }
}

/** The event as the handler reads it: the name and the ids, typed. Null for
 *  a name this host does not emit. */
export function eventOf(name: string | null, payload: Record<string, unknown>): AppTriggerEvent | null {
  const ticketId = str(payload.ticketId) ?? ''
  const boardId = str(payload.boardId) ?? ''
  if (name === 'ticket.created') return { name, ticketId, boardId }
  if (name === 'ticket.moved') {
    return { name, ticketId, boardId, from: str(payload.from) ?? '', to: str(payload.to) ?? '', closed: payload.closed === true }
  }
  return null
}

/** How a run went, as the route answers it. */
export type TriggerOutcome = { status: number; body: { ok: boolean; detail?: string; error?: string } }

class Cut {}

/** Run `apps/<slug>`'s trigger `id`. `signal` is the api's call: it hanging
 *  up cuts the run as the deadline does. */
export async function runAppTrigger(
  slug: string,
  id: string,
  run: TriggerRunBody,
  signal?: AbortSignal,
  edges: Partial<{ enabled: () => Promise<string[]>; load: (slug: string) => Promise<unknown> }> = {},
): Promise<TriggerOutcome> {
  const enabled = await (edges.enabled ?? enabledAppSlugs)().catch(() => null)
  if (enabled === null) return { status: 503, body: { ok: false, error: 'the enabled apps could not be read' } }
  if (!enabled.includes(slug)) return { status: 409, body: { ok: false, error: `app "${slug}" is not enabled` } }

  let cause: AppTriggerCause
  if (run.kind === 'cron') {
    cause = { kind: 'cron', dueAt: run.dueAt ?? new Date().toISOString() }
  } else {
    const event = eventOf(run.event, run.payload)
    if (!event) {
      return { status: 400, body: { ok: false, error: `"${run.event}" is not a platform event this host knows (${PLATFORM_EVENTS.join(', ')})` } }
    }
    cause = { kind: 'event', event }
  }

  const mod = (await (edges.load ?? appServerModule)(slug).catch((e: unknown) => {
    console.error(`[apps:${slug}] trigger ${id}: the server module did not load`, e)
    return null
  })) as { default?: AppServer } | null
  const handler = mod?.default?.triggers?.[id]
  if (typeof handler !== 'function') {
    return {
      status: 404,
      body: { ok: false, error: `app "${slug}" has no handler for the trigger "${id}" — its server.ts must answer it in defineAppServer({ triggers: { "${id}": … } })` },
    }
  }

  const ctl = new AbortController()
  const timer = setTimeout(
    () => ctl.abort(new DOMException(`the trigger did not finish within ${Math.round(run.deadlineMs / 1000)} s`, 'TimeoutError')),
    run.deadlineMs,
  )
  const callerLeft = () => ctl.abort(signal?.reason ?? new DOMException('the api hung up', 'AbortError'))
  signal?.addEventListener('abort', callerLeft, { once: true })
  // Armed before the handler starts, so a handler that ignores its signal is
  // still answered for at the deadline (it runs on, but nothing it returns
  // reaches the record).
  const cutting = new Promise<never>((_, reject) => ctl.signal.addEventListener('abort', () => reject(new Cut()), { once: true }))
  cutting.catch(() => {})

  const services = appServices(slug, { kind: 'trigger', trigger: id }, ctl.signal)
  const ctx: AppTriggerContext = {
    app: slug,
    trigger: id,
    runId: run.runId,
    cause,
    store: services.store,
    agents: services.agents,
    harness: services.harness,
    secret: appSecretReader(slug, null),
    origin: getAuthConfig().publicUrl,
    signal: ctl.signal,
  }
  try {
    // As the app: every model call the handler makes, at any depth, goes to
    // the relay on this app's key (app-scope.ts).
    const running = Promise.resolve().then(() => withApp(slug, () => handler(ctx)))
    running.catch(() => {}) // a cut run's late rejection is nobody's
    const result = await Promise.race([running, cutting])
    const detail = typeof result === 'string' && result.trim() ? result.trim() : undefined
    return { status: 200, body: { ok: true, ...(detail ? { detail } : {}) } }
  } catch (e) {
    if (e instanceof Cut) {
      const why = ctl.signal.reason instanceof Error ? ctl.signal.reason.message : 'the run was cut off'
      return { status: 200, body: { ok: false, error: `${why}; it may have done part of its work` } }
    }
    console.error(`[apps:${slug}] trigger ${id}`, e)
    return { status: 200, body: { ok: false, error: errorMessageOf(e) } }
  } finally {
    clearTimeout(timer)
    signal?.removeEventListener('abort', callerLeft)
  }
}
