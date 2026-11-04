import { describe, expect, it } from 'vitest'
import { isolateApp } from './app-isolate'
import { appInScope } from './app-scope'

describe('isolateApp', () => {
  it('returns the value on success', async () => {
    const r = await isolateApp('x', 'ok', async () => 7)
    expect(r).toEqual({ ok: true, value: 7 })
  })

  it('swallows a throw into a structured failure', async () => {
    const r = await isolateApp('boom', 'fetch', async () => {
      throw new Error('nope')
    })
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error).toContain('nope')
  })

  it('runs the app as itself — the scope its model calls are signed by follows its async work', async () => {
    const r = await isolateApp('crm', 'server', async () => {
      await new Promise((resolve) => setTimeout(resolve, 1))
      return appInScope()
    })
    expect(r).toEqual({ ok: true, value: 'crm' })
    expect(appInScope()).toBeNull()
    // The host's own sweep label is no app.
    expect(await isolateApp('_host', 'sweep', async () => appInScope())).toEqual({ ok: true, value: null })
  })
})
