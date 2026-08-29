import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// googleLoginEnabled is the login screen's whole policy: who may flip it (an
// authenticated admin through the UI toggle, or env as a bootstrap pin), and
// what it still requires regardless (a resolvable client). The bug this file
// guards against is the one the alpha found: a client configured and the org
// connected through the Admin UI, with no env flag, rendered no sign-in button
// anywhere — and no UI surface could turn it on.

const state = vi.hoisted(() => ({
  settings: new Map<string, unknown>(),
}))

vi.mock('../audit', () => ({
  getSetting: async (key: string, dflt: unknown) => (state.settings.has(key) ? state.settings.get(key) : dflt),
  setSetting: async (key: string, value: unknown) => {
    state.settings.set(key, value)
  },
}))

vi.mock('../db/pg', () => ({ db: async () => { throw new Error('db is not under test here') } }))

import { googleLoginEnabled, googleLoginPinnedByEnv, setGoogleLoginEnabled } from './client-config'

// resolveGoogleClient falls to env creds when no Admin record is stored — drive
// the "client" half through env so no secretbox key needs arming.
const SAVED = {
  AUTH_GOOGLE_ENABLED: process.env.AUTH_GOOGLE_ENABLED,
  AUTH_GOOGLE_CLIENT_ID: process.env.AUTH_GOOGLE_CLIENT_ID,
  AUTH_GOOGLE_CLIENT_SECRET: process.env.AUTH_GOOGLE_CLIENT_SECRET,
}

beforeEach(() => {
  state.settings.clear()
  delete process.env.AUTH_GOOGLE_ENABLED
  process.env.AUTH_GOOGLE_CLIENT_ID = 'cid'
  process.env.AUTH_GOOGLE_CLIENT_SECRET = 'sec'
})

afterEach(() => {
  for (const [k, v] of Object.entries(SAVED)) {
    if (v === undefined) delete process.env[k]
    else process.env[k] = v
  }
})

describe('googleLoginEnabled', () => {
  it('is off by default even with a client configured — a credential is not consent', async () => {
    expect(await googleLoginEnabled()).toBe(false)
    expect(googleLoginPinnedByEnv()).toBe(false)
  })

  it('turns on with the Admin UI toggle — the client + toggle pair is sufficient', async () => {
    await setGoogleLoginEnabled(true)
    expect(state.settings.get('google_login_enabled')).toBe(true)
    expect(await googleLoginEnabled()).toBe(true)
    expect(googleLoginPinnedByEnv()).toBe(false)
  })

  it('accepts the env flag as a bootstrap that pins login on past the toggle', async () => {
    process.env.AUTH_GOOGLE_ENABLED = '1'
    expect(await googleLoginEnabled()).toBe(true)
    expect(googleLoginPinnedByEnv()).toBe(true)
    await setGoogleLoginEnabled(false)
    expect(await googleLoginEnabled()).toBe(true)
  })

  it('never enables without a resolvable client — the button must work', async () => {
    await setGoogleLoginEnabled(true)
    delete process.env.AUTH_GOOGLE_CLIENT_ID
    delete process.env.AUTH_GOOGLE_CLIENT_SECRET
    expect(await googleLoginEnabled()).toBe(false)
  })
})
