import { describe, expect, it } from 'vitest'
import { isolateApp } from './app-isolate'

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
})
