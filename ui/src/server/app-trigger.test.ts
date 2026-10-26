// The host's half of an app trigger (app-trigger.ts): the api's body read,
// the event typed, and a run's outcome — the handler's line, its throw, the
// deadline, a handler that is not there, an app that is off.
import { describe, expect, it } from 'vitest'
import type { AppTriggerContext } from '@/sdk/server'
import { eventOf, parseTriggerBody, runAppTrigger, type TriggerRunBody } from './app-trigger'
import { appInScope } from './app-scope'

const cron = (over: Partial<TriggerRunBody> = {}): TriggerRunBody => ({
  runId: '11111111-1111-4111-8111-111111111111',
  kind: 'cron',
  dueAt: '2026-10-26T03:00:00.000Z',
  event: null,
  payload: {},
  deadlineMs: 5_000,
  ...over,
})

const serving = (triggers: Record<string, (ctx: AppTriggerContext) => unknown>) => ({
  enabled: async () => ['crm'],
  load: async () => ({ default: { triggers } }),
})

describe('parseTriggerBody', () => {
  it('reads what the api sends, and caps the deadline', () => {
    const r = parseTriggerBody({ runId: 'r1', kind: 'event', event: 'ticket.created', payload: { ticketId: 't' }, deadlineMs: 99 * 60 * 60_000 })
    expect(r).toEqual({
      ok: true,
      body: { runId: 'r1', kind: 'event', dueAt: null, event: 'ticket.created', payload: { ticketId: 't' }, deadlineMs: 60 * 60_000 },
    })
  })
  it('refuses a body that is not a run', () => {
    expect(parseTriggerBody(null)).toMatchObject({ ok: false })
    expect(parseTriggerBody({ kind: 'cron' })).toEqual({ ok: false, error: 'runId is required' })
    expect(parseTriggerBody({ runId: 'r', kind: 'webhook' })).toEqual({ ok: false, error: 'kind must be cron or event' })
  })
})

describe('eventOf', () => {
  it('types the two events from their ids, and nothing else', () => {
    expect(eventOf('ticket.created', { ticketId: 't1', boardId: 'b1', title: 'leaked?' })).toEqual({ name: 'ticket.created', ticketId: 't1', boardId: 'b1' })
    expect(eventOf('ticket.moved', { ticketId: 't1', boardId: 'b1', from: 'todo', to: 'done', closed: true })).toEqual({
      name: 'ticket.moved',
      ticketId: 't1',
      boardId: 'b1',
      from: 'todo',
      to: 'done',
      closed: true,
    })
    expect(eventOf('ticket.deleted', {})).toBeNull()
  })
})

describe('runAppTrigger', () => {
  it("answers ok with the handler's own line, and hands it the app's context with no user", async () => {
    let seen: AppTriggerContext | null = null
    const out = await runAppTrigger(
      'crm',
      'nightly',
      cron(),
      undefined,
      serving({
        nightly: (ctx) => {
          seen = ctx
          return '  scored 3 leads  '
        },
      }),
    )
    expect(out).toEqual({ status: 200, body: { ok: true, detail: 'scored 3 leads' } })
    const ctx = seen as unknown as AppTriggerContext & Record<string, unknown>
    expect(ctx.app).toBe('crm')
    expect(ctx.trigger).toBe('nightly')
    expect(ctx.cause).toEqual({ kind: 'cron', dueAt: '2026-10-26T03:00:00.000Z' })
    expect('user' in ctx || 'api' in ctx || 'access' in ctx).toBe(false)
    expect(typeof ctx.agents.turn).toBe('function')
  })

  it("runs the handler as the app, with a harness runner that may pin no agent", async () => {
    let scope: string | null = null
    let ctx: AppTriggerContext | null = null
    await runAppTrigger(
      'crm',
      'nightly',
      cron(),
      undefined,
      serving({
        nightly: async (c) => {
          await Promise.resolve()
          scope = appInScope()
          ctx = c
        },
      }),
    )
    // A model call the handler makes goes to the relay on THIS app's key.
    expect(scope).toBe('crm')
    const got = ctx as unknown as AppTriggerContext
    expect(typeof got.harness.run).toBe('function')
    expect(typeof got.harness.stream).toBe('function')
  })

  it('types an event cause for the handler', async () => {
    let cause: unknown = null
    await runAppTrigger(
      'crm',
      'on-move',
      cron({ kind: 'event', dueAt: null, event: 'ticket.moved', payload: { ticketId: 't', boardId: 'b', from: 'a', to: 'done', closed: true } }),
      undefined,
      serving({ 'on-move': (ctx) => void (cause = ctx.cause) }),
    )
    expect(cause).toEqual({ kind: 'event', event: { name: 'ticket.moved', ticketId: 't', boardId: 'b', from: 'a', to: 'done', closed: true } })
  })

  it('records a throw as a failed run, not a host error', async () => {
    const out = await runAppTrigger(
      'crm',
      'nightly',
      cron(),
      undefined,
      serving({
        nightly: () => {
          throw new Error('the CRM is down')
        },
      }),
    )
    expect(out).toEqual({ status: 200, body: { ok: false, error: 'the CRM is down' } })
  })

  it('answers at the deadline for a handler that ignores its signal', async () => {
    let aborted = false
    const out = await runAppTrigger(
      'crm',
      'nightly',
      cron({ deadlineMs: 30 }),
      undefined,
      serving({
        nightly: (ctx) =>
          new Promise<void>((resolve) => {
            ctx.signal.addEventListener('abort', () => (aborted = true))
            setTimeout(resolve, 2_000)
          }),
      }),
    )
    expect(out.body.ok).toBe(false)
    expect(out.body.error).toContain('did not finish within')
    expect(aborted).toBe(true)
  })

  it('names a trigger the server has no handler for', async () => {
    const out = await runAppTrigger('crm', 'weekly', cron(), undefined, serving({ nightly: () => {} }))
    expect(out.status).toBe(404)
    expect(out.body.error).toContain('no handler for the trigger "weekly"')
  })

  it('runs nothing for an app that is not enabled', async () => {
    let ran = false
    const out = await runAppTrigger('crm', 'nightly', cron(), undefined, {
      enabled: async () => [],
      load: async () => ({ default: { triggers: { nightly: () => void (ran = true) } } }),
    })
    expect(out).toEqual({ status: 409, body: { ok: false, error: 'app "crm" is not enabled' } })
    expect(ran).toBe(false)
  })
})
