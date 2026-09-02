// boot-health — the bridge between server-entry.js's migration boot step and
// the healthz probe. The properties that matter: silence when the pass is
// fine or merely slow (the check appears only when it has something to say),
// and a failing check carrying nothing but a safe short code when it isn't.
import { afterEach, describe, expect, it } from 'vitest'
import { bootMigrationCheck } from './boot-health'

const g = globalThis as { __talariaBootMigrationError?: { code: string; at: number } }
afterEach(() => {
  delete g.__talariaBootMigrationError
})

describe('bootMigrationCheck', () => {
  it('stays silent when the boot pass has nothing to report', () => {
    expect(bootMigrationCheck()).toBeNull()
  })

  it('fails with the recorded code when the boot pass died', () => {
    g.__talariaBootMigrationError = { code: 'MIGRATION_FAILED', at: Date.now() }
    expect(bootMigrationCheck()).toEqual({ ok: false, latencyMs: null, error: 'MIGRATION_FAILED' })
  })

  it('carries only the code — never a driver message', () => {
    g.__talariaBootMigrationError = { code: '42602', at: Date.now() }
    const check = bootMigrationCheck()
    expect(Object.keys(check ?? {}).sort()).toEqual(['error', 'latencyMs', 'ok'])
  })
})
